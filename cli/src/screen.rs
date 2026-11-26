//! ScreenCodec as the eyes and hands of the computer.
//!
//! ScreenCodec speaks MCP on stdin and stdout. This module keeps one ScreenCodec process alive, so the page a model
//! opened stays open between calls, sends one request at a time and waits for the answer with the same id. A timeout
//! drops the process, so the next call starts clean instead of talking into a dead pipe.
//!
//! The tool `screen` is a small fixed contract (`open`, `snapshot`, `click`, `type`, `key`, `scroll`, `image`, `call`,
//! `tools`); it maps onto the tools of ScreenCodec and refuses a name that the server did not list.

use crate::config::Ctx;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Mutex;
use std::time::Duration;

struct Bridge {
    child: Child,
    input: ChildStdin,
    rx: Receiver<(u64, Value)>,
    next: u64,
    /// Command, arguments, hosts, image folders and desktop mode. A settings change starts a new process.
    signature: String,
    tools: Vec<Value>,
}

static BRIDGE: Mutex<Option<Bridge>> = Mutex::new(None);

/// Drops the running ScreenCodec process. The next call starts a new one.
#[cfg(test)]
pub fn reset() {
    if let Ok(mut b) = BRIDGE.lock() {
        *b = None;
    }
}

fn command_of(ctx: &Ctx) -> Option<(String, Vec<String>)> {
    let configured = ctx.cfg["screencodec"]["command"].as_str().unwrap_or("").trim().to_string();
    let path = if configured.is_empty() {
        crate::config::which("screencodec-mcp").map(|p| p.display().to_string())?
    } else {
        configured
    };
    let args = ctx.cfg["screencodec"]["args"].as_str().unwrap_or("").split_whitespace().map(str::to_string).collect();
    Some((path, args))
}

fn signature_of(ctx: &Ctx) -> String {
    let (command, args) = command_of(ctx).unwrap_or_default();
    format!("{}|{}|{}|{}|{}|{}|{}", command, args.join(" "), ctx.cfg["screencodec"]["hosts"].as_str().unwrap_or(""),
            ctx.cfg["screencodec"]["images"].as_str().unwrap_or(""), ctx.cfg["screencodec"]["desktop"].as_str().unwrap_or("off"),
            ctx.cfg["screencodec"]["desktop_input"].as_str().unwrap_or("off"),
            ctx.cfg["screencodec"]["input_cmd"].as_str().unwrap_or(""))
}

fn spawn(ctx: &Ctx) -> Result<Bridge, String> {
    let Some((command, args)) = command_of(ctx) else {
        return Err("NOT done. No ScreenCodec: set screencodec.command in .penelope/config.json, or put screencodec-mcp on the PATH.".to_string());
    };
    let host = ctx.cfg["screencodec"]["hosts"].as_str().unwrap_or("");
    let images = ctx.cfg["screencodec"]["images"].as_str().unwrap_or("");
    let desktop = ctx.cfg["screencodec"]["desktop"].as_str().unwrap_or("off");
    let desktop_input = ctx.cfg["screencodec"]["desktop_input"].as_str().unwrap_or("off");
    // A Wayland desktop without ydotool can use the small uinput helper. Only when input is already on.
    let mut input_cmd = ctx.cfg["screencodec"]["input_cmd"].as_str().unwrap_or("").to_string();
    if input_cmd.is_empty() && desktop_input != "off" && std::env::var("WAYLAND_DISPLAY").is_ok() && crate::config::which("ydotool").is_none() {
        let helper = crate::input::binary();
        if helper.is_file() {
            input_cmd = helper.to_string_lossy().to_string();
        }
    }
    let mut child = Command::new(&command).args(&args)
        .env("SDL_MCP_ALLOW_HOSTS", host).env("SDL_MCP_IMAGE_DIRS", images).env("SDL_MCP_DESKTOP", desktop)
        .env("SDL_MCP_DESKTOP_INPUT", desktop_input)
        .env("SDL_MCP_INPUT_CMD", input_cmd)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().map_err(|e| format!("NOT done. ScreenCodec could not start ({}): {}", command, e))?;
    let input = child.stdin.take().ok_or("ScreenCodec has no stdin")?;
    let stdout = child.stdout.take().ok_or("ScreenCodec has no stdout")?;
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
    let mut bridge = Bridge { child, input, rx, next: 0, signature: signature_of(ctx), tools: Vec::new() };
    let hello = bridge.request("initialize", json!({
        "protocolVersion": "2025-06-18", "capabilities": {},
        "clientInfo": {"name": "penelope", "version": env!("CARGO_PKG_VERSION")}
    }), 30)?;
    let _ = hello;
    bridge.notify("notifications/initialized", json!({}));
    let list = bridge.request("tools/list", json!({}), 30)?;
    bridge.tools = list["tools"].as_array().cloned().unwrap_or_default();
    Ok(bridge)
}

