//! End-to-end proof for `penelope eval` and `penelope replay` without a real provider.
//!
//! The test starts a small local chat-completions endpoint, points a temporary `.penelope` folder at it and runs the
//! built binary. It proves the command line, the table, the `docs/RUNS.md` write and the read-only replay gate.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

struct Seen {
    path: String,
    body: Value,
}

fn fake(replies: Vec<(u16, String)>) -> (String, Arc<Mutex<Vec<Seen>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    std::thread::spawn(move || {
        for (status, reply) in replies {
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
            log.lock().unwrap().push(Seen { path: first.split_whitespace().nth(1).unwrap_or("").to_string(),
                                            body: serde_json::from_slice(&body).unwrap_or(Value::Null) });
            let mut stream = reader.into_inner();
            let _ = write!(stream, "HTTP/1.1 {} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                           status, reply.len(), reply);
            let _ = stream.flush();
        }
    });
    (address, seen)
}

fn reply(text: &str, tin: u64, tout: u64) -> (u16, String) {
    (200, json!({"choices": [{"finish_reason": "stop", "message": {"role": "assistant", "content": text}}],
                 "usage": {"prompt_tokens": tin, "completion_tokens": tout}}).to_string())
}

fn tool_call(name: &str, args: Value, tin: u64, tout: u64) -> (u16, String) {
    (200, json!({"choices": [{"finish_reason": "tool_calls", "message": {"role": "assistant", "content": null, "tool_calls": [
        {"id": "call_1", "type": "function", "function": {"name": name, "arguments": args.to_string()}}]}}],
                 "usage": {"prompt_tokens": tin, "completion_tokens": tout}}).to_string())
}

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("penelope-eval-cli-{}-{}", name, std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn root_with_model(d: &Path, address: &str) -> PathBuf {
    let root = d.join(".penelope");
    std::fs::create_dir_all(&root).unwrap();
    let model = json!({"kind": "openai", "base_url": format!("{}/v1", address), "model": "test-model"});
    let cfg = json!({"main": model, "weak": model, "prices": {"test-model": [1.0, 2.0]}, "eur_per_usd": 0.92,
                     "edit_policy": "all", "auto_learn": false, "delegate": "off"});
    std::fs::write(root.join("config.json"), serde_json::to_string_pretty(&cfg).unwrap()).unwrap();
    root
}

fn run_bin(args: &[&str]) -> (bool, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_penelope")).args(args).output().unwrap();
    (out.status.success(), String::from_utf8_lossy(&out.stdout).to_string(), String::from_utf8_lossy(&out.stderr).to_string())
}

#[test]
fn the_fifty_task_set_has_three_setups_and_fifty_checks() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/eval/tasks50.jsonl");
    let text = std::fs::read_to_string(&path).unwrap();
    let tasks: Vec<Value> = text.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(tasks.len(), 50);
    assert!(tasks.iter().all(|t| t["check"].as_str().map(|c| !c.is_empty()).unwrap_or(false)));
    assert!(tasks.iter().all(|t| t["setup"]["repo"] == "repo50"));
}

#[test]
fn help_lists_eval_and_replay() {
    let (ok, out, err) = run_bin(&["--help"]);
    assert!(ok, "{} {}", out, err);
    assert!(out.contains("penelope eval TASKS.jsonl --setup large|large-subagents|cluster|all"), "{}", out);
    assert!(out.contains("penelope replay ID --model MODEL"), "{}", out);
}

#[test]
fn eval_prints_a_table_writes_runs_and_replay_refuses_a_write() {
    let d = temp("command");
    let repo = d.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::write(repo.join("README.md"), "# task repo\n").unwrap();
    let tasks = d.join("tasks.jsonl");
    std::fs::write(&tasks, format!("{}\n", json!({
        "id": "write-answer",
        "prompt": "Create answer.txt with the single word hello.",
        "setup": {"repo": repo.to_string_lossy()},
        "check": "grep -qx hello answer.txt"
    }))).unwrap();

    let (address, seen) = fake(vec![
        tool_call("write", json!({"path": "answer.txt", "text": "hello\n"}), 100, 10),
        reply("created answer.txt", 120, 5),
    ]);
    let root = root_with_model(&d, &address);
    let runs = d.join("RUNS.md");
    let (ok, out, err) = run_bin(&["--root", root.to_str().unwrap(), "eval", tasks.to_str().unwrap(),
                                   "--setup", "large", "--runs", runs.to_str().unwrap()]);
    assert!(ok, "{} {}", out, err);
    assert!(out.contains("large") && out.contains("write-answer: pass") && out.contains("220 in / 15 out"), "{}", out);
    let doc = std::fs::read_to_string(&runs).unwrap();
    assert!(doc.contains("| large | 1 | 1 | 220 | 15 |"), "{}", doc);
    assert_eq!(seen.lock().unwrap().len(), 2);
    let id = out.lines().find(|l| l.contains("large-")).unwrap().split_whitespace().last().unwrap().to_string();
    assert!(root.join("eval/runs").join(format!("{}.json", id)).is_file());

    // Replay the stored prompt against the same fake endpoint. A write call must be refused and the file absent.
    let forbidden = d.join("forbidden.txt");
    let (address, seen) = fake(vec![
        tool_call("write", json!({"path": forbidden.to_string_lossy(), "text": "nope"}), 50, 5),
        reply("I did not write; replay is read-only.", 60, 6),
    ]);
    let root = root_with_model(&d, &address);
    let (ok, out, err) = run_bin(&["--root", root.to_str().unwrap(), "replay", &id, "--model", "test-model"]);
    assert!(ok, "{} {}", out, err);
    assert!(out.starts_with("REPLAY IS READ-ONLY") && out.contains("refused: write") && out.contains("110 in / 11 out"), "{}", out);
    assert!(!forbidden.exists());
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0].path, "/v1/chat/completions");
    assert!(seen[1].body["messages"].as_array().unwrap().iter().any(|m| m["role"] == "tool" &&
        m["content"].as_str().unwrap_or("").contains("replay is read-only")));
    let _ = std::fs::remove_dir_all(&d);
}

