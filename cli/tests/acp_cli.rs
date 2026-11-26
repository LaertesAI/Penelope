//! End-to-end proof for `penelope acp`: a piped child process, a fake local chat-completions server, JSON-RPC over
//! stdin and stdout, streamed updates, cancel, an unknown method and malformed JSON. No test reaches a real provider.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("penelope-acp-cli-{}-{}", name, std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A chat-completions endpoint that plays the prepared answers one after the other. `delay` waits after the request
/// has arrived, so a test can cancel while the model call is still running.
struct Fake {
    address: String,
    seen: Arc<Mutex<usize>>,
}

fn fake(replies: Vec<(u16, Value, u64)>) -> Fake {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(0usize));
    let counter = seen.clone();
    std::thread::spawn(move || {
        for (status, reply, delay) in replies {
            let Ok((stream, _)) = listener.accept() else { return };
            let mut reader = BufReader::new(stream);
            let (mut first, mut length) = (String::new(), 0usize);
            let _ = reader.read_line(&mut first);
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
                    break;
                }
                if let Some(n) = line.trim().to_lowercase().strip_prefix("content-length:") {
                    length = n.trim().parse().unwrap_or(0);
                }
            }
            let mut body = vec![0u8; length];
            let _ = reader.read_exact(&mut body);
            *counter.lock().unwrap() += 1;
            if delay > 0 {
                std::thread::sleep(Duration::from_millis(delay));
            }
            let reply = serde_json::to_string(&reply).unwrap();
            let mut stream = reader.into_inner();
            let _ = write!(stream, "HTTP/1.1 {} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                           status, reply.len(), reply);
            let _ = stream.flush();
        }
    });
    Fake { address, seen }
}

/// A `.penelope` folder whose main and weak model is the fake endpoint.
fn root_with_model(d: &Path, address: &str) -> PathBuf {
    let root = d.join(".penelope");
    std::fs::create_dir_all(&root).unwrap();
    let model = json!({"kind": "openai", "base_url": format!("{}/v1", address), "model": "test-model", "api_key_env": ""});
    let cfg = json!({"main": model, "weak": model, "prices": {"test-model": [1.0, 2.0]}, "eur_per_usd": 0.92,
                     "edit_policy": "all", "auto_learn": false, "delegate": "off", "privacy": "off", "privacy_answers": false});
    std::fs::write(root.join("config.json"), serde_json::to_string_pretty(&cfg).unwrap()).unwrap();
    root
}

struct Client {
    child: Child,
    stdin: Option<ChildStdin>,
    rx: Receiver<String>,
    stderr: Arc<Mutex<String>>,
    stderr_thread: Option<std::thread::JoinHandle<()>>,
}

