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
        "route": "fixed",
        "route_context_limit": 100000,
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
                   "searxng_url": "", "api_url": "", "server_api_key_env": "PENELOPE_SEARCH_API_KEY", "urls": [],
                   "rerank": {"url": "", "api_key_env": "RERANK_API_KEY", "enabled": false}},
        "research": {"max_rounds": 3, "max_parallel": 3, "pages_per_sub": 4, "page_chars": 12000, "max_queries": 3, "max_checks": 80, "default_price": [3.0, 15.0]},
        "reader": {"enabled": true, "url": "", "api_key_env": "PENELOPE_READER_API_KEY"},
        "privacy": "rules",
        "privacy_answers": true,
        "auto_learn": true,
        "auto_skills": true,
        "mode": "auto",
        "modes": {"default": "code", "confirm_rights": true,
                  "chat": {"model": "weak", "effort": "low", "route": "fixed"},
                  "work": {"model": "", "effort": "", "route": ""},
                  "agents": {"model": "", "effort": "", "route": ""},
                  "code": {"model": "", "effort": "", "route": ""}},
        "agent_verify": true,
        "roles": {},
        "screencodec": {"command": "", "hosts": "", "images": "", "args": "", "desktop": "off", "desktop_input": "off", "input_cmd": ""},
        "cursor": {"command": "", "enabled": true},
        "eur_per_usd": 0.92,
        "monthly_budget_eur": 0.0,
        "session_budget_eur": 0.0,
        "prices": default_prices(),
        "plugin_dirs": null,
        "plugin_home": null,
        "brief": {"hosts": [], "roots": []},
        "mitos": {"attach_over_bytes": 4096, "fetch_cap_bytes": 16384, "store_max_bytes": 524288000},
        "apps": {"allow_install": false, "rights": {"files": false, "network": false, "shell": false}},
        "update": {"url": "", "channel": "stable", "public_key": ""},
        "global_memory": null,
        "clipboard": {"read": "", "write": ""},
        "ask": {"timeout_secs": 900}
    })
}

/// List prices in dollars per million tokens, input then output. The longest name in a model name wins, so an older
/// family keeps its own price. Anthropic list, read 2026-11-27: https://platform.claude.com/docs/en/about-claude/pricing
/// Haiku 5.5 up to 100K prompt $0.10/$0.50, Sonnet 5.5 $2/$10, Opus 5.5 $4/$20.
fn default_prices() -> Value {
    json!({"opus": [4.0, 20.0], "sonnet": [2.0, 10.0], "haiku": [0.1, 0.5],
           "claude-sonnet-5-5": [2.0, 10.0], "claude-haiku-5-5": [0.1, 0.5],
           "claude-opus-5-5": [4.0, 20.0], "claude-opus-5": [5.0, 25.0], "claude-opus-4-5": [5.0, 25.0],
           "claude-opus-4-6": [5.0, 25.0], "claude-opus-4-7": [5.0, 25.0], "claude-opus-4-8": [5.0, 25.0],
           "claude-opus-4": [15.0, 75.0],
           "claude-sonnet-5": [2.0, 10.0], "claude-sonnet-4": [3.0, 15.0], "claude-haiku-4": [1.0, 5.0],
           "deepseek-v4.1": [0.15, 0.6], "mimo-v2.6": [0.14, 0.28],
           "glm-5.3": [0.2, 0.8], "qwen3.8": [0.2, 0.6], "kimi-k3": [0.6, 2.5]})
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
    let redirected = state_dir(root);
    let root = redirected.as_path();
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
    apply_policy_env(&mut cfg);
    Ctx { root: root.to_path_buf(), cfg }
}

/// A child process can be pinned to the policies of its parent. The claude-cli path starts a fresh
/// `penelope mcp` that would otherwise read `run_policy: all` from disk and ignore the workflow's safe ctx.
fn apply_policy_env(cfg: &mut Value) {
    let run = std::env::var("PENELOPE_RUN_POLICY").ok();
    let edit = std::env::var("PENELOPE_EDIT_POLICY").ok();
    apply_policy(cfg, run.as_deref(), edit.as_deref());
}

fn apply_policy(cfg: &mut Value, run: Option<&str>, edit: Option<&str>) {
    for (key, value, allowed) in [
        ("run_policy", run, &["safe", "all"][..]),
        ("edit_policy", edit, &["ask", "all", "never"][..]),
    ] {
        if let Some(value) = value.map(str::trim).filter(|v| allowed.contains(v)) {
            cfg[key] = serde_json::json!(value);
        }
    }
}

/// The folder that holds Penelope's own files. A path that is a project (it has `.git` or a `.penelope` folder) and is
/// not itself named `.penelope` gets its `.penelope` folder, so `config.json`, `runs/` and `stats.json` never land
/// between the files of the project.
pub fn state_dir(root: &Path) -> PathBuf {
    let named = root.file_name().map(|n| n == ".penelope").unwrap_or(false);
    if !named && (root.join(".git").exists() || root.join(".penelope").is_dir()) {
        root.join(".penelope")
    } else {
        root.to_path_buf()
    }
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

#[cfg(test)]
mod state_dir_tests {
    use super::*;

    #[test]
    fn a_project_folder_gets_its_penelope_folder_and_no_files_of_its_own() {
        let dir = std::env::temp_dir().join(format!("penelope-state-dir-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join(".git")).unwrap();
        let ctx = load(&dir);
        assert_eq!(ctx.root, dir.join(".penelope"));
        assert!(dir.join(".penelope/config.json").is_file());
        assert!(!dir.join("config.json").exists() && !dir.join("runs").exists(), "files between the project files");
        // A state folder that is not a project stays as it is.
        let plain = std::env::temp_dir().join(format!("penelope-state-plain-{}", std::process::id()));
        let _ = fs::remove_dir_all(&plain);
        assert_eq!(load(&plain).root, plain);
        assert_eq!(state_dir(&dir.join(".penelope")), dir.join(".penelope"));
    }

    #[test]
    fn a_policy_env_pins_only_known_values() {
        let mut cfg = default_config();
        cfg["run_policy"] = serde_json::json!("all");
        apply_policy(&mut cfg, Some("safe"), Some("ask"));
        assert_eq!((cfg["run_policy"].as_str(), cfg["edit_policy"].as_str()), (Some("safe"), Some("ask")));
        apply_policy(&mut cfg, Some("bogus"), Some("bogus"));
        assert_eq!((cfg["run_policy"].as_str(), cfg["edit_policy"].as_str()), (Some("safe"), Some("ask")));
    }
}
