//! The small JSON API between the core and a frontend.
//!
//! Every function here is a plain call with Serde data. The command line uses the modules directly; a desktop layer
//! uses these snapshots and commands so that it never reaches into a private field or copies a rule.

use crate::config::{self, Ctx};
use crate::{auth, autoroute, backend, clusters, cost, providers};
use serde_json::{json, Value};
use std::path::Path;

/// The folder Penelope would use here, loaded.
pub fn root() -> Ctx {
    config::load(&config::find_root(None))
}

/// One folder, loaded.
pub fn load(root: &Path) -> Ctx {
    config::load(root)
}

/// One turn of a chat. The callback receives the events while the turn runs.
pub fn chat(
    ctx: &Ctx,
    chat: &mut backend::Chat,
    text: &str,
    memory: &str,
    on: &mut dyn FnMut(backend::Event),
) -> Result<String, String> {
    chat.turn_with(ctx, text, memory, on)
}

/// One event as JSON. The desktop sends this over its event channel.
pub fn event_json(event: &backend::Event) -> Value {
    serde_json::to_value(event).unwrap_or_else(|_| json!({"type": "unknown"}))
}

/// The provider table with the state of each key and the roles that use it now.
pub fn provider_rows(ctx: &Ctx) -> Value {
    let mut out = Vec::new();
    for p in providers::TABLE {
        let roles: Vec<&str> = ["main", "weak", "scout"]
            .into_iter()
            .filter(|role| {
                providers::of(&ctx.cfg[*role])
                    .map(|x| x.name == p.name)
                    .unwrap_or(false)
            })
            .collect();
        out.push(json!({
            "name": p.name, "label": p.label, "kind": p.kind, "base": p.base, "key_env": p.key_env,
            "key_optional": p.key_optional, "model": p.model, "note": p.note,
            "state": providers::state_of(p), "connected": providers::connected(p),
            "stored_key": auth::has(p.name), "roles": roles,
        }));
    }
    Value::Array(out)
}

/// The chat models of one provider, with the effort levels it accepts. The cached list wins, the catalog is the floor.
pub fn model_rows(ctx: &Ctx, provider: &str) -> Value {
    let mut list = providers::cached_models(ctx, provider);
    if list.is_empty() {
        list = providers::static_models(provider);
    }
    Value::Array(
        list.iter()
            .map(|m| json!({"id": m.id, "label": m.label, "efforts": m.efforts}))
            .collect(),
    )
}

/// Writes one provider setting for a role. The message of the core is returned unchanged.
pub fn set_provider(
    ctx: &mut Ctx,
    provider: &str,
    role: &str,
    model: Option<&str>,
    workspace: Option<&str>,
    base_url: Option<&str>,
) -> Result<String, String> {
    let mut args = vec![provider.to_string(), "--role".to_string(), role.to_string()];
    if let Some(m) = model.filter(|m| !m.trim().is_empty()) {
        args.push("--model".to_string());
        args.push(m.to_string());
    }
    if let Some(w) = workspace.filter(|w| !w.trim().is_empty()) {
        args.push("--workspace".to_string());
        args.push(w.to_string());
    }
    if let Some(b) = base_url.filter(|b| !b.trim().is_empty()) {
        args.push("--base-url".to_string());
        args.push(b.to_string());
    }
    let said = providers::use_provider(ctx, &args);
    if said.starts_with("NOT ") {
        Err(said)
    } else {
        Ok(said)
    }
}

/// Stores one API key. It goes to the Secret Service, or to the 0600 file. The key is never returned.
pub fn save_key(provider: &str, key: &str) -> Result<String, String> {
    let where_ = auth::store(provider, key)?;
    Ok(format!(
        "{}: key stored in {}. It is never shown again.",
        provider, where_
    ))
}

/// Removes one stored key.
pub fn remove_key(provider: &str) -> String {
    auth::remove(provider)
}

