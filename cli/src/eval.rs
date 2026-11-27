//! Measurements: run the same tasks under three setups and put the numbers in one table.
//!
//! A task is one line of JSONL with an id, a prompt, a setup folder and a check command. The runner copies the setup
//! folder to a scratch worktree, gives the model only that folder as its working directory, and then runs the check.
//! Three setups are measured:
//!
//! - `large`: the main model alone, through the normal chat path.
//! - `large-subagents`: the main model through `agents::run`, with one worker and one judge.
//! - `cluster`: `clusters::orchestrate` with the small profile `eval-small` (worker and validator on the weak model).
//!
//! Every setup records passed checks, tokens in and out, euro from `cost.rs`, and wall time. `penelope eval` prints the
//! table and appends it to `docs/RUNS.md`. `penelope replay ID --model MODEL` runs the stored prompts again through a
//! read-only tool gate: no write, edit, run, job, screen or MCP call can happen, and no task folder or check is touched.

use crate::config::Ctx;
use serde_json::{json, Value};
use std::cell::RefCell;
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The setups the runner knows.
pub const SETUPS: &[&str] = &["large", "large-subagents", "cluster"];

/// The profile the cluster setup writes. It is small on purpose: one worker, one validator, the weak model.
const CLUSTER_PROFILE: &str = "eval-small";

/// The cluster starts at three independent parts. A one-part task goes straight to the large model.
const MIN_CLUSTER_WORKERS: usize = 3;

/// Tools replay may call. Everything else is refused before it reaches a handler, so replay cannot run a command or
/// change a file. `run_detail` is left out because it reads logs of earlier commands, not the project.
const REPLAY_TOOLS: &[&str] = &["read", "find", "grep", "tree", "outline", "json_query"];

/// Folders the scratch copy does not take along. A task that needs git history or build output sets `"copy": false`
/// in its setup and works in the folder itself.
const COPY_SKIP: &[&str] = &[".git", "target", "node_modules", ".venv", "venv", "__pycache__", ".penelope", "dist", ".cache", ".next"];

thread_local! {
    /// The working folder of the running eval task. The tool table in `mcp.rs` uses it when a call names no cwd, so a
    /// model does not have to repeat the path in every call. Thread-local: clusters run in one thread for the eval.
    static TASK_CWD: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

/// The working folder of the eval task on this thread, or an empty string when no eval is running. An MCP server
/// child starts with `PENELOPE_TASK_CWD` when the claude command runs an eval task; that process has no thread-local.
pub fn task_cwd() -> String {
    let local = TASK_CWD.with(|d| d.borrow().as_ref().map(|p| p.to_string_lossy().to_string()).unwrap_or_default());
    if !local.is_empty() {
        return local;
    }
    std::env::var("PENELOPE_TASK_CWD").unwrap_or_default()
}

fn with_task_cwd<T>(dir: &Path, f: impl FnOnce() -> T) -> T {
    let before = TASK_CWD.with(|d| d.replace(Some(dir.to_path_buf())));
    let out = f();
    TASK_CWD.with(|d| *d.borrow_mut() = before);
    out
}

#[derive(Clone, Debug, PartialEq)]
pub struct Task {
    pub id: String,
    pub prompt: String,
    /// Absolute path of the setup folder.
    pub repo: PathBuf,
    /// Folder inside `repo`, empty for the root.
    pub cwd: String,
    /// Copy the setup folder to a scratch worktree before the task. True unless the line says false.
    pub copy: bool,
    pub check: String,
    pub check_timeout_seconds: u64,
    pub max_steps: usize,
}

#[derive(Clone, Debug, Default)]
pub struct TaskResult {
    pub id: String,
    pub passed: bool,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub eur: f64,
    pub wall_ms: u128,
    pub check_output: String,
    pub answer: String,
    pub error: String,
}

#[derive(Clone, Debug, Default)]
pub struct SetupResult {
    pub setup: String,
    pub run_id: String,
    pub model: String,
    pub tasks: Vec<TaskResult>,
}

impl SetupResult {
    pub fn passed(&self) -> usize {
        self.tasks.iter().filter(|t| t.passed).count()
    }
    pub fn tokens(&self) -> (u64, u64) {
        self.tasks.iter().fold((0, 0), |(i, o), t| (i + t.tokens_in, o + t.tokens_out))
    }
    pub fn eur(&self) -> f64 {
        self.tasks.iter().map(|t| t.eur).sum()
    }
    pub fn wall_ms(&self) -> u128 {
        self.tasks.iter().map(|t| t.wall_ms).sum()
    }
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn now_ms() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)
}

fn safe_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 80 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

fn rel_cwd(text: &str) -> Result<String, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(String::new());
    }
    let p = Path::new(text);
    if p.is_absolute() {
        return Err(format!("setup.cwd is relative to the setup repo, got the absolute path {}", text));
    }
    for c in p.components() {
        match c {
            Component::Normal(_) | Component::CurDir => {}
            _ => return Err(format!("setup.cwd may not leave the setup repo: {}", text)),
        }
    }
    Ok(text.to_string())
}

