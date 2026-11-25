//! A client for the MCP servers of plugins that lives between two calls.
//!
//! The claude command starts the servers of a plugin itself. A model behind an HTTP endpoint does not see them, so this
//! module starts the servers and speaks the handshake. One process is kept per server for the life of the session, so a
//! browser, a database session or a counter survives between two tool calls. Every request has a timeout; a server that
//! stops or does not answer is dropped, its process is stopped, and the next call starts it cleanly.
//!
//! A server is allowed only when the plugin or server that owns it says `"allow": true`; that switch is the default for
//! all of its tools. A single tool can be allowed or denied in the same server object:
//! `"tools": {"danger": false, "read": {"allow": true}}`, or with `"allow_tools"` / `"deny_tools"`. A deny rule wins.
//! The process reads `env`, `cwd` and an optional `timeout` in seconds from the server object.
//!
//! Results may hold text, pictures, audio and resources. Text is passed on unchanged. A picture or a binary resource is
//! written to `<root>/mcp/images` or `<root>/mcp/resources` and shown as one short line with its type, its size and its
//! path; base64 never reaches the prompt.

use crate::config::Ctx;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Keeps two saved files apart even when they arrive in the same millisecond.
static NEXT_FILE: AtomicU64 = AtomicU64::new(0);

/// How long the handshake may take. A server object may shorten or lengthen it with `"timeout": SECONDS`.
const INIT_TIMEOUT: Duration = Duration::from_secs(20);
const LIST_TIMEOUT: Duration = Duration::from_secs(30);
const CALL_TIMEOUT: Duration = Duration::from_secs(120);
/// How long a process gets to leave after its stdin is closed before it is killed.
const STOP_WAIT: Duration = Duration::from_millis(800);

#[derive(Debug)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
    /// The full JSON schema. The brief packing reads the property `brief` from it.
    pub input_schema: Value,
}

/// One answer: a result, or a JSON-RPC error from a server that is still alive.
enum Reply {
    Ok(Value),
    Server(String),
}

/// One live server process and the thread that reads its answers.
struct Server {
    name: String,
    /// Command, arguments, env and cwd. A change starts a new process.
    signature: String,
    child: Child,
    input: Option<ChildStdin>,
    rx: Receiver<(u64, Value)>,
    next: u64,
    alive: bool,
    /// The brief extension from `initialize`, if this server announced it.
    extension: Option<crate::brief::Extension>,
    /// `tools/list` as long as this process lives. It is read only when a call carries a brief.
    tools: Option<Vec<Value>>,
}

type Cell = Arc<Mutex<Option<Server>>>;

fn pool() -> &'static Mutex<HashMap<String, Cell>> {
    static POOL: OnceLock<Mutex<HashMap<String, Cell>>> = OnceLock::new();
    POOL.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The key of one server in one session. The root keeps two temporary projects with the same server name apart.
fn pool_key(ctx: &Ctx, name: &str) -> String {
    format!("{}|{}", ctx.root.display(), name)
}

fn cell_for(ctx: &Ctx, name: &str) -> Cell {
    let mut map = pool().lock().unwrap_or_else(|e| e.into_inner());
    map.entry(pool_key(ctx, name)).or_insert_with(|| Arc::new(Mutex::new(None))).clone()
}

/// Everything that decides which process runs. A tool rule or a timeout does not restart a healthy server.
fn signature_of(server: &Value) -> String {
    serde_json::to_string(&json!({
        "command": server["command"],
        "args": server["args"],
        "env": server["env"],
        "cwd": server["cwd"],
    })).unwrap_or_default()
}

fn timeout_of(server: &Value, default: Duration) -> Duration {
    server["timeout"].as_u64().filter(|s| *s > 0).map(|s| Duration::from_secs(s.min(600))).unwrap_or(default)
}

fn spawn(name: &str, server: &Value, signature: String) -> Result<Server, String> {
    let Some(command) = server["command"].as_str().filter(|c| !c.trim().is_empty()) else {
        return Err(format!("MCP server {} has no command (only stdio servers are supported)", name));
    };
    let mut cmd = Command::new(command);
    if let Some(args) = server["args"].as_array() {
        cmd.args(args.iter().filter_map(|a| a.as_str()));
    }
    // Both forms are accepted: an object {"NAME": "VALUE"} and a list ["NAME=VALUE"].
    if let Some(env) = server["env"].as_object() {
        for (k, v) in env {
            match v {
                Value::String(s) => { cmd.env(k, s); }
                Value::Null => {}
                other => { cmd.env(k, other.to_string()); }
            }
        }
    } else if let Some(list) = server["env"].as_array() {
        for item in list.iter().filter_map(|v| v.as_str()) {
            if let Some((k, v)) = item.split_once('=') {
                cmd.env(k.trim(), v);
            }
        }
    }
    if let Some(cwd) = server["cwd"].as_str().map(str::trim).filter(|c| !c.is_empty()) {
        let dir = PathBuf::from(cwd);
        if !dir.is_dir() {
            return Err(format!("MCP server {}: cwd {} is no folder", name, dir.display()));
        }
        cmd.current_dir(dir);
    }
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
    let mut child = cmd.spawn().map_err(|e| format!("cannot start MCP server {} ({}): {}", name, command, e))?;
    let Some(input) = child.stdin.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(format!("MCP server {} has no stdin", name));
    };
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(format!("MCP server {} has no stdout", name));
    };
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            let Ok(v) = serde_json::from_str::<Value>(line.trim()) else { continue };
            let Some(id) = v["id"].as_u64() else { continue };
            if tx.send((id, v)).is_err() {
                break;
            }
        }
    });
    let mut live = Server { name: name.to_string(), signature, child, input: Some(input), rx, next: 0, alive: true,
                             extension: None, tools: None };
    let hello = live.request("initialize", json!({
        "protocolVersion": "2025-06-18", "capabilities": {},
        "clientInfo": {"name": "penelope", "version": env!("CARGO_PKG_VERSION")}
    }), timeout_of(server, INIT_TIMEOUT))?;
    match hello {
        Reply::Ok(value) => live.extension = crate::brief::extension_of(&value),
        Reply::Server(e) => return Err(format!("MCP server {} refused initialize: {}", name, e)),
    }
    live.notify("notifications/initialized", json!({}))?;
    Ok(live)
}

