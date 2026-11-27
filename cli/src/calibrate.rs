//! Self-calibration per model: run one task set in several variants and keep the variant that passed the most checks
//! as a model profile.
//!
//! A variant changes one factor of the default at a time:
//!
//! - `text`: tool calls as `<tool_call>` text blocks in the answer; the tool list moves into the system prompt and the
//!   API tool field stays empty. The default sends the list in the API tool field (native tool calls).
//! - `index`: the Mitos index of the tool list (name, first sentence, empty schema) plus `tool_describe`, which returns
//!   the full schema of named tools. The default sends every full schema.
//! - `effort-low`: effort `low` where the model has levels and its default is `high`.
//! - `prompt-short`: a 6-line system prompt in place of the 11 rules of `backend::rules`.
//!
//! The claude command runs its own tool loop, so a `claude-cli` model gets only the effort and prompt variants.
//!
//! `penelope calibrate MODEL` writes the winner to `.penelope/models/<provider>__<model>.json`. `backend.rs` reads that
//! file before every turn on the model (`apply`) and uses its tool format, tool list, effort and prompt.
//!
//! Every task run gets its own folder under the output folder: `work/` holds the copy of the setup repo, `.penelope/`
//! holds the config, the variant as profile file and the usage of that run. The turn runs in a child process
//! (`penelope calibrate-one`) in its own process group; a turn or a check over its time limit loses the whole group.
//! At most 3 runs go in parallel. A finished run is one line in `results.jsonl`; a second call skips the runs it finds there.

use crate::config::Ctx;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The tool that returns full schemas in the `index` variant.
pub const DESCRIBE: &str = "tool_describe";
/// The most task runs at once.
pub const MAX_JOBS: usize = 3;
/// The time one turn may take before its process group is killed.
pub const TURN_TIMEOUT_SECS: u64 = 600;

/// One setting of the four factors.
#[derive(Clone, Debug, PartialEq)]
pub struct Variant {
    pub name: String,
    /// `native` or `text`.
    pub tool_format: String,
    /// `full` or `index`.
    pub tool_list: String,
    /// The effort level, or empty for the model's default (or no effort field).
    pub effort: String,
    /// `current` or `short`.
    pub prompt: String,
    /// The most tool-call rounds of one turn through an HTTP endpoint; 0 keeps the built-in 12.
    pub rounds: u64,
}

impl Variant {
    pub fn default_for(effort: &str) -> Variant {
        Variant { name: "default".into(), tool_format: "native".into(), tool_list: "full".into(), effort: effort.into(), prompt: "current".into(), rounds: 0 }
    }

    pub fn to_json(&self) -> Value {
        json!({"variant": self.name, "tool_format": self.tool_format, "tool_list": self.tool_list, "effort": self.effort, "prompt": self.prompt, "rounds": self.rounds})
    }

    pub fn from_json(v: &Value) -> Variant {
        let s = |k: &str, d: &str| v[k].as_str().filter(|x| !x.is_empty()).unwrap_or(d).to_string();
        Variant { name: s("variant", "default"), tool_format: s("tool_format", "native"), tool_list: s("tool_list", "full"),
                  effort: v["effort"].as_str().unwrap_or("").to_string(), prompt: s("prompt", "current"), rounds: v["rounds"].as_u64().unwrap_or(0) }
    }
}

/// The provider name of a model setting: `claude-cli`, `opencode`, ... or its kind when no table entry matches.
pub fn provider_of(model: &Value) -> String {
    crate::providers::of(model).map(|p| p.name.to_string()).unwrap_or_else(|| crate::llm::kind_of(model).to_string())
}

/// The default and one variant per factor that applies to the model.
pub fn variants(provider: &str, model: &str) -> Vec<Variant> {
    let default_effort = crate::providers::default_effort(provider, model);
    let base = Variant::default_for(default_effort);
    let mut out = vec![base.clone()];
    if provider != "claude-cli" {
        out.push(Variant { name: "text".into(), tool_format: "text".into(), ..base.clone() });
        out.push(Variant { name: "index".into(), tool_list: "index".into(), ..base.clone() });
    }
    let levels = crate::providers::efforts(provider, model);
    if default_effort == "high" && levels.contains(&"low") {
        out.push(Variant { name: "effort-low".into(), effort: "low".into(), ..base.clone() });
    } else if default_effort == "low" && levels.contains(&"high") {
        out.push(Variant { name: "effort-high".into(), effort: "high".into(), ..base.clone() });
    }
    out.push(Variant { name: "prompt-short".into(), prompt: "short".into(), ..base.clone() });
    if provider != "claude-cli" {
        out.push(Variant { name: "rounds-24".into(), rounds: 24, ..base });
    }
    out
}

fn safe(text: &str) -> String {
    text.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '-' }).collect()
}

/// Where the profile of one model lives.
pub fn profile_path(root: &Path, provider: &str, model: &str) -> PathBuf {
    root.join("models").join(format!("{}__{}.json", safe(provider), safe(model)))
}

pub fn save_profile(root: &Path, provider: &str, model: &str, profile: &Value) -> Result<PathBuf, String> {
    let path = profile_path(root, provider, model);
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("cannot make {}: {}", dir.display(), e))?;
    }
    let mut v = profile.clone();
    v["provider"] = json!(provider);
    v["model"] = json!(model);
    fs::write(&path, serde_json::to_string_pretty(&v).unwrap()).map_err(|e| format!("cannot write {}: {}", path.display(), e))?;
    Ok(path)
}

/// The stored profile of a model setting, None when there is none or it does not parse.
pub fn load_profile(root: &Path, model: &Value) -> Option<Variant> {
    let name = model["model"].as_str()?;
    let text = fs::read_to_string(profile_path(root, &provider_of(model), name)).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    Some(Variant::from_json(&v))
}

/// The context of one turn with the profile of the main model applied. Unchanged when no profile exists or the config
/// says `"calibration": "off"`. The profile's choices travel as fields of the main setting: `tool_format`,
/// `tool_list`, `prompt`, and `effort` when the model has levels.
pub fn apply(ctx: &Ctx) -> Ctx {
    let mut run = ctx.clone();
    if ctx.cfg["calibration"].as_str() == Some("off") {
        return run;
    }
    let Some(p) = load_profile(&ctx.root, &ctx.cfg["main"]) else { return run };
    let m = &mut run.cfg["main"];
    let provider = provider_of(m);
    let name = m["model"].as_str().unwrap_or("").to_string();
    if !p.effort.is_empty() && crate::providers::efforts(&provider, &name).contains(&p.effort.as_str()) {
        m["effort"] = json!(p.effort);
    }
    m["tool_format"] = json!(p.tool_format);
    m["tool_list"] = json!(p.tool_list);
    m["prompt"] = json!(p.prompt);
    if p.rounds > 0 {
        m["max_rounds"] = json!(p.rounds);
    }
    run
}

