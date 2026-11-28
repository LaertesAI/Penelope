//! Cost mode with automatic escalation. Config `route`: `fixed` (default) or `auto`.
//!
//! In `auto`, a task starts on the weak model with effort low. It moves to the main model with effort high when one
//! trigger fires, and stays there until the next task starts. A task is one `Chat`: a new session, `/new` or a new
//! eval task starts a new one. Triggers, each counted per task:
//! - the test command fails twice in a row (the first failure only raises the weak effort from low to medium),
//! - Eurykleia reports `E_STRIKES` (3 failed calls of one tool in one turn),
//! - the user types `/escalate`, or a message that is only "nochmal", "retry" or "try again",
//! - the context of the last turn exceeds `route_context_limit` tokens (default 100000).
//!
//! Tools may run in another process (the claude command starts an MCP server), so test results and strikes travel
//! as small signed files in a folder of Penelope's own state directory (outside the workspace, one folder per chat).
//! The chat takes each file by renaming it, so no event is lost or read twice. A file without a valid signature is
//! ignored, and `write` and `edit` refuse the state directory. A change applies from the next turn; when the weak
//! turn itself triggered it, `Chat::turn_with` runs one follow-up turn on the main model.
//! Every task, whether it escalated and why go to `usage.json` under `route`.

use crate::config::Ctx;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::sync::Mutex;

pub const DEFAULT_LIMIT: u64 = 100_000;
/// Weak phase effort per step: low, then medium after the first failed test.
const WEAK_EFFORT: [&str; 2] = ["low", "medium"];
const ESCALATED_EFFORT: &str = "high";
/// Failed runs in a row of the test command that escalate.
const TEST_FAILS: u32 = 2;

pub fn is_auto(ctx: &Ctx) -> bool {
    ctx.cfg["route"].as_str().map(|s| s.trim().eq_ignore_ascii_case("auto")).unwrap_or(false)
}

pub fn limit(ctx: &Ctx) -> u64 {
    ctx.cfg["route_context_limit"].as_u64().filter(|n| *n > 0).unwrap_or(DEFAULT_LIMIT)
}

/// The state of one task and the counts of the session.
#[derive(Debug, Clone, Default)]
pub struct Router {
    step: usize,
    fails: u32,
    escalated: Option<String>,
    pub tasks: u32,
    pub escalations: u32,
    begun: bool,
    /// The number of this task in the `usage.json` log, and whether its escalation is written there.
    log_id: u64,
    logged: bool,
}

impl Router {
    /// Starts the task. An escalation asked for before the first turn belongs to this task and stays.
    pub fn new_task(&mut self) {
        if self.begun {
            self.step = 0;
            self.fails = 0;
            self.escalated = None;
            self.logged = false;
        }
        self.begun = true;
        self.tasks += 1;
    }

    #[cfg(test)]
    pub fn reason(&self) -> Option<&str> {
        self.escalated.as_deref()
    }

    /// Moves the task to the main model. Returns the reason only the first time in a task.
    pub fn escalate(&mut self, why: &str) -> Option<String> {
        if self.escalated.is_some() {
            return None;
        }
        self.escalated = Some(why.to_string());
        self.escalations += 1;
        Some(why.to_string())
    }

    /// One run of the test command. Two failures in a row escalate; a pass starts the count again.
    pub fn note_test(&mut self, passed: bool) -> Option<String> {
        if passed {
            self.fails = 0;
            return None;
        }
        self.fails += 1;
        if self.fails >= TEST_FAILS {
            return self.escalate("tests failed 2x");
        }
        self.step = (self.step + 1).min(WEAK_EFFORT.len() - 1);
        None
    }

    pub fn note_strikes(&mut self) -> Option<String> {
        self.escalate("Eurykleia 3 strikes")
    }

    pub fn note_user(&mut self, text: &str) -> Option<String> {
        if is_retry(text) { self.escalate("user asked to retry") } else { None }
    }

    pub fn note_context(&mut self, tokens: u64, limit: u64) -> Option<String> {
        if tokens > limit { self.escalate(&format!("context {} tokens over {}", tokens, limit)) } else { None }
    }