impl Server {
    fn write(&mut self, value: &Value) -> Result<(), String> {
        let Some(input) = self.input.as_mut() else {
            self.alive = false;
            return Err(format!("MCP server {} has no input", self.name));
        };
        if let Err(e) = writeln!(input, "{}", value).and_then(|_| input.flush()) {
            self.alive = false;
            return Err(format!("MCP server {} stopped reading: {}", self.name, e));
        }
        Ok(())
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<(), String> {
        self.write(&json!({"jsonrpc": "2.0", "method": method, "params": params}))
    }

    fn request(&mut self, method: &str, params: Value, wait: Duration) -> Result<Reply, String> {
        if !self.alive {
            return Err(format!("MCP server {} is not running", self.name));
        }
        self.next += 1;
        let id = self.next;
        self.write(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))?;
        let deadline = Instant::now() + wait;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                self.alive = false;
                return Err(format!("MCP server {} did not answer {} within {} s", self.name, method, wait.as_secs()));
            }
            match self.rx.recv_timeout(left) {
                Ok((got, value)) if got == id => {
                    if let Some(e) = value.get("error") {
                        let message = e["message"].as_str().map(str::to_string).unwrap_or_else(|| e.to_string());
                        return Ok(Reply::Server(message));
                    }
                    return Ok(Reply::Ok(value["result"].clone()));
                }
                Ok(_) => continue, // an answer to a request that already timed out
                Err(RecvTimeoutError::Timeout) => {
                    self.alive = false;
                    return Err(format!("MCP server {} did not answer {} within {} s", self.name, method, wait.as_secs()));
                }
                Err(RecvTimeoutError::Disconnected) => {
                    self.alive = false;
                    return Err(format!("MCP server {} stopped before it answered {}", self.name, method));
                }
            }
        }
    }

    /// The tools of this process, read once. Only a call that carries a brief asks for the list.
    fn tools(&mut self, wait: Duration) -> Result<Vec<Value>, String> {
        if let Some(tools) = &self.tools {
            return Ok(tools.clone());
        }
        let result = match self.request("tools/list", json!({}), wait)? {
            Reply::Ok(value) => value,
            Reply::Server(e) => return Err(format!("MCP server {} refused tools/list: {}", self.name, e)),
        };
        let tools = result["tools"].as_array().cloned().unwrap_or_default();
        self.tools = Some(tools.clone());
        Ok(tools)
    }

    /// Closes stdin, gives a well-behaved server a moment to leave, then kills it.
    fn stop(&mut self) {
        self.alive = false;
        if let Some(mut input) = self.input.take() {
            let _ = input.flush();
        }
        let deadline = Instant::now() + STOP_WAIT;
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) => {}
                Err(_) => break,
            }
            if Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Runs one operation on the process of a server, starting it on demand. A transport error drops the process so that
/// the next call starts a clean one; a JSON-RPC error leaves it running.
fn with_server<T>(ctx: &Ctx, name: &str, server: &Value, f: impl FnOnce(&mut Server) -> Result<T, String>) -> Result<T, String> {
    let signature = signature_of(server);
    let cell = cell_for(ctx, name);
    let mut slot = cell.lock().unwrap_or_else(|e| e.into_inner());
    let reusable = slot.as_ref().map(|s| s.alive && s.signature == signature).unwrap_or(false);
    if !reusable {
        *slot = None;
        *slot = Some(spawn(name, server, signature)?);
    }
    let out = f(slot.as_mut().unwrap());
    if !slot.as_ref().map(|s| s.alive).unwrap_or(false) {
        *slot = None;
    }
    out
}

/// Stops every server this session started. Call it when Penelope leaves; a well-behaved server gets to close first.
pub fn shutdown() {
    let cells: Vec<Cell> = {
        let mut map = pool().lock().unwrap_or_else(|e| e.into_inner());
        map.drain().map(|(_, cell)| cell).collect()
    };
    for cell in cells {
        let mut slot = cell.lock().unwrap_or_else(|e| e.into_inner());
        *slot = None;
    }
}

/// The tools of one server. The plugin must be registered; `allow` is not needed to read the list.
pub fn probe(ctx: &Ctx, server: &str) -> Result<Vec<ToolInfo>, String> {
    if !crate::agents::mcp_allowed(server) {
        return Err(format!("NOT listed. This agent run may use the MCP servers: {}.", crate::agents::mcp_scope().unwrap_or_default().join(", ")));
    }
    let (entry, _) = crate::plugins::server_entry(ctx, server).ok_or_else(|| format!("no MCP server named {}", server))?;
    let tools = with_server(ctx, server, &entry, |live| live.tools(timeout_of(&entry, LIST_TIMEOUT)))?;
    Ok(tools.into_iter().map(|t| ToolInfo {
        name: t["name"].as_str().unwrap_or("").to_string(),
        description: t["description"].as_str().unwrap_or("").to_string(),
        input_schema: if t["inputSchema"].is_null() { t["input_schema"].clone() } else { t["inputSchema"].clone() },
    }).collect())
}

/// True when the tool schema names a property `brief`.
fn has_brief_property(tool: &Value) -> bool {
    let schema = if tool["inputSchema"].is_null() { &tool["input_schema"] } else { &tool["inputSchema"] };
    schema["properties"].get("brief").map(|brief| !brief.is_null()).unwrap_or(false)
}