/// The system rules of a turn: the short form when the profile asks for it, else `backend::rules`.
pub fn rules_for(ctx: &Ctx) -> String {
    if ctx.cfg["main"]["prompt"].as_str() != Some("short") {
        return crate::backend::rules(ctx);
    }
    let cwd = std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default();
    format!("You work inside Penelope, an agent harness. The working directory is {}; paths are relative to it.\n\
1. Use run for shell commands.\n\
2. Use tree, find, grep and read to look at files.\n\
3. Read a file before you change it; edit changes a part, write replaces the whole file.\n\
4. After a change, run the check that proves it.\n\
5. Answer in one or two sentences.{}", cwd, crate::checker::rule(ctx))
}

/// The spec of `tool_describe`.
pub fn describe_spec() -> Value {
    json!({"name": DESCRIBE,
           "description": "Returns the full argument schema of the named tools. Call it before you use a tool whose arguments you do not know.",
           "inputSchema": {"type": "object", "properties": {"names": {"type": "array", "items": {"type": "string"}, "description": "tool names from the list"}},
                           "required": ["names"]}})
}

/// The answer of `tool_describe`: the full specs of the named tools, unknown names listed.
pub fn describe(full: &[Value], args: &Value) -> (String, bool) {
    let answer = crate::mitos::describe(full, args);
    let ok = answer["tools"].as_array().map(|a| !a.is_empty()).unwrap_or(false);
    (serde_json::to_string(&json!({"tools": answer["tools"], "unknown": answer["unknown"]})).unwrap_or_default(), ok)
}

/// The tools to send and the system text for one turn, after the profile's tool list and tool format.
pub fn shape_tools(model: &Value, tools: Vec<Value>, system: String) -> (Vec<Value>, String) {
    let tools = if model["tool_list"].as_str() == Some("index") {
        let mut index: Vec<Value> = tools.iter().map(crate::mitos::index_entry).collect();
        index.push(describe_spec());
        index
    } else {
        tools
    };
    if model["tool_format"].as_str() != Some("text") {
        return (tools, system);
    }
    let mut block = String::from("\n\nTool calls go in your answer as text. The API tool field is empty in this session. For each call write one block:\n\
<tool_call>\n{\"name\": \"read\", \"arguments\": {\"path\": \"notes.txt\"}}\n</tool_call>\n\
Several blocks in one answer are allowed. The results come back in <tool_response> blocks in the next message. \
When the task is done, answer without a block.\n\nTools, one JSON object per line:\n");
    for t in &tools {
        block.push_str(&serde_json::to_string(&json!({"name": t["name"], "description": t["description"], "parameters": t["inputSchema"]})).unwrap_or_default());
        block.push('\n');
    }
    (Vec::new(), format!("{}{}", system, block))
}

/// One task of a run: which task, which variant, and the folders.
pub struct TaskRun {
    pub task: crate::eval::Task,
    pub variant: Variant,
    /// The folder of this run: `work/` and `.penelope/` inside.
    pub dir: PathBuf,
    /// The working folder of the model: `work/` plus the task's cwd.
    pub workdir: PathBuf,
    /// The `.penelope` folder of this run.
    pub root: PathBuf,
    pub turn_timeout: Duration,
}

/// What the turn reported.
#[derive(Default, Debug, Clone)]
pub struct Turn {
    pub answer: String,
    pub error: String,
    pub timed_out: bool,
}

/// What one task run measured.
#[derive(Default, Debug, Clone, PartialEq)]
pub struct RunResult {
    pub variant: String,
    pub task: String,
    pub passed: bool,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub eur: f64,
    pub wall_ms: u64,
    pub error: String,
    pub check: String,
}

impl RunResult {
    fn to_json(&self) -> Value {
        json!({"variant": self.variant, "task": self.task, "passed": self.passed, "tokens_in": self.tokens_in, "tokens_out": self.tokens_out,
               "eur": self.eur, "wall_ms": self.wall_ms, "error": self.error, "check": self.check})
    }

    fn from_json(v: &Value) -> RunResult {
        RunResult { variant: v["variant"].as_str().unwrap_or("").into(), task: v["task"].as_str().unwrap_or("").into(),
                    passed: v["passed"].as_bool().unwrap_or(false), tokens_in: v["tokens_in"].as_u64().unwrap_or(0),
                    tokens_out: v["tokens_out"].as_u64().unwrap_or(0), eur: v["eur"].as_f64().unwrap_or(0.0),
                    wall_ms: v["wall_ms"].as_u64().unwrap_or(0), error: v["error"].as_str().unwrap_or("").into(),
                    check: v["check"].as_str().unwrap_or("").into() }
    }
}

/// How a command over its time limit ended: its status, or None after the kill.
pub struct Bounded {
    pub status: Option<std::process::ExitStatus>,
    pub timed_out: bool,
}

/// Runs a command in its own process group with stdout and stderr in `log`. Over `limit`, the whole group is killed:
/// a check that starts a server in the background loses that server too.
pub fn run_bounded(mut cmd: Command, limit: Duration, log: &Path) -> Result<Bounded, String> {
    let out = fs::File::create(log).map_err(|e| format!("cannot write {}: {}", log.display(), e))?;
    let err = out.try_clone().map_err(|e| format!("cannot write {}: {}", log.display(), e))?;
    cmd.stdin(Stdio::null()).stdout(Stdio::from(out)).stderr(Stdio::from(err));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = cmd.spawn().map_err(|e| format!("cannot start the command: {}", e))?;
    let deadline = Instant::now() + limit;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(Bounded { status: Some(status), timed_out: false }),
            Ok(None) if Instant::now() >= deadline => {
                crate::platform::kill_tree(child.id() as u64);
                let _ = child.kill();
                let _ = child.wait();
                return Ok(Bounded { status: None, timed_out: true });
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => return Err(format!("the command failed: {}", e)),
        }
    }
}

