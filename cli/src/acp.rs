//! The Agent Client Protocol for editors. Zed or Neovim start `penelope acp` and talk JSON-RPC 2.0 over stdin and
//! stdout: initialize, session/new, session/prompt, session/cancel, session/update and shutdown. The transport is the
//! newline-delimited JSON of ACP; a Content-Length frame is accepted too.
//!
//! One session holds one `backend::Chat`, so every prompt runs the same tool gate as `penelope ask`. A turn runs on its
//! own thread: the reader stays free and `session/cancel` stops the turn. Nothing but protocol goes to stdout; every
//! log line goes to stderr.

use crate::backend::{self, Chat};
use crate::config::Ctx;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufWriter, Write};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// The protocol version of ACP this server speaks.
const PROTOCOL_VERSION: u64 = 1;

/// How the peer frames a message. ACP uses one JSON object per line; LSP-style Content-Length is accepted as well.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Framing {
    Lines,
    Headers,
}

/// Writes JSON-RPC messages to stdout. The mutex keeps a streaming update and a response from interleaving.
struct Protocol {
    out: Mutex<BufWriter<std::io::Stdout>>,
    headers: AtomicBool,
}

impl Protocol {
    fn new() -> Protocol {
        Protocol { out: Mutex::new(BufWriter::new(std::io::stdout())), headers: AtomicBool::new(false) }
    }

    fn send(&self, message: &Value) {
        let text = serde_json::to_string(message).unwrap_or_else(|_| "{}".to_string());
        let mut out = self.out.lock().unwrap_or_else(|e| e.into_inner());
        if self.headers.load(Ordering::SeqCst) {
            let _ = write!(out, "Content-Length: {}\r\n\r\n", text.len());
        }
        let _ = out.write_all(text.as_bytes());
        if !self.headers.load(Ordering::SeqCst) {
            let _ = out.write_all(b"\n");
        }
        let _ = out.flush();
    }

    fn result(&self, id: Value, result: Value) {
        self.send(&json!({"jsonrpc": "2.0", "id": id, "result": result}));
    }

    fn error(&self, id: Value, code: i64, message: impl Into<String>) {
        self.send(&json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message.into()}}));
    }

    /// One `session/update` notification. The update object is the ACP union member (`agent_message_chunk`, ...).
    fn update(&self, session: &str, update: Value) {
        self.send(&json!({"jsonrpc": "2.0", "method": "session/update",
                          "params": {"sessionId": session, "update": update}}));
    }
}

/// One editor session. The chat lock is held while its turn runs; cancel and pid stay reachable without that lock.
struct Session {
    chat: Mutex<Chat>,
    cancel: Arc<AtomicBool>,
    pid: Arc<AtomicU32>,
    cwd: String,
    busy: AtomicBool,
}

struct Server {
    ctx: Ctx,
    protocol: Arc<Protocol>,
    sessions: Mutex<HashMap<String, Arc<Session>>>,
    seq: AtomicU64,
}

/// Runs the ACP endpoint on stdin and stdout until `shutdown`, `exit` or end of input.
pub fn run(ctx: &Ctx) -> Result<(), String> {
    let protocol = Arc::new(Protocol::new());
    let server = Server { ctx: ctx.clone(), protocol: protocol.clone(), sessions: Mutex::new(HashMap::new()), seq: AtomicU64::new(1) };
    let stdin = std::io::stdin();
    let mut input = std::io::BufReader::new(stdin.lock());
    let mut framing = Framing::Lines;
    loop {
        let frame = match read_frame(&mut input, &mut framing) {
            Ok(Some(frame)) => frame,
            Ok(None) => break,
            Err(e) => return Err(format!("cannot read stdin: {}", e)),
        };
        protocol.headers.store(framing == Framing::Headers, Ordering::SeqCst);
        let value: Value = serde_json::from_slice(&frame).map_err(|e| format!("invalid JSON on stdin: {}", e))?;
        if !server.handle(value) {
            break;
        }
    }
    server.cancel_all();
    Ok(())
}

