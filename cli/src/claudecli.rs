//! The official `claude` command as a long-lived child: one process per session that talks stream-json over
//! stdin and stdout, the protocol the Claude Agent SDKs speak (`--input-format stream-json --output-format stream-json
//! --verbose`). A user message goes in as one JSON line, `system`, `assistant`, `user`, `stream_event` and finally
//! `result` lines come out. Penelope never opens, copies or refreshes the credential files of the claude command;
//! the child logs in by itself. Config `claude.transport`: `sdk-stream` (default) or `per-turn` (one process per turn).

use serde_json::{json, Value};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver};

/// The variables that would bill an API key instead of the Claude subscription. They stay out of the child unless the
/// person chose API-key mode (config `claude.api_key_mode`: true).
const KEY_VARS: [&str; 2] = ["ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN"];

static API_KEY_MODE: AtomicBool = AtomicBool::new(false);

/// Reads the switches of config `claude`. Called once after the config is loaded.
pub fn configure(cfg: &Value) {
    API_KEY_MODE.store(cfg["claude"]["api_key_mode"].as_bool().unwrap_or(false), Ordering::SeqCst);
}

pub fn api_key_mode() -> bool {
    API_KEY_MODE.load(Ordering::SeqCst)
}

/// True for the long-lived transport. Anything but `per-turn` means `sdk-stream`.
pub fn streaming(cfg: &Value) -> bool {
    let env = std::env::var("PENELOPE_CLAUDE_TRANSPORT").ok();
    env.as_deref().or_else(|| cfg["claude"]["transport"].as_str()).unwrap_or("sdk-stream") != "per-turn"
}

/// Flags that keep the child away from the Claude Code setup of the person: the user settings (hooks of other tools
/// that run at the start and the end of every turn, enabled plugins) and the skills and commands of that setup, which
/// Penelope offers by itself. Project and local settings still load. Config `claude.user_settings: true` loads all.
pub fn isolation_flags(cfg: &Value) -> Vec<String> {
    if cfg["claude"]["user_settings"].as_bool().unwrap_or(false) {
        return Vec::new();
    }
    vec!["--setting-sources".into(), "project,local".into(), "--disable-slash-commands".into()]
}

/// A command for the claude executable with the key variables removed (unless the person chose API-key mode).
pub fn command(cfg: &Value) -> Command {
    let program = cfg["claude"]["command"].as_str().filter(|c| !c.trim().is_empty()).unwrap_or("claude");
    let mut cmd = Command::new(program);
    scrub(&mut cmd);
    cmd
}

/// Removes the variables that would switch the child to API-key billing.
pub fn scrub(cmd: &mut Command) {
    if !api_key_mode() {
        for var in KEY_VARS {
            cmd.env_remove(var);
        }
    }
    cmd.env_remove("CLAUDECODE");
    // The claude command asks a model for a summary of the end of every turn and holds the result back until it
    // arrives (about 2 seconds). Penelope does not show it, so the cheap local rule does the job.
    if std::env::var_os("CLAUDE_CODE_CLASSIFIER_SUMMARY").is_none() {
        cmd.env("CLAUDE_CODE_CLASSIFIER_SUMMARY", "0");
    }           // a child of Penelope is no child of a Claude Code session
}

/// A stable fingerprint of what the child was started with. A different one means a new process.
pub fn fingerprint(parts: &[String]) -> u64 {
    let mut h = DefaultHasher::new();
    parts.hash(&mut h);
    h.finish()
}

/// What `Stream::recv` found.
pub enum Next {
    Line(Value),
    /// The process closed its output: it ended or crashed.
    Closed,
}

pub struct Stream {
    child: Child,
    stdin: Option<ChildStdin>,
    rx: Receiver<Value>,
    pub key: u64,
    /// The file the MCP server reads the gates state from, removed with the process.
    pub live: Option<std::path::PathBuf>,
}

impl Stream {
    /// Starts the process. `args` come without the program name and must not hold `-p`.
    pub fn spawn(cfg: &Value, args: &[String], key: u64, live: Option<std::path::PathBuf>) -> Result<Stream, String> {
        let mut cmd = command(cfg);
        cmd.args(["--input-format", "stream-json", "--output-format", "stream-json", "--verbose", "--include-partial-messages"]);
        cmd.args(args);
        let mut child = cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn()
            .map_err(|e| format!("cannot start the claude command: {}", e))?;
        let stdin = child.stdin.take();
        let out = child.stdout.take().ok_or("the claude command has no output")?;
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(out).lines() {
                let Ok(line) = line else { break };
                let v = serde_json::from_str::<Value>(line.trim()).unwrap_or_else(|_| json!({"type": "raw", "text": line}));
                if tx.send(v).is_err() {
                    break;
                }
            }
        });
        Ok(Stream { child, stdin, rx, key, live })
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// One user message, in the shape the Agent SDKs write.
    pub fn send_user(&mut self, text: &str) -> Result<(), String> {
        let line = json!({"type": "user", "message": {"role": "user", "content": text}, "parent_tool_use_id": null, "session_id": "default"});
        let stdin = self.stdin.as_mut().ok_or("the claude command has no input")?;
        writeln!(stdin, "{}", line).and_then(|_| stdin.flush()).map_err(|e| format!("cannot write to the claude command: {}", e))
    }

    pub fn recv(&mut self) -> Next {
        match self.rx.recv() {
            Ok(v) => Next::Line(v),
            Err(_) => Next::Closed,
        }
    }

    /// True while the process runs.
    pub fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        drop(self.stdin.take());                // end of input lets the process leave by itself
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(path) = &self.live {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_key_variables_are_removed_from_the_child() {
        let mut cmd = Command::new("true");
        scrub(&mut cmd);
        let removed: Vec<String> = cmd.get_envs().filter(|(_, v)| v.is_none()).map(|(k, _)| k.to_string_lossy().to_string()).collect();
        assert!(removed.contains(&"ANTHROPIC_API_KEY".to_string()) && removed.contains(&"ANTHROPIC_AUTH_TOKEN".to_string()), "{:?}", removed);
    }

    #[test]
    fn the_transport_is_a_stream_unless_per_turn_is_set() {
        assert!(streaming(&json!({})) && streaming(&json!({"claude": {"transport": "sdk-stream"}})));
        assert!(!streaming(&json!({"claude": {"transport": "per-turn"}})));
    }

    #[test]
    fn a_changed_part_changes_the_fingerprint() {
        let a = fingerprint(&["--model".into(), "sonnet".into()]);
        assert_eq!(a, fingerprint(&["--model".into(), "sonnet".into()]));
        assert_ne!(a, fingerprint(&["--model".into(), "haiku".into()]));
    }
}