/// Reads a JSONL task file. A line without the fields a task needs is refused with its line number; a broken line is
/// never skipped silently, because a skipped task would make a table look better than the run was.
pub fn parse_tasks(path: &Path) -> Result<Vec<Task>, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("cannot read the task file {}: {}", path.display(), e))?;
    let base = path.parent().filter(|p| !p.as_os_str().is_empty()).map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."));
    let mut tasks: Vec<Task> = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let line_no = n + 1;
        let v: Value = serde_json::from_str(line).map_err(|e| format!("task line {} is not JSON: {}", line_no, e))?;
        let id = v["id"].as_str().unwrap_or("").trim().to_string();
        if !safe_id(&id) {
            return Err(format!("task line {}: id is 1 to 80 letters, digits, - _ or .", line_no));
        }
        if tasks.iter().any(|t| t.id == id) {
            return Err(format!("task line {}: id {} is used twice", line_no, id));
        }
        let prompt = v["prompt"].as_str().unwrap_or("").trim().to_string();
        if prompt.len() < 3 {
            return Err(format!("task line {} ({}): prompt is missing or shorter than 3 characters", line_no, id));
        }
        let setup = &v["setup"];
        let repo_text = if let Some(s) = setup.as_str() {
            s.trim()
        } else {
            setup["repo"].as_str().map(str::trim).unwrap_or("")
        };
        if repo_text.is_empty() {
            return Err(format!("task line {} ({}): setup needs a repo folder", line_no, id));
        }
        let repo = if Path::new(repo_text).is_absolute() { PathBuf::from(repo_text) } else { base.join(repo_text) };
        let repo = repo.canonicalize().unwrap_or(repo);
        if !repo.is_dir() {
            return Err(format!("task line {} ({}): setup repo {} is not a folder", line_no, id, repo.display()));
        }
        let cwd = rel_cwd(setup["cwd"].as_str().unwrap_or("")).map_err(|e| format!("task line {} ({}): {}", line_no, id, e))?;
        let copy = if setup.is_object() { setup["copy"].as_bool().unwrap_or(true) } else { true };
        let check = v["check"].as_str().unwrap_or("").trim().to_string();
        if check.is_empty() {
            return Err(format!("task line {} ({}): check command is missing", line_no, id));
        }
        let timeout = v["check_timeout_seconds"].as_u64().unwrap_or(300).clamp(1, 3600);
        let max_steps = v["max_steps"].as_u64().unwrap_or(12).clamp(1, 30) as usize;
        tasks.push(Task { id, prompt, repo, cwd, copy, check, check_timeout_seconds: timeout, max_steps });
    }
    if tasks.is_empty() {
        return Err(format!("the task file {} holds no task", path.display()));
    }
    Ok(tasks)
}

fn copy_tree(src: &Path, dst: &Path) -> Result<(), String> {
    fs::create_dir_all(dst).map_err(|e| format!("cannot make {}: {}", dst.display(), e))?;
    let entries = fs::read_dir(src).map_err(|e| format!("cannot read {}: {}", src.display(), e))?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        if COPY_SKIP.iter().any(|s| name.to_string_lossy() == *s) {
            continue;
        }
        let from = entry.path();
        let to = dst.join(&name);
        let kind = match entry.file_type() {
            Ok(k) => k,
            Err(_) => continue,
        };
        if kind.is_symlink() {
            continue;              // a link may point outside the setup; the scratch copy does not follow it
        }
        if kind.is_dir() {
            copy_tree(&from, &to)?;
        } else if kind.is_file() {
            fs::copy(&from, &to).map_err(|e| format!("cannot copy {} to {}: {}", from.display(), to.display(), e))?;
            #[cfg(unix)]
            {
                if let Ok(meta) = fs::metadata(&from) {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = fs::set_permissions(&to, fs::Permissions::from_mode(meta.permissions().mode()));
                }
            }
        }
    }
    Ok(())
}

struct Workspace {
    dir: PathBuf,
    /// The root of the scratch copy, removed after the task. None when the task works in the setup folder itself.
    scratch: Option<PathBuf>,
}

fn prepare_workspace(ctx: &Ctx, run_id: &str, task: &Task) -> Result<Workspace, String> {
    let repo = task.repo.canonicalize().unwrap_or_else(|_| task.repo.clone());
    if !task.copy {
        let dir = if task.cwd.is_empty() { repo.clone() } else { repo.join(&task.cwd) };
        if !dir.is_dir() {
            return Err(format!("setup cwd {} is not a folder", dir.display()));
        }
        return Ok(Workspace { dir, scratch: None });
    }
    let root = ctx.root.join("eval").join("work").join(safe_name(run_id)).join(safe_name(&task.id));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).map_err(|e| format!("cannot make {}: {}", root.display(), e))?;
    copy_tree(&repo, &root)?;
    let dir = if task.cwd.is_empty() { root.clone() } else { root.join(&task.cwd) };
    if !dir.is_dir() {
        return Err(format!("copied setup has no cwd {}", dir.display()));
    }
    Ok(Workspace { dir, scratch: Some(root) })
}

fn safe_name(text: &str) -> String {
    text.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '-' }).collect()
}

fn task_text(task: &Task, workdir: &Path) -> String {
    format!("This is an automated evaluation task. The project is at {}. Ignore any other working directory named in \
your system prompt; for this task it is {}. Work only inside that folder. The file and run tools use it as their \
working directory; if you set cwd, use that path. The check command that judges the result is `{}`. Do not start a \
cluster or a subagent of your own; do the task here and then stop.\n\nTask: {}",
            workdir.display(), workdir.display(), task.check, task.prompt)
}

#[derive(Default)]
struct CheckRun {
    passed: bool,
    output: String,
}

