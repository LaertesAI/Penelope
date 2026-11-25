//! Agents that the user writes into Penelope with `$`.
//!
//! `$builder(role=orchestrator,thinking=max,goal="Build a site",max_sub_agents=3)` names an agent, its role, how hard
//! it thinks, its goal and its limits. The name and the settings are stored in `.penelope/agents.json`; the goal itself
//! is not stored, because a goal may hold private text.
//!
//! A run turns the goal into a small plan: the lead inspects and plans, a worker does the change, the judge checks the
//! evidence, the lead reports. Every step is a real model call under the role of the step, with the tools of that role
//! and through the same gate as every other call. `max_sub_agents`, `max_parallel`, `max_steps` and `budget_eur` are
//! limits, and a limit that is reached stops the run instead of being ignored.

use crate::config::Ctx;
use serde_json::{json, Value};
use std::fs;
use std::process::{Command, Stdio};

thread_local! {
    /// How many agent runs are open on this thread. A subagent cannot start another subagent.
    static DEPTH: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// The MCP servers a run may call. None means every server the plugins allow.
    static MCP_ALLOW: std::cell::RefCell<Option<Vec<String>>> = const { std::cell::RefCell::new(None) };
}

/// Runs a block with a list of MCP servers. The list is passed from the lead to the worker and the judge of a run.
pub fn with_mcp_scope<T>(list: &[String], f: impl FnOnce() -> T) -> T {
    let before = MCP_ALLOW.with(|m| m.replace(if list.is_empty() { None } else { Some(list.to_vec()) }));
    let out = f();
    MCP_ALLOW.with(|m| *m.borrow_mut() = before);
    out
}

/// True when the current run may call this MCP server. An empty scope allows every server the plugins allow.
pub fn mcp_allowed(server: &str) -> bool {
    MCP_ALLOW.with(|m| m.borrow().as_ref().map(|list| list.iter().any(|s| s == server)).unwrap_or(true))
}

pub fn mcp_scope() -> Option<Vec<String>> {
    MCP_ALLOW.with(|m| m.borrow().clone())
}

pub fn depth() -> usize {
    DEPTH.with(|d| d.get())
}

struct McpGuard(Option<Vec<String>>);
impl McpGuard {
    fn new(list: &[String]) -> McpGuard {
        let before = MCP_ALLOW.with(|m| m.replace(if list.is_empty() { None } else { Some(list.to_vec()) }));
        McpGuard(before)
    }
}
impl Drop for McpGuard {
    fn drop(&mut self) {
        let before = self.0.take();
        MCP_ALLOW.with(|m| *m.borrow_mut() = before);
    }
}