    /// The model and effort for the next turn, and the line for the footer.
    pub fn choose(&self, weak: &Value, main: &Value) -> (Value, String) {
        let (mut model, effort) = match &self.escalated {
            Some(_) => (main.clone(), ESCALATED_EFFORT),
            None => (weak.clone(), WEAK_EFFORT[self.step]),
        };
        model["effort"] = json!(effort);
        let name = model["model"].as_str().unwrap_or("?").to_string();
        let label = match &self.escalated {
            Some(why) => format!("auto · {} {} (escalated: {})", name, effort, why),
            None => format!("auto · {} {}", name, effort),
        };
        (model, label)
    }
}

/// "nochmal", "retry" or "try again" and nothing else.
pub fn is_retry(text: &str) -> bool {
    let t = text.trim().trim_end_matches(['.', '!', '?']).trim().to_lowercase();
    matches!(t.as_str(), "nochmal" | "retry" | "try again")
}

/// The identity of one chat for the cost mode: a name and a secret that signs its events.
#[derive(Clone, Debug)]
pub struct RouteId {
    pub id: String,
    key: u64,
}

static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

impl RouteId {
    pub fn new() -> RouteId {
        use std::hash::{BuildHasher, Hasher};
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        // RandomState carries random keys per process, so the secret cannot be computed from the visible name.
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u64(n);
        h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
        let key = h.finish();
        RouteId { id: format!("c{}-{}", std::process::id(), n), key }
    }

    /// The folder of this chat's events and the environment that gives them to an MCP server process.
    pub fn env(&self, ctx: &Ctx) -> Vec<(String, String)> {
        vec![("PENELOPE_ROUTE_DIR".to_string(), events_dir(ctx, &self.id).to_string_lossy().to_string()),
             ("PENELOPE_ROUTE_KEY".to_string(), format!("{:x}", self.key))]
    }
}

impl Default for RouteId {
    fn default() -> Self {
        RouteId::new()
    }
}

/// Penelope's own state directory, `~/.penelope/state`, outside every workspace. `PENELOPE_STATE_DIR` overrides it.
pub fn state_dir() -> std::path::PathBuf {
    match std::env::var("PENELOPE_STATE_DIR") {
        Ok(p) if !p.trim().is_empty() => std::path::PathBuf::from(p),
        _ => crate::platform::penelope_home().join("state"),
    }
}

/// True for a path inside the state directory. `write` and `edit` refuse it.
pub fn is_state_path(p: &std::path::Path) -> bool {
    p.starts_with(state_dir())
}

fn events_dir(ctx: &Ctx, id: &str) -> std::path::PathBuf {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    ctx.root.hash(&mut h);
    state_dir().join(format!("route-{:x}", h.finish())).join(id)
}

fn sign(key: u64, kind: &str, name: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (key, kind, name).hash(&mut h);
    format!("{:016x}", h.finish())
}

thread_local! {
    /// The chat whose turn runs on this thread: folder and key of its events.
    static CURRENT: std::cell::RefCell<Option<(std::path::PathBuf, u64)>> = const { std::cell::RefCell::new(None) };
}

/// Runs a turn so that tool calls on this thread write their events for `rid`.
pub fn with_chat<T>(ctx: &Ctx, rid: &RouteId, f: impl FnOnce() -> T) -> T {
    let before = CURRENT.with(|c| c.replace(Some((events_dir(ctx, &rid.id), rid.key))));
    let out = f();
    CURRENT.with(|c| *c.borrow_mut() = before);
    out
}

fn current() -> Option<(std::path::PathBuf, u64)> {
    if let Some(c) = CURRENT.with(|c| c.borrow().clone()) {
        return Some(c);
    }
    let dir = std::env::var("PENELOPE_ROUTE_DIR").ok().filter(|d| !d.is_empty())?;
    let key = u64::from_str_radix(std::env::var("PENELOPE_ROUTE_KEY").ok()?.trim(), 16).ok()?;
    Some((std::path::PathBuf::from(dir), key))
}

type Key = (String, String);

