//! What a session costs, in euro.
//!
//! Every model call adds its tokens to `.penelope/usage.json`, under the role and the model that spent them. The price
//! table in `config.json` names dollars per million tokens; a model without a price is counted and marked, never
//! guessed. The rate `eur_per_usd` turns dollars into euro.

use crate::config::Ctx;
use serde_json::{json, Value};
use std::fs;
use std::sync::Mutex;

/// Parallel clusters add their rows at the same time; one writer at a time keeps usage.json readable.
static USAGE_LOCK: Mutex<()> = Mutex::new(());

fn key(role: &str, model: &str) -> String {
    format!("{}|{}", role, model)
}

pub fn record(ctx: &Ctx, role: &str, model: &str, tokens_in: u64, tokens_out: u64) {
    if model.is_empty() || (tokens_in == 0 && tokens_out == 0) {
        return;
    }
    let _guard = USAGE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let path = ctx.root.join("usage.json");
    let mut v: Value = fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_else(|| json!({"models": {}}));
    if !v["models"].is_object() {
        v["models"] = json!({});
    }
    let e = &mut v["models"][key(role, model)];
    *e = json!({
        "role": role, "model": model,
        "calls": e["calls"].as_u64().unwrap_or(0) + 1,
        "in": e["in"].as_u64().unwrap_or(0) + tokens_in,
        "out": e["out"].as_u64().unwrap_or(0) + tokens_out
    });
    let month = crate::sessions::stamp(now()).chars().take(7).collect::<String>();
    if !v["months"].is_object() { v["months"] = json!({}); }
    let me = &mut v["months"][&month]["models"][key(role, model)];
    *me = json!({
        "role": role, "model": model,
        "calls": me["calls"].as_u64().unwrap_or(0) + 1,
        "in": me["in"].as_u64().unwrap_or(0) + tokens_in,
        "out": me["out"].as_u64().unwrap_or(0) + tokens_out
    });
    let _ = fs::write(path, serde_json::to_string_pretty(&v).unwrap());
}

/// Reads `usage.json`, lets `f` change it and writes it back, under the same lock as `record`.
pub fn update(ctx: &Ctx, f: impl FnOnce(&mut Value)) {
    if !ctx.root.is_dir() {
        return;
    }
    let _guard = USAGE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let path = ctx.root.join("usage.json");
    let mut v: Value = fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_else(|| json!({"models": {}}));
    f(&mut v);
    if let Ok(text) = serde_json::to_string_pretty(&v) {
        let _ = fs::write(path, text);
    }
}

fn price(ctx: &Ctx, model: &str) -> Option<(f64, f64)> {
    let table = ctx.cfg["prices"].as_object()?;
    let lower = model.to_lowercase();
    let mut best: Option<(usize, f64, f64)> = None;
    for (name, p) in table {
        if !lower.contains(&name.to_lowercase()) {
            continue;
        }
        let (Some(i), Some(o)) = (p[0].as_f64(), p[1].as_f64()) else { continue };
        if best.map(|(l, _, _)| name.len() > l).unwrap_or(true) {
            best = Some((name.len(), i, o));
        }
    }
    best.map(|(_, i, o)| (i, o))
}

