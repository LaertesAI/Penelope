//! Eurykleia, the watcher of tool calls. She checks every call before it runs, without a model and without network.
//!
//! Checks, in this order, each with a short code:
//! `E_NAME` unknown tool, `E_REQ` missing required argument, `E_TYPE` wrong JSON type, `E_EXTRA` argument not in the
//! schema, `E_FMT` value outside a format the schema or the description states (date, date-time, "City, State",
//! e-mail, URL, enum, integer range). A failed call does not run; the model reads one line such as
//! `NOT run. E_REQ path: required by read.` The third failed call of one tool in one turn ends in `E_STRIKES`, and
//! the tool stays closed for the rest of the turn, so the harness or a cluster can escalate.
//! Config `eurykleia`: `on` (default) or `off`. Counts per code go to `stats.json` under `eurykleia`.

use crate::config::Ctx;
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::HashMap;

/// Failed calls of one tool after which the watcher closes it for the turn.
pub const MAX_STRIKES: usize = 3;

thread_local! {
    /// Failed calls per tool name in the current user turn on this thread.
    static STRIKES: RefCell<HashMap<String, usize>> = RefCell::new(HashMap::new());
}

/// Starts a new user turn: every tool is open again.
pub fn begin_turn() {
    STRIKES.with(|s| s.borrow_mut().clear());
}

/// One failed check: the code, the argument it names and the line the model reads.
#[derive(Debug, Clone, PartialEq)]
pub struct Fail {
    pub code: &'static str,
    pub arg: String,
    pub text: String,
}

fn fail(code: &'static str, arg: &str, what: String) -> Fail {
    Fail { code, arg: arg.to_string(), text: format!("NOT run. {} {}: {}", code, arg, what) }
}

