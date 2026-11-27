//! Who works right now, under which name, on which model.
//!
//! The orchestrator and the user name the agents; this module keeps one small file per member under
//! `.penelope/crew/`, so two scouts that finish at the same time cannot overwrite each other. The side panel reads the
//! same files, which is why it can show `builder.work · Haiku 5.5 Mid · working` while the step runs.
//!
//! A model of a Laertes family (Nostos, Tiresias, Utis, Aiolos, Argos, Klea, Telemach, Proteus) is recognised by
//! `"kind": "laertes"` or by the first word of its name. Such a model is shown in gold and marked `in training`,
//! because none of the eight families can answer yet. It is never sent to an endpoint as if it were a hosted model.

use crate::config::Ctx;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

pub const FAMILIES: &[&str] = &["Nostos", "Tiresias", "Utis", "Aiolos", "Argos", "Klea", "Telemach", "Proteus"];

pub struct Member {
    pub name: String,
    pub role: String,
    pub model: String,
    pub state: String,
    pub family: Option<String>,
}

fn dir(ctx: &Ctx) -> PathBuf {
    ctx.root.join("crew")
}

fn safe(name: &str) -> String {
    name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '-' }).collect()
}

/// The family of a model setting, if it names one of the eight in-training families.
pub fn family_of(model: &Value) -> Option<&'static str> {
    if model["kind"].as_str() == Some("laertes") {
        let said = model["family"].as_str().unwrap_or("").to_lowercase();
        return FAMILIES.iter().copied().find(|f| f.to_lowercase() == said);
    }
    let name = model["model"].as_str().unwrap_or("").trim().to_lowercase();
    if name.is_empty() {
        return None;
    }
    let parts: Vec<&str> = name.split([' ', '-', '_', '/']).filter(|p| !p.is_empty()).collect();
    FAMILIES.iter().copied().find(|f| {
        let fam = f.to_lowercase();
        // "Utis", "Utis 1", "Telemach 1 Pro", "argos-1" name a family. "utis-test" is a hosted model name.
        parts.first() == Some(&fam.as_str()) && (parts.len() == 1 || parts.get(1).map(|p| p.chars().next().unwrap_or('x').is_ascii_digit()).unwrap_or(false))
    })
}

/// What the side panel writes: the label if the setting gives one, else the model name. A Laertes model says so.
pub fn describe(model: &Value) -> (String, Option<&'static str>) {
    let family = family_of(model);
    let shown = model["label"].as_str().filter(|l| !l.trim().is_empty()).map(str::to_string)
        .or_else(|| model["model"].as_str().filter(|m| !m.is_empty()).map(str::to_string))
        .unwrap_or_else(|| "no model".to_string());
    match family {
        Some(f) => (shown, Some(f)),
        None => (shown, None),
    }
}

/// Writes or replaces one member. A step that starts and ends calls this twice; the file is small.
pub fn set(ctx: &Ctx, name: &str, role: &str, model: &Value, state: &str) {
    let (shown, family) = describe(model);
    let _ = fs::create_dir_all(dir(ctx));
    let value = json!({
        "name": name.chars().take(40).collect::<String>(),
        "role": role,
        "model": shown,
        "kind": model["kind"].as_str().unwrap_or("claude-cli"),
        "state": state,
        "family": family,
        "at": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
    });
    let _ = fs::write(dir(ctx).join(format!("{}.json", safe(name))), serde_json::to_string_pretty(&value).unwrap());
}

/// Every member, working ones first, then by name. A broken file is skipped.
pub fn list(ctx: &Ctx) -> Vec<Member> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(dir(ctx)) else { return out };
    for e in entries.flatten() {
        let path: &Path = &e.path();
        if path.extension().and_then(|x| x.to_str()) != Some("json") {
            continue;
        }
        let Some(v) = fs::read_to_string(path).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok()) else { continue };
        out.push(Member {
            name: v["name"].as_str().unwrap_or("?").to_string(),
            role: v["role"].as_str().unwrap_or("?").to_string(),
            model: v["model"].as_str().unwrap_or("?").to_string(),
            state: v["state"].as_str().unwrap_or("?").to_string(),
            family: v["family"].as_str().map(str::to_string),
        });
    }
    out.sort_by(|a, b| {
        let rank = |s: &str| match s { "working" => 0, "asking" => 1, "done" => 2, "failed" => 3, _ => 4 };
        rank(&a.state).cmp(&rank(&b.state)).then(a.name.cmp(&b.name))
    });
    out
}

/// Ends the list of a finished run. Called when a new agent run starts, not when one ends, so the last names stay
/// readable until the next run replaces them.
pub fn clear(ctx: &Ctx) {
    let _ = fs::remove_dir_all(dir(ctx));
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn names_models_and_families_are_kept_per_member() {
        let dir = std::env::temp_dir().join(format!("penelope-crew-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let ctx = crate::config::load(&dir.join(".penelope"));
        set(&ctx, "builder.plan", "lead", &json!({"kind": "claude-cli", "model": "sonnet"}), "done");
        set(&ctx, "builder.work", "worker", &json!({"kind": "anthropic", "model": "claude-haiku-5-5", "label": "Haiku 5.5 Mid"}), "working");
        set(&ctx, "builder.check", "judge", &json!({"kind": "laertes", "family": "Argos", "model": "argos-1"}), "working");
        let members = self::list(&ctx);
        assert_eq!(members.len(), 3);
        assert_eq!((members[0].name.as_str(), members[0].model.as_str(), members[0].state.as_str()), ("builder.check", "argos-1", "working"));
        assert!(members[0].family.as_deref() == Some("Argos"));
        assert_eq!((members[1].name.as_str(), members[1].model.as_str()), ("builder.work", "Haiku 5.5 Mid"));
        clear(&ctx);
        assert!(list(&ctx).is_empty() && !dir.join(".penelope/crew").exists());
        assert_eq!(family_of(&json!({"model": "Telemach 1 Pro"})), Some("Telemach"));
        assert_eq!(family_of(&json!({"model": "claude-sonnet-5"})), None);
        let _ = fs::remove_dir_all(&dir);
    }
}
