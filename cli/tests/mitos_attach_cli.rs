//! End-to-end proof for Mitos version 1 attachments: a real `penelope mcp` process, JSON-RPC over stdin and stdout.
//! A client with the `attachments` feature gets a resource link and reads ranges with `mitos/fetch`; a client without
//! it gets the text inline and no `mitos/fetch`.

use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("penelope-mitos-attach-cli-{}-{}", name, std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d.canonicalize().unwrap()
}

fn serve(cwd: &Path, lines: &[Value]) -> Vec<Value> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_penelope"))
        .args(["--root", cwd.join(".penelope").to_str().unwrap(), "mcp"])
        .current_dir(cwd).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
    {
        let stdin = child.stdin.as_mut().unwrap();
        for line in lines {
            writeln!(stdin, "{}", line).unwrap();
        }
    }
    let out = child.wait_with_output().unwrap();
    String::from_utf8(out.stdout).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect()
}

fn init(id: u64, manifest: Option<Value>) -> Value {
    let caps = match manifest {
        Some(m) => json!({"experimental": {"laertes/mitos": m}}),
        None => json!({}),
    };
    json!({"jsonrpc": "2.0", "id": id, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": caps}})
}

fn call(id: u64, name: &str, arguments: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": name, "arguments": arguments}})
}

fn fetch(id: u64, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": "mitos/fetch", "params": params})
}

fn big_file(cwd: &Path) {
    let text: String = (1..=300).map(|i| format!("row {:03} of the table, see https://example.org/r{}\n", i, i % 5)).collect();
    std::fs::write(cwd.join("big.txt"), text).unwrap();
}

#[test]
fn a_version_one_client_gets_a_link_and_reads_a_range() {
    let cwd = temp("v1");
    big_file(&cwd);
    let v1 = json!({"version": 1, "features": ["index", "attachments"]});
    let first = serve(&cwd, &[init(1, Some(v1.clone())), call(2, "read", json!({"path": "big.txt"}))]);
    assert_eq!(first[0]["result"]["capabilities"]["experimental"]["laertes/mitos"]["version"], 1);
    assert!(first[0]["result"]["capabilities"]["experimental"]["laertes/mitos"]["features"].as_array().unwrap().contains(&json!("attachments")));
    let item = &first[1]["result"]["content"][0];
    assert_eq!(item["type"], "resource_link");
    let meta = &item["_meta"]["laertes/mitos"];
    assert_eq!(meta["sha256"].as_str().unwrap().len(), 64);
    assert_eq!(meta["l"], 1);
    assert!(meta["summary"].as_str().unwrap().chars().count() <= 200);
    assert_eq!(first[1]["result"]["_meta"]["laertes/mitos"]["l"], 1);
    let uri = item["uri"].as_str().unwrap().to_string();

    // A second process of the same project reads the stored attachment: range, cap, foreign uri.
    let more = serve(&cwd, &[
        init(1, Some(v1.clone())),
        fetch(2, json!({"uri": uri, "bytes": [0, 20]})),
        fetch(3, json!({"uri": uri})),
        fetch(4, json!({"uri": "mitos://attachment/".to_string() + &"0".repeat(64)})),
        fetch(5, json!({"uri": "file:///etc/passwd"})),
        call(6, "attachment_read", json!({"uri": uri, "lines": [2, 2]})),
    ]);
    let ranged = more[1]["result"]["content"][0]["text"].as_str().unwrap();
    assert_eq!(ranged.len(), 20);
    assert!(more[2]["result"]["_meta"]["laertes/mitos"]["returned"].as_u64().unwrap() <= 16384);
    assert_eq!(more[2]["result"]["_meta"]["laertes/mitos"]["truncated"], true);
    assert_eq!(more[3]["error"]["code"], -32602);
    assert!(more[3]["error"]["message"].as_str().unwrap().contains("did not issue"));
    assert_eq!(more[4]["error"]["code"], -32602);
    let line = more[5]["result"]["content"][0]["text"].as_str().unwrap();
    assert!(line.contains("row 001") && !line.contains("row 002"), "{}", line);
    assert!(cwd.join(".penelope/attachments").is_dir());
    let _ = std::fs::remove_dir_all(&cwd);
}

#[test]
fn a_version_zero_client_gets_the_text_and_no_fetch() {
    let cwd = temp("v0");
    big_file(&cwd);
    for manifest in [None, Some(json!({"version": 0, "features": ["index"]}))] {
        let out = serve(&cwd, &[init(1, manifest), call(2, "read", json!({"path": "big.txt"})),
                                fetch(3, json!({"uri": "mitos://attachment/".to_string() + &"1".repeat(64)}))]);
        let item = &out[1]["result"]["content"][0];
        assert_eq!(item["type"], "text");
        assert!(item["text"].as_str().unwrap().contains("row 001"));
        assert_eq!(out[2]["error"]["code"], -32601, "no mitos/fetch without the announcement");
    }
    assert!(!cwd.join(".penelope/attachments").exists(), "nothing was stored");
    let _ = std::fs::remove_dir_all(&cwd);
}