struct DepthGuard;
impl Drop for DepthGuard {
    fn drop(&mut self) {
        DEPTH.with(|d| d.set(d.get().saturating_sub(1)));
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Spec {
    pub name: String,
    /// orchestrator, lead, worker or judge.
    pub role: String,
    /// max, high, medium or low.
    pub thinking: String,
    pub goal: String,
    pub max_sub_agents: usize,
    pub max_parallel: usize,
    pub max_steps: usize,
    /// 0 means no money limit.
    pub budget_eur: f64,
    pub model: String,
    pub verify: bool,
    pub language: String,
    /// MCP servers of plugins that this run may call. Empty means every server the plugins allow.
    pub mcp: Vec<String>,
}

const KEYS: &[&str] = &["role", "thinking", "goal", "max_sub_agents", "max_parallel", "max_steps", "budget_eur", "model", "verify", "language", "mcp"];

fn split_args(text: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut escape = false;
    for c in text.chars() {
        if escape {
            cur.push(match c { 'n' => '\n', 't' => '\t', other => other });
            escape = false;
            continue;
        }
        match (quote, c) {
            (Some(_), '\\') => escape = true,
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => cur.push(c),
            (None, '"') | (None, '\'') => quote = Some(c),
            (None, ',') => { out.push(std::mem::take(&mut cur)); }
            (None, c) => cur.push(c),
        }
    }
    if quote.is_some() {
        return Err("NOT parsed. A quote is not closed.".to_string());
    }
    out.push(cur);
    Ok(out.into_iter().map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect())
}

/// Parses `$name(...)`. A number that does not fit its range, an unknown key and a missing goal are refused with the
/// exact word that is wrong, so the person can repair the call.
pub fn parse(text: &str) -> Result<Spec, String> {
    let trimmed = text.trim();
    let Some(rest) = trimmed.strip_prefix('$') else {
        return Err("NOT parsed. An agent call starts with $, for example $builder(goal=\"...\").".to_string());
    };
    let Some(open) = rest.find('(') else {
        return Err("NOT parsed. An agent call needs (...), for example $builder(goal=\"...\").".to_string());
    };
    let Some(inside) = rest[open + 1..].strip_suffix(')') else {
        return Err("NOT parsed. The agent call ends with ).".to_string());
    };
    let name = rest[..open].trim();
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-') {
        return Err("NOT parsed. The name of an agent is 1 to 32 lower case letters, digits, _ or -.".to_string());
    }
    let mut spec = Spec { name: name.to_string(), role: "orchestrator".into(), thinking: "high".into(), goal: String::new(),
                          max_sub_agents: 3, max_parallel: 2, max_steps: 12, budget_eur: 0.0, model: String::new(), verify: true, language: "en".into(), mcp: Vec::new() };
    for part in split_args(inside)? {
        let Some((key, value)) = part.split_once('=') else {
            return Err(format!("NOT parsed. {} is no key=value pair.", part));
        };
        let key = key.trim().to_lowercase();
        let value = value.trim();
        let unquote = |v: &str| v.trim_matches(|c| c == '"' || c == '\'').trim().to_string();
        if !KEYS.contains(&key.as_str()) {
            return Err(format!("NOT parsed. Unknown setting {}. The settings are: {}.", key, KEYS.join(", ")));
        }
        let number = |v: &str, low: f64, high: f64| -> Result<f64, String> {
            let n: f64 = v.parse().map_err(|_| format!("NOT parsed. {} needs a number, got {}", key, v))?;
            if n < low || n > high {
                return Err(format!("NOT parsed. {} is between {} and {}, got {}.", key, low, high, n));
            }
            Ok(n)
        };
        match key.as_str() {
            "role" => {
                let v = unquote(value).to_lowercase();
                if !["orchestrator", "lead", "worker", "judge"].contains(&v.as_str()) {
                    return Err("NOT parsed. role is orchestrator, lead, worker or judge.".to_string());
                }
                spec.role = v;
            }
            "thinking" => {
                let v = unquote(value).to_lowercase();
                if !["max", "high", "medium", "low"].contains(&v.as_str()) {
                    return Err("NOT parsed. thinking is max, high, medium or low.".to_string());
                }
                spec.thinking = v;
            }
            "goal" => {
                let v = unquote(value);
                if v.len() < 3 || v.len() > 2000 {
                    return Err("NOT parsed. The goal is 3 to 2000 characters.".to_string());
                }
                spec.goal = v;
            }
            "max_sub_agents" => spec.max_sub_agents = number(value, 1.0, 8.0)? as usize,
            "max_parallel" => spec.max_parallel = number(value, 1.0, 4.0)? as usize,
            "max_steps" => spec.max_steps = number(value, 2.0, 30.0)? as usize,
            "budget_eur" => spec.budget_eur = number(value, 0.0, 100.0)?,
            "model" => spec.model = unquote(value),
            "verify" => spec.verify = matches!(unquote(value).to_lowercase().as_str(), "true" | "yes" | "1" | "on"),
            "language" => spec.language = unquote(value),
            "mcp" => {
                let list: Vec<String> = unquote(value).split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect();
                for name in &list {
                    if name.len() > 60 || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.') {
                        return Err(format!("NOT parsed. {} is no MCP server name.", name));
                    }
                }
                spec.mcp = list;
            }
            _ => {}
        }
    }
    if spec.goal.is_empty() {
        return Err("NOT parsed. An agent needs goal=\"what it should do\".".to_string());
    }
    if spec.model.len() > 80 {
        return Err("NOT parsed. The model name is at most 80 characters.".to_string());
    }
    Ok(spec)
}