fn run_check(ctx: &Ctx, run_id: &str, task: &Task, dir: &Path) -> CheckRun {
    let tmp = ctx.root.join("eval").join("tmp");
    let _ = fs::create_dir_all(&tmp);
    let log = tmp.join(format!("{}-{}.log", safe_name(run_id), safe_name(&task.id)));
    let out = match fs::File::create(&log) {
        Ok(f) => f,
        Err(e) => return CheckRun { passed: false, output: format!("cannot write the check log: {}", e) },
    };
    let err = match out.try_clone() {
        Ok(f) => f,
        Err(e) => return CheckRun { passed: false, output: format!("cannot write the check log: {}", e) },
    };
    let mut cmd = crate::platform::shell(&task.check);
    cmd.current_dir(dir).stdin(Stdio::null()).stdout(Stdio::from(out)).stderr(Stdio::from(err));
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return CheckRun { passed: false, output: format!("the check command could not start: {}", e) },
    };
    let deadline = Instant::now() + Duration::from_secs(task.check_timeout_seconds);
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() >= deadline => {
                timed_out = true;
                let _ = crate::platform::kill_tree(child.id() as u64);
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(25)),
            Err(e) => {
                let _ = fs::remove_file(&log);
                return CheckRun { passed: false, output: format!("the check command failed: {}", e) };
            }
        }
    };
    let output = fs::read_to_string(&log).unwrap_or_default();
    let _ = fs::remove_file(&log);
    let output: String = output.trim().chars().take(4000).collect();
    if timed_out {
        return CheckRun { passed: false, output: format!("check timed out after {} s\n{}", task.check_timeout_seconds, output) };
    }
    let passed = status.map(|s| s.success()).unwrap_or(false);
    CheckRun { passed, output }
}

fn usage_totals(ctx: &Ctx) -> (u64, u64) {
    let v: Value = fs::read_to_string(ctx.root.join("usage.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_else(|| json!({"models": {}}));
    let mut total = (0u64, 0u64);
    if let Some(models) = v["models"].as_object() {
        for e in models.values() {
            total.0 += e["in"].as_u64().unwrap_or(0);
            total.1 += e["out"].as_u64().unwrap_or(0);
        }
    }
    total
}

/// The root config with the unattended policies, and the text to put back when the eval returns.
struct EvalConfig {
    path: PathBuf,
    before: Option<String>,
}

impl Drop for EvalConfig {
    fn drop(&mut self) {
        match &self.before {
            Some(text) => { let _ = fs::write(&self.path, text); }
            None => { let _ = fs::remove_file(&self.path); }
        }
    }
}

fn widen_eval_config(ctx: &Ctx) -> Result<EvalConfig, String> {
    let path = ctx.root.join("config.json");
    let before = fs::read_to_string(&path).ok();
    let mut cfg: Value = before.as_deref().and_then(|s| serde_json::from_str(s).ok()).unwrap_or_else(|| ctx.cfg.clone());
    cfg["edit_policy"] = json!("all");
    cfg["run_policy"] = json!("all");
    cfg["sandbox"] = json!("bwrap");
    fs::write(&path, serde_json::to_string_pretty(&cfg).map_err(|e| format!("cannot encode the eval config: {}", e))?)
        .map_err(|e| format!("cannot write {}: {}", path.display(), e))?;
    Ok(EvalConfig { path, before })
}

fn eval_context(ctx: &Ctx, setup: &str) -> Ctx {
    let mut run = ctx.clone();
    // An eval is unattended: a change must happen without a person at the interface. The eval folder is a scratch copy.
    run.cfg["edit_policy"] = json!("all");
    // The task's own check and the file actions need commands outside the read-only safe list. The sandbox keeps the
    // rest of the machine read-only, makes only the task folder writable and cuts the network. Both setups get the
    // same policy, so the table measures the setup and not the permission prompt.
    run.cfg["run_policy"] = json!("all");
    run.cfg["sandbox"] = json!("bwrap");
    run.cfg["auto_learn"] = json!(false);
    run.cfg["auto_skills"] = json!(false);
    // Every search belongs to the model under test; no hidden scout on the weak model.
    run.cfg["delegate"] = json!("off");
    if setup == "cluster" {
        // The small model gets a low thinking budget. The baseline spent 94.8 % of its output on text that was not
        // stored; this is the lever.
        if run.cfg["weak"].is_object() {
            run.cfg["weak"]["effort"] = json!("low");
        }
    }
    if setup == "large-subagents" {
        let main = run.cfg["main"].clone();
        let mut roles = run.cfg["roles"].clone();
        if !roles.is_object() {
            roles = json!({});
        }
        roles["worker"] = main.clone();
        roles["judge"] = main;
        roles["router"] = json!(null);
        run.cfg["roles"] = roles;
    }
    run
}

fn setup_model(ctx: &Ctx, setup: &str) -> String {
    let model = match setup {
        "cluster" => &ctx.cfg["weak"],
        _ => &ctx.cfg["main"],
    };
    model["model"].as_str().unwrap_or("?").to_string()
}

/// The number of workers in the eval cluster profile.
fn eval_profile_workers(ctx: &Ctx) -> Result<usize, String> {
    let path = ctx.root.join("clusters").join("profiles").join(format!("{}.json", CLUSTER_PROFILE));
    let text = fs::read_to_string(&path).map_err(|e| format!("cannot read {}: {}", path.display(), e))?;
    let profile: Value = serde_json::from_str(&text).map_err(|e| format!("cannot read {}: {}", path.display(), e))?;
    Ok(profile["workers"].as_array().map_or(0, Vec::len))
}

/// One-part tasks go straight to the large model. The cluster only starts at three parts.
fn route_setup<'a>(setup: &'a str, workers: usize) -> &'a str {
    if setup == "cluster" && workers < MIN_CLUSTER_WORKERS { "large" } else { setup }
}

fn ensure_cluster_profile(ctx: &Ctx) -> Result<(), String> {
    let dir = ctx.root.join("clusters").join("profiles");
    fs::create_dir_all(&dir).map_err(|e| format!("cannot make {}: {}", dir.display(), e))?;
    let profile = json!({
        "name": CLUSTER_PROFILE,
        "workers": [{"name": "coder", "role": "worker", "slot": "small",
                     "model": "weak",
                     "task": "Do the task in the project. Read the project first, change only what the task needs, and report the evidence."}],
        "validator": {"name": "check", "role": "judge", "slot": "small-check",
                      "model": "weak",
                      "task": "Check the coder's evidence against the task. Answer PASS or FAIL with one reason."},
        "max_models": 2, "max_depth": 1, "max_steps": 8, "retries": 0, "single_model": true, "worktree": false
    });
    fs::write(dir.join(format!("{}.json", CLUSTER_PROFILE)), serde_json::to_string_pretty(&profile).unwrap())
        .map_err(|e| format!("cannot write the eval cluster profile: {}", e))
}

