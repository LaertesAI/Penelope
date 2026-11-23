//! The weaker model. It reads long tool output so that the main model gets a few lines.

use serde_json::{json, Value};
use std::io::Write;
use std::process::{Command, Stdio};

/// One question to a model described by `{"kind": ..., "model": ...}`. Returns the answer text.
pub fn ask(model: &Value, system: &str, user: &str) -> Result<String, String> {
    match model["kind"].as_str().unwrap_or("claude-cli") {
        "openai" => ask_openai(model, system, user),
        _ => ask_claude_cli(model, system, user),
    }
}

fn ask_claude_cli(model: &Value, system: &str, user: &str) -> Result<String, String> {
    let mut child = Command::new("claude")
        .args(["-p", system, "--model", model["model"].as_str().unwrap_or("haiku"), "--output-format", "text", "--tools", "",
               "--strict-mcp-config", "--mcp-config", "{\"mcpServers\":{}}"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("cannot start the claude command: {}", e))?;
    child.stdin.take().unwrap().write_all(user.as_bytes()).map_err(|e| e.to_string())?;
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!("the claude command ended with {}", out.status));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn ask_openai(model: &Value, system: &str, user: &str) -> Result<String, String> {
    let base = model["base_url"].as_str().unwrap_or("http://localhost:11434/v1").trim_end_matches('/').to_string();
    let key = model["api_key_env"].as_str().and_then(|n| std::env::var(n).ok()).unwrap_or_default();
    let body = json!({
        "model": model["model"].as_str().unwrap_or(""),
        "temperature": 0,
        "messages": [{"role": "system", "content": system}, {"role": "user", "content": user}]
    });
    let mut req = ureq::post(&format!("{}/chat/completions", base)).timeout(std::time::Duration::from_secs(180));
    if !key.is_empty() {
        req = req.set("Authorization", &format!("Bearer {}", key));
    }
    let resp: Value = req.send_json(body).map_err(|e| format!("request failed: {}", e))?.into_json().map_err(|e| e.to_string())?;
    resp["choices"][0]["message"]["content"].as_str().map(|s| s.trim().to_string()).ok_or_else(|| "no answer text in the response".to_string())
}