fn registry(ctx: &Ctx) -> std::path::PathBuf {
    ctx.root.join("agents.json")
}

/// Stores the settings of an agent under its name. The goal is left out: it can hold private text.
pub fn save(ctx: &Ctx, spec: &Spec) -> Result<(), String> {
    let mut all: Vec<Value> = fs::read_to_string(registry(ctx)).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    all.retain(|a| a["name"].as_str() != Some(spec.name.as_str()));
    all.push(json!({"name": spec.name, "role": spec.role, "thinking": spec.thinking, "max_sub_agents": spec.max_sub_agents,
                    "max_parallel": spec.max_parallel, "max_steps": spec.max_steps, "budget_eur": spec.budget_eur,
                    "model": spec.model, "verify": spec.verify, "language": spec.language, "mcp": spec.mcp}));
    all.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    fs::write(registry(ctx), serde_json::to_string_pretty(&all).unwrap()).map_err(|e| format!("cannot write agents.json: {}", e))
}

pub fn list(ctx: &Ctx) -> String {
    let all: Vec<Value> = fs::read_to_string(registry(ctx)).ok().and_then(|s| serde_json::from_str::<Vec<Value>>(&s).ok()).unwrap_or_default();
    if all.is_empty() {
        return "no agent yet. $name(role=orchestrator,goal=\"...\") writes one; $name(...) later runs it.\n".to_string();
    }
    let mut out = String::new();
    for a in all {
        out.push_str(&format!("${}(role={}, thinking={}, max_sub_agents={}, max_parallel={}, budget_eur={}{}{})\n",
                              a["name"].as_str().unwrap_or("?"), a["role"].as_str().unwrap_or("?"), a["thinking"].as_str().unwrap_or("?"),
                              a["max_sub_agents"].as_u64().unwrap_or(0), a["max_parallel"].as_u64().unwrap_or(0), a["budget_eur"].as_f64().unwrap_or(0.0),
                              if a["verify"].as_bool().unwrap_or(true) { ", verify=true" } else { ", verify=false" },
                              a["mcp"].as_array().map(|m| format!(", mcp={}", m.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join(","))).unwrap_or_default()));
    }
    out.push_str("\nThe goal is not stored. Write it again when you run the agent.\n");
    out
}

#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    /// The name the user or the router gave the step, shown next to the role in the crew panel.
    pub name: String,
    pub role: &'static str,
    pub text: String,
}

/// The plan of one run. A max_sub_agents of 1 gives inspection and work to one worker; more room adds a judge. The lead
/// always writes the plan and the report.
pub fn plan(spec: &Spec) -> Vec<Step> {
    if spec.role == "worker" || spec.role == "judge" {
        return vec![Step { name: if spec.role == "worker" { "work" } else { "check" }.to_string(),
                            role: if spec.role == "worker" { "worker" } else { "judge" }, text: format!("Do exactly this and report the evidence: {}", spec.goal) }];
    }
    let mut steps = vec![Step { name: "plan".to_string(), role: "lead", text: format!("Inspect the project and write the shortest plan for this goal. Name the files and the check that will prove the result. Goal: {}", spec.goal) }];
    let room = spec.max_sub_agents;
    if room >= 1 {
        steps.push(Step { name: "work".to_string(), role: "worker", text: format!("Do the work of the goal. Change only what the goal needs. Run the check and copy its output as evidence. Goal: {}", spec.goal) });
    }
    if spec.verify && room >= 2 {
        steps.push(Step { name: "check".to_string(), role: "judge", text: format!("Check the worker's evidence against the goal and the project's own checks. Answer PASS or FAIL and one reason. Goal: {}", spec.goal) });
    }
    steps.push(Step { name: "report".to_string(), role: "lead", text: format!("Report what was done, where it is, what passed and what is still open. Goal: {}", spec.goal) });
    if steps.len() > spec.max_steps {
        let report = steps.pop().unwrap();
        steps.truncate(spec.max_steps.saturating_sub(1));
        steps.push(report);
    }
    steps
}