/// Calls one tool of one plugin server. A denied plugin changes nothing.
pub fn call(ctx: &Ctx, server: &str, tool: &str, args: &Value) -> Result<String, String> {
    if !crate::agents::mcp_allowed(server) {
        return Err(format!("NOT called. This agent run may use the MCP servers: {}. Ask the lead to pass {} in the mcp list of the agent.",
                           crate::agents::mcp_scope().unwrap_or_default().join(", "), server));
    }
    if tool.is_empty() {
        return Err("NOT called. Name the tool of the server.".to_string());
    }
    let (entry, allow) = crate::plugins::server_entry(ctx, server).ok_or_else(|| format!("NOT called. no MCP server named {}", server))?;
    if !crate::plugins::tool_allowed(&entry, allow, tool) {
        let hint = format!("/mcp allow-tool {} {} or penelope mcp allow-tool {} {}", server, tool, server, tool);
        return Err(if allow {
            format!("NOT called. The tool {} of MCP server {} is denied. Allow it with {}.", tool, server, hint)
        } else {
            format!("NOT called. The plugin that holds {} is denied, and {} has no allow rule. Allow the plugin with /plugins allow NAME, or this tool with {}.", server, tool, hint)
        });
    }
    let result = with_server(ctx, server, &entry, |live| {
        let mut arguments = args.clone();
        // Only a Laertes server that announced the extension and whose tool names `brief` gets the packed form.
        // Every other server, and every call without a brief, keeps exactly the old path.
        if let Some(ext) = live.extension.clone() {
            let short = arguments.get("brief").filter(|brief| brief.is_object() && brief.get("items").is_none()).cloned();
            if let Some(short) = short {
                let tools = live.tools(timeout_of(&entry, LIST_TIMEOUT))?;
                let wanted = tools.iter().any(|t| t["name"].as_str() == Some(tool) && has_brief_property(t));
                if wanted {
                    let roots = crate::brief::allowed_roots(ctx, server);
                    let packed = crate::brief::pack(&short, &roots, &ext)
                        .map_err(|e| format!("NOT sent. brief: {}", e))?;
                    let crate::brief::Packed { argument, uploads } = packed;
                    if !uploads.is_empty() && !crate::plugins::tool_allowed(&entry, allow, &ext.upload_tool) {
                        return Err(format!("NOT sent. brief upload tool {} is denied for MCP server {}", ext.upload_tool, server));
                    }
                    for upload in uploads {
                        for chunk in crate::brief::chunk_arguments(&upload.id, &upload.data) {
                            match live.request("tools/call", json!({"name": ext.upload_tool, "arguments": chunk}),
                                               timeout_of(&entry, CALL_TIMEOUT))? {
                                Reply::Ok(value) => {
                                    let (text, is_error) = render_result(ctx, &ext.upload_tool, &value);
                                    if is_error {
                                        let reason = if text.trim().is_empty() { "the server answered with an error" } else { text.as_str() };
                                        return Err(format!("NOT sent. brief upload {}: {}", upload.id, reason));
                                    }
                                }
                                Reply::Server(e) => return Err(format!("NOT sent. brief upload {}: {}", upload.id, e)),
                            }
                        }
                    }
                    arguments["brief"] = argument;
                }
            }
        }
        match live.request("tools/call", json!({"name": tool, "arguments": arguments}), timeout_of(&entry, CALL_TIMEOUT))? {
            Reply::Ok(value) => Ok(value),
            Reply::Server(e) => Err(format!("MCP server {} refused {}: {}", server, tool, e)),
        }
    })?;
    let (text, is_error) = render_result(ctx, tool, &result);
    if is_error {
        let text = if text.trim().is_empty() { format!("{} answered with an error", tool) } else { text };
        return Err(format!("NOT done. {}", text));
    }
    Ok(if text.trim().is_empty() { format!("{} answered without text", tool) } else { crate::tools::guard(ctx, &text) })
}

/// One short line for a picture or a sound. The bytes go to a file below the Penelope root; the data never does.
fn save_media(ctx: &Ctx, tool: &str, kind: &str, mime: &str, uri: Option<&str>, data: &[u8], index: usize) -> Option<PathBuf> {
    let dir = ctx.root.join("mcp").join(if kind == "image" { "images" } else { "media" });
    std::fs::create_dir_all(&dir).ok()?;
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let base: String = tool.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).take(40).collect();
    let base = if base.trim_matches('-').is_empty() { kind.to_string() } else { base };
    let sequence = NEXT_FILE.fetch_add(1, Ordering::Relaxed);
    let path = dir.join(format!("{}-{}-{}-{}.{}", base, stamp, sequence, index, extension_for(mime, uri)));
    std::fs::write(&path, data).ok()?;
    Some(path)
}

fn save_resource(ctx: &Ctx, tool: &str, mime: &str, uri: Option<&str>, data: &[u8], index: usize) -> Option<PathBuf> {
    let dir = ctx.root.join("mcp").join("resources");
    std::fs::create_dir_all(&dir).ok()?;
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let base: String = tool.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).take(40).collect();
    let base = if base.trim_matches('-').is_empty() { "resource".to_string() } else { base };
    let sequence = NEXT_FILE.fetch_add(1, Ordering::Relaxed);
    let path = dir.join(format!("{}-{}-{}-{}.{}", base, stamp, sequence, index, extension_for(mime, uri)));
    std::fs::write(&path, data).ok()?;
    Some(path)
}

fn extension_for(mime: &str, uri: Option<&str>) -> String {
    let mime = mime.split(';').next().unwrap_or("").trim().to_lowercase();
    let known = match mime.as_str() {
        "image/png" => Some("png"),
        "image/jpeg" | "image/jpg" => Some("jpg"),
        "image/webp" => Some("webp"),
        "image/gif" => Some("gif"),
        "image/svg+xml" => Some("svg"),
        "image/bmp" => Some("bmp"),
        "audio/mpeg" | "audio/mp3" => Some("mp3"),
        "audio/wav" | "audio/x-wav" => Some("wav"),
        "audio/ogg" => Some("ogg"),
        "application/pdf" => Some("pdf"),
        "application/json" => Some("json"),
        "text/plain" => Some("txt"),
        "text/markdown" => Some("md"),
        _ => None,
    };
    if let Some(ext) = known {
        return ext.to_string();
    }
    if let Some(uri) = uri {
        let clean = uri.split(['?', '#']).next().unwrap_or(uri);
        if let Some(ext) = Path::new(clean).extension().and_then(|e| e.to_str()) {
            let ext = ext.to_lowercase();
            if !ext.is_empty() && ext.len() <= 6 && ext.chars().all(|c| c.is_ascii_alphanumeric()) {
                return ext;
            }
        }
    }
    "bin".to_string()
}