#[derive(Default)]
struct Registry {
    routers: HashMap<Key, Router>,
    /// The chat that started last on each root, for `/escalate` while a turn runs.
    latest: HashMap<String, String>,
}

static ROUTERS: Mutex<Option<Registry>> = Mutex::new(None);

fn root_key(ctx: &Ctx) -> String {
    ctx.root.to_string_lossy().to_string()
}

fn with<T>(ctx: &Ctx, id: Option<&str>, f: impl FnOnce(&mut Router) -> T) -> T {
    let mut all = ROUTERS.lock().unwrap_or_else(|e| e.into_inner());
    let reg = all.get_or_insert_with(Registry::default);
    let root = root_key(ctx);
    let id = id.map(String::from).or_else(|| reg.latest.get(&root).cloned()).unwrap_or_default();
    f(reg.routers.entry((root, id)).or_default())
}

fn remember(ctx: &Ctx, id: &str) {
    let mut all = ROUTERS.lock().unwrap_or_else(|e| e.into_inner());
    all.get_or_insert_with(Registry::default).latest.insert(root_key(ctx), id.to_string());
}

/// Where a setting sends its history: kind, provider and base URL. Weak and main must agree, or an escalation would
/// send the weak history to another host.
fn endpoint(m: &Value) -> String {
    let provider = crate::providers::of(m).map(|p| p.name).unwrap_or("");
    format!("{}|{}|{}", crate::llm::kind_of(m), provider, m["base_url"].as_str().unwrap_or("").trim().trim_end_matches('/').to_lowercase())
}

fn same_kind(ctx: &Ctx) -> bool {
    endpoint(&ctx.cfg["weak"]) == endpoint(&ctx.cfg["main"])
}

/// True when `auto` is set and can run.
pub fn active(ctx: &Ctx) -> bool {
    is_auto(ctx) && same_kind(ctx)
}

fn log_task(ctx: &Ctx) -> u64 {
    let mut id = 0;
    crate::cost::update(ctx, |v| {
        let r = &mut v["route"];
        id = r["tasks"].as_u64().unwrap_or(0) + 1;
        r["tasks"] = json!(id);
        if !r["log"].is_array() {
            r["log"] = json!([]);
        }
        let log = r["log"].as_array_mut().unwrap();
        log.push(json!({"task": id, "escalated": false, "reason": "", "at": crate::sessions::stamp(crate::cost::now())}));
        if log.len() > 500 {
            log.remove(0);
        }
    });
    id
}

fn log_escalation(ctx: &Ctx, task: u64, why: &str) {
    crate::cost::update(ctx, |v| {
        let r = &mut v["route"];
        r["escalated"] = json!(r["escalated"].as_u64().unwrap_or(0) + 1);
        r["reasons"][why] = json!(r["reasons"][why].as_u64().unwrap_or(0) + 1);
        if let Some(entry) = r["log"].as_array_mut().and_then(|l| l.iter_mut().find(|e| e["task"].as_u64() == Some(task))) {
            entry["escalated"] = json!(true);
            entry["reason"] = json!(why);
        }
    });
}

/// Writes the escalation of a task that has begun to the log, once. A task that has not begun writes it at its start.
fn write_log(ctx: &Ctx, id: &str) {
    let todo = with(ctx, Some(id), |r| {
        if !r.begun || r.logged || r.escalated.is_none() {
            return None;
        }
        r.logged = true;
        r.escalated.clone().map(|w| (r.log_id, w))
    });
    if let Some((task, why)) = todo {
        log_escalation(ctx, task, &why);
    }
}

/// Starts the task of a chat. Its own event folder is cleared; the folders of other chats are not touched.
pub fn start_task(ctx: &Ctx, rid: &RouteId) {
    if !is_auto(ctx) {
        return;
    }
    let task = log_task(ctx);
    with(ctx, Some(&rid.id), |r| {
        r.new_task();
        r.log_id = task;
    });
    remember(ctx, &rid.id);
    let _ = fs::remove_dir_all(events_dir(ctx, &rid.id));
    write_log(ctx, &rid.id);
}

