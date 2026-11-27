//! End-to-end proof for `penelope mcp serve kerkis` and `penelope mcp serve vet`: a real child process, JSON-RPC over
//! stdin and stdout, the start flags, a bad line and an unknown profile.

use serde_json::{json, Value};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("penelope-mcp-servers-cli-{}-{}", name, std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d.canonicalize().unwrap()
}

/// Runs the server with the lines on stdin and returns the answers. The child exits when stdin ends.
fn serve(cwd: &PathBuf, args: &[&str], lines: &[Value]) -> Vec<Value> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_penelope"))
        .args(["--root", cwd.join(".penelope").to_str().unwrap(), "mcp", "serve"]).args(args)
        .current_dir(cwd).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
    {
        let stdin = child.stdin.as_mut().unwrap();
        for line in lines {
            writeln!(stdin, "{}", line).unwrap();
        }
        writeln!(stdin, "this is not json").unwrap();
        writeln!(stdin, "{}", json!({"jsonrpc": "2.0", "id": 99, "method": "ping"})).unwrap();
    }
    let out = child.wait_with_output().unwrap();
    String::from_utf8(out.stdout).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect()
}

fn call(id: u64, name: &str, arguments: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": name, "arguments": arguments}})
}

#[test]
fn the_kerkis_server_runs_as_a_process_and_guards_level_three() {
    let cwd = temp("kerkis");
    let add = |level: u64| json!({"ENT": "build", "ATT": "tool", "VAL": "cargo", "level": level, "source": "Cargo.toml"});
    let lines = [call(1, "memory_add", add(2)), call(2, "memory_add", add(3)), call(3, "memory_search", json!({"query": "cargo"}))];
    let out = serve(&cwd, &["kerkis"], &lines);
    assert_eq!(out[0]["result"]["isError"], false);
    assert_eq!(out[1]["result"]["isError"], true, "level 3 without --allow-person");
    let found: Value = serde_json::from_str(out[2]["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(found["total"], 1);
    assert_eq!(out[3]["error"]["code"], -32700);
    assert_eq!(out[4]["id"], 99, "the server answers after the bad line");

    let allowed = serve(&cwd, &["kerkis", "--allow-person"], &[call(1, "memory_add", add(3))]);
    assert_eq!(allowed[0]["result"]["_meta"]["laertes/mitos"]["l"], 3);
    let _ = std::fs::remove_dir_all(&cwd);
}

#[test]
fn the_vet_server_runs_as_a_process_and_stays_inside_the_workspace() {
    let cwd = temp("vet");
    std::fs::write(cwd.join("SKILL.md"), "---\nname: a\ndescription: b\n---\nunzip -P secret archive.zip\n").unwrap();
    let outside = temp("vet-outside");
    let lines = [call(1, "vet_scan_path", json!({"path": "SKILL.md"})), call(2, "vet_scan_path", json!({"path": outside.to_str().unwrap()}))];
    let out = serve(&cwd, &["vet"], &lines);
    let body: Value = serde_json::from_str(out[0]["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(body["blocked"], true);
    assert_eq!(body["blocking"][0]["line"], 5);
    assert_eq!(out[1]["result"]["isError"], true);
    let open = serve(&cwd, &["vet", "--scan-root", outside.to_str().unwrap()], &[lines[1].clone()]);
    assert_eq!(open[0]["result"]["isError"], false);
    let _ = std::fs::remove_dir_all(&cwd);
    let _ = std::fs::remove_dir_all(&outside);
}

#[test]
fn an_unknown_profile_or_option_exits_with_code_2() {
    let cwd = temp("unknown");
    for args in [vec!["nope"], vec!["kerkis", "--allow-persons"], vec!["vet", "--scan-root"]] {
        let status = Command::new(env!("CARGO_BIN_EXE_penelope")).args(["--root", cwd.join(".penelope").to_str().unwrap(), "mcp", "serve"]).args(&args)
            .current_dir(&cwd).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().unwrap();
        assert_eq!(status.code(), Some(2), "{:?}", args);
    }
    let _ = std::fs::remove_dir_all(&cwd);
}