fn run_one(ctx: &Ctx, setup: &str, task: &Task, workdir: &Path) -> Result<String, String> {
    let prompt = task_text(task, workdir);
    match setup {
        "large" => {
            let mut chat = crate::backend::Chat::new();
            chat.turn_with(ctx, &prompt, "", &mut |_| {})
        }
        "large-subagents" => {
            let spec = crate::agents::Spec {
                name: "eval".into(), role: "orchestrator".into(), thinking: "high".into(), goal: prompt,
                max_sub_agents: 2, max_parallel: 1, max_steps: task.max_steps.min(12), budget_eur: 0.0,
                model: String::new(), verify: true, language: "en".into(), mcp: Vec::new(),
            };
            crate::agents::run(ctx, &spec)
        }
        "cluster" => {
            crate::clusters::orchestrate(ctx, &prompt, Some(CLUSTER_PROFILE), false)
        }
        other => Err(format!("unknown setup {} (use {})", other, SETUPS.join(", "))),
    }
}

fn next_run_id(ctx: &Ctx, setup: &str) -> String {
    let base = now_ms();
    let dir = ctx.root.join("eval").join("runs");
    for n in 0..1000u32 {
        let id = if n == 0 { format!("{}-{}", setup, base) } else { format!("{}-{}-{}", setup, base, n) };
        if !dir.join(format!("{}.json", id)).exists() {
            return id;
        }
    }
    format!("{}-{}-{}", setup, base, std::process::id())
}

fn run_setup(ctx: &Ctx, tasks_path: &str, setup: &str, tasks: &[Task]) -> Result<SetupResult, String> {
    if !SETUPS.contains(&setup) {
        return Err(format!("unknown setup {} (use {} or all)", setup, SETUPS.join(", ")));
    }
    if setup == "cluster" {
        ensure_cluster_profile(ctx)?;
    }
    // A cluster only pays off with at least three independent parts. The eval profile has one worker, so the
    // one-part tasks in this runner go down the large path and the cluster arm measures that route.
    let route = if setup == "cluster" { route_setup(setup, eval_profile_workers(ctx)?) } else { setup };
    let run_id = next_run_id(ctx, setup);
    let run_ctx = eval_context(ctx, route);
    let model = setup_model(&run_ctx, route);
    let mut out = SetupResult { setup: setup.to_string(), run_id: run_id.clone(), model: model.clone(), tasks: Vec::new() };
    let mut stored: Vec<Value> = Vec::new();
    for task in tasks {
        let start = Instant::now();
        let (usage_before, eur_before) = (usage_totals(&run_ctx), crate::cost::total_eur(&run_ctx));
        let workspace = match prepare_workspace(&run_ctx, &run_id, task) {
            Ok(w) => w,
            Err(e) => {
                out.tasks.push(TaskResult { id: task.id.clone(), error: e.clone(), wall_ms: start.elapsed().as_millis(), ..Default::default() });
                stored.push(json!({"id": task.id, "passed": false, "tokens_in": 0, "tokens_out": 0, "eur": 0.0,
                                   "wall_ms": start.elapsed().as_millis() as u64, "check_output": "", "answer": "", "error": e}));
                continue;
            }
        };
        let answer = with_task_cwd(&workspace.dir, || run_one(&run_ctx, route, task, &workspace.dir));
        let check = run_check(&run_ctx, &run_id, task, &workspace.dir);
        if let Some(scratch) = &workspace.scratch {
            let _ = fs::remove_dir_all(scratch);
        }
        let (usage_after, eur_after) = (usage_totals(&run_ctx), crate::cost::total_eur(&run_ctx));
        let mut result = TaskResult {
            id: task.id.clone(),
            passed: check.passed,
            tokens_in: usage_after.0.saturating_sub(usage_before.0),
            tokens_out: usage_after.1.saturating_sub(usage_before.1),
            eur: (eur_after - eur_before).max(0.0),
            wall_ms: start.elapsed().as_millis(),
            check_output: check.output,
            answer: String::new(),
            error: String::new(),
        };
        match answer {
            Ok(text) => result.answer = text.chars().take(4000).collect(),
            Err(e) => result.error = e,
        }
        stored.push(json!({
            "id": task.id,
            "passed": result.passed,
            "tokens_in": result.tokens_in,
            "tokens_out": result.tokens_out,
            "eur": result.eur,
            "wall_ms": result.wall_ms as u64,
            "check_output": result.check_output,
            "answer": result.answer,
            "error": result.error
        }));
        out.tasks.push(result);
    }
    let record = json!({
        "schema": 1,
        "id": run_id,
        "setup": setup,
        "route": route,
        "started": crate::sessions::stamp(now()),
        "tasks_path": tasks_path,
        "model": model,
        "dry": false,
        "prompts": tasks.iter().map(|t| json!({"id": t.id, "prompt": t.prompt, "check": t.check})).collect::<Vec<_>>(),
        "results": stored
    });
    save_run(ctx, &run_id, &record)?;
    Ok(out)
}

fn save_run(ctx: &Ctx, id: &str, record: &Value) -> Result<PathBuf, String> {
    let dir = ctx.root.join("eval").join("runs");
    fs::create_dir_all(&dir).map_err(|e| format!("cannot make {}: {}", dir.display(), e))?;
    let path = dir.join(format!("{}.json", safe_name(id)));
    fs::write(&path, serde_json::to_string_pretty(record).unwrap()).map_err(|e| format!("cannot write {}: {}", path.display(), e))?;
    Ok(path)
}

