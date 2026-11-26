//! A read-only LSP tool backed by rust-analyzer.
//!
//! One `lsp` call starts a rust-analyzer child, speaks the Language Server Protocol with Content-Length framing,
//! opens the named Rust file, waits for an answer with a timeout and shuts the server down again. Diagnostics,
//! the type at a position and the definition of a symbol are read from the server; no file is ever changed.
//!
//! rust-analyzer publishes an empty diagnostic list before `cargo check` has finished. An empty list is therefore
//! not trusted at once: it must stay quiet for a few seconds before the answer counts as "no diagnostics". A
//! missing binary, a timeout and a crashed server are ordinary errors; none of them can hang a turn.

use crate::config::Ctx;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

/// How long the initialize handshake may take.
const INIT_TIMEOUT: Duration = Duration::from_secs(20);
/// How long a process gets to leave after `shutdown` before it is killed.
const STOP_WAIT: Duration = Duration::from_millis(800);
/// The wait for diagnostics and requests when `lsp.timeout_seconds` is not set.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
/// An empty diagnostic report may be followed by the real one from `cargo check`. Wait this long after an empty report.
const EMPTY_SETTLE: Duration = Duration::from_secs(4);
/// A non-empty report is nearly final; a short quiet period catches one correction from the same analysis.
const REPORT_SETTLE: Duration = Duration::from_secs(1);
/// A frame larger than this is refused instead of allocating without end.
const MAX_MESSAGE: usize = 64 * 1024 * 1024;

/// The `lsp` tool. `a` is the checked argument object from the schema in `spec.rs`.
pub fn tool(ctx: &Ctx, a: &Value) -> Result<String, String> {
    let action = a["action"].as_str().unwrap_or("").trim();
    let path = a["path"].as_str().unwrap_or("").trim();
    if path.is_empty() {
        return Err("NOT done. lsp needs the path of a Rust file. Set path, for example src/main.rs.".to_string());
    }
    let cwd = a["cwd"].as_str().unwrap_or("");
    match action {
        "diagnostics" => diagnostics(ctx, path, cwd),
        "type" => type_at(ctx, path, &a["line"], &a["column"], cwd),
        "definition" => definition(ctx, path, &a["line"], &a["column"], cwd),
        other => Err(format!("NOT done. lsp action {} is not known. Use diagnostics, type or definition.",
                             if other.is_empty() { "(empty)" } else { other })),
    }
}

/// All errors and warnings of one file, with file:line:column and message.
fn diagnostics(ctx: &Ctx, path: &str, cwd: &str) -> Result<String, String> {
    let file = source_file(path, cwd)?;
    let text = source_text(&file)?;
    let shown = crate::files::show(&file);
    let wait = timeout_of(ctx);
    let mut client = start(ctx, &file, &text)?;
    let uri = file_uri(&file);
    let (mut items, saw_report, timed_out) = client.wait_diagnostics(&uri, wait)?;
    // The server is shut down by Drop as soon as the values are read.
    items.sort_by_key(|d| {
        (d["range"]["start"]["line"].as_u64().unwrap_or(0), d["range"]["start"]["character"].as_u64().unwrap_or(0))
    });
    let mut lines = Vec::new();
    for d in &items {
        let severity = match d["severity"].as_u64().unwrap_or(1) {
            1 => "error",
            2 => "warning",
            3 => "info",
            _ => "hint",
        };
        let line = d["range"]["start"]["line"].as_u64().unwrap_or(0) + 1;
        let column = d["range"]["start"]["character"].as_u64().unwrap_or(0) + 1;
        let code = d["code"].as_str().map(str::to_string)
            .or_else(|| d["code"].as_i64().map(|n| n.to_string()))
            .or_else(|| d["code"]["value"].as_str().map(str::to_string));
        let message = d["message"].as_str().unwrap_or("").replace('\n', " ");
        lines.push(format!("{}:{}:{}: {}{}: {}", shown, line, column, severity,
                           code.map(|c| format!("[{}]", c)).unwrap_or_default(), message));
    }
    if lines.is_empty() {
        if timed_out {
            return Err(format!("lsp diagnostics timed out after {} s for {}. rust-analyzer may still be loading the project; \
                                try again or raise lsp.timeout_seconds in .penelope/config.json.", wait.as_secs(), shown));
        }
        return Ok(format!("no errors or warnings in {} (rust-analyzer reported an empty result).", shown));
    }
    let mut out = format!("diagnostics in {} ({} result{}):\n{}", shown, lines.len(),
                          if lines.len() == 1 { "" } else { "s" }, lines.join("\n"));
    if timed_out {
        out.push_str(&format!("\nnote: the wait timed out after {} s, so the list may be incomplete.", wait.as_secs()));
    } else if !saw_report {
        out.push_str("\nnote: rust-analyzer published no report for this file.");
    }
    Ok(out)
}