/// The stored-key state of every provider. No key and no length is ever returned.
pub fn key_rows(_ctx: &Ctx) -> Value {
    Value::Array(providers::TABLE.iter().map(|p| json!({
        "name": p.name, "label": p.label, "key_env": p.key_env, "state": providers::state_of(p),
        "stored": auth::has(p.name), "connected": providers::connected(p),
    })).collect())
}

/// Field names in a role that hold a key. `settings_json` returns only whether one is set.
const SECRET_FIELDS: [&str; 4] = ["api_key", "key", "token", "secret"];

/// One role entry without its secrets. A key field becomes `"<field>_set": true`; every header value becomes
/// `"(hidden)"`, because `llm.rs` sends the headers as they are and one may carry `Authorization`.
fn redact_role(model: &Value) -> Value {
    let mut out = model.clone();
    if let Some(obj) = out.as_object_mut() {
        for field in SECRET_FIELDS {
            if obj.remove(field).is_some() {
                obj.insert(format!("{}_set", field), json!(true));
            }
        }
        if let Some(headers) = obj.get_mut("headers").and_then(Value::as_object_mut) {
            for value in headers.values_mut() {
                *value = json!("(hidden)");
            }
        }
    }
    out
}

/// The settings the desktop may show and change. No key and no header value is returned.
pub fn settings_json(ctx: &Ctx) -> Value {
    let roles = match ctx.cfg["roles"].as_object() {
        Some(map) => Value::Object(
            map.iter()
                .map(|(name, model)| (name.clone(), redact_role(model)))
                .collect(),
        ),
        None => ctx.cfg["roles"].clone(),
    };
    json!({
        "root": ctx.root.display().to_string(),
        "main": redact_role(&ctx.cfg["main"]), "weak": redact_role(&ctx.cfg["weak"]),
        "scout": redact_role(&ctx.cfg["scout"]),
        "roles": roles, "route": ctx.cfg["route"].as_str().unwrap_or("fixed"),
        "sandbox": ctx.cfg["sandbox"].as_str().unwrap_or("off"),
        "run_policy": ctx.cfg["run_policy"].as_str().unwrap_or("safe"),
        "edit_policy": ctx.cfg["edit_policy"].as_str().unwrap_or("ask"),
        "eurykleia": ctx.cfg["eurykleia"].as_str().unwrap_or("on"),
        "delegate": ctx.cfg["delegate"].as_str().unwrap_or("auto"),
        "mode": ctx.cfg["mode"].as_str().unwrap_or("auto"),
        "prices": ctx.cfg["prices"],
        "monthly_budget_eur": ctx.cfg["monthly_budget_eur"],
        "session_budget_eur": ctx.cfg["session_budget_eur"],
        "eur_per_usd": ctx.cfg["eur_per_usd"],
        "test_command": ctx.cfg["test_command"],
        "doctor": crate::doctor::text(ctx),
    })
}

/// One euro field of the settings as a number. A number or a numeric string (the value of an HTML input) is
/// accepted; an empty string or null clears the field. A negative value, or zero for `eur_per_usd`, is an error.
fn number_field(key: &str, value: &Value) -> Result<Value, String> {
    let n = match value {
        Value::Null => return Ok(Value::Null),
        Value::Number(n) => n.as_f64(),
        Value::String(s) if s.trim().is_empty() => return Ok(Value::Null),
        Value::String(s) => s.trim().replace(',', ".").parse::<f64>().ok(),
        _ => None,
    };
    let n = n
        .filter(|n| n.is_finite())
        .ok_or_else(|| format!("{} must be a number", key))?;
    if n < 0.0 || (key == "eur_per_usd" && n == 0.0) {
        return Err(format!(
            "{} must be {}",
            key,
            if key == "eur_per_usd" {
                "above 0"
            } else {
                "0 or more"
            }
        ));
    }
    Ok(json!(n))
}

