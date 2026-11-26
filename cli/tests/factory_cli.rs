//! End-to-end proof for factories without a provider and without the real user scheduler.
//!
//! The tests start a local chat-completions endpoint and use a temporary `.penelope` folder. A `--dry` factory only
//! prints the systemd unit, and `factory run` starts the normal cluster command three times; the fake model appends one
//! line per run. No unit file is written into `~/.config/systemd/user` and no timer is enabled.

use serde_json::{json, Value};
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

fn text(text: &str) -> (u16, String) {
    (200, json!({"choices": [{"finish_reason": "stop", "message": {"role": "assistant", "content": text}}],
                 "usage": {"prompt_tokens": 10, "completion_tokens": 2}}).to_string())
}

fn tool(name: &str, args: Value) -> (u16, String) {
    (200, json!({"choices": [{"finish_reason": "tool_calls", "message": {"role": "assistant", "content": "",
        "tool_calls": [{"id": "call-1", "type": "function", "function": {"name": name, "arguments": args.to_string()}}]}}],
        "usage": {"prompt_tokens": 10, "completion_tokens": 2}}).to_string())
}

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("penelope-factory-cli-{}-{}", name, std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn run_bin(args: &[&str]) -> (bool, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_penelope")).args(args).output().unwrap();
    (out.status.success(), String::from_utf8_lossy(&out.stdout).to_string(), String::from_utf8_lossy(&out.stderr).to_string())
}

fn base_profile(root: &Path, name: &str) {
    std::fs::create_dir_all(root.join("clusters/profiles")).unwrap();
    std::fs::write(root.join("clusters/profiles").join(format!("{}.json", name)), format!(r#"{{
        "name": "{}",
        "workers": [{{"name": "tick", "role": "worker", "model": "main", "task": "Append one line to the tick file."}}],
        "validator": {{"name": "check", "role": "judge", "model": "judge", "task": "Check the tick file."}},
        "max_models": 2, "max_steps": 3, "retries": 0, "worktree": false
    }}"#, name)).unwrap();
}

fn root_with_model(d: &Path, address: &str) -> PathBuf {
    let root = d.join(".penelope");
    std::fs::create_dir_all(&root).unwrap();
    let model = json!({"kind": "openai", "base_url": format!("{}/v1", address), "model": "test-model", "api_key_env": ""});
    let cfg = json!({"main": model, "weak": model, "roles": {"judge": model}, "prices": {"test-model": [1.0, 2.0]},
                     "eur_per_usd": 0.92, "run_policy": "all", "edit_policy": "all", "auto_learn": false, "delegate": "off"});
    std::fs::write(root.join("config.json"), serde_json::to_string_pretty(&cfg).unwrap()).unwrap();
    root
}

#[test]
fn help_names_the_factory_commands() {
    let (ok, out, err) = run_bin(&["--help"]);
    assert!(ok, "{} {}", out, err);
    assert!(out.contains("penelope factory new NAME --profile P (--every 3h | --at HH:MM)"), "{}", out);
    assert!(out.contains("penelope factory list"), "{}", out);
    assert!(out.contains("penelope factory run NAME"), "{}", out);
    assert!(out.contains("penelope factory stop NAME"), "{}", out);
}

#[test]
fn a_dry_factory_prints_the_unit_and_writes_neither_profile_nor_registry() {
    let d = temp("dry");
    let root = d.join(".penelope");
    std::fs::create_dir_all(&root).unwrap();
    base_profile(&root, "base");
    let (ok, out, err) = run_bin(&["--root", root.to_str().unwrap(), "factory", "new", "tick", "--profile", "base", "--every", "3h", "--dry"]);
    assert!(ok, "{} {}", out, err);
    assert!(out.starts_with("Dry run"), "{}", out);
    assert!(out.contains("penelope-factory-tick.service") && out.contains("penelope-factory-tick.timer"), "{}", out);
    assert!(out.contains("OnUnitActiveSec=3h") && out.contains("Unit=penelope-factory-tick.service"), "{}", out);
    assert!(out.contains("cluster") && out.contains("run") && out.contains("--profile") && out.contains("tick"), "{}", out);
    assert!(!root.join("clusters/profiles/tick.json").exists(), "a dry run wrote a factory profile");
    assert!(!root.join("factories.json").exists(), "a dry run wrote the registry");
    let (ok, out, err) = run_bin(&["--root", root.to_str().unwrap(), "factory", "new", "morning", "--profile", "base", "--at", "09:30", "--dry"]);
    assert!(ok, "{} {}", out, err);
    assert!(out.contains("OnCalendar=*-*-* 09:30:00"), "{}", out);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn three_manual_factory_runs_write_three_lines_with_a_fake_local_model() {
    let d = temp("run");
    let ticks = d.join("ticks.txt");
    let append = format!("printf 'tick\\n' >> {}", ticks.display());
    let read = format!("cat {}", ticks.display());
    let mut replies = Vec::new();
    for _ in 0..3 {
        replies.push(tool("run", json!({"command": append})));
        replies.push(text("one tick appended"));
        replies.push(tool("run", json!({"command": read})));
        replies.push(text("PASS: the tick file has the line"));
    }
    let (address, _seen) = fake(replies);
    let root = root_with_model(&d, &address);
    base_profile(&root, "base");

    let (ok, out, err) = run_bin(&["--root", root.to_str().unwrap(), "factory", "new", "ticker", "--profile", "base",
                                   "--every", "1m", "--goal", "append one tick"]);
    assert!(ok, "{} {}", out, err);
    assert!(out.contains("factory ticker created") && out.contains("Not installed yet"), "{}", out);
    assert!(root.join("clusters/profiles/ticker.json").is_file() && root.join("factories.json").is_file());
    let profile = std::fs::read_to_string(root.join("clusters/profiles/ticker.json")).unwrap();
    assert!(profile.contains("\"every\": \"1m\"") && profile.contains("\"purpose\": \"append one tick\""), "{}", profile);

    for run in 1..=3 {
        let (ok, out, err) = run_bin(&["--root", root.to_str().unwrap(), "factory", "run", "ticker"]);
        assert!(ok, "run {} failed: {}\n{}", run, out, err);
        assert!(out.contains(&format!("factory ticker ran · run {} · exit 0", run)), "{}", out);
    }
    let lines: Vec<String> = std::fs::read_to_string(&ticks).unwrap().lines().map(str::to_string).collect();
    assert_eq!(lines, vec!["tick", "tick", "tick"], "three simulated invocations must write three lines");
    let registry = std::fs::read_to_string(root.join("factories.json")).unwrap();
    assert!(registry.contains("\"runs\": 3") && registry.contains("\"last_result\": \"exit 0\""), "{}", registry);
    let log = std::fs::read_to_string(root.join("factories/ticker/runs.log")).unwrap();
    assert_eq!(log.matches("exit=0").count(), 3, "{}", log);
    let _ = std::fs::remove_dir_all(&d);
}