/// Standard base64. Pictures and blobs are decoded so the file is real binary; no crate is added for it.
fn b64_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let mut buffer: u32 = 0;
    let mut bits = 0u32;
    for byte in text.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            b' ' | b'\t' | b'\r' | b'\n' => continue,
            _ => return None,
        } as u32;
        buffer = (buffer << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }
    Some(out)
}

fn base64_size(text: &str) -> usize {
    let clean = text.bytes().filter(|b| !b.is_ascii_whitespace()).count();
    let pad = text.bytes().rev().take_while(|b| *b == b'=').count().min(2);
    (clean / 4).saturating_mul(3).saturating_sub(pad)
}

fn mime_of(block: &Value) -> String {
    block["mimeType"].as_str()
        .or_else(|| block["mime_type"].as_str())
        .or_else(|| block["source"]["media_type"].as_str())
        .or_else(|| block["source"]["mimeType"].as_str())
        .unwrap_or("application/octet-stream")
        .to_string()
}

fn data_of(block: &Value) -> Option<&str> {
    block["data"].as_str().or_else(|| block["source"]["data"].as_str())
}

fn media_line(ctx: &Ctx, tool: &str, kind: &str, block: &Value, index: usize) -> String {
    let mime = mime_of(block);
    let Some(data) = data_of(block) else {
        return format!("[{} {}]", kind, mime);
    };
    let bytes = b64_decode(data);
    let size = bytes.as_ref().map(|b| b.len()).unwrap_or_else(|| base64_size(data));
    match bytes.and_then(|b| save_media(ctx, tool, kind, &mime, None, &b, index).map(|p| (b.len(), p))) {
        Some((size, path)) => format!("[{} {} · {} bytes · saved at {}]", kind, mime, size, path.display()),
        None => format!("[{} {} · {} bytes]", kind, mime, size),
    }
}

fn resource_line(ctx: &Ctx, tool: &str, block: &Value, index: usize) -> String {
    let resource = block.get("resource").unwrap_or(block);
    let uri = resource["uri"].as_str().unwrap_or("");
    let mime = resource["mimeType"].as_str()
        .or_else(|| resource["mime_type"].as_str())
        .unwrap_or("application/octet-stream");
    let head = if uri.is_empty() { format!("[resource {}]", mime) } else { format!("[resource {} · {}]", uri, mime) };
    if let Some(text) = resource["text"].as_str() {
        return format!("{}\n{}", head, text);
    }
    let Some(blob) = resource["blob"].as_str() else {
        return head;
    };
    let bytes = b64_decode(blob);
    let size = bytes.as_ref().map(|b| b.len()).unwrap_or_else(|| base64_size(blob));
    match bytes.and_then(|b| save_resource(ctx, tool, mime, Some(uri), &b, index).map(|p| (b.len(), p))) {
        Some((size, path)) => format!("[resource {} · {} · {} bytes · saved at {}]", uri, mime, size, path.display()),
        None => format!("[resource {} · {} · {} bytes]", uri, mime, size),
    }
}

/// Text stays text. Pictures, sounds and binary resources become a short descriptor and a file; base64 is never passed.
fn render_result(ctx: &Ctx, tool: &str, result: &Value) -> (String, bool) {
    let mut parts: Vec<String> = Vec::new();
    if let Some(content) = result["content"].as_array() {
        for (index, block) in content.iter().enumerate() {
            if let Some(text) = block.as_str() {
                parts.push(text.to_string());
                continue;
            }
            match block["type"].as_str().unwrap_or("") {
                "text" => {
                    if let Some(text) = block["text"].as_str() {
                        parts.push(text.to_string());
                    }
                }
                "image" => parts.push(media_line(ctx, tool, "image", block, index)),
                "audio" => parts.push(media_line(ctx, tool, "audio", block, index)),
                "resource" => parts.push(resource_line(ctx, tool, block, index)),
                "resource_link" => {
                    let uri = block["uri"].as_str().unwrap_or("");
                    let name = block["name"].as_str().unwrap_or("");
                    let mime = block["mimeType"].as_str().or_else(|| block["mime_type"].as_str()).unwrap_or("");
                    let mut line = format!("[resource link {}", uri);
                    if !mime.is_empty() { line.push_str(&format!(" · {}", mime)); }
                    if !name.is_empty() { line.push_str(&format!(" · {}", name)); }
                    line.push(']');
                    parts.push(line);
                }
                other => {
                    if let Some(text) = block["text"].as_str() {
                        parts.push(text.to_string());
                    } else if let Some(uri) = block["uri"].as_str() {
                        parts.push(format!("[{} {}]", if other.is_empty() { "content" } else { other }, uri));
                    } else {
                        parts.push(format!("[{} content]", if other.is_empty() { "unknown" } else { other }));
                    }
                }
            }
        }
    }
    let mut text = parts.into_iter().filter(|p| !p.is_empty()).collect::<Vec<_>>().join("\n");
    if text.trim().is_empty() {
        if let Some(structured) = result.get("structuredContent").filter(|v| !v.is_null()) {
            text = serde_json::to_string(structured).unwrap_or_default();
        }
    }
    (text, result["isError"].as_bool().unwrap_or(false))
}