/// Writes the settings a desktop may change. Keys, paths and other fields stay out of this list.
///
/// A changed `mode` sets the same policies as `penelope mode NAME`, through `mode::preset`; the policy fields it
/// sets are then not taken from the patch, so a panel that sends every field cannot keep `power` policies under
/// `safe`. Nothing is written when one field is wrong.
pub fn save_settings(ctx: &mut Ctx, patch: &Value) -> Result<(), String> {
    let allowed = [
        "sandbox",
        "run_policy",
        "edit_policy",
        "eurykleia",
        "delegate",
        "route",
        "test_command",
    ];
    let numbers = ["monthly_budget_eur", "session_budget_eur", "eur_per_usd"];
    let mut cfg = ctx.cfg.clone();
    let new_mode = match patch.get("mode") {
        None | Some(Value::Null) => None,
        Some(Value::String(name)) => {
            let name = name.trim().to_lowercase();
            if name != crate::mode::current(ctx) || !crate::mode::names().contains(&name.as_str()) {
                Some(crate::mode::preset(&mut cfg, &name)?)
            } else {
                None
            }
        }
        Some(_) => return Err("mode must be a string".to_string()),
    };
    for key in allowed {
        if new_mode.is_some() && ["run_policy", "edit_policy", "delegate"].contains(&key) {
            continue;
        }
        if let Some(value) = patch.get(key) {
            if value.is_string() || value.is_null() {
                cfg[key] = value.clone();
            } else {
                return Err(format!("{} must be a string", key));
            }
        }
    }
    for key in numbers {
        if let Some(value) = patch.get(key) {
            cfg[key] = number_field(key, value)?;
        }
    }
    if let Some(prices) = patch.get("prices").and_then(Value::as_object) {
        if !cfg["prices"].is_object() {
            cfg["prices"] = json!({});
        }
        for (name, price) in prices {
            let ok = price
                .as_array()
                .map(|a| {
                    a.len() == 2
                        && a.iter()
                            .all(|x| x.as_f64().map(|n| n >= 0.0).unwrap_or(false))
                })
                .unwrap_or(false);
            if !ok {
                return Err(format!(
                    "price for {} must be [input, output] in dollars per million tokens",
                    name
                ));
            }
            cfg["prices"][name.as_str()] = price.clone();
        }
    }
    std::fs::write(
        ctx.root.join("config.json"),
        serde_json::to_string_pretty(&cfg).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("cannot write config.json: {}", e))?;
    ctx.cfg = cfg;
    Ok(())
}