/// A small SHA-256, so the hold-out freeze is checked without a new dependency.
struct Sha256 {
    h: [u32; 8],
    buf: Vec<u8>,
    len: u64,
}

impl Sha256 {
    fn new() -> Sha256 {
        Sha256 {
            h: [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19],
            buf: Vec::new(),
            len: 0,
        }
    }

    fn update(&mut self, data: &[u8]) {
        self.len += data.len() as u64;
        self.buf.extend_from_slice(data);
        while self.buf.len() >= 64 {
            let mut block = [0u8; 64];
            block.copy_from_slice(&self.buf[..64]);
            self.compress(&block);
            self.buf.drain(..64);
        }
    }

    fn finish(mut self) -> String {
        let bits = self.len * 8;
        self.buf.push(0x80);
        while self.buf.len() % 64 != 56 {
            self.buf.push(0);
        }
        self.buf.extend_from_slice(&bits.to_be_bytes());
        let mut i = 0;
        while i < self.buf.len() {
            let mut block = [0u8; 64];
            block.copy_from_slice(&self.buf[i..i + 64]);
            self.compress(&block);
            i += 64;
        }
        self.h.iter().map(|x| format!("{:08x}", x)).collect()
    }

    fn compress(&mut self, block: &[u8; 64]) {
        const K: [u32; 64] = [
            0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
            0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
            0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
            0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
            0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
            0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
            0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
            0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
        ];
        let mut w = [0u32; 64];
        for (i, chunk) in block.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let mut a = self.h[0];
        let mut b = self.h[1];
        let mut c = self.h[2];
        let mut d = self.h[3];
        let mut e = self.h[4];
        let mut f = self.h[5];
        let mut g = self.h[6];
        let mut h = self.h[7];
        for (i, k) in K.iter().enumerate() {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = h.wrapping_add(s1).wrapping_add(ch).wrapping_add(*k).wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        self.h[0] = self.h[0].wrapping_add(a);
        self.h[1] = self.h[1].wrapping_add(b);
        self.h[2] = self.h[2].wrapping_add(c);
        self.h[3] = self.h[3].wrapping_add(d);
        self.h[4] = self.h[4].wrapping_add(e);
        self.h[5] = self.h[5].wrapping_add(f);
        self.h[6] = self.h[6].wrapping_add(g);
        self.h[7] = self.h[7].wrapping_add(h);
    }
}

fn sha256(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    h.finish()
}

#[test]
fn the_swebench_holdout_is_frozen() {
    assert_eq!(sha256(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/eval");
    let dev = std::fs::read(base.join("swebench_dev30.jsonl")).unwrap();
    let hold = std::fs::read(base.join("swebench_holdout20.jsonl")).unwrap();
    let meta = std::fs::read(base.join("swebench50.json")).unwrap();
    assert_eq!(sha256(&dev), "7819859228608f74c7bc1e240c69f0d78df3614921ae69d92f004109404fe94b");
    assert_eq!(sha256(&hold), "626fbf6d14e0d4340bc1b7e6d741b96a5ca437ce3d5f3a18e9427d509b36be90");
    assert_eq!(sha256(&meta), "88238bf835cc7eb727039729bc1bd449a0fd51614c77b580bdee3b728715254b");
    assert_eq!(String::from_utf8(dev).unwrap().lines().count(), 30);
    assert_eq!(String::from_utf8(hold).unwrap().lines().count(), 20);
    let doc = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../predictions.md")).unwrap();
    assert!(doc.contains("626fbf6d14e0d4340bc1b7e6d741b96a5ca437ce3d5f3a18e9427d509b36be90"), "the hash is in predictions.md");
}
