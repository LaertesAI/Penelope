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
    /// Requests this server sent to the editor and still waits for, by id.
    pending: Mutex<HashMap<String, std::sync::mpsc::Sender<Value>>>,
    next_request: AtomicU64,
}

impl Protocol {
    fn new() -> Protocol {
        Protocol { out: Mutex::new(BufWriter::new(std::io::stdout())), headers: AtomicBool::new(false), pending: Mutex::new(HashMap::new()), next_request: AtomicU64::new(1) }
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

    /// Sends a request to the editor and waits for its response. Err holds the JSON-RPC error text, or `timeout`.
    fn request(&self, method: &str, params: Value, timeout: std::time::Duration) -> Result<Value, String> {
        let id = format!("penelope-{}", self.next_request.fetch_add(1, Ordering::SeqCst));
        let (tx, rx) = std::sync::mpsc::channel();
        self.pending.lock().unwrap_or_else(|e| e.into_inner()).insert(id.clone(), tx);
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        let got = rx.recv_timeout(timeout);
        self.pending.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
        let response = got.map_err(|_| "timeout".to_string())?;
        match response.get("error") {
            Some(e) => Err(e["message"].as_str().unwrap_or("the editor refused the request").to_string()),
            None => Ok(response["result"].clone()),
        }
    }

    /// Hands a response of the editor to the request that waits for it. False if no request waits.
    fn deliver(&self, id: &Value, response: Value) -> bool {
        let key = id.as_str().map(String::from).unwrap_or_else(|| id.to_string());
        match self.pending.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
            Some(tx) => tx.send(response).is_ok(),
            None => false,
        }
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
    let stop_asking = Arc::new(AtomicBool::new(false));
    start_ask_poller(&protocol, &ctx.root, &stop_asking);
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
    stop_asking.store(true, Ordering::SeqCst);
    Ok(())
}

/// Sessions that run a turn now. A question that names no session goes to the only one.
static ACTIVE_SESSIONS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Rounds one multi-select question may take, valid or not.
const MAX_ROUNDS: usize = 12;

/// The folder in which the question dialog of the model arrives as files, as in the full screen interface. The tool
/// runs in this process or in an MCP child that inherits PENELOPE_ASK_DIR.
fn start_ask_poller(protocol: &Arc<Protocol>, root: &std::path::Path, stop: &Arc<AtomicBool>) {
    // a folder of a dead ACP process is left over: take it away
    if let Ok(entries) = std::fs::read_dir(root) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if let Some(pid) = name.strip_prefix("ask-acp-").and_then(|p| p.parse::<u32>().ok()) {
                if pid != std::process::id() && !crate::ask::pid_alive(pid) {
                    let _ = std::fs::remove_dir_all(e.path());
                }
            }
        }
    }
    let dir = root.join(format!("ask-acp-{}", std::process::id()));
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    std::env::set_var("PENELOPE_ASK_DIR", &dir);
    let (protocol, stop) = (protocol.clone(), stop.clone());
    std::thread::spawn(move || {
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        while !stop.load(Ordering::SeqCst) {
            if let Ok(entries) = std::fs::read_dir(&dir) {
                for e in entries.flatten() {
                    let name = e.file_name().to_string_lossy().to_string();
                    // an answer whose request is gone came too late
                    if let Some(id) = name.strip_suffix(".ans") {
                        if !dir.join(format!("{}.req", id)).exists() {
                            let _ = std::fs::remove_file(e.path());
                        }
                        continue;
                    }
                    let Some(id) = name.strip_suffix(".req").map(String::from) else { continue };
                    if !seen.insert(id.clone()) {
                        continue;
                    }
                    let body: Value = std::fs::read_to_string(e.path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
                    let (protocol, dir) = (protocol.clone(), dir.clone());
                    std::thread::spawn(move || {
                        let session = body["session"].as_str().filter(|s| !s.is_empty()).map(String::from).unwrap_or_else(|| {
                            let active = ACTIVE_SESSIONS.lock().unwrap_or_else(|e| e.into_inner());
                            if active.len() == 1 { active[0].clone() } else { String::new() }
                        });
                        let (req, ans) = (dir.join(format!("{}.req", id)), dir.join(format!("{}.ans", id)));
                        let answer = answer_questions(&protocol, &session, &body, &req);
                        if req.exists() {                                   // the asker gave up: write no late answer
                            let tmp = dir.join(format!("{}.tmp", id));
                            let _ = std::fs::write(&tmp, answer);
                            let _ = std::fs::rename(&tmp, &ans);
                            if !req.exists() { let _ = std::fs::remove_file(&ans); }
                        }
                    });
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        let _ = std::fs::remove_dir_all(&dir);
    });
}

/// What one reply of the editor did to a question.
#[derive(Debug, PartialEq)]
enum Step {
    /// An option was chosen.
    Picked,
    /// The person ended the question: Done, Skip or Other.
    Ended,
    /// The id named no option, or an option that was chosen before. Nothing changed.
    Ignored,
    /// The person cancelled the whole dialog.
    Cancelled,
}

/// Applies a `session/request_permission` result to the labels picked so far.
fn apply_reply(q: &crate::ask::Question, picked: &mut Vec<String>, result: &Value) -> Step {
    match (result["outcome"]["outcome"].as_str(), result["outcome"]["optionId"].as_str()) {
        (Some("selected"), Some("end")) => Step::Ended,
        (Some("selected"), Some(id)) => match id.parse::<usize>().ok().and_then(|k| q.options.get(k)) {
            Some(o) if !picked.contains(&o.label) => { picked.push(o.label.clone()); Step::Picked }
            _ => Step::Ignored,
        },
        _ => Step::Cancelled,
    }
}

/// The `session/request_permission` params of one round. The model's options are marked as a question from the model and
/// use the `reject_once` kind, so no editor offers them as "Allow". Only the create confirmation of Penelope itself
/// (`confirm`) may use `allow_once`, and only for its yes.
fn permission_params(q: &crate::ask::Question, qi: usize, total: usize, picked: &[String], confirm: bool, session: &str, deadline: u64) -> Value {
    let mut options: Vec<Value> = Vec::new();
    for (k, o) in q.options.iter().enumerate().filter(|(_, o)| !picked.contains(&o.label)) {
        let name = if o.description.is_empty() { o.label.clone() } else { format!("{}: {}", o.label, o.description) };
        let kind = if confirm && o.label.starts_with("Yes") { "allow_once" } else { "reject_once" };
        options.push(json!({"optionId": k.to_string(), "name": name, "kind": kind}));
    }
    let tail = if q.multi_select && !picked.is_empty() { "Done" } else if q.allow_text { "Other (answer in the chat)" } else { "Skip" };
    options.push(json!({"optionId": "end", "name": tail, "kind": "reject_once"}));
    let title = if confirm {
        format!("{}: {}", q.header, q.question)
    } else {
        format!("Question from the model: {} ({}/{}): {}", q.header, qi + 1, total, q.question)
    };
    let call = json!({"toolCallId": format!("ask-{}-{}", deadline, qi), "title": title, "kind": "other", "status": "pending"});
    json!({"sessionId": session, "toolCall": call, "options": options})
}

/// Shows a question set to the editor with `session/request_permission`, one request per question. The model's options
/// are marked as a question from the model and use the `reject_once` kind, so no editor offers them as "Allow". Only the
/// final create confirmation of Penelope itself (`confirm` in the request) uses `allow_once` for its yes. A question
/// with several answers repeats the request until "Done", at most MAX_ROUNDS times. An editor without the method gets
/// the questions as chat text and the model is told to wait for the reply.
fn answer_questions(protocol: &Protocol, session: &str, body: &Value, req: &std::path::Path) -> String {
    use crate::ask::{answers_file, Answer};
    let Ok(qs) = crate::ask::parse_questions(&body["questions"]) else { return String::new() };
    let confirm = body["confirm"].as_bool().unwrap_or(false);
    let deadline = body["deadline"].as_u64().unwrap_or(0);
    let mut answers: Vec<Answer> = Vec::new();
    for (qi, q) in qs.iter().enumerate() {
        let mut picked: Vec<String> = Vec::new();
        for _round in 0..MAX_ROUNDS {
            let left = deadline.saturating_sub(crate::ask::now_secs());
            if !req.exists() || left == 0 {
                return String::new();                                  // the asker gave up: ask nothing more
            }
            let params = permission_params(q, qi, qs.len(), &picked, confirm, session, body["deadline"].as_u64().unwrap_or(0));
            let reply = protocol.request("session/request_permission", params, std::time::Duration::from_secs(left));
            match reply {
                Err(e) if e == "timeout" => return String::new(),
                Err(_) => return fallback_text(protocol, session, &qs),
                Ok(r) => match apply_reply(q, &mut picked, &r) {
                    Step::Picked if !q.multi_select || picked.len() == q.options.len() => break,
                    Step::Picked | Step::Ignored => {}
                    Step::Ended => break,
                    Step::Cancelled => return String::new(),                // the dialog is closed
                },
            }
        }
        answers.push(if picked.is_empty() { Answer::closed() } else { Answer { labels: picked, text: None, closed: false } });
    }
    answers_file(&answers)
}

/// The editor has no permission request. The questions go into the chat and the model ends its turn.
fn fallback_text(protocol: &Protocol, session: &str, qs: &[crate::ask::Question]) -> String {
    let mut text = String::from("Penelope has questions before it goes on. Answer them in your next message:\n");
    for (i, q) in qs.iter().enumerate() {
        text.push_str(&format!("\n{}. {}\n", i + 1, q.question));
        for (k, o) in q.options.iter().enumerate() {
            text.push_str(&format!("   {}) {}{}\n", (b'a' + k as u8) as char, o.label, if o.description.is_empty() { String::new() } else { format!(": {}", o.description) }));
        }
    }
    protocol.update(session, json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": text}}));
    json!({"text_fallback": "the questions were posted in the chat because the editor cannot show a dialog. End your turn now and wait for the answers in the next message."}).to_string()
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
            // A response to a request of this server (the question dialog). Other responses are ignored.
            if obj.contains_key("result") || obj.contains_key("error") {
                if let Some(id) = &id {
                    self.protocol.deliver(id, value.clone());
                }
            } else {
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
        ACTIVE_SESSIONS.lock().unwrap_or_else(|e| e.into_inner()).push(session_id.clone());
        std::thread::spawn(move || {
            let outcome = crate::ask::with_session(&session_id, || run_turn(&ctx, &session, &session_id, &text, &protocol));
            ACTIVE_SESSIONS.lock().unwrap_or_else(|e| e.into_inner()).retain(|s| s != &session_id);
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
    // The editor names the project folder of the session. Tools without a cwd work there. The folder is a
    // thread-local task cwd, never the process cwd: two sessions and the tests run at the same time.
    if !session.cwd.is_empty() && std::path::Path::new(&session.cwd).is_dir() {
        return crate::eval::with_task_cwd(std::path::Path::new(&session.cwd),
            || run_turn_in(ctx, session, session_id, text, protocol));
    }
    run_turn_in(ctx, session, session_id, text, protocol)
}

fn run_turn_in(ctx: &Ctx, session: &Session, session_id: &str, text: &str, protocol: &Protocol) -> Result<String, String> {
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
    fn a_response_of_the_editor_reaches_the_waiting_request() {
        let protocol = Arc::new(Protocol::new());
        let p2 = protocol.clone();
        let waiter = std::thread::spawn(move || p2.request("session/request_permission", json!({}), std::time::Duration::from_secs(5)));
        for _ in 0..50 {
            std::thread::sleep(std::time::Duration::from_millis(20));
            if protocol.deliver(&json!("penelope-1"), json!({"jsonrpc": "2.0", "id": "penelope-1", "result": {"outcome": {"outcome": "selected", "optionId": "1"}}})) {
                break;
            }
        }
        assert_eq!(waiter.join().unwrap().unwrap()["outcome"]["optionId"], "1");
        assert!(!protocol.deliver(&json!("penelope-9"), json!({})), "no request waits for this id");
        let lone = Protocol::new();
        assert_eq!(lone.request("x", json!({}), std::time::Duration::from_millis(30)), Err("timeout".to_string()));
    }

    fn pick_question(multi: bool) -> crate::ask::Question {
        crate::ask::parse_questions(&json!([{"question": "Which?", "header": "H", "multi_select": multi, "options": ["a", "b", "c"]}])).unwrap().remove(0)
    }

    fn selected(id: &str) -> Value {
        json!({"outcome": {"outcome": "selected", "optionId": id}})
    }

    #[test]
    fn an_invalid_or_repeated_option_id_changes_nothing() {
        let q = pick_question(true);
        let mut picked = Vec::new();
        assert_eq!(apply_reply(&q, &mut picked, &selected("abc")), Step::Ignored);
        assert_eq!(apply_reply(&q, &mut picked, &selected("99")), Step::Ignored);
        assert_eq!(apply_reply(&q, &mut picked, &selected("1")), Step::Picked);
        assert_eq!(apply_reply(&q, &mut picked, &selected("1")), Step::Ignored);
        assert_eq!(picked, vec!["b"]);
        assert_eq!(apply_reply(&q, &mut picked, &selected("end")), Step::Ended);
        assert_eq!(apply_reply(&q, &mut picked, &json!({"outcome": {"outcome": "cancelled"}})), Step::Cancelled);
    }

    /// Plays an editor that answers every request with `reply` and returns the answer file and the requests it saw.
    fn with_editor(body: Value, reply: Value) -> (String, Vec<Value>) {
        let protocol = Arc::new(Protocol::new());
        let dir = std::env::temp_dir().join(format!("penelope-acp-{}-{:?}", std::process::id(), std::thread::current().id()));
        std::fs::create_dir_all(&dir).unwrap();
        let req = dir.join("1.req");
        std::fs::write(&req, "{}").unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (p2, seen2, stop) = (protocol.clone(), seen.clone(), Arc::new(AtomicBool::new(false)));
        let stop2 = stop.clone();
        let editor = std::thread::spawn(move || {
            let mut n = 1;
            while !stop2.load(Ordering::SeqCst) {
                let id = format!("penelope-{}", n);
                let pending = p2.pending.lock().unwrap().contains_key(&id);
                if pending {
                    p2.deliver(&json!(id), json!({"jsonrpc": "2.0", "id": id, "result": reply.clone()}));
                    seen2.lock().unwrap().push(json!(n));
                    n += 1;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        });
        let out = answer_questions(&protocol, "s1", &body, &req);
        stop.store(true, Ordering::SeqCst);
        editor.join().unwrap();
        let n = seen.lock().unwrap().len();
        let _ = std::fs::remove_dir_all(&dir);
        (out, vec![json!(n)])
    }

    #[test]
    fn a_multi_select_loop_is_bounded_when_the_editor_keeps_sending_bad_ids() {
        let body = json!({"questions": [{"question": "Which?", "header": "H", "multi_select": true, "options": ["a", "b"]}], "deadline": crate::ask::now_secs() + 60});
        for bad in ["abc", "99"] {
            let (out, n) = with_editor(body.clone(), selected(bad));
            assert_eq!(n[0], MAX_ROUNDS, "{} must stop after {} rounds", bad, MAX_ROUNDS);
            assert!(out.contains("\"closed\":true"), "{}", out);
        }
        let (out, n) = with_editor(body, selected("0"));
        assert_eq!(n[0], MAX_ROUNDS, "a repeated id stops after {} rounds", MAX_ROUNDS);
        assert_eq!(out.matches("\"a\"").count(), 1, "a is chosen once: {}", out);
    }

    #[test]
    fn model_options_never_look_like_an_approval_and_the_title_says_who_asks() {
        let q = crate::ask::parse_questions(&json!([{"question": "Allow it?", "header": "Create?", "options": ["Yes, create it", "No"]}])).unwrap().remove(0);
        let model = permission_params(&q, 0, 1, &[], false, "s1", 5);
        assert!(model["toolCall"]["title"].as_str().unwrap().starts_with("Question from the model:"), "{}", model);
        assert!(model["options"].as_array().unwrap().iter().all(|o| o["kind"] == "reject_once"), "{}", model);
        let own = permission_params(&q, 0, 1, &[], true, "s1", 5);
        assert!(!own["toolCall"]["title"].as_str().unwrap().starts_with("Question from the model"));
        let kinds: Vec<&str> = own["options"].as_array().unwrap().iter().map(|o| o["kind"].as_str().unwrap()).collect();
        assert_eq!(kinds, vec!["allow_once", "reject_once", "reject_once"], "only the yes of Penelope is an approval");
        assert_eq!(model["sessionId"], "s1");
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