/// Reads one message in the framing of the connection. A line that starts a JSON object or array is one message; a
/// `Content-Length` header reads exactly that many bytes after the empty line. Returns None at end of input.
fn read_frame(input: &mut dyn BufRead, framing: &mut Framing) -> Result<Option<Vec<u8>>, String> {
    loop {
        let mut line = String::new();
        match input.read_line(&mut line) {
            Ok(0) => return Ok(None),
            Ok(_) => {}
            Err(e) => return Err(e.to_string()),
        }
        if line.trim().is_empty() {
            continue;
        }
        let lower = line.trim_start().to_ascii_lowercase();
        if let Some(rest) = lower.strip_prefix("content-length:") {
            let length: usize = rest.trim().parse().map_err(|_| format!("bad Content-Length: {}", line.trim()))?;
            if length > 64 * 1024 * 1024 {
                return Err(format!("a frame of {} bytes is too large", length));
            }
            loop {
                let mut header = String::new();
                if input.read_line(&mut header).map_err(|e| e.to_string())? == 0 {
                    return Err("the connection ended inside a frame".to_string());
                }
                if header.trim().is_empty() {
                    break;
                }
            }
            let mut body = vec![0u8; length];
            input.read_exact(&mut body).map_err(|e| e.to_string())?;
            *framing = Framing::Headers;
            return Ok(Some(body));
        }
        if framing == &Framing::Headers {
            return Err(format!("expected a Content-Length header, got: {}", line.trim().chars().take(80).collect::<String>()));
        }
        *framing = Framing::Lines;
        return Ok(Some(line.trim_end().as_bytes().to_vec()));
    }
}

impl Server {
    /// Handles one JSON-RPC message. False ends the connection.
    fn handle(&self, value: Value) -> bool {
        let Some(obj) = value.as_object() else {
            self.protocol.error(Value::Null, -32600, "invalid request: a JSON-RPC message must be an object");
            return true;
        };
        let id = obj.get("id").cloned().filter(|v| !v.is_null());
        let Some(method) = obj.get("method").and_then(Value::as_str) else {
            // A response to a request of this server. The server sends no requests of its own, so it is ignored.
            if !obj.contains_key("result") && !obj.contains_key("error") {
                if let Some(id) = id {
                    self.protocol.error(id, -32600, "invalid request: no method");
                }
            }
            return true;
        };
        let params = obj.get("params").cloned().unwrap_or(Value::Null);
        match method {
            "initialize" => {
                if let Some(id) = id {
                    self.protocol.result(id, initialize(&params));
                }
            }
            "session/new" => {
                if let Some(id) = id {
                    self.new_session(id, &params);
                }
            }
            "session/prompt" => match id {
                Some(id) => self.prompt(id, &params),
                None => eprintln!("penelope acp: session/prompt without an id, ignored"),
            },
            "session/cancel" => {
                self.cancel(&params);
                if let Some(id) = id {
                    self.protocol.result(id, Value::Null);
                }
            }
            "shutdown" | "exit" => {
                if let Some(id) = id {
                    self.protocol.result(id, Value::Null);
                }
                return false;
            }
            other => match id {
                Some(id) => self.protocol.error(id, -32601, format!("method not found: {}", other)),
                None => eprintln!("penelope acp: ignoring unknown notification {}", other),
            },
        }
        true
    }

    fn new_session(&self, id: Value, params: &Value) {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let session_id = format!("acp-{:x}-{}", nanos, self.seq.fetch_add(1, Ordering::SeqCst));
        let chat = Chat::new();
        let session = Arc::new(Session {
            cancel: chat.cancel.clone(),
            pid: chat.pid.clone(),
            chat: Mutex::new(chat),
            cwd: params["cwd"].as_str().unwrap_or("").to_string(),
            busy: AtomicBool::new(false),
        });
        self.sessions.lock().unwrap_or_else(|e| e.into_inner()).insert(session_id.clone(), session);
        self.protocol.result(id, json!({"sessionId": session_id}));
    }