pub fn load_run(ctx: &Ctx, id: &str) -> Result<Value, String> {
    if !safe_id(id) {
        return Err(format!("{} is no eval run id", id));
    }
    let path = ctx.root.join("eval").join("runs").join(format!("{}.json", id));
    let text = fs::read_to_string(&path).map_err(|_| format!("no eval run {} under {}. Run penelope eval first, or name a run id from docs/RUNS.md.", id, path.display()))?;
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("eval run {} is not JSON: {}", id, e))?;
    Ok(v)
}

fn table(results: &[SetupResult]) -> String {
    let mut out = format!("{:<16} {:>5} {:>6} {:>11} {:>11} {:>9} {:>8}\n", "setup", "tasks", "passed", "tokens in", "tokens out", "euro", "wall");
    for r in results {
        let (tin, tout) = r.tokens();
        out.push_str(&format!("{:<16} {:>5} {:>6} {:>11} {:>11} {:>9.4} {:>7.1}s\n",
                              r.setup, r.tasks.len(), r.passed(), tin, tout, r.eur(), r.wall_ms() as f64 / 1000.0));
    }
    out.push('\n');
    for r in results {
        for t in &r.tasks {
            let state = if t.passed { "pass" } else { "FAIL" };
            out.push_str(&format!("  {} · {}: {} · {} in / {} out · {:.4} EUR · {:.1}s\n",
                                  r.setup, t.id, state, t.tokens_in, t.tokens_out, t.eur, t.wall_ms as f64 / 1000.0));
            if !t.error.is_empty() {
                out.push_str(&format!("    error: {}\n", t.error.chars().take(240).collect::<String>()));
            }
            if !t.passed && !t.check_output.is_empty() {
                out.push_str(&format!("    check: {}\n", t.check_output.replace('\n', "\n           ").chars().take(500).collect::<String>()));
            }
        }
    }
    out.push_str("\nrun ids (penelope replay ID --model MODEL re-runs one read-only):\n");
    for r in results {
        out.push_str(&format!("  {:<16} {}\n", r.setup, r.run_id));
    }
    out
}

fn append_runs_doc(path: &Path, tasks_path: &str, results: &[SetupResult]) -> Result<(), String> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent).map_err(|e| format!("cannot make {}: {}", parent.display(), e))?;
    }
    let mut text = if path.is_file() && fs::metadata(path).map(|m| m.len() > 0).unwrap_or(false) {
        String::new()
    } else {
        String::from("# Penelope: real runs\n\nEvery number below comes from a command on this machine. Runs with a fake endpoint in the test suite never write here.\n")
    };
    let setups: Vec<&str> = results.iter().map(|r| r.setup.as_str()).collect();
    text.push_str(&format!("\n## Eval run {} · {}\n\n", crate::sessions::stamp(now()), setups.join(", ")));
    text.push_str(&format!("`penelope eval {} --setup {}`\n\n", tasks_path, if setups.len() == SETUPS.len() { "all".to_string() } else { setups.join(",") }));
    text.push_str(&format!("Tasks: `{}`. Models: {}.\n\n", tasks_path,
                           results.iter().map(|r| format!("{} = {}", r.setup, r.model)).collect::<Vec<_>>().join(", ")));
    text.push_str("| setup | tasks | passed | tokens in | tokens out | euro | wall | run id |\n|---|---|---|---|---|---|---|---|\n");
    for r in results {
        let (tin, tout) = r.tokens();
        text.push_str(&format!("| {} | {} | {} | {} | {} | {:.4} | {:.1}s | `{}` |\n",
                               r.setup, r.tasks.len(), r.passed(), tin, tout, r.eur(), r.wall_ms() as f64 / 1000.0, r.run_id));
    }
    text.push_str("\nPassed means the task's own check command exited 0 in the task folder. Tokens and euro are the delta of `.penelope/usage.json`\n");
    text.push_str("during the task; euro uses the prices in `config.json` and marks an unpriced model as 0.0000 rather than guessing.\n");
    fs::OpenOptions::new().create(true).append(true).open(path).map_err(|e| format!("cannot open {}: {}", path.display(), e))?
        .write_all(text.as_bytes()).map_err(|e| format!("cannot write {}: {}", path.display(), e))?;
    Ok(())
}

/// Runs the named setup (or all of them), prints a table and appends the measured table to `runs_path`.
pub fn eval(ctx: &Ctx, tasks_path: &str, setup: &str, runs_path: Option<&Path>) -> Result<String, String> {
    // A Claude CLI part reads the config file again in its MCP server. The in-memory change of eval_context is not
    // enough for that process, so the unattended policies are written to the scratch root for the run and restored
    // when eval returns, also on an error. The root must be a scratch root; the sandbox keeps the machine read-only.
    let _config = widen_eval_config(ctx)?;
    let tasks = parse_tasks(Path::new(tasks_path))?;
    let list: Vec<&str> = if setup == "all" { SETUPS.to_vec() } else { vec![setup] };
    for name in &list {
        if !SETUPS.contains(name) {
            return Err(format!("unknown setup {} (use {} or all)", name, SETUPS.join(", ")));
        }
    }
    let mut results = Vec::new();
    for name in list {
        results.push(run_setup(ctx, tasks_path, name, &tasks)?);
    }
    let mut out = format!("{} tasks in {} setup{}. No answer of a model is stored in this table; the check decides.\n\n",
                          tasks.len(), results.len(), if results.len() == 1 { "" } else { "s" });
    out.push_str(&table(&results));
    if let Some(path) = runs_path {
        match append_runs_doc(path, tasks_path, &results) {
            Ok(()) => out.push_str(&format!("\nwritten: {}\n", path.display())),
            Err(e) => out.push_str(&format!("\nNOT written: {}\n", e)),
        }
    }
    Ok(out)
}

