//! D-02: one turn over the core API, without the terminal interface.
//!
//! The endpoint is a local socket in this test. No provider is called, no key leaves the machine and no paid request
//! happens.

use penelope_core::{api, backend, providers, sessions};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::TcpListener;

fn fake_openai(answer: &str) -> (String, std::thread::JoinHandle<Value>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let body = json!({
        "choices": [{"finish_reason": "stop", "message": {"role": "assistant", "content": answer}}],
        "usage": {"prompt_tokens": 21, "completion_tokens": 7}
    })
    .to_string();
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut raw = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            let n = stream.read(&mut buf).unwrap_or(0);
            if n == 0 {
                break;
            }
            raw.extend_from_slice(&buf[..n]);
            let text = String::from_utf8_lossy(&raw);
            if let Some(head) = text.split("\r\n\r\n").next() {
                let len = head
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                    })
                    .unwrap_or(0);
                if raw.len() >= head.len() + 4 + len {
                    break;
                }
            }
        }
        let text = String::from_utf8_lossy(&raw).to_string();
        let asked: Value = text
            .split("\r\n\r\n")
            .nth(1)
            .and_then(|b| serde_json::from_str(b).ok())
            .unwrap_or(Value::Null);
        let reply = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body);
        let _ = stream.write_all(reply.as_bytes());
        asked
    });
    (address, handle)
}

#[test]
fn one_turn_runs_over_the_core_api_and_reports_serde_events() {
    let dir = std::env::temp_dir().join(format!("penelope-core-api-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (address, server) = fake_openai("Hello from the core.");
    let mut ctx = api::load(&dir);
    ctx.cfg["main"] = json!({"kind": "openai", "base_url": format!("{}/v1", address), "model": "test-model", "api_key_env": "PENELOPE_CORE_TEST_KEY"});
    ctx.cfg["auto_learn"] = json!(false);
    std::env::set_var("PENELOPE_CORE_TEST_KEY", "test-key-0123456789");

    let mut chat = backend::Chat::new();
    let mut events: Vec<Value> = Vec::new();
    let answer = api::chat(&ctx, &mut chat, "Say hello", "", &mut |event| {
        events.push(api::event_json(&event))
    })
    .unwrap();
    assert_eq!(answer, "Hello from the core.");
    assert!(events.iter().all(|e| e["type"].is_string()), "{:?}", events);
    let shape = api::event_json(&backend::Event::Text("a block".to_string()));
    assert_eq!(shape["type"], "text");
    assert_eq!(shape["data"], "a block");
    let tool = api::event_json(&backend::Event::ToolStart {
        id: "1".into(),
        name: "read".into(),
        detail: "a.rs".into(),
    });
    assert_eq!(tool["type"], "tool_start");
    assert_eq!(tool["data"]["name"], "read");
    assert_eq!(
        (chat.turns, chat.input_tokens, chat.output_tokens),
        (1, 21, 7)
    );
    assert!(chat
        .session
        .as_deref()
        .map(|id| !id.is_empty())
        .unwrap_or(false));

    let asked = server.join().unwrap();
    assert_eq!(asked["model"], "test-model");
    assert_eq!(asked["messages"][1]["role"], "user");
    assert_eq!(asked["messages"][1]["content"], "Say hello");
    assert!(asked["messages"][0]["content"]
        .as_str()
        .unwrap_or("")
        .contains("Penelope"));

    let stored = sessions::all(&ctx);
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0]["id"], chat.session.clone().unwrap());
    assert!(stored[0]["turns"].as_u64() == Some(1));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_provider_snapshot_names_the_roles_and_never_a_key() {
    let dir = std::env::temp_dir().join(format!("penelope-core-providers-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let ctx = api::load(&dir);
    let rows = api::provider_rows(&ctx);
    assert!(rows
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p["name"] == "claude-cli"));
    assert!(rows
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p["name"] == "openai" && p["key_env"] == "OPENAI_API_KEY"));
    let shown = rows.to_string();
    assert!(!shown.contains("sk-"));
    assert_eq!(providers::TABLE.len(), rows.as_array().unwrap().len());
    let settings = api::settings_json(&ctx);
    assert_eq!(settings["main"]["kind"], "claude-cli");
    assert!(settings["doctor"].as_str().unwrap().contains("folder"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_provider_choice_survives_a_reload_and_a_key_is_never_returned() {
    let dir = std::env::temp_dir().join(format!("penelope-core-choice-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut ctx = api::load(&dir);
    let said = api::set_provider(&mut ctx, "openai", "main", Some("gpt-test"), None, None).unwrap();
    assert!(said.starts_with("set: main model gpt-test"), "{}", said);
    let reloaded = api::load(&dir);
    assert_eq!(reloaded.cfg["main"]["kind"], "openai");
    assert_eq!(reloaded.cfg["main"]["model"], "gpt-test");
    assert_eq!(reloaded.cfg["main"]["api_key_env"], "OPENAI_API_KEY");
    assert!(api::set_provider(&mut ctx, "not-a-provider", "main", None, None, None).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}