/// The plan a router model chooses, or the fixed plan when no router is set or its answer is broken. The router gets the
/// goal and the limits and answers one JSON object: `{"tasks":[{"role":"lead|worker|judge","text":"..."}]}`. A task that
/// names another role, an empty text or a list longer than `max_steps` is refused and the fixed plan is used instead.
pub fn routed_plan(ctx: &Ctx, spec: &Spec) -> (Vec<Step>, Option<String>) {
    let Some(model) = crate::route::model_for(ctx, "router") else {
        return (plan(spec), None);
    };
    if !model["model"].is_string() {
        return (plan(spec), None);
    }
    let system = "You are Tiresias, the router of a coding agent. You get one goal and its limits. Choose the smallest plan of 2 to \
{max} steps. Each step names a role: lead (talks, plans, reports), worker (changes files and runs the check) or judge (checks the evidence). \
The first step is lead and the last is lead. At most {sub} steps may be worker or judge. Answer ONLY one JSON object: {\"tasks\": [{\"role\": \"...\", \
\"text\": \"one concrete task\"}]}. No prose, no code fences.";
    let system = system.replace("{max}", &spec.max_steps.to_string()).replace("{sub}", &spec.max_sub_agents.to_string());
    let user = format!("Goal: {}\nVerify with a judge: {}\nLanguage: {}", spec.goal, spec.verify, spec.language);
    let (answer, tin, tout) = match crate::weak::ask_count(model, &system, &user) {
        Ok(a) => a,
        Err(e) => return (plan(spec), Some(format!("router failed ({}); fixed plan", e))),
    };
    crate::cost::record(ctx, "router", model["model"].as_str().unwrap_or(""), tin, tout);
    let Some(start) = answer.find('{') else { return (plan(spec), Some("router wrote no JSON; fixed plan".into())) };
    let Some(end) = answer.rfind('}') else { return (plan(spec), Some("router wrote no closed JSON; fixed plan".into())) };
    let v: Value = match serde_json::from_str(&answer[start..=end]) {
        Ok(v) => v,
        Err(e) => return (plan(spec), Some(format!("router wrote no valid JSON ({}); fixed plan", e))),
    };
    let Some(tasks) = v["tasks"].as_array() else { return (plan(spec), Some("router wrote no tasks; fixed plan".into())) };
    if tasks.len() < 2 || tasks.len() > spec.max_steps {
        return (plan(spec), Some(format!("router chose {} steps, outside 2 to {}; fixed plan", tasks.len(), spec.max_steps)));
    }
    let mut sub = 0;
    let mut steps = Vec::new();
    for task in tasks {
        let role = task["role"].as_str().unwrap_or("");
        let text = task["text"].as_str().unwrap_or("").trim().to_string();
        let role_ok = matches!(role, "lead" | "worker" | "judge");
        if !role_ok || text.len() < 5 || text.len() > 600 {
            return (plan(spec), Some("router named an unknown role or an empty task; fixed plan".into()));
        }
        let fallback = match role { "lead" => "lead", "worker" => "work", _ => "check" };
        let name = task["name"].as_str().map(str::trim).filter(|n| !n.is_empty()).unwrap_or(fallback).to_string();
        if name.len() > 24 || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.') {
            return (plan(spec), Some("router named a step with a broken name; fixed plan".into()));
        }
        if role != "lead" { sub += 1; }
        steps.push(Step { name, role: if role == "lead" { "lead" } else if role == "worker" { "worker" } else { "judge" }, text });
    }
    if sub > spec.max_sub_agents || steps.first().map(|s| s.role) != Some("lead") || steps.last().map(|s| s.role) != Some("lead") {
        return (plan(spec), Some("router broke the limits of the agent; fixed plan".into()));
    }
    (steps, Some("router chose the plan".to_string()))
}