/// One stored prompt, replayed. The gate names the refused tool, so the output shows that read-only was enforced.
#[derive(Default)]
pub struct ReplayResult {
    pub id: String,
    pub answer: String,
    pub error: String,
    pub refused: Vec<String>,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub eur: f64,
    pub wall_ms: u128,
}

/// The gate of replay. Only read-only tools reach the dispatcher; `run`, `write`, `edit`, `job_start`, `screen`,
/// `mcp_call`, `cluster_start` and every forged tool are refused by name before they can do anything. The scout role
/// allows the six read-only tools and `run`; the gate is what keeps `run` out.
fn replay_gate(ctx: &Ctx, refused: &RefCell<Vec<String>>, name: &str, args: &Value) -> (String, bool) {
    if !REPLAY_TOOLS.contains(&name) {
        refused.borrow_mut().push(name.to_string());
        return (format!("NOT called. replay is read-only: the tool {} is refused. Read-only tools: {}. No file, command, job or cluster was touched.",
                        name, REPLAY_TOOLS.join(", ")), false);
    }
    crate::tools::dispatch_retry(ctx, "scout", name, args)
}

fn replay_tools(ctx: &Ctx) -> Vec<Value> {
    crate::mcp::tool_specs_for(ctx, "scout").as_array().cloned().unwrap_or_default()
        .into_iter().filter(|t| REPLAY_TOOLS.contains(&t["name"].as_str().unwrap_or(""))).collect()
}

/// Re-runs the stored prompts of one eval run against `model_name`. It does not enter a task folder, run a check or
/// write an eval run. It records the token use of the replay itself, as every model call does.
pub fn replay(ctx: &Ctx, id: &str, model_name: &str) -> Result<String, String> {
    let name = model_name.trim();
    if name.is_empty() {
        return Err("replay needs --model MODEL".to_string());
    }
    let run = load_run(ctx, id)?;
    let kind = crate::llm::kind_of(&ctx.cfg["main"]).to_string();
    if !crate::llm::is_api(&kind) {
        return Err(format!("replay needs an HTTP model setting for the main role (openai or anthropic), got {}. \
The claude command cannot be held to the read-only gate from here.", kind));
    }
    let mut model = ctx.cfg["main"].clone();
    model["model"] = json!(name);
    let prompts = run["prompts"].as_array().cloned().ok_or_else(|| format!("eval run {} stores no prompts", id))?;
    if prompts.is_empty() {
        return Err(format!("eval run {} stores no prompt", id));
    }
    let tools = replay_tools(ctx);
    let tools_text = REPLAY_TOOLS.join(", ");
    let mut out = String::from("REPLAY IS READ-ONLY. No write, edit, run, job, screen, MCP, cluster or check call can happen. No task folder is entered.\n");
    out.push_str(&format!("run {} · model {} · {} prompt{}\n\n", id, name, prompts.len(), if prompts.len() == 1 { "" } else { "s" }));
    for p in prompts {
        let task_id = p["id"].as_str().unwrap_or("?").to_string();
        let prompt = p["prompt"].as_str().unwrap_or("").to_string();
        if prompt.trim().is_empty() {
            continue;
        }
        let start = Instant::now();
        let eur_before = crate::cost::total_eur(ctx);
        let refused = RefCell::new(Vec::new());
        let system = format!("You replay one recorded evaluation prompt for Penelope. This is a read-only replay: the only tools are {}. \
A write, edit, run, job, screen, MCP or cluster call is refused before it does anything, and no task folder is entered. \
Answer the task with facts you can read, or say what you would do. Never claim a change that you did not make.\n\nRecorded prompt:\n{}", tools_text, prompt);
        let mut history = vec![crate::llm::user_message(&prompt)];
        let mut usage = crate::llm::Usage::default();
        let answer = crate::llm::run_loop(&model, &system, &mut history, &tools, 8, None, &mut usage,
            &mut |call| replay_gate(ctx, &refused, &call.name, &call.args), &mut |_| {});
        crate::cost::record(ctx, "replay", name, usage.tokens_in, usage.tokens_out);
        let result = match answer {
            Ok(text) => ReplayResult { id: task_id.clone(), answer: text.chars().take(4000).collect(), refused: refused.into_inner(),
                                       tokens_in: usage.tokens_in, tokens_out: usage.tokens_out,
                                       eur: (crate::cost::total_eur(ctx) - eur_before).max(0.0), wall_ms: start.elapsed().as_millis(), ..Default::default() },
            Err(e) => ReplayResult { id: task_id.clone(), error: e, refused: refused.into_inner(),
                                     tokens_in: usage.tokens_in, tokens_out: usage.tokens_out,
                                     eur: (crate::cost::total_eur(ctx) - eur_before).max(0.0), wall_ms: start.elapsed().as_millis(), ..Default::default() },
        };
        out.push_str(&format!("--- {} · {} in / {} out · {:.4} EUR · {:.1}s\n",
                              result.id, result.tokens_in, result.tokens_out, result.eur, result.wall_ms as f64 / 1000.0));
        if !result.refused.is_empty() {
            out.push_str(&format!("refused: {}\n", result.refused.join(", ")));
        }
        if !result.error.is_empty() {
            out.push_str(&format!("error: {}\n", result.error));
        }
        if !result.answer.is_empty() {
            out.push_str(&answer_text(&result.answer));
            out.push('\n');
        }
    }
    Ok(out)
}