impl Bridge {
    fn write(&mut self, value: &Value) -> Result<(), String> {
        writeln!(self.input, "{}", value).map_err(|e| format!("ScreenCodec stopped reading: {}", e))?;
        self.input.flush().map_err(|e| e.to_string())
    }

    fn notify(&mut self, method: &str, params: Value) {
        let _ = self.write(&json!({"jsonrpc": "2.0", "method": method, "params": params}));
    }

    fn request(&mut self, method: &str, params: Value, seconds: u64) -> Result<Value, String> {
        self.next += 1;
        let id = self.next;
        self.write(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))?;
        let wait = Duration::from_secs(seconds);
        loop {
            match self.rx.recv_timeout(wait) {
                Ok((got, v)) if got == id => {
                    if let Some(e) = v.get("error") {
                        return Err(format!("ScreenCodec says: {}", e["message"].as_str().unwrap_or(&e.to_string())));
                    }
                    return Ok(v["result"].clone());
                }
                Ok(_) => continue,                     // an answer to a request that already timed out
                Err(_) => return Err(format!("ScreenCodec gave no answer to {} within {} s", method, seconds)),
            }
        }
    }

    fn call(&mut self, tool: &str, args: Value) -> Result<String, String> {
        let result = self.request("tools/call", json!({"name": tool, "arguments": args}), 120)?;
        let text = result["content"].as_array().map(|a| a.iter().filter_map(|c| c["text"].as_str()).collect::<Vec<_>>().join("\n")).unwrap_or_default();
        if result["isError"].as_bool().unwrap_or(false) {
            return Err(format!("NOT done. {}", text));
        }
        Ok(text)
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn with_bridge<T>(ctx: &Ctx, f: &mut dyn FnMut(&mut Bridge) -> Result<T, String>) -> Result<T, String> {
    let wanted = signature_of(ctx);
    let mut guard = BRIDGE.lock().map_err(|_| "the ScreenCodec bridge is poisoned".to_string())?;
    if guard.as_ref().map(|b| b.signature != wanted).unwrap_or(true) {
        *guard = None;
        if !wanted.is_empty() && wanted.split('|').next().map(|c| !c.is_empty()).unwrap_or(false) {
            *guard = match spawn(ctx) {
                Ok(b) => Some(b),
                Err(e) => return Err(e),
            };
        } else {
            return Err("NOT done. No ScreenCodec: set screencodec.command in .penelope/config.json, or put screencodec-mcp on the PATH.".to_string());
        }
    }
    let out = f(guard.as_mut().unwrap());
    if out.is_err() {
        *guard = None;          // one timeout or a dead process must not poison every later call
    }
    out
}

/// The names and descriptions of the ScreenCodec tools, for `penelope doctor` and a model that asks.
pub fn tools(ctx: &Ctx) -> Result<Vec<(String, String)>, String> {
    with_bridge(ctx, &mut |b| Ok(b.tools.iter().map(|t| (t["name"].as_str().unwrap_or("").to_string(), t["description"].as_str().unwrap_or("").to_string())).collect()))
}

/// Calls one ScreenCodec tool by its own name. The tool has to be on the list of the server.
pub fn call(ctx: &Ctx, name: &str, args: Value) -> Result<String, String> {
    if name.is_empty() {
        return Err("NOT done. Give the ScreenCodec tool name.".to_string());
    }
    with_bridge(ctx, &mut |b| {
        if !b.tools.iter().any(|t| t["name"].as_str() == Some(name)) {
            let names = b.tools.iter().filter_map(|t| t["name"].as_str()).collect::<Vec<_>>().join(", ");
            return Err(format!("NOT done. ScreenCodec has no tool {}. Its tools: {}.", name, names));
        }
        b.call(name, args.clone())
    })
}

/// The tool `screen`. The actions map onto the real tools of ScreenCodec 0.8.1. `click`, `type` and `select` take the
/// element id from the last snapshot, which is what the click check compares; x and y only move Penelope's cursor and
/// never replace the id. A tool the server did not list is refused with the list it did send.
pub fn tool(ctx: &Ctx, args: &Value) -> Result<String, String> {
    let action = args["action"].as_str().unwrap_or("");
    let level = args["level"].as_str().filter(|l| !l.is_empty());
    let with_level = |mut v: Value| { if let Some(l) = level { v["level"] = json!(l); } v };
    let id = || args["id"].as_str().filter(|s| !s.is_empty()).map(str::to_string);
    let need_id = |what: &str| id().ok_or_else(|| format!("NOT done. {} needs the element id from the last snapshot (action snapshot). x and y only move the cursor.", what));
    match action {
        "tools" => {
            let list = tools(ctx)?;
            Ok(list.iter().map(|(n, d)| format!("{}: {}", n, d.chars().take(120).collect::<String>())).collect::<Vec<_>>().join("\n"))
        }
        "open" => {
            let url = args["url"].as_str().unwrap_or("");
            if !(url.starts_with("http://") || url.starts_with("https://") || url.starts_with("about:")) {
                return Err("NOT done. open needs an http or https address.".to_string());
            }
            let task = args["task"].as_str().or_else(|| args["question"].as_str()).unwrap_or("open the page");
            let said = call(ctx, "open", with_level(json!({"url": url, "task": task})))?;
            Ok(if said.trim().is_empty() { format!("opened {}", url) } else { said })
        }
        "snapshot" => {
            let said = call(ctx, "screen", with_level(json!({})))?;
            let mut out = if said.trim().is_empty() { "the snapshot is empty".to_string() } else { said };
            if let Some(q) = args["question"].as_str().filter(|q| !q.is_empty()) {
                out.push_str(&format!("\n(question to answer from this text: {})", q));
            }
            Ok(out)
        }
        "click" => {
            let id = need_id("click")?;
            let moved = match (args["x"].as_i64(), args["y"].as_i64()) {
                (Some(x), Some(y)) => crate::cursor::move_to(ctx, x, y, "click"),
                _ => None,
            };
            let said = call(ctx, "click", json!({"id": id}))?;
            Ok(format!("{}{}", moved.map(|m| format!("{} · ", m)).unwrap_or_default(), said))
        }
        "type" => {
            let id = need_id("type")?;
            let text = args["text"].as_str().unwrap_or("");
            if text.is_empty() {
                return Err("NOT done. type needs text.".to_string());
            }
            call(ctx, "type", json!({"id": id, "text": text}))
        }
        "select" => {
            let id = need_id("select")?;
            let mut body = json!({"id": id});
            if let Some(option) = args["option"].as_str().filter(|o| !o.is_empty()) {
                body["option"] = json!(option);
            }
            call(ctx, "select", body)
        }
        "key" => {
            let key = args["key"].as_str().unwrap_or("");
            if key.is_empty() {
                return Err("NOT done. key needs a key name such as Tab, Escape or PageDown.".to_string());
            }
            call(ctx, "key", json!({"name": key}))
        }
        "scroll" => call(ctx, "scroll", json!({"dy": args["amount"].as_i64().unwrap_or(300)})),
        "zoom" => {
            let n = |k: &str| args[k].as_i64().unwrap_or(0);
            call(ctx, "zoom", json!({"x0": n("x0"), "y0": n("y0"), "x1": n("x1"), "y1": n("y1")}))
        }
        "image" => {
            let path = args["path"].as_str().unwrap_or("");
            if path.is_empty() {
                return Err("NOT done. image needs the path of a picture inside SDL_MCP_IMAGE_DIRS.".to_string());
            }
            call(ctx, "image", with_level(json!({"path": path})))
        }
        "desktop" => {
            let mut body = with_level(json!({}));
            for key in ["scope", "find", "code"] {
                if let Some(v) = args[key].as_str().filter(|v| !v.is_empty()) { body[key] = json!(v); }
            }
            call(ctx, "desktop", body)
        }
        "desktop_click" => {
            let id = need_id("desktop_click")?;
            let mut body = json!({"id": id});
            if let Some(code) = args["code"].as_str().filter(|c| !c.is_empty()) { body["code"] = json!(code); }
            call(ctx, "desktop_click", body)
        }
        "confirm" => {
            let code = args["code"].as_str().unwrap_or("");
            let summary = args["summary"].as_str().unwrap_or("");
            if code.is_empty() || summary.is_empty() {
                return Err("NOT done. confirm needs the code and the summary that ScreenCodec returned.".to_string());
            }
            call(ctx, "confirm", json!({"code": code, "summary": summary}))
        }
        "call" => {
            let tool = args["tool"].as_str().unwrap_or("");
            let arguments = if args["arguments"].is_object() { args["arguments"].clone() } else { json!({}) };
            call(ctx, tool, arguments)
        }
        other => Err(format!("NOT done. action {} is none of open, snapshot, click, type, select, key, scroll, zoom, image, desktop, desktop_click, confirm, call, tools.", if other.is_empty() { "(none)" } else { other })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn a_fake_screencodec_keeps_its_page_across_calls_and_checks_tool_names() {
        reset();
        let dir = std::env::temp_dir().join(format!("penelope-screen-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let log = dir.join("rpc.log");
        let script = dir.join("fake-screencodec");
        fs::write(&script, format!(r#"#!/usr/bin/env python3
import sys, json
log = open({:?}, "a")
for line in sys.stdin:
    m = json.loads(line)
    log.write(json.dumps(m, sort_keys=True) + "\n"); log.flush()
    if "id" not in m: continue
    method = m.get("method")
    if method == "initialize":
        r = {{"protocolVersion": "2025-06-18", "capabilities": {{"tools": {{}}}}, "serverInfo": {{"name": "fake"}}}}
    elif method == "tools/list":
        r = {{"tools": [{{"name": "open", "description": "open a page"}}, {{"name": "screen", "description": "see"}},
                       {{"name": "click", "description": "click"}}, {{"name": "type", "description": "type"}}]}}
    elif method == "tools/call":
        n = m["params"]["name"]; a = m["params"].get("arguments", {{}})
        r = {{"content": [{{"type": "text", "text": n + ":" + json.dumps(a, sort_keys=True)}}], "isError": False}}
    else:
        r = {{}}
    print(json.dumps({{"jsonrpc": "2.0", "id": m["id"], "result": r}}), flush=True)
"#, log.display())).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let mut ctx = crate::config::load(&dir.join(".penelope"));
        ctx.cfg["screencodec"] = json!({"command": script.display().to_string(), "hosts": "", "images": ""});
        ctx.cfg["cursor"] = json!({"enabled": false});
        let opened = tool(&ctx, &json!({"action": "open", "url": "https://example.org", "task": "read the page", "level": "lean"})).unwrap();
        assert_eq!(opened, "open:{\"level\": \"lean\", \"task\": \"read the page\", \"url\": \"https://example.org\"}");
        let seen = tool(&ctx, &json!({"action": "snapshot", "question": "what is on the page?", "level": "lean"})).unwrap();
        assert!(seen.starts_with("screen:{\"level\": \"lean\"}") && seen.contains("question to answer"));
        let clicked = tool(&ctx, &json!({"action": "click", "id": "button1", "x": 12, "y": 34})).unwrap();
        assert_eq!(clicked, "click:{\"id\": \"button1\"}");
        assert!(tool(&ctx, &json!({"action": "click", "x": 12, "y": 34})).unwrap_err().contains("needs the element id"));
        let names = tool(&ctx, &json!({"action": "tools"})).unwrap();
        assert!(names.contains("open: open a page") && names.contains("screen"));
        let bad = tool(&ctx, &json!({"action": "call", "tool": "zoom", "arguments": {}})).unwrap_err();
        assert!(bad.contains("no tool zoom") && bad.contains("open, screen, click, type"), "{}", bad);
        let log = fs::read_to_string(&log).unwrap();
        assert!(log.contains("\"method\": \"initialize\"") && log.contains("notifications/initialized") && log.lines().count() >= 6, "{}", log);
        reset();
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn without_screencodec_the_refusal_names_the_setting() {
        reset();
        let dir = std::env::temp_dir().join(format!("penelope-screen-none-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let mut ctx = crate::config::load(&dir.join(".penelope"));
        ctx.cfg["screencodec"]["command"] = json!("/nonexistent/screencodec-mcp");
        let said = tool(&ctx, &json!({"action": "open", "url": "https://example.org"})).unwrap_err();
        assert!(said.contains("could not start") && said.contains("/nonexistent"), "{}", said);
        reset();
        let _ = fs::remove_dir_all(&dir);
    }
}
