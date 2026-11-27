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
use std::time::Duration;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError};

/// The variables that would bill an API key instead of the Claude subscription. They stay out of the child unless the
/// person chose API-key mode (config `claude.api_key_mode`: true).
const KEY_VARS: [&str; 2] = ["ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN"];

static API_KEY_MODE: AtomicBool = AtomicBool::new(false);
static USER_SETTINGS: AtomicBool = AtomicBool::new(false);
static COMMAND: std::sync::RwLock<String> = std::sync::RwLock::new(String::new());

/// Reads the switches of config `claude` into the process: callers that hold no config (the weaker model, a scout, an
/// agent step) spawn the command with the same settings. Called after the config is loaded and before each claude turn.
pub fn configure(cfg: &Value) {
    API_KEY_MODE.store(cfg["claude"]["api_key_mode"].as_bool().unwrap_or(false), Ordering::SeqCst);
    USER_SETTINGS.store(cfg["claude"]["user_settings"].as_bool().unwrap_or(false), Ordering::SeqCst);
    if let Ok(mut c) = COMMAND.write() {
        *c = cfg["claude"]["command"].as_str().unwrap_or("").trim().to_string();
    }
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
    if cfg["claude"]["user_settings"].as_bool().unwrap_or_else(|| USER_SETTINGS.load(Ordering::SeqCst)) {
        return Vec::new();
    }
    vec!["--setting-sources".into(), "project,local".into(), "--disable-slash-commands".into()]
}

/// A command for the claude executable with the key variables removed (unless the person chose API-key mode).
pub fn command(cfg: &Value) -> Command {
    let program = match cfg["claude"]["command"].as_str().filter(|c| !c.trim().is_empty()) {
        Some(c) => c.to_string(),
        None => COMMAND.read().map(|c| c.clone()).ok().filter(|c| !c.is_empty()).unwrap_or_else(|| "claude".to_string()),
    };
    let mut cmd = Command::new(program);
    scrub(&mut cmd);
    cmd
}

/// The one way to start `claude -p` for a turn, a scout, an agent step or the weaker model: the key variables removed
/// and the isolation flags set. `cfg` may be `Value::Null`; the process-wide settings of `configure` apply then.
pub fn print_command(cfg: &Value) -> Command {
    let mut cmd = command(cfg);
    cmd.args(isolation_flags(cfg));
    cmd
}

/// 128 random bits as hex, from the keys the standard library seeds per process and the clock. For a secret that two
/// processes of one Penelope share through their environment.
pub fn nonce() -> String {
    use std::hash::BuildHasher;
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let part = |salt: u64| {
        std::collections::hash_map::RandomState::new().hash_one((now, std::process::id(), salt))
    };
    format!("{:016x}{:016x}", part(1), part(2))
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
    /// Nothing came for the whole idle time.
    TimedOut,
}

pub struct Stream {
    child: Child,
    stdin: Option<ChildStdin>,
    rx: Receiver<Value>,
    pub key: u64,
    /// The file the MCP server reads the gates state from, removed with the process.
    pub live: Option<std::path::PathBuf>,
    /// The secret that signs the live file. Only this process and its MCP server know it.
    pub nonce: String,
}

impl Stream {
    /// Starts the process in its own process group. `args` come without the program name and must not hold `-p`.
    pub fn spawn(cfg: &Value, args: &[String], key: u64, live: Option<std::path::PathBuf>, nonce: String) -> Result<Stream, String> {
        let mut cmd = print_command(cfg);
        cmd.args(["--input-format", "stream-json", "--output-format", "stream-json", "--verbose", "--include-partial-messages"]);
        cmd.args(args);
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);                 // the MCP servers of the process share the group and leave with it
        }
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
        Ok(Stream { child, stdin, rx, key, live, nonce })
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

    /// Throws away every line the process wrote since the last turn ended (a late summary, a status line, a result of a
    /// background task). A line that arrives after the next message was sent is told apart by the caller.
    pub fn drain_stale(&mut self) -> usize {
        let mut n = 0;
        while self.rx.try_recv().is_ok() {
            n += 1;
        }
        n
    }

    pub fn recv(&mut self, idle: Duration) -> Next {
        match self.rx.recv_timeout(idle) {
            Ok(v) => Next::Line(v),
            Err(RecvTimeoutError::Timeout) => Next::TimedOut,
            Err(RecvTimeoutError::Disconnected) => Next::Closed,
        }
    }

    /// True while the process runs.
    pub fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

/// Ends a whole process group: the claude command and the MCP servers it started.
fn kill_group(pid: u32) {
    #[cfg(unix)]
    {
        let _ = Command::new("kill").args(["-KILL", "--", &format!("-{}", pid)]).stdout(Stdio::null()).stderr(Stdio::null()).status();
    }
    #[cfg(not(unix))]
    {
        crate::platform::kill_tree(pid as u64);
    }
}

impl Drop for Stream {
    /// Closing the input lets the process and its servers leave on their own; after about 2 seconds the group is killed.
    fn drop(&mut self) {
        drop(self.stdin.take());
        let pid = self.child.id();
        for _ in 0..20 {
            if !matches!(self.child.try_wait(), Ok(None)) {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        kill_group(pid);                            // also what an exited leader left behind
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

    #[test]
    fn every_way_to_start_claude_removes_the_key_variables() {
        // the helper that agents, scouts, the weaker model and the turns use
        let cmd = print_command(&Value::Null);
        let removed: Vec<String> = cmd.get_envs().filter(|(_, v)| v.is_none()).map(|(k, _)| k.to_string_lossy().to_string()).collect();
        assert!(removed.contains(&"ANTHROPIC_API_KEY".to_string()) && removed.contains(&"ANTHROPIC_AUTH_TOKEN".to_string()));
        assert!(cmd.get_args().any(|a| a == "--setting-sources"));
        // no other file starts the command by hand (clusters.rs is outside this change)
        for (name, text) in [("agents.rs", include_str!("agents.rs")), ("scout.rs", include_str!("scout.rs")), ("weak.rs", include_str!("weak.rs")), ("backend.rs", include_str!("backend.rs"))] {
            let code = text.split("#[cfg(test)]").next().unwrap();
            assert!(!code.contains("Command::new(\"claude\")"), "{} starts claude without the helper", name);
        }
    }

    #[test]
    fn a_nonce_differs_each_time() {
        assert_ne!(nonce(), nonce());
        assert_eq!(nonce().len(), 32);
    }

    #[cfg(unix)]
    #[test]
    fn no_child_of_the_process_survives_the_stream() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("penelope-group-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("claude");
        std::fs::write(&script, "#!/bin/sh\nsleep 300 &\necho $! > \"$(dirname \"$0\")/child.pid\"\nsleep 300\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let cfg = json!({"claude": {"command": script.to_string_lossy()}});
        let stream = Stream::spawn(&cfg, &[], 1, None, nonce()).unwrap();
        for _ in 0..50 {
            if dir.join("child.pid").exists() { break; }
            std::thread::sleep(Duration::from_millis(50));
        }
        let child: u32 = std::fs::read_to_string(dir.join("child.pid")).unwrap().trim().parse().unwrap();
        let alive = |pid: u32| Command::new("kill").args(["-0", &pid.to_string()]).stderr(Stdio::null()).status().map(|s| s.success()).unwrap_or(false);
        assert!(alive(child));
        let leader = stream.pid();
        drop(stream);
        std::thread::sleep(Duration::from_millis(300));
        assert!(!alive(child), "the sleep the process started is gone");
        assert!(!alive(leader));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