pub fn enabled(ctx: &Ctx) -> bool {
    match &ctx.cfg["eurykleia"] {
        Value::String(s) => !matches!(s.as_str(), "off" | "false" | "no" | "0"),
        Value::Bool(b) => *b,
        _ => true,
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Pass {
    Req,
    Type,
    Extra,
    Fmt,
}

const PASSES: [Pass; 4] = [Pass::Req, Pass::Type, Pass::Extra, Pass::Fmt];

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(n) if n.is_i64() || n.is_u64() => "integer",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn fits(kind: &str, v: &Value) -> bool {
    match kind {
        "string" => v.is_string(),
        "integer" => v.as_i64().is_some() || v.as_u64().is_some(),
        "number" => v.is_number(),
        "boolean" => v.is_boolean(),
        "array" => v.is_array(),
        "object" => v.is_object(),
        _ => true,
    }
}

fn join(path: &str, key: &str) -> String {
    if path.is_empty() { key.to_string() } else { format!("{}.{}", path, key) }
}

/// Checks the arguments of one call against a schema. `label` names the tool in the message.
pub fn check_schema(label: &str, schema: &Value, args: &Value) -> Result<(), Fail> {
    let empty = json!({});
    let args = if args.is_null() { &empty } else { args };
    if !args.is_object() {
        return Err(fail("E_TYPE", "arguments", format!("expected object, got {}.", type_name(args))));
    }
    for pass in PASSES {
        walk(pass, label, schema, args, "")?;
    }
    Ok(())
}

fn walk(pass: Pass, label: &str, schema: &Value, value: &Value, path: &str) -> Result<(), Fail> {
    let kind = schema["type"].as_str().unwrap_or("");
    match pass {
        Pass::Type if !kind.is_empty() && !fits(kind, value) => {
            return Err(fail("E_TYPE", path, format!("expected {}, got {}.", kind, type_name(value))));
        }
        Pass::Fmt => value_format(schema, value, path)?,
        _ => {}
    }
    match value {
        Value::Object(obj) => {
            let props = schema["properties"].as_object();
            if pass == Pass::Req {
                for key in schema["required"].as_array().into_iter().flatten().filter_map(|v| v.as_str()) {
                    match obj.get(key) {
                        None | Some(Value::Null) => return Err(fail("E_REQ", &join(path, key), format!("required by {}.", label))),
                        Some(Value::String(s)) if s.is_empty() => return Err(fail("E_REQ", &join(path, key), format!("required by {}, got an empty string.", label))),
                        _ => {}
                    }
                }
            }
            if pass == Pass::Extra {
                if let Some(props) = props {
                    if !matches!(schema["additionalProperties"], Value::Bool(true) | Value::Object(_)) {
                        for key in obj.keys() {
                            if !props.contains_key(key) {
                                return Err(fail("E_EXTRA", &join(path, key), format!("not an argument of {}.", label)));
                            }
                        }
                    }
                }
            }
            if let Some(props) = props {
                for (key, sub) in props {
                    if let Some(v) = obj.get(key).filter(|v| !v.is_null()) {
                        walk(pass, label, sub, v, &join(path, key))?;
                    }
                }
            }
        }
        Value::Array(items) => {
            if let Some(sub) = schema.get("items").filter(|s| s.is_object()) {
                for (i, item) in items.iter().enumerate() {
                    walk(pass, label, sub, item, &format!("{}[{}]", path, i))?;
                }
            }
        }
        _ => {}
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq)]
enum Hint {
    Date,
    DateTime,
    City,
    Email,
    Url,
}

/// The format a property states in its schema or in its description, if any.
fn hint_of(schema: &Value) -> Option<Hint> {
    match schema["format"].as_str() {
        Some("date") => return Some(Hint::Date),
        Some("date-time") => return Some(Hint::DateTime),
        Some("email") => return Some(Hint::Email),
        Some("uri" | "url") => return Some(Hint::Url),
        _ => {}
    }
    let d = schema["description"].as_str()?.to_lowercase();
    if d.contains("yyyy-mm-ddt") || d.contains("iso 8601 date-time") || d.contains("iso date-time") || d.contains("iso datetime") {
        Some(Hint::DateTime)
    } else if d.contains("yyyy-mm-dd") {
        Some(Hint::Date)
    } else if d.contains("city, state") || d.contains("city, country") || d.contains("city and state") {
        Some(Hint::City)
    } else if d.contains("email address") || d.contains("e-mail address") {
        Some(Hint::Email)
    } else if d.split(|c: char| !c.is_ascii_alphanumeric()).any(|w| w == "url" || w == "uri") {
        Some(Hint::Url)
    } else {
        None
    }
}

fn value_format(schema: &Value, value: &Value, path: &str) -> Result<(), Fail> {
    if let Some(list) = schema["enum"].as_array() {
        if !list.iter().any(|v| v == value) {
            let options = list.iter().map(|v| v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string())).collect::<Vec<_>>().join(", ");
            return Err(fail("E_FMT", path, format!("expected one of {}, got {}.", options, value)));
        }
    }
    if let Some(n) = value.as_f64().filter(|_| value.is_number()) {
        if let Some(min) = schema["minimum"].as_f64() {
            if n < min {
                return Err(fail("E_FMT", path, format!("expected at least {}, got {}.", min, value)));
            }
        }
        if let Some(max) = schema["maximum"].as_f64() {
            if n > max {
                return Err(fail("E_FMT", path, format!("expected at most {}, got {}.", max, value)));
            }
        }
    }
    if let Some(s) = value.as_str().filter(|s| !s.is_empty()) {
        let (ok, want) = match hint_of(schema) {
            Some(Hint::Date) => (is_date(s), "a date as YYYY-MM-DD"),
            Some(Hint::DateTime) => (is_datetime(s), "an ISO date-time such as 2026-03-01T09:30:00Z"),
            Some(Hint::City) => (is_city(s), "\"City, State\" or \"City, Country\""),
            Some(Hint::Email) => (is_email(s), "an e-mail address"),
            Some(Hint::Url) => (is_url(s), "a URL with http:// or https://"),
            None => (true, ""),
        };
        if !ok {
            return Err(fail("E_FMT", path, format!("expected {}.", want)));
        }
    }
    Ok(())
}

fn digits(s: &str, n: usize) -> Option<u32> {
    if s.len() == n && s.bytes().all(|b| b.is_ascii_digit()) { s.parse().ok() } else { None }
}

pub fn is_date(s: &str) -> bool {
    let mut p = s.split('-');
    let (Some(y), Some(m), Some(d), None) = (p.next(), p.next(), p.next(), p.next()) else { return false };
    let (Some(y), Some(m), Some(d)) = (digits(y, 4), digits(m, 2), digits(d, 2)) else { return false };
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let days = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => if leap { 29 } else { 28 },
        _ => return false,
    };
    (1..=days).contains(&d)
}

