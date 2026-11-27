//! The executable checker for code tasks. Eurykleia checks the shape of a tool call; the checker runs the code.
//!
//! Before the final answer on a code task the model writes a reproduction test from the issue text and calls
//! `verify_fix`. The checker adds a throwaway git worktree at HEAD, copies the test in and runs it there: it must fail.
//! Then it applies the change of the working tree (tracked diff plus untracked files, without the test) and runs the
//! test again: it must pass. Then the module tests run on the changed copy: they must pass, or must already fail on
//! HEAD. The working tree of the user is never written.
//!
//! Best-of-N: N candidates (default 3) from the weaker model as SEARCH/REPLACE blocks, plus the working tree. They are
//! sorted by diff size, smallest first, and the first that passes wins. The test on HEAD runs once for all of them.
//!
//! Config `check`: `mode` gate (default: one reminder before a final answer that follows a code edit without a check),
//! report (only the verdict line under the answer) or off; `module_tests` (empty: `test_command`); `run` (command for
//! the test, `{test}` is the path; empty: python3, node, sh, ruby by extension); `candidates` (N, 1 to 8); `models`
//! (roles or model objects for the candidates, cycled; default `["weak"]`); `timeout` (seconds per command); `nudges`.
//! Every verdict goes to `.penelope/check/log.jsonl`, the last check to `last.json`.

use crate::config::Ctx;
use serde_json::{json, Value};
use std::cell::Cell;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// File endings that make an edit a code edit. A change to Markdown or JSON alone does not arm the gate.
const CODE: &[&str] = &["py", "rs", "js", "mjs", "cjs", "ts", "tsx", "jsx", "go", "java", "kt", "c", "h", "cc", "cpp", "hpp",
                        "cs", "rb", "php", "swift", "scala", "sh", "lua", "ex", "exs", "hs", "ml", "zig", "dart", "r", "jl"];

pub struct Settings {
    pub mode: String,
    pub module_tests: String,
    pub run: String,
    pub candidates: usize,
    pub models: Vec<Value>,
    pub timeout: u64,
    pub nudges: usize,
}

pub fn settings(ctx: &Ctx) -> Settings {
    let c = &ctx.cfg["check"];
    let mode = match c["mode"].as_str().or(c.as_str()).unwrap_or("gate") {
        "off" | "false" | "no" | "0" => "off",
        "report" => "report",
        _ => "gate",
    };
    let module = c["module_tests"].as_str().unwrap_or("").trim();
    let module = if module.is_empty() { ctx.cfg["test_command"].as_str().unwrap_or("").trim() } else { module };
    let models: Vec<Value> = c["models"].as_array().cloned().unwrap_or_default().into_iter()
        .filter_map(|m| match m {
            Value::String(role) => Some(ctx.cfg[role.as_str()].clone()).filter(|v| v.is_object()),
            Value::Object(_) => Some(m),
            _ => None,
        }).collect();
    Settings {
        mode: mode.to_string(),
        module_tests: module.to_string(),
        run: c["run"].as_str().unwrap_or("").trim().to_string(),
        candidates: c["candidates"].as_u64().unwrap_or(3).clamp(1, 8) as usize,
        models: if models.is_empty() { vec![ctx.cfg["weak"].clone()] } else { models },
        timeout: c["timeout"].as_u64().unwrap_or(300).clamp(5, 3600),
        nudges: c["nudges"].as_u64().unwrap_or(1).min(3) as usize,
    }
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn unique() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    format!("{}-{}-{}", std::process::id(), now_ms(), N.fetch_add(1, Ordering::SeqCst))
}

// ---------------------------------------------------------------------------------------------------------------
// Commands with a time limit.

/// One command run: exit status, whether the time limit ended it, and the last 15 lines of stdout and stderr.
#[derive(Debug, Clone)]
pub struct Run {
    pub ok: bool,
    pub code: i32,
    pub timed_out: bool,
    pub tail: String,
}

fn last_lines(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.trim_end().lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

/// Runs a shell line in `dir`. Output goes to files, so a chatty test cannot block on a full pipe. After `timeout`
/// seconds the process tree is killed and the run counts as failed. `roots` go first on PYTHONPATH: a package installed
/// with `pip install -e` points at the original checkout, and without this the copy would import the user's files.
pub fn run_command_in(command: &str, dir: &Path, roots: &[PathBuf], timeout: u64) -> Run {
    let base = std::env::temp_dir().join(format!("penelope-check-out-{}", unique()));
    let (out_path, err_path) = (base.with_extension("out"), base.with_extension("err"));
    let fail = |e: String| Run { ok: false, code: -1, timed_out: false, tail: e };
    let (Ok(out), Ok(err)) = (fs::File::create(&out_path), fs::File::create(&err_path)) else {
        return fail("the output files could not be created".to_string());
    };
    let mut cmd = crate::platform::shell(command);
    cmd.current_dir(dir).stdin(Stdio::null()).stdout(out).stderr(err).env("PENELOPE_CHECK", "1");
    if !roots.is_empty() {
        let mut paths: Vec<PathBuf> = roots.to_vec();
        if let Some(old) = std::env::var_os("PYTHONPATH") {
            paths.extend(std::env::split_paths(&old));
        }
        if let Ok(joined) = std::env::join_paths(paths) {
            cmd.env("PYTHONPATH", joined);
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return fail(format!("the command could not start: {}", e)),
    };
    let start = Instant::now();
    let (status, timed_out) = loop {
        match child.try_wait() {
            Ok(Some(status)) => break (Some(status), false),
            Ok(None) if start.elapsed() > Duration::from_secs(timeout) => {
                crate::platform::kill_tree(child.id() as u64);
                let _ = child.kill();
                let _ = child.wait();
                break (None, true);
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(40)),
            Err(_) => break (None, false),
        }
    };
    let mut text = fs::read_to_string(&out_path).unwrap_or_default();
    let err = fs::read_to_string(&err_path).unwrap_or_default();
    if !err.trim().is_empty() {
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&err);
    }
    let _ = fs::remove_file(&out_path);
    let _ = fs::remove_file(&err_path);
    let code = status.and_then(|s| s.code()).unwrap_or(-1);
    let mut tail = last_lines(&text, 15);
    if timed_out {
        tail = format!("{}\n(killed after {} s)", tail, timeout).trim().to_string();
    }
    Run { ok: status.map(|s| s.success()).unwrap_or(false), code, timed_out, tail }
}

pub fn run_command(command: &str, dir: &Path, timeout: u64) -> Run {
    run_command_in(command, dir, &[], timeout)
}

// ---------------------------------------------------------------------------------------------------------------
// Git helpers and the throwaway worktree.

fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git").arg("-C").arg(dir).args(args).stdin(Stdio::null()).output()
        .map_err(|e| format!("git could not start: {}", e))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    } else {
        Err(format!("git {}: {}", args.first().unwrap_or(&""), last_lines(&String::from_utf8_lossy(&out.stderr), 3)))
    }
}

/// A detached git worktree at HEAD under the temp folder. Dropping it removes the folder and the worktree entry.
struct Sandbox {
    repo: PathBuf,
    dir: PathBuf,
}

impl Sandbox {
    fn new(repo: &Path) -> Result<Sandbox, String> {
        let dir = std::env::temp_dir().join(format!("penelope-check-{}", unique()));
        git(repo, &["worktree", "add", "--detach", "--quiet", &dir.to_string_lossy(), "HEAD"])?;
        Ok(Sandbox { repo: repo.to_path_buf(), dir })
    }