    /// Starts one turn on a worker thread. The reader goes on, so a cancel notification can arrive while the turn runs.
    fn prompt(&self, id: Value, params: &Value) {
        let session_id = params["sessionId"].as_str().unwrap_or("").to_string();
        let session = match self.sessions.lock().unwrap_or_else(|e| e.into_inner()).get(&session_id).cloned() {
            Some(s) => s,
            None => {
                self.protocol.error(id, -32602, format!("unknown session: {}", session_id));
                return;
            }
        };
        let Some(text) = prompt_text(params) else {
            self.protocol.error(id, -32602, "the prompt holds no text");
            return;
        };
        if session.busy.swap(true, Ordering::SeqCst) {
            self.protocol.error(id, -32000, "this session already runs a turn");
            return;
        }
        let ctx = self.ctx.clone();
        let protocol = self.protocol.clone();
        std::thread::spawn(move || {
            let outcome = run_turn(&ctx, &session, &session_id, &text, &protocol);
            session.busy.store(false, Ordering::SeqCst);
            let cancelled = session.cancel.load(Ordering::SeqCst);
            match outcome {
                Ok(_) if cancelled => protocol.result(id, json!({"stopReason": "cancelled"})),
                Ok(_) => protocol.result(id, json!({"stopReason": "end_turn"})),
                Err(e) if cancelled || e == "stopped" => protocol.result(id, json!({"stopReason": "cancelled"})),
                Err(e) if e.contains("declined this request") => protocol.result(id, json!({"stopReason": "refusal"})),
                Err(e) => protocol.error(id, -32001, format!("the turn failed: {}", e)),
            }
        });
    }

    fn cancel(&self, params: &Value) {
        let session_id = params["sessionId"].as_str().unwrap_or("");
        let session = self.sessions.lock().unwrap_or_else(|e| e.into_inner()).get(session_id).cloned();
        match session {
            Some(session) => stop(&session),
            None => eprintln!("penelope acp: session/cancel for an unknown session: {}", session_id),
        }
    }

    fn cancel_all(&self) {
        let sessions: Vec<Arc<Session>> = self.sessions.lock().unwrap_or_else(|e| e.into_inner()).values().cloned().collect();
        for session in sessions {
            stop(&session);
        }
    }
}

/// Sets the cancel flag and ends a running claude child. The turn sees the flag and answers with stopReason cancelled.
fn stop(session: &Session) {
    session.cancel.store(true, Ordering::SeqCst);
    let pid = session.pid.load(Ordering::SeqCst);
    if pid != 0 {
        // The kill waits for the child, so it runs apart from the reader that must stay free for a shutdown.
        std::thread::spawn(move || { crate::platform::kill_tree(pid as u64); });
    }
}

/// The initialize handshake. The client may ask for a newer version; this server answers with the one it speaks.
fn initialize(params: &Value) -> Value {
    let asked = params["protocolVersion"].as_u64().unwrap_or(PROTOCOL_VERSION).max(1);
    json!({
        "protocolVersion": PROTOCOL_VERSION.min(asked),
        "agentCapabilities": {
            "promptCapabilities": {"image": false, "audio": false, "embeddedContext": true},
            "loadSession": false
        },
        "authMethods": [],
        "agentInfo": {"name": "penelope", "title": "Penelope", "version": env!("CARGO_PKG_VERSION")}
    })
}