/// The type at one position, read from `textDocument/hover`.
fn type_at(ctx: &Ctx, path: &str, line: &Value, column: &Value, cwd: &str) -> Result<String, String> {
    let file = source_file(path, cwd)?;
    let text = source_text(&file)?;
    let (line, column) = point(line, column)?;
    let (lsp_line, lsp_column) = position(&text, line, column)?;
    let shown = crate::files::show(&file);
    let uri = file_uri(&file);
    let mut client = start(ctx, &file, &text)?;
    let answer = client.ask_until(
        "textDocument/hover",
        json!({"textDocument": {"uri": uri}, "position": {"line": lsp_line, "character": lsp_column}}),
        timeout_of(ctx),
        |answer| !answer.is_null() && !answer["contents"].is_null(),
    )?;
    if answer.is_null() || answer["contents"].is_null() {
        return Ok(format!("no type at {}:{}:{} (the position may be whitespace, a keyword or outside a symbol).", shown, line, column));
    }
    let Some(said) = type_line(&answer["contents"]) else {
        return Ok(format!("no type at {}:{}:{} (rust-analyzer returned an empty hover).", shown, line, column));
    };
    Ok(format!("{}:{}:{}: {}", shown, line, column, said))
}

/// Where the symbol at one position is defined, read from `textDocument/definition`.
fn definition(ctx: &Ctx, path: &str, line: &Value, column: &Value, cwd: &str) -> Result<String, String> {
    let file = source_file(path, cwd)?;
    let text = source_text(&file)?;
    let (line, column) = point(line, column)?;
    let (lsp_line, lsp_column) = position(&text, line, column)?;
    let shown = crate::files::show(&file);
    let uri = file_uri(&file);
    let mut client = start(ctx, &file, &text)?;
    let answer = client.ask_until(
        "textDocument/definition",
        json!({"textDocument": {"uri": uri}, "position": {"line": lsp_line, "character": lsp_column}}),
        timeout_of(ctx),
        |answer| !answer.is_null() && answer.as_array().map(|a| !a.is_empty()).unwrap_or(true),
    )?;
    let places = definition_places(&answer);
    if places.is_empty() {
        return Ok(format!("no definition at {}:{}:{} (the position may not name a symbol).", shown, line, column));
    }
    Ok(format!("definition of {}:{}:{}:\n{}", shown, line, column, places.join("\n")))
}

/// The path and text of the file, or a clear refusal for a path that is not a readable Rust file.
fn source_file(path: &str, cwd: &str) -> Result<PathBuf, String> {
    let raw = crate::files::resolve(path, cwd);
    let shown = crate::files::show(&raw);
    if raw.extension().and_then(|e| e.to_str()) != Some("rs") {
        return Err(format!("NOT done. lsp speaks Rust through rust-analyzer; {} is not a .rs file.", shown));
    }
    let file = std::fs::canonicalize(&raw)
        .map_err(|e| format!("NOT done. cannot read {}: {}", shown, e))?;
    if !file.is_file() {
        return Err(format!("NOT done. {} is not a file.", shown));
    }
    Ok(file)
}

fn source_text(file: &Path) -> Result<String, String> {
    std::fs::read_to_string(file)
        .map_err(|e| format!("NOT done. cannot read {} as UTF-8: {}", crate::files::show(file), e))
}