    /// Stages everything except the test and returns the diff against HEAD with its count of changed lines.
    fn staged_diff(&self, test_rel: &str) -> Result<(usize, String), String> {
        let exclude = format!(":(exclude){}", test_rel);
        git(&self.dir, &["add", "-A", "--", ".", &exclude])?;
        let numstat = git(&self.dir, &["diff", "--cached", "--numstat", "HEAD", "--", ".", &exclude])?;
        let lines = numstat.lines().map(|l| {
            let mut f = l.split('\t');
            f.next().and_then(|a| a.parse::<usize>().ok()).unwrap_or(0) + f.next().and_then(|d| d.parse::<usize>().ok()).unwrap_or(0)
        }).sum();
        let diff = git(&self.dir, &["diff", "--cached", "HEAD", "--", ".", &exclude])?;
        Ok((lines, diff))
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = git(&self.repo, &["worktree", "remove", "--force", &self.dir.to_string_lossy()]);
        let _ = fs::remove_dir_all(&self.dir);
        let _ = git(&self.repo, &["worktree", "prune"]);
    }
}

/// A relative path inside the repository, with forward slashes, or None for an absolute path or one that climbs out.
fn safe_rel(path: &str) -> Option<String> {
    let p = path.trim().replace('\\', "/");
    let p = p.trim_start_matches("./").to_string();
    if p.is_empty() || p.starts_with('/') || p.contains(':') || p.split('/').any(|c| c == "..") {
        return None;
    }
    Some(p)
}

fn copy_into(from: &Path, to: &Path) -> Result<(), String> {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    fs::copy(from, to).map(|_| ()).map_err(|e| format!("copy {}: {}", from.display(), e))
}

// ---------------------------------------------------------------------------------------------------------------
// One check job.

/// Everything one check needs. `top` is the repository root; `sub` the working folder relative to it, where the
/// commands run; `test_rel` the test relative to `top`, `test_arg` the same file relative to the working folder.
pub struct Job {
    top: PathBuf,
    sub: PathBuf,
    test_rel: String,
    test_arg: String,
    run: String,
    module: String,
    timeout: u64,
}

fn default_run(test: &str) -> Option<&'static str> {
    let ext = Path::new(test).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "py" => Some(if cfg!(windows) { "python {test}" } else { "python3 {test}" }),
        "js" | "mjs" | "cjs" => Some("node {test}"),
        "sh" => Some("sh {test}"),
        "rb" => Some("ruby {test}"),
        _ => None,
    }
}

impl Job {
    /// Builds a job from the working folder `cwd`, the test path (relative to `cwd`) and the optional commands.
    pub fn new(cwd: &Path, test: &str, run: &str, module: &str, timeout: u64) -> Result<Job, String> {
        let test_arg = safe_rel(test).ok_or_else(|| format!("the test path {} must be relative and stay inside the project", test))?;
        let top = PathBuf::from(git(cwd, &["rev-parse", "--show-toplevel"]).map_err(|_| {
            "the folder is not inside a git repository. The checker needs git to run the test on the original code.".to_string()
        })?.trim());
        git(&top, &["rev-parse", "--verify", "--quiet", "HEAD"]).map_err(|_| "the repository has no commit yet, so there is no original code to test".to_string())?;
        let canon = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
        let sub = canon(cwd).strip_prefix(canon(&top)).map(Path::to_path_buf).unwrap_or_default();
        let sub_str = sub.to_string_lossy().replace('\\', "/");
        let test_rel = if sub_str.is_empty() { test_arg.clone() } else { format!("{}/{}", sub_str.trim_end_matches('/'), test_arg) };
        if !top.join(&test_rel).is_file() {
            return Err(format!("the test {} does not exist. Write it first, then call the check.", test_arg));
        }
        let run = if run.trim().is_empty() {
            default_run(&test_arg).ok_or_else(|| format!("no default command for {}. Pass run, for example `cargo test repro_issue` or `pytest {{test}}`.", test_arg))?.to_string()
        } else {
            run.trim().to_string()
        };
        let quoted = crate::platform::shell_quote(&test_arg);
        let run = if run.contains("{test}") { run.replace("{test}", &quoted) } else { run };
        Ok(Job { top, sub, test_rel, test_arg, run, module: module.trim().to_string(), timeout })
    }

    fn workdir(&self, sandbox: &Sandbox) -> PathBuf {
        sandbox.dir.join(&self.sub)
    }

    /// Runs a command of the job in the working folder of a sandbox, with the sandbox first on PYTHONPATH.
    fn exec(&self, command: &str, sandbox: &Sandbox) -> Run {
        let work = self.workdir(sandbox);
        let roots = if work == sandbox.dir { vec![work.clone()] } else { vec![work.clone(), sandbox.dir.clone()] };
        run_command_in(command, &work, &roots, self.timeout)
    }

    /// The test on the original code, with the test file copied in.
    fn before(&self) -> Result<Run, String> {
        let sandbox = Sandbox::new(&self.top)?;
        copy_into(&self.top.join(&self.test_rel), &sandbox.dir.join(&self.test_rel))?;
        Ok(self.exec(&self.run, &sandbox))
    }

    /// The module tests on the original code, without the reproduction test.
    fn module_before(&self) -> Result<Run, String> {
        let sandbox = Sandbox::new(&self.top)?;
        Ok(self.exec(&self.module, &sandbox))
    }

    /// Applies one candidate in a fresh sandbox and runs the test and the module tests on it.
    fn after(&self, candidate: &Candidate) -> Result<After, String> {
        let sandbox = Sandbox::new(&self.top)?;
        candidate.apply(self, &sandbox)?;
        copy_into(&self.top.join(&self.test_rel), &sandbox.dir.join(&self.test_rel))?;
        let (diff_lines, diff) = sandbox.staged_diff(&self.test_rel)?;
        if diff_lines == 0 {
            return Ok(After { diff_lines, diff, test: None, module: None });
        }
        let test = self.exec(&self.run, &sandbox);
        let module = if test.ok && !self.module.is_empty() { Some(self.exec(&self.module, &sandbox)) } else { None };
        Ok(After { diff_lines, diff, test: Some(test), module })
    }
}

struct After {
    diff_lines: usize,
    diff: String,
    test: Option<Run>,
    module: Option<Run>,
}

/// One proposed fix: the working tree of the user, or SEARCH/REPLACE blocks from a model.
pub enum Candidate {
    WorkingTree,
    Blocks(String, Vec<Block>),
}

impl Candidate {
    fn label(&self) -> String {
        match self {
            Candidate::WorkingTree => "working tree".to_string(),
            Candidate::Blocks(name, _) => name.clone(),
        }
    }

