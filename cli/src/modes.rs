//! The four work modes of Penelope: chat, work, agents and code.
//!
//! A work mode is a profile for one chat: a tool set, a part of the system prompt, a default model, an effort and a
//! cost route. It is not the policy preset of `mode.rs` (auto, safe, power), which sets the gates for commands and file
//! changes. Both apply: a work mode can only narrow the tools, never widen what the policy allows.
//!
//! The mode reaches a turn as a copy of the settings (`effective`): `main` and `route` are replaced, and the keys
//! `_mode` and `_mode_depth` tell `mcp.rs`, `route.rs` and `backend.rs` which tools the lead may see and call. The MCP
//! server of a `claude` turn gets the same values through `PENELOPE_MODE` and `PENELOPE_HANDOFF_DEPTH`.
//!
//! `hand_off` lets one mode give a task to another. It runs a sub-session in the target mode on a thread of its own
//! (so the gates of the caller stay as they are), stores the full result as a Mitos attachment and returns one summary
//! line with the attachment card. The link between both sessions is kept in `handoffs.json`.

use crate::config::Ctx;
use serde_json::{json, Value};
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

pub const NAMES: &[&str] = &["chat", "work", "agents", "code"];

/// A hand-off may start at most this many nested sub-sessions. The sub-session at the limit has no hand_off.
pub const MAX_DEPTH: u32 = 2;

/// Characters of `context` that a hand-off carries.
const CONTEXT_MAX: usize = 1500;

pub fn valid(name: &str) -> bool {
    NAMES.contains(&name)
}

pub fn what(name: &str) -> &'static str {
    match name {
        "chat" => "plain conversation; only memory_find and ask_user; the cheapest model",
        "work" => "documents, files, web search, deep research and attachments; commands pass the gates",
        "agents" => "clusters, factories and workflows; Penelope asks about the limits before it creates anything",
        _ => "a repository: edits, tests, the checker and the cost mode",
    }
}

const CHAT_TOOLS: &[&str] = &["memory_find", "ask_user"];

const WORK_TOOLS: &[&str] = &[
    "read", "write", "edit", "unzip", "undo", "find", "grep", "tree", "outline", "json_query", "attachment_read",
    "web_search", "fetch", "research", "deep_research", "http_request", "scout", "scouts", "tool_search", "session_search",
    "run", "run_detail", "plan", "ask_user", "skill", "clipboard", "mcp_servers", "mcp_call", "brief_check", "brief_put",
    "memory_go", "memory_note", "memory_forget", "memory_find", "memory_map", "hand_off",
];

const AGENTS_TOOLS: &[&str] = &[
    "cluster_start", "cluster_spawn", "cluster_ask", "cluster_status", "cluster_kill", "cluster_dump", "cluster_respawn", "cluster_profile",
    "agent", "scout", "scouts", "read", "find", "grep", "tree", "outline", "json_query", "attachment_read", "session_search", "tool_search",
    "run_detail", "plan", "ask_user", "mcp_servers", "memory_go", "memory_note", "memory_forget", "memory_find", "memory_map", "hand_off",
];

fn default_tools(name: &str) -> Vec<String> {
    let list: &[&str] = match name {
        "chat" => CHAT_TOOLS,
        "work" => WORK_TOOLS,
        "agents" => AGENTS_TOOLS,
        _ => &["*"],
    };
    list.iter().map(|s| s.to_string()).collect()
}

/// The mode a new chat starts in: `modes.default`, else code.
pub fn default_mode(ctx: &Ctx) -> String {
    let name = ctx.cfg["modes"]["default"].as_str().unwrap_or("code");
    if valid(name) { name.to_string() } else { "code".to_string() }
}

/// The mode of this copy of the settings, `None` for a chat without a work mode.
pub fn current(ctx: &Ctx) -> Option<String> {
    ctx.cfg["_mode"].as_str().filter(|m| valid(m)).map(String::from)
}

pub fn depth(ctx: &Ctx) -> u32 {
    ctx.cfg["_mode_depth"].as_u64().unwrap_or(0) as u32
}

/// The entries of `modes.<name>.tools`, or the default list. An entry is a tool name, `*` for every tool, or `-name`
/// to take one away: `["*", "-computer"]`.
fn entries(ctx: &Ctx, name: &str) -> Vec<String> {
    match ctx.cfg["modes"][name]["tools"].as_array() {
        Some(list) if !list.is_empty() => list.iter().filter_map(|v| v.as_str().map(|s| s.trim().to_string())).filter(|s| !s.is_empty()).collect(),
        _ => default_tools(name),
    }
}

/// True when the tool list of the mode names every tool, so a tool Penelope wrote for this project is offered too.
fn open_ended(ctx: &Ctx, name: &str) -> bool {
    entries(ctx, name).iter().any(|e| e == "*")
}