/// The line and column of the model, both counted from 1.
fn point(line: &Value, column: &Value) -> Result<(u64, u64), String> {
    let (line, column) = (line.as_u64().unwrap_or(0), column.as_u64().unwrap_or(0));
    if line == 0 || column == 0 {
        return Err("NOT done. lsp type and definition need line and column, both counted from 1.".to_string());
    }
    Ok((line, column))
}

/// A 1-based character position in a line as the 0-based UTF-16 position of LSP.
fn position(text: &str, line: u64, column: u64) -> Result<(u64, u64), String> {
    let source = text.lines().nth(line as usize - 1).ok_or_else(|| {
        format!("NOT done. line {} is past the end of the file ({} lines).", line, text.lines().count())
    })?;
    let count = source.chars().count();
    if column as usize > count + 1 {
        return Err(format!("NOT done. column {} is past the end of line {} ({} characters).", column, line, count));
    }
    let mut units = 0u64;
    for (i, ch) in source.chars().enumerate() {
        if i + 1 == column as usize {
            return Ok((line - 1, units));
        }
        units += ch.len_utf16() as u64;
    }
    Ok((line - 1, units))
}

/// An LSP error that means "ask again when the index is ready" rather than "this call is wrong".
fn retryable(error: &str) -> bool {
    let said = error.to_ascii_lowercase();
    said.contains("content modified") || said.contains("request cancelled") || said.contains("server cancelled")
}

/// The first type line of a hover answer, plain text or a Markdown code block.
fn type_line(contents: &Value) -> Option<String> {
    let raw = match contents {
        Value::String(s) => s.clone(),
        Value::Object(o) => o["value"].as_str().unwrap_or("").to_string(),
        Value::Array(a) => a.iter().filter_map(|v| {
            v.as_str().map(str::to_string).or_else(|| v["value"].as_str().map(str::to_string))
        }).collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    };
    let mut in_fence = false;
    for line in raw.lines() {
        let said = line.trim();
        if said.starts_with("```") {
            if in_fence {
                break;
            }
            in_fence = true;
            continue;
        }
        if in_fence && !said.is_empty() {
            return Some(said.to_string());
        }
    }
    raw.lines().map(str::trim).find(|l| !l.is_empty()).map(str::to_string)
}

/// Location and LocationLink answers of `textDocument/definition` as path:line:column lines.
fn definition_places(answer: &Value) -> Vec<String> {
    let list = match answer.as_array() {
        Some(a) => a.clone(),
        None if answer.is_null() => Vec::new(),
        None => vec![answer.clone()],
    };
    let mut out = Vec::new();
    for item in list {
        let (uri, range) = if item["uri"].is_string() {
            (&item["uri"], &item["range"])
        } else if item["targetUri"].is_string() {
            (&item["targetUri"], if item["targetSelectionRange"].is_object() { &item["targetSelectionRange"] } else { &item["targetRange"] })
        } else {
            continue;
        };
        let Some(uri) = uri.as_str() else { continue };
        let line = range["start"]["line"].as_u64().unwrap_or(0) + 1;
        let column = range["start"]["character"].as_u64().unwrap_or(0) + 1;
        out.push(format!("{}:{}:{}", crate::files::show(Path::new(&uri_path(uri))), line, column));
    }
    out
}

fn timeout_of(ctx: &Ctx) -> Duration {
    ctx.cfg["lsp"]["timeout_seconds"].as_u64().filter(|s| *s > 0)
        .map(|s| Duration::from_secs(s.min(120))).unwrap_or(DEFAULT_TIMEOUT)
}