/// `/escalate` for the chat `id`, or the latest chat of this root. Before the first turn it applies to the coming task.
pub fn escalate_now(ctx: &Ctx, id: Option<&str>) -> Result<String, String> {
    if !active(ctx) {
        return Err("route is fixed; /route auto turns the cost mode on".to_string());
    }
    let target = id.map(String::from).or_else(|| ROUTERS.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|r| r.latest.get(&root_key(ctx)).cloned())).unwrap_or_default();
    if with(ctx, Some(&target), |r| r.escalate("user /escalate")).is_none() {
        return Err("this task already runs on the main model".to_string());
    }
    remember(ctx, &target);
    write_log(ctx, &target);
    Ok(format!("escalated to {} for the rest of this task", ctx.cfg["main"]["model"].as_str().unwrap_or("main")))
}

/// The model setting for the next turn. In `fixed` it is the main setting unchanged. The text of the user may escalate.
pub fn pick(ctx: &Ctx, rid: &RouteId, user: &str) -> Value {
    if !active(ctx) {
        return ctx.cfg["main"].clone();
    }
    with(ctx, Some(&rid.id), |r| r.note_user(user));
    write_log(ctx, &rid.id);
    with(ctx, Some(&rid.id), |r| r.choose(&ctx.cfg["weak"], &ctx.cfg["main"])).0
}

/// Takes the events of one chat after a turn: each file is renamed first, so an event written meanwhile waits for the
/// next call and is never lost. Returns the reason when a trigger fired in this turn.
pub fn after_turn(ctx: &Ctx, rid: &RouteId, context_tokens: u64) -> Option<String> {
    if !active(ctx) {
        return None;
    }
    let dir = events_dir(ctx, &rid.id);
    let mut names: Vec<String> = fs::read_dir(&dir).map(|d| d.flatten().filter_map(|e| e.file_name().to_str().map(String::from)).collect()).unwrap_or_default();
    names.retain(|n| !n.ends_with(".taken"));
    names.sort();
    let mut fired = None;
    for name in names {
        let (from, to) = (dir.join(&name), dir.join(format!("{}.taken", name)));
        if fs::rename(&from, &to).is_err() {
            continue;
        }
        let text = fs::read_to_string(&to).unwrap_or_default();
        let _ = fs::remove_file(&to);
        let Some((kind, sig)) = text.trim().split_once(' ') else { continue };
        if sig != sign(rid.key, kind, &name) {
            continue;
        }
        let r = with(ctx, Some(&rid.id), |r| match kind {
            "test_fail" => r.note_test(false),
            "test_pass" => r.note_test(true),
            "strikes" => r.note_strikes(),
            _ => None,
        });
        if r.is_some() {
            write_log(ctx, &rid.id);
        }
        fired = fired.or(r);
    }
    let limit = limit(ctx);
    with(ctx, Some(&rid.id), |r| r.note_context(context_tokens, limit));
    write_log(ctx, &rid.id);
    fired
}

/// A tool process tells the chat what happened. Only written in `auto`, and only where the chat's folder is known.
fn event(ctx: &Ctx, kind: &str) {
    if !is_auto(ctx) {
        return;
    }
    let Some((dir, key)) = current() else { return };
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        let _ = std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir);
    }
    #[cfg(not(unix))]
    let _ = fs::create_dir_all(&dir);
    let seq = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let name = format!("{:032}-{:06}-{}", nanos, seq, std::process::id());
    if let Ok(mut f) = fs::OpenOptions::new().create_new(true).write(true).open(dir.join(&name)) {
        let _ = write!(f, "{} {}", kind, sign(key, kind, &name));
    }
}

pub fn note_test(ctx: &Ctx, passed: bool) {
    event(ctx, if passed { "test_pass" } else { "test_fail" });
}

pub fn note_strikes(ctx: &Ctx) {
    event(ctx, "strikes");
}