/// The task text the model gets, the same wording as `penelope eval`.
pub fn task_text(task: &crate::eval::Task, workdir: &Path) -> String {
    format!("This is an automated evaluation task. The project is at {}. Ignore any other working directory named in \
your system prompt; for this task it is {}. Work only inside that folder. The file and run tools use it as their \
working directory; if you set cwd, use that path. The check command that judges the result is `{}`. Do not start a \
cluster or a subagent of your own; do the task here and then stop.\n\nTask: {}",
            workdir.display(), workdir.display(), task.check, task.prompt)
}

const COPY_SKIP: &[&str] = &[".git", "target", "node_modules", ".venv", "venv", "__pycache__", ".penelope"];

fn copy_tree(src: &Path, dst: &Path) -> Result<(), String> {
    fs::create_dir_all(dst).map_err(|e| format!("cannot make {}: {}", dst.display(), e))?;
    for entry in fs::read_dir(src).map_err(|e| format!("cannot read {}: {}", src.display(), e))?.flatten() {
        let name = entry.file_name();
        if COPY_SKIP.iter().any(|s| name.to_string_lossy() == *s) {
            continue;
        }
        let Ok(kind) = entry.file_type() else { continue };
        let to = dst.join(&name);
        if kind.is_dir() {
            copy_tree(&entry.path(), &to)?;
        } else if kind.is_file() {
            fs::copy(entry.path(), &to).map_err(|e| format!("cannot copy {}: {}", entry.path().display(), e))?;
        }
    }
    Ok(())
}

/// The config of one task run: the caller's settings with the model under test as main and weak model, every tool
/// allowed inside the sandbox, no learning, no scout. Same policy as `penelope eval`.
pub fn run_config(base: &Value, model: &Value) -> Value {
    let mut cfg = base.clone();
    cfg["main"] = model.clone();
    cfg["weak"] = model.clone();
    cfg["roles"] = json!({});
    cfg["route"] = json!("fixed");
    cfg["edit_policy"] = json!("all");
    cfg["run_policy"] = json!("all");
    cfg["sandbox"] = json!(if cfg!(target_os = "linux") && crate::config::which("bwrap").is_some() { "bwrap" } else { "off" });
    cfg["auto_learn"] = json!(false);
    cfg["auto_skills"] = json!(false);
    cfg["delegate"] = json!("off");
    cfg["calibration"] = json!("on");
    cfg
}

/// Makes the folder of one run: the copy of the setup repo, the config and the variant as profile file.
pub fn prepare(base: &Value, model: &Value, out: &Path, task: &crate::eval::Task, variant: &Variant, turn_timeout: Duration) -> Result<TaskRun, String> {
    let dir = out.join(safe(&variant.name)).join(safe(&task.id));
    let _ = fs::remove_dir_all(&dir);
    let root = dir.join(".penelope");
    fs::create_dir_all(root.join("runs")).map_err(|e| format!("cannot make {}: {}", root.display(), e))?;
    fs::write(root.join("config.json"), serde_json::to_string_pretty(&run_config(base, model)).unwrap())
        .map_err(|e| format!("cannot write the run config: {}", e))?;
    let provider = provider_of(model);
    save_profile(&root, &provider, model["model"].as_str().unwrap_or(""), &variant.to_json())?;
    let work = dir.join("work");
    if task.copy {
        copy_tree(&task.repo, &work)?;
    } else {
        return Err(format!("task {} sets copy false; calibration runs only on copies", task.id));
    }
    let workdir = if task.cwd.is_empty() { work } else { work.join(&task.cwd) };
    if !workdir.is_dir() {
        return Err(format!("the copy has no folder {}", workdir.display()));
    }
    let workdir = workdir.canonicalize().unwrap_or(workdir);
    fs::write(root.join("calibrate-prompt.txt"), task_text(task, &workdir)).map_err(|e| format!("cannot write the prompt: {}", e))?;
    Ok(TaskRun { task: task.clone(), variant: variant.clone(), dir, workdir, root, turn_timeout })
}