/// The configured rust-analyzer, or the one on PATH. A missing program names the install command.
fn server_path(ctx: &Ctx) -> Result<String, String> {
    let configured = ctx.cfg["lsp"]["rust_analyzer"].as_str().unwrap_or("").trim();
    let found = if configured.is_empty() {
        crate::config::which("rust-analyzer")
    } else {
        crate::config::which(configured)
    };
    match found {
        Some(path) => Ok(path.to_string_lossy().to_string()),
        None => {
            let what = if configured.is_empty() { "rust-analyzer".to_string() } else { format!("rust-analyzer at {}", configured) };
            Err(format!("NOT done. {} was not found. Install it with `rustup component add rust-analyzer` \
                         (or `cargo install rust-analyzer`), or set lsp.rust_analyzer in .penelope/config.json.",
                        what))
        }
    }
}

/// Starts rust-analyzer in the cargo root of the file, performs the handshake and opens the file.
fn start(ctx: &Ctx, file: &Path, text: &str) -> Result<Client, String> {
    let command = server_path(ctx)?;
    let root = workspace_root(file);
    let root_uri = file_uri(&root);
    let name = root.file_name().and_then(|n| n.to_str()).unwrap_or("workspace").to_string();
    let mut child = Command::new(&command)
        .current_dir(&root)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("NOT done. rust-analyzer could not start ({}): {}. Install it with \
                              `rustup component add rust-analyzer`.", command, e))?;
    let Some(input) = child.stdin.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err("NOT done. rust-analyzer has no stdin.".to_string());
    };
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err("NOT done. rust-analyzer has no stdout.".to_string());
    };
    let (tx, rx) = channel();
    std::thread::spawn(move || read_loop(stdout, tx));
    let mut client = Client { child, input: Some(input), rx, next: 0, alive: true, ready: false, stopping: false };
    let hello = client.request("initialize", json!({
        "processId": std::process::id(),
        "clientInfo": {"name": "penelope", "version": env!("CARGO_PKG_VERSION")},
        "rootUri": root_uri,
        "workspaceFolders": [{"uri": root_uri, "name": name}],
        "capabilities": {
            "workspace": {"workspaceFolders": true},
            "textDocument": {
                "publishDiagnostics": {"relatedInformation": true, "versionSupport": true},
                "hover": {"contentFormat": ["markdown", "plaintext"]},
                "definition": {"linkSupport": true},
                "diagnostic": {}
            }
        }
    }), INIT_TIMEOUT)?;
    if hello["capabilities"].is_null() {
        return Err(format!("rust-analyzer refused initialize: {}", hello));
    }
    client.notify("initialized", json!({}))?;
    client.ready = true;
    client.notify("textDocument/didOpen", json!({
        "textDocument": {"uri": file_uri(file), "languageId": "rust", "version": 1, "text": text}
    }))?;
    Ok(client)
}

/// The nearest folder above the file that holds a Cargo.toml, else the folder of the file.
fn workspace_root(file: &Path) -> PathBuf {
    for dir in file.ancestors().skip(1) {
        if dir.join("Cargo.toml").is_file() {
            return dir.to_path_buf();
        }
    }
    file.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."))
}

/// A percent-encoded `file://` URI. Spaces in a project path must be encoded, or rust-analyzer cannot find the file.
fn file_uri(path: &Path) -> String {
    let mut text = path.to_string_lossy().replace('\\', "/");
    if !text.starts_with('/') {
        text.insert(0, '/');
    }
    let mut out = String::from("file://");
    for byte in text.as_bytes() {
        match *byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' | b':' => out.push(*byte as char),
            other => out.push_str(&format!("%{:02X}", other)),
        }
    }
    out
}

/// The path of a `file://` URI, percent-decoded.
fn uri_path(uri: &str) -> String {
    let raw = uri.strip_prefix("file://").unwrap_or(uri);
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok());
            if let Some(byte) = hex {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    let mut text = String::from_utf8_lossy(&out).to_string();
    if text.len() >= 3 && text.starts_with('/') && text.as_bytes()[2] == b':' {
        text.remove(0); // /C:/project on Windows
    }
    text
}

/// Reads frames on the reader thread. A body that is not JSON is skipped; the next frame is still read.
fn read_loop(stdout: impl Read, tx: std::sync::mpsc::Sender<Value>) {
    let mut reader = BufReader::new(stdout);
    while let Ok(Some(body)) = read_frame(&mut reader) {
        if let Some(value) = parse_body(&body) {
            if tx.send(value).is_err() {
                break;
            }
        }
    }
}

