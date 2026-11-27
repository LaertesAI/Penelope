//! The work modes through the real binary: the MCP server of a `claude` turn follows `PENELOPE_MODE`, and a bad
//! `--mode` is a usage error.

use serde_json::Value;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn project(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("penelope-modes-cli-{}-{}", name, std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join(".penelope")).unwrap();
    d.canonicalize().unwrap()
}

fn run(dir: &PathBuf, env: &[(&str, &str)], args: &[&str], stdin: &str) -> (i32, String, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_penelope"));
    cmd.args(["--root", dir.join(".penelope").to_str().unwrap()]).args(args).current_dir(dir)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().unwrap();
    let _ = child.stdin.take().unwrap().write_all(stdin.as_bytes());
    let out = child.wait_with_output().unwrap();
    (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).to_string(), String::from_utf8_lossy(&out.stderr).to_string())
}

fn tool_names(out: &str) -> Vec<String> {
    let line = out.lines().find(|l| l.contains("\"tools\"")).unwrap_or("{}");
    let v: Value = serde_json::from_str(line).unwrap();
    v["result"]["tools"].as_array().map(|a| a.iter().filter_map(|t| t["name"].as_str().map(String::from)).collect()).unwrap_or_default()
}

#[test]
fn the_mcp_server_offers_the_tools_of_the_mode_in_its_environment_and_refuses_the_others() {
    let dir = project("mcp");
    let list = "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/list\"}\n";
    let call = "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"run\",\"arguments\":{\"command\":\"ls\"}}}\n";
    let (code, out, _) = run(&dir, &[("PENELOPE_MODE", "chat")], &["mcp"], &format!("{}{}", list, call));
    assert_eq!(code, 0);
    let mut names = tool_names(&out);
    names.sort();
    assert_eq!(names, vec!["ask_user", "memory_find"], "{}", out);
    assert!(out.contains("NOT called. Mode chat has no tool run"), "{}", out);
    // Without the variable the lead keeps every tool, hand_off included.
    let (_, plain, _) = run(&dir, &[], &["mcp"], list);
    assert!(tool_names(&plain).iter().any(|n| n == "hand_off") && tool_names(&plain).len() > 20, "{}", plain);
    // Work mode: file and web tools, no cluster tools.
    let (_, work, _) = run(&dir, &[("PENELOPE_MODE", "work")], &["mcp"], list);
    let names = tool_names(&work);
    assert!(names.iter().any(|n| n == "read") && names.iter().any(|n| n == "deep_research") && !names.iter().any(|n| n == "cluster_start"), "{:?}", names);
}

#[test]
fn a_bad_mode_name_is_a_usage_error_for_ask_and_chat() {
    let dir = project("usage");
    for args in [&["ask", "--mode", "kitchen", "hello"][..], &["chat", "--mode", "kitchen", "--plain"], &["ask", "--mode"]] {
        let (code, out, err) = run(&dir, &[], args, "");
        assert_eq!(code, 2, "{:?} {} {}", args, out, err);
        assert!(err.contains("usage: penelope") && err.contains("chat|work|agents|code"), "{:?} {}", args, err);
    }
}
