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
//! through the file `route-events` in the root. The chat reads it after each turn. A change applies from the next
//! turn; when the weak turn itself triggered it, `Chat::turn_with` runs one follow-up turn on the main model.
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
}

impl Router {
    pub fn new_task(&mut self) {
        self.step = 0;
        self.fails = 0;
        self.escalated = None;
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

static ROUTERS: Mutex<Option<HashMap<String, Router>>> = Mutex::new(None);

fn with<T>(ctx: &Ctx, f: impl FnOnce(&mut Router) -> T) -> T {
    let mut all = ROUTERS.lock().unwrap_or_else(|e| e.into_inner());
    let r = all.get_or_insert_with(HashMap::new).entry(ctx.root.to_string_lossy().to_string()).or_default();
    f(r)
}

/// The kinds of the weak and the main setting must match, or a session could not change its model.
fn same_kind(ctx: &Ctx) -> bool {
    crate::llm::kind_of(&ctx.cfg["weak"]) == crate::llm::kind_of(&ctx.cfg["main"])
}

/// True when `auto` is set and can run.
pub fn active(ctx: &Ctx) -> bool {
    is_auto(ctx) && same_kind(ctx)
}

fn log_task(ctx: &Ctx, n: u32) {
    crate::cost::update(ctx, |v| {
        let r = &mut v["route"];
        r["tasks"] = json!(r["tasks"].as_u64().unwrap_or(0) + 1);
        if !r["log"].is_array() {
            r["log"] = json!([]);
        }
        let log = r["log"].as_array_mut().unwrap();
        log.push(json!({"task": n, "escalated": false, "reason": "", "at": crate::sessions::stamp(crate::cost::now())}));
        if log.len() > 500 {
            log.remove(0);
        }
    });
}

fn log_escalation(ctx: &Ctx, why: &str) {
    crate::cost::update(ctx, |v| {
        let r = &mut v["route"];
        r["escalated"] = json!(r["escalated"].as_u64().unwrap_or(0) + 1);
        r["reasons"][why] = json!(r["reasons"][why].as_u64().unwrap_or(0) + 1);
        if let Some(last) = r["log"].as_array_mut().and_then(|l| l.last_mut()) {
            last["escalated"] = json!(true);
            last["reason"] = json!(why);
        }
    });
}

/// Starts a new task for this root. Called at the first turn of a chat.
pub fn start_task(ctx: &Ctx) {
    if !is_auto(ctx) {
        return;
    }
    let n = with(ctx, |r| {
        r.new_task();
        r.tasks
    });
    log_task(ctx, n);
    let _ = fs::remove_file(ctx.root.join("route-events"));
}

/// Applies an escalation to the log. `Some(reason)` when this call escalated.
fn escalated(ctx: &Ctx, why: Option<String>) -> Option<String> {
    if let Some(w) = &why {
        log_escalation(ctx, w);
    }
    why
}

/// `/escalate`. `Err` when the mode is not `auto` or the task already runs on the main model.
pub fn escalate_now(ctx: &Ctx) -> Result<String, String> {
    if !active(ctx) {
        return Err("route is fixed; /route auto turns the cost mode on".to_string());
    }
    match escalated(ctx, with(ctx, |r| r.escalate("user /escalate"))) {
        Some(_) => Ok(format!("escalated to {} for the rest of this task", ctx.cfg["main"]["model"].as_str().unwrap_or("main"))),
        None => Err("this task already runs on the main model".to_string()),
    }
}

/// The model setting for the next turn. In `fixed` it is the main setting unchanged. The text of the user may escalate.
pub fn pick(ctx: &Ctx, user: &str) -> Value {
    if !active(ctx) {
        return ctx.cfg["main"].clone();
    }
    escalated(ctx, with(ctx, |r| r.note_user(user)));
    with(ctx, |r| r.choose(&ctx.cfg["weak"], &ctx.cfg["main"])).0
}

/// What happened during one turn: read from the events file after the turn.
/// Returns the reason when a trigger fired in this turn, so the caller can run a follow-up on the main model.
pub fn after_turn(ctx: &Ctx, context_tokens: u64) -> Option<String> {
    if !active(ctx) {
        return None;
    }
    let events = fs::read_to_string(ctx.root.join("route-events")).unwrap_or_default();
    let _ = fs::remove_file(ctx.root.join("route-events"));
    let mut fired = None;
    for line in events.lines() {
        let r = with(ctx, |r| match line.trim() {
            "test_fail" => r.note_test(false),
            "test_pass" => r.note_test(true),
            "strikes" => r.note_strikes(),
            _ => None,
        });
        fired = fired.or(escalated(ctx, r));
    }
    let limit = limit(ctx);
    escalated(ctx, with(ctx, |r| r.note_context(context_tokens, limit)));
    fired
}

/// A tool process tells the chat what happened. Only written in `auto`.
fn event(ctx: &Ctx, name: &str) {
    if !is_auto(ctx) || !ctx.root.is_dir() {
        return;
    }
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(ctx.root.join("route-events")) {
        let _ = writeln!(f, "{}", name);
    }
}

pub fn note_test(ctx: &Ctx, passed: bool) {
    event(ctx, if passed { "test_pass" } else { "test_fail" });
}

pub fn note_strikes(ctx: &Ctx) {
    event(ctx, "strikes");
}

/// The footer text, `auto · haiku low`.
pub fn label(ctx: &Ctx) -> Option<String> {
    if !is_auto(ctx) {
        return None;
    }
    if !same_kind(ctx) {
        return Some("auto off · weak and main differ in kind".to_string());
    }
    Some(with(ctx, |r| r.choose(&ctx.cfg["weak"], &ctx.cfg["main"])).1)
}

/// `/route` with no word, or `auto` / `fixed`. The caller saves the config after `Ok(true)`.
pub fn command(ctx: &mut Ctx, rest: &str) -> Result<(String, bool), String> {
    match rest.trim() {
        "" => Ok((status(ctx), false)),
        w @ ("auto" | "fixed") => {
            ctx.cfg["route"] = json!(w);
            Ok((format!("route {}", w), true))
        }
        other => Err(format!("/route takes auto or fixed, not {}", other)),
    }
}

/// Mode, current model, effort and the escalation count of this session.
pub fn status(ctx: &Ctx) -> String {
    if !is_auto(ctx) {
        return format!("route fixed · {} {}", ctx.cfg["main"]["model"].as_str().unwrap_or("?"), ctx.cfg["main"]["effort"].as_str().unwrap_or(""));
    }
    let (n, line) = with(ctx, |r| (r.escalations, r.choose(&ctx.cfg["weak"], &ctx.cfg["main"]).1));
    let note = if same_kind(ctx) { "" } else { " · inactive: weak and main use different kinds" };
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

    #[test]
    fn fixed_leaves_the_main_setting_unchanged() {
        let c = ctx("fixed", "fixed");
        start_task(&c);
        note_test(&c, false);
        note_test(&c, false);
        note_strikes(&c);
        assert_eq!(pick(&c, "retry"), c.cfg["main"]);
        assert_eq!(after_turn(&c, 900_000), None);
        assert!(label(&c).is_none());
        assert!(!c.root.join("route-events").exists() && !c.root.join("usage.json").exists());
        let _ = fs::remove_dir_all(&c.root);
    }

    #[test]
    fn events_from_a_tool_process_escalate_the_chat_and_reach_usage_json() {
        let c = ctx("events", "auto");
        start_task(&c);
        assert_eq!(pick(&c, "fix the bug")["model"], "haiku");
        note_test(&c, false);
        note_test(&c, false);
        assert_eq!(after_turn(&c, 5000).as_deref(), Some("tests failed 2x"));
        assert_eq!(pick(&c, "go on")["model"], "sonnet");
        assert!(label(&c).unwrap().contains("escalated: tests failed 2x"));
        start_task(&c);
        assert_eq!(pick(&c, "next")["model"], "haiku");
        let v: Value = serde_json::from_str(&fs::read_to_string(c.root.join("usage.json")).unwrap()).unwrap();
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
        start_task(&c);
        assert!(escalate_now(&c).is_ok());
        assert!(escalate_now(&c).unwrap_err().contains("already"));
        start_task(&c);
        assert_eq!(after_turn(&c, 100_001), None);
        assert!(label(&c).unwrap().contains("escalated: context"));
        start_task(&c);
        note_strikes(&c);
        assert_eq!(after_turn(&c, 10).as_deref(), Some("Eurykleia 3 strikes"));
        let _ = fs::remove_dir_all(&c.root);
    }

    #[test]
    fn route_command_sets_the_mode_and_status_names_the_count() {
        let mut c = ctx("status", "fixed");
        assert!(status(&c).starts_with("route fixed · sonnet"));
        assert_eq!(command(&mut c, "auto").unwrap(), ("route auto".to_string(), true));
        start_task(&c);
        assert!(status(&c).contains("auto · haiku low") && status(&c).contains("escalations this session 0"));
        assert!(command(&mut c, "sometimes").is_err());
        let _ = fs::remove_dir_all(&c.root);
    }
}