/// The built-in tools of a mode, in the order of the tool table.
pub fn tool_list(ctx: &Ctx, name: &str) -> Vec<&'static str> {
    let mut set: Vec<&'static str> = Vec::new();
    for e in entries(ctx, name) {
        if e == "*" {
            set = crate::spec::names();
        } else if let Some(gone) = e.strip_prefix('-') {
            set.retain(|t| *t != gone.trim());
        } else if let Some(t) = crate::spec::names().into_iter().find(|t| *t == e.trim_start_matches('+').trim()) {
            if !set.contains(&t) { set.push(t); }
        }
    }
    crate::spec::names().into_iter().filter(|t| set.contains(t)).collect()
}

/// True when the lead of this copy of the settings may see and call the tool. A copy without a mode allows every tool.
pub fn allows(ctx: &Ctx, tool: &str) -> bool {
    let Some(mode) = current(ctx) else { return true };
    if crate::spec::get(tool).is_some() {
        return tool_list(ctx, &mode).contains(&tool);
    }
    open_ended(ctx, &mode)          // a tool Penelope wrote for this project
}

/// The text of a refusal for a tool the mode does not have.
pub fn refusal(ctx: &Ctx, tool: &str) -> String {
    let mode = current(ctx).unwrap_or_default();
    format!("NOT called. Mode {} has no tool {}. Its tools: {}. /mode NAME switches the mode; hand_off gives the task to another one.",
            mode, tool, tool_list(ctx, &mode).join(", "))
}

/// The model setting of a mode: `main` or an empty string keeps the main model, `weak` takes the weaker model, any
/// other word is a model name on the main provider, and an object is a whole setting.
fn model_for(ctx: &Ctx, name: &str, base: &Value) -> Value {
    match &ctx.cfg["modes"][name]["model"] {
        Value::String(s) if s.trim() == "weak" => ctx.cfg["weak"].clone(),
        Value::String(s) if s.trim().is_empty() || s.trim() == "main" => base.clone(),
        Value::String(s) => {
            let mut m = base.clone();
            m["model"] = json!(s.trim());
            m
        }
        Value::Object(_) => ctx.cfg["modes"][name]["model"].clone(),
        _ => base.clone(),
    }
}

/// The settings a chat in this mode runs on. The first call keeps the person's `main` and `route` under `_base_main`
/// and `_base_route`, so a copy that is made into another mode (a hand-off) starts from the same base.
pub fn effective(ctx: &Ctx, name: &str, depth: u32) -> Ctx {
    let mut out = ctx.clone();
    if !valid(name) {
        return out;
    }
    let base_main = if ctx.cfg["_base_main"].is_null() { ctx.cfg["main"].clone() } else { ctx.cfg["_base_main"].clone() };
    let base_route = if ctx.cfg["_base_route"].is_null() { ctx.cfg["route"].clone() } else { ctx.cfg["_base_route"].clone() };
    let mut main = model_for(ctx, name, &base_main);
    if let Some(effort) = ctx.cfg["modes"][name]["effort"].as_str().map(str::trim).filter(|e| !e.is_empty()) {
        main["effort"] = json!(effort);
    }
    out.cfg["_base_main"] = base_main;
    out.cfg["_base_route"] = base_route.clone();
    out.cfg["main"] = main;
    out.cfg["route"] = match ctx.cfg["modes"][name]["route"].as_str().map(str::trim).filter(|r| !r.is_empty()) {
        Some(route) => json!(route),
        None => base_route,
    };
    out.cfg["_mode"] = json!(name);
    out.cfg["_mode_depth"] = json!(depth);
    out
}

/// The settings of a mode from the environment of an MCP server process that a `claude` turn started.
pub fn from_env(ctx: &Ctx) -> Ctx {
    let Ok(name) = std::env::var("PENELOPE_MODE") else { return ctx.clone() };
    let depth = std::env::var("PENELOPE_HANDOFF_DEPTH").ok().and_then(|d| d.parse().ok()).unwrap_or(0);
    let mut out = effective(ctx, name.trim(), depth);
    if let Ok(key) = std::env::var("PENELOPE_CALLER") {
        out.cfg["_caller"] = json!(key);
    }
    if let Ok(session) = std::env::var("PENELOPE_CALLER_SESSION") {
        out.cfg["_caller_session"] = json!(session);
    }
    out
}

/// The variables a `claude` turn passes to its MCP server so the server applies the same mode.
pub fn env_for(ctx: &Ctx) -> Vec<(&'static str, String)> {
    let Some(mode) = current(ctx) else { return Vec::new() };
    let mut out = vec![("PENELOPE_MODE", mode), ("PENELOPE_HANDOFF_DEPTH", depth(ctx).to_string())];
    if let Some(key) = ctx.cfg["_caller"].as_str() {
        out.push(("PENELOPE_CALLER", key.to_string()));
    }
    if let Some(session) = ctx.cfg["_caller_session"].as_str() {
        out.push(("PENELOPE_CALLER_SESSION", session.to_string()));
    }
    out
}