impl Client {
    fn start(root: &Path) -> Client {
        let mut child = Command::new(env!("CARGO_BIN_EXE_penelope"))
            .args(["--root", root.to_str().unwrap(), "acp"])
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
            .spawn().unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let stderr_pipe = child.stderr.take().unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    return;
                }
            }
        });
        let stderr = Arc::new(Mutex::new(String::new()));
        let slot = stderr.clone();
        let stderr_thread = std::thread::spawn(move || {
            let mut text = String::new();
            let _ = BufReader::new(stderr_pipe).read_to_string(&mut text);
            *slot.lock().unwrap() = text;
        });
        Client { child, stdin: Some(stdin), rx, stderr, stderr_thread: Some(stderr_thread) }
    }

    fn send(&mut self, message: Value) {
        let text = serde_json::to_string(&message).unwrap();
        let stdin = self.stdin.as_mut().expect("stdin is closed");
        writeln!(stdin, "{}", text).unwrap();
        stdin.flush().unwrap();
    }

    /// One message from stdout. Every line must be a JSON-RPC 2.0 message: stdout is protocol only.
    fn recv(&self) -> Value {
        let line = self.rx.recv_timeout(Duration::from_secs(15)).unwrap_or_else(|_| panic!("no message from penelope acp"));
        let value: Value = serde_json::from_str(&line).unwrap_or_else(|e| panic!("stdout holds no JSON: {:?} ({})", line, e));
        assert_eq!(value["jsonrpc"], "2.0", "stdout holds no JSON-RPC message: {}", line);
        value
    }

    /// Waits for the response with this id and keeps every session/update that arrives before it.
    fn response(&self, id: i64) -> (Value, Vec<Value>) {
        let mut updates = Vec::new();
        loop {
            let value = self.recv();
            if value["method"] == json!("session/update") {
                updates.push(value);
                continue;
            }
            if value["id"] == json!(id) {
                return (value, updates);
            }
            panic!("unexpected message while waiting for {}: {}", id, value);
        }
    }

    fn handshake(&mut self, cwd: &Path) -> String {
        self.send(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
                         "params": {"protocolVersion": 1, "clientCapabilities": {},
                                    "clientInfo": {"name": "acp-test", "version": "1"}}}));
        let (initialized, _) = self.response(1);
        assert_eq!(initialized["result"]["protocolVersion"], 1, "{}", initialized);
        assert_eq!(initialized["result"]["agentInfo"]["name"], "penelope");
        self.send(json!({"jsonrpc": "2.0", "id": 2, "method": "session/new",
                         "params": {"cwd": cwd.to_string_lossy(), "mcpServers": []}}));
        let (new, _) = self.response(2);
        new["result"]["sessionId"].as_str().expect("session/new returned no sessionId").to_string()
    }

    fn prompt(&mut self, id: i64, session: &str, text: &str) {
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": "session/prompt",
                         "params": {"sessionId": session, "prompt": [{"type": "text", "text": text}]}}));
    }

    fn shutdown(&mut self) {
        self.send(json!({"jsonrpc": "2.0", "id": 99, "method": "shutdown"}));
        let (response, _) = self.response(99);
        assert!(response["result"].is_null(), "{}", response);
    }

    fn wait_exit(&mut self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status.success();
            }
            if Instant::now() > deadline {
                let _ = self.child.kill();
                let _ = self.child.wait();
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn stderr_text(&mut self) -> String {
        if let Some(handle) = self.stderr_thread.take() {
            let _ = handle.join();
        }
        self.stderr.lock().unwrap().clone()
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.stdin = None;
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn update_kind(update: &Value) -> &str {
    update["params"]["update"]["sessionUpdate"].as_str().unwrap_or("")
}

fn chunks(updates: &[Value]) -> String {
    updates.iter().filter(|u| update_kind(u) == "agent_message_chunk")
        .filter_map(|u| u["params"]["update"]["content"]["text"].as_str()).collect::<Vec<_>>().join("")
}

#[test]
fn help_names_the_acp_command() {
    let out = Command::new(env!("CARGO_BIN_EXE_penelope")).arg("--help").output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(text.contains("penelope acp") && text.contains("Agent Client Protocol"), "{}", text);
}

#[test]
fn initialize_new_and_prompt_stream_text_and_tool_updates() {
    let d = temp("prompt");
    std::fs::write(d.join("note.txt"), "hello from the note\n").unwrap();
    let args = json!({"path": "note.txt", "cwd": d.to_string_lossy()}).to_string();
    let f = fake(vec![
        (200, json!({"choices": [{"finish_reason": "tool_calls", "message": {"role": "assistant", "content": "I will read the note.",
            "tool_calls": [{"id": "call_1", "type": "function", "function": {"name": "read", "arguments": args}}]}}],
            "usage": {"prompt_tokens": 20, "completion_tokens": 5}}), 0),
        (200, json!({"choices": [{"finish_reason": "stop", "message": {"role": "assistant", "content": "The note says hello."}}],
            "usage": {"prompt_tokens": 30, "completion_tokens": 4}}), 0),
    ]);
    let root = root_with_model(&d, &f.address);
    let mut client = Client::start(&root);
    let session = client.handshake(&d);

    client.prompt(3, &session, "What does note.txt say?");
    let (response, updates) = client.response(3);
    assert_eq!(response["result"]["stopReason"], "end_turn", "{}", response);

    let kinds: Vec<&str> = updates.iter().map(update_kind).collect();
    assert!(kinds.contains(&"agent_message_chunk"), "{:?}", updates);
    assert!(kinds.contains(&"tool_call") && kinds.contains(&"tool_call_update"), "{:?}", updates);
    let text = chunks(&updates);
    assert!(text.contains("I will read the note.") && text.contains("The note says hello."), "{}", text);

    let start = updates.iter().find(|u| update_kind(u) == "tool_call").unwrap();
    let tool = &start["params"]["update"];
    assert_eq!(tool["toolCallId"], "call_1");
    assert_eq!(tool["kind"], "read");
    assert!(tool["title"].as_str().unwrap().contains("read") && tool["title"].as_str().unwrap().contains("note.txt"), "{}", tool);

    let end = updates.iter().find(|u| update_kind(u) == "tool_call_update").unwrap();
    let tool = &end["params"]["update"];
    assert_eq!(tool["status"], "completed");
    assert!(tool["content"][0]["content"]["text"].as_str().unwrap().contains("lines 1 to 1 of 1"), "{}", tool);

    client.shutdown();
    assert!(client.wait_exit(Duration::from_secs(10)), "penelope acp did not exit after shutdown");
    let err = client.stderr_text();
    assert!(!err.contains("panicked"), "{}", err);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn cancel_stops_the_running_turn() {
    let d = temp("cancel");
    let f = fake(vec![
        (200, json!({"choices": [{"finish_reason": "stop", "message": {"role": "assistant", "content": "too late"}}]}), 5000),
    ]);
    let root = root_with_model(&d, &f.address);
    let mut client = Client::start(&root);
    let session = client.handshake(&d);

    client.prompt(3, &session, "This turn must be stopped.");
    let deadline = Instant::now() + Duration::from_secs(10);
    while *f.seen.lock().unwrap() == 0 {
        assert!(Instant::now() < deadline, "the prompt never reached the fake endpoint");
        std::thread::sleep(Duration::from_millis(20));
    }
    let sent = Instant::now();
    client.send(json!({"jsonrpc": "2.0", "method": "session/cancel", "params": {"sessionId": session}}));
    let (response, _) = client.response(3);
    assert_eq!(response["result"]["stopReason"], "cancelled", "{}", response);
    assert!(sent.elapsed() < Duration::from_secs(3), "cancel waited for the model: {:?}", sent.elapsed());

    client.shutdown();
    assert!(client.wait_exit(Duration::from_secs(10)));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn an_unknown_method_is_a_standard_error_and_the_connection_stays_open() {
    let d = temp("unknown");
    let root = d.join(".penelope");
    std::fs::create_dir_all(&root).unwrap();
    let mut client = Client::start(&root);
    client.send(json!({"jsonrpc": "2.0", "id": 4, "method": "fs/read_text_file", "params": {"path": "x"}}));
    let (response, _) = client.response(4);
    assert_eq!(response["error"]["code"], -32601, "{}", response);
    assert!(response["error"]["message"].as_str().unwrap().contains("method not found"), "{}", response);

    client.send(json!({"jsonrpc": "2.0", "id": 5, "method": "initialize", "params": {"protocolVersion": 1}}));
    let (response, _) = client.response(5);
    assert_eq!(response["result"]["protocolVersion"], 1, "{}", response);
    client.shutdown();
    assert!(client.wait_exit(Duration::from_secs(10)));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn malformed_json_ends_the_connection_with_a_clear_stderr_line() {
    let d = temp("malformed");
    let root = d.join(".penelope");
    std::fs::create_dir_all(&root).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_penelope"))
        .args(["--root", root.to_str().unwrap(), "acp"])
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().unwrap();
    child.stdin.take().unwrap().write_all(b"this is not json\n").unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(!out.status.success(), "a malformed line must end the connection");
    assert!(out.stdout.is_empty(), "stdout must stay protocol only: {}", String::from_utf8_lossy(&out.stdout));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("invalid JSON") && err.contains("stdin"), "{}", err);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_content_length_frame_is_answered_in_the_same_framing() {
    let d = temp("headers");
    let root = d.join(".penelope");
    std::fs::create_dir_all(&root).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_penelope"))
        .args(["--root", root.to_str().unwrap(), "acp"])
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());

    let initialize = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": 1}});
    write_frame(&mut stdin, &initialize);
    let answer = read_frame(&mut stdout);
    assert_eq!(answer["result"]["protocolVersion"], 1, "{}", answer);
    assert_eq!(answer["id"], 1);

    write_frame(&mut stdin, &json!({"jsonrpc": "2.0", "id": 2, "method": "shutdown"}));
    let answer = read_frame(&mut stdout);
    assert!(answer["result"].is_null(), "{}", answer);
    drop(stdin);
    let status = child.wait().unwrap();
    assert!(status.success(), "penelope acp ended with {}", status);
    let _ = std::fs::remove_dir_all(&d);
}

fn write_frame(stdin: &mut ChildStdin, message: &Value) {
    let body = serde_json::to_string(message).unwrap();
    write!(stdin, "Content-Length: {}\r\n\r\n{}", body.len(), body).unwrap();
    stdin.flush().unwrap();
}

fn read_frame(stdout: &mut BufReader<ChildStdout>) -> Value {
    let mut length = String::new();
    stdout.read_line(&mut length).unwrap();
    assert!(length.to_ascii_lowercase().starts_with("content-length:"), "no Content-Length header: {:?}", length);
    let n: usize = length.split(':').nth(1).unwrap().trim().parse().unwrap();
    let mut blank = String::new();
    stdout.read_line(&mut blank).unwrap();
    assert!(blank.trim().is_empty(), "no empty line after the header: {:?}", blank);
    let mut body = vec![0u8; n];
    stdout.read_exact(&mut body).unwrap();
    serde_json::from_slice(&body).unwrap()
}