/// The turn in a child process: `penelope calibrate-one --root ROOT`, working folder and task cwd set to the copy.
pub fn child_turn(run: &TaskRun) -> Turn {
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(e) => return Turn { error: format!("no executable: {}", e), ..Default::default() },
    };
    let mut cmd = Command::new(exe);
    cmd.args(["calibrate-one", "--root"]).arg(&run.root).current_dir(&run.workdir).env("PENELOPE_TASK_CWD", &run.workdir);
    let bounded = match run_bounded(cmd, run.turn_timeout, &run.root.join("turn.log")) {
        Ok(b) => b,
        Err(e) => return Turn { error: e, ..Default::default() },
    };
    let v: Value = fs::read_to_string(run.root.join("turn.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Value::Null);
    let mut turn = Turn { answer: v["answer"].as_str().unwrap_or("").into(), error: v["error"].as_str().unwrap_or("").into(), timed_out: bounded.timed_out };
    if bounded.timed_out {
        turn.error = format!("turn timed out after {} s", run.turn_timeout.as_secs());
    } else if v.is_null() {
        let log = fs::read_to_string(run.root.join("turn.log")).unwrap_or_default();
        turn.error = format!("the turn wrote no result: {}", log.chars().rev().take(300).collect::<String>().chars().rev().collect::<String>());
    }
    turn
}

/// The body of `penelope calibrate-one`: one turn on the run's root, the answer in `turn.json`.
pub fn one(ctx: &Ctx) -> Result<(), String> {
    let prompt = fs::read_to_string(ctx.root.join("calibrate-prompt.txt")).map_err(|e| format!("no prompt in {}: {}", ctx.root.display(), e))?;
    let mut chat = crate::backend::Chat::new();
    let out = chat.turn_with(ctx, &prompt, "", &mut |_| {});
    let v = match out {
        Ok(answer) => json!({"answer": answer.chars().take(4000).collect::<String>(), "error": ""}),
        Err(e) => json!({"answer": "", "error": e}),
    };
    fs::write(ctx.root.join("turn.json"), v.to_string()).map_err(|e| format!("cannot write turn.json: {}", e))
}

/// Tokens and euro of one run from its `usage.json`. A model without a price adds tokens and no euro.
pub fn usage_of(root: &Path, prices: &Ctx) -> (u64, u64, f64) {
    let v: Value = fs::read_to_string(root.join("usage.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Value::Null);
    let mut total = (0u64, 0u64, 0.0f64);
    if let Some(models) = v["models"].as_object() {
        for e in models.values() {
            let (i, o) = (e["in"].as_u64().unwrap_or(0), e["out"].as_u64().unwrap_or(0));
            total.0 += i;
            total.1 += o;
            total.2 += crate::cost::euro_for(prices, e["model"].as_str().unwrap_or(""), i, o).unwrap_or(0.0);
        }
    }
    total
}

/// Runs the check in the work folder, bounded by the task's timeout.
pub fn check(run: &TaskRun) -> (bool, String) {
    let log = run.dir.join("check.log");
    let mut cmd = crate::platform::shell(&run.task.check);
    cmd.current_dir(&run.workdir);
    match run_bounded(cmd, Duration::from_secs(run.task.check_timeout_seconds), &log) {
        Ok(b) => {
            let text: String = fs::read_to_string(&log).unwrap_or_default().trim().chars().take(600).collect();
            if b.timed_out {
                (false, format!("check timed out after {} s {}", run.task.check_timeout_seconds, text))
            } else {
                (b.status.map(|s| s.success()).unwrap_or(false), text)
            }
        }
        Err(e) => (false, e),
    }
}

/// How often a run that ended on a rate limit (status 429) is started again in all.
pub const RATE_LIMIT_TRIES: u32 = 3;

/// True when a turn ended on a rate limit of the provider.
pub fn rate_limited(error: &str) -> bool {
    error.contains("status 429")
}

/// The wait until a usage window resets, from a provider text such as "usage limit reached (5-hour, resets in 34m)"
/// or "resets in 2h 5m", plus one minute. None for a plain rate limit.
pub fn reset_wait(error: &str) -> Option<Duration> {
    if !rate_limited(error) {
        return None;
    }
    let caps = regex::Regex::new(r"resets in (?:(\d+)h)?\s*(?:(\d+)m)?").unwrap().captures(error)?;
    let h: u64 = caps.get(1).and_then(|m| m.as_str().parse().ok()).unwrap_or(0);
    let m: u64 = caps.get(2).and_then(|m| m.as_str().parse().ok()).unwrap_or(0);
    if h == 0 && m == 0 {
        return None;
    }
    Some(Duration::from_secs((h * 60 + m + 1) * 60))
}

/// What a set of runs needs besides the tasks.
pub struct Plan<'a> {
    pub base: &'a Value,
    pub model: &'a Value,
    pub prices: &'a Ctx,
    pub out: &'a Path,
    pub jobs: usize,
    pub turn_timeout: Duration,
    /// The pause before a run that hit a rate limit starts again.
    pub rate_limit_pause: Duration,
}

/// Runs every task in every variant, at most `jobs` at once, through `exec` (the child process, or a fake model in
/// tests). Results already in `out/results.jsonl` are kept and not run again.
pub fn run_all(plan: &Plan, tasks: &[crate::eval::Task], variants: &[Variant], exec: &(dyn Fn(&TaskRun) -> Turn + Sync),
               on_done: &(dyn Fn(&RunResult) + Sync)) -> Result<Vec<RunResult>, String> {
    fs::create_dir_all(plan.out).map_err(|e| format!("cannot make {}: {}", plan.out.display(), e))?;
    let file = plan.out.join("results.jsonl");
    let done: Vec<RunResult> = fs::read_to_string(&file).unwrap_or_default().lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok()).map(|v| RunResult::from_json(&v)).collect();
    let mut queue: Vec<(&crate::eval::Task, &Variant)> = Vec::new();
    // Task-major order: the variants of one task run close in time, so a slow hour of a provider hits all of them.
    for task in tasks {
        for v in variants {
            if !done.iter().any(|r| r.task == task.id && r.variant == v.name) {
                queue.push((task, v));
            }
        }
    }
    let next = AtomicUsize::new(0);
    let results = Mutex::new(done);
    let writer = Mutex::new(());
    let jobs = plan.jobs.clamp(1, MAX_JOBS);
    std::thread::scope(|s| {
        for _ in 0..jobs {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::SeqCst);
                let Some((task, variant)) = queue.get(i) else { break };
                let (mut attempt, mut waits) = (0u32, 0u32);
                let result = loop {
                    attempt += 1;
                    let start = Instant::now();
                    let result = match prepare(plan.base, plan.model, plan.out, task, variant, plan.turn_timeout) {
                        Ok(run) => {
                            let turn = exec(&run);
                            let (passed, text) = check(&run);
                            let (tin, tout, eur) = usage_of(&run.root, plan.prices);
                            RunResult { variant: variant.name.clone(), task: task.id.clone(), passed, tokens_in: tin, tokens_out: tout, eur,
                                        wall_ms: start.elapsed().as_millis() as u64, error: turn.error.chars().take(400).collect(), check: text }
                        }
                        Err(e) => RunResult { variant: variant.name.clone(), task: task.id.clone(), error: e, wall_ms: start.elapsed().as_millis() as u64, ..Default::default() },
                    };
                    // A rate limit of the provider is no result of the variant: the run starts again from a fresh copy.
                    // A usage window that names its reset ("resets in 34m") is waited out, up to 5 hours, 3 times at most.
                    if let Some(wait) = reset_wait(&result.error).filter(|_| waits < RATE_LIMIT_TRIES) {
                        waits += 1;
                        attempt -= 1;
                        std::thread::sleep(wait.min(plan.rate_limit_pause * 300));
                        continue;
                    }
                    if !rate_limited(&result.error) || attempt >= RATE_LIMIT_TRIES {
                        break result;
                    }
                    std::thread::sleep(plan.rate_limit_pause);
                };
                {
                    let _g = writer.lock().unwrap_or_else(|e| e.into_inner());
                    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(&file) {
                        use std::io::Write;
                        let _ = writeln!(f, "{}", result.to_json());
                    }
                }
                on_done(&result);
                results.lock().unwrap_or_else(|e| e.into_inner()).push(result);
            });
        }
    });
    let all = results.into_inner().unwrap_or_else(|e| e.into_inner());
    let keep: Vec<RunResult> = all.into_iter().filter(|r| tasks.iter().any(|t| t.id == r.task) && variants.iter().any(|v| v.name == r.variant)).collect();
    Ok(keep)
}

/// The sums of one variant.
#[derive(Default, Debug, Clone, PartialEq)]
pub struct Score {
    pub variant: String,
    pub tasks: usize,
    pub passed: usize,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub eur: f64,
    pub wall_ms: u64,
}