fn thinking_line(level: &str) -> &'static str {
    match level {
        "max" => "Think as carefully as the model allows. Check every assumption against a tool result before you act.",
        "high" => "Think before every change and keep the plan short.",
        "medium" => "Work directly and check the result once.",
        _ => "Work quickly and keep every answer short.",
    }
}

struct Done {
    text: String,
    tokens_in: u64,
    tokens_out: u64,
}

fn api_task(ctx: &Ctx, spec: &Spec, step: &Step, model: &Value, previous: &str, cancel: Option<&std::sync::atomic::AtomicBool>) -> Result<Done, String> {
    let system = format!("You are the {} of a Penelope agent run. {}\nGoal: {}\nThe user talks to the lead only; your answer goes back to the run. \\
Report facts, paths and command output. Say plainly what failed. Tools are checked by the harness: a refusal starts with NOT called.\nWhat earlier steps found:\n{}",
                         step.role, thinking_line(&spec.thinking), spec.goal, if previous.is_empty() { "(nothing yet)" } else { previous });
    let tools: Vec<Value> = crate::mcp::tool_specs_for(ctx, step.role).as_array().cloned().unwrap_or_default();
    let mut history = vec![crate::llm::user_message(&step.text)];
    let mut usage = crate::llm::Usage::default();
    let out = crate::route::with_role(step.role, || crate::llm::run_loop(model, &system, &mut history, &tools, spec.max_steps, cancel, &mut usage, &mut |call| {
        crate::tools::dispatch_retry(ctx, step.role, &call.name, &call.args)
    }, &mut |_| {}));
    crate::cost::record(ctx, step.role, model["model"].as_str().unwrap_or(""), usage.tokens_in, usage.tokens_out);
    Ok(Done { text: out?, tokens_in: usage.tokens_in, tokens_out: usage.tokens_out })
}

fn claude_task(ctx: &Ctx, spec: &Spec, step: &Step, model: &Value, previous: &str) -> Result<Done, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut env = json!({"PENELOPE_ROLE": step.role});
    if let Ok(dir) = std::env::var("PENELOPE_APPROVE_DIR") {
        env["PENELOPE_APPROVE_DIR"] = json!(dir);
    }
    let cfg = json!({"mcpServers": {"penelope": {"command": exe.to_string_lossy(), "args": ["mcp", "--root", ctx.root.to_string_lossy()], "env": env}}}).to_string();
    let allowed: Vec<String> = crate::route::allowed_for(step.role).iter().map(|t| format!("mcp__penelope__{}", t)).collect();
    let system = format!("You are the {} of a Penelope agent run. {}\nGoal: {}\nYour answer goes back to the run, not to the user. \\
Report facts, paths and command output. What earlier steps found:\n{}", step.role, thinking_line(&spec.thinking), spec.goal,
                         if previous.is_empty() { "(nothing yet)" } else { previous });
    let mut cmd = Command::new("claude");
    cmd.args(["-p", &step.text, "--output-format", "json", "--model", model["model"].as_str().unwrap_or("sonnet"),
              "--append-system-prompt", &system, "--strict-mcp-config", "--mcp-config", &cfg, "--tools", "", "--allowedTools", &allowed.join(",")]);
    if let Some(level) = crate::llm::effort(model) {
        cmd.args(["--effort", level]);
    }
    let out = cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).output()
        .map_err(|e| format!("cannot start the claude command: {}", e))?;
    let v: Value = serde_json::from_slice(&out.stdout).map_err(|_| format!("the claude command ended with {} and no result", out.status))?;
    if v["is_error"].as_bool().unwrap_or(false) {
        return Err(v["result"].as_str().unwrap_or("the claude command reported an error").trim().to_string());
    }
    let tokens_in = v["usage"]["input_tokens"].as_u64().unwrap_or(0) + v["usage"]["cache_creation_input_tokens"].as_u64().unwrap_or(0);
    let tokens_out = v["usage"]["output_tokens"].as_u64().unwrap_or(0);
    crate::cost::record(ctx, step.role, model["model"].as_str().unwrap_or(""), tokens_in, tokens_out);
    Ok(Done { text: v["result"].as_str().unwrap_or("").trim().to_string(), tokens_in, tokens_out })
}