fn parse_body(body: &[u8]) -> Option<Value> {
    serde_json::from_slice(body).ok()
}

/// One Content-Length frame. Headers are read until the empty line, then exactly the declared number of bytes.
fn read_frame(reader: &mut impl BufRead) -> Result<Option<Vec<u8>>, String> {
    let mut length = None;
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line).map_err(|e| e.to_string())?;
        if n == 0 {
            return Ok(None);
        }
        let header = line.trim();
        if header.is_empty() {
            if length.is_some() {
                break;
            }
            continue;
        }
        if let Some((name, value)) = header.split_once(':') {
            if name.trim().eq_ignore_ascii_case("content-length") {
                length = value.trim().parse::<usize>().ok();
            }
        }
    }
    let length = length.ok_or("a frame without Content-Length")?;
    if length > MAX_MESSAGE {
        return Err(format!("a frame of {} bytes is too large", length));
    }
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body).map_err(|e| e.to_string())?;
    Ok(Some(body))
}

/// One rust-analyzer process and the thread that reads its answers.
struct Client {
    child: Child,
    input: Option<ChildStdin>,
    rx: Receiver<Value>,
    next: u64,
    alive: bool,
    /// True after `initialized`: only then is a graceful `shutdown` worth sending.
    ready: bool,
    stopping: bool,
}