    fn apply(&self, job: &Job, sandbox: &Sandbox) -> Result<(), String> {
        match self {
            Candidate::WorkingTree => {
                let exclude = format!(":(exclude){}", job.test_rel);
                let diff = git(&job.top, &["diff", "HEAD", "--binary", "--", ".", &exclude])?;
                if !diff.trim().is_empty() {
                    let patch = std::env::temp_dir().join(format!("penelope-check-{}.diff", unique()));
                    fs::write(&patch, &diff).map_err(|e| e.to_string())?;
                    let applied = git(&sandbox.dir, &["apply", "--whitespace=nowarn", &patch.to_string_lossy()]);
                    let _ = fs::remove_file(&patch);
                    applied.map_err(|e| format!("the working-tree diff does not apply to HEAD: {}", e))?;
                }
                let others = git(&job.top, &["ls-files", "--others", "--exclude-standard", "-z"])?;
                for rel in others.split('\0').filter(|r| !r.is_empty() && *r != job.test_rel && !r.starts_with(".penelope/")) {
                    copy_into(&job.top.join(rel), &sandbox.dir.join(rel))?;
                }
                Ok(())
            }
            Candidate::Blocks(_, blocks) => apply_blocks(&sandbox.dir, blocks),
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Verdicts.

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Status {
    /// Fails before, passes after, module tests pass or failed already on HEAD.
    Passed,
    /// The test passes on the original code: it does not reproduce the issue.
    PassesBefore,
    /// The candidate changes nothing outside the test.
    NoChange,
    /// The test still fails after the change.
    StillFails,
    /// The module tests pass on HEAD and fail after the change.
    BreaksModule,
    /// The check could not run: no git, a patch that does not apply, a missing file.
    Error,
}

impl Status {
    pub fn word(self) -> &'static str {
        match self {
            Status::Passed => "passed",
            Status::PassesBefore => "test_passes_before",
            Status::NoChange => "no_change",
            Status::StillFails => "still_fails",
            Status::BreaksModule => "breaks_module",
            Status::Error => "error",
        }
    }
}

pub struct Verdict {
    pub status: Status,
    pub before: Option<Run>,
    pub after: Option<Run>,
    pub module_after: Option<Run>,
    pub module_before: Option<Run>,
    pub diff_lines: usize,
    pub diff: String,
    pub error: String,
}

impl Verdict {
    fn error(e: String) -> Verdict {
        Verdict { status: Status::Error, before: None, after: None, module_after: None, module_before: None, diff_lines: 0, diff: String::new(), error: e }
    }
}

/// Decides one candidate from the runs. Kept apart from the runs so the rule has its own tests.
pub fn decide(before_ok: bool, diff_lines: usize, after_ok: Option<bool>, module_after: Option<bool>, module_before: Option<bool>) -> Status {
    if before_ok {
        return Status::PassesBefore;
    }
    if diff_lines == 0 {
        return Status::NoChange;
    }
    if after_ok != Some(true) {
        return Status::StillFails;
    }
    if module_after == Some(false) && module_before != Some(false) {
        return Status::BreaksModule;
    }
    Status::Passed
}

fn check_one(job: &Job, before: &Run, candidate: &Candidate, module_before: &mut Option<Option<Run>>) -> Verdict {
    if before.ok {
        return Verdict { status: Status::PassesBefore, before: Some(before.clone()), after: None, module_after: None, module_before: None,
                         diff_lines: 0, diff: String::new(), error: String::new() };
    }
    let after = match job.after(candidate) {
        Ok(a) => a,
        Err(e) => return Verdict { before: Some(before.clone()), ..Verdict::error(e) },
    };
    let module_ok = after.module.as_ref().map(|r| r.ok);
    if module_ok == Some(false) && module_before.is_none() {
        *module_before = Some(job.module_before().ok());
    }
    let mb = module_before.clone().flatten();
    let status = decide(false, after.diff_lines, after.test.as_ref().map(|r| r.ok), module_ok, mb.as_ref().map(|r| r.ok));
    Verdict { status, before: Some(before.clone()), after: after.test, module_after: after.module,
              module_before: if module_ok == Some(false) { mb } else { None }, diff_lines: after.diff_lines, diff: after.diff, error: String::new() }
}

/// Checks the working tree against the reproduction test of the job.
pub fn check_working_tree(job: &Job) -> Verdict {
    let before = match job.before() {
        Ok(r) => r,
        Err(e) => return Verdict::error(e),
    };
    check_one(job, &before, &Candidate::WorkingTree, &mut None)
}

fn describe(job: &Job, v: &Verdict) -> String {
    let code = |r: &Option<Run>| r.as_ref().map(|r| if r.timed_out { "timeout".to_string() } else { format!("exit {}", r.code) }).unwrap_or_else(|| "-".to_string());
    let tail = |r: &Option<Run>| r.as_ref().map(|r| r.tail.clone()).filter(|t| !t.is_empty()).map(|t| format!("\n{}", t)).unwrap_or_default();
    let module = if job.module.is_empty() { "No module tests set (check.module_tests or test_command).".to_string() } else {
        match (&v.module_after, &v.module_before) {
            (Some(a), _) if a.ok => format!("Module tests `{}` pass.", job.module),
            (Some(_), Some(b)) if !b.ok => format!("Module tests `{}` fail, but they failed on the original code too ({}).", job.module, code(&v.module_before)),
            (Some(_), _) => format!("Module tests `{}` FAIL after the change ({}).{}", job.module, code(&v.module_after), tail(&v.module_after)),
            (None, _) => String::new(),
        }
    };
    match v.status {
        Status::Passed => format!("CHECK PASSED. `{}` failed on the original code ({}) and passes after the fix. {} Diff: {} lines.",
                                  job.run, code(&v.before), module, v.diff_lines),
        Status::PassesBefore => format!("CHECK FAILED: test_passes_before. `{}` passes on the original code, so {} does not reproduce the issue. \
Rewrite the test so it fails before the fix, then check again.{}", job.run, job.test_arg, tail(&v.before)),
        Status::NoChange => format!("CHECK FAILED: no_change. The test fails on the original code ({}), but there is no change outside the test.", code(&v.before)),
        Status::StillFails => format!("CHECK FAILED: still_fails. `{}` fails on the original code ({}) and still fails after the change ({}).{}",
                                      job.run, code(&v.before), code(&v.after), tail(&v.after)),
        Status::BreaksModule => format!("CHECK FAILED: breaks_module. The reproduction test passes after the change. {}", module),
        Status::Error => format!("CHECK NOT RUN: {}", v.error),
    }
}

// ---------------------------------------------------------------------------------------------------------------
// The record of verdicts.

fn dir(ctx: &Ctx) -> PathBuf {
    ctx.root.join("check")
}

/// Appends one line to `log.jsonl`; a check (not a turn note) also becomes `last.json`.
fn record(ctx: &Ctx, entry: Value, is_check: bool) {
    let d = dir(ctx);
    let _ = fs::create_dir_all(&d);
    if is_check {
        let _ = fs::write(d.join("last.json"), serde_json::to_string_pretty(&entry).unwrap_or_default());
    }
    use std::io::Write;
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(d.join("log.jsonl")) {
        let _ = writeln!(f, "{}", entry);
    }
}

fn entry(source: &str, job: &Job, v: &Verdict, extra: Value) -> Value {
    let mut e = json!({"time_ms": now_ms(), "source": source, "status": v.status.word(), "test": job.test_rel,
                       "diff_lines": v.diff_lines, "before_exit": v.before.as_ref().map(|r| r.code),
                       "after_exit": v.after.as_ref().map(|r| r.code), "module": !job.module.is_empty(),
                       "module_ok": v.module_after.as_ref().map(|r| r.ok)});
    if let (Some(o), Some(x)) = (e.as_object_mut(), extra.as_object()) {
        for (k, val) in x {
            o.insert(k.clone(), val.clone());
        }
    }
    e
}

fn last(ctx: &Ctx) -> Option<Value> {
    fs::read_to_string(dir(ctx).join("last.json")).ok().and_then(|t| serde_json::from_str(&t).ok())
}

// ---------------------------------------------------------------------------------------------------------------
// Best-of-N.

/// One SEARCH/REPLACE edit. An empty `search` creates the file, or appends to it when it exists.
#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub path: String,
    pub search: String,
    pub replace: String,
}

/// Reads SEARCH/REPLACE blocks. The file path is the last non-empty line before `<<<<<<< SEARCH`, without fences,
/// backticks or a leading `###`.
pub fn parse_blocks(text: &str) -> Vec<Block> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if lines[i].trim_end() != "<<<<<<< SEARCH" {
            i += 1;
            continue;
        }
        let path = lines[..i].iter().rev().map(|l| l.trim()).find(|l| !l.is_empty() && !l.starts_with("```"))
            .map(|l| l.trim_start_matches('#').trim().trim_matches('`').trim().to_string()).unwrap_or_default();
        let Some(mid) = (i + 1..lines.len()).find(|&j| lines[j].trim_end() == "=======") else { break };
        let Some(end) = (mid + 1..lines.len()).find(|&j| lines[j].trim_end() == ">>>>>>> REPLACE") else { break };
        let join = |part: &[&str]| if part.is_empty() { String::new() } else { format!("{}\n", part.join("\n")) };
        out.push(Block { path, search: join(&lines[i + 1..mid]), replace: join(&lines[mid + 1..end]) });
        i = end + 1;
    }
    out
}