/// The rows of `usage.json`, with the euro of each row at the prices in the settings.
pub fn usage_rows(ctx: &Ctx) -> Value {
    let v: Value = std::fs::read_to_string(ctx.root.join("usage.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| json!({"models": {}}));
    let mut rows = Vec::new();
    if let Some(models) = v["models"].as_object() {
        for e in models.values() {
            let model = e["model"].as_str().unwrap_or("");
            let (tin, tout) = (
                e["in"].as_u64().unwrap_or(0),
                e["out"].as_u64().unwrap_or(0),
            );
            rows.push(json!({
                "role": e["role"].as_str().unwrap_or(""), "model": model,
                "calls": e["calls"].as_u64().unwrap_or(0), "in": tin, "out": tout,
                "eur": cost::euro_for(ctx, model, tin, tout),
            }));
        }
    }
    rows.sort_by(|a, b| {
        b["eur"]
            .as_f64()
            .unwrap_or(0.0)
            .partial_cmp(&a["eur"].as_f64().unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Value::Array(rows)
}

/// Cost, escalation and budget in one snapshot for the eval screen.
pub fn costs_json(ctx: &Ctx) -> Value {
    let (tasks, escalated) = autoroute::counters(ctx);
    let v: Value = std::fs::read_to_string(ctx.root.join("usage.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| json!({"models": {}}));
    json!({
        "total_eur": cost::total_eur(ctx),
        "month_eur": cost::month_eur(ctx),
        "budget": cost::budget_summary(ctx),
        "monthly_budget_eur": ctx.cfg["monthly_budget_eur"],
        "eur_per_usd": ctx.cfg["eur_per_usd"],
        "rows": usage_rows(ctx),
        "route": {"tasks": tasks, "escalated": escalated, "rate": if tasks > 0 { escalated as f64 * 100.0 / tasks as f64 } else { 0.0 },
                   "reasons": v["route"]["reasons"]},
        "privacy": v["privacy"],
        "subscription": crate::auth::claude_status(),
        "summary": cost::summary(ctx),
    })
}

/// Every cluster as a tree node with its parts and its cost. The cost comes from the `cluster:NAME` rows of usage.json.
pub fn clusters_json(ctx: &Ctx) -> Value {
    let list = clusters::list(ctx);
    let usage: Value = std::fs::read_to_string(ctx.root.join("usage.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| json!({"models": {}}));
    let cluster_rows: Vec<&Value> = usage["models"]
        .as_object()
        .map(|m| m.values().collect())
        .unwrap_or_default();
    let mut out = Vec::new();
    for c in list {
        let role = format!("cluster:{}", c.name);
        let mut eur = 0.0;
        let mut calls = 0u64;
        for e in &cluster_rows {
            if e["role"].as_str() != Some(role.as_str()) {
                continue;
            }
            let model = e["model"].as_str().unwrap_or("");
            let (tin, tout) = (
                e["in"].as_u64().unwrap_or(0),
                e["out"].as_u64().unwrap_or(0),
            );
            eur += cost::euro_for(ctx, model, tin, tout).unwrap_or(0.0);
            calls += e["calls"].as_u64().unwrap_or(0);
        }
        out.push(json!({
            "id": c.id, "name": c.name, "profile": c.profile, "parent": c.parent, "depth": c.depth,
            "state": c.state, "done": c.done, "total": c.total, "started": c.started, "updated": c.updated,
            "task": c.task, "error": c.error, "worktree": c.worktree, "branch": c.branch,
            "shadow": c.shadow, "shadow_of": c.shadow_of, "match_rate": c.match_rate,
            "quorum_agreed": c.quorum_agreed, "quorum_escalated": c.quorum_escalated,
            "parts": c.parts.iter().map(|p| json!({"name": p.name, "slot": p.slot, "role": p.role, "model": p.model,
                                                    "state": p.state, "error": p.error})).collect::<Vec<_>>(),
            "eur": eur, "calls": calls,
        }));
    }
    Value::Array(out)
}

/// The eval runs under `.penelope/eval/runs`, newest first, one row per setup.
pub fn eval_runs(ctx: &Ctx) -> Value {
    let dir = ctx.root.join("eval").join("runs");
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
        .map(|d| {
            d.flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("json"))
                .collect()
        })
        .unwrap_or_default();
    files.sort_by_key(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok());
    files.reverse();
    let mut out = Vec::new();
    for path in files.into_iter().take(30) {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        let results = v["results"].as_array().cloned().unwrap_or_default();
        for r in results {
            let tasks = r["tasks"].as_array().cloned().unwrap_or_default();
            let passed = tasks
                .iter()
                .filter(|t| t["passed"].as_bool().unwrap_or(false))
                .count();
            out.push(json!({
                "id": r["run_id"].as_str().or_else(|| v["id"].as_str()).unwrap_or(""),
                "setup": r["setup"].as_str().unwrap_or(""), "model": r["model"].as_str().unwrap_or(""),
                "started": v["started"].as_str().unwrap_or(""), "tasks": tasks.len(), "passed": passed,
                "eur": tasks.iter().filter_map(|t| t["eur"].as_f64()).sum::<f64>(),
                "wall_ms": tasks.iter().filter_map(|t| t["wall_ms"].as_u64()).sum::<u64>(),
            }));
        }
    }
    Value::Array(out)
}

/// The last eval run as one object, or null.
pub fn last_eval(ctx: &Ctx) -> Value {
    eval_runs(ctx)
        .as_array()
        .and_then(|a| a.first().cloned())
        .unwrap_or(Value::Null)
}

/// One cluster by id, with its report text. `None` when the id is unknown.
pub fn cluster_detail(ctx: &Ctx, id: &str) -> Option<Value> {
    let list = clusters_json(ctx);
    let found = list
        .as_array()?
        .iter()
        .find(|c| c["id"].as_str() == Some(id))?
        .clone();
    let status = clusters::status_text(ctx, Some(id));
    Some(json!({"cluster": found, "status": status,
                "report": std::fs::read_to_string(ctx.root.join("clusters").join(id).join("report.txt")).unwrap_or_default()}))
}

/// The attachment store as cards: name, size, type, origin, level and a preview of the summary.
pub fn attachments_json(ctx: &Ctx) -> Value {
    let mut out = Vec::new();
    for a in crate::attach::list(ctx) {
        let origin = crate::attach::origin(&a);
        let uri = a.uri();
        out.push(json!({
            "sha256": a.sha256, "name": a.name, "bytes": a.bytes, "mime": a.mime, "lines": a.lines,
            "level": a.level, "origin": origin, "summary": a.summary, "uri": uri, "links": a.links,
        }));
    }
    out.sort_by(|a: &Value, b: &Value| {
        b["bytes"]
            .as_u64()
            .unwrap_or(0)
            .cmp(&a["bytes"].as_u64().unwrap_or(0))
    });
    Value::Array(out)
}

/// The cost mode and the last logged escalation.
pub fn route_state(ctx: &Ctx) -> Value {
    let v: Value = std::fs::read_to_string(ctx.root.join("usage.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| json!({"models": {}}));
    let (tasks, escalated) = autoroute::counters(ctx);
    let last = v["route"]["log"]
        .as_array()
        .and_then(|l| l.last())
        .cloned()
        .unwrap_or(Value::Null);
    json!({
        "mode": if autoroute::is_auto(ctx) { "auto" } else { "fixed" },
        "active": autoroute::active(ctx),
        "tasks": tasks, "escalated": escalated,
        "rate": if tasks > 0 { escalated as f64 * 100.0 / tasks as f64 } else { 0.0 },
        "reasons": v["route"]["reasons"], "last": last,
        "label": route_label(ctx, None),
    })
}

/// The line under the composer: fixed, or auto with the model and the escalation.
pub fn route_label(ctx: &Ctx, chat: Option<&backend::Chat>) -> String {
    autoroute::label(ctx, chat.map(|c| c.route_id())).unwrap_or_else(|| {
        format!(
            "fixed · {} {}",
            ctx.cfg["main"]["model"].as_str().unwrap_or("?"),
            ctx.cfg["main"]["effort"].as_str().unwrap_or("")
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_settings_patch_refuses_a_bad_price_and_keeps_keys_out() {
        let dir =
            std::env::temp_dir().join(format!("penelope-api-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut ctx = load(&dir);
        assert!(save_settings(
            &mut ctx,
            &json!({"sandbox": "bwrap", "api_key": "sk-secret"})
        )
        .is_ok());
        assert_eq!(ctx.cfg["sandbox"], "bwrap");
        assert!(ctx.cfg["api_key"].is_null());
        assert!(save_settings(&mut ctx, &json!({"prices": {"x": [1]}})).is_err());
        assert!(save_settings(&mut ctx, &json!({"prices": {"x": [1.0, 2.0]}})).is_ok());
        assert_eq!(ctx.cfg["prices"]["x"], json!([1.0, 2.0]));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_provider_rows_never_carry_a_key() {
        let dir =
            std::env::temp_dir().join(format!("penelope-api-providers-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let ctx = load(&dir);
        let rows = provider_rows(&ctx);
        let text = rows.to_string();
        assert!(text.contains("claude-cli") && text.contains("state"));
        assert!(!text.contains("sk-"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
