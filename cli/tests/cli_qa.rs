//! Regression tests from the command line walk-through: each one runs the real binary in a scratch project.

use serde_json::{json, Value};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn project(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("penelope-cli-qa-{}-{}", name, std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join(".penelope")).unwrap();
    d.canonicalize().unwrap()
}

/// Runs the binary in the scratch project with the given stdin; returns (exit code, stdout, stderr).
fn run(dir: &PathBuf, args: &[&str], stdin: &[u8]) -> (i32, String, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_penelope"))
        .args(["--root", dir.join(".penelope").to_str().unwrap()]).args(args)
        .current_dir(dir).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    // The child may exit before it reads (a usage error); a closed pipe is then the expected end, not a failure.
    let _ = child.stdin.take().unwrap().write_all(stdin);
    let out = child.wait_with_output().unwrap();
    (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).to_string(), String::from_utf8_lossy(&out.stderr).to_string())
}

#[test]
fn version_prints_the_package_version() {
    let dir = project("version");
    for flag in ["--version", "-V", "version"] {
        let (code, out, _) = run(&dir, &[flag], b"");
        assert_eq!(code, 0, "{}", flag);
        assert_eq!(out.trim(), format!("penelope {}", env!("CARGO_PKG_VERSION")));
    }
}

#[test]
fn an_incomplete_mcp_verb_is_a_usage_error_and_does_not_start_the_server() {
    let dir = project("mcpverb");
    for args in [&["mcp", "add"][..], &["mcp", "add", "x"], &["mcp", "deny-tool", "x"], &["mcp", "call"], &["mcp", "bogus"]] {
        // Stdin stays open with a request on it: a server would answer it, a usage error must not.
        let (code, out, err) = run(&dir, args, b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n");
        assert_eq!(code, 2, "{:?} {} {}", args, out, err);
        assert!(out.is_empty(), "{:?} answered as a server: {}", args, out);
        assert!(err.contains("usage: penelope mcp"), "{:?} {}", args, err);
    }
}

#[test]
fn the_mcp_tool_server_survives_a_line_that_is_not_utf8_not_json_or_not_an_object() {
    let dir = project("mcputf8");
    let mut input: Vec<u8> = Vec::new();
    input.extend_from_slice(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n");
    input.extend_from_slice(b"{\"id\":2,\"method\":\"ping\",\"x\":\"\xff\xfe\"}\n");
    input.extend_from_slice(b"{not json\n");
    input.extend_from_slice(b"[]\n");
    input.extend_from_slice(b"{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"ping\"}\n");
    let (code, out, _) = run(&dir, &["mcp"], &input);
    assert_eq!(code, 0);
    let answers: Vec<Value> = out.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(answers.len(), 5, "{:?}", answers);
    assert_eq!(answers[0]["id"], json!(1));
    assert_eq!(answers[1]["error"]["code"], json!(-32700));
    assert_eq!(answers[2]["error"]["code"], json!(-32700));
    assert_eq!(answers[3]["error"]["code"], json!(-32600));
    assert_eq!(answers[4]["id"], json!(3), "the line after the bad ones is still served");
}

#[test]
fn an_empty_memory_note_is_refused() {
    let dir = project("note");
    let (code, out, err) = run(&dir, &["memory", "note"], b"");
    assert_eq!(code, 2, "{} {}", out, err);
    let (code, _, _) = run(&dir, &["memory", "note", "--path", "a/b", "  "], b"");
    assert_eq!(code, 2);
    let (code, out, _) = run(&dir, &["memory", "note", "keep this"], b"");
    assert_eq!(code, 0);
    assert!(out.contains("noted at"), "{}", out);
}

#[test]
fn a_price_must_be_a_finite_non_negative_number_for_a_named_model() {
    let dir = project("price");
    for bad in [&["price", "m", "nan", "1"][..], &["price", "m", "inf", "1"], &["price", "m", "-1", "2"], &["price", "", "1", "2"]] {
        let (code, _, _) = run(&dir, bad, b"");
        assert_eq!(code, 2, "{:?}", bad);
    }
    let (code, out, _) = run(&dir, &["price", "m", "1.5", "2"], b"");
    assert_eq!(code, 0, "{}", out);
    let config: Value = serde_json::from_str(&std::fs::read_to_string(dir.join(".penelope/config.json")).unwrap()).unwrap();
    assert_eq!(config["prices"]["m"], json!([1.5, 2.0]));
}

#[test]
fn ask_without_a_question_is_a_usage_error_before_any_model_starts() {
    let dir = project("ask");
    for args in [&["ask"][..], &["ask", ""], &["ask", "--json"], &["ask", "--json", "  "]] {
        let (code, out, err) = run(&dir, args, b"");
        assert_eq!(code, 2, "{:?} {} {}", args, out, err);
        assert!(err.contains("usage: penelope ask"), "{}", err);
        assert!(out.is_empty(), "{}", out);
    }
}

#[test]
fn cursor_refuses_a_coordinate_that_is_not_a_number() {
    let dir = project("cursor");
    for args in [&["cursor", "a", "b"][..], &["cursor", "1", "x"]] {
        let (code, _, err) = run(&dir, args, b"");
        assert_eq!(code, 2, "{:?}", args);
        assert!(err.contains("usage: penelope cursor X Y"), "{}", err);
    }
}

#[test]
fn run_agent_refuses_an_agent_that_was_never_stored() {
    let dir = project("runagent");
    let (code, out, err) = run(&dir, &["run-agent", "nosuch", "--goal", "x"], b"");
    assert_eq!(code, 2, "{} {}", out, err);
    assert!(err.contains("no stored agent nosuch"), "{}", err);
}

#[test]
fn a_refusal_exits_2_for_plugins_mcp_servers_and_worktrees() {
    let dir = project("refusals");
    for args in [
        &["plugin", "remove", "nosuch"][..], &["plugin", "allow", "nosuch"], &["plugin", "deny", "nosuch"], &["plugin", "update", "nosuch"],
        &["plugin", "add", "/nonexistent"], &["mcp", "remove", "nosuch"], &["mcp", "allow", "nosuch"], &["mcp", "allow-tool", "nosuch", "t"],
        &["mcp", "call", "nosuch", "t", "{}"], &["work", "diff", "nosuch"], &["work", "apply", "nosuch"], &["jobs", "stop", "99"],
    ] {
        let (code, out, err) = run(&dir, args, b"");
        assert_eq!(code, 2, "{:?} {} {}", args, out, err);
    }
}

#[test]
fn app_install_and_update_check_the_target_before_the_yes_message() {
    let dir = project("appcheck");
    for args in [&["app", "install", "/nonexistent/app"][..], &["app", "install", "/nonexistent/app", "--yes"], &["app", "update", "nosuch"]] {
        let (code, out, err) = run(&dir, args, b"");
        assert_eq!(code, 2, "{:?} {} {}", args, out, err);
        assert!(!err.contains("A pipe cannot answer yes"), "{:?} {}", args, err);
    }
}

#[test]
fn bad_values_get_a_usage_line_and_exit_2() {
    let dir = project("usage");
    for args in [
        &["jobs", "stop", "abc"][..], &["jobs", "stop"], &["jobs", "bogus"], &["search", "fanout", "maybe"], &["input", "bogus"],
        &["serve", "--bind", "nonsense", "--print-config"], &["serve", "--bind", "host:0", "--print-config"],
        &["provider", "use", "claude-cli", "--workspace"], &["provider", "use", "claude-cli", "--model"],
        &["plugin", "add"], &["plugin", "deny"], &["plugin", "allow"], &["chat", "--resume"], &["work", "diff"],
    ] {
        let (code, out, err) = run(&dir, args, b"");
        assert_eq!(code, 2, "{:?} {} {}", args, out, err);
        let said = format!("{}{}", out, err);
        assert!(said.contains("usage") || said.contains("needs") || said.contains("not HOST:PORT"), "{:?} {}", args, said);
    }
    let (code, _, _) = run(&dir, &["serve", "--bind", "127.0.0.1:9", "--print-config"], b"");
    assert_eq!(code, 0);
    let (code, out, _) = run(&dir, &["jobs"], b"");
    assert_eq!(code, 0, "{}", out);
}

#[test]
fn an_unlisted_model_is_set_with_a_warning() {
    let dir = project("modelwarn");
    let (code, out, _) = run(&dir, &["provider", "use", "opencode", "--model", "brand-new-model"], b"");
    assert_eq!(code, 0, "{}", out);
    assert!(out.contains("Warning: brand-new-model"), "{}", out);
    let (_, out, _) = run(&dir, &["provider", "use", "claude-cli", "--model", "haiku"], b"");
    assert!(!out.contains("Warning"), "{}", out);
}

#[test]
fn the_mcp_server_with_root_refuses_a_file_outside_the_project() {
    let dir = project("mcproot");
    std::fs::write(dir.join("a.txt"), "inside").unwrap();
    let call = |id: u64, path: &str| json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": "read", "arguments": {"path": path}}});
    let input = format!("{}\n{}\n{}\n", call(1, "a.txt"), call(2, "/etc/hostname"), call(3, "../../outside"));
    let (code, out, _) = run(&dir, &["mcp"], input.as_bytes());
    assert_eq!(code, 0);
    let answers: Vec<Value> = out.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert!(answers[0]["result"]["content"][0]["text"].as_str().unwrap().contains("inside"), "{}", answers[0]);
    for a in &answers[1..] {
        assert_eq!(a["result"]["isError"], json!(true), "{}", a);
        assert!(a["result"]["content"][0]["text"].as_str().unwrap().contains("outside the project"), "{}", a);
    }
}