/// What a mode adds to the system prompt.
fn part(name: &str) -> &'static str {
    match name {
        "work" => "Mode work: you handle tasks with documents, files, the web, deep research and attachments. Read before you change a file. \
Use research for a question and deep_research for a report with citations. Keep long results as attachments and read ranges of them. \
You do not edit a repository on your own: for code changes, tests or a pull request give the task to mode code with hand_off, and for a larger job to mode agents.",
        "agents" => "Mode agents: you run clusters, factories and workflows. Before you create one, state the goal, the done criterion, the abort rule, \
the budget and the rights; Penelope asks the person for every field that is still open and for a final yes. Report with the cluster ids. \
For a change inside one repository without a plan give the task to mode code with hand_off.",
        "code" => "Mode code: you work in a repository. Read before you change a file, make small edits, run the tests after a change and hand the claim \
and the command output to check. Cost mode routing may run a turn on the weaker model. For a research report or documents give the task to mode work with hand_off, \
for a long multi-part job to mode agents.",
        _ => "",
    }
}

const HAND_OFF_RULES: &str = " hand_off(to_mode, task, context) opens a sub-session in another mode and returns one summary line and an attachment card. \
The other mode does not see this conversation, so write the task with its done criterion and put what it must know in context. At most two hand-offs deep.";

/// The system prompt of a turn: the rules of the harness with the part of the mode, or the short prompt of chat.
pub fn compose(ctx: &Ctx, rules: String) -> String {
    let Some(mode) = current(ctx) else { return rules };
    if mode == "chat" {
        let today = &crate::sessions::stamp(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0))[..10];
        let mut text = format!("You work inside Penelope in mode chat: a plain conversation. Answer from what you know and answer briefly. \
Your only tools are memory_find, to look up what the person asked Penelope to keep, and ask_user, when two readings of a request lead to different answers. \
You cannot read files, search the web or run commands. When a request needs them, say so in one sentence and tell the person to type /send work, /send agents or /send code with the task. Today is {} (UTC).", today);
        if tool_list(ctx, "chat").contains(&"hand_off") {
            text.push_str(HAND_OFF_RULES);
        }
        return text;
    }
    let mut text = format!("{}\n\n{}", rules, part(&mode));
    if tool_list(ctx, &mode).contains(&"hand_off") {
        text.push_str(HAND_OFF_RULES);
    }
    if depth(ctx) > 0 {
        text.push_str(&format!(" This is a hand-off sub-session at depth {} of at most {}. Finish the task you were given and answer with the result; do not hand it back.", depth(ctx), MAX_DEPTH));
    }
    text
}

/// The footer text: the mode with its label.
pub fn footer(mode: &str) -> String {
    (if valid(mode) { mode } else { "code" }).to_string()
}

/// `/mode` and `penelope mode work`: the four work modes with their model, effort, route and tool count.
pub fn report(ctx: &Ctx, active: &str, session: &str) -> String {
    let mut out = format!("work mode: {}\n\n", active);
    for name in NAMES {
        let eff = effective(ctx, name, 0);
        let model = eff.cfg["main"]["model"].as_str().unwrap_or("?");
        let effort = eff.cfg["main"]["effort"].as_str().filter(|e| !e.is_empty()).unwrap_or("default");
        out.push_str(&format!("{} {:<7} {}\n", if *name == active { "●" } else { "○" }, name, what(name)));
        out.push_str(&format!("          model {} · effort {} · route {} · {} tools\n", model, effort,
                              eff.cfg["route"].as_str().unwrap_or("fixed"), tool_list(ctx, name).len()));
    }
    out.push_str("\n/mode NAME switches in the interface, Shift+Tab cycles, penelope ask --mode NAME starts in one.\n\
/send MODE TASK hands a task to another mode, /back returns. Settings: modes.NAME.model, effort, tools and route in config.json.\n");
    let linked = links_of(ctx, session);
    if !linked.is_empty() {
        out.push_str("\nhand-offs of this session\n");
        for l in linked.iter().rev().take(8) {
            out.push_str(&format!("  {} → {}  {}  (from {} to {})\n", l["from_mode"].as_str().unwrap_or("?"), l["to_mode"].as_str().unwrap_or("?"), l["task"].as_str().unwrap_or(""),
                                  l["from_session"].as_str().unwrap_or("this chat"), l["to_session"].as_str().unwrap_or("?")));
        }
    }
    out
}

// ---------- the person's own hand-off summary ----------