/// The footer text, `auto · haiku low`.
pub fn label(ctx: &Ctx, id: Option<&str>) -> Option<String> {
    if !is_auto(ctx) {
        return None;
    }
    if !same_kind(ctx) {
        return Some("auto off · weak and main use different hosts".to_string());
    }
    Some(with(ctx, id, |r| r.choose(&ctx.cfg["weak"], &ctx.cfg["main"])).1)
}

/// `/route` with no word, or `auto` / `fixed`. The caller saves the config after `Ok(true)`.
pub fn command(ctx: &mut Ctx, rest: &str, id: Option<&str>) -> Result<(String, bool), String> {
    match rest.trim() {
        "" => Ok((status(ctx, id), false)),
        w @ ("auto" | "fixed") => {
            ctx.cfg["route"] = json!(w);
            Ok((format!("route {}", w), true))
        }
        other => Err(format!("/route takes auto or fixed, not {}", other)),
    }
}

/// Mode, current model, effort and the escalation count of this session.
pub fn status(ctx: &Ctx, id: Option<&str>) -> String {
    if !is_auto(ctx) {
        return format!("route fixed · {} {}", ctx.cfg["main"]["model"].as_str().unwrap_or("?"), ctx.cfg["main"]["effort"].as_str().unwrap_or(""));
    }
    let (n, line) = with(ctx, id, |r| (r.escalations, r.choose(&ctx.cfg["weak"], &ctx.cfg["main"]).1));
    let note = if same_kind(ctx) { "" } else { " · inactive: weak and main use different hosts" };
    format!("route {} · escalations this session {} · context limit {} tokens{}", line, n, limit(ctx), note)
}

