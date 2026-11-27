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
//! - `prompt-short`: a 6-line system prompt in place of the 11 rules of `backend::rules`; rule 2 (where lookups go)
//!   is the same line as in the full rules.
//! - `rounds-24`: 24 tool-call rounds per turn in place of the task's `max_steps`.
//!
//! The claude command runs its own tool loop, so a `claude-cli` model gets only the effort and prompt variants, and
//! its turns have no round cap.
//!
//! `penelope calibrate MODEL` writes the winner to `.penelope/models/<provider>__<model>.json` when the run held the
//! default and every variant that applies to the model. `backend.rs` reads that file before every turn on the model
//! (`apply`) and uses its tool format, tool list, effort, prompt and round cap.
//!
//! Every task run gets its own folder under the output folder: `work/` holds the copy of the setup repo, `.penelope/`
//! holds the config, the variant as profile file, the usage and the message history of that run. The turn runs in a
//! child process (`penelope calibrate-one`) in its own process group. A turn or a check over its time limit is killed
//! with every process below it, also those in a process group of their own (the claude command and its MCP servers).
//! The check runs in bubblewrap: read-only root, only the copy writable, no network. Without bubblewrap the command
//! refuses to start unless `--no-sandbox` is given. At most 3 runs go in parallel, and a lock on the output folder
//! keeps a second calibrate process out. A finished run is one line in `results.jsonl`; a second call skips the runs
//! it finds there. A run that ended on a rate limit or a provider failure writes no line, so the next call runs it again.

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
/// `tool_list`, `prompt`, `max_rounds`, and `effort` when the model has that level. The effort is set only while the
/// main setting holds no effort or the provider's default: an effort that auto routing, a work mode or the user chose
/// stays.
pub fn apply(ctx: &Ctx) -> Ctx {
    let mut run = ctx.clone();
    if ctx.cfg["calibration"].as_str() == Some("off") {
        return run;
    }
    let Some(p) = load_profile(&ctx.root, &ctx.cfg["main"]) else { return run };
    let m = &mut run.cfg["main"];
    let provider = provider_of(m);
    let name = m["model"].as_str().unwrap_or("").to_string();
    let current = m["effort"].as_str().unwrap_or("").to_string();
    let untouched = current.is_empty() || current == crate::providers::default_effort(&provider, &name);
    if untouched && !p.effort.is_empty() && crate::providers::efforts(&provider, &name).contains(&p.effort.as_str()) {
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
{}\n\
3. Read a file before you change it; edit changes a part, write replaces the whole file.\n\
4. After a change, run the check that proves it.\n\
5. Answer in one or two sentences.{}", cwd, crate::backend::look_rule(ctx), crate::checker::rule(ctx))
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

/// A text tool call with its values typed after the tool's schema: a text value becomes a number, bool, array or
/// object where the schema asks for one and the text parses as that. Values the schema types as text stay text.
pub fn typed_call(full: &[Value], call: &crate::llm::ToolCall) -> crate::llm::ToolCall {
    let mut args = call.args.clone();
    let props = full.iter().find(|t| t["name"] == call.name.as_str()).map(|t| t["inputSchema"]["properties"].clone()).unwrap_or(Value::Null);
    if let (Some(obj), Some(props)) = (args.as_object_mut(), props.as_object()) {
        for (key, value) in obj.iter_mut() {
            let (Some(text), Some(kind)) = (value.as_str(), props.get(key).and_then(|p| p["type"].as_str())) else { continue };
            let Ok(parsed) = serde_json::from_str::<Value>(text.trim()) else { continue };
            let fits = match kind {
                "integer" => parsed.is_i64() || parsed.is_u64(),
                "number" => parsed.is_number(),
                "boolean" => parsed.is_boolean(),
                "array" => parsed.is_array(),
                "object" => parsed.is_object(),
                _ => false,
            };
            if fits {
                *value = parsed;
            }
        }
    }
    crate::llm::ToolCall { id: call.id.clone(), name: call.name.clone(), args }
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
    /// True when the check runs in bubblewrap (the run config says `"sandbox": "bwrap"`).
    pub sandbox: bool,
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
                kill_descendants(child.id());
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

/// Kills every process below `pid`, also those that started a process group of their own: the claude command and
/// its MCP servers, and the background jobs of `jobs.rs`. The tree is read first and killed after, so a child whose
/// parent died first is still found. Linux reads /proc, macOS asks pgrep; Windows kills the tree with taskkill /T.
pub fn kill_descendants(pid: u32) {
    #[cfg(unix)]
    {
        let mut all = Vec::new();
        let mut open = vec![pid];
        while let Some(p) = open.pop() {
            for c in children_of(p) {
                if !all.contains(&c) && c != pid {
                    all.push(c);
                    open.push(c);
                }
            }
        }
        for c in &all {
            let _ = Command::new("kill").args(["-KILL", &c.to_string()]).stderr(Stdio::null()).status();
        }
        for c in &all {
            let _ = Command::new("kill").args(["-KILL", "--", &format!("-{}", c)]).stderr(Stdio::null()).status();
        }
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
    }
}

#[cfg(unix)]
fn children_of(pid: u32) -> Vec<u32> {
    if let Ok(dir) = fs::read_dir("/proc") {
        let mut out = Vec::new();
        let mut seen = false;
        for e in dir.flatten() {
            let Ok(n) = e.file_name().to_string_lossy().parse::<u32>() else { continue };
            seen = true;
            // The fields after the command name, which sits in parentheses and may hold spaces: state, ppid, ...
            let Ok(stat) = fs::read_to_string(format!("/proc/{}/stat", n)) else { continue };
            let Some(rest) = stat.rfind(')').map(|i| &stat[i + 1..]) else { continue };
            if rest.split_whitespace().nth(1).and_then(|p| p.parse::<u32>().ok()) == Some(pid) {
                out.push(n);
            }
        }
        if seen {
            return out;
        }
    }
    let out = Command::new("pgrep").args(["-P", &pid.to_string()]).output();
    out.map(|o| String::from_utf8_lossy(&o.stdout).lines().filter_map(|l| l.trim().parse().ok()).collect()).unwrap_or_default()
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

/// True on Linux with bubblewrap on the PATH: tool runs and checks go into the sandbox.
pub fn sandbox_available() -> bool {
    cfg!(target_os = "linux") && crate::config::which("bwrap").is_some()
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
    cfg["sandbox"] = json!(if sandbox_available() { "bwrap" } else { "off" });
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
    let mut cfg = run_config(base, model);
    // The task's step budget is the round cap of the turn; a profile with its own cap (rounds-24) replaces it.
    if task.max_steps > 0 {
        cfg["main"]["max_rounds"] = json!(task.max_steps);
    }
    let sandbox = cfg["sandbox"] == "bwrap";
    fs::write(root.join("config.json"), serde_json::to_string_pretty(&cfg).unwrap())
        .map_err(|e| format!("cannot write the run config: {}", e))?;
    let provider = provider_of(model);
    save_profile(&root, &provider, model["model"].as_str().unwrap_or(""), &variant.to_json())?;
    let work = dir.join("work");
    if !task.base_commit.is_empty() {
        return Err(format!("task {} names base_commit {}; calibration copies the folder without git history", task.id, task.base_commit));
    }
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
    Ok(TaskRun { task: task.clone(), variant: variant.clone(), dir, workdir, root, turn_timeout, sandbox })
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
    // Every message of the turn, so a cap hit or a tool_describe call can be counted later. Empty for the claude
    // command, which keeps its own history.
    let _ = fs::write(ctx.root.join("history.json"), serde_json::to_string(chat.history()).unwrap_or_default());
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

/// The bubblewrap command of a check: read-only root, the copy `work` writable, a private /tmp, no network.
pub fn sandboxed(command: &str, work: &Path, dir: &Path) -> Command {
    let mut c = Command::new("bwrap");
    let w = work.to_string_lossy().to_string();
    c.args(["--die-with-parent", "--unshare-net", "--ro-bind", "/", "/", "--dev", "/dev", "--proc", "/proc", "--tmpfs", "/tmp",
            "--bind", &w, &w]).arg("--chdir").arg(dir).args(["--", "/bin/sh", "-c", command]);
    c
}

/// Runs the check in the work folder, bounded by the task's timeout. The check runs code the model wrote, so it goes
/// into the sandbox when the run has one.
pub fn check(run: &TaskRun) -> (bool, String) {
    let log = run.dir.join("check.log");
    let mut cmd = if run.sandbox {
        let work = run.dir.join("work");
        sandboxed(&run.task.check, &work.canonicalize().unwrap_or(work), &run.workdir)
    } else {
        crate::platform::shell(&run.task.check)
    };
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

/// How often a run that ended on a rate limit or a provider failure is started in all.
pub const RATE_LIMIT_TRIES: u32 = 3;

/// True when a turn ended on a rate limit of the provider.
pub fn rate_limited(error: &str) -> bool {
    error.contains("status 429")
}

/// True when a turn ended on the provider and not on the model: a rate limit, no answer, or a server error. Such a
/// run is no result of the variant.
pub fn provider_failure(error: &str) -> bool {
    rate_limited(error) || error.contains("no answer from")
        || ["status 500", "status 502", "status 503", "status 504", "status 529"].iter().any(|s| error.contains(s))
}

/// The wait until a usage window resets, from a provider text such as "usage limit reached (5-hour, resets in 34m)",
/// "resets in 2h 5m", "resets in 45s" or "resets at 03:08" (local time), plus one minute. None for a plain rate limit.
pub fn reset_wait(error: &str) -> Option<Duration> {
    reset_wait_at(error, local_minutes())
}

/// `reset_wait` with the local time of day in minutes after midnight; None turns "resets at" off.
pub fn reset_wait_at(error: &str, now_minutes: Option<u32>) -> Option<Duration> {
    if !rate_limited(error) {
        return None;
    }
    if let Some(caps) = regex::Regex::new(r"resets in ((?:\d+\s*[hms]\s*)+)").unwrap().captures(error) {
        let mut secs = 0u64;
        for part in regex::Regex::new(r"(\d+)\s*([hms])").unwrap().captures_iter(&caps[1]) {
            let n: u64 = part[1].parse().unwrap_or(0);
            secs += n * match &part[2] { "h" => 3600, "m" => 60, _ => 1 };
        }
        return (secs > 0).then(|| Duration::from_secs(secs + 60));
    }
    let caps = regex::Regex::new(r"resets at (\d{1,2}):(\d{2})").unwrap().captures(error)?;
    let (h, m): (u32, u32) = (caps[1].parse().ok()?, caps[2].parse().ok()?);
    if h > 23 || m > 59 {
        return None;
    }
    let now = now_minutes?;
    let wait = (h * 60 + m + 1440 - now % 1440) % 1440;
    Some(Duration::from_secs((wait as u64 + 1) * 60))
}

/// The local time of day in minutes after midnight, from `date`; None where that fails.
fn local_minutes() -> Option<u32> {
    let out = Command::new("date").arg("+%H:%M").stderr(Stdio::null()).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let (h, m) = text.trim().split_once(':')?;
    Some(h.parse::<u32>().ok()? * 60 + m.parse::<u32>().ok()?)
}

/// Takes the lock of an output folder: a second calibrate process on the same folder would remove the run folders
/// of the first. The lock goes with the returned file, also when the process dies.
pub fn lock_out(out: &Path) -> Result<fs::File, String> {
    fs::create_dir_all(out).map_err(|e| format!("cannot make {}: {}", out.display(), e))?;
    let path = out.join(".lock");
    let f = fs::OpenOptions::new().create(true).truncate(false).write(true).open(&path).map_err(|e| format!("cannot open {}: {}", path.display(), e))?;
    f.try_lock().map_err(|_| format!("another calibrate process works in {}; wait for it or choose another --out", out.display()))?;
    Ok(f)
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
    let _lock = lock_out(plan.out)?;
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
                    if !provider_failure(&result.error) || attempt >= RATE_LIMIT_TRIES {
                        break result;
                    }
                    std::thread::sleep(plan.rate_limit_pause);
                };
                // Still a provider failure after the last try: no line, so the next call runs the task again.
                if provider_failure(&result.error) {
                    on_done(&result);
                    continue;
                }
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

/// The share of tasks on which two runs of the same setting differ (Kimi K3 on the hold-out: 4 of 28).
pub const AA_DISCORDANCE: f64 = 0.14;

/// The smallest gain k in passes that two runs of the same setting reach with at most 5 % chance on `n` tasks, when
/// each task differs with chance `pd` and a difference goes either way with chance one half: 4 at n = 20 and pd 0.14.
pub fn aa_band(n: usize, pd: f64) -> usize {
    let binom = |n: usize, k: usize| (0..k).fold(1.0f64, |c, i| c * (n - i) as f64 / (i + 1) as f64);
    let tail = |k: i64| {
        let mut p = 0.0;
        for d in 0..=n {
            let pd_d = binom(n, d) * pd.powi(d as i32) * (1.0 - pd).powi((n - d) as i32);
            for b in 0..=d {
                if 2 * b as i64 - d as i64 >= k {
                    p += pd_d * binom(d, b) / 2f64.powi(d as i32);
                }
            }
        }
        p
    };
    (1..=n.max(1)).find(|k| tail(*k as i64) <= 0.05).unwrap_or(n.max(1))
}

/// The index of the winning score; index 0 is the default. A variant wins with at least `aa_band` more passed checks
/// than the default (4 of 20), the gain that two runs of the same setting reach in at most 5 % of cases. With no
/// fewer passes it also wins at 80 % of the default's tokens or less. Of several, the most passes, then the fewest
/// tokens. Everything else keeps the default: DeepSeek's effort-low at 18 against 17 does not move a profile.
pub fn select(scores: &[Score]) -> usize {
    let Some(base) = scores.first() else { return 0 };
    let band = aa_band(base.tasks, AA_DISCORDANCE);
    let tokens = |s: &Score| s.tokens_in + s.tokens_out;
    let mut best = 0usize;
    for (i, s) in scores.iter().enumerate().skip(1) {
        let qualifies = s.passed >= base.passed + band || (s.passed >= base.passed && (tokens(s) as f64) <= 0.8 * tokens(base) as f64);
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
    /// Run without bubblewrap where it is missing: the model's commands and the checks then run on the host.
    pub no_sandbox: bool,
}

pub fn parse_args(args: &[String]) -> Result<Options, String> {
    let mut o = Options { n: 0, jobs: MAX_JOBS, turn_timeout: TURN_TIMEOUT_SECS, ..Default::default() };
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
            "--no-sandbox" => { o.no_sandbox = true; i += 1; }
            other if other.starts_with("--") => return Err(format!("unknown option {}", other)),
            other => { o.model = other.to_string(); i += 1; }
        }
    }
    if o.model.is_empty() {
        return Err("name the model: penelope calibrate MODEL [--provider P] [--tasks FILE] [--n N]".into());
    }
    if o.validate && !o.only.is_empty() && !(o.only.iter().any(|v| v == "default") && o.only.iter().any(|v| v == "calibrated")) {
        return Err("--validate compares two arms: --variants must name default and calibrated, or be left out".into());
    }
    Ok(o)
}

/// True when two variants run the same setting: then a validation measures run-to-run noise (A/A).
pub fn same_setting(a: &Variant, b: &Variant, default_effort: &str) -> bool {
    let effort = |v: &Variant| if v.effort.is_empty() { default_effort.to_string() } else { v.effort.clone() };
    let rounds = |v: &Variant| if v.rounds == 0 { None } else { Some(v.rounds) };
    (&a.tool_format, &a.tool_list, effort(a), &a.prompt, rounds(a)) == (&b.tool_format, &b.tool_list, effort(b), &b.prompt, rounds(b))
}

/// `penelope calibrate`: calibrate (default) or validate the stored profile against the default (`--validate`).
pub fn command(ctx: &Ctx, o: &Options) -> Result<String, String> {
    command_with(ctx, o, &child_turn)
}

/// `command` with the turn of each run given: the child process, or a fake model in tests.
pub fn command_with(ctx: &Ctx, o: &Options, exec: &(dyn Fn(&TaskRun) -> Turn + Sync)) -> Result<String, String> {
    if !sandbox_available() && !o.no_sandbox {
        return Err("calibration needs bubblewrap (Linux, bwrap on the PATH): the model runs any shell command and the check runs code the \
model wrote. Install bubblewrap, or pass --no-sandbox to run both on this machine without a sandbox.".into());
    }
    let provider = if o.provider.is_empty() { provider_of(&ctx.cfg["main"]) } else { o.provider.clone() };
    let p = crate::providers::get(&provider).ok_or_else(|| format!("no provider named {} (penelope providers)", provider))?;
    let model = crate::providers::setting(p, Some(&o.model), None, None)?;
    let tasks_path = if o.tasks.is_empty() { "tests/data/eval/tasks50.jsonl".to_string() } else { o.tasks.clone() };
    let mut tasks = crate::eval::parse_tasks(Path::new(&tasks_path))?;
    // --validate runs the whole hold-out unless --n says otherwise; calibration takes the first 20.
    let n = if o.n > 0 { o.n } else if o.validate { tasks.len() } else { 20 };
    if n < tasks.len() {
        eprintln!("using the first {} of the {} tasks in {} (--n)", n, tasks.len(), tasks_path);
    }
    tasks.truncate(n.max(1));
    let sha = crate::brief::sha256_hex(&fs::read(&tasks_path).unwrap_or_default());
    let out = if o.out.is_empty() { ctx.root.join("calibrate") } else { PathBuf::from(&o.out) };
    let tag = format!("{}__{}", safe(&provider), safe(&o.model));
    let phase = if o.validate { "validate" } else { "calibrate" };
    let file_tag = safe(Path::new(&tasks_path).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default().as_str());
    let out = out.join(&tag).join(format!("{}-{}", phase, file_tag));
    let default_effort = crate::providers::default_effort(&provider, &o.model);
    let all = variants(&provider, &o.model);
    let mut aa = false;
    let mut variants = if o.validate {
        let stored = load_profile(&ctx.root, &model).ok_or_else(|| format!("no profile for {} yet; run penelope calibrate {} first", o.model, o.model))?;
        let base = Variant::default_for(default_effort);
        aa = same_setting(&stored, &base, default_effort);
        vec![base, Variant { name: "calibrated".into(), ..stored }]
    } else {
        all.clone()
    };
    if !o.only.is_empty() {
        variants.retain(|v| o.only.contains(&v.name));
    }
    if variants.is_empty() {
        return Err("no variant left after --variants".into());
    }
    if o.validate && variants.len() != 2 {
        return Err("--validate compares two arms, default and calibrated".into());
    }
    let plan = Plan { base: &ctx.cfg, model: &model, prices: ctx, out: &out, jobs: o.jobs, turn_timeout: Duration::from_secs(o.turn_timeout),
                      rate_limit_pause: Duration::from_secs(60) };
    let count = Mutex::new(0usize);
    let total = tasks.len() * variants.len();
    let results = run_all(&plan, &tasks, &variants, exec, &|r| {
        let mut c = count.lock().unwrap_or_else(|e| e.into_inner());
        *c += 1;
        let state = if provider_failure(&r.error) { "LEFT OUT (provider)" } else if r.passed { "pass" } else { "FAIL" };
        eprintln!("[{}/{}] {} {} {} {} in/{} out {:.0}s{}", c, total, r.variant, r.task, state, r.tokens_in, r.tokens_out,
                  r.wall_ms as f64 / 1000.0, if r.error.is_empty() { String::new() } else { format!(" error: {}", r.error.chars().take(120).collect::<String>()) });
    })?;
    let scores: Vec<Score> = variants.iter().map(|v| score(&results, &v.name)).collect();
    let missing = scores.iter().map(|s| tasks.len().saturating_sub(s.tasks)).sum::<usize>();
    let mut text = format!("{} {} on {} ({} tasks, sha256 {})\n", phase, o.model, tasks_path, tasks.len(), &sha[..16]);
    if missing > 0 {
        text.push_str(&format!("{} runs ended on the provider and are left out; run the same command again to fill them in\n", missing));
    }
    if o.validate {
        let (b, c) = paired(&results, "default", "calibrated");
        text.push_str(&table(&scores, None));
        text.push_str(&format!("paired: b = {} (only calibrated passed), c = {} (only default passed), exact McNemar p = {:.4}\n", b, c, mcnemar(b, c)));
        if aa {
            text.push_str("A/A: the stored profile runs the default setting, so this pair measures run-to-run noise; leave it out of a pooled test\n");
        }
        text.push_str(&format!("calibrated variant: {}\n", variants[1].to_json()));
        return Ok(text);
    }
    let chosen = select(&scores);
    text.push_str(&table(&scores, Some(chosen)));
    // A profile needs the default as the baseline and every variant that applies, each on every task.
    let complete = variants[0].name == "default" && variants.len() == all.len() && missing == 0;
    if !complete {
        text.push_str("no profile written: the run lacks the default, a variant or a task (it needs every variant on every task)\n");
        return Ok(text);
    }
    let v = &variants[chosen];
    let (b, c) = paired(&results, "default", &v.name);
    let profile = json!({
        "schema": 1, "variant": v.name, "tool_format": v.tool_format, "tool_list": v.tool_list,
        "effort": v.effort, "prompt": v.prompt, "rounds": v.rounds,
        "calibrated": crate::sessions::stamp(now()), "tasks": tasks_path, "tasks_sha256": sha, "n": tasks.len(),
        "paired": {"b": b, "c": c, "aa_band": aa_band(tasks.len(), AA_DISCORDANCE), "aa_discordance": AA_DISCORDANCE},
        "scores": scores.iter().map(|s| json!({"variant": s.variant, "passed": s.passed, "tasks": s.tasks, "tokens_in": s.tokens_in,
                                               "tokens_out": s.tokens_out, "eur": s.eur, "wall_ms": s.wall_ms})).collect::<Vec<_>>()
    });
    let path = save_profile(&ctx.root, &provider, &o.model, &profile)?;
    text.push_str(&format!("profile: {}\n", path.display()));
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
                            check: check.into(), check_timeout_seconds: timeout, max_steps: 6, base_commit: String::new() }
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
        assert_eq!(select(&[s("default", 15, 1000), s("text", 19, 2000), s("index", 15, 500)]), 1);   // 4 more passes beat fewer tokens
        assert_eq!(select(&[s("default", 15, 1000), s("text", 18, 2000)]), 0);                       // 3 more passes sit inside the A/A band
        assert_eq!(select(&[s("default", 17, 1000), s("effort-low", 18, 900)]), 0);                  // DeepSeek's calibration
        assert_eq!(select(&[s("default", 15, 1000), s("text", 15, 800), s("index", 15, 500)]), 2);
        assert_eq!(select(&[s("default", 15, 1000), s("text", 17, 900), s("index", 17, 800)]), 2);
        assert_eq!(select(&[]), 0);
    }

    #[test]
    fn the_aa_band_matches_the_binomial_model() {
        // Two runs of one setting, 14 % of tasks differ: a gain of 4 of 20 has a chance of 1.8 %, 3 of 20 of 6.4 %.
        assert_eq!(aa_band(20, 0.14), 4);
        assert_eq!(aa_band(30, 0.14), 4);
        assert_eq!(aa_band(20, 0.233), 5);
        assert_eq!(aa_band(50, 0.233), 7);
    }

    #[test]
    fn a_usage_window_is_waited_out_and_a_plain_limit_is_not() {
        let e = "https://opencode.ai/zen/go/v1/chat/completions answered with status 429: OpenCode Go usage limit reached (5-hour, resets in 34m).";
        assert_eq!(reset_wait(e), Some(Duration::from_secs(35 * 60)));
        assert_eq!(reset_wait("status 429: usage limit reached (resets in 2h 5m)"), Some(Duration::from_secs(126 * 60)));
        assert_eq!(reset_wait("status 429: Rate limit exceeded. Please try again later."), None);
        assert_eq!(reset_wait("resets in 34m"), None);            // no 429: not a rate limit
        assert_eq!(reset_wait("status 429: limit reached, resets in 45s"), Some(Duration::from_secs(105)));
        assert_eq!(reset_wait("status 429: resets in 1h 2m 3s"), Some(Duration::from_secs(3723 + 60)));
        // "resets at" counts from the local time: 22:40 to 03:08 is 268 minutes, plus one.
        assert_eq!(reset_wait_at("status 429: usage limit reached, resets at 03:08", Some(22 * 60 + 40)), Some(Duration::from_secs(269 * 60)));
        assert_eq!(reset_wait_at("status 429: resets at 23:00", Some(22 * 60)), Some(Duration::from_secs(61 * 60)));
        assert_eq!(reset_wait_at("status 429: resets at 03:08", None), None);
        assert!(provider_failure("no answer from opencode.ai: connection reset") && provider_failure("status 503: overloaded"));
        assert!(!provider_failure("the check failed"));
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
        assert_eq!(select(&scores), 0);                  // 1 more pass of 2 sits inside the A/A band of 2
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
        let a: Vec<String> = ["glm-5.3", "--provider", "opencode", "--n", "10", "--variants", "default,text"].iter().map(|s| s.to_string()).collect();
        let o = parse_args(&a).unwrap();
        assert_eq!((o.model.as_str(), o.provider.as_str(), o.n, o.validate, o.no_sandbox), ("glm-5.3", "opencode", 10, false, false));
        assert_eq!(o.only, ["default", "text"]);
        // --validate needs both arms.
        let v = |list: &str| parse_args(&["glm-5.3", "--validate", "--variants", list].iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert!(v("default").is_err() && v("calibrated").is_err() && v("default,calibrated").is_ok());
        assert!(parse_args(&["x".to_string(), "--no-sandbox".to_string()]).unwrap().no_sandbox);
        assert!(parse_args(&["--n".to_string()]).is_err());
        assert!(parse_args(&[]).is_err());
    }

    #[test]
    fn a_profile_effort_leaves_an_effort_that_routing_or_the_user_chose() {
        let dir = temp("effort");
        let model = json!({"kind": "openai", "base_url": "https://opencode.ai/zen/go/v1", "model": "glm-5.3", "effort": "low"});
        let ctx = Ctx { root: dir.clone(), cfg: json!({"main": model}) };
        let high = Variant { name: "x".into(), effort: "high".into(), ..Variant::default_for("high") };
        save_profile(&dir, "opencode", "glm-5.3", &high.to_json()).unwrap();
        assert_eq!(apply(&ctx).cfg["main"]["effort"], "low");
        // No effort set: the profile's effort goes in.
        let ctx = Ctx { root: dir.clone(), cfg: json!({"main": {"kind": "openai", "base_url": "https://opencode.ai/zen/go/v1", "model": "glm-5.3"}}) };
        let low = Variant { name: "effort-low".into(), effort: "low".into(), ..Variant::default_for("high") };
        save_profile(&dir, "opencode", "glm-5.3", &low.to_json()).unwrap();
        assert_eq!(apply(&ctx).cfg["main"]["effort"], "low");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn text_values_get_the_type_of_the_schema() {
        let full = vec![json!({"name": "t", "inputSchema": {"type": "object", "properties": {
            "text": {"type": "string"}, "n": {"type": "integer"}, "flag": {"type": "boolean"}, "list": {"type": "array"}}}})];
        let call = crate::llm::ToolCall { id: "text-call-1".into(), name: "t".into(),
                                         args: json!({"text": "{\"a\": 1}", "n": "30", "flag": "true", "list": "[1, 2]", "other": "7"}) };
        let typed = typed_call(&full, &call);
        assert_eq!(typed.args, json!({"text": "{\"a\": 1}", "n": 30, "flag": true, "list": [1, 2], "other": "7"}));
        let bad = crate::llm::ToolCall { id: "text-call-2".into(), name: "t".into(), args: json!({"n": "thirty"}) };
        assert_eq!(typed_call(&full, &bad).args, json!({"n": "thirty"}));
    }

    #[test]
    fn the_short_prompt_names_no_tool_the_lead_lacks() {
        let names: Vec<String> = crate::spec::list().iter().filter_map(|t| t["name"].as_str().map(str::to_string)).collect();
        for delegate in ["auto", "off", "offer", "web"] {
            let ctx = Ctx { root: PathBuf::from("."), cfg: json!({"delegate": delegate, "main": {"prompt": "short"}}) };
            let short = rules_for(&ctx);
            let mine = crate::mcp::main_tools(&ctx);
            let words: Vec<&str> = short.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).collect();
            for name in &names {
                // "write" and "read" in rule 3 name the tools; a missing one would fail with "unknown tool".
                if words.contains(&name.as_str()) && !["scout"].contains(&name.as_str()) {
                    assert!(mine.contains(&name.as_str()), "delegate {}: the short prompt names {}, which the lead lacks", delegate, name);
                }
            }
            assert!(short.contains(crate::backend::look_rule(&ctx)));
        }
    }

    #[test]
    fn same_setting_tells_an_aa_validation_apart() {
        let base = Variant::default_for("high");
        let stored = Variant { name: "default".into(), effort: String::new(), ..Variant::default_for("") };
        assert!(same_setting(&stored, &base, "high"));
        let low = Variant { name: "effort-low".into(), effort: "low".into(), ..base.clone() };
        assert!(!same_setting(&low, &base, "high"));
        let rounds = Variant { name: "rounds-24".into(), rounds: 24, ..base.clone() };
        assert!(!same_setting(&rounds, &base, "high"));
    }

    /// A task file with `n` tasks on a one-file repo; each check wants `ok` in out.txt.
    fn task_file(dir: &Path, n: usize) -> PathBuf {
        let repo = dir.join("repo");
        fs::create_dir_all(&repo).unwrap();
        fs::write(repo.join("seed.txt"), "x").unwrap();
        let check = if cfg!(windows) { "type out.txt | findstr ok" } else { "grep -qx ok out.txt" };
        let lines: Vec<String> = (1..=n).map(|i| json!({"id": format!("t{}", i), "prompt": format!("task {}", i), "setup": "repo",
                                                          "check": check, "max_steps": 12}).to_string()).collect();
        let path = dir.join("tasks.jsonl");
        fs::write(&path, lines.join("\n")).unwrap();
        path
    }

    fn options(tasks: &Path, out: &Path, only: &[&str], validate: bool) -> Options {
        Options { model: "glm-5.3".into(), provider: "opencode".into(), tasks: tasks.to_string_lossy().into(), n: 0, jobs: 3,
                  out: out.to_string_lossy().into(), validate, only: only.iter().map(|s| s.to_string()).collect(), turn_timeout: 30, no_sandbox: true }
    }

    #[test]
    fn a_rounds_winner_is_saved_with_its_cap_and_partial_runs_save_nothing() {
        let dir = temp("command");
        let tasks = task_file(&dir, 3);
        let ctx = Ctx { root: dir.join(".penelope"), cfg: json!({"prices": {"glm-5.3": [1.0, 2.0]}, "eur_per_usd": 1.0}) };
        // The fake model passes only with 24 rounds.
        let fake = |run: &TaskRun| {
            fs::write(run.workdir.join("out.txt"), if run.variant.rounds == 24 { "ok\n" } else { "no\n" }).unwrap();
            Turn::default()
        };
        let model = json!({"kind": "openai", "base_url": "https://opencode.ai/zen/go/v1", "model": "glm-5.3"});
        let path = profile_path(&ctx.root, "opencode", "glm-5.3");
        // A run without the default, or without every variant, writes no profile.
        let text = command_with(&ctx, &options(&tasks, &dir.join("out"), &["text", "index"], false), &fake).unwrap();
        assert!(text.contains("no profile written") && !path.exists(), "{}", text);
        let text = command_with(&ctx, &options(&tasks, &dir.join("out"), &["default", "rounds-24"], false), &fake).unwrap();
        assert!(text.contains("no profile written") && !path.exists(), "{}", text);
        let text = command_with(&ctx, &options(&tasks, &dir.join("out"), &[], false), &fake).unwrap();
        assert!(text.contains("profile:"), "{}", text);
        let stored: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!((stored["variant"].as_str(), stored["rounds"].as_u64()), (Some("rounds-24"), Some(24)));
        let run = apply(&Ctx { root: ctx.root.clone(), cfg: json!({"main": model}) });
        assert_eq!(run.cfg["main"]["max_rounds"], 24);
        // --validate needs both arms; with the rounds profile the pair is no A/A pair.
        assert!(command_with(&ctx, &options(&tasks, &dir.join("out"), &["default"], true), &fake).is_err());
        let text = command_with(&ctx, &options(&tasks, &dir.join("out"), &[], true), &fake).unwrap();
        assert!(text.contains("b = 3") && !text.contains("A/A"), "{}", text);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_provider_failure_writes_no_line_and_runs_again() {
        let dir = temp("provider");
        let repo = dir.join("repo");
        fs::create_dir_all(&repo).unwrap();
        let check = if cfg!(windows) { "type out.txt | findstr ok" } else { "grep -qx ok out.txt" };
        let tasks = vec![task(&repo, "t1", check, 30)];
        let model = json!({"kind": "openai", "base_url": "https://opencode.ai/zen/go/v1", "model": "glm-5.3"});
        let prices = Ctx { root: dir.clone(), cfg: json!({}) };
        let out = dir.join("out");
        let plan = Plan { base: &json!({}), model: &model, prices: &prices, out: &out, jobs: 1, turn_timeout: Duration::from_secs(5),
                          rate_limit_pause: Duration::from_millis(5) };
        let vars = vec![Variant::default_for("high")];
        let calls = AtomicUsize::new(0);
        let always = |_: &TaskRun| { calls.fetch_add(1, Ordering::SeqCst); Turn { error: "status 429: Rate limit exceeded.".into(), ..Default::default() } };
        assert!(run_all(&plan, &tasks, &vars, &always, &|_| {}).unwrap().is_empty());
        assert_eq!(calls.load(Ordering::SeqCst), RATE_LIMIT_TRIES as usize);
        assert!(fs::read_to_string(out.join("results.jsonl")).unwrap_or_default().trim().is_empty());
        let gone = |_: &TaskRun| Turn { error: "no answer from opencode.ai".into(), ..Default::default() };
        assert!(run_all(&plan, &tasks, &vars, &gone, &|_| {}).unwrap().is_empty());
        let good = |run: &TaskRun| { fs::write(run.workdir.join("out.txt"), "ok\n").unwrap(); Turn::default() };
        let r = run_all(&plan, &tasks, &vars, &good, &|_| {}).unwrap();
        assert_eq!((r.len(), r[0].passed), (1, true));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_second_process_on_the_same_output_folder_is_refused() {
        let dir = temp("lock");
        let out = dir.join("out");
        let held = lock_out(&out).unwrap();
        let model = json!({"kind": "openai", "model": "m"});
        let prices = Ctx { root: dir.clone(), cfg: json!({}) };
        let plan = Plan { base: &json!({}), model: &model, prices: &prices, out: &out, jobs: 1, turn_timeout: Duration::from_secs(5),
                          rate_limit_pause: Duration::from_millis(5) };
        let err = run_all(&plan, &[], &[], &|_| Turn::default(), &|_| {}).unwrap_err();
        assert!(err.contains("another calibrate process"), "{}", err);
        drop(held);
        assert!(run_all(&plan, &[], &[], &|_| Turn::default(), &|_| {}).is_ok());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_task_budget_caps_the_rounds() {
        let dir = temp("steps");
        let repo = dir.join("repo");
        fs::create_dir_all(&repo).unwrap();
        fs::write(repo.join("note.txt"), "x\n").unwrap();
        let call = json!({"choices": [{"finish_reason": "tool_calls", "message": {"role": "assistant", "content": null, "tool_calls": [
            {"id": "c1", "type": "function", "function": {"name": "read", "arguments": json!({"path": "note.txt"}).to_string()}}]}}],
            "usage": {"prompt_tokens": 10, "completion_tokens": 2}}).to_string();
        let (address, seen) = crate::llm::fake::serve(vec![(200, call); 8]);
        let model = json!({"kind": "openai", "base_url": format!("{}/v1", address), "model": "test-model", "api_key_env": ""});
        let mut t = task(&repo, "t", "true", 30);
        t.max_steps = 3;
        let run = prepare(&json!({}), &model, &dir.join("out"), &t, &Variant::default_for(""), Duration::from_secs(30)).unwrap();
        let mut ctx = crate::config::load(&run.root);
        ctx.cfg["sandbox"] = json!("off");
        assert_eq!(ctx.cfg["main"]["max_rounds"], 3);
        let _ = crate::backend::Chat::new().turn_with(&ctx, "Read note.txt forever.", "", &mut |_| {});
        assert_eq!(seen.lock().unwrap().len(), 3);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_run_keeps_its_whole_history() {
        let (mut ctx, dir) = endpoint("history", "http://127.0.0.1:9", &Variant::default_for(""));
        let call = json!({"choices": [{"finish_reason": "tool_calls", "message": {"role": "assistant", "content": null, "tool_calls": [
            {"id": "c1", "type": "function", "function": {"name": "read", "arguments": json!({"path": "note.txt", "cwd": dir.to_string_lossy()}).to_string()}}]}}],
            "usage": {"prompt_tokens": 10, "completion_tokens": 2}}).to_string();
        let (address, _seen) = crate::llm::fake::serve(vec![(200, call), reply("The note says hello.")]);
        ctx.cfg["main"]["base_url"] = json!(format!("{}/v1", address));
        fs::write(ctx.root.join("calibrate-prompt.txt"), "What does note.txt say?").unwrap();
        one(&ctx).unwrap();
        let history: Vec<Value> = serde_json::from_str(&fs::read_to_string(ctx.root.join("history.json")).unwrap()).unwrap();
        let roles: Vec<&str> = history.iter().filter_map(|m| m["role"].as_str()).collect();
        assert_eq!(roles, ["user", "assistant", "tool", "assistant"]);
        assert!(history[2]["content"].as_str().unwrap().contains("hello from the note"));
        assert_eq!(history[3]["content"], "The note says hello.");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn native_mode_leaves_quoted_template_markup_as_text() {
        let quoted = "The README shows GLM's format:\n```\n<tool_call>run<arg_key>command</arg_key><arg_value>rm -rf build</arg_value></tool_call>\n```";
        let (address, seen) = crate::llm::fake::serve(vec![reply(quoted)]);
        let (ctx, dir) = endpoint("native", &address, &Variant::default_for(""));
        let answer = crate::backend::Chat::new().turn_with(&ctx, "What format does GLM use?", "", &mut |_| {}).unwrap();
        assert_eq!(answer, quoted);
        assert_eq!(seen.lock().unwrap().len(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_turn_over_its_time_loses_children_in_their_own_process_group() {
        let dir = temp("group");
        // Like the claude command: a child that starts a process group of its own and lives for 30 s.
        let script = format!("python3 -c \"import os,time; os.setpgid(0,0); open('{}','w').write(str(os.getpid())); time.sleep(30)\" & wait",
                             dir.join("child.pid").display());
        let mut cmd = Command::new("sh");
        cmd.args(["-c", &script]);
        let start = Instant::now();
        let b = run_bounded(cmd, Duration::from_secs(2), &dir.join("log")).unwrap();
        assert!(b.timed_out && start.elapsed() < Duration::from_secs(10));
        let pid = fs::read_to_string(dir.join("child.pid")).unwrap().trim().to_string();
        std::thread::sleep(Duration::from_millis(300));
        let alive = Command::new("kill").args(["-0", &pid]).stderr(Stdio::null()).status().map(|s| s.success()).unwrap_or(false);
        assert!(!alive, "the child {} in its own group survived the timeout", pid);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_check_cannot_write_outside_the_copy() {
        if !sandbox_available() {
            return;                      // the command refuses to start without bubblewrap; nothing to test here
        }
        let dir = temp("escape");
        let repo = dir.join("repo");
        fs::create_dir_all(repo.join("app")).unwrap();
        let home = std::env::var("HOME").unwrap();
        let target = PathBuf::from(&home).join(format!("penelope-escape-{}", std::process::id()));
        let _ = fs::remove_file(&target);
        // The model's code, run by the check: it tries to write into the home folder and to reach the network.
        fs::write(repo.join("app").join("cli.sh"), format!("touch '{}'; echo inside > inside.txt\n", target.display())).unwrap();
        let t = task(&repo, "escape", "sh app/cli.sh; test -f inside.txt", 30);
        let model = json!({"kind": "openai", "model": "m"});
        let run = prepare(&json!({}), &model, &dir.join("out"), &t, &Variant::default_for(""), Duration::from_secs(5)).unwrap();
        assert!(run.sandbox);
        let (passed, text) = check(&run);
        assert!(passed, "{}", text);
        assert!(!target.exists(), "the check wrote {}", target.display());
        let _ = fs::remove_file(&target);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_frozen_hard_set_hashes_on_every_system() {
        // .gitattributes keeps these files byte for byte (-text); with CRLF the checks that hash test files would fail.
        let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../cli/tests/data/eval");
        let list = fs::read_to_string(base.join("hard.sha256")).unwrap();
        let mut n = 0;
        for line in list.lines().filter(|l| !l.trim().is_empty()) {
            let (hash, name) = line.split_once("  ").unwrap();
            let bytes = fs::read(base.join(name.trim())).unwrap_or_else(|e| panic!("{}: {}", name, e));
            assert_eq!(crate::brief::sha256_hex(&bytes), hash.trim(), "{} changed (line endings?)", name);
            n += 1;
        }
        assert_eq!(n, 84);
    }
}