/// A short context for a hand-off that the person starts: the last two exchanges of the transcript, cut.
pub fn summarize(exchanges: &[(String, String)]) -> String {
    let cut = |t: &str, n: usize| -> String {
        let flat = t.split_whitespace().collect::<Vec<_>>().join(" ");
        if flat.chars().count() <= n { flat } else { format!("{}…", flat.chars().take(n.saturating_sub(1)).collect::<String>()) }
    };
    let from = exchanges.len().saturating_sub(2);
    exchanges[from..].iter().map(|(u, r)| format!("The person asked: {}\nThe answer was: {}", cut(u, 300), cut(r, 400))).collect::<Vec<_>>().join("\n")
}

// ---------- hand-off ----------

thread_local! {
    /// The stop flag of the turn on this thread. A hand-off watches it, so the person's stop reaches the sub-session.
    static CALLER_CANCEL: RefCell<Option<Arc<AtomicBool>>> = const { RefCell::new(None) };
}

/// Runs a turn with the stop flag of its chat visible to `hand_off` on this thread.
pub fn with_cancel<T>(cancel: &Arc<AtomicBool>, f: impl FnOnce() -> T) -> T {
    let before = CALLER_CANCEL.with(|c| c.replace(Some(cancel.clone())));
    let out = f();
    CALLER_CANCEL.with(|c| *c.borrow_mut() = before);
    out
}

/// One hand-off: who gives, who takes, what, and what the taker knows.
#[derive(Clone, Debug)]
pub struct Request {
    pub from: String,
    pub to: String,
    pub task: String,
    pub context: String,
}

/// What a hand-off returns: the full text, the line for the caller and the card of the stored attachment.
#[derive(Clone, Debug)]
pub struct Outcome {
    pub text: String,
    pub summary: String,
    pub card: String,
    pub session: String,
}

impl Outcome {
    /// The tool result: one line and the attachment card.
    pub fn result(&self) -> String {
        format!("{}\n{}", self.summary, self.card)
    }
}

