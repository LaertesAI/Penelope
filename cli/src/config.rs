//! Where Penelope keeps its files and which models it talks to.

use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone)]
pub struct Ctx {
    pub root: PathBuf,
    pub cfg: Value,
}

pub fn default_config() -> Value {
    json!({
        "main": {"kind": "claude-cli", "model": "sonnet", "effort": "high"},
        "weak": {"kind": "claude-cli", "model": "haiku", "effort": "high"},
        "run_policy": "safe",
        "sandbox": "off",
        "edit_policy": "ask",
        "test_command": "",
        "test_attempts": 3,
        "eurykleia": "on",
        "check": {"mode": "gate", "module_tests": "", "run": "", "candidates": 3, "models": ["weak"], "timeout": 300, "budget": 1200, "nudges": 1},
        "delegate": "auto",
        "search": {"kind": "auto", "url": "", "api_key_env": "BRAVE_API_KEY", "tavily_api_key_env": "TAVILY_API_KEY",
                   "exa_api_key_env": "EXA_API_KEY", "firecrawl_api_key_env": "FIRECRAWL_API_KEY", "fanout": true,
                   "cache": true, "cache_ttl": 3600, "limit": 8, "language": "en", "safe": true,
                   "jina": false, "jina_base": "https://r.jina.ai/", "wikipedia": false,
                   "api_url": "", "server_api_key_env": "PENELOPE_SEARCH_API_KEY", "urls": [],
                   "rerank": {"url": "", "api_key_env": "RERANK_API_KEY", "enabled": false}},
        "reader": {"enabled": true, "url": "", "api_key_env": "PENELOPE_READER_API_KEY"},
        "privacy": "rules",
        "privacy_answers": true,
        "auto_learn": true,
        "auto_skills": true,
        "mode": "auto",
        "agent_verify": true,
        "roles": {},
        "screencodec": {"command": "", "hosts": "", "images": "", "args": "", "desktop": "off", "desktop_input": "off", "input_cmd": ""},
        "cursor": {"command": "", "enabled": true},
        "eur_per_usd": 0.92,
        "monthly_budget_eur": 0.0,
        "session_budget_eur": 0.0,
        "prices": {"opus": [15.0, 75.0], "sonnet": [3.0, 15.0], "haiku": [0.8, 4.0],
                   "claude-haiku-5-5": [0.1, 0.5], "deepseek-v4.1": [0.15, 0.6], "mimo-v2.6": [0.14, 0.28],
                   "glm-5.3": [0.2, 0.8], "qwen3.8": [0.2, 0.6], "kimi-k3": [0.6, 2.5]},
        "plugin_dirs": null,
        "plugin_home": null,
        "brief": {"hosts": [], "roots": []},
        "mitos": {"attach_over_bytes": 4096, "fetch_cap_bytes": 16384, "store_max_bytes": 524288000},
        "apps": {"allow_install": false, "rights": {"files": false, "network": false, "shell": false}},
        "update": {"url": "", "channel": "stable", "public_key": ""},
        "global_memory": null,
        "clipboard": {"read": "", "write": ""}
    })
}

/// The nearest `.penelope` folder upwards from the working directory, else `~/.penelope`.
pub fn find_root(explicit: Option<&str>) -> PathBuf {
    if let Some(p) = explicit {
        return PathBuf::from(p);
    }
    root_from(&std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")), true)
}

/// The same search from a given folder. With `rename` false an old `.strix` or `.kate` folder is left alone, so a
/// folder switch never changes files on disk.
pub fn root_from(start: &Path, rename: bool) -> PathBuf {
    let mut dir = start.to_path_buf();
    loop {
        if dir.join(".penelope").is_dir() {
            return dir.join(".penelope");
        }
        if rename {
            if let Some(old) = adopt(&dir) {
                return old;
            }
        }
        if !dir.pop() {
            break;
        }
    }
    let home = crate::platform::home();
    if !home.join(".penelope").is_dir() && rename {
        if let Some(old) = adopt(&home) {
            return old;
        }
    }
    home.join(".penelope")
}

/// A folder `.strix` or `.kate` from versions before 0.5.0 becomes `.penelope`. If the rename fails, the old folder stays in use.
fn adopt(dir: &Path) -> Option<PathBuf> {
    for old in [".strix", ".kate"] {
        if dir.join(old).is_dir() {
            return Some(match fs::rename(dir.join(old), dir.join(".penelope")) {
                Ok(()) => dir.join(".penelope"),
                Err(_) => dir.join(old),
            });
        }
    }
    None
}

pub fn load(root: &Path) -> Ctx {
    let _ = fs::create_dir_all(root.join("runs"));
    let path = root.join("config.json");
    let mut cfg = default_config();
    if let Ok(text) = fs::read_to_string(&path) {
        if let Ok(user) = serde_json::from_str::<Value>(&text) {
            merge(&mut cfg, user);
        }
    } else {
        let _ = fs::write(&path, serde_json::to_string_pretty(&cfg).unwrap());
    }
    Ctx { root: root.to_path_buf(), cfg }
}

/// Merges the saved settings over the defaults, one level deeper for objects. A new setting reaches an old folder
/// without dropping the settings of the person; a value of the person always wins.
fn merge(base: &mut Value, user: Value) {
    match (base, user) {
        (Value::Object(b), Value::Object(u)) => {
            for (k, v) in u {
                if let Some(slot) = b.get_mut(&k) {
                    merge(slot, v);
                } else {
                    b.insert(k, v);
                }
            }
        }
        (slot, value) => *slot = value,
    }
}

/// First match of a program on the PATH. Windows also tries the suffixes of PATHEXT (`.exe`, `.cmd`, ...), and the
/// folders are split with the platform's own separator instead of a hard-coded colon.
pub fn which(name: &str) -> Option<PathBuf> {
    if name.contains('/') || name.contains('\\') {
        let p = PathBuf::from(name);
        return if p.is_file() { Some(p) } else { None };
    }
    let path = std::env::var_os("PATH")?;
    let suffixes: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into()).split(';').map(|s| s.to_lowercase()).collect()
    } else {
        vec![String::new()]
    };
    for dir in std::env::split_paths(&path) {
        for suffix in &suffixes {
            let candidate = dir.join(format!("{}{}", name, suffix));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}
