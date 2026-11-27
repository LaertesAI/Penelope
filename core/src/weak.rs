//! The weaker model. It reads long tool output so that the main model gets a few lines.

use serde_json::Value;
use std::io::Write;
use std::process::Stdio;

/// One question to a model described by `{"kind": ..., "model": ...}`. Returns the answer text.
pub fn ask(model: &Value, system: &str, user: &str) -> Result<String, String> {
    ask_count(model, system, user).map(|(text, _, _)| text)
}

/// The same question, with the tokens the endpoint counted. The claude command in text form reports no usage, so its
/// two numbers stay zero rather than a guess.
pub fn ask_count(model: &Value, system: &str, user: &str) -> Result<(String, u64, u64), String> {
    if let Some(family) = crate::crew::family_of(model) {
        return Err(format!("{} 1 is one of the Laertes families and is in training. Choose a hosted model for this role.", family));
    }
    match model["kind"].as_str().unwrap_or("claude-cli") {
        kind if crate::llm::is_api(kind) => ask_api(model, system, user),
        _ => ask_claude_cli(model, system, user).map(|t| (t, 0, 0)),
    }
}

fn ask_claude_cli(model: &Value, system: &str, user: &str) -> Result<String, String> {
    let mut cmd = crate::claudecli::print_command(&Value::Null);
    cmd.args(["-p", system, "--model", model["model"].as_str().unwrap_or("haiku"), "--output-format", "text", "--tools", "",
              "--strict-mcp-config", "--mcp-config", "{\"mcpServers\":{}}"]);
    if let Some(level) = crate::llm::effort(model) {
        cmd.args(["--effort", level]);
    }
    let mut child = cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().map_err(|e| format!("cannot start the claude command: {}", e))?;
    child.stdin.take().unwrap().write_all(user.as_bytes()).map_err(|e| e.to_string())?;
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!("the claude command ended with {}", out.status));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// One question through an HTTP endpoint, in the format of the setting's kind and without tools.
fn ask_api(model: &Value, system: &str, user: &str) -> Result<(String, u64, u64), String> {
    let reply = crate::llm::chat(model, system, &[crate::llm::user_message(user)], &[])?;
    if reply.text.is_empty() {
        return Err(format!("no answer text in the response (stop reason {})", reply.stop));
    }
    Ok((reply.text, reply.tokens_in, reply.tokens_out))
}
