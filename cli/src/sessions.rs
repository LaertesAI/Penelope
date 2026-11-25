//! The list of past conversations. Every turn updates one entry in `sessions.json`, so a conversation can go on
//! later with `penelope chat --resume N`. The claude command keeps the messages of its sessions itself. For an HTTP
//! endpoint the messages lie in `sessions/ID.history.json`.

use crate::config::Ctx;
use serde_json::{json, Value};
use std::fs;

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Seconds since 1970 as `2026-11-24 16:40`, in UTC.
pub fn stamp(secs: u64) -> String {
    let (days, rest) = ((secs / 86400) as i64, secs % 86400);
    let z = days + 719_468;
    let (era, doe) = (z.div_euclid(146_097), z.rem_euclid(146_097));
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let (d, m) = (doy - (153 * mp + 2) / 5 + 1, if mp < 10 { mp + 3 } else { mp - 9 });
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{:04}-{:02}-{:02} {:02}:{:02}", y, m, d, rest / 3600, rest % 3600 / 60)
}

fn load(ctx: &Ctx) -> Vec<Value> {
    fs::read_to_string(ctx.root.join("sessions.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

/// Writes or updates the entry of one conversation. The title is the first request, cut to 70 characters.
pub fn record(ctx: &Ctx, id: &str, request: &str, turns: u64, tokens: (u64, u64)) {
    let mut all = load(ctx);
    let mut entry = match all.iter().position(|s| s["id"].as_str() == Some(id)) {
        Some(i) => all.remove(i),
        None => {
            let title: String = request.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(70).collect();
            let dir = std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default();
            json!({"id": id, "title": title, "dir": dir, "started": now()})
        }
    };
    entry["last"] = json!(now());
    entry["turns"] = json!(turns);
    entry["in"] = json!(tokens.0);
    entry["out"] = json!(tokens.1);
    all.push(entry);                 // the list is ordered by the last turn, newest at the end
    let from = all.len().saturating_sub(200);
    let _ = fs::write(ctx.root.join("sessions.json"), serde_json::to_string_pretty(&all[from..]).unwrap());
}

/// The conversations, newest first, one line each.
pub fn list(ctx: &Ctx) -> String {
    let all = load(ctx);
    if all.is_empty() {
        return "no sessions yet. A session is stored after its first answer.\n".to_string();
    }
    let mut out = String::new();
    for (i, s) in all.iter().rev().enumerate() {
        out.push_str(&format!("{:>3}  {} UTC  {:>3} turns  {:>8} tokens in  {}\n", i + 1, stamp(s["last"].as_u64().unwrap_or(0)),
                              s["turns"].as_u64().unwrap_or(0), s["in"].as_u64().unwrap_or(0), s["title"].as_str().unwrap_or("")));
    }
    out.push_str("\npenelope chat --resume N goes on with session N.\n");
    out
}

/// Every stored conversation, oldest first by its start. The tabs of the interface keep this order.
pub fn all(ctx: &Ctx) -> Vec<Value> {
    let mut all = load(ctx);
    all.sort_by_key(|s| s["started"].as_u64().unwrap_or(0));
    all
}

fn transcript_path(ctx: &Ctx, id: &str) -> std::path::PathBuf {
    let name: String = id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect();
    ctx.root.join("sessions").join(format!("{}.json", name))
}

/// What the interface showed in a conversation, so that a tab can show it again.
pub fn save_transcript(ctx: &Ctx, id: &str, items: &[Value]) {
    let _ = fs::create_dir_all(ctx.root.join("sessions"));
    let _ = fs::write(transcript_path(ctx, id), serde_json::to_string(items).unwrap());
}

pub fn load_transcript(ctx: &Ctx, id: &str) -> Vec<Value> {
    fs::read_to_string(transcript_path(ctx, id)).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

/// An id for a conversation through an HTTP endpoint. The claude command names its own sessions.
pub fn new_id() -> String {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    format!("api-{:x}-{}", nanos, std::process::id())
}

fn history_path(ctx: &Ctx, id: &str) -> std::path::PathBuf {
    let name: String = id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect();
    ctx.root.join("sessions").join(format!("{}.history.json", name))
}

/// Writes the messages of a conversation through an HTTP endpoint, in the wire format of its kind. The file holds
/// requests after the privacy filter, tool results after the privacy filter, and the blocks of the model as they came.
pub fn save_history(ctx: &Ctx, id: &str, kind: &str, model: &str, messages: &[Value]) {
    let _ = fs::create_dir_all(ctx.root.join("sessions"));
    let _ = fs::write(history_path(ctx, id), json!({"kind": kind, "model": model, "messages": messages}).to_string());
}

/// The stored messages of a conversation and their wire format. None for a session of the claude command.
pub fn load_history(ctx: &Ctx, id: &str) -> Option<(String, Vec<Value>)> {
    let v: Value = serde_json::from_str(&fs::read_to_string(history_path(ctx, id)).ok()?).ok()?;
    Some((v["kind"].as_str()?.to_string(), v["messages"].as_array()?.clone()))
}

/// Deletes one conversation from the list, its transcript and its HTTP history. Returns false when the id is unknown.
pub fn delete(ctx: &Ctx, id: &str) -> bool {
    let mut all = load(ctx);
    let before = all.len();
    all.retain(|s| s["id"].as_str() != Some(id));
    let found = all.len() != before;
    if found {
        let _ = fs::write(ctx.root.join("sessions.json"), serde_json::to_string_pretty(&all).unwrap());
        let _ = fs::remove_file(transcript_path(ctx, id));
        let _ = fs::remove_file(history_path(ctx, id));
    }
    found
}

/// A conversation by its number in the list (1 is the newest) or by the start of its id.
pub fn pick(ctx: &Ctx, key: &str) -> Option<Value> {
    let all = load(ctx);
    if let Ok(n) = key.parse::<usize>() {
        return if n >= 1 && n <= all.len() { Some(all[all.len() - n].clone()) } else { None };
    }
    all.into_iter().rev().find(|s| !key.is_empty() && s["id"].as_str().map(|id| id.starts_with(key)).unwrap_or(false))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamps_and_picks() {
        assert_eq!(stamp(0), "1970-01-01 00:00");
        assert_eq!(stamp(1_795_528_800), "2026-11-24 14:00");
        assert_eq!(stamp(951_782_400), "2000-02-29 00:00");
        let root = std::env::temp_dir().join(format!("penelope-sessions-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let ctx = crate::config::load(&root);
        assert!(list(&ctx).starts_with("no sessions yet"));
        record(&ctx, "aaa-1", "first   request\nwith two lines", 1, (100, 10));
        record(&ctx, "bbb-2", "second request", 1, (50, 5));
        record(&ctx, "aaa-1", "ignored", 2, (300, 30));
        let text = list(&ctx);
        assert!(text.lines().next().unwrap().ends_with("first request with two lines") && text.contains("  2 turns"), "{}", text);
        assert_eq!(pick(&ctx, "1").unwrap()["id"], "aaa-1");
        assert_eq!(pick(&ctx, "bbb").unwrap()["turns"], 1);
        assert!(pick(&ctx, "3").is_none() && pick(&ctx, "zzz").is_none());
        let _ = fs::remove_dir_all(&root);
    }
}