pub fn score(results: &[RunResult], variant: &str) -> Score {
    let mut s = Score { variant: variant.to_string(), ..Default::default() };
    for r in results.iter().filter(|r| r.variant == variant) {
        s.tasks += 1;
        s.passed += r.passed as usize;
        s.tokens_in += r.tokens_in;
        s.tokens_out += r.tokens_out;
        s.eur += r.eur;
        s.wall_ms += r.wall_ms;
    }
    s
}

/// The index of the winning score; index 0 is the default. A variant wins with more passed checks than the default.
/// At equal passes it wins only with at most 80 % of the default's tokens; of several such, the most passes, then
/// the fewest tokens. Everything else keeps the default, so noise of one task does not move a profile.
pub fn select(scores: &[Score]) -> usize {
    let Some(base) = scores.first() else { return 0 };
    let tokens = |s: &Score| s.tokens_in + s.tokens_out;
    let mut best = 0usize;
    for (i, s) in scores.iter().enumerate().skip(1) {
        let qualifies = s.passed > base.passed || (s.passed == base.passed && (tokens(s) as f64) <= 0.8 * tokens(base) as f64);
        if !qualifies {
            continue;
        }
        let b = &scores[best];
        if best == 0 || s.passed > b.passed || (s.passed == b.passed && tokens(s) < tokens(b)) {
            best = i;
        }
    }
    best
}

/// Exact two-sided McNemar test: b and c are the two discordant counts. 1.0 when both are 0.
pub fn mcnemar(b: usize, c: usize) -> f64 {
    let n = b + c;
    if n == 0 {
        return 1.0;
    }
    let k = b.min(c);
    let mut sum = 0.0f64;
    let mut coef = 1.0f64;               // C(n, 0)
    for i in 0..=k {
        if i > 0 {
            coef = coef * (n - i + 1) as f64 / i as f64;
        }
        sum += coef;
    }
    (2.0 * sum / 2f64.powi(n as i32)).min(1.0)
}

/// Paired counts of two variants over the same tasks: (b, c) with b = only `new` passed, c = only `old` passed.
pub fn paired(results: &[RunResult], old: &str, new: &str) -> (usize, usize) {
    let (mut b, mut c) = (0, 0);
    for r in results.iter().filter(|r| r.variant == old) {
        if let Some(n) = results.iter().find(|x| x.variant == new && x.task == r.task) {
            match (r.passed, n.passed) {
                (false, true) => b += 1,
                (true, false) => c += 1,
                _ => {}
            }
        }
    }
    (b, c)
}