pub fn is_datetime(s: &str) -> bool {
    let Some((date, rest)) = s.split_once(['T', 't', ' ']) else { return false };
    if !is_date(date) {
        return false;
    }
    // Split off the zone: Z, +HH:MM, -HH:MM or +HHMM.
    let (time, zone) = match rest.find(['Z', 'z', '+', '-']) {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    let mut t = time.split(':');
    let (Some(h), Some(mi)) = (t.next(), t.next()) else { return false };
    let sec = t.next();
    if t.next().is_some() {
        return false;
    }
    if !matches!(digits(h, 2), Some(h) if h < 24) || !matches!(digits(mi, 2), Some(m) if m < 60) {
        return false;
    }
    if let Some(sec) = sec {
        let (whole, frac) = sec.split_once('.').unwrap_or((sec, "0"));
        if !matches!(digits(whole, 2), Some(s) if s < 61) || frac.is_empty() || !frac.bytes().all(|b| b.is_ascii_digit()) {
            return false;
        }
    }
    match zone {
        "" | "Z" | "z" => true,
        z => {
            let body = &z[1..];
            let (hh, mm) = match body.split_once(':') {
                Some((a, b)) => (a, b),
                None if body.len() == 4 => (&body[..2], &body[2..]),
                None => (body, "00"),
            };
            matches!(digits(hh, 2), Some(h) if h < 24) && matches!(digits(mm, 2), Some(m) if m < 60)
        }
    }
}

pub fn is_city(s: &str) -> bool {
    let parts: Vec<&str> = s.split(',').map(str::trim).collect();
    parts.len() == 2 && parts.iter().all(|p| !p.is_empty() && p.chars().any(|c| c.is_alphabetic()))
}

pub fn is_email(s: &str) -> bool {
    let Some((local, domain)) = s.split_once('@') else { return false };
    !local.is_empty() && !s.chars().any(char::is_whitespace) && !domain.contains('@') && domain.contains('.')
        && !domain.starts_with('.') && !domain.ends_with('.') && !domain.contains("..")
}

pub fn is_url(s: &str) -> bool {
    let Some((scheme, rest)) = s.split_once("://") else { return false };
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    !scheme.is_empty() && scheme.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        && !host.is_empty() && !s.chars().any(char::is_whitespace)
}

/// The schema of a tool: built in, forged for this project, or none.
fn schema_of(ctx: &Ctx, name: &str) -> Option<Value> {
    crate::spec::schema(name).or_else(|| crate::forge::schema(ctx, name))
}

/// Checks one call. Reads schemas only: no model, no network, no process.
pub fn check(ctx: &Ctx, name: &str, args: &Value) -> Result<(), Fail> {
    let Some(schema) = schema_of(ctx, name) else {
        return Err(fail("E_NAME", name, "no tool of this name.".to_string()));
    };
    check_schema(name, &schema, args)?;
    if name == "mcp_call" {
        check_mcp(ctx, args)?;
    }
    Ok(())
}

/// The schema of an MCP tool as the client holds it. A server that is not running, or a Mitos index entry without a
/// schema, is not checked here; the server itself answers then.
fn check_mcp(ctx: &Ctx, args: &Value) -> Result<(), Fail> {
    let (Some(server), Some(tool)) = (args["server"].as_str(), args["tool"].as_str()) else { return Ok(()) };
    let Some(tools) = crate::mcpclient::cached_tools(ctx, server) else { return Ok(()) };
    let Some(entry) = tools.iter().find(|t| t["name"].as_str() == Some(tool)) else {
        return Err(fail("E_NAME", "tool", format!("{} has no tool {}.", server, tool)));
    };
    let schema = if entry["inputSchema"].is_null() { &entry["input_schema"] } else { &entry["inputSchema"] };
    if !schema.is_object() || crate::mitos::is_lazy(entry) {
        return Ok(());
    }
    check_schema(&format!("{}.{}", server, tool), schema, &args["arguments"]).map_err(|f| {
        let arg = if f.arg == "arguments" { f.arg.clone() } else { join("arguments", &f.arg) };
        let text = f.text.replacen(&format!("{} {}:", f.code, f.arg), &format!("{} {}:", f.code, arg), 1);
        Fail { code: f.code, arg, text }
    })
}

/// The gate in front of a tool call. `None` lets the call run. `Some(text)` is the refusal the model reads; the call
/// does not run. Counts the check in `stats.json`.
pub fn guard(ctx: &Ctx, name: &str, args: &Value) -> Option<String> {
    if !enabled(ctx) {
        return None;
    }
    let before = STRIKES.with(|s| s.borrow().get(name).copied().unwrap_or(0));
    if before >= MAX_STRIKES {
        record(ctx, Some("E_STRIKES"));
        return Some(strikes_text(name, None));
    }
    match check(ctx, name, args) {
        Ok(()) => {
            record(ctx, None);
            None
        }
        Err(f) => {
            record(ctx, Some(f.code));
            let now = before + 1;
            STRIKES.with(|s| s.borrow_mut().insert(name.to_string(), now));
            if now >= MAX_STRIKES {
                return Some(strikes_text(name, Some(&f)));
            }
            Some(f.text)
        }
    }
}

fn strikes_text(name: &str, last: Option<&Fail>) -> String {
    match last {
        Some(f) => format!("NOT run. E_STRIKES {}: the watcher stops this tool for the turn after {} failed calls (last {} {}).", name, MAX_STRIKES, f.code, f.arg),
        None => format!("NOT run. E_STRIKES {}: the watcher stops this tool for the turn after {} failed calls.", name, MAX_STRIKES),
    }
}

/// Adds one check to `stats.json`: `checks`, `passes`, `fails` and `codes`, a count per code.
fn record(ctx: &Ctx, code: Option<&str>) {
    if !ctx.root.is_dir() {
        return;
    }
    let path = ctx.root.join("stats.json");
    let mut v: Value = std::fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_else(|| json!({}));
    let e = &mut v["eurykleia"];
    let n = |x: &Value| x.as_u64().unwrap_or(0);
    e["checks"] = json!(n(&e["checks"]) + 1);
    match code {
        None => e["passes"] = json!(n(&e["passes"]) + 1),
        Some(c) => {
            e["fails"] = json!(n(&e["fails"]) + 1);
            e["codes"][c] = json!(n(&e["codes"][c]) + 1);
        }
    }
    if let Ok(text) = serde_json::to_string_pretty(&v) {
        let _ = std::fs::write(path, text);
    }
}

/// The line for `penelope stats`.
pub fn stats_line(ctx: &Ctx) -> String {
    let v: Value = std::fs::read_to_string(ctx.root.join("stats.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    let e = &v["eurykleia"];
    let codes = e["codes"].as_object().map(|o| o.iter().map(|(k, n)| format!("{} {}", k, n)).collect::<Vec<_>>().join(", ")).unwrap_or_default();
    format!("eurykleia: {}, {} checks, {} passes, {} fails{}\n", if enabled(ctx) { "on" } else { "off" },
            e["checks"].as_u64().unwrap_or(0), e["passes"].as_u64().unwrap_or(0), e["fails"].as_u64().unwrap_or(0),
            if codes.is_empty() { String::new() } else { format!(" ({})", codes) })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(tag: &str) -> Ctx {
        let root = std::env::temp_dir().join(format!("penelope-eurykleia-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        crate::config::load(&root)
    }

    fn code(r: Result<(), Fail>) -> String {
        r.err().map(|f| format!("{} {}", f.code, f.arg)).unwrap_or_default()
    }

    #[test]
    fn e_name_unknown_tool() {
        let c = ctx("name");
        let f = check(&c, "reed", &json!({})).unwrap_err();
        assert_eq!(f.text, "NOT run. E_NAME reed: no tool of this name.");
    }

    #[test]
    fn e_req_missing_argument() {
        let c = ctx("req");
        let f = check(&c, "read", &json!({})).unwrap_err();
        assert_eq!(f.text, "NOT run. E_REQ path: required by read.");
        assert_eq!(code(check(&c, "edit", &json!({"path": "a", "edits": [{"old": "x"}]}))), "E_REQ edits[0].new");
    }

    #[test]
    fn e_type_wrong_json_type() {
        let c = ctx("type");
        let f = check(&c, "read", &json!({"path": 7})).unwrap_err();
        assert_eq!(f.text, "NOT run. E_TYPE path: expected string, got integer.");
        assert_eq!(code(check(&c, "read", &json!({"path": "a", "limit": "many"}))), "E_TYPE limit");
        assert_eq!(code(check(&c, "read", &json!(["a"]))), "E_TYPE arguments");
    }

    #[test]
    fn e_extra_argument_outside_the_schema() {
        let c = ctx("extra");
        let f = check(&c, "read", &json!({"path": "a", "colour": "red"})).unwrap_err();
        assert_eq!(f.text, "NOT run. E_EXTRA colour: not an argument of read.");
        assert_eq!(code(check(&c, "plan", &json!({"items": [{"text": "a", "state": "done", "id": 1}]}))), "E_EXTRA items[0].id");
        // An open object takes any key.
        assert!(check(&c, "http_request", &json!({"url": "https://a.example", "headers": {"X-Any": "1"}})).is_ok());
    }

    #[test]
    fn e_fmt_enum_range_and_described_formats() {
        let c = ctx("fmt");
        assert_eq!(code(check(&c, "memory_note", &json!({"text": "x", "scope": "everywhere"}))), "E_FMT scope");
        let schema = json!({"type": "object", "required": ["day"], "properties": {
            "day": {"type": "string", "description": "Day as YYYY-MM-DD."},
            "at": {"type": "string", "description": "ISO 8601 date-time."},
            "where": {"type": "string", "description": "City, State, for example Austin, TX."},
            "to": {"type": "string", "description": "The email address of the recipient."},
            "link": {"type": "string", "format": "uri"},
            "n": {"type": "integer", "minimum": 1, "maximum": 20}}});
        let ok = json!({"day": "2024-02-29", "at": "2026-03-01T09:30:00Z", "where": "Austin, TX", "to": "a@b.example", "link": "https://b.example/x", "n": 20});
        assert!(check_schema("t", &schema, &ok).is_ok());
        let bad = |k: &str, v: Value| { let mut a = ok.clone(); a[k] = v; code(check_schema("t", &schema, &a)) };
        assert_eq!(bad("day", json!("2023-02-29")), "E_FMT day");
        assert_eq!(bad("day", json!("01.03.2026")), "E_FMT day");
        assert_eq!(bad("at", json!("2026-03-01")), "E_FMT at");
        assert_eq!(bad("at", json!("2026-03-01T25:00:00Z")), "E_FMT at");
        assert_eq!(bad("where", json!("Austin")), "E_FMT where");
        assert_eq!(bad("to", json!("a.b.example")), "E_FMT to");
        assert_eq!(bad("link", json!("b.example/x")), "E_FMT link");
        assert_eq!(bad("n", json!(0)), "E_FMT n");
        assert_eq!(bad("n", json!(21)), "E_FMT n");
        for good in ["2026-03-01T09:30", "2026-03-01T09:30:00.5+02:00", "2026-03-01 09:30:00-0500"] {
            assert!(is_datetime(good), "{}", good);
        }
    }

    #[test]
    fn checks_run_in_the_stated_order() {
        let c = ctx("order");
        // Missing path and a stray argument: E_REQ comes first. A wrong type and a stray argument: E_TYPE first.
        assert_eq!(code(check(&c, "read", &json!({"stray": 1}))), "E_REQ path");
        assert_eq!(code(check(&c, "read", &json!({"path": 1, "stray": 1}))), "E_TYPE path");
        assert_eq!(code(check(&c, "memory_note", &json!({"text": "x", "scope": "no", "stray": 1}))), "E_EXTRA stray");
    }

    #[test]
    fn a_valid_call_passes_and_runs_unchanged() {
        let c = ctx("valid");
        let dir = c.root.join("w");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
        let args = json!({"path": "a.txt", "cwd": dir.to_string_lossy()});
        assert!(guard(&c, "read", &args).is_none());
        let on = crate::tools::dispatch_retry(&c, "lead", "read", &args);
        let mut off_ctx = c.clone();
        off_ctx.cfg["eurykleia"] = json!("off");
        let off = crate::tools::dispatch_retry(&off_ctx, "lead", "read", &args);
        assert_eq!(on, off);
        assert!(on.1 && on.0.contains("one"), "{:?}", on);
    }

    #[test]
    fn three_failed_calls_close_the_tool_for_the_turn() {
        let c = ctx("strikes");
        begin_turn();
        let a = guard(&c, "read", &json!({})).unwrap();
        let b = guard(&c, "read", &json!({})).unwrap();
        assert!(a.starts_with("NOT run. E_REQ path") && b.starts_with("NOT run. E_REQ path"));
        let third = guard(&c, "read", &json!({})).unwrap();
        assert!(third.starts_with("NOT run. E_STRIKES read:") && third.contains("stops this tool for the turn"), "{}", third);
        // Now even a valid call is stopped, other tools are not, and a new turn opens the tool again.
        assert!(guard(&c, "read", &json!({"path": "a"})).unwrap().starts_with("NOT run. E_STRIKES read"));
        assert!(guard(&c, "tree", &json!({})).is_none());
        begin_turn();
        assert!(guard(&c, "read", &json!({"path": "a"})).is_none());
        let said = crate::tools::dispatch_retry(&c, "lead", "read", &json!({}));
        assert!(!said.1 && said.0.starts_with("NOT run. E_REQ"));
    }

    #[test]
    fn off_disables_the_watcher() {
        let mut c = ctx("off");
        c.cfg["eurykleia"] = json!("off");
        begin_turn();
        for _ in 0..5 {
            assert!(guard(&c, "read", &json!({})).is_none());
        }
        assert!(!enabled(&c));
        assert!(!c.root.join("stats.json").exists());
    }

    #[test]
    fn the_counts_land_in_stats() {
        let c = ctx("stats");
        begin_turn();
        guard(&c, "tree", &json!({}));
        guard(&c, "read", &json!({}));
        guard(&c, "nope", &json!({}));
        let v: Value = serde_json::from_str(&std::fs::read_to_string(c.root.join("stats.json")).unwrap()).unwrap();
        let e = &v["eurykleia"];
        assert_eq!((e["checks"].as_u64(), e["passes"].as_u64(), e["fails"].as_u64()), (Some(3), Some(1), Some(2)));
        assert_eq!(e["codes"]["E_REQ"], 1);
        assert_eq!(e["codes"]["E_NAME"], 1);
        assert!(stats_line(&c).contains("3 checks, 1 passes, 2 fails"));
    }

    #[test]
    fn every_built_in_tool_accepts_its_own_minimal_call() {
        let c = ctx("builtin");
        for t in crate::spec::TOOLS {
            let schema: Value = serde_json::from_str(t.schema).unwrap();
            let mut args = serde_json::Map::new();
            for (k, sub) in schema["properties"].as_object().unwrap() {
                if !schema["required"].as_array().unwrap_or(&vec![]).iter().any(|r| r == k) {
                    continue;
                }
                let v = match sub["type"].as_str().unwrap_or("string") {
                    "integer" => json!(1),
                    "boolean" => json!(true),
                    "array" => json!(["x"]),
                    "object" => json!({"task": "t"}),
                    _ => sub["enum"][0].as_str().map(|s| json!(s)).unwrap_or(json!("https://a.example/x")),
                };
                args.insert(k.clone(), v);
            }
            // Nested required fields need their own values; only the top level is checked here.
            let r = check(&c, t.name, &Value::Object(args));
            if let Err(f) = &r {
                assert!(f.arg.contains('[') || f.arg.contains('.'), "{}: {}", t.name, f.text);
            }
        }
    }
}
