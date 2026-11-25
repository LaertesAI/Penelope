//! End-to-end proof for `penelope cluster run --dry` and a cluster file, without a provider.
//!
//! The test starts a local chat-completions endpoint that would answer one call, writes a `.cluster` profile into a
//! temporary `.penelope` folder and runs the built binary. The dry run must print the parts, their models and the
//! estimate, exit 0, reach no endpoint and leave no cluster or usage file behind.

use serde_json::json;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

struct Seen;

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
            log.lock().unwrap().push(Seen);
            let mut stream = reader.into_inner();
            let _ = write!(stream, "HTTP/1.1 {} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                           status, reply.len(), reply);
            let _ = stream.flush();
        }
    });
    (address, seen)
}

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("penelope-cluster-cli-{}-{}", name, std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn root_with_model(d: &Path, address: &str) -> PathBuf {
    let root = d.join(".penelope");
    std::fs::create_dir_all(root.join("clusters/profiles")).unwrap();
    let model = json!({"kind": "openai", "base_url": format!("{}/v1", address), "model": "test-model", "api_key_env": ""});
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
fn help_names_the_dry_run_and_the_cluster_file() {
    let (ok, out, err) = run_bin(&["--help"]);
    assert!(ok, "{} {}", out, err);
    assert!(out.contains("penelope cluster run GOAL [--profile NAME] [--dry] [--sequential]"), "{}", out);
    assert!(out.contains("--dry prints parts, model and an estimate, starts nothing"), "{}", out);
    assert!(out.contains("(built-in, .json or .cluster)"), "{}", out);
}

#[test]
fn a_dry_run_prints_the_cluster_file_and_exits_zero_without_a_call_or_a_file() {
    let d = temp("dry");
    let (address, seen) = fake(vec![(200, json!({"choices": [{"finish_reason": "stop", "message": {"role": "assistant", "content": "this must not run"}}],
        "usage": {"prompt_tokens": 10, "completion_tokens": 2}}).to_string())]);
    let root = root_with_model(&d, &address);
    std::fs::write(root.join("clusters/profiles/dev.cluster"),
                   "$dev(parts=[researcher, coder*3, validator, tool], max_models=8, max_depth=3, budget_eur=2)").unwrap();

    let (ok, out, err) = run_bin(&["--root", root.to_str().unwrap(), "cluster", "run", "--dry", "--profile", "dev", "build", "a", "bridge"]);
    assert!(ok, "exit was not 0\\n{}\\n{}", out, err);
    assert!(out.starts_with("Dry run"), "{}", out);
    assert!(out.contains("cluster dev"), "{}", out);
    for name in ["researcher", "coder", "coder2", "coder3", "validator", "tool"] {
        assert!(out.contains(name), "{} misses {}: {}", out, name, out);
    }
    assert!(out.contains("model test-model"), "{}", out);
    assert!(out.contains("estimate:") && out.contains("tokens in") && out.contains("EUR"), "{}", out);
    assert!(seen.lock().unwrap().is_empty(), "the dry run reached the model endpoint");
    assert!(!root.join("usage.json").exists(), "the dry run wrote usage.json");
    let state_files: Vec<String> = std::fs::read_dir(root.join("clusters")).unwrap().flatten()
        .map(|e| e.path()).filter(|p| p.extension().and_then(|x| x.to_str()) == Some("json"))
        .map(|p| p.display().to_string()).collect();
    assert!(state_files.is_empty(), "the dry run wrote cluster state: {:?}", state_files);
    assert!(!root.join("worktrees").exists(), "the dry run made a worktree");
    let _ = std::fs::remove_dir_all(&d);
}
