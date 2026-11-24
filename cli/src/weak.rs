//! The weaker model. It reads long tool output so that the main model gets a few lines.

use serde_json::Value;
use std::io::Write;
use std::process::{Command, Stdio};

/// One question to a model described by `{"kind": ..., "model": ...}`. Returns the answer text.
pub fn ask(model: &Value, system: &str, user: &str) -> Result<String, String> {
    match model["kind"].as_str().unwrap_or("claude-cli") {
        kind if crate::llm::is_api(kind) => ask_api(model, system, user),
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

/// One question through an HTTP endpoint, in the format of the setting's kind and without tools.
fn ask_api(model: &Value, system: &str, user: &str) -> Result<String, String> {
    let reply = crate::llm::chat(model, system, &[crate::llm::user_message(user)], &[])?;
    if reply.text.is_empty() {
        return Err(format!("no answer text in the response (stop reason {})", reply.stop));
    }
    Ok(reply.text)
}