fn answer_text(text: &str) -> String {
    let mut out = String::new();
    for line in text.lines() {
        out.push_str("  ");
        out.push_str(line);
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("penelope-eval-{}-{}-{:?}", name, std::process::id(), std::thread::current().id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn ctx(name: &str) -> (Ctx, PathBuf) {
        let d = dir(name);
        (crate::config::load(&d.join(".penelope")), d)
    }

    fn repo(d: &Path) -> PathBuf {
        let r = d.join("repo");
        fs::create_dir_all(&r).unwrap();
        fs::write(r.join("README.md"), "# task repo\n").unwrap();
        r
    }

    fn endpoint(ctx: &mut Ctx, address: &str, role: &str) {
        ctx.cfg[role] = json!({"kind": "openai", "base_url": format!("{}/v1", address), "model": "test-model"});
    }

    fn tasks(d: &Path, repo: &Path, check: &str, prompt: &str) -> PathBuf {
        let p = d.join("tasks.jsonl");
        let line = json!({"id": "one", "prompt": prompt, "setup": {"repo": repo.to_string_lossy()}, "check": check});
        fs::write(&p, format!("{}\n", line)).unwrap();
        p
    }

    fn reply(text: &str, tin: u64, tout: u64) -> (u16, String) {
        (200, json!({"choices": [{"finish_reason": "stop", "message": {"role": "assistant", "content": text}}],
                     "usage": {"prompt_tokens": tin, "completion_tokens": tout}}).to_string())
    }

    fn tool_call(name: &str, args: Value, tin: u64, tout: u64) -> (u16, String) {
        (200, json!({"choices": [{"finish_reason": "tool_calls", "message": {"role": "assistant", "content": null, "tool_calls": [
            {"id": "call_1", "type": "function", "function": {"name": name, "arguments": args.to_string()}}]}}],
                     "usage": {"prompt_tokens": tin, "completion_tokens": tout}}).to_string())
    }

    #[test]
    fn the_task_schema_is_read_and_a_broken_line_is_refused() {
        let d = dir("schema");
        let repo = repo(&d);
        let file = tasks(&d, &repo, "test -f README.md", "Read the project.");
        let parsed = parse_tasks(&file).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].id, "one");
        assert!(parsed[0].repo.is_dir() && parsed[0].copy && parsed[0].check_timeout_seconds == 300);
        // The example shipped in the repository reads too.
        let example = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/eval/tasks.jsonl");
        let example = parse_tasks(&example).unwrap();
        assert_eq!(example[0].id, "write-answer");
        let bad = d.join("bad.jsonl");
        fs::write(&bad, "{\"id\":\"x\",\"prompt\":\"do it\",\"setup\":{\"repo\":\".\"}}\n").unwrap();
        assert!(parse_tasks(&bad).unwrap_err().contains("check command is missing"));
        fs::write(&bad, "{\"id\":\"x\",\"prompt\":\"do it\",\"setup\":{\"repo\":\".\",\"cwd\":\"../out\"},\"check\":\"true\"}\n").unwrap();
        assert!(parse_tasks(&bad).unwrap_err().contains("may not leave"));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn the_large_setup_runs_a_task_on_a_fake_endpoint_and_the_table_is_written() {
        let (mut ctx, d) = ctx("large");
        let repo = repo(&d);
        let file = tasks(&d, &repo, "grep -qx hello answer.txt", "Create answer.txt with the single word hello.");
        let (address, _seen) = crate::llm::fake::serve(vec![
            tool_call("write", json!({"path": "answer.txt", "text": "hello\n"}), 100, 10),
            reply("created answer.txt", 120, 5),
        ]);
        endpoint(&mut ctx, &address, "main");
        ctx.cfg["prices"] = json!({"test-model": [1.0, 2.0]});
        ctx.cfg["eur_per_usd"] = json!(0.92);
        let runs = d.join("RUNS.md");
        let text = eval(&ctx, file.to_str().unwrap(), "large", Some(&runs)).unwrap();
        assert!(text.contains("large") && text.contains("one: pass") && text.contains("220 in / 15 out"), "{}", text);
        assert!(text.contains("large-") && text.contains("written:"), "{}", text);
        let doc = fs::read_to_string(&runs).unwrap();
        assert!(doc.contains("| large | 1 | 1 | 220 | 15 |") && doc.contains("`large-"), "{}", doc);
        // The default cwd in mcp.rs puts the write into the scratch copy, so the task check passed there.
        assert!(!d.join("answer.txt").exists());
        let run = load_run(&ctx, text.lines().find(|l| l.contains("large-")).unwrap().split_whitespace().last().unwrap()).unwrap();
        assert_eq!(run["setup"], "large");
        assert_eq!(run["results"][0]["passed"], true);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn subagents_run_one_worker_and_one_judge_on_the_same_strong_model() {
        let (mut ctx, d) = ctx("subagents");
        let repo = repo(&d);
        let file = tasks(&d, &repo, "test -f README.md", "Say what the project is.");
        let (address, seen) = crate::llm::fake::serve(vec![
            reply("plan: read the README", 10, 1),
            reply("worker read the README", 20, 2),
            reply("PASS the worker read it", 30, 3),
            reply("report done", 40, 4),
        ]);
        endpoint(&mut ctx, &address, "main");
        ctx.cfg["prices"] = json!({"test-model": [1.0, 2.0]});
        let parsed = parse_tasks(&file).unwrap();
        let result = run_setup(&ctx, file.to_str().unwrap(), "large-subagents", &parsed).unwrap();
        assert_eq!((result.passed(), result.tokens(), result.tasks[0].passed), (1, (100, 10), true));
        assert!(result.tasks[0].eur > 0.0, "{:?}", result.tasks[0]);
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 4);
        let systems: Vec<String> = seen.iter().map(|s| s.body["messages"][0]["content"].as_str().unwrap_or("").to_string()).collect();
        assert!(systems.iter().any(|s| s.contains("worker of a Penelope agent run")) && systems.iter().any(|s| s.contains("judge of a Penelope agent run")), "{:?}", systems);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn the_eval_widens_the_root_config_and_restores_it() {
        let (ctx, d) = ctx("config-guard");
        let path = ctx.root.join("config.json");
        fs::write(&path, r#"{"edit_policy":"ask","run_policy":"safe","sandbox":"off"}"#).unwrap();
        {
            let _guard = widen_eval_config(&ctx).unwrap();
            let now: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
            assert_eq!((now["edit_policy"].as_str(), now["run_policy"].as_str(), now["sandbox"].as_str()),
                       (Some("all"), Some("all"), Some("bwrap")));
        }
        let after: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!((after["edit_policy"].as_str(), after["run_policy"].as_str(), after["sandbox"].as_str()),
                   (Some("ask"), Some("safe"), Some("off")));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn the_cluster_eval_lowers_the_weak_effort() {
        let (ctx, d) = ctx("weak-effort");
        let cluster = eval_context(&ctx, "cluster");
        assert_eq!(cluster.cfg["weak"]["effort"].as_str(), Some("low"));
        let large = eval_context(&ctx, "large");
        assert_ne!(large.cfg["weak"]["effort"].as_str(), Some("low"));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn an_eval_runs_unattended_in_the_sandbox_without_changing_the_caller() {
        let (ctx, d) = ctx("unattended");
        let run = eval_context(&ctx, "large");
        assert_eq!((run.cfg["edit_policy"].as_str(), run.cfg["run_policy"].as_str(), run.cfg["sandbox"].as_str()),
                   (Some("all"), Some("all"), Some("bwrap")));
        assert_eq!(run.cfg["auto_learn"], json!(false));
        assert_eq!(run.cfg["auto_skills"], json!(false));
        assert_eq!(ctx.cfg["run_policy"].as_str(), Some("safe"), "the caller keeps its policy");
        assert_eq!(ctx.cfg["sandbox"].as_str(), Some("off"), "the caller keeps its sandbox");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn a_one_part_cluster_setup_uses_the_large_model() {
        let (mut ctx, d) = ctx("cluster-large");
        let repo = repo(&d);
        let file = tasks(&d, &repo, "test -f README.md", "Say what the project is.");
        let (address, seen) = crate::llm::fake::serve(vec![reply("the task is done", 30, 3)]);
        endpoint(&mut ctx, &address, "main");
        ctx.cfg["prices"] = json!({"test-model": [1.0, 2.0]});
        let parsed = parse_tasks(&file).unwrap();
        let result = run_setup(&ctx, file.to_str().unwrap(), "cluster", &parsed).unwrap();
        assert_eq!((result.passed(), result.tokens()), (1, (30, 3)));
        assert_eq!(result.model, "test-model");
        assert_eq!(eval_profile_workers(&ctx).unwrap(), 1);
        assert!(ctx.root.join("clusters/profiles").join(format!("{}.json", CLUSTER_PROFILE)).is_file());
        let profile: Value = serde_json::from_str(&fs::read_to_string(ctx.root.join("clusters/profiles").join(format!("{}.json", CLUSTER_PROFILE))).unwrap()).unwrap();
        assert_eq!(profile["single_model"], true);
        let run: Value = serde_json::from_str(&fs::read_to_string(ctx.root.join("eval/runs").join(format!("{}.json", result.run_id))).unwrap()).unwrap();
        assert_eq!((run["setup"].as_str(), run["route"].as_str()), (Some("cluster"), Some("large")));
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1, "a one-part task must make one model call, not a cluster");
        let systems: Vec<String> = seen.iter().map(|s| s.body["messages"][0]["content"].as_str().unwrap_or("").to_string()).collect();
        assert!(systems.iter().all(|s| !s.contains("part of the Penelope cluster")), "a one-part task must not start a cluster: {:?}", systems);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn the_eval_profile_worker_count_builds_the_route() {
        let (ctx, d) = ctx("profile-workers");
        fs::create_dir_all(ctx.root.join("clusters/profiles")).unwrap();
        let path = ctx.root.join("clusters/profiles").join(format!("{}.json", CLUSTER_PROFILE));
        fs::write(&path, json!({"workers": [{"name": "a"}, {"name": "b"}, {"name": "c"}]}).to_string()).unwrap();
        assert_eq!(eval_profile_workers(&ctx).unwrap(), 3);
        assert_eq!(route_setup("cluster", 3), "cluster");
        assert_eq!(route_setup("cluster", 2), "large");
        assert_eq!(route_setup("large", 1), "large");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn replay_refuses_a_writing_tool_and_touches_no_file() {
        let (mut ctx, d) = ctx("replay");
        let (address, seen) = crate::llm::fake::serve(vec![
            tool_call("write", json!({"path": d.join("forbidden.txt").to_string_lossy(), "text": "nope"}), 50, 5),
            reply("I did not write; replay is read-only.", 60, 6),
        ]);
        endpoint(&mut ctx, &address, "main");
        ctx.cfg["prices"] = json!({"test-model": [1.0, 2.0]});
        let id = "large-test";
        let record = json!({"schema": 1, "id": id, "setup": "large", "started": "2026-11-25 10:00", "model": "test-model", "dry": false,
                            "prompts": [{"id": "one", "prompt": "Create forbidden.txt with the text nope.", "check": "test -f forbidden.txt"}],
                            "results": []});
        save_run(&ctx, id, &record).unwrap();
        let text = replay(&ctx, id, "test-model").unwrap();
        assert!(text.starts_with("REPLAY IS READ-ONLY") && text.contains("refused: write") && text.contains("110 in / 11 out"), "{}", text);
        assert!(!d.join("forbidden.txt").exists());
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[0].body["messages"][1]["content"], "Create forbidden.txt with the text nope.");
        let tool_result = seen[1].body["messages"].as_array().unwrap().iter().find(|m| m["role"] == "tool").unwrap();
        assert!(tool_result["content"].as_str().unwrap().contains("replay is read-only"), "{}", tool_result);
        assert!(replay(&ctx, "no-such-run", "test-model").unwrap_err().contains("no eval run"));
        let _ = fs::remove_dir_all(&d);
    }
}