/// Applies blocks under `root`. A search text must occur in the file; the first occurrence is replaced.
pub fn apply_blocks(root: &Path, blocks: &[Block]) -> Result<(), String> {
    if blocks.is_empty() {
        return Err("the answer holds no SEARCH/REPLACE block".to_string());
    }
    for b in blocks {
        let rel = safe_rel(&b.path).ok_or_else(|| format!("bad path in a block: {:?}", b.path))?;
        let path = root.join(&rel);
        let old = fs::read_to_string(&path).ok();
        let new = match (&old, b.search.is_empty()) {
            (None, true) => b.replace.clone(),
            (Some(text), true) => format!("{}{}", text, b.replace),
            (None, false) => return Err(format!("{} does not exist", rel)),
            (Some(text), false) => {
                let text = text.replace("\r\n", "\n");
                match text.find(&b.search) {
                    Some(at) => format!("{}{}{}", &text[..at], b.replace, &text[at + b.search.len()..]),
                    None => return Err(format!("the SEARCH text of a block does not occur in {}", rel)),
                }
            }
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        fs::write(&path, new).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// The order the candidates are tried in: smallest diff first, the earlier candidate on a tie. Candidates without a
/// diff (`None`: did not apply, or empty) are left out.
pub fn order(sizes: &[Option<usize>]) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..sizes.len()).filter(|&i| matches!(sizes[i], Some(n) if n > 0)).collect();
    idx.sort_by_key(|&i| (sizes[i].unwrap_or(usize::MAX), i));
    idx
}

/// Hints that push the candidates apart. Two runs of one model on the same 28 tasks differed on one task only, so
/// identical prompts would mostly return the same patch.
const HINTS: &[&str] = &[
    "Make the smallest change that fixes the root cause.",
    "Fix the place where the wrong value is first produced, not where it is noticed.",
    "Handle the edge case the issue describes and keep every other behaviour unchanged.",
    "Read the reproduction test first and make it pass without special-casing the test input.",
];

fn candidate_prompt(issue: &str, test_arg: &str, test_text: &str, files: &[(String, String)], hint: &str) -> String {
    let mut p = format!("Issue:\n{}\n\nReproduction test {}:\n```\n{}\n```\n\nFiles at the original commit:\n", issue.trim(), test_arg, test_text.trim_end());
    for (path, text) in files {
        p.push_str(&format!("\n{}\n```\n{}\n```\n", path, text.trim_end()));
    }
    p.push_str(&format!("\n{}\nDo not change the test.", hint));
    p
}

const CANDIDATE_SYSTEM: &str = "You fix one issue in a code base. Answer only with SEARCH/REPLACE blocks in this format:\n\
path/to/file.py\n<<<<<<< SEARCH\nexact lines from the file\n=======\nnew lines\n>>>>>>> REPLACE\n\
The SEARCH lines must match the file exactly, including indentation. Use as few blocks as needed. No other text.";

/// Reads the files the candidates may change, at HEAD, at most 400 lines and 40,000 characters in all.
fn head_files(job: &Job, files: &[String]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut budget = 40_000usize;
    for f in files {
        let Some(rel) = safe_rel(f) else { continue };
        let Ok(text) = git(&job.top, &["show", &format!("HEAD:{}", rel)]) else { continue };
        let text: String = text.lines().take(400).collect::<Vec<_>>().join("\n");
        if text.len() > budget {
            break;
        }
        budget -= text.len();
        out.push((rel, text));
    }
    out
}

fn changed_files(job: &Job) -> Vec<String> {
    let exclude = format!(":(exclude){}", job.test_rel);
    git(&job.top, &["diff", "HEAD", "--name-only", "--", ".", &exclude]).unwrap_or_default().lines().map(str::to_string).collect()
}

/// Best-of-N: the working tree (when it has a change) and `n` candidates from the candidate models. The first
/// candidate in diff-size order that passes wins; its diff goes to `.penelope/check/winner.diff`.
pub fn best_of(ctx: &Ctx, job: &Job, issue: &str, files: &[String], n: usize) -> String {
    let s = settings(ctx);
    let files: Vec<String> = if files.is_empty() { changed_files(job) } else { files.to_vec() };
    let sources = head_files(job, &files);
    let test_text = fs::read_to_string(job.top.join(&job.test_rel)).unwrap_or_default();
    let mut candidates = Vec::new();
    if !changed_files(job).is_empty() || has_untracked(job) {
        candidates.push(Candidate::WorkingTree);
    }
    let mut notes = Vec::new();
    if issue.trim().is_empty() || sources.is_empty() {
        notes.push("No model candidates: they need the issue text and at least one file at HEAD.".to_string());
    } else {
        for i in 0..n {
            let model = &s.models[i % s.models.len()];
            let prompt = candidate_prompt(issue, &job.test_arg, &test_text, &sources, HINTS[i % HINTS.len()]);
            match crate::weak::ask(model, CANDIDATE_SYSTEM, &prompt) {
                Ok(text) => candidates.push(Candidate::Blocks(format!("candidate {} ({})", i + 1, model["model"].as_str().unwrap_or("?")), parse_blocks(&text))),
                Err(e) => notes.push(format!("candidate {}: the model gave no answer: {}", i + 1, e)),
            }
        }
    }
    best_of_candidates(ctx, job, candidates, notes)
}

fn has_untracked(job: &Job) -> bool {
    git(&job.top, &["ls-files", "--others", "--exclude-standard", "-z"]).unwrap_or_default()
        .split('\0').any(|r| !r.is_empty() && r != job.test_rel && !r.starts_with(".penelope/"))
}

/// The selection part of best-of-N, without model calls, so tests can hand in fixed candidates.
pub fn best_of_candidates(ctx: &Ctx, job: &Job, candidates: Vec<Candidate>, mut notes: Vec<String>) -> String {
    let before = match job.before() {
        Ok(r) => r,
        Err(e) => return format!("CHECK NOT RUN: {}", e),
    };
    if before.ok {
        let v = Verdict { status: Status::PassesBefore, before: Some(before), after: None, module_after: None, module_before: None,
                          diff_lines: 0, diff: String::new(), error: String::new() };
        record(ctx, entry("best", job, &v, json!({"candidates": candidates.len()})), true);
        return describe(job, &v);
    }
    // Diff sizes first: applying is cheap, the tests are not.
    let sizes: Vec<Option<usize>> = candidates.iter().map(|c| {
        let sandbox = Sandbox::new(&job.top).ok()?;
        c.apply(job, &sandbox).ok()?;
        sandbox.staged_diff(&job.test_rel).ok().map(|(n, _)| n)
    }).collect();
    for (i, c) in candidates.iter().enumerate() {
        match sizes[i] {
            None => notes.push(format!("{}: does not apply", c.label())),
            Some(0) => notes.push(format!("{}: no change", c.label())),
            Some(n) => notes.push(format!("{}: {} lines", c.label(), n)),
        }
    }
    let mut module_before = None;
    let mut tried = Vec::new();
    for i in order(&sizes) {
        let v = check_one(job, &before, &candidates[i], &mut module_before);
        tried.push(format!("{}: {}", candidates[i].label(), v.status.word()));
        if v.status == Status::Passed {
            let _ = fs::create_dir_all(dir(ctx));
            let _ = fs::write(dir(ctx).join("winner.diff"), &v.diff);
            record(ctx, entry("best", job, &v, json!({"candidates": candidates.len(), "winner": candidates[i].label()})), true);
            return format!("BEST OF {}: {} wins. {}\nWinner diff: {}{}\nTried: {}\nCandidates: {}", candidates.len(), candidates[i].label(),
                           describe(job, &v), dir(ctx).join("winner.diff").display(),
                           if matches!(candidates[i], Candidate::WorkingTree) { " (already in the working tree)" } else { "; apply with git apply" },
                           tried.join("; "), notes.join("; "));
        }
    }
    let v = Verdict { status: Status::StillFails, before: Some(before), after: None, module_after: None, module_before: None,
                      diff_lines: 0, diff: String::new(), error: String::new() };
    record(ctx, entry("best", job, &v, json!({"candidates": candidates.len()})), true);
    format!("BEST OF {}: no candidate passed. The test fails on the original code. Tried: {}\nCandidates: {}",
            candidates.len(), if tried.is_empty() { "none".to_string() } else { tried.join("; ") }, notes.join("; "))
}

// ---------------------------------------------------------------------------------------------------------------
// Entry points: the tool, the command and the turn.

fn task_dir(cwd: &str) -> PathBuf {
    let cwd = if cwd.trim().is_empty() { crate::eval::task_cwd() } else { cwd.to_string() };
    if cwd.trim().is_empty() { std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")) } else { PathBuf::from(cwd) }
}

/// The tool `verify_fix`.
pub fn verify(ctx: &Ctx, args: &Value) -> String {
    let s = settings(ctx);
    let str_arg = |k: &str| args[k].as_str().unwrap_or("").to_string();
    let module = { let m = str_arg("module_tests"); if m.trim().is_empty() { s.module_tests.clone() } else { m } };
    let run = { let r = str_arg("run"); if r.trim().is_empty() { s.run.clone() } else { r } };
    let job = match Job::new(&task_dir(&str_arg("cwd")), &str_arg("test"), &run, &module, s.timeout) {
        Ok(j) => j,
        Err(e) => return format!("CHECK NOT RUN: {}", e),
    };
    let n = args["best_of"].as_u64().unwrap_or(0) as usize;
    if n > 0 {
        let files: Vec<String> = args["files"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default();
        return best_of(ctx, &job, &str_arg("issue"), &files, n.min(8));
    }
    let v = check_working_tree(&job);
    record(ctx, entry("tool", &job, &v, json!({})), true);
    describe(&job, &v)
}

/// Splits a command line into words; double and single quotes keep spaces.
fn words(text: &str) -> Vec<String> {
    let (mut out, mut cur, mut quote, mut any) = (Vec::new(), String::new(), None, false);
    for c in text.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => cur.push(c),
            (None, '"') | (None, '\'') => { quote = Some(c); any = true; }
            (None, c) if c.is_whitespace() => {
                if any || !cur.is_empty() { out.push(std::mem::take(&mut cur)); }
                any = false;
            }
            (None, c) => cur.push(c),
        }
    }
    if any || !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// What `/check` and `penelope check` print with no arguments: the settings, the last check and the counts.
pub fn status(ctx: &Ctx) -> String {
    let s = settings(ctx);
    let mut out = format!("checker: {} (gate reminds once before a final answer after a code edit; report only notes the verdict; off)\n", s.mode);
    out.push_str(&format!("module tests: {}\n", if s.module_tests.is_empty() { "none (set check.module_tests or test_command)" } else { &s.module_tests }));
    out.push_str(&format!("test command: {}\n", if s.run.is_empty() { "by extension (python3, node, sh, ruby)" } else { &s.run }));
    out.push_str(&format!("best-of: {} candidates from {}\n", s.candidates,
                          s.models.iter().map(|m| m["model"].as_str().unwrap_or("?").to_string()).collect::<Vec<_>>().join(", ")));
    match last(ctx) {
        Some(l) => out.push_str(&format!("last check: {} on {}, diff {} lines\n", l["status"].as_str().unwrap_or("?"), l["test"].as_str().unwrap_or("?"), l["diff_lines"])),
        None => out.push_str("last check: none\n"),
    }
    let log = fs::read_to_string(dir(ctx).join("log.jsonl")).unwrap_or_default();
    let mut counts: Vec<(String, usize)> = Vec::new();
    for line in log.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        let key = format!("{}:{}", v["source"].as_str().unwrap_or("?"), v["status"].as_str().unwrap_or("?"));
        match counts.iter_mut().find(|(k, _)| *k == key) {
            Some(slot) => slot.1 += 1,
            None => counts.push((key, 1)),
        }
    }
    if !counts.is_empty() {
        out.push_str(&format!("log: {}\n", counts.iter().map(|(k, n)| format!("{} {}", k, n)).collect::<Vec<_>>().join(", ")));
    }
    out.push_str("usage: /check TEST [--run CMD] [--module CMD] [--cwd DIR]; /check best TEST [N] [--files a,b] -- ISSUE; /check gate|report|off\n");
    out
}

/// `/check` and `penelope check`. Mode words are handled by the caller, which owns the config file.
pub fn command(ctx: &Ctx, text: &str) -> String {
    let w = words(text);
    if w.is_empty() {
        return status(ctx);
    }
    let s = settings(ctx);
    let best = w[0] == "best";
    let rest: Vec<String> = if best { w[1..].to_vec() } else { w.clone() };
    let (mut test, mut run, mut module, mut cwd, mut n, mut files, mut issue) = (String::new(), s.run.clone(), s.module_tests.clone(), String::new(), s.candidates, Vec::new(), String::new());
    let mut i = 0;
    while i < rest.len() {
        let next = rest.get(i + 1).cloned().unwrap_or_default();
        match rest[i].as_str() {
            "--run" => { run = next; i += 1; }
            "--module" => { module = next; i += 1; }
            "--cwd" => { cwd = next; i += 1; }
            "--files" => { files = next.split(',').map(|f| f.trim().to_string()).filter(|f| !f.is_empty()).collect(); i += 1; }
            "--" => { issue = rest[i + 1..].join(" "); break; }
            word if test.is_empty() => test = word.to_string(),
            word => match word.parse::<usize>() {
                Ok(k) if best => n = k.clamp(1, 8),
                _ => return format!("unknown word {:?}\n{}", word, status(ctx)),
            },
        }
        i += 1;
    }
    if test.is_empty() {
        return status(ctx);
    }
    let job = match Job::new(&task_dir(&cwd), &test, &run, &module, s.timeout) {
        Ok(j) => j,
        Err(e) => return format!("CHECK NOT RUN: {}", e),
    };
    if best {
        return best_of(ctx, &job, &issue, &files, n);
    }
    let v = check_working_tree(&job);
    record(ctx, entry("command", &job, &v, json!({})), true);
    describe(&job, &v)
}

/// The rule the main model reads in its system prompt.
pub fn rule(ctx: &Ctx) -> String {
    if settings(ctx).mode == "off" {
        return String::new();
    }
    " 12. On a code task, before the final answer: write a reproduction test from the issue text as its own file and call verify_fix \
with its path. It must fail on the original code and pass after your fix, and the module tests must still pass. When the check fails, \
fix the code or the test; when no test can reproduce the issue, say so in the answer.".to_string()
}

thread_local! {
    static ARMED: Cell<bool> = const { Cell::new(false) };
    static NUDGED: Cell<usize> = const { Cell::new(0) };
    static MAX_NUDGES: Cell<usize> = const { Cell::new(0) };
    static TURN_START: Cell<u64> = const { Cell::new(0) };
    static ROOT: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

/// Starts a new user turn: no code edit seen, no reminder given.
pub fn begin_turn() {
    ARMED.with(|a| a.set(false));
    NUDGED.with(|n| n.set(0));
    TURN_START.with(|t| t.set(now_ms()));
}

pub fn turn_start() -> u64 {
    TURN_START.with(|t| t.get())
}

pub fn is_code(path: &str) -> bool {
    let ext = Path::new(path.trim()).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    CODE.contains(&ext.as_str())
}

/// Called after a successful `write` or `edit`. A code edit of the lead model outside a subagent or a cluster arms the
/// reminder for this turn.
pub fn note_edit(ctx: &Ctx, role: &str, args: &Value) {
    let s = settings(ctx);
    if s.mode == "off" || role != "lead" || crate::agents::depth() > 0 || crate::clusters::current_cluster().is_some() {
        return;
    }
    if !is_code(args["path"].as_str().unwrap_or("")) {
        return;
    }
    ARMED.with(|a| a.set(true));
    MAX_NUDGES.with(|m| m.set(if s.mode == "gate" { s.nudges } else { 0 }));
    ROOT.with(|r| *r.borrow_mut() = Some(ctx.root.clone()));
}

fn checked_since(root: &Path, start: u64) -> bool {
    fs::read_to_string(root.join("check").join("last.json")).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v["time_ms"].as_u64()).map(|t| t >= start).unwrap_or(false)
}

/// Called when the model gives a final answer in the tool loop. Returns the reminder that goes back as a user message
/// when the turn changed code and no check ran since the turn began, at most `nudges` times per turn.
pub fn before_final() -> Option<String> {
    if !ARMED.with(|a| a.get()) || crate::agents::depth() > 0 || crate::route::current_role() != "lead" {
        return None;
    }
    let root = ROOT.with(|r| r.borrow().clone())?;
    if checked_since(&root, turn_start()) || NUDGED.with(|n| n.get()) >= MAX_NUDGES.with(|m| m.get()) {
        return None;
    }
    NUDGED.with(|n| n.set(n.get() + 1));
    Some("Penelope checker: this turn changed code, but no reproduction test ran. Before the final answer, write a test from \
the issue text that fails on the original code, then call verify_fix with its path. If no test can reproduce the issue, \
say so in the final answer.".to_string())
}

fn edit_name(name: &str) -> bool {
    let n = name.rsplit("__").next().unwrap_or(name).to_ascii_lowercase();
    matches!(n.as_str(), "edit" | "write" | "multiedit" | "notebookedit")
}

/// The verdict line under the answer of a turn that changed code. Also counts an unverified turn in the log, so the
/// share of code turns without a check can be read back. `trace` holds the tool calls of the turn.
pub fn turn_note(ctx: &Ctx, trace: &[crate::tools::Call], start: u64) -> Option<String> {
    if settings(ctx).mode == "off" {
        return None;
    }
    let edited = ARMED.with(|a| a.get())
        || trace.iter().any(|(name, detail, ok, _)| *ok && edit_name(name) && detail.split(|c: char| c.is_whitespace() || "\"',:{}".contains(c)).any(is_code));
    if !edited {
        return None;
    }
    let line = match last(ctx).filter(|l| l["time_ms"].as_u64().unwrap_or(0) >= start) {
        Some(l) if l["status"] == "passed" => "Checker: verified. The reproduction test failed on the original code and passes after the fix.".to_string(),
        Some(l) => format!("Checker: NOT verified ({}). See /check.", l["status"].as_str().unwrap_or("?")),
        None => {
            record(ctx, json!({"time_ms": now_ms(), "source": "turn", "status": "unverified"}), false);
            "Checker: not verified. No reproduction test ran after the code change in this turn.".to_string()
        }
    };
    Some(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh_git(dir: &Path, args: &[&str]) {
        let out = Command::new("git").arg("-C").arg(dir).args(args).output().expect("git");
        assert!(out.status.success(), "git {:?}: {}", args, String::from_utf8_lossy(&out.stderr));
    }

    /// A fixture repository with one committed file `calc.txt` that holds a bug marker, and a context under it.
    fn fixture(name: &str) -> (PathBuf, Ctx) {
        let base = std::env::temp_dir().join(format!("penelope-checker-{}-{}", name, unique()));
        let _ = fs::remove_dir_all(&base);
        let repo = base.join("repo");
        fs::create_dir_all(&repo).unwrap();
        sh_git(&repo, &["init", "-q"]);
        sh_git(&repo, &["config", "user.email", "t@example.org"]);
        sh_git(&repo, &["config", "user.name", "t"]);
        sh_git(&repo, &["config", "core.autocrlf", "false"]);
        fs::write(repo.join("calc.txt"), "def add(a, b):\n    return a - b\n").unwrap();
        fs::write(repo.join("other.txt"), "keep\n").unwrap();
        sh_git(&repo, &["add", "-A"]);
        sh_git(&repo, &["commit", "-q", "-m", "init"]);
        let ctx = crate::config::load(&base.join(".penelope"));
        (repo, ctx)
    }

    // The "tests" of the fixture are git commands, which exist on every CI runner: `git grep -q` exits 0 when the
    // text occurs and 1 when it does not.
    const REPRO: &str = "git grep -q --untracked -e \"a + b\" -- calc.txt";
    const MODULE: &str = "git grep -q --untracked -e keep -- other.txt";

    fn job(repo: &Path) -> Job {
        fs::write(repo.join("repro_test.txt"), "issue: add subtracts\n").unwrap();
        Job::new(repo, "repro_test.txt", REPRO, MODULE, 60).unwrap()
    }

    #[test]
    fn the_rule_fails_before_passes_after_and_keeps_module_tests() {
        assert_eq!(decide(true, 3, Some(true), Some(true), None), Status::PassesBefore);
        assert_eq!(decide(false, 0, None, None, None), Status::NoChange);
        assert_eq!(decide(false, 2, Some(false), None, None), Status::StillFails);
        assert_eq!(decide(false, 2, Some(true), Some(false), Some(true)), Status::BreaksModule);
        assert_eq!(decide(false, 2, Some(true), Some(false), Some(false)), Status::Passed, "module tests that failed on HEAD do not count");
        assert_eq!(decide(false, 2, Some(true), None, None), Status::Passed);
    }

    #[test]
    fn a_correct_fix_passes_and_the_working_tree_stays_as_it_was() {
        let (repo, ctx) = fixture("pass");
        fs::write(repo.join("calc.txt"), "def add(a, b):\n    return a + b\n").unwrap();
        let j = job(&repo);
        let v = check_working_tree(&j);
        assert_eq!(v.status, Status::Passed, "{}", describe(&j, &v));
        assert_eq!(v.diff_lines, 2);
        assert!(describe(&j, &v).starts_with("CHECK PASSED") && describe(&j, &v).contains("Module tests"));
        assert_eq!(fs::read_to_string(repo.join("calc.txt")).unwrap(), "def add(a, b):\n    return a + b\n");
        let worktrees = git(&repo, &["worktree", "list"]).unwrap();
        assert_eq!(worktrees.lines().count(), 1, "the sandbox worktree is gone: {}", worktrees);
        let said = verify(&ctx, &json!({"test": "repro_test.txt", "run": REPRO, "module_tests": MODULE, "cwd": repo.to_string_lossy()}));
        assert!(said.starts_with("CHECK PASSED"), "{}", said);
        assert_eq!(last(&ctx).unwrap()["status"], "passed");
        let _ = fs::remove_dir_all(repo.parent().unwrap());
    }

    #[test]
    fn a_test_that_passes_on_the_original_code_is_rejected() {
        let (repo, _ctx) = fixture("before");
        fs::write(repo.join("calc.txt"), "def add(a, b):\n    return a + b\n").unwrap();
        fs::write(repo.join("repro_test.txt"), "x\n").unwrap();
        let j = Job::new(&repo, "repro_test.txt", "git grep -q -e return -- calc.txt", "", 60).unwrap();
        let v = check_working_tree(&j);
        assert_eq!(v.status, Status::PassesBefore);
        assert!(describe(&j, &v).contains("does not reproduce"), "{}", describe(&j, &v));
        let _ = fs::remove_dir_all(repo.parent().unwrap());
    }

    #[test]
    fn a_wrong_fix_and_a_fix_that_breaks_the_module_both_fail() {
        let (repo, _ctx) = fixture("wrong");
        fs::write(repo.join("calc.txt"), "def add(a, b):\n    return b - a\n").unwrap();
        let j = job(&repo);
        assert_eq!(check_working_tree(&j).status, Status::StillFails);
        fs::write(repo.join("calc.txt"), "def add(a, b):\n    return a + b\n").unwrap();
        fs::write(repo.join("other.txt"), "gone\n").unwrap();
        let v = check_working_tree(&j);
        assert_eq!(v.status, Status::BreaksModule, "{}", describe(&j, &v));
        assert!(describe(&j, &v).contains("FAIL after the change"));
        let _ = fs::remove_dir_all(repo.parent().unwrap());
    }

    #[test]
    fn an_untracked_new_file_is_part_of_the_change() {
        let (repo, _ctx) = fixture("untracked");
        fs::write(repo.join("fix.txt"), "def add(a, b):\n    return a + b\n").unwrap();
        fs::write(repo.join("repro_test.txt"), "x\n").unwrap();
        let j = Job::new(&repo, "repro_test.txt", "git grep -q --untracked -e \"a + b\" -- fix.txt", "", 60).unwrap();
        let v = check_working_tree(&j);
        assert_eq!(v.status, Status::Passed, "{}", describe(&j, &v));
        let _ = fs::remove_dir_all(repo.parent().unwrap());
    }

    #[test]
    fn no_git_and_a_missing_test_are_named() {
        let dir = std::env::temp_dir().join(format!("penelope-checker-nogit-{}", unique()));
        fs::create_dir_all(&dir).unwrap();
        let e = Job::new(&dir, "t.py", "", "", 10).err().unwrap();
        assert!(e.contains("git") || e.contains("commit"), "{}", e);
        let (repo, _ctx) = fixture("missing");
        assert!(Job::new(&repo, "nope.py", "", "", 10).err().unwrap().contains("does not exist"));
        assert!(Job::new(&repo, "../x.py", "", "", 10).err().unwrap().contains("relative"));
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(repo.parent().unwrap());
    }

    #[test]
    fn blocks_parse_and_apply() {
        let text = "Here:\n```\ncalc.txt\n<<<<<<< SEARCH\n    return a - b\n=======\n    return a + b\n>>>>>>> REPLACE\n```\n### new/file.txt\n<<<<<<< SEARCH\n=======\nhello\n>>>>>>> REPLACE\n";
        let blocks = parse_blocks(text);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0], Block { path: "calc.txt".into(), search: "    return a - b\n".into(), replace: "    return a + b\n".into() });
        assert_eq!((blocks[1].path.as_str(), blocks[1].search.as_str()), ("new/file.txt", ""));
        let dir = std::env::temp_dir().join(format!("penelope-checker-blocks-{}", unique()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("calc.txt"), "def add(a, b):\r\n    return a - b\r\n").unwrap();
        apply_blocks(&dir, &blocks).unwrap();
        assert_eq!(fs::read_to_string(dir.join("calc.txt")).unwrap(), "def add(a, b):\n    return a + b\n");
        assert_eq!(fs::read_to_string(dir.join("new/file.txt")).unwrap(), "hello\n");
        let miss = vec![Block { path: "calc.txt".into(), search: "nothing\n".into(), replace: "x\n".into() }];
        assert!(apply_blocks(&dir, &miss).unwrap_err().contains("does not occur"));
        assert!(apply_blocks(&dir, &[Block { path: "../evil".into(), search: String::new(), replace: "x".into() }]).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_order_is_smallest_diff_first_and_skips_empty_ones() {
        assert_eq!(order(&[Some(5), Some(2), None, Some(2), Some(0)]), vec![1, 3, 0]);
        assert!(order(&[None, Some(0)]).is_empty());
    }

    #[test]
    fn best_of_picks_the_smallest_passing_candidate() {
        let (repo, ctx) = fixture("best");
        let j = job(&repo);
        let blocks = |text: &str| parse_blocks(text);
        let wrong = Candidate::Blocks("wrong".into(), blocks("calc.txt\n<<<<<<< SEARCH\n    return a - b\n=======\n    return b - a\n>>>>>>> REPLACE\n"));
        let big = Candidate::Blocks("big".into(), blocks("calc.txt\n<<<<<<< SEARCH\ndef add(a, b):\n    return a - b\n=======\n# fixed\ndef add(a, b):\n    total = 0\n    return a + b\n>>>>>>> REPLACE\n"));
        let small = Candidate::Blocks("small".into(), blocks("calc.txt\n<<<<<<< SEARCH\n    return a - b\n=======\n    return a + b\n>>>>>>> REPLACE\n"));
        let broken = Candidate::Blocks("broken".into(), blocks("calc.txt\n<<<<<<< SEARCH\nnot there\n=======\nx\n>>>>>>> REPLACE\n"));
        let said = best_of_candidates(&ctx, &j, vec![big, wrong, broken, small], Vec::new());
        assert!(said.contains("small wins"), "{}", said);
        assert!(said.contains("broken: does not apply"), "{}", said);
        let winner = fs::read_to_string(dir(&ctx).join("winner.diff")).unwrap();
        assert!(winner.contains("+    return a + b") && !winner.contains("total"), "{}", winner);
        assert_eq!(fs::read_to_string(repo.join("calc.txt")).unwrap(), "def add(a, b):\n    return a - b\n", "the working tree is untouched");
        let none = best_of_candidates(&ctx, &j, vec![Candidate::Blocks("w".into(), blocks("calc.txt\n<<<<<<< SEARCH\n    return a - b\n=======\n    return 0\n>>>>>>> REPLACE\n"))], Vec::new());
        assert!(none.contains("no candidate passed"), "{}", none);
        let _ = fs::remove_dir_all(repo.parent().unwrap());
    }

    #[test]
    fn a_hanging_test_is_killed_at_the_limit() {
        let dir = std::env::temp_dir();
        let slow = if cfg!(windows) { "ping -n 30 127.0.0.1 > NUL" } else { "sleep 30" };
        let t = Instant::now();
        let r = run_command(slow, &dir, 1);
        assert!(r.timed_out && !r.ok, "{:?}", r);
        assert!(t.elapsed() < Duration::from_secs(15));
    }

    #[test]
    fn a_python_fixture_runs_for_real_when_python_is_there() {
        let python = if cfg!(windows) { "python" } else { "python3" };
        if crate::config::which(python).is_none() {
            return;
        }
        let (repo, _ctx) = fixture("python");
        fs::write(repo.join("calc.py"), "def add(a, b):\n    return a - b\n").unwrap();
        sh_git(&repo, &["add", "calc.py"]);
        sh_git(&repo, &["commit", "-q", "-m", "calc"]);
        fs::write(repo.join("test_issue.py"), "import sys\nfrom calc import add\nsys.exit(0 if add(2, 3) == 5 else 1)\n").unwrap();
        fs::write(repo.join("calc.py"), "def add(a, b):\n    return a + b\n").unwrap();
        let j = Job::new(&repo, "test_issue.py", "", "", 60).unwrap();
        // The test sits in a subfolder and imports `calc` from the root, as an editable install would allow. The
        // checker must import the sandbox copy, never the fixed file of the user.
        fs::create_dir_all(repo.join("tests")).unwrap();
        fs::rename(repo.join("test_issue.py"), repo.join("tests").join("test_issue.py")).unwrap();
        let j = Job { test_rel: "tests/test_issue.py".into(), test_arg: "tests/test_issue.py".into(),
                      run: format!("{} tests/test_issue.py", python), ..j };
        let v = check_working_tree(&j);
        assert_eq!(v.status, Status::Passed, "{}", describe(&j, &v));
        assert_eq!(v.before.as_ref().map(|r| r.code), Some(1), "the original code fails: {}", describe(&j, &v));
        let _ = fs::remove_dir_all(repo.parent().unwrap());
    }

    #[test]
    fn the_reminder_comes_once_after_a_code_edit_and_not_after_a_check() {
        let (repo, mut ctx) = fixture("gate");
        ctx.cfg["check"]["mode"] = json!("gate");
        begin_turn();
        assert!(before_final().is_none(), "no edit, no reminder");
        note_edit(&ctx, "lead", &json!({"path": "README.md"}));
        assert!(before_final().is_none(), "a Markdown edit is no code edit");
        note_edit(&ctx, "worker", &json!({"path": "a.py"}));
        assert!(before_final().is_none(), "only the lead is reminded");
        note_edit(&ctx, "lead", &json!({"path": "src/a.py"}));
        assert!(before_final().unwrap().contains("verify_fix"));
        assert!(before_final().is_none(), "one reminder per turn");
        let note = turn_note(&ctx, &[], turn_start()).unwrap();
        assert!(note.contains("not verified"), "{}", note);
        begin_turn();
        note_edit(&ctx, "lead", &json!({"path": "src/a.py"}));
        record(&ctx, json!({"time_ms": now_ms(), "source": "tool", "status": "passed"}), true);
        assert!(before_final().is_none(), "a check in this turn silences the reminder");
        assert!(turn_note(&ctx, &[], turn_start()).unwrap().contains("verified."));
        ctx.cfg["check"]["mode"] = json!("off");
        begin_turn();
        note_edit(&ctx, "lead", &json!({"path": "src/a.py"}));
        assert!(before_final().is_none() && turn_note(&ctx, &[], turn_start()).is_none());
        let trace = vec![("mcp__penelope__edit".to_string(), "{\"path\": \"src/x.rs\"}".to_string(), true, String::new())];
        ctx.cfg["check"]["mode"] = json!("report");
        std::thread::sleep(Duration::from_millis(5));   // the check above must lie before this turn
        begin_turn();
        assert!(turn_note(&ctx, &trace, turn_start()).unwrap().contains("not verified"));
        let _ = fs::remove_dir_all(repo.parent().unwrap());
    }

    #[test]
    fn words_keep_quoted_commands_together() {
        assert_eq!(words("best t.py 3 --run \"python -m pytest {test}\" -- add is wrong"),
                   vec!["best", "t.py", "3", "--run", "python -m pytest {test}", "--", "add", "is", "wrong"]);
        assert_eq!(words("a '' b"), vec!["a", "", "b"]);
    }

    /// The expected hit rate of best-of-N with a checker, from docs/CHECKER.md: a task solved with chance p per
    /// candidate, a checker that passes a correct candidate with chance t and a wrong one with chance f, the first
    /// passing candidate wins, the first candidate when none passes.
    fn expected_hit(p: f64, t: f64, f: f64, n: i32) -> f64 {
        let q = p * t + (1.0 - p) * f;
        if q == 0.0 {
            return p;
        }
        p * t * (1.0 - (1.0 - q).powi(n)) / q + p * (1.0 - t) * (1.0 - q).powi(n - 1)
    }

    #[test]
    fn the_hit_formula_matches_a_simulation() {
        let mut seed: u64 = 7;
        let mut rnd = || { seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407); (seed >> 11) as f64 / (1u64 << 53) as f64 };
        for &(p, t, f, n) in &[(0.5, 0.7, 0.2, 3), (0.1, 0.44, 0.2, 5), (0.97, 0.9, 0.1, 3)] {
            let rounds = 100_000;
            let mut hits = 0;
            for _ in 0..rounds {
                let good: Vec<bool> = (0..n).map(|_| rnd() < p).collect();
                let pass: Vec<bool> = good.iter().map(|&g| rnd() < if g { t } else { f }).collect();
                let pick = pass.iter().position(|&x| x).unwrap_or(0);
                hits += good[pick] as usize;
            }
            let sim = hits as f64 / rounds as f64;
            assert!((sim - expected_hit(p, t, f, n)).abs() < 0.01, "p {} t {} f {} n {}: {} vs {}", p, t, f, n, sim, expected_hit(p, t, f, n));
            assert!((expected_hit(p, t, f, 1) - p).abs() < 1e-12, "one candidate keeps the hit rate");
        }
    }
}