/// One task on one role, without a plan. The tool role uses this to drive the computer: the lead hands a task to a
/// smaller model, that model calls `screen`, `cursor` or a plugin MCP tool and returns a short report.
pub fn free_task(ctx: &Ctx, role: &str, task: &str, max_steps: usize) -> Result<String, String> {
    if task.trim().is_empty() {
        return Err("NOT run. The task is empty.".to_string());
    }
    let Some(model) = crate::route::model_for(ctx, role).cloned() else {
        return Err(format!("NOT run. No model is set for the role {}.", role));
    };
    if let Some(family) = crate::crew::family_of(&model) {
        return Err(format!("{} 1 is in training and cannot take this task.", family));
    }
    let role_static: &'static str = match role { "tool" => "tool", "scout" => "scout", "worker" => "worker", "judge" => "judge", _ => "lead" };
    let spec = Spec { name: role.to_string(), role: "worker".into(), thinking: "medium".into(), goal: task.to_string(),
                      max_sub_agents: 1, max_parallel: 1, max_steps: max_steps.clamp(1, 20), budget_eur: 0.0, model: String::new(),
                      verify: false, language: "en".into(), mcp: Vec::new() };
    let step = Step { name: "task".to_string(), role: role_static, text: task.to_string() };
    let done = match model["kind"].as_str().unwrap_or("claude-cli") {
        kind if crate::llm::is_api(kind) => api_task(ctx, &spec, &step, &model, "", None),
        _ => claude_task(ctx, &spec, &step, &model, ""),
    }?;
    Ok(done.text)
}

/// Runs the plan. Every step sees the short reports of the steps before it. A step that fails is reported and the run
/// continues with what is known; a budget that is used up stops the run.
pub fn run(ctx: &Ctx, spec: &Spec) -> Result<String, String> {
    run_with_cancel(ctx, spec, None)
}

