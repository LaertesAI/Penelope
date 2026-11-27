//! Named MCP server profiles: `penelope mcp serve kerkis` and `penelope mcp serve vet`. Each profile has its own tool
//! set and speaks Mitos version 0 through `mitos.rs`. The default `penelope mcp` server in `mcp.rs` is not touched.
//!
//! Errors follow JSON-RPC. A line that is not JSON gets -32700, a request without a method -32600, an unknown method
//! -32601, bad parameters or an unknown tool -32602. A tool that refuses a valid call answers with `isError: true`
//! and a text that starts with `NOT`. The loop never exits on a bad line.

use crate::config::Ctx;
use crate::mitos;
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::path::PathBuf;

/// Why a tool call failed.
pub enum Fail {
    /// The arguments do not fit the schema: a JSON-RPC error, code -32602.
    Params(String),
    /// The arguments are valid but the server refuses: a result with `isError: true`.
    Refused(String),
}

/// A successful tool answer: the text and the origin level L of the text in it.
pub struct Reply {
    pub text: String,
    pub level: u8,
}

pub type Handler = Box<dyn Fn(&str, &Value) -> Result<Reply, Fail>>;

pub struct Profile {
    pub name: &'static str,
    pub instructions: &'static str,
    pub specs: Vec<Value>,
    pub call: Handler,
}

/// The profile names `penelope mcp serve` accepts.
pub const NAMES: [&str; 2] = ["kerkis", "vet"];

/// Starts the named profile on stdin and stdout. `args` is everything after `serve`.
pub fn serve_named(ctx: &Ctx, args: &[String]) -> Result<(), String> {
    let Some(name) = args.first().map(String::as_str) else {
        return Err(format!("penelope mcp serve NAME: NAME is one of {}", NAMES.join(", ")));
    };
    let flags = &args[1..];
    let profile = build(ctx, name, flags)?;
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    run(&profile, stdin.lock(), &mut out);
    Ok(())
}

/// Builds a profile from its name and flags. An unknown flag is an error, so a typo such as `--allow-persons` cannot
/// silently leave a guard on.
pub fn build(ctx: &Ctx, name: &str, flags: &[String]) -> Result<Profile, String> {
    match name {
        "kerkis" => {
            let mut allow_person = false;
            for flag in flags {
                match flag.as_str() {
                    "--allow-person" => allow_person = true,
                    other => return Err(format!("penelope mcp serve kerkis: unknown option {} (known: --allow-person)", other)),
                }
            }
            Ok(crate::mcpmem::profile(ctx, allow_person))
        }
        "vet" => {
            let mut roots: Vec<PathBuf> = Vec::new();
            let mut i = 0;
            while i < flags.len() {
                match flags[i].as_str() {
                    "--scan-root" if i + 1 < flags.len() => {
                        roots.push(PathBuf::from(&flags[i + 1]));
                        i += 2;
                    }
                    other => return Err(format!("penelope mcp serve vet: unknown option {} (known: --scan-root DIR, repeatable)", other)),
                }
            }
            crate::mcpvet::profile(roots)
        }
        other => Err(format!("unknown MCP server profile {}. Known: {}", other, NAMES.join(", "))),
    }
}