impl Client {
    fn send(&mut self, value: &Value) -> Result<(), String> {
        let Some(input) = self.input.as_mut() else {
            self.alive = false;
            return Err("rust-analyzer has no input".to_string());
        };
        let body = serde_json::to_vec(value).map_err(|e| e.to_string())?;
        write!(input, "Content-Length: {}\r\n\r\n", body.len()).map_err(|e| format!("rust-analyzer stopped reading: {}", e))?;
        input.write_all(&body).and_then(|_| input.flush()).map_err(|e| {
            self.alive = false;
            format!("rust-analyzer stopped reading: {}", e)
        })
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<(), String> {
        self.send(&json!({"jsonrpc": "2.0", "method": method, "params": params}))
    }

    fn request(&mut self, method: &str, params: Value, wait: Duration) -> Result<Value, String> {
        if !self.alive {
            return Err("rust-analyzer is not running".to_string());
        }
        self.next += 1;
        let id = self.next;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))?;
        let deadline = Instant::now() + wait;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                self.alive = false;
                return Err(format!("rust-analyzer did not answer {} within {} s.", method, wait.as_secs()));
            }
            match self.rx.recv_timeout(left) {
                Ok(message) if message["id"].as_u64() == Some(id) => {
                    if let Some(error) = message.get("error") {
                        let said = error["message"].as_str().map(str::to_string).unwrap_or_else(|| error.to_string());
                        return Err(format!("rust-analyzer refused {}: {}", method, said));
                    }
                    return Ok(message["result"].clone());
                }
                Ok(message) => self.answer(&message),
                Err(RecvTimeoutError::Timeout) => {
                    self.alive = false;
                    return Err(format!("rust-analyzer did not answer {} within {} s.", method, wait.as_secs()));
                }
                Err(RecvTimeoutError::Disconnected) => {
                    self.alive = false;
                    return Err(format!("rust-analyzer stopped before it answered {} ({}).", method, self.exit_state()));
                }
            }
        }
    }

    /// A request whose null answer may mean "the project is still loading": ask again until `ok` accepts an answer or
    /// the deadline passes. The last null answer is returned, so the caller can say that nothing was found.
    fn ask_until(&mut self, method: &str, params: Value, wait: Duration, ok: impl Fn(&Value) -> bool) -> Result<Value, String> {
        let deadline = Instant::now() + wait;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(Value::Null);
            }
            let answer = match self.request(method, params.clone(), left) {
                Ok(answer) => answer,
                // rust-analyzer answers ContentModified while it still builds its index. That is a "not ready yet",
                // not a failure, so the same request is sent again until the deadline.
                Err(e) if retryable(&e) && Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(250));
                    continue;
                }
                Err(e) => return Err(e),
            };
            if ok(&answer) || Instant::now() >= deadline {
                return Ok(answer);
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }

    /// Collects `textDocument/publishDiagnostics` for one file. The empty first report is not trusted until it has
    /// stayed quiet for `EMPTY_SETTLE`; a non-empty report is returned after `REPORT_SETTLE`.
    /// Returns the last list, whether any report arrived, and whether the deadline passed first.
    fn wait_diagnostics(&mut self, uri: &str, wait: Duration) -> Result<(Vec<Value>, bool, bool), String> {
        let deadline = Instant::now() + wait;
        let mut diagnostics = Vec::new();
        let mut saw_report = false;
        let mut last_report: Option<Instant> = None;
        let mut non_empty = false;
        loop {
            let now = Instant::now();
            if now >= deadline {
                return Ok((diagnostics, saw_report, true));
            }
            let until = match last_report {
                Some(report) => deadline.min(report + if non_empty { REPORT_SETTLE } else { EMPTY_SETTLE }),
                None => deadline,
            };
            let left = until.saturating_duration_since(now);
            if left.is_zero() {
                return Ok((diagnostics, saw_report, false));
            }
            match self.rx.recv_timeout(left) {
                Ok(message) => {
                    if message["method"].as_str() == Some("textDocument/publishDiagnostics") {
                        let params = &message["params"];
                        if params["uri"].as_str() == Some(uri) {
                            saw_report = true;
                            diagnostics = params["diagnostics"].as_array().cloned().unwrap_or_default();
                            non_empty = !diagnostics.is_empty();
                            last_report = Some(Instant::now());
                        }
                    } else {
                        self.answer(&message);
                    }
                }
                Err(RecvTimeoutError::Timeout) => {
                    // The settle or the deadline is checked at the top of the loop.
                }
                Err(RecvTimeoutError::Disconnected) => {
                    self.alive = false;
                    return Err(format!("rust-analyzer stopped before it published diagnostics ({}).", self.exit_state()));
                }
            }
        }
    }

    /// Answers a request the server sent to the client, so a server waiting for us cannot stall.
    fn answer(&mut self, message: &Value) {
        let Some(method) = message["method"].as_str() else { return };
        let Some(id) = message.get("id") else { return };
        let result = if method == "workspace/configuration" {
            let count = message["params"]["items"].as_array().map(Vec::len).unwrap_or(0);
            Value::Array(vec![Value::Null; count])
        } else {
            Value::Null
        };
        let _ = self.send(&json!({"jsonrpc": "2.0", "id": id.clone(), "result": result}));
    }

    fn exit_state(&mut self) -> String {
        // stdout can close a moment before the process is reaped; give it a short chance to report its code.
        let deadline = Instant::now() + Duration::from_millis(300);
        loop {
            match self.child.try_wait() {
                Ok(Some(status)) => return format!("{}", status),
                Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
                Ok(None) => return "still running".to_string(),
                Err(e) => return format!("exit status unreadable: {}", e),
            }
        }
    }

    /// Closes stdin, asks a healthy server to shut down, gives it a moment, then kills it. Safe to call twice.
    fn stop(&mut self) {
        if self.stopping {
            return;
        }
        self.stopping = true;
        if self.ready && self.alive {
            self.next += 1;
            let id = self.next;
            if self.send(&json!({"jsonrpc": "2.0", "id": id, "method": "shutdown", "params": null})).is_ok() {
                let deadline = Instant::now() + Duration::from_secs(2);
                loop {
                    let left = deadline.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        break;
                    }
                    match self.rx.recv_timeout(left) {
                        Ok(message) if message["id"].as_u64() == Some(id) => break,
                        Ok(message) => self.answer(&message),
                        Err(_) => break,
                    }
                }
                let _ = self.notify("exit", json!({}));
            }
        }
        self.input.take();
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