/// The list of plugin servers, with their tools when `with_tools` is true. At most 8 servers are probed, so one broken
/// server cannot hold the turn forever.
pub fn list(ctx: &Ctx, with_tools: bool) -> String {
    let servers = crate::plugins::servers(ctx);
    let Some(obj) = servers.as_object() else { return "no MCP server is registered. /mcp add NAME COMMAND adds one, or /plugins install LINK.".to_string() };
    if obj.is_empty() {
        return "no MCP server is registered. /mcp add NAME COMMAND adds one, or /plugins install LINK.".to_string();
    }
    let mut out = String::new();
    for (i, (name, _)) in obj.iter().enumerate() {
        if !crate::agents::mcp_allowed(name) {
            continue;
        }
        let Some((entry, allow)) = crate::plugins::server_entry(ctx, name) else { continue };
        let state = if allow {
            "allowed".to_string()
        } else {
            let named = crate::plugins::explicitly_allowed(&entry);
            if named.is_empty() {
                "denied (/mcp allow NAME or /plugins allow NAME)".to_string()
            } else {
                format!("denied, {} by name (/mcp allow-tool NAME TOOL)", if named.len() == 1 { "1 tool allowed".to_string() } else { format!("{} tools allowed", named.len()) })
            }
        };
        out.push_str(&format!("{} · tools {}\n", name, state));
        if with_tools && i < 8 {
            match probe(ctx, name) {
                Ok(tools) => {
                    for t in tools {
                        let denied = !crate::plugins::tool_allowed(&entry, allow, &t.name);
                        let brief = if t.input_schema["properties"].get("brief").is_some() { " · brief" } else { "" };
                        out.push_str(&format!("  {} — {}{}{}\n", t.name, t.description.chars().take(110).collect::<String>(), brief,
                                              if denied { format!(" · denied (/mcp allow-tool {} {})", name, t.name) } else { String::new() }));
                    }
                }
                Err(e) => out.push_str(&format!("  could not list: {}\n", e)),
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("penelope-mcp-{}-{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn server_script(dir: &Path, body: &str) -> PathBuf {
        let script = dir.join("server");
        fs::write(&script, body).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        }
        script
    }

    const STATE_SERVER: &str = r#"#!/usr/bin/env python3
import sys, json, os
count = 0
def answer(m, r):
    print(json.dumps({"jsonrpc": "2.0", "id": m["id"], "result": r}), flush=True)
for line in sys.stdin:
    m = json.loads(line)
    if "id" not in m: continue
    if m["method"] == "initialize":
        answer(m, {"protocolVersion": "2025-06-18", "capabilities": {"tools": {}}, "serverInfo": {"name": "state"}})
    elif m["method"] == "tools/list":
        answer(m, {"tools": [
            {"name": "count", "description": "count the calls in this process"},
            {"name": "where", "description": "read cwd and env"},
            {"name": "media", "description": "text, image and resource content"},
            {"name": "danger", "description": "a tool that can be denied"}
        ]})
    elif m["method"] == "tools/call":
        name = m["params"]["name"]
        if name == "count":
            count += 1
            answer(m, {"content": [{"type": "text", "text": "count:%d pid:%d" % (count, os.getpid())}]})
        elif name == "where":
            answer(m, {"content": [{"type": "text", "text": "cwd:%s env:%s" % (os.getcwd(), os.environ.get("PENELOPE_TEST_ENV", "unset"))}]})
        elif name == "danger":
            answer(m, {"content": [{"type": "text", "text": "danger ran"}]})
        elif name == "media":
            answer(m, {"content": [
                {"type": "text", "text": "the text"},
                {"type": "image", "mimeType": "image/png", "data": "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII="},
                {"type": "resource", "resource": {"uri": "file:///note.txt", "mimeType": "text/plain", "text": "resource text"}},
                {"type": "resource", "resource": {"uri": "file:///blob.bin", "mimeType": "application/octet-stream", "blob": "aGVsbG8="}},
                {"type": "resource_link", "uri": "file:///linked.txt", "name": "linked", "mimeType": "text/plain"}
            ]})
        else:
            answer(m, {"content": [{"type": "text", "text": "unknown " + name}], "isError": True})
    else:
        answer(m, {})
"#;

    const BRIEF_SERVER: &str = r#"#!/usr/bin/env python3
import sys, json, os, base64, hashlib
EXT = "io.github.laertesai/brief"
upload_tool = "" if os.environ.get("BRIEF_NO_UPLOAD") else "brief_put"
ext = {"version": 1, "kinds": ["image", "file", "context", "link"], "maxInlineBytes": 262144,
       "maxItemBytes": 4194304, "maxItems": 16, "ways": ["text", "data", "upload", "uri"],
       "uploadTool": upload_tool}
uploads = {}
def answer(m, r):
    print(json.dumps({"jsonrpc": "2.0", "id": m["id"], "result": r}), flush=True)
for line in sys.stdin:
    m = json.loads(line)
    if "id" not in m:
        continue
    method = m["method"]
    if method == "initialize":
        caps = {"tools": {}}
        if not os.environ.get("BRIEF_NO_EXT"):
            caps["experimental"] = {EXT: ext}
        answer(m, {"protocolVersion": "2025-06-18", "capabilities": caps, "serverInfo": {"name": "brief"}})
    elif method == "tools/list":
        answer(m, {"tools": [
            {"name": "check", "description": "one brief",
             "inputSchema": {"type": "object", "properties": {"brief": {"type": "object", "properties": {
                 "task": {"type": "string"}, "attach": {"type": "array", "items": {"type": "string"}},
                 "items": {"type": "array"}}}}, "required": ["brief"]}},
            {"name": "plain", "description": "no brief",
             "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}}}
        ]})
    elif method == "tools/call":
        name = m["params"].get("name")
        a = m["params"].get("arguments") or {}
        if name == "brief_put":
            uid = a["id"]
            slot = uploads.setdefault(uid, {"seq": 0, "data": b""})
            if a["seq"] != slot["seq"]:
                answer(m, {"content": [{"type": "text", "text": "out of order"}], "isError": True})
            else:
                slot["data"] += base64.b64decode(a["data"])
                slot["seq"] += 1
                answer(m, {"content": [{"type": "text", "text": "upload %s: %d bytes%s" % (
                    uid, len(slot["data"]), ", complete" if a["last"] else "")}]})
        elif name == "check":
            b = a.get("brief", {})
            seen = []
            for it in b.get("items", []):
                if "upload" in it:
                    data = uploads.get(it["upload"], {"data": b""})["data"]
                    seen.append("upload:%s:%d:%s" % (it["upload"], len(data), hashlib.sha256(data).hexdigest()))
                elif "text" in it:
                    seen.append("text:%s:%s" % (it.get("name", ""), it["text"]))
                elif "data" in it:
                    raw = base64.b64decode(it["data"])
                    seen.append("data:%s:%d:%s" % (it.get("name", ""), len(raw), hashlib.sha256(raw).hexdigest()))
                elif "uri" in it:
                    seen.append("uri:%s" % it["uri"])
            answer(m, {"content": [{"type": "text", "text": json.dumps({"brief": b, "seen": seen})}]})
        elif name == "plain":
            answer(m, {"content": [{"type": "text", "text": "plain:" + a.get("text", "")}]})
        else:
            answer(m, {"content": [{"type": "text", "text": "unknown " + str(name)}], "isError": True})
    else:
        answer(m, {})
"#;

    fn brief_ctx(dir: &Path, env: Option<Value>) -> crate::config::Ctx {
        let script = server_script(dir, BRIEF_SERVER);
        let ctx = crate::config::load(&dir.join(".penelope"));
        let config = json!({"servers": {"brief": {
            "command": script.display().to_string(),
            "args": [],
            "allow": true,
            "env": env.unwrap_or_else(|| json!({}))
        }}});
        fs::write(dir.join(".penelope/mcp.json"), serde_json::to_string_pretty(&config).unwrap()).unwrap();
        ctx
    }

    #[test]
    fn a_small_text_brief_is_packed_for_a_server_with_the_extension() {
        let dir = temp("brief-text");
        let ctx = brief_ctx(&dir, None);
        let text = "# Rules\nUse blue.";
        fs::write(dir.join("rules.md"), text).unwrap();
        let out = call(&ctx, "brief", "check", &json!({
            "brief": {"task": "check the rules", "attach": ["file:rules.md"]}
        })).unwrap();
        let echoed: Value = serde_json::from_str(&out).unwrap();
        let item = &echoed["brief"]["items"][0];
        assert_eq!(echoed["brief"]["task"], "check the rules");
        assert_eq!(item["kind"], "file");
        assert_eq!(item["name"], "rules.md");
        assert_eq!(item["text"], text);
        assert_eq!(item["bytes"].as_u64().unwrap(), text.len() as u64);
        assert_eq!(item["sha256"], crate::brief::sha256_hex(text.as_bytes()));
        assert!(echoed["brief"].get("attach").is_none());
        assert!(echoed["seen"][0].as_str().unwrap().starts_with("text:rules.md:"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_image_brief_travels_as_data_with_a_mime_and_a_hash() {
        let dir = temp("brief-image");
        let ctx = brief_ctx(&dir, None);
        let png = b64_decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=").unwrap();
        fs::write(dir.join("shot.png"), &png).unwrap();
        let out = call(&ctx, "brief", "check", &json!({
            "brief": {"task": "look at the picture", "attach": ["image:shot.png"]}
        })).unwrap();
        let echoed: Value = serde_json::from_str(&out).unwrap();
        let item = &echoed["brief"]["items"][0];
        assert_eq!(item["kind"], "image");
        assert_eq!(item["mime"], "image/png");
        assert_eq!(item["bytes"].as_u64().unwrap(), png.len() as u64);
        assert_eq!(b64_decode(item["data"].as_str().unwrap()).unwrap(), png);
        assert_eq!(item["sha256"], crate::brief::sha256_hex(&png));
        assert!(echoed["seen"][0].as_str().unwrap().starts_with("data:shot.png:68:"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_over_the_inline_limit_is_uploaded_before_the_call() {
        let dir = temp("brief-upload");
        let ctx = brief_ctx(&dir, None);
        let body = "x".repeat(300_000);
        fs::write(dir.join("big.txt"), &body).unwrap();
        let out = call(&ctx, "brief", "check", &json!({
            "brief": {"task": "look at the file", "attach": ["file:big.txt"]}
        })).unwrap();
        let echoed: Value = serde_json::from_str(&out).unwrap();
        let item = &echoed["brief"]["items"][0];
        let upload = item["upload"].as_str().unwrap();
        assert!(upload.starts_with("u-"), "{}", upload);
        assert!(item.get("text").is_none() && item.get("data").is_none(), "the large content stayed inline: {}", out);
        assert_eq!(item["bytes"].as_u64().unwrap(), 300_000);
        let seen = echoed["seen"][0].as_str().unwrap();
        assert!(seen.starts_with(&format!("upload:{}:300000:", upload)), "{}", seen);
        assert!(seen.ends_with(&crate::brief::sha256_hex(body.as_bytes())), "{}", seen);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn two_identical_large_files_get_two_uploads() {
        let dir = temp("brief-twice");
        let ctx = brief_ctx(&dir, None);
        fs::write(dir.join("same.txt"), "y".repeat(300_000)).unwrap();
        let out = call(&ctx, "brief", "check", &json!({
            "brief": {"task": "look twice", "attach": ["file:same.txt", "file:same.txt"]}
        })).unwrap();
        let echoed: Value = serde_json::from_str(&out).unwrap();
        let first = echoed["brief"]["items"][0]["upload"].as_str().unwrap();
        let second = echoed["brief"]["items"][1]["upload"].as_str().unwrap();
        assert_ne!(first, second, "{}", out);
        let seen = echoed["seen"].as_array().unwrap();
        assert_eq!(seen.len(), 2);
        assert!(seen.iter().all(|line| line.as_str().unwrap().contains(":300000:")), "{}", out);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_large_file_is_refused_when_the_server_has_no_upload_tool() {
        let dir = temp("brief-noupload");
        let ctx = brief_ctx(&dir, Some(json!({"BRIEF_NO_UPLOAD": "1"})));
        fs::write(dir.join("big.txt"), "x".repeat(300_000)).unwrap();
        let err = call(&ctx, "brief", "check", &json!({
            "brief": {"task": "look", "attach": ["file:big.txt"]}
        })).unwrap_err();
        assert!(err.contains("inline limit") && err.contains("upload"), "{}", err);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_path_outside_the_project_is_refused() {
        let dir = temp("brief-outside");
        let ctx = brief_ctx(&dir, None);
        let outside = std::env::temp_dir().join(format!("penelope-brief-outside-{}.txt", std::process::id()));
        fs::write(&outside, "secret").unwrap();
        let name = outside.file_name().unwrap().to_string_lossy().to_string();
        let err = call(&ctx, "brief", "check", &json!({
            "brief": {"task": "look", "attach": [format!("file:../{}", name)]}
        })).unwrap_err();
        assert!(err.contains("outside"), "{}", err);
        let _ = fs::remove_file(&outside);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_normal_tool_call_without_a_brief_still_works() {
        let dir = temp("brief-plain");
        let ctx = brief_ctx(&dir, None);
        assert_eq!(call(&ctx, "brief", "plain", &json!({"text": "hi"})).unwrap(), "plain:hi");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_foreign_server_gets_the_short_form_unchanged() {
        let dir = temp("brief-foreign");
        let ctx = brief_ctx(&dir, Some(json!({"BRIEF_NO_EXT": "1"})));
        fs::write(dir.join("rules.md"), "rules").unwrap();
        let out = call(&ctx, "brief", "check", &json!({
            "brief": {"task": "look", "attach": ["file:rules.md"]}
        })).unwrap();
        let echoed: Value = serde_json::from_str(&out).unwrap();
        assert!(echoed["brief"].get("attach").is_some(), "the short form was changed: {}", out);
        assert!(echoed["brief"].get("items").is_none(), "the short form was packed: {}", out);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_tool_without_a_brief_property_gets_no_packed_brief() {
        let dir = temp("brief-schema");
        let ctx = brief_ctx(&dir, None);
        assert_eq!(call(&ctx, "brief", "plain", &json!({
            "text": "hi", "brief": {"task": "look", "attach": []}
        })).unwrap(), "plain:hi");
        let tools = probe(&ctx, "brief").unwrap();
        assert_eq!(tools[0].input_schema["properties"]["brief"]["type"], "object");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn two_calls_reach_the_same_process() {
        let dir = temp("state");
        let script = server_script(&dir, STATE_SERVER);
        let ctx = crate::config::load(&dir.join(".penelope"));
        assert!(crate::plugins::mcp_add(&ctx, "state", &script.display().to_string(), &[], true).contains("state added"));
        assert_eq!(probe(&ctx, "state").unwrap().len(), 4);
        let first = call(&ctx, "state", "count", &json!({})).unwrap();
        let second = call(&ctx, "state", "count", &json!({})).unwrap();
        assert!(first.starts_with("count:1 pid:"), "{}", first);
        let pid = first.rsplit("pid:").next().unwrap().trim().to_string();
        assert_eq!(second, format!("count:2 pid:{}", pid));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn env_and_cwd_reach_the_server() {
        let dir = temp("env");
        let script = server_script(&dir, STATE_SERVER);
        let work = dir.join("work");
        fs::create_dir_all(&work).unwrap();
        let ctx = crate::config::load(&dir.join(".penelope"));
        let config = json!({"servers": {"envtest": {
            "command": script.display().to_string(),
            "args": [],
            "env": {"PENELOPE_TEST_ENV": "arrived"},
            "cwd": work.display().to_string(),
            "allow": true
        }}});
        fs::write(dir.join(".penelope/mcp.json"), serde_json::to_string_pretty(&config).unwrap()).unwrap();
        let out = call(&ctx, "envtest", "where", &json!({})).unwrap();
        let want = fs::canonicalize(&work).unwrap();
        assert!(out.contains(&format!("cwd:{}", want.display())), "{}", out);
        assert!(out.contains("env:arrived"), "{}", out);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_denied_tool_is_refused_and_a_named_tool_can_be_allowed() {
        let dir = temp("tools");
        let script = server_script(&dir, STATE_SERVER);
        let ctx = crate::config::load(&dir.join(".penelope"));
        assert!(crate::plugins::mcp_add(&ctx, "guard", &script.display().to_string(), &[], true).contains("added"));
        assert_eq!(call(&ctx, "guard", "danger", &json!({})).unwrap(), "danger ran");
        assert!(crate::plugins::mcp_set_tool_allow(&ctx, "guard", "danger", false).contains("denied"));
        let refused = call(&ctx, "guard", "danger", &json!({})).unwrap_err();
        assert!(refused.contains("denied") && refused.contains("danger"), "{}", refused);
        // The old allow switch stays the default: with the server denied, a named allow opens only that tool.
        assert!(crate::plugins::mcp_set_allow(&ctx, "guard", false).contains("denied"));
        assert!(call(&ctx, "guard", "danger", &json!({})).unwrap_err().contains("denied"));
        assert!(crate::plugins::mcp_set_tool_allow(&ctx, "guard", "count", true).contains("allowed"));
        assert!(call(&ctx, "guard", "count", &json!({})).unwrap().starts_with("count:1 "));
        assert!(call(&ctx, "guard", "danger", &json!({})).unwrap_err().contains("denied"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_dead_server_returns_a_clear_error() {
        let dir = temp("dead");
        let ctx = crate::config::load(&dir.join(".penelope"));
        assert!(crate::plugins::mcp_add(&ctx, "dead", "/bin/sh", &["-c".to_string(), "exit 7".to_string()], true).contains("added"));
        let first = call(&ctx, "dead", "echo", &json!({})).unwrap_err();
        assert!(first.contains("dead"), "{}", first);
        assert!(first.to_lowercase().contains("stopped") || first.to_lowercase().contains("did not answer"), "{}", first);
        // The dead process is not kept: the second call gets the same clear answer instead of a hang.
        let second = call(&ctx, "dead", "echo", &json!({})).unwrap_err();
        assert!(second.contains("dead"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_server_that_never_answers_is_stopped_after_its_timeout() {
        let dir = temp("timeout");
        let script = server_script(&dir, r#"#!/usr/bin/env python3
import sys
for line in sys.stdin:
    pass
"#);
        let ctx = crate::config::load(&dir.join(".penelope"));
        let config = json!({"servers": {"slow": {
            "command": script.display().to_string(), "args": [], "allow": true, "timeout": 1
        }}});
        fs::write(dir.join(".penelope/mcp.json"), serde_json::to_string_pretty(&config).unwrap()).unwrap();
        let started = Instant::now();
        let err = call(&ctx, "slow", "anything", &json!({})).unwrap_err();
        assert!(err.contains("slow") && err.contains("did not answer"), "{}", err);
        assert!(started.elapsed() < Duration::from_secs(10), "the timeout did not fire");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn image_and_resource_content_are_read_without_base64() {
        let dir = temp("content");
        let script = server_script(&dir, STATE_SERVER);
        let ctx = crate::config::load(&dir.join(".penelope"));
        assert!(crate::plugins::mcp_add(&ctx, "content", &script.display().to_string(), &[], true).contains("added"));
        let out = call(&ctx, "content", "media", &json!({})).unwrap();
        assert!(out.contains("the text"), "{}", out);
        assert!(out.contains("[image image/png") && out.contains("saved at"), "{}", out);
        assert!(out.contains("resource text") && out.contains("[resource link file:///linked.txt"), "{}", out);
        assert!(!out.contains("iVBORw0KGgo"), "base64 reached the prompt: {}", out);
        assert!(!out.contains("aGVsbG8="), "base64 reached the prompt: {}", out);
        let images = fs::read_dir(ctx.root.join("mcp/images")).unwrap().next().unwrap().unwrap().path();
        assert!(fs::read(&images).unwrap().starts_with(&[0x89, b'P', b'N', b'G']));
        let resources = fs::read_dir(ctx.root.join("mcp/resources")).unwrap().next().unwrap().unwrap().path();
        assert_eq!(fs::read(&resources).unwrap(), b"hello");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn base64_is_decoded_without_a_crate() {
        assert_eq!(b64_decode("aGVsbG8=").unwrap(), b"hello");
        let png = b64_decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=").unwrap();
        assert_eq!(png.len(), 68);
        assert!(png.starts_with(&[0x89, b'P', b'N', b'G']));
    }

    // The two tests below are the old format checks: they must keep working with one process per server.
    #[test]
    fn a_standalone_server_is_added_allowed_called_and_removed() {
        let dir = temp("standalone");
        let script = server_script(&dir, r#"#!/usr/bin/env python3
import sys, json
for line in sys.stdin:
    m = json.loads(line)
    if "id" not in m: continue
    if m["method"] == "initialize":
        r = {"protocolVersion": "2025-06-18", "capabilities": {"tools": {}}, "serverInfo": {"name": "echo"}}
    elif m["method"] == "tools/list":
        r = {"tools": [{"name": "echo", "description": "echo one text"}]}
    elif m["method"] == "tools/call":
        r = {"content": [{"type": "text", "text": "standalone:" + m["params"]["arguments"].get("text", "")}], "isError": False}
    else:
        r = {}
    print(json.dumps({"jsonrpc": "2.0", "id": m["id"], "result": r}), flush=True)
"#);
        let ctx = crate::config::load(&dir.join(".penelope"));
        let said = crate::plugins::mcp_add(&ctx, "echo", &script.display().to_string(), &[], false);
        assert!(said.contains("echo added"), "{}", said);
        assert_eq!(probe(&ctx, "echo").unwrap()[0].name, "echo");
        assert!(call(&ctx, "echo", "echo", &json!({"text": "hi"})).unwrap_err().contains("denied"));
        assert!(crate::plugins::mcp_set_allow(&ctx, "echo", true).contains("allowed"));
        assert_eq!(call(&ctx, "echo", "echo", &json!({"text": "hi"})).unwrap(), "standalone:hi");
        assert!(crate::plugins::allowed(&ctx).contains(&"mcp__echo__*".to_string()));
        assert!(crate::plugins::mcp_remove(&ctx, "echo").contains("removed"));
        assert!(probe(&ctx, "echo").is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_plugin_server_is_probed_and_called_only_after_allow() {
        let dir = temp("plugin");
        let script = server_script(&dir, r#"#!/usr/bin/env python3
import sys, json
for line in sys.stdin:
    m = json.loads(line)
    if "id" not in m: continue
    if m["method"] == "initialize":
        r = {"protocolVersion": "2025-06-18", "capabilities": {"tools": {}}, "serverInfo": {"name": "echo"}}
    elif m["method"] == "tools/list":
        r = {"tools": [{"name": "echo", "description": "echo one text"}]}
    elif m["method"] == "tools/call":
        r = {"content": [{"type": "text", "text": "echo:" + m["params"]["arguments"].get("text", "")}], "isError": False}
    else:
        r = {}
    print(json.dumps({"jsonrpc": "2.0", "id": m["id"], "result": r}), flush=True)
"#);
        let plugin = dir.join("demo");
        fs::create_dir_all(plugin.join(".claude-plugin")).unwrap();
        fs::write(plugin.join(".claude-plugin/plugin.json"), r#"{"name": "demo.mcp"}"#).unwrap();
        fs::write(plugin.join(".mcp.json"), json!({"mcpServers": {"echo": {"command": script.display().to_string()}}}).to_string()).unwrap();
        let ctx = crate::config::load(&dir.join(".penelope"));
        assert!(crate::plugins::add(&ctx, &plugin.to_string_lossy()).contains("demo.mcp"));
        let tools = probe(&ctx, "demo.mcp-echo").unwrap();
        assert_eq!((tools.len(), tools[0].name.as_str()), (1, "echo"));
        assert!(call(&ctx, "demo.mcp-echo", "echo", &json!({"text": "hi"})).unwrap_err().contains("denied"));
        crate::plugins::set_allow(&ctx, "demo.mcp", true);
        assert_eq!(call(&ctx, "demo.mcp-echo", "echo", &json!({"text": "hi"})).unwrap(), "echo:hi");
        assert!(list(&ctx, true).contains("echo — echo one text"));
        // A run that names other servers cannot reach this one.
        crate::agents::with_mcp_scope(&["another.server".to_string()], || {
            assert!(call(&ctx, "demo.mcp-echo", "echo", &json!({"text": "hi"})).unwrap_err().contains("may use the MCP servers: another.server"));
            assert!(probe(&ctx, "demo.mcp-echo").unwrap_err().contains("may use"));
            assert!(!list(&ctx, true).contains("echo one text"));
        });
        let _ = fs::remove_dir_all(&dir);
    }
}