/// The same run, with a flag another thread can set to stop it before the next step and inside the tool loop of an
/// HTTP model. A running claude command is not killed; the run stops after it returns.
pub fn run_with_cancel(ctx: &Ctx, spec: &Spec, cancel: Option<&std::sync::atomic::AtomicBool>) -> Result<String, String> {
    use std::sync::atomic::Ordering;
    if spec.goal.trim().is_empty() {
        return Err("NOT run. The agent has no goal.".to_string());
    }
    DEPTH.with(|d| d.set(d.get() + 1));
    let _depth = DepthGuard;
    crate::crew::clear(ctx);
    let servers = spec.mcp.clone();
    let _mcp = McpGuard::new(&servers);
    let (steps, routing) = routed_plan(ctx, spec);
    let mut out = format!("${} · role {} · thinking {} · goal: {}\n{} steps, at most {} sub agents, {} in parallel, {} steps per task.{}{}\n",
                          spec.name, spec.role, spec.thinking, spec.goal, steps.len(), spec.max_sub_agents, spec.max_parallel, spec.max_steps,
                          routing.map(|r| format!(" {}", r)).unwrap_or_default(),
                          if spec.mcp.is_empty() { String::new() } else { format!(" MCP: {}.", spec.mcp.join(", ")) });
    let before = crate::cost::total_eur(ctx);
    let mut previous = String::new();
    let mut log = Vec::new();
    let mut spent_tokens = (0u64, 0u64);
    for (i, step) in steps.iter().enumerate() {
        if cancel.map(|c| c.load(Ordering::SeqCst)).unwrap_or(false) {
            out.push_str(&format!("\nStopped before step {}.\n", i + 1));
            break;
        }
        if spec.budget_eur > 0.0 {
            let spent = crate::cost::total_eur(ctx) - before;
            if spent >= spec.budget_eur {
                out.push_str(&format!("\nStopped before step {}: {:.4} of {:.2} EUR used.\n", i + 1, spent, spec.budget_eur));
                break;
            }
        }
        let member = format!("{}.{}", spec.name, step.name);
        out.push_str(&format!("\n[{}/{}] {} · {} · {}\n", i + 1, steps.len(), member, step.role, crate::route::model_name(ctx, step.role)));
        let model = if !spec.model.is_empty() && step.role == "lead" {
            let mut m = crate::route::model_for(ctx, "lead").cloned().unwrap_or_else(|| json!({"kind": "claude-cli"}));
            m["model"] = json!(spec.model);
            m
        } else {
            match crate::route::model_for(ctx, step.role) {
                Some(m) => m.clone(),
                None => {
                    out.push_str(&format!("\n[{}] {} · skipped: no model is set for this role (penelope provider use NAME --role {}).\n", i + 1, step.role, step.role));
                    crate::crew::set(ctx, &member, step.role, &json!({"model": "no model"}), "no model");
                    continue;
                }
            }
        };
        if let Some(family) = crate::crew::family_of(&model) {
            out.push_str(&format!("{} is in training and cannot answer yet. The step stays open.\n", family));
            crate::crew::set(ctx, &member, step.role, &model, "in training");
            log.push(json!({"name": member, "role": step.role, "task": step.text, "ok": false, "error": format!("{} is in training", family)}));
            continue;
        }
        crate::crew::set(ctx, &member, step.role, &model, "working");
        let result = match model["kind"].as_str().unwrap_or("claude-cli") {
            kind if crate::llm::is_api(kind) => api_task(ctx, spec, step, &model, &previous, cancel),
            kind => {
                let _ = kind;
                claude_task(ctx, spec, step, &model, &previous)
            }
        };
        match result {
            Ok(d) => {
                spent_tokens.0 += d.tokens_in;
                spent_tokens.1 += d.tokens_out;
                let short: String = d.text.chars().take(2400).collect();
                out.push_str(&short);
                out.push('\n');
                previous.push_str(&format!("[{}] {}\n{}\n", member, step.text.chars().take(160).collect::<String>(), short.chars().take(700).collect::<String>()));
                log.push(json!({"name": member, "role": step.role, "task": step.text, "ok": true, "text": short, "tokens_in": d.tokens_in, "tokens_out": d.tokens_out}));
                crate::crew::set(ctx, &member, step.role, &model, "done");
            }
            Err(e) => {
                out.push_str(&format!("NOT done: {}\n", e));
                log.push(json!({"name": member, "role": step.role, "task": step.text, "ok": false, "error": e}));
                crate::crew::set(ctx, &member, step.role, &model, "failed");
            }
        }
    }
    let spent = crate::cost::total_eur(ctx) - before;
    let ran = log.iter().filter(|l| l["ok"].as_bool().unwrap_or(false)).count();
    out.push_str(&format!("\n{} steps ran, {} tokens in and {} out, {:.4} EUR this run.\n", ran, spent_tokens.0, spent_tokens.1, spent));
    let dir = ctx.root.join("agents").join("runs");
    let _ = fs::create_dir_all(&dir);
    let id = format!("{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0));
    let _ = fs::write(dir.join(format!("{}.json", id)), serde_json::to_string_pretty(&json!({"agent": spec.name, "goal": spec.goal, "steps": log})).unwrap());
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_example_from_the_user_parses_into_a_spec() {
        let spec = parse("$claude(role=orchestrator,thinking=max,goal=\"Baue mir eine komplette Website\",max_sub_agents=3)").unwrap();
        assert_eq!(spec.name, "claude");
        assert_eq!((spec.role.as_str(), spec.thinking.as_str(), spec.goal.as_str()), ("orchestrator", "max", "Baue mir eine komplette Website"));
        assert_eq!((spec.max_sub_agents, spec.max_parallel, spec.verify), (3, 2, true));
        assert_eq!(plan(&spec).len(), 4);
        let quiet = parse("$w(role=worker, goal='fix the test', verify=false, max_sub_agents=1, max_steps=4, budget_eur=1.5, model=haiku, mcp='demo.echo,notes')").unwrap();
        assert_eq!(quiet.mcp, vec!["demo.echo".to_string(), "notes".to_string()]);
        assert_eq!((quiet.max_sub_agents, quiet.max_steps, quiet.verify), (1, 4, false));
        assert_eq!(quiet.model, "haiku");
        assert_eq!(plan(&quiet).len(), 1);
        let one = parse("$o(goal=\"build\", max_sub_agents=1)").unwrap();
        assert_eq!(plan(&one).iter().map(|s| s.role).collect::<Vec<_>>(), vec!["lead", "worker", "lead"]);
    }

    #[test]
    fn a_broken_call_names_the_exact_word() {
        for (text, want) in [
            ("builder(goal=\"x\")", "starts with $"),
            ("$builder(goal=\"x\"", "ends with )"),
            ("$builder(role=king, goal=\"x\")", "role is orchestrator"),
            ("$builder(goal=\"build\", nope=1)", "Unknown setting nope"),
            ("$builder(goal=\"build\", max_sub_agents=99)", "max_sub_agents is between"),
            ("$builder()", "needs goal"),
            ("$builder(goal=\"build\", budget_eur=-1)", "budget_eur is between"),
        ] {
            let err = parse(text).unwrap_err();
            assert!(err.contains(want), "{} -> {}", text, err);
        }
    }

    #[test]
    fn a_router_model_chooses_the_plan_and_a_broken_answer_falls_back() {
        let (address, _seen) = crate::llm::fake::serve(vec![
            (200, serde_json::json!({"choices": [{"finish_reason": "stop", "message": {"role": "assistant", "content":
                "{\"tasks\":[{\"role\":\"lead\",\"text\":\"inspect the repo\"},{\"role\":\"worker\",\"text\":\"build the page\"},{\"role\":\"lead\",\"text\":\"report the result\"}]}"}}],
                "usage": {"prompt_tokens": 50, "completion_tokens": 30}}).to_string()),
            (200, serde_json::json!({"choices": [{"finish_reason": "stop", "message": {"role": "assistant", "content": "no json"}}]}).to_string()),
        ]);
        let dir = std::env::temp_dir().join(format!("penelope-router-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let mut ctx = crate::config::load(&dir.join(".penelope"));
        ctx.cfg["roles"] = serde_json::json!({"router": {"kind": "openai", "base_url": format!("{}/v1", address), "model": "router-test", "api_key_env": ""}});
        let spec = parse("$o(goal=\"build a page\", max_sub_agents=2)").unwrap();
        let (steps, note) = routed_plan(&ctx, &spec);
        assert_eq!(steps.iter().map(|s| s.role).collect::<Vec<_>>(), vec!["lead", "worker", "lead"]);
        assert_eq!(note.as_deref(), Some("router chose the plan"));
        let (fallback, why) = routed_plan(&ctx, &spec);
        assert_eq!(fallback.iter().map(|s| s.role).collect::<Vec<_>>(), vec!["lead", "worker", "judge", "lead"]);
        assert!(why.as_deref().unwrap_or("").contains("no JSON"), "{:?}", why);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn settings_are_stored_without_the_goal() {
        let dir = std::env::temp_dir().join(format!("penelope-agents-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let ctx = crate::config::load(&dir.join(".penelope"));
        assert!(list(&ctx).starts_with("no agent yet"));
        let spec = parse("$claude(role=orchestrator,thinking=max,goal=\"Privates Ziel\",max_sub_agents=3)").unwrap();
        save(&ctx, &spec).unwrap();
        let text = list(&ctx);
        assert!(text.contains("$claude(role=orchestrator, thinking=max") && !text.contains("Privates Ziel"), "{}", text);
        let stored = fs::read_to_string(dir.join(".penelope/agents.json")).unwrap();
        assert!(!stored.contains("Privates Ziel") && stored.contains("max_sub_agents"));
        let _ = fs::remove_dir_all(&dir);
    }
}