/// The text blocks of one ACP prompt. Text and embedded resources travel in; image and audio are named, not decoded.
fn prompt_text(params: &Value) -> Option<String> {
    if let Some(text) = params["prompt"].as_str() {
        let text = text.trim();
        return if text.is_empty() { None } else { Some(text.to_string()) };
    }
    let mut parts = Vec::new();
    for block in params["prompt"].as_array().cloned().unwrap_or_default() {
        match block["type"].as_str().unwrap_or("text") {
            "text" => {
                if let Some(text) = block["text"].as_str().filter(|t| !t.trim().is_empty()) {
                    parts.push(text.to_string());
                }
            }
            "resource_link" => {
                let uri = block["uri"].as_str().unwrap_or("");
                let name = block["name"].as_str().unwrap_or(uri);
                if !uri.is_empty() {
                    parts.push(format!("[resource {} at {}]", name, uri));
                }
            }
            "resource" => {
                if let Some(text) = block["resource"]["text"].as_str() {
                    parts.push(text.to_string());
                } else if let Some(uri) = block["resource"]["uri"].as_str() {
                    parts.push(format!("[resource at {}]", uri));
                }
            }
            "image" => parts.push("[image omitted: Penelope takes text only]".to_string()),
            "audio" => parts.push("[audio omitted: Penelope takes text only]".to_string()),
            _ => {}
        }
    }
    let text = parts.join("\n");
    let text = text.trim();
    if text.is_empty() { None } else { Some(text.to_string()) }
}

/// One turn with the normal context expansion, memory lookup and tool gate. Text and tool calls stream as
/// `session/update`; the final answer is sent too when the backend returned it without a text block before.
fn run_turn(ctx: &Ctx, session: &Session, session_id: &str, text: &str, protocol: &Protocol) -> Result<String, String> {
    if !session.cwd.is_empty() && std::path::Path::new(&session.cwd).is_dir() {
        // The editor names the project folder of the session. Tools without a cwd work there.
        let _ = std::env::set_current_dir(&session.cwd);
    }
    let expanded = crate::context::expand(ctx, text);
    let (notes, trail, _cached) = crate::tools::recall_all(ctx, &expanded.text, 1600);
    let mut emitted: Vec<String> = Vec::new();
    let mut last_tool: Option<String> = None;
    let mut tool_seq = 0u64;
    let result = {
        let mut chat = session.chat.lock().unwrap_or_else(|e| e.into_inner());
        chat.turn_with(ctx, &expanded.text, &notes, &mut |event| match event {
            backend::Event::Thinking(text) => {
                protocol.update(session_id, json!({"sessionUpdate": "agent_thought_chunk",
                                                   "content": {"type": "text", "text": text}}));
            }
            backend::Event::Text(text) => {
                emitted.push(text.trim().to_string());
                protocol.update(session_id, json!({"sessionUpdate": "agent_message_chunk",
                                                   "content": {"type": "text", "text": text}}));
            }
            backend::Event::ToolStart { id, name, detail } => {
                let tool_id = if id.is_empty() {
                    tool_seq += 1;
                    format!("acp-tool-{}", tool_seq)
                } else {
                    id
                };
                last_tool = Some(tool_id.clone());
                let kind = tool_kind(&name);
                let title = format!("{} {}", name, detail).trim().to_string();
                let title = if title.is_empty() { name } else { title };
                protocol.update(session_id, json!({
                    "sessionUpdate": "tool_call",
                    "toolCallId": tool_id,
                    "title": title,
                    "kind": kind,
                    "status": "in_progress",
                    "rawInput": {"detail": detail}
                }));
            }
            backend::Event::ToolEnd { id, ok, summary } => {
                let tool_id = if id.is_empty() { last_tool.clone().unwrap_or_else(|| "tool".to_string()) } else { id };
                let summary: String = summary.chars().take(4000).collect();
                protocol.update(session_id, json!({
                    "sessionUpdate": "tool_call_update",
                    "toolCallId": tool_id,
                    "status": if ok { "completed" } else { "failed" },
                    "content": [{"type": "content", "content": {"type": "text", "text": summary}}],
                    "rawOutput": {"summary": summary}
                }));
            }
            backend::Event::Masked(n) => eprintln!("penelope acp: the privacy filter masked {} items in the request", n),
            backend::Event::MaskedAnswer(n) => eprintln!("penelope acp: the privacy filter masked {} items in the answer", n),
            backend::Event::Delta(_) => {
                // Deltas are not filtered; only the finished block goes out, so no secret travels through a chunk.
            }
        })
    };
    if result.is_ok() && !trail.is_empty() {
        crate::memory::Memory::open(&ctx.root).mark_used(&trail);
    }
    let answer = result?;
    if !answer.is_empty() && !emitted.iter().any(|t| t == answer.trim()) {
        protocol.update(session_id, json!({"sessionUpdate": "agent_message_chunk",
                                           "content": {"type": "text", "text": answer}}));
    }
    Ok(answer)
}