/// One row per role and model, then the total. `cost` is in euro, four decimals.
pub fn report(ctx: &Ctx) -> String {
    let v: Value = fs::read_to_string(ctx.root.join("usage.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_else(|| json!({"models": {}}));
    let rate = ctx.cfg["eur_per_usd"].as_f64().unwrap_or(0.92);
    let mut rows: Vec<(String, String, u64, u64, u64, Option<f64>)> = Vec::new();
    if let Some(models) = v["models"].as_object() {
        for (_, e) in models {
            let role = e["role"].as_str().unwrap_or("?").to_string();
            let model = e["model"].as_str().unwrap_or("?").to_string();
            let (ci, co) = (e["in"].as_u64().unwrap_or(0), e["out"].as_u64().unwrap_or(0));
            let cost = price(ctx, &model).map(|(i, o)| (ci as f64 * i + co as f64 * o) / 1_000_000.0 * rate);
            rows.push((role, model, e["calls"].as_u64().unwrap_or(0), ci, co, cost));
        }
    }
    rows.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    let mut out = format!("{:<10} {:<24} {:>6} {:>10} {:>9} {:>10}\n", "role", "model", "calls", "tokens in", "tokens out", "euro");
    let (mut total, mut unpriced) = (0.0, 0);
    for (role, model, calls, ci, co, cost) in &rows {
        match cost {
            Some(c) => total += c,
            None => unpriced += calls,
        }
        out.push_str(&format!("{:<10} {:<24} {:>6} {:>10} {:>9} {:>10}\n", role, model.chars().take(24).collect::<String>(), calls, ci, co,
                              cost.map(|c| format!("{:.4}", c)).unwrap_or_else(|| "no price".to_string())));
    }
    if rows.is_empty() {
        out.push_str("no model call is counted yet. The first turn writes the first row.\n");
    }
    let budget = ctx.cfg["monthly_budget_eur"].as_f64().unwrap_or(0.0);
    out.push_str(&format!("\ntotal {:.4} EUR at {:.2} EUR per dollar{}.\n", total, rate,
                          if unpriced > 0 { format!(" · {} calls have no price in config.json (prices)", unpriced) } else { String::new() }));
    out.push_str(&format!("this month: {}{}.\n", crate::cost::budget_summary(ctx),
                          if budget > 0.0 { "" } else { " · set a limit with /budget EUR" }));
    out.push_str("Prices are proposals in config.json, not a bill. Set prices per million tokens in dollars: \"prices\": {\"model-name\": [input, output]}.\n");
    out
}

/// The euro one call costs at the prices in config.json. `None` when the model has no price: a hard euro limit cannot
/// stop a model that is not priced, and the caller must not pretend that it can.
pub fn euro_for(ctx: &Ctx, model: &str, tokens_in: u64, tokens_out: u64) -> Option<f64> {
    let (i, o) = price(ctx, model)?;
    let rate = ctx.cfg["eur_per_usd"].as_f64().unwrap_or(0.92);
    Some((tokens_in as f64 * i + tokens_out as f64 * o) / 1_000_000.0 * rate)
}

/// The hard limit of one whole session in euro. 0 means no limit.
pub fn session_budget(ctx: &Ctx) -> f64 {
    ctx.cfg["session_budget_eur"].as_f64().unwrap_or(0.0).max(0.0)
}

/// All tokens in and out of this folder, from the usage file.
pub fn total_tokens(ctx: &Ctx) -> u64 {
    let v: Value = fs::read_to_string(ctx.root.join("usage.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_else(|| json!({"models": {}}));
    v["models"].as_object().map(|m| m.values().map(|e| e["in"].as_u64().unwrap_or(0) + e["out"].as_u64().unwrap_or(0)).sum()).unwrap_or(0)
}

/// The euro spent so far in this folder. A model without a price adds nothing and is counted in `unpriced` by `summary`.
pub fn total_eur(ctx: &Ctx) -> f64 {
    let v: Value = fs::read_to_string(ctx.root.join("usage.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_else(|| json!({"models": {}}));
    let rate = ctx.cfg["eur_per_usd"].as_f64().unwrap_or(0.92);
    let mut total = 0.0;
    if let Some(models) = v["models"].as_object() {
        for e in models.values() {
            let (ci, co) = (e["in"].as_u64().unwrap_or(0), e["out"].as_u64().unwrap_or(0));
            if let Some((i, o)) = price(ctx, e["model"].as_str().unwrap_or("")) {
                total += (ci as f64 * i + co as f64 * o) / 1_000_000.0 * rate;
            }
        }
    }
    total
}

/// One line for the side panel.
pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Euro spent in the running month, from the monthly rows. Older usage files fall back to the total.
pub fn month_eur(ctx: &Ctx) -> f64 {
    let v: Value = fs::read_to_string(ctx.root.join("usage.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_else(|| json!({"models": {}}));
    let month = crate::sessions::stamp(now()).chars().take(7).collect::<String>();
    let rate = ctx.cfg["eur_per_usd"].as_f64().unwrap_or(0.92);
    let Some(models) = v["months"][&month]["models"].as_object() else { return 0.0; };
    let mut total = 0.0;
    for e in models.values() {
        let (ci, co) = (e["in"].as_u64().unwrap_or(0), e["out"].as_u64().unwrap_or(0));
        if let Some((i, o)) = price(ctx, e["model"].as_str().unwrap_or("")) {
            total += (ci as f64 * i + co as f64 * o) / 1_000_000.0 * rate;
        }
    }
    total
}

/// Consumption and the remaining monthly budget, for the side panel.
pub fn budget_summary(ctx: &Ctx) -> String {
    let budget = ctx.cfg["monthly_budget_eur"].as_f64().unwrap_or(0.0);
    let spent = month_eur(ctx);
    let month = crate::sessions::stamp(now()).chars().take(7).collect::<String>();
    if budget <= 0.0 {
        return format!("{:.3} EUR in {}", spent, month);
    }
    format!("{:.3} EUR in {} · {:.2} left of {:.2}", spent, month, (budget - spent).max(0.0), budget)
}

pub fn summary(ctx: &Ctx) -> String {
    let v: Value = fs::read_to_string(ctx.root.join("usage.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_else(|| json!({"models": {}}));
    let rate = ctx.cfg["eur_per_usd"].as_f64().unwrap_or(0.92);
    let (mut total, mut calls, mut unpriced) = (0.0, 0u64, 0u64);
    if let Some(models) = v["models"].as_object() {
        for e in models.values() {
            let (ci, co) = (e["in"].as_u64().unwrap_or(0), e["out"].as_u64().unwrap_or(0));
            let c = e["calls"].as_u64().unwrap_or(0);
            calls += c;
            match price(ctx, e["model"].as_str().unwrap_or("")) {
                Some((i, o)) => total += (ci as f64 * i + co as f64 * o) / 1_000_000.0 * rate,
                None => unpriced += c,
            }
        }
    }
    if calls == 0 {
        return "no call yet".to_string();
    }
    let budget = ctx.cfg["monthly_budget_eur"].as_f64().unwrap_or(0.0);
    if budget > 0.0 {
        let spent = month_eur(ctx);
        return format!("{:.3} EUR in {} · {:.2} left of {:.2}", spent, crate::sessions::stamp(now()).chars().take(7).collect::<String>(), (budget - spent).max(0.0), budget);
    }
    format!("{:.3} EUR · {} calls{}", total, calls, if unpriced > 0 { format!(" · {} unpriced", unpriced) } else { String::new() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_become_euro_and_an_unknown_model_is_marked_not_guessed() {
        let dir = std::env::temp_dir().join(format!("penelope-cost-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let mut ctx = crate::config::load(&dir.join(".penelope"));
        ctx.cfg["prices"] = json!({"sonnet": [3.0, 15.0]});
        ctx.cfg["eur_per_usd"] = json!(0.5);
        record(&ctx, "lead", "claude-sonnet-5", 1_000_000, 100_000);
        record(&ctx, "lead", "claude-sonnet-5", 0, 0);
        record(&ctx, "scout", "mystery-1", 10, 10);
        let text = report(&ctx);
        assert!(text.contains("lead") && text.contains("2.2500") && text.contains("no price"), "{}", text);
        assert!(text.ends_with("Prices are proposals in config.json, not a bill. Set prices per million tokens in dollars: \"prices\": {\"model-name\": [input, output]}.\n"));
        assert!(text.contains("total 2.2500 EUR at 0.50 EUR per dollar · 1 calls have no price"), "{}", text);
        assert!(summary(&ctx).starts_with("2.250 EUR · 2 calls · 1 unpriced"), "{}", summary(&ctx));
        ctx.cfg["monthly_budget_eur"] = json!(10.0);
        assert!(budget_summary(&ctx).contains("left of 10.00"), "{}", budget_summary(&ctx));
        let _ = fs::remove_dir_all(&dir);
    }
}