impl Drop for Client {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("penelope-lsp-{}-{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn project(name: &str, source: &str) -> (PathBuf, crate::config::Ctx) {
        let dir = temp(name);
        fs::write(dir.join("Cargo.toml"), "[package]\nname = \"penelope_lsp_test\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(dir.join("src/main.rs"), source).unwrap();
        let ctx = crate::config::load(&dir.join(".penelope"));
        (dir, ctx)
    }

    fn rust_analyzer() -> Option<PathBuf> {
        let path = crate::config::which("rust-analyzer")?;
        let works = Command::new(&path).arg("--version").output().map(|o| o.status.success()).unwrap_or(false);
        if works { Some(path) } else { None }
    }

    /// A Python stand-in for rust-analyzer. Unix runs the file through its shebang. Windows cannot start a script
    /// directly (os error 193), so the body goes into `NAME.py` and a `NAME.cmd` starts it with `python`.
    fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
        #[cfg(windows)]
        {
            let py = dir.join(format!("{}.py", name));
            fs::write(&py, body).unwrap();
            let cmd = dir.join(format!("{}.cmd", name));
            fs::write(&cmd, format!("@python \"{}\" %*\r\n", py.display())).unwrap();
            cmd
        }
        #[cfg(not(windows))]
        {
            let path = dir.join(name);
            fs::write(&path, body).unwrap();
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
            path
        }
    }

    /// A path with a space and a non-ASCII character survives the round trip through an LSP URI.
    #[test]
    fn a_path_with_a_space_is_a_percent_encoded_uri() {
        let path = Path::new("/tmp/penelope lsp/grüße/main.rs");
        let uri = file_uri(path);
        assert!(uri.starts_with("file:///tmp/penelope%20lsp/"), "{}", uri);
        assert!(uri.contains("%C3%BC"), "{}", uri);
        assert_eq!(uri_path(&uri), path.to_string_lossy());
    }

    /// A malformed body is skipped and the next frame is still read; no panic, no lost stream.
    #[test]
    fn a_malformed_json_frame_does_not_break_the_next_frame() {
        let mut reader = std::io::Cursor::new(b"Content-Length: 9\r\n\r\n{not jsonContent-Length: 2\r\n\r\n{}".to_vec());
        let first = read_frame(&mut reader).unwrap().unwrap();
        assert!(parse_body(&first).is_none());
        let second = read_frame(&mut reader).unwrap().unwrap();
        assert!(parse_body(&second).is_some());
    }