fn table(scores: &[Score], chosen: Option<usize>) -> String {
    let mut out = format!("{:<14} {:>7} {:>11} {:>10} {:>9} {:>8}\n", "variant", "passed", "tokens in", "tokens out", "EUR", "wall");
    for (i, s) in scores.iter().enumerate() {
        out.push_str(&format!("{:<14} {:>3}/{:<3} {:>11} {:>10} {:>9.4} {:>7.0}s{}\n", s.variant, s.passed, s.tasks, s.tokens_in, s.tokens_out, s.eur,
                              s.wall_ms as f64 / 1000.0, if chosen == Some(i) { "  <- chosen" } else { "" }));
    }
    out
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// The options of `penelope calibrate`.
#[derive(Debug, Default)]
pub struct Options {
    pub model: String,
    pub provider: String,
    pub tasks: String,
    pub n: usize,
    pub jobs: usize,
    pub out: String,
    pub validate: bool,
    pub only: Vec<String>,
    pub turn_timeout: u64,
}

pub fn parse_args(args: &[String]) -> Result<Options, String> {
    let mut o = Options { n: 20, jobs: MAX_JOBS, turn_timeout: TURN_TIMEOUT_SECS, ..Default::default() };
    let mut i = 0;
    while i < args.len() {
        let val = |i: usize| args.get(i + 1).cloned().ok_or_else(|| format!("{} needs a value", args[i]));
        match args[i].as_str() {
            "--provider" => { o.provider = val(i)?; i += 2; }
            "--tasks" => { o.tasks = val(i)?; i += 2; }
            "--n" => { o.n = val(i)?.parse().map_err(|_| "--n takes a number".to_string())?; i += 2; }
            "--jobs" => { o.jobs = val(i)?.parse().map_err(|_| "--jobs takes a number".to_string())?; i += 2; }
            "--out" => { o.out = val(i)?; i += 2; }
            "--variants" => { o.only = val(i)?.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(); i += 2; }
            "--turn-timeout" => { o.turn_timeout = val(i)?.parse().map_err(|_| "--turn-timeout takes seconds".to_string())?; i += 2; }
            "--validate" => { o.validate = true; i += 1; }
            other if other.starts_with("--") => return Err(format!("unknown option {}", other)),
            other => { o.model = other.to_string(); i += 1; }
        }
    }
    if o.model.is_empty() {
        return Err("name the model: penelope calibrate MODEL [--provider P] [--tasks FILE] [--n N]".into());
    }
    Ok(o)
}

/// `penelope calibrate`: calibrate (default) or validate the stored profile against the default (`--validate`).
pub fn command(ctx: &Ctx, o: &Options) -> Result<String, String> {
    let provider = if o.provider.is_empty() { provider_of(&ctx.cfg["main"]) } else { o.provider.clone() };
    let p = crate::providers::get(&provider).ok_or_else(|| format!("no provider named {} (penelope providers)", provider))?;
    let model = crate::providers::setting(p, Some(&o.model), None, None)?;
    let tasks_path = if o.tasks.is_empty() { "tests/data/eval/tasks50.jsonl".to_string() } else { o.tasks.clone() };
    let mut tasks = crate::eval::parse_tasks(Path::new(&tasks_path))?;
    tasks.truncate(o.n.max(1));
    let sha = crate::brief::sha256_hex(&fs::read(&tasks_path).unwrap_or_default());
    let out = if o.out.is_empty() { ctx.root.join("calibrate") } else { PathBuf::from(&o.out) };
    let tag = format!("{}__{}", safe(&provider), safe(&o.model));
    let phase = if o.validate { "validate" } else { "calibrate" };
    let file_tag = safe(Path::new(&tasks_path).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default().as_str());
    let out = out.join(&tag).join(format!("{}-{}", phase, file_tag));
    let mut variants = if o.validate {
        let stored = load_profile(&ctx.root, &model).ok_or_else(|| format!("no profile for {} yet; run penelope calibrate {} first", o.model, o.model))?;
        let base = Variant::default_for(crate::providers::default_effort(&provider, &o.model));
        vec![base, Variant { name: "calibrated".into(), ..stored }]
    } else {
        variants(&provider, &o.model)
    };
    if !o.only.is_empty() {
        variants.retain(|v| o.only.contains(&v.name));
    }
    if variants.is_empty() {
        return Err("no variant left after --variants".into());
    }
    let plan = Plan { base: &ctx.cfg, model: &model, prices: ctx, out: &out, jobs: o.jobs, turn_timeout: Duration::from_secs(o.turn_timeout),
                      rate_limit_pause: Duration::from_secs(60) };
    let count = Mutex::new(0usize);
    let total = tasks.len() * variants.len();
    let results = run_all(&plan, &tasks, &variants, &child_turn, &|r| {
        let mut c = count.lock().unwrap_or_else(|e| e.into_inner());
        *c += 1;
        eprintln!("[{}/{}] {} {} {} {} in/{} out {:.0}s{}", c, total, r.variant, r.task, if r.passed { "pass" } else { "FAIL" }, r.tokens_in, r.tokens_out,
                  r.wall_ms as f64 / 1000.0, if r.error.is_empty() { String::new() } else { format!(" error: {}", r.error.chars().take(120).collect::<String>()) });
    })?;
    let scores: Vec<Score> = variants.iter().map(|v| score(&results, &v.name)).collect();
    let mut text = format!("{} {} on {} ({} tasks, sha256 {})\n", phase, o.model, tasks_path, tasks.len(), &sha[..16]);
    if o.validate {
        let (b, c) = paired(&results, "default", "calibrated");
        text.push_str(&table(&scores, None));
        text.push_str(&format!("paired: b = {} (only calibrated passed), c = {} (only default passed), exact McNemar p = {:.4}\n", b, c, mcnemar(b, c)));
        text.push_str(&format!("calibrated variant: {}\n", variants[1].to_json()));
        return Ok(text);
    }
    let chosen = select(&scores);
    text.push_str(&table(&scores, Some(chosen)));
    if o.only.is_empty() || o.only.len() > 1 {
        let profile = json!({
            "schema": 1, "variant": variants[chosen].name, "tool_format": variants[chosen].tool_format, "tool_list": variants[chosen].tool_list,
            "effort": variants[chosen].effort, "prompt": variants[chosen].prompt,
            "calibrated": crate::sessions::stamp(now()), "tasks": tasks_path, "tasks_sha256": sha, "n": tasks.len(),
            "scores": scores.iter().map(|s| json!({"variant": s.variant, "passed": s.passed, "tasks": s.tasks, "tokens_in": s.tokens_in,
                                                   "tokens_out": s.tokens_out, "eur": s.eur, "wall_ms": s.wall_ms})).collect::<Vec<_>>()
        });
        let path = save_profile(&ctx.root, &provider, &o.model, &profile)?;
        text.push_str(&format!("profile: {}\n", path.display()));
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let base = std::env::var("PENELOPE_CALIBRATE_TEST_DIR").map(PathBuf::from).unwrap_or_else(|_| std::env::temp_dir());
        let dir = base.join(format!("penelope-calibrate-{}-{}-{}", name, std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn task(dir: &Path, id: &str, check: &str, timeout: u64) -> crate::eval::Task {
        crate::eval::Task { id: id.into(), prompt: format!("write {}", id), repo: dir.to_path_buf(), cwd: String::new(), copy: true,
                            check: check.into(), check_timeout_seconds: timeout, max_steps: 6 }
    }

    #[test]
    fn variants_change_one_factor_each() {
        let v = variants("opencode", "glm-5.3");
        let names: Vec<&str> = v.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(names, ["default", "text", "index", "effort-low", "prompt-short", "rounds-24"]);
        assert_eq!(v[0].effort, "high");
        for x in &v[1..] {
            let diffs = [x.tool_format != v[0].tool_format, x.tool_list != v[0].tool_list, x.effort != v[0].effort, x.prompt != v[0].prompt, x.rounds != v[0].rounds];
            assert_eq!(diffs.iter().filter(|d| **d).count(), 1, "{} changes one factor", x.name);
        }
        // Kimi K3 has only "max", MiniMax M3 none: no effort variant. The claude command owns its tool loop.
        assert!(!variants("opencode", "kimi-k3").iter().any(|x| x.name.starts_with("effort")));
        assert!(!variants("opencode", "minimax-m3").iter().any(|x| x.name.starts_with("effort")));
        let claude: Vec<String> = variants("claude-cli", "haiku").into_iter().map(|x| x.name).collect();
        assert_eq!(claude, ["default", "effort-low", "prompt-short"]);
    }

    #[test]
    fn a_saved_profile_loads_and_changes_the_main_setting() {
        let dir = temp("profile");
        let model = json!({"kind": "openai", "base_url": "https://opencode.ai/zen/go/v1", "model": "glm-5.3", "effort": "high"});
        let ctx = Ctx { root: dir.clone(), cfg: json!({"main": model.clone()}) };
        assert_eq!(apply(&ctx).cfg["main"], model);           // no profile: nothing changes
        let v = Variant { name: "effort-low".into(), effort: "low".into(), tool_format: "text".into(), ..Variant::default_for("high") };
        let path = save_profile(&dir, "opencode", "glm-5.3", &v.to_json()).unwrap();
        assert_eq!(path, dir.join("models").join("opencode__glm-5.3.json"));
        assert_eq!(load_profile(&dir, &model), Some(v));
        let run = apply(&ctx);
        assert_eq!(run.cfg["main"]["effort"], "low");
        assert_eq!(run.cfg["main"]["tool_format"], "text");
        assert_eq!(run.cfg["main"]["tool_list"], "full");
        assert!(run.cfg["main"]["max_rounds"].is_null());
        let rounds = Variant { name: "rounds-24".into(), rounds: 24, ..Variant::default_for("high") };
        save_profile(&dir, "opencode", "glm-5.3", &rounds.to_json()).unwrap();
        assert_eq!(apply(&ctx).cfg["main"]["max_rounds"], 24);
        // A level the model does not have is not sent; the switch turns the profile off.
        save_profile(&dir, "opencode", "glm-5.3", &json!({"variant": "x", "effort": "xhigh"})).unwrap();
        assert_eq!(apply(&ctx).cfg["main"]["effort"], "high");
        let off = Ctx { root: dir.clone(), cfg: json!({"main": model.clone(), "calibration": "off"}) };
        assert!(apply(&off).cfg["main"]["tool_format"].is_null());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn text_format_moves_the_tools_into_the_prompt_and_index_adds_describe() {
        let specs = crate::spec::list();
        let n = specs.len();
        let (tools, system) = shape_tools(&json!({"tool_format": "native", "tool_list": "full"}), specs.clone(), "rules".into());
        assert_eq!((tools.len(), system.as_str()), (n, "rules"));
        let (tools, system) = shape_tools(&json!({"tool_format": "text"}), specs.clone(), "rules".into());
        assert!(tools.is_empty() && system.starts_with("rules") && system.contains("<tool_call>") && system.contains("\"name\":\"read\""));
        let (tools, _) = shape_tools(&json!({"tool_list": "index"}), specs.clone(), "rules".into());
        assert_eq!(tools.len(), n + 1);
        assert!(tools.iter().filter(|t| t["name"] != DESCRIBE).all(|t| t["inputSchema"] == json!({"type": "object"})));
        let (text, ok) = describe(&specs, &json!({"names": ["read", "nope"]}));
        assert!(ok && text.contains("\"unknown\":[\"nope\"]") && text.contains("properties"));
    }

    #[test]
    fn select_keeps_the_default_unless_a_variant_is_better() {
        let s = |name: &str, passed: usize, tokens: u64| Score { variant: name.into(), tasks: 20, passed, tokens_in: tokens, ..Default::default() };
        assert_eq!(select(&[s("default", 15, 1000), s("text", 15, 900), s("index", 14, 100)]), 0);   // 90 % of the tokens is not enough
        assert_eq!(select(&[s("default", 15, 1000), s("text", 16, 2000), s("index", 15, 500)]), 1);   // more passes beat fewer tokens
        assert_eq!(select(&[s("default", 15, 1000), s("text", 15, 800), s("index", 15, 500)]), 2);
        assert_eq!(select(&[s("default", 15, 1000), s("text", 17, 900), s("index", 17, 800)]), 2);
        assert_eq!(select(&[]), 0);
    }

    #[test]
    fn a_usage_window_is_waited_out_and_a_plain_limit_is_not() {
        let e = "https://opencode.ai/zen/go/v1/chat/completions answered with status 429: OpenCode Go usage limit reached (5-hour, resets in 34m).";
        assert_eq!(reset_wait(e), Some(Duration::from_secs(35 * 60)));
        assert_eq!(reset_wait("status 429: usage limit reached (resets in 2h 5m)"), Some(Duration::from_secs(126 * 60)));
        assert_eq!(reset_wait("status 429: Rate limit exceeded. Please try again later."), None);
        assert_eq!(reset_wait("resets in 34m"), None);            // no 429: not a rate limit
    }

    #[test]
    fn mcnemar_matches_the_binomial_table() {
        assert_eq!(mcnemar(0, 0), 1.0);
        assert!((mcnemar(3, 0) - 0.25).abs() < 1e-12);
        assert!((mcnemar(0, 5) - 0.0625).abs() < 1e-12);
        assert!((mcnemar(6, 1) - 0.125).abs() < 1e-12);
        assert!((mcnemar(10, 2) - 0.0385742).abs() < 1e-6);
        assert_eq!(mcnemar(2, 2), 1.0);
    }

    #[test]
    fn the_runner_checks_counts_and_resumes_with_a_fake_model() {
        let dir = temp("runner");
        let repo = dir.join("repo");
        fs::create_dir_all(&repo).unwrap();
        fs::write(repo.join("seed.txt"), "x").unwrap();
        let check = if cfg!(windows) { "type out.txt | findstr ok" } else { "grep -qx ok out.txt" };
        let tasks = vec![task(&repo, "t1", check, 30), task(&repo, "t2", check, 30)];
        let model = json!({"kind": "openai", "base_url": "https://opencode.ai/zen/go/v1", "model": "glm-5.3", "effort": "high"});
        let prices = Ctx { root: dir.clone(), cfg: json!({"prices": {"glm-5.3": [1.0, 2.0]}, "eur_per_usd": 1.0}) };
        let out = dir.join("out");
        let plan = Plan { base: &json!({}), model: &model, prices: &prices, out: &out, jobs: 3, turn_timeout: Duration::from_secs(5),
                          rate_limit_pause: Duration::from_millis(10) };
        let vars = variants("opencode", "glm-5.3")[..2].to_vec();
        let calls = AtomicUsize::new(0);
        // The fake model: "text" writes the right answer, "default" solves t1 only. It records 1000 in / 100 out.
        let fake = |run: &TaskRun| {
            calls.fetch_add(1, Ordering::SeqCst);
            assert!(fs::read_to_string(run.root.join("calibrate-prompt.txt")).unwrap().contains(&run.task.prompt));
            let stored = load_profile(&run.root, &json!({"kind": "openai", "base_url": "https://opencode.ai/zen/go/v1", "model": "glm-5.3"})).unwrap();
            assert_eq!(stored, run.variant);
            let good = run.variant.name == "text" || run.task.id == "t1";
            fs::write(run.workdir.join("out.txt"), if good { "ok\n" } else { "no\n" }).unwrap();
            let ctx = crate::config::load(&run.root);
            crate::cost::record(&ctx, "lead", "glm-5.3", 1000, 100);
            Turn::default()
        };
        let results = run_all(&plan, &tasks, &vars, &fake, &|_| {}).unwrap();
        assert_eq!(results.len(), 4);
        let scores: Vec<Score> = vars.iter().map(|v| score(&results, &v.name)).collect();
        assert_eq!((scores[0].passed, scores[1].passed), (1, 2));
        assert_eq!((scores[0].tokens_in, scores[0].tokens_out), (2000, 200));
        assert!((scores[0].eur - 0.0024).abs() < 1e-9);
        assert_eq!(select(&scores), 1);
        assert_eq!(paired(&results, "default", "text"), (1, 0));
        // A second call finds every result on disk and runs nothing.
        let limited = AtomicUsize::new(0);
        let flaky = |run: &TaskRun| {
            if limited.fetch_add(1, Ordering::SeqCst) == 0 {
                return Turn { error: "https://opencode.ai/zen/go/v1/chat/completions answered with status 429: Rate limit exceeded.".into(), ..Default::default() };
            }
            fs::write(run.workdir.join("out.txt"), "ok\n").unwrap();
            Turn::default()
        };
        let out2 = dir.join("out2");
        let plan2 = Plan { out: &out2, jobs: 1, ..Plan { ..plan_copy(&plan) } };
        let retried = run_all(&plan2, &tasks[..1], &vars[..1], &flaky, &|_| {}).unwrap();
        assert_eq!((retried.len(), retried[0].passed, retried[0].error.as_str(), limited.load(Ordering::SeqCst)), (1, true, "", 2));
        let again = run_all(&plan, &tasks, &vars, &fake, &|_| {}).unwrap();
        assert_eq!((again.len(), calls.load(Ordering::SeqCst)), (4, 4));
        let _ = fs::remove_dir_all(&dir);
    }

    fn plan_copy<'a>(p: &Plan<'a>) -> Plan<'a> {
        Plan { base: p.base, model: p.model, prices: p.prices, out: p.out, jobs: p.jobs, turn_timeout: p.turn_timeout, rate_limit_pause: p.rate_limit_pause }
    }

    #[cfg(unix)]
    #[test]
    fn a_check_over_its_timeout_loses_its_whole_process_group() {
        let dir = temp("timeout");
        let repo = dir.join("repo");
        fs::create_dir_all(&repo).unwrap();
        // The check starts a grandchild that writes its pid and would live for 30 s.
        let t = task(&repo, "slow", "sh -c 'echo $$ > grandchild.pid; sleep 30' & sleep 30", 1);
        let model = json!({"kind": "claude-cli", "model": "haiku"});
        let run = prepare(&json!({}), &model, &dir.join("out"), &t, &Variant::default_for("high"), Duration::from_secs(5)).unwrap();
        let start = Instant::now();
        let (passed, text) = check(&run);
        assert!(!passed && text.starts_with("check timed out after 1 s"), "{}", text);
        assert!(start.elapsed() < Duration::from_secs(10));
        let pid = fs::read_to_string(run.workdir.join("grandchild.pid")).unwrap().trim().to_string();
        std::thread::sleep(Duration::from_millis(300));
        let alive = Command::new("kill").args(["-0", &pid]).stderr(Stdio::null()).status().map(|s| s.success()).unwrap_or(false);
        assert!(!alive, "grandchild {} survived the timeout", pid);
        let _ = fs::remove_dir_all(&dir);
    }

    /// A folder with a note, an openai-shaped fake model and the given profile stored for it.
    fn endpoint(name: &str, address: &str, variant: &Variant) -> (Ctx, PathBuf) {
        let dir = temp(name);
        fs::write(dir.join("note.txt"), "hello from the note\n").unwrap();
        let mut ctx = crate::config::load(&dir.join(".penelope"));
        ctx.cfg["main"] = json!({"kind": "openai", "base_url": format!("{}/v1", address), "model": "test-model", "api_key_env": ""});
        ctx.cfg["auto_learn"] = json!(false);
        ctx.cfg["delegate"] = json!("off");
        save_profile(&ctx.root, "openai", "test-model", &variant.to_json()).unwrap();
        (ctx, dir)
    }

    fn reply(content: &str) -> (u16, String) {
        (200, json!({"choices": [{"finish_reason": "stop", "message": {"role": "assistant", "content": content}}],
                     "usage": {"prompt_tokens": 10, "completion_tokens": 2}}).to_string())
    }

    #[test]
    fn a_text_profile_sends_no_tool_field_and_answers_tool_calls_in_text() {
        let call = |dir: &Path| format!("<tool_call>\n{}\n</tool_call>", json!({"name": "read", "arguments": {"path": "note.txt", "cwd": dir.to_string_lossy()}}));
        let text = Variant { name: "text".into(), tool_format: "text".into(), ..Variant::default_for("") };
        let (mut ctx, dir) = endpoint("text", "http://127.0.0.1:9", &text);
        let (address, seen) = crate::llm::fake::serve(vec![reply(&call(&dir)), reply("The note says hello.")]);
        ctx.cfg["main"]["base_url"] = json!(format!("{}/v1", address));
        let answer = crate::backend::Chat::new().turn_with(&ctx, "What does note.txt say?", "", &mut |_| {}).unwrap();
        assert_eq!(answer, "The note says hello.");
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 2);
        assert!(seen[0].body["tools"].is_null(), "no API tool field in the text format");
        assert!(seen[0].body["messages"][0]["content"].as_str().unwrap().contains("<tool_call>"));
        let last = seen[1].body["messages"].as_array().unwrap().last().unwrap().clone();
        assert_eq!(last["role"], "user");
        let content = last["content"].as_str().unwrap();
        assert!(content.starts_with("<tool_response id=\"text-call-1\">") && content.contains("hello from the note"), "{}", content);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_index_profile_sends_lazy_schemas_and_answers_describe() {
        let describe_call = json!({"choices": [{"finish_reason": "tool_calls", "message": {"role": "assistant", "content": null, "tool_calls": [
            {"id": "c1", "type": "function", "function": {"name": DESCRIBE, "arguments": json!({"names": ["read"]}).to_string()}}]}}],
            "usage": {"prompt_tokens": 10, "completion_tokens": 2}}).to_string();
        let (address, seen) = crate::llm::fake::serve(vec![(200, describe_call), reply("done")]);
        let index = Variant { name: "index".into(), tool_list: "index".into(), ..Variant::default_for("") };
        let (ctx, dir) = endpoint("index", &address, &index);
        assert_eq!(crate::backend::Chat::new().turn_with(&ctx, "Look at the read tool.", "", &mut |_| {}).unwrap(), "done");
        let seen = seen.lock().unwrap();
        let tools = seen[0].body["tools"].as_array().unwrap();
        assert!(tools.iter().any(|t| t["function"]["name"] == DESCRIBE));
        let read = tools.iter().find(|t| t["function"]["name"] == "read").unwrap();
        assert_eq!(read["function"]["parameters"], json!({"type": "object"}));
        let result = seen[1].body["messages"].as_array().unwrap().last().unwrap().clone();
        assert_eq!(result["role"], "tool");
        assert!(result["content"].as_str().unwrap().contains("\"properties\""));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn options_parse() {
        let a: Vec<String> = ["glm-5.3", "--provider", "opencode", "--n", "10", "--variants", "default,text", "--validate"].iter().map(|s| s.to_string()).collect();
        let o = parse_args(&a).unwrap();
        assert_eq!((o.model.as_str(), o.provider.as_str(), o.n, o.validate), ("glm-5.3", "opencode", 10, true));
        assert_eq!(o.only, ["default", "text"]);
        assert!(parse_args(&["--n".to_string()]).is_err());
        assert!(parse_args(&[]).is_err());
    }
}