/// The ACP tool kind of a Penelope tool, for the icon the editor draws.
fn tool_kind(name: &str) -> &'static str {
    let head = name.split_whitespace().next().unwrap_or("");
    match head {
        "read" | "outline" => "read",
        "find" | "grep" | "tree" | "web_search" | "research" | "session_search" => "search",
        "write" | "edit" | "undo" | "unzip" | "forge" | "tool_new" => "edit",
        "run" | "job_start" | "job_stop" | "cursor" | "screen" => "execute",
        "fetch" => "fetch",
        "memory_go" | "memory_map" | "memory_note" | "plan" => "think",
        _ => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_message_per_line_and_content_length_frames_are_both_read() {
        let mut lines = std::io::Cursor::new(b"{\"a\":1}\n\n{\"b\":2}\n".to_vec());
        let mut framing = Framing::Lines;
        assert_eq!(read_frame(&mut lines, &mut framing).unwrap(), Some(b"{\"a\":1}".to_vec()));
        assert_eq!(read_frame(&mut lines, &mut framing).unwrap(), Some(b"{\"b\":2}".to_vec()));
        assert_eq!(read_frame(&mut lines, &mut framing).unwrap(), None);
        assert_eq!(framing, Framing::Lines);

        let body = b"{\"jsonrpc\":\"2.0\",\"id\":1}";
        let frame = format!("Content-Length: {}\r\nContent-Type: application/json\r\n\r\n{}", body.len(), String::from_utf8_lossy(body));
        let mut headers = std::io::Cursor::new(frame.into_bytes());
        let mut framing = Framing::Lines;
        assert_eq!(read_frame(&mut headers, &mut framing).unwrap(), Some(body.to_vec()));
        assert_eq!(framing, Framing::Headers);
    }

    #[test]
    fn a_prompt_is_read_from_text_and_resource_blocks() {
        let params = json!({"prompt": [
            {"type": "text", "text": "What is in note.txt?"},
            {"type": "resource_link", "uri": "file:///tmp/note.txt", "name": "note.txt"},
            {"type": "image", "data": "..."}
        ]});
        let text = prompt_text(&params).unwrap();
        assert!(text.starts_with("What is in note.txt?") && text.contains("[resource note.txt at file:///tmp/note.txt]"), "{}", text);
        assert!(text.contains("[image omitted"), "{}", text);
        assert_eq!(prompt_text(&json!({"prompt": []})), None);
        assert_eq!(prompt_text(&json!({"prompt": [{"type": "text", "text": "  "}]})), None);
    }

    #[test]
    fn tool_names_map_to_the_acp_kinds() {
        assert_eq!(tool_kind("read"), "read");
        assert_eq!(tool_kind("grep"), "search");
        assert_eq!(tool_kind("edit"), "edit");
        assert_eq!(tool_kind("run"), "execute");
        assert_eq!(tool_kind("fetch"), "fetch");
        assert_eq!(tool_kind("screen open"), "execute");
        assert_eq!(tool_kind("something else"), "other");
    }

    #[test]
    fn initialize_never_answers_with_a_newer_protocol_than_it_speaks() {
        assert_eq!(initialize(&json!({"protocolVersion": 1}))["protocolVersion"], 1);
        assert_eq!(initialize(&json!({"protocolVersion": 9}))["protocolVersion"], 1);
        assert_eq!(initialize(&json!({}))["protocolVersion"], 1);
        assert_eq!(initialize(&json!({"protocolVersion": 1}))["agentCapabilities"]["loadSession"], false);
    }
}