/// Tasks and escalations written to `usage.json` so far.
pub fn counters(ctx: &Ctx) -> (u64, u64) {
    let v: Value = fs::read_to_string(ctx.root.join("usage.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    (v["route"]["tasks"].as_u64().unwrap_or(0), v["route"]["escalated"].as_u64().unwrap_or(0))
}

/// The line for an eval setup: escalated tasks of all tasks. Empty when no task ran in `auto`.
pub fn rate_line(setup: &str, before: (u64, u64), after: (u64, u64)) -> String {
    let (tasks, esc) = (after.0.saturating_sub(before.0), after.1.saturating_sub(before.1));
    if tasks == 0 {
        return String::new();
    }
    format!("  {} · escalation rate: {} of {} tasks ({:.1} %)\n", setup, esc, tasks, esc as f64 * 100.0 / tasks as f64)
}

/// The context of one model step: input, cache creation and cache read tokens of its usage record.
pub fn step_context(usage: &Value) -> u64 {
    ["input_tokens", "cache_creation_input_tokens", "cache_read_input_tokens"].iter().map(|k| usage[*k].as_u64().unwrap_or(0)).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn models() -> (Value, Value) {
        (json!({"kind": "claude-cli", "model": "haiku", "effort": "high"}), json!({"kind": "claude-cli", "model": "sonnet", "effort": "high"}))
    }

    fn router() -> Router {
        let mut r = Router::default();
        r.new_task();
        r
    }

    #[test]
    fn a_task_starts_on_the_weak_model_with_effort_low() {
        let (w, m) = models();
        let (model, label) = router().choose(&w, &m);
        assert_eq!((model["model"].as_str(), model["effort"].as_str()), (Some("haiku"), Some("low")));
        assert_eq!(label, "auto · haiku low");
    }

    #[test]
    fn the_first_failed_test_raises_the_effort_and_the_second_escalates() {
        let (w, m) = models();
        let mut r = router();
        assert_eq!(r.note_test(false), None);
        assert_eq!(r.choose(&w, &m).0["effort"], "medium");
        assert_eq!(r.choose(&w, &m).0["model"], "haiku");
        assert_eq!(r.note_test(false).as_deref(), Some("tests failed 2x"));
        let (model, label) = r.choose(&w, &m);
        assert_eq!((model["model"].as_str(), model["effort"].as_str()), (Some("sonnet"), Some("high")));
        assert_eq!(label, "auto · sonnet high (escalated: tests failed 2x)");
        assert_eq!(r.escalations, 1);
    }

    #[test]
    fn a_pass_between_two_failures_does_not_escalate() {
        let mut r = router();
        r.note_test(false);
        r.note_test(true);
        assert_eq!(r.note_test(false), None);
        assert!(r.reason().is_none());
    }

    #[test]
    fn three_eurykleia_strikes_escalate() {
        let mut r = router();
        assert_eq!(r.note_strikes().as_deref(), Some("Eurykleia 3 strikes"));
        assert_eq!(r.reason(), Some("Eurykleia 3 strikes"));
    }

    #[test]
    fn the_user_can_ask_for_the_main_model() {
        for text in ["nochmal", " Retry. ", "try again!", "NOCHMAL"] {
            let mut r = router();
            assert!(r.note_user(text).is_some(), "{}", text);
        }
        for text in ["nochmal bitte mit Tests", "retry the build", "ok"] {
            let mut r = router();
            assert!(r.note_user(text).is_none(), "{}", text);
        }
    }

    #[test]
    fn a_context_over_the_limit_escalates() {
        let mut r = router();
        assert_eq!(r.note_context(100_000, 100_000), None);
        assert_eq!(r.note_context(100_001, 100_000).as_deref(), Some("context 100001 tokens over 100000"));
    }

    #[test]
    fn the_task_stays_escalated_and_counts_once_until_a_new_task() {
        let (w, m) = models();
        let mut r = router();
        assert!(r.note_strikes().is_some());
        assert_eq!(r.note_test(false), None);
        assert_eq!(r.note_test(false), None);
        assert_eq!(r.escalations, 1);
        assert_eq!(r.choose(&w, &m).0["model"], "sonnet");
        r.new_task();
        assert_eq!(r.choose(&w, &m).1, "auto · haiku low");
        assert_eq!((r.tasks, r.escalations), (2, 1));
    }

    fn ctx(name: &str, route: &str) -> Ctx {
        let root = std::env::temp_dir().join(format!("penelope-autoroute-{}-{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let mut c = crate::config::load(&root);
        c.cfg["route"] = json!(route);
        c.cfg["main"] = json!({"kind": "claude-cli", "model": "sonnet", "effort": "high"});
        c.cfg["weak"] = json!({"kind": "claude-cli", "model": "haiku", "effort": "high"});
        c
    }

    fn usage(c: &Ctx) -> Value {
        serde_json::from_str(&fs::read_to_string(c.root.join("usage.json")).unwrap()).unwrap()
    }

    /// One tool call of the chat: a test run or a strike, as the tool process writes it.
    fn tool(c: &Ctx, rid: &RouteId, f: impl FnOnce()) {
        with_chat(c, rid, f);
    }

    #[test]
    fn fixed_leaves_the_main_setting_unchanged() {
        let c = ctx("fixed", "fixed");
        let rid = RouteId::new();
        start_task(&c, &rid);
        tool(&c, &rid, || {
            note_test(&c, false);
            note_test(&c, false);
            note_strikes(&c);
        });
        assert_eq!(pick(&c, &rid, "retry"), c.cfg["main"]);
        assert_eq!(after_turn(&c, &rid, 900_000), None);
        assert!(label(&c, Some(&rid.id)).is_none());
        assert!(!c.root.join("usage.json").exists() && !events_dir(&c, &rid.id).exists());
        let _ = fs::remove_dir_all(&c.root);
    }

    #[test]
    fn events_from_a_tool_process_escalate_the_chat_and_reach_usage_json() {
        let c = ctx("events", "auto");
        let rid = RouteId::new();
        start_task(&c, &rid);
        assert_eq!(pick(&c, &rid, "fix the bug")["model"], "haiku");
        tool(&c, &rid, || {
            note_test(&c, false);
            note_test(&c, false);
        });
        assert_eq!(after_turn(&c, &rid, 5000).as_deref(), Some("tests failed 2x"));
        assert_eq!(pick(&c, &rid, "go on")["model"], "sonnet");
        assert!(label(&c, Some(&rid.id)).unwrap().contains("escalated: tests failed 2x"));
        let next = RouteId::new();
        start_task(&c, &next);
        assert_eq!(pick(&c, &next, "next")["model"], "haiku");
        let v = usage(&c);
        assert_eq!((v["route"]["tasks"].as_u64(), v["route"]["escalated"].as_u64()), (Some(2), Some(1)));
        assert_eq!(v["route"]["reasons"]["tests failed 2x"], 1);
        assert_eq!(v["route"]["log"][0]["escalated"], true);
        assert_eq!(v["route"]["log"][1]["escalated"], false);
        assert_eq!(counters(&c), (2, 1));
        assert!(rate_line("large", (0, 0), counters(&c)).contains("1 of 2 tasks (50.0 %)"));
        let _ = fs::remove_dir_all(&c.root);
    }

    #[test]
    fn escalate_command_context_and_strikes_reach_the_chat() {
        let c = ctx("cmd", "auto");
        let a = RouteId::new();
        start_task(&c, &a);
        assert!(escalate_now(&c, Some(&a.id)).is_ok());
        assert!(escalate_now(&c, Some(&a.id)).unwrap_err().contains("already"));
        let b = RouteId::new();
        start_task(&c, &b);
        assert_eq!(after_turn(&c, &b, 100_001), None);
        assert!(label(&c, Some(&b.id)).unwrap().contains("escalated: context"));
        let d = RouteId::new();
        start_task(&c, &d);
        tool(&c, &d, || note_strikes(&c));
        assert_eq!(after_turn(&c, &d, 10).as_deref(), Some("Eurykleia 3 strikes"));
        let _ = fs::remove_dir_all(&c.root);
    }

    #[test]
    fn an_escalation_before_the_first_turn_applies_to_the_coming_task_once() {
        let c = ctx("before", "auto");
        let old = RouteId::new();
        start_task(&c, &old);
        let fresh = RouteId::new();
        assert!(escalate_now(&c, Some(&fresh.id)).is_ok());
        assert_eq!(counters(&c), (1, 0), "nothing is logged before the task starts");
        assert!(label(&c, Some(&fresh.id)).unwrap().contains("escalated: user /escalate"));
        start_task(&c, &fresh);
        assert_eq!(pick(&c, &fresh, "go")["model"], "sonnet");
        let v = usage(&c);
        assert_eq!(counters(&c), (2, 1));
        assert_eq!(v["route"]["log"][0]["escalated"], false, "the earlier task is not marked");
        assert_eq!(v["route"]["log"][1]["escalated"], true);
        assert_eq!(v["route"]["log"][1]["reason"], "user /escalate");
        assert!(status(&c, Some(&fresh.id)).contains("escalations this session 1"));
        let _ = fs::remove_dir_all(&c.root);
    }

    #[test]
    fn two_chats_on_one_root_keep_their_own_events_and_state() {
        let c = ctx("two", "auto");
        let (a, b) = (RouteId::new(), RouteId::new());
        start_task(&c, &a);
        start_task(&c, &b);
        tool(&c, &a, || {
            note_test(&c, false);
            note_test(&c, false);
        });
        tool(&c, &b, || note_test(&c, false));
        start_task(&c, &RouteId::new());          // a third chat starting does not delete the events of a and b
        assert_eq!(after_turn(&c, &b, 1), None);
        assert_eq!(after_turn(&c, &a, 1).as_deref(), Some("tests failed 2x"));
        assert!(label(&c, Some(&a.id)).unwrap().contains("escalated"));
        assert_eq!(label(&c, Some(&b.id)).unwrap(), "auto · haiku medium");
        let _ = fs::remove_dir_all(&c.root);
    }

    #[test]
    fn an_event_written_between_two_reads_is_not_lost() {
        let c = ctx("atomic", "auto");
        let a = RouteId::new();
        start_task(&c, &a);
        tool(&c, &a, || note_test(&c, false));
        assert_eq!(after_turn(&c, &a, 1), None);
        tool(&c, &a, || note_test(&c, false));
        assert_eq!(after_turn(&c, &a, 1).as_deref(), Some("tests failed 2x"), "one failure per turn still counts to two");
        assert_eq!(fs::read_dir(events_dir(&c, &a.id)).unwrap().count(), 0);
        let _ = fs::remove_dir_all(&c.root);
    }

    #[test]
    fn a_forged_or_unsigned_event_is_ignored_and_the_state_dir_is_closed_to_write() {
        let c = ctx("forged", "auto");
        let a = RouteId::new();
        start_task(&c, &a);
        let dir = events_dir(&c, &a.id);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("0001"), "strikes").unwrap();
        fs::write(dir.join("0002"), "strikes 0000000000000000").unwrap();
        fs::write(dir.join("0003"), format!("strikes {}", sign(a.key ^ 1, "strikes", "0003"))).unwrap();
        assert_eq!(after_turn(&c, &a, 1), None);
        assert!(label(&c, Some(&a.id)).unwrap().ends_with("haiku low"));
        // an event of another chat's key does not help either
        let b = RouteId::new();
        tool(&c, &b, || note_strikes(&c));
        assert_eq!(after_turn(&c, &a, 1), None);
        assert!(!dir.starts_with(&c.root) && is_state_path(&dir));
        let out = crate::files::write(&c, &dir.join("0004").to_string_lossy(), "strikes x", "");
        assert!(out.starts_with("NOT written") && !dir.join("0004").exists(), "{}", out);
        let _ = fs::remove_dir_all(&c.root);
    }

    #[test]
    fn auto_stays_off_when_weak_and_main_use_different_hosts() {
        let mut c = ctx("hosts", "auto");
        c.cfg["weak"] = json!({"kind": "openai", "base_url": "https://a.example/v1", "model": "w"});
        c.cfg["main"] = json!({"kind": "openai", "base_url": "https://b.example/v1", "model": "m"});
        assert!(!active(&c));
        assert!(label(&c, None).unwrap().contains("different hosts"));
        assert_eq!(pick(&c, &RouteId::new(), "x"), c.cfg["main"]);
        c.cfg["main"]["base_url"] = json!("https://A.example/v1/");
        assert!(active(&c), "same host, other spelling and model");
        c.cfg["weak"] = json!({"kind": "anthropic", "model": "w"});
        c.cfg["main"] = json!({"kind": "anthropic", "base_url": "https://proxy.example", "model": "m"});
        assert!(!active(&c));
        let _ = fs::remove_dir_all(&c.root);
    }

    #[test]
    fn the_context_of_a_step_is_input_plus_cache() {
        let u = json!({"input_tokens": 10, "cache_creation_input_tokens": 5, "cache_read_input_tokens": 15_000});
        assert_eq!(step_context(&u), 15_015);
        assert_eq!(step_context(&json!({})), 0);
        // 8 steps of 15k each: the sum is 120k, the context is 15k
        let steps: Vec<u64> = (0..8).map(|_| step_context(&u)).collect();
        let mut r = Router::default();
        r.new_task();
        assert_eq!(steps.iter().sum::<u64>(), 120_120);
        assert_eq!(r.note_context(*steps.iter().max().unwrap(), DEFAULT_LIMIT), None);
    }

    #[test]
    fn a_pre_escalated_router_keeps_its_reason_through_new_task() {
        let mut r = Router::default();
        assert!(r.escalate("user /escalate").is_some());
        r.new_task();
        assert_eq!((r.reason(), r.escalations, r.tasks), (Some("user /escalate"), 1, 1));
        r.new_task();
        assert_eq!((r.reason(), r.escalations, r.tasks), (None, 1, 2));
    }

    #[test]
    fn route_command_sets_the_mode_and_status_names_the_count() {
        let mut c = ctx("status", "fixed");
        assert!(status(&c, None).starts_with("route fixed · sonnet"));
        assert_eq!(command(&mut c, "auto", None).unwrap(), ("route auto".to_string(), true));
        let a = RouteId::new();
        start_task(&c, &a);
        assert!(status(&c, Some(&a.id)).contains("auto · haiku low") && status(&c, Some(&a.id)).contains("escalations this session 0"));
        assert!(command(&mut c, "sometimes", None).is_err());
        let _ = fs::remove_dir_all(&c.root);
    }
}