fn error(id: &Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

/// The JSON-RPC loop. Reads lines from `input`, writes one line per request to `out`, stops at the end of input.
pub fn run<R: BufRead, W: Write>(profile: &Profile, mut input: R, out: &mut W) {
    let mut mitos_on = false;
    let mut raw: Vec<u8> = Vec::new();
    loop {
        raw.clear();
        match input.read_until(b'\n', &mut raw) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        // A line that is not UTF-8 is a parse error for that line, not the end of the server.
        let answer = match std::str::from_utf8(&raw) {
            Ok(line) if line.trim().is_empty() => continue,
            Ok(line) => handle(profile, &mut mitos_on, line.trim()),
            Err(_) => Some(error(&Value::Null, -32700, "parse error: the line is not valid UTF-8")),
        };
        if let Some(answer) = answer {
            if writeln!(out, "{}", answer).is_err() || out.flush().is_err() {
                break;
            }
        }
    }
}

/// One request line to one answer line. A notification has no answer.
pub fn handle(profile: &Profile, mitos_on: &mut bool, line: &str) -> Option<Value> {
    let msg: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(_) => return Some(error(&Value::Null, -32700, "parse error: the line is not JSON")),
    };
    let Some(object) = msg.as_object() else {
        return Some(error(&Value::Null, -32600, "invalid request: not a JSON object"));
    };
    let id = object.get("id").cloned().unwrap_or(Value::Null);
    let method = object.get("method").and_then(Value::as_str);
    if id.is_null() {
        // A notification, or a request without an id: no answer. A request that has no method either is invalid.
        return if method.is_none() { Some(error(&id, -32600, "invalid request: no method")) } else { None };
    }
    let Some(method) = method else {
        return Some(error(&id, -32600, "invalid request: method must be a string"));
    };
    let params = object.get("params").cloned().unwrap_or(Value::Null);
    if !params.is_null() && !params.is_object() {
        return Some(error(&id, -32602, "invalid params: params must be an object"));
    }
    let result = match method {
        "initialize" => {
            *mitos_on = mitos::announced(&params);
            json!({
                "protocolVersion": params["protocolVersion"].as_str().unwrap_or("2025-06-18"),
                "capabilities": {"tools": {"listChanged": false}, "experimental": {mitos::EXTENSION_ID: mitos::manifest()}},
                "serverInfo": {"name": format!("penelope-{}", profile.name), "version": env!("CARGO_PKG_VERSION")},
                "instructions": profile.instructions,
            })
        }
        "ping" => json!({}),
        "tools/list" => mitos::tools_list(&profile.specs, *mitos_on),
        "mitos/describe" => {
            if !params["names"].is_array() || params["names"].as_array().unwrap().iter().any(|n| !n.is_string()) {
                return Some(error(&id, -32602, "invalid params: names must be an array of strings"));
            }
            mitos::describe(&profile.specs, &params)
        }
        "tools/call" => {
            let Some(name) = params["name"].as_str() else {
                return Some(error(&id, -32602, "invalid params: name must be a string"));
            };
            let args = match params.get("arguments") {
                None | Some(Value::Null) => json!({}),
                Some(v) if v.is_object() => v.clone(),
                Some(_) => return Some(error(&id, -32602, "invalid params: arguments must be an object")),
            };
            if !profile.specs.iter().any(|s| s["name"].as_str() == Some(name)) {
                return Some(error(&id, -32602, &format!("invalid params: unknown tool {}", name)));
            }
            match (profile.call)(name, &args) {
                Ok(reply) => json!({"content": [{"type": "text", "text": reply.text}], "isError": false,
                                    "_meta": {mitos::EXTENSION_ID: {"l": reply.level}}}),
                Err(Fail::Refused(text)) => json!({"content": [{"type": "text", "text": text}], "isError": true,
                                                   "_meta": {mitos::EXTENSION_ID: {"l": 1}}}),
                Err(Fail::Params(message)) => return Some(error(&id, -32602, &format!("invalid params: {}", message))),
            }
        }
        _ => return Some(error(&id, -32601, "method not found")),
    };
    Some(json!({"jsonrpc": "2.0", "id": id, "result": result}))
}

/// Compact JSON bytes of one `tools/list` answer, the measure used in `docs/MITOS.md`.
#[cfg(test)]
pub fn list_bytes(specs: &[Value], mitos_on: bool) -> usize {
    serde_json::to_string(&mitos::tools_list(specs, mitos_on)).map(|s| s.len()).unwrap_or(0)
}

/// Shared test helpers for the two profiles.
#[cfg(test)]
pub mod testing {
    use super::*;

    /// Sends the requests in order and returns the answers, one per request that has an id.
    pub fn exchange(profile: &Profile, lines: &[String]) -> Vec<Value> {
        exchange_bytes(profile, (lines.join("\n") + "\n").into_bytes())
    }

    pub fn exchange_bytes(profile: &Profile, input: Vec<u8>) -> Vec<Value> {
        let mut out: Vec<u8> = Vec::new();
        run(profile, std::io::Cursor::new(input), &mut out);
        String::from_utf8(out).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect()
    }

    pub fn request(id: u64, method: &str, params: Value) -> String {
        json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string()
    }

    pub fn call(id: u64, name: &str, arguments: Value) -> String {
        request(id, "tools/call", json!({"name": name, "arguments": arguments}))
    }

    pub fn mitos_init() -> String {
        request(1, "initialize", json!({"protocolVersion": "2025-06-18",
            "capabilities": {"experimental": {mitos::EXTENSION_ID: mitos::manifest()}}}))
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;

    #[test]
    fn a_line_with_invalid_utf8_gets_a_parse_error_and_the_server_keeps_running() {
        let profile = Profile { name: "t", instructions: "", specs: Vec::new(), call: Box::new(|_, _| Err(Fail::Refused("x".into()))) };
        let mut input = b"\xff\xfe{\"jsonrpc\"\n".to_vec();
        input.extend_from_slice(request(7, "ping", json!({})).as_bytes());
        input.push(b'\n');
        let out = exchange_bytes(&profile, input);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0]["error"]["code"], -32700);
        assert_eq!(out[1]["id"], 7);
        assert!(out[1]["result"].is_object());
    }
}