fn one_line(text: &str, n: usize) -> String {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    let flat = line.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= n { flat } else { format!("{}…", flat.chars().take(n.saturating_sub(1)).collect::<String>()) }
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn links_path(ctx: &Ctx) -> std::path::PathBuf {
    ctx.root.join("handoffs.json")
}

/// Every hand-off of this folder, oldest first.
pub fn links(ctx: &Ctx) -> Vec<Value> {
    std::fs::read_to_string(links_path(ctx)).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn save_links(ctx: &Ctx, all: &[Value]) {
    let from = all.len().saturating_sub(500);
    let _ = std::fs::write(links_path(ctx), serde_json::to_string_pretty(&all[from..]).unwrap_or_default());
}

/// The hand-offs that start or end in a session. The link is found from both sides.
pub fn links_of(ctx: &Ctx, session: &str) -> Vec<Value> {
    links(ctx).into_iter().filter(|l| !session.is_empty() && (l["from_session"].as_str() == Some(session) || l["to_session"].as_str() == Some(session))).collect()
}

/// Tells the links of a chat which session it became. The first turn of a `claude` chat has no session id while a
/// hand-off runs, so the link carries the key of the chat until the turn is over.
pub fn bind_caller(ctx: &Ctx, key: &str, session: &str) {
    if key.is_empty() || session.is_empty() {
        return;
    }
    let mut all = links(ctx);
    let mut changed = false;
    for l in all.iter_mut().filter(|l| l["caller"].as_str() == Some(key) && l["from_session"].is_null()) {
        l["from_session"] = json!(session);
        changed = true;
    }
    if changed {
        save_links(ctx, &all);
    }
}

fn add_link(ctx: &Ctx, entry: Value) {
    let mut all = links(ctx);
    all.push(entry);
    save_links(ctx, &all);
}

fn prompt_for(req: &Request, depth: u32) -> String {
    let mut text = format!("This task was handed to you from mode {} (hand-off, depth {}). You do not see that conversation.\n\nTask: {}\n", req.from, depth, req.task.trim());
    let context: String = req.context.trim().chars().take(CONTEXT_MAX).collect();
    if !context.is_empty() {
        text.push_str(&format!("\nContext from the caller:\n{}\n", context));
    }
    text.push_str("\nDo the task in your mode. End with a short result: what you did, what you found, what is still open. Keep the details in files or attachments and name them.");
    text
}

/// Runs the sub-session. `cancel` is the stop flag of the person; a stop ends the sub-session and returns an error.
pub fn run(ctx: &Ctx, req: &Request, cancel: Option<Arc<AtomicBool>>) -> Result<Outcome, String> {
    if !valid(&req.to) {
        return Err(format!("NOT handed off. {} is no mode. The modes are: {}.", req.to, NAMES.join(", ")));
    }
    if req.task.trim().is_empty() {
        return Err("NOT handed off. The task is empty.".to_string());
    }
    if depth(ctx) >= MAX_DEPTH {
        return Err(format!("NOT handed off. Hand-offs are {} deep already, and {} is the limit. Do the task yourself or answer with what you have.", depth(ctx), MAX_DEPTH));
    }
    if req.from == req.to {
        return Err(format!("NOT handed off. You are in mode {} already.", req.to));
    }
    let cancel = cancel.or_else(|| CALLER_CANCEL.with(|c| c.borrow().clone()));
    if cancel.as_ref().map(|c| c.load(Ordering::SeqCst)).unwrap_or(false) {
        return Err("NOT handed off. The turn was stopped.".to_string());
    }
    let deeper = depth(ctx) + 1;
    let sub_ctx = effective(ctx, &req.to, deeper);
    let prompt = prompt_for(req, deeper);
    let taint = crate::gates::snapshot().map(|t| t.source).or_else(|| ctx.cfg["_gate_taint"].as_str().map(String::from));
    let (text, session, calls, tainted) = if req.to == "agents" {
        // The cluster runner asks the person for every open limit and for the yes, then runs the clusters.
        let goal = format!("{}{}", req.task.trim(), if req.context.trim().is_empty() { String::new() } else { format!("\n\nContext: {}", req.context.trim().chars().take(CONTEXT_MAX).collect::<String>()) });
        let text = crate::clarify::start_cluster(&sub_ctx, &goal, None, true, &serde_json::Map::new())
            .map_err(|e| if e.starts_with("NOT") { e } else { format!("NOT handed off. {}", e) })?;
        (text, crate::sessions::new_id().replacen("api-", "hand-", 1), 0u64, taint.is_some())
    } else {
        let mut chat = crate::backend::Chat::new();
        chat.mode = req.to.clone();
        chat.set_taint(taint);
        let (sub_cancel, sub_pid) = (chat.cancel.clone(), chat.pid.clone());
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx2 = sub_ctx.clone();
        std::thread::spawn(move || {
            let mut calls = 0u64;
            let out = chat.turn_with(&ctx2, &prompt, "", &mut |e| {
                if matches!(e, crate::backend::Event::ToolStart { .. }) { calls += 1; }
            });
            let _ = tx.send((out, chat.session.clone(), calls, chat.tainted()));
        });
        let result = loop {
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(done) => break done,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    if cancel.as_ref().map(|c| c.load(Ordering::SeqCst)).unwrap_or(false) {
                        sub_cancel.store(true, Ordering::SeqCst);
                        let pid = sub_pid.load(Ordering::SeqCst);
                        if pid != 0 {
                            let _ = crate::platform::kill_tree(pid as u64);
                        }
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return Err(format!("NOT handed off. The sub-session in mode {} ended without a result.", req.to)),
            }
        };
        let (out, session, calls, tainted) = result;
        let text = out.map_err(|e| format!("NOT handed off. The sub-session in mode {} stopped: {}", req.to, e))?;
        (text, session.unwrap_or_else(crate::sessions::new_id), calls, tainted)
    };
    // The full result is an attachment: the caller reads a line and the card, and reads ranges when it needs them.
    let level = if tainted { 0 } else { 1 };
    let card = crate::attach::store_text(ctx, "hand_off", level, &text).map(|a| a.compact_line()).unwrap_or_else(|_| text.chars().take(1200).collect());
    let summary = format!("hand-off to mode {} done: {}", req.to, one_line(&text, 160));
    let summary = if calls > 0 { format!("{} ({} tool calls, session {})", summary, calls, session) } else { format!("{} (session {})", summary, session) };
    let link = format!("h-{:x}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
    let from_session = ctx.cfg["_caller_session"].as_str().filter(|s| !s.is_empty()).map(String::from);
    add_link(ctx, json!({"id": link, "at": now(), "from_mode": req.from, "to_mode": req.to, "from_session": from_session,
                          "caller": ctx.cfg["_caller"], "to_session": session, "depth": deeper,
                          "task": req.task.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(200).collect::<String>()}));
    // The sub-session is a stored session with its own transcript, so a tab can show it.
    let items = vec![json!({"k": "note", "label": "mode", "t": format!("mode {} · hand-off from {}", req.to, req.from), "c": [120, 170, 180]}),
                     json!({"k": "user", "t": req.task.trim()}), json!({"k": "reply", "t": text})];
    crate::sessions::save_transcript(ctx, &session, &items);
    crate::sessions::record(ctx, &session, &format!("[hand-off from {}] {}", req.from, req.task), 1, (0, 0));
    Ok(Outcome { text, summary, card, session })
}

/// The tool `hand_off`: the calling mode is the mode of this copy of the settings.
pub fn hand_off(ctx: &Ctx, a: &Value) -> Result<String, String> {
    let req = Request {
        from: current(ctx).unwrap_or_else(|| "code".to_string()),
        to: a["to_mode"].as_str().unwrap_or("").trim().to_lowercase(),
        task: a["task"].as_str().unwrap_or("").to_string(),
        context: a["context"].as_str().unwrap_or("").to_string(),
    };
    run(ctx, &req, None).map(|o| o.result())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(tag: &str) -> Ctx {
        let root = std::env::temp_dir().join(format!("penelope-modes-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        crate::config::load(&root)
    }

    #[test]
    fn the_defaults_name_the_four_modes_with_their_tools_model_and_effort() {
        let c = ctx("defaults");
        assert_eq!(default_mode(&c), "code");
        let chat_tools = tool_list(&c, "chat");
        assert_eq!(chat_tools.len(), 2);
        assert!(chat_tools.contains(&"memory_find") && chat_tools.contains(&"ask_user"));
        let work = tool_list(&c, "work");
        assert!(["read", "write", "edit", "web_search", "deep_research", "attachment_read", "run", "hand_off"].iter().all(|t| work.contains(t)));
        assert!(!work.contains(&"cluster_start") && !work.contains(&"git_commit") && !work.contains(&"computer"));
        let agents = tool_list(&c, "agents");
        assert!(agents.contains(&"cluster_start") && agents.contains(&"cluster_status") && !agents.contains(&"write") && !agents.contains(&"edit"));
        assert_eq!(tool_list(&c, "code").len(), crate::spec::names().len());
        let chat = effective(&c, "chat", 0);
        assert_eq!((chat.cfg["main"]["model"].as_str(), chat.cfg["main"]["effort"].as_str()), (Some("haiku"), Some("low")));
        let code = effective(&c, "code", 0);
        assert_eq!((code.cfg["main"]["model"].as_str(), code.cfg["main"]["effort"].as_str(), code.cfg["route"].as_str()), (Some("sonnet"), Some("high"), Some("fixed")));
    }

    #[test]
    fn the_config_overrides_model_effort_tools_and_route() {
        let mut c = ctx("override");
        c.cfg["modes"]["work"] = json!({"model": "opus", "effort": "max", "route": "auto", "tools": ["read", "web_search", "nope"]});
        c.cfg["modes"]["code"]["tools"] = json!(["*", "-computer", "-screen"]);
        c.cfg["modes"]["agents"]["model"] = json!({"kind": "openai", "model": "gpt-x", "base_url": "http://127.0.0.1:1", "api_key_env": ""});
        let work = effective(&c, "work", 0);
        assert_eq!((work.cfg["main"]["model"].as_str(), work.cfg["main"]["effort"].as_str(), work.cfg["route"].as_str()), (Some("opus"), Some("max"), Some("auto")));
        assert_eq!(work.cfg["main"]["kind"], "claude-cli");
        assert_eq!(tool_list(&c, "work"), vec!["web_search", "read"]);
        assert!(!allows(&work, "write") && allows(&work, "read"));
        let code = effective(&c, "code", 0);
        assert!(!allows(&code, "computer") && !allows(&code, "screen") && allows(&code, "run"));
        assert_eq!(effective(&c, "agents", 0).cfg["main"]["model"], "gpt-x");
        // A copy made from a copy starts from the person's own main model, not from the mode before.
        let again = effective(&work, "chat", 1);
        assert_eq!(again.cfg["main"]["model"], "haiku");
        assert_eq!(effective(&again, "code", 2).cfg["main"]["model"], "sonnet");
        assert_eq!(depth(&effective(&again, "code", 2)), 2);
    }

    #[test]
    fn a_copy_without_a_mode_allows_every_tool_and_a_mode_refuses_the_others() {
        let c = ctx("refuse");
        assert!(allows(&c, "write") && current(&c).is_none());
        let chat = effective(&c, "chat", 0);
        assert!(allows(&chat, "memory_find") && !allows(&chat, "run") && !allows(&chat, "some_forged_tool"));
        let said = crate::route::dispatch_as(&chat, "lead", "run", &json!({"command": "ls"})).unwrap_err();
        assert!(said.starts_with("NOT called. Mode chat has no tool run") && said.contains("ask_user, memory_find"), "{}", said);
        assert!(crate::mcp::main_tools(&chat).iter().all(|t| matches!(*t, "memory_find" | "ask_user")));
        assert_eq!(crate::mcp::tool_specs_for(&chat, "lead").as_array().unwrap().len(), 2);
        assert!(crate::mcp::main_tools(&effective(&c, "code", 0)).contains(&"hand_off"));
    }

    // ---------- hand-off ----------

    fn reply(text: &str) -> (u16, String) {
        (200, json!({"choices": [{"finish_reason": "stop", "message": {"role": "assistant", "content": text}}], "usage": {"prompt_tokens": 50, "completion_tokens": 9}}).to_string())
    }

    fn call_tool(name: &str, args: Value) -> (u16, String) {
        (200, json!({"choices": [{"finish_reason": "tool_calls", "message": {"role": "assistant", "content": null, "tool_calls": [
            {"id": "call_1", "type": "function", "function": {"name": name, "arguments": args.to_string()}}]}}], "usage": {"prompt_tokens": 40, "completion_tokens": 8}}).to_string())
    }

    /// Settings whose main and weaker model are one fake endpoint that answers `replies` in order.
    fn endpoint(tag: &str, replies: Vec<(u16, String)>) -> (Ctx, std::sync::Arc<std::sync::Mutex<Vec<crate::llm::fake::Seen>>>) {
        let (address, seen) = crate::llm::fake::serve(replies);
        let mut c = ctx(tag);
        c.cfg["main"] = json!({"kind": "openai", "base_url": format!("{}/v1", address), "model": "gpt-test", "api_key_env": ""});
        c.cfg["weak"] = c.cfg["main"].clone();
        c.cfg["auto_learn"] = json!(false);
        (c, seen)
    }

    fn caller(c: &Ctx, mode: &str) -> Ctx {
        let mut k = effective(c, mode, 0);
        k.cfg["_caller"] = json!("chat-key-1");
        k.cfg["_caller_session"] = json!("caller-session-1");
        k
    }

    #[test]
    fn a_hand_off_opens_a_sub_session_with_the_context_and_returns_a_line_and_an_attachment() {
        let (c, seen) = endpoint("handoff", vec![reply("Fixed parser.rs: the off-by-one is gone.\nTests pass: 41 of 41.")]);
        let from = caller(&c, "work");
        let text = hand_off(&from, &json!({"to_mode": "code", "task": "fix the failing test in parser.rs", "context": "The person wants the parser tests green."})).unwrap();
        let mut lines = text.lines();
        let first = lines.next().unwrap();
        assert!(first.starts_with("hand-off to mode code done: Fixed parser.rs: the off-by-one is gone.") && first.contains("session api-"), "{}", text);
        let card = lines.next().unwrap();
        assert!(card.starts_with("[attachment hand_off-") && card.contains("2 lines") && card.contains("attachment_read uri=penelope-local://"), "{}", card);
        // The sub-session saw the task, the context and its own mode, not the conversation of the caller.
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        let body = &seen[0].body;
        let user = body["messages"].as_array().unwrap().iter().find(|m| m["role"] == "user").unwrap()["content"].as_str().unwrap();
        assert!(user.contains("from mode work") && user.contains("fix the failing test in parser.rs") && user.contains("The person wants the parser tests green."), "{}", user);
        assert!(body["messages"][0]["content"].as_str().unwrap().contains("Mode code:"));
        let tools: Vec<&str> = body["tools"].as_array().unwrap().iter().filter_map(|t| t["function"]["name"].as_str()).collect();
        assert!(tools.contains(&"edit") && tools.contains(&"hand_off"), "{:?}", tools);
        // The full result is the attachment; the id of the sub-session and the link are stored.
        let uri = card.split("uri=").nth(1).unwrap().trim();
        assert!(crate::attach::read_tool(&c, &json!({"uri": uri})).unwrap().contains("Tests pass: 41 of 41."));
        let linked = links_of(&c, "caller-session-1");
        assert_eq!(linked.len(), 1);
        assert_eq!((linked[0]["from_mode"].as_str(), linked[0]["to_mode"].as_str(), linked[0]["depth"].as_u64()), (Some("work"), Some("code"), Some(1)));
        let sub = linked[0]["to_session"].as_str().unwrap().to_string();
        assert_eq!(links_of(&c, &sub).len(), 1, "the link is found from the sub-session too");
        assert!(crate::sessions::pick(&c, &sub).is_some() && crate::sessions::load_transcript(&c, &sub).len() == 3);
        assert!(report(&c, "work", "caller-session-1").contains("work → code  fix the failing test in parser.rs"));
    }

    #[test]
    fn a_chat_that_started_without_a_session_id_gets_its_link_bound_later() {
        let (c, _seen) = endpoint("bind", vec![reply("done")]);
        let mut from = effective(&c, "work", 0);
        from.cfg["_caller"] = json!("chat-key-2");
        hand_off(&from, &json!({"to_mode": "chat", "task": "say done"})).unwrap();
        assert!(links(&c)[0]["from_session"].is_null());
        bind_caller(&c, "chat-key-2", "real-session-9");
        assert_eq!(links_of(&c, "real-session-9").len(), 1);
    }

    #[test]
    fn a_hand_off_is_refused_for_a_bad_mode_an_empty_task_the_same_mode_and_at_the_depth_limit() {
        let c = ctx("refusals");
        let work = caller(&c, "work");
        assert!(hand_off(&work, &json!({"to_mode": "kitchen", "task": "x"})).unwrap_err().contains("kitchen is no mode"));
        assert!(hand_off(&work, &json!({"to_mode": "code", "task": "  "})).unwrap_err().contains("task is empty"));
        assert!(hand_off(&work, &json!({"to_mode": "work", "task": "x"})).unwrap_err().contains("in mode work already"));
        let deep = effective(&c, "work", MAX_DEPTH);
        let said = hand_off(&deep, &json!({"to_mode": "code", "task": "x"})).unwrap_err();
        assert!(said.starts_with("NOT handed off. Hand-offs are 2 deep already"), "{}", said);
        assert!(links(&c).is_empty());
    }

    #[test]
    fn two_hand_offs_deep_work_and_the_third_is_refused_inside_the_model_loop() {
        // work -> code (depth 1) -> work (depth 2); the model of depth 2 tries once more and reads the refusal.
        let (c, seen) = endpoint("depth", vec![
            call_tool("hand_off", json!({"to_mode": "work", "task": "look up what 2 plus 2 is"})),        // depth 1 hands on
            call_tool("hand_off", json!({"to_mode": "chat", "task": "and again"})),                          // depth 2 tries a third
            reply("Cannot hand on. Answering myself: 4."),                                                     // depth 2 answers after the refusal
            reply("The helper said: 4."),                                                                      // depth 1 answers
        ]);
        let text = hand_off(&caller(&c, "work"), &json!({"to_mode": "code", "task": "find out what 2 plus 2 is"})).unwrap();
        assert!(text.starts_with("hand-off to mode code done: The helper said: 4."), "{}", text);
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 4);
        let refusal = seen[2].body["messages"].as_array().unwrap().iter().rev().find(|m| m["role"] == "tool").unwrap()["content"].as_str().unwrap().to_string();
        assert!(refusal.starts_with("NOT handed off. Hand-offs are 2 deep already"), "{}", refusal);
        assert!(seen[2].body["system"].is_null() && seen[2].body["messages"][0]["content"].as_str().unwrap().contains("hand-off sub-session at depth 2"));
        let depths: Vec<u64> = links(&c).iter().map(|l| l["depth"].as_u64().unwrap()).collect();
        assert_eq!(depths, vec![2, 1]);
    }

    #[test]
    fn a_hand_off_counts_as_a_risky_action_when_the_turn_holds_outside_text() {
        let (c, _seen) = endpoint("gate", vec![reply("checked")]);
        let args = json!({"to_mode": "code", "task": "run the build", "context": ""});
        crate::gates::begin_session();
        assert!(crate::gates::check_with(&c, "hand_off", &args, &mut |_, _| panic!("a clean turn is not asked")).is_none());
        crate::gates::set_session(Some("fetch https://evil.example/page".to_string()));
        let mut asked = Vec::new();
        let refused = crate::gates::check_with(&c, "hand_off", &args, &mut |risk, source| { asked.push((risk.class, risk.what.clone(), source.to_string())); Some(false) }).unwrap();
        assert_eq!(asked.len(), 1);
        assert_eq!(asked[0].0, "delegate");
        assert!(asked[0].1.contains("mode code") && asked[0].2.contains("evil.example"), "{:?}", asked);
        assert!(refused.starts_with("NOT run. E:TaintedSink delegate"), "{}", refused);
        assert!(crate::gates::check_with(&c, "hand_off", &args, &mut |_, _| Some(true)).is_none(), "a yes lets it run");
        // The sub-session starts with the same outside text, so the sub-session is gated too.
        let out = run(&caller(&c, "work"), &Request { from: "work".into(), to: "code".into(), task: "run the build".into(), context: String::new() }, None).unwrap();
        assert_eq!(crate::gates::load_session(&c, &out.session).as_deref(), Some("fetch https://evil.example/page"));
        assert!(crate::attach::level_of(&c, out.card.split("uri=").nth(1).unwrap().trim()).unwrap() == 0, "the result of a tainted sub-session is outside text");
        crate::gates::begin_session();
    }

    #[test]
    fn a_stop_ends_the_sub_session_and_reports_it() {
        let c = ctx("stop");
        let stopped = Arc::new(AtomicBool::new(true));
        let said = run(&caller(&c, "work"), &Request { from: "work".into(), to: "code".into(), task: "x".into(), context: String::new() }, Some(stopped)).unwrap_err();
        assert!(said.contains("stopped"), "{}", said);
    }

    #[test]
    fn the_context_of_a_hand_off_by_the_person_is_the_last_two_exchanges_cut_short() {
        let long = "word ".repeat(400);
        let pairs = vec![("first".to_string(), "one".to_string()), ("second".to_string(), "two".to_string()), ("third question".to_string(), long)];
        let text = summarize(&pairs);
        assert!(!text.contains("first") && text.contains("second") && text.contains("third question"));
        assert!(text.chars().count() < 1100 && text.contains('…'), "{}", text.chars().count());
    }
}