    #[test]
    fn a_missing_binary_names_the_install_command() {
        let dir = temp("missing");
        let file = dir.join("src/main.rs");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, "fn main() {}\n").unwrap();
        let mut ctx = crate::config::load(&dir.join(".penelope"));
        ctx.cfg["lsp"] = json!({"rust_analyzer": dir.join("no-rust-analyzer").display().to_string(), "timeout_seconds": 5});
        let err = tool(&ctx, &json!({"action": "diagnostics", "path": file.display().to_string()})).unwrap_err();
        assert!(err.contains("rust-analyzer") && err.contains("rustup component add rust-analyzer"), "{}", err);
        let _ = fs::remove_dir_all(&dir);
    }

    /// A server that never publishes diagnostics is left after the timeout; the call returns instead of hanging.
    #[test]
    fn a_silent_server_times_out_without_hanging() {
        let dir = temp("silent");
        let script = script(&dir, "silent-lsp", SILENT_SERVER);
        let file = dir.join("src/main.rs");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, "fn main() {}\n").unwrap();
        let mut ctx = crate::config::load(&dir.join(".penelope"));
        ctx.cfg["lsp"] = json!({"rust_analyzer": script.display().to_string(), "timeout_seconds": 1});
        let started = Instant::now();
        let err = tool(&ctx, &json!({"action": "diagnostics", "path": file.display().to_string()})).unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(8), "the timeout took {:?}: {}", started.elapsed(), err);
        assert!(err.contains("timed out"), "{}", err);
        let _ = fs::remove_dir_all(&dir);
    }

    /// A server that exits with a nonzero code is reported with its exit code; the call returns instead of hanging.
    #[test]
    fn a_crashed_server_reports_its_exit_instead_of_hanging() {
        let dir = temp("crash");
        let script = script(&dir, "crash-lsp", CRASH_SERVER);
        let file = dir.join("src/main.rs");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, "fn main() {}\n").unwrap();
        let mut ctx = crate::config::load(&dir.join(".penelope"));
        ctx.cfg["lsp"] = json!({"rust_analyzer": script.display().to_string(), "timeout_seconds": 5});
        let started = Instant::now();
        let err = tool(&ctx, &json!({"action": "diagnostics", "path": file.display().to_string()})).unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(8), "the crash took {:?}: {}", started.elapsed(), err);
        assert!(err.contains("3") && (err.contains("stopped") || err.contains("exit")), "{}", err);
        let _ = fs::remove_dir_all(&dir);
    }

    /// The live test: a planted type error and a valid symbol, against the real rust-analyzer when it is installed.
    #[test]
    fn real_rust_analyzer_finds_the_error_and_the_type() {
        let Some(server) = rust_analyzer() else {
            eprintln!("skipped lsp live test: rust-analyzer is not installed (install it with `rustup component add rust-analyzer`)");
            return;
        };
        let source = "fn main() {\n    let bad: u32 = \"text\";\n    let good: i32 = 5;\n    let _ = good;\n}\n";
        let (dir, mut ctx) = project("live", source);
        let file = dir.join("src/main.rs");
        ctx.cfg["lsp"] = json!({"rust_analyzer": server.display().to_string(), "timeout_seconds": 60});
        let said = tool(&ctx, &json!({"action": "diagnostics", "path": file.display().to_string()})).unwrap();
        assert!(said.contains("mismatched types"), "{}", said);
        assert!(said.contains(":2:") && said.contains("error"), "{}", said);
        let typed = tool(&ctx, &json!({"action": "type", "path": file.display().to_string(), "line": 3, "column": 9})).unwrap();
        assert!(typed.contains("i32"), "{}", typed);
        let defined = tool(&ctx, &json!({"action": "definition", "path": file.display().to_string(), "line": 4, "column": 13})).unwrap();
        assert!(defined.contains(":3:"), "{}", defined);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_roles_that_may_ask_for_a_type_are_lead_worker_and_judge() {
        assert!(crate::route::may_call("lead", "lsp"));
        assert!(crate::route::may_call("worker", "lsp"));
        assert!(crate::route::may_call("judge", "lsp"));
        assert!(!crate::route::may_call("scout", "lsp"));
        assert!(!crate::route::may_call("router", "lsp"));
    }

    const SILENT_SERVER: &str = r#"#!/usr/bin/env python3
import sys, json
def read():
    headers = {}
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        line = line.strip()
        if not line:
            break
        k, _, v = line.decode().partition(":")
        headers[k.lower()] = v.strip()
    body = sys.stdin.buffer.read(int(headers["content-length"]))
    return json.loads(body)
def send(o):
    b = json.dumps(o).encode()
    sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(b) + b)
    sys.stdout.buffer.flush()
for msg in iter(read, None):
    if "id" in msg and msg.get("method") == "initialize":
        send({"jsonrpc": "2.0", "id": msg["id"], "result": {"capabilities": {}}})
"#;

    const CRASH_SERVER: &str = r#"#!/usr/bin/env python3
import sys, json
def read():
    headers = {}
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        line = line.strip()
        if not line:
            break
        k, _, v = line.decode().partition(":")
        headers[k.lower()] = v.strip()
    body = sys.stdin.buffer.read(int(headers["content-length"]))
    return json.loads(body)
def send(o):
    b = json.dumps(o).encode()
    sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(b) + b)
    sys.stdout.buffer.flush()
for msg in iter(read, None):
    if "id" in msg and msg.get("method") == "initialize":
        send({"jsonrpc": "2.0", "id": msg["id"], "result": {"capabilities": {}}})
    elif msg.get("method") == "textDocument/didOpen":
        sys.exit(3)
"#;
}
