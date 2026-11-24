//! Where Strix keeps its files and which models it talks to.

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
        "main": {"kind": "claude-cli", "model": "sonnet"},
        "weak": {"kind": "claude-cli", "model": "haiku"},
        "run_policy": "safe",
        "privacy": "rules",
        "auto_learn": true,
        "screencodec": {"command": "", "hosts": "", "images": ""}
    })
}

/// The nearest `.strix` folder upwards from the working directory, else `~/.strix`.
pub fn find_root(explicit: Option<&str>) -> PathBuf {
    if let Some(p) = explicit {
        return PathBuf::from(p);
    }
    let mut dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    loop {
        if dir.join(".strix").is_dir() {
            return dir.join(".strix");
        }
        if dir.join(".kate").is_dir() {
            return adopt(&dir);
        }
        if !dir.pop() {
            break;
        }
    }
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()));
    if !home.join(".strix").is_dir() && home.join(".kate").is_dir() {
        return adopt(&home);
    }
    home.join(".strix")
}

/// A folder `.kate` from versions before 0.4.0 becomes `.strix`. If the rename fails, the old folder stays in use.
fn adopt(dir: &Path) -> PathBuf {
    match fs::rename(dir.join(".kate"), dir.join(".strix")) {
        Ok(()) => dir.join(".strix"),
        Err(_) => dir.join(".kate"),
    }
}

pub fn load(root: &Path) -> Ctx {
    let _ = fs::create_dir_all(root.join("runs"));
    let path = root.join("config.json");
    let mut cfg = default_config();
    if let Ok(text) = fs::read_to_string(&path) {
        if let Ok(Value::Object(user)) = serde_json::from_str::<Value>(&text) {
            for (k, v) in user {
                cfg[k] = v;
            }
        }
    } else {
        let _ = fs::write(&path, serde_json::to_string_pretty(&cfg).unwrap());
    }
    Ctx { root: root.to_path_buf(), cfg }
}

/// First match of a program on the PATH.
pub fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var("PATH").unwrap_or_default();
    for dir in path.split(':') {
        let p = Path::new(dir).join(name);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}
