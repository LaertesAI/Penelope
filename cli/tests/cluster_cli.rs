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
    assert!(out.contains("penelope cluster run GOAL [--profile NAME] [--shadow PROFILE] [--dry] [--sequential]"), "{}", out);
    assert!(out.contains("a shadow profile runs beside the first"), "{}", out);
    assert!(out.contains("prints parts, model and an estimate, starts nothing"), "{}", out);
    assert!(out.contains("penelope cluster advise [NAME]"), "{}", out);
    assert!(out.contains("--apply writes the suggested model into the profile"), "{}", out);
    assert!(out.contains("(built-in, .json or .cluster)"), "{}", out);
}

#[test]
fn a_shadow_flag_runs_a_second_cluster_beside_the_main_one() {
    let d = temp("shadow");
    let probe = d.join("probe.txt");
    std::fs::write(&probe, "base\n").unwrap();
    let (address, _seen) = fake(vec![
        (200, json!({"choices": [{"finish_reason": "stop", "message": {"role": "assistant", "content": "same answer"}}],
            "usage": {"prompt_tokens": 10, "completion_tokens": 2}}).to_string()),
        (200, json!({"choices": [{"finish_reason": "tool_calls", "message": {"role": "assistant", "content": "",
            "tool_calls": [{"id": "c1", "type": "function", "function": {"name": "read", "arguments": json!({"path": probe.clone()}).to_string()}}]}}],
            "usage": {"prompt_tokens": 10, "completion_tokens": 2}}).to_string()),
        (200, json!({"choices": [{"finish_reason": "stop", "message": {"role": "assistant", "content": "PASS: main checked"}}],
            "usage": {"prompt_tokens": 10, "completion_tokens": 2}}).to_string()),
        (200, json!({"choices": [{"finish_reason": "stop", "message": {"role": "assistant", "content": "same answer"}}],
            "usage": {"prompt_tokens": 10, "completion_tokens": 2}}).to_string()),
        (200, json!({"choices": [{"finish_reason": "tool_calls", "message": {"role": "assistant", "content": "",
            "tool_calls": [{"id": "c2", "type": "function", "function": {"name": "read", "arguments": json!({"path": probe}).to_string()}}]}}],
            "usage": {"prompt_tokens": 10, "completion_tokens": 2}}).to_string()),
        (200, json!({"choices": [{"finish_reason": "stop", "message": {"role": "assistant", "content": "PASS: shadow checked"}}],
            "usage": {"prompt_tokens": 10, "completion_tokens": 2}}).to_string()),
    ]);
    let root = root_with_model(&d, &address);
    for (name, model) in [("large", "main"), ("small", "weak")] {
        let profile = json!({
            "name": name,
            "workers": [{"name": "one", "role": "scout", "model": model, "task": "Answer the question."}],
            "validator": {"name": "check", "role": "judge", "model": "judge"},
            "max_models": 2, "max_depth": 1, "retries": 0, "worktree": false
        });
        std::fs::write(root.join(format!("clusters/profiles/{}.json", name)), profile.to_string()).unwrap();
    }
    let (ok, out, err) = run_bin(&["--root", root.to_str().unwrap(), "cluster", "run", "--profile", "large",
                                   "--shadow", "small", "answer", "the", "question"]);
    assert!(ok, "{} {}", out, err);
    assert!(out.contains("SHADOW") && out.contains("match 100.0%"), "{}", out);
    let stored: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(root.join("clusters/shadow.json")).unwrap()).unwrap();
    assert_eq!(stored["runs"][0]["match_rate"].as_f64(), Some(1.0), "{}", stored);
    assert_eq!(stored["runs"][0]["outputs"]["matched"].as_u64(), Some(1));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn cluster_advise_prints_the_scored_suggestion_without_a_model() {
    let d = temp("advise");
    let root = d.join(".penelope");
    std::fs::create_dir_all(root.join("clusters")).unwrap();
    let scores = json!({"coder-4": {"rust-fix": {"accepted": 1, "rejected": 4}},
                        "coder-5": {"rust-fix": {"accepted": 5, "rejected": 0}}});
    std::fs::write(root.join("clusters/scores.json"), serde_json::to_string_pretty(&scores).unwrap()).unwrap();

    let (ok, out, err) = run_bin(&["--root", root.to_str().unwrap(), "cluster", "advise"]);
    assert!(ok, "{} {}", out, err);
    assert_eq!(out.lines().filter(|l| l.contains("was rejected")).count(), 1, "{}", out);
    assert!(out.contains("coder-4 was rejected 4 of 5 times on rust-fix: change its model."), "{}", out);
    assert!(!out.contains("coder-5"), "an accepted part must not get a false suggestion: {}", out);

    let (ok, out, err) = run_bin(&["--root", root.to_str().unwrap(), "cluster", "advise", "coder-4"]);
    assert!(ok, "{} {}", out, err);
    assert!(out.contains("coder-4") && out.contains("4 of 5"), "{}", out);

    let empty = temp("advise-empty");
    let empty_root = empty.join(".penelope");
    std::fs::create_dir_all(&empty_root).unwrap();
    let (ok, out, err) = run_bin(&["--root", empty_root.to_str().unwrap(), "cluster", "advise"]);
    assert!(ok, "{} {}", out, err);
    assert!(out.contains("no cluster scores yet"), "{}", out);
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&empty);
}

#[test]
fn cluster_advise_apply_writes_the_best_model_into_the_profile() {
    let d = temp("advise-apply");
    let root = d.join(".penelope");
    std::fs::create_dir_all(root.join("clusters/profiles")).unwrap();
    let profile = json!({
        "name": "routed",
        "workers": [{"name": "coder-4", "role": "scout", "model": "weak", "task": "Do the task.", "task_type": "rust-fix"}],
        "validator": {"name": "check", "role": "judge", "model": "judge"},
        "max_models": 2, "max_depth": 1, "retries": 0, "worktree": false
    });
    std::fs::write(root.join("clusters/profiles/routed.json"), serde_json::to_string_pretty(&profile).unwrap()).unwrap();
    let scores = json!({"coder-4": {"rust-fix": {
        "accepted": 1, "rejected": 4,
        "models": {"weak": {"accepted": 1, "rejected": 4}, "main": {"accepted": 3, "rejected": 0}}
    }}});
    std::fs::write(root.join("clusters/scores.json"), serde_json::to_string_pretty(&scores).unwrap()).unwrap();

    let (ok, out, err) = run_bin(&["--root", root.to_str().unwrap(), "cluster", "advise", "routed", "--apply"]);
    assert!(ok, "{} {}", out, err);
    assert!(out.contains("weak -> main") && out.contains("next run"), "{}", out);
    let changed: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(root.join("clusters/profiles/routed.json")).unwrap()).unwrap();
    assert_eq!(changed["workers"][0]["model"].as_str(), Some("main"), "{}", changed);
    assert!(!root.join("usage.json").exists(), "advise --apply must not call a model");
    let _ = std::fs::remove_dir_all(&d);
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
