//! The modes of Penelope: one name that sets the policies a turn works under.
//!
//! A mode is a preset, not a hidden switch: `penelope mode auto` writes the same fields into `config.json` that the
//! setup writes. The mode is shown in the side panel. `safe` refuses every file change and every command outside the
//! read-only list; `power` runs and changes without a question, and is meant for a folder you can throw away.

use crate::config::Ctx;
use serde_json::json;

pub const MODES: &[(&str, &str)] = &[
    ("auto", "searches go to the scout, safe commands run, a file change asks"),
    ("manual", "the lead does every step itself; no scout"),
    ("safe", "read-only: no file change, no command outside the read-only list"),
    ("review", "like auto, and an agent always adds the judge step"),
    ("privacy", "the fixed rules and the PII level on every text, no scout, no auto-learning"),
    ("power", "every command and every change runs without a question; use a throw-away folder"),
];

pub fn names() -> Vec<&'static str> {
    MODES.iter().map(|(n, _)| *n).collect()
}

pub fn current(ctx: &Ctx) -> String {
    let name = ctx.cfg["mode"].as_str().unwrap_or("auto").to_string();
    if names().contains(&name.as_str()) { name } else { "auto".to_string() }
}

/// The settings of one mode. `mode` itself is stored as well, so `penelope mode` can show it after a restart.
pub fn apply(ctx: &mut Ctx, name: &str) -> Result<String, String> {
    let name = name.trim().to_lowercase();
    if !names().contains(&name.as_str()) {
        return Err(format!("NOT set. {} is no mode. The modes are: {}.", name, names().join(", ")));
    }
    let (delegate, run, edit, learn, privacy, verify) = match name.as_str() {
        "manual" => ("off", "safe", "ask", true, "rules", true),
        "safe" => ("off", "safe", "never", false, "rules", true),
        "review" => ("auto", "safe", "ask", true, "rules", true),
        "privacy" => ("off", "safe", "ask", false, "pii", true),
        "power" => ("auto", "all", "all", true, "rules", false),
        _ => ("auto", "safe", "ask", true, "rules", true),
    };
    ctx.cfg["mode"] = json!(name);
    ctx.cfg["delegate"] = json!(delegate);
    ctx.cfg["run_policy"] = json!(run);
    ctx.cfg["edit_policy"] = json!(edit);
    ctx.cfg["auto_learn"] = json!(learn);
    ctx.cfg["privacy"] = json!(privacy);
    ctx.cfg["privacy_answers"] = json!(privacy != "off");
    ctx.cfg["agent_verify"] = json!(verify);
    if let Err(e) = std::fs::write(ctx.root.join("config.json"), serde_json::to_string_pretty(&ctx.cfg).unwrap()) {
        return Err(format!("NOT saved. {}", e));
    }
    Ok(format!("mode {}: {}. delegate {}, commands {}, file changes {}{}.", name, MODES.iter().find(|(n, _)| *n == name).map(|(_, d)| *d).unwrap_or(""),
               delegate, run, if edit == "never" { "refused" } else { edit },
               if name == "review" { ", the judge is always added" } else { "" }))
}

pub fn report(ctx: &Ctx) -> String {
    let now = current(ctx);
    let mut out = format!("mode: {}\n\n", now);
    for (name, what) in MODES {
        out.push_str(&format!("{} {:<8} {}\n", if *name == now { "●" } else { "○" }, name, what));
    }
    out.push_str("\npenelope mode NAME sets one; /mode NAME does the same in the interface.\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mode_sets_the_policies_and_safe_refuses_changes() {
        let dir = std::env::temp_dir().join(format!("penelope-mode-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut ctx = crate::config::load(&dir.join(".penelope"));
        assert!(apply(&mut ctx, "safe").unwrap().contains("file changes refused"));
        assert_eq!((ctx.cfg["edit_policy"].as_str(), ctx.cfg["run_policy"].as_str(), ctx.cfg["delegate"].as_str()), (Some("never"), Some("safe"), Some("off")));
        assert_eq!(current(&ctx), "safe");
        assert!(apply(&mut ctx, "power").unwrap().contains("commands all"));
        assert_eq!((ctx.cfg["edit_policy"].as_str(), ctx.cfg["run_policy"].as_str()), (Some("all"), Some("all")));
        assert!(apply(&mut ctx, "nope").unwrap_err().contains("no mode"));
        assert!(report(&ctx).contains("● power"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
