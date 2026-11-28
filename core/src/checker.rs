//! The executable checker for code tasks. Eurykleia checks the shape of a tool call; the checker runs the code.
//!
//! Before the final answer on a code task the model writes a reproduction test from the issue text and calls
//! `verify_fix`. The checker adds a throwaway git worktree at HEAD, copies the test in and runs it there: it must fail.
//! Then it applies the change of the working tree (tracked diff plus untracked files, without the test) and runs the
//! test again: it must pass. Then the module tests run on the changed copy: they must pass, or must already fail on
//! HEAD. The files of the user are never written; git adds and removes one entry under `.git/worktrees`, with hooks off.
//!
//! The commands pass the same gate as the `run` tool: the safe list, the project allow list or a yes from the person,
//! and bubblewrap when `sandbox` is `bwrap`. The default interpreter run of a model-written test needs one yes per
//! session unless `run_policy` is `all`.
//!
//! Best-of-N: N candidates (default 3) from the weaker model as SEARCH/REPLACE blocks, plus the working tree. The working
//! tree is tried first and wins when it passes; the model candidates follow in diff-size order, smallest first. A
//! candidate may change only the listed files and never a file that runs before every test (sitecustomize.py,
//! conftest.py, *.pth and the test configs). The test on HEAD runs once for all of them.
//!
//! Config `check`: `mode` gate (default: one reminder before a final answer that follows a code edit without a check),
//! report (only the verdict line under the answer) or off; `module_tests` (empty: `test_command`); `run` (command for
//! the test, `{test}` is the path; empty: python3, node, sh, ruby by extension); `candidates` (N, 1 to 8); `models`
//! (roles or model objects for the candidates, cycled; default `["weak"]`); `timeout` (seconds per command); `budget`
//! (seconds for one whole check); `nudges`. Every verdict goes to `.penelope/check/log.jsonl`; the last check of a task
//! goes to `.penelope/check/KEY/last.json`, where KEY hashes the repository root and the session.

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
    pub budget: u64,
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
        budget: c["budget"].as_u64().unwrap_or(1200).clamp(30, 14_400),
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
/// `skipped` means the command never started because the time budget of the whole check was used up.
#[derive(Debug, Clone)]
pub struct Run {
    pub ok: bool,
    pub code: i32,
    pub timed_out: bool,
    pub tail: String,
    pub skipped: bool,
}

fn last_lines(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.trim_end().lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

/// The shared cargo target folder for checks, so a Rust test in a fresh worktree does not rebuild everything.
fn cargo_target() -> Option<PathBuf> {
    if std::env::var_os("CARGO_TARGET_DIR").is_some() {
        return None;
    }
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    Some(PathBuf::from(home).join(".cache").join("penelope").join("check-target"))
}

/// The bubblewrap command for one check run: read-only root, the worktree `writable` and the cargo target folder
/// writable, a private /tmp, no network, started in `dir`.
fn bwrap(command: &str, writable: &Path, dir: &Path, target: Option<&Path>) -> Command {
    let mut c = Command::new("bwrap");
    let w = writable.to_string_lossy().to_string();
    c.args(["--die-with-parent", "--unshare-net", "--ro-bind", "/", "/", "--dev", "/dev", "--proc", "/proc", "--tmpfs", "/tmp"]);
    c.args(["--bind", &w, &w]);
    if let Some(t) = target {
        let _ = fs::create_dir_all(t);
        let t = t.to_string_lossy().to_string();
        c.args(["--bind", &t, &t]);
    }
    c.arg("--chdir").arg(dir).args(["--", "/bin/sh", "-c", command]);
    c
}

/// Runs a shell line in `dir`. Output goes to files, so a chatty test cannot block on a full pipe. After `timeout`
/// seconds the process tree is killed and the run counts as failed. `roots` go first on PYTHONPATH: a package installed
/// with `pip install -e` points at the original checkout, and without this the copy would import the user's files.
#[cfg(test)]
pub fn run_command_in(command: &str, dir: &Path, roots: &[PathBuf], timeout: u64) -> Run {
    run_command_with(command, dir, roots, timeout, None)
}

/// `run_command_in`, inside bubblewrap when `sandbox` names the writable worktree.
fn run_command_with(command: &str, dir: &Path, roots: &[PathBuf], timeout: u64, sandbox: Option<&Path>) -> Run {
    let base = crate::platform::tmp_dir().join(format!("penelope-check-out-{}", unique()));
    let (out_path, err_path) = (base.with_extension("out"), base.with_extension("err"));
    let fail = |e: String| Run { ok: false, code: -1, timed_out: false, tail: e, skipped: false };
    let (Ok(out), Ok(err)) = (fs::File::create(&out_path), fs::File::create(&err_path)) else {
        return fail("the output files could not be created".to_string());
    };
    let target = cargo_target();
    let mut cmd = match sandbox {
        Some(w) => bwrap(command, w, dir, target.as_deref()),
        None => crate::platform::shell(command),
    };
    cmd.current_dir(dir).stdin(Stdio::null()).stdout(out).stderr(err).env("PENELOPE_CHECK", "1");
    if let Some(t) = &target {
        cmd.env("CARGO_TARGET_DIR", t);
    }
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
    Run { ok: status.map(|s| s.success()).unwrap_or(false), code, timed_out, tail, skipped: false }
}

#[cfg(test)]
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

/// An empty folder for `core.hooksPath`, so `git worktree add` runs no post-checkout hook of the user (husky and the like).
fn no_hooks() -> PathBuf {
    let d = crate::platform::tmp_dir().join("penelope-check-no-hooks");
    let _ = fs::create_dir_all(&d);
    d
}

/// Folders of installed dependencies that git ignores and the throwaway worktree would miss. They are linked in.
const LINKED: &[&str] = &["node_modules", ".venv", "venv"];
/// Endings of compiled extensions that a package builds in place next to its sources. They are copied in.
const BUILT: &[&str] = &[".so", ".pyd", ".dylib"];

/// A detached git worktree at HEAD under the temp folder. Dropping it removes the folder and the worktree entry.
/// `excludes` is a git ignore file that lists what the sandbox brought in from the ignored files of the repository.
struct Sandbox {
    repo: PathBuf,
    dir: PathBuf,
    excludes: PathBuf,
}

impl Sandbox {
    fn new(repo: &Path) -> Result<Sandbox, String> {
        let id = unique();
        let dir = crate::platform::tmp_dir().join(format!("penelope-check-{}", id));
        let hooks = format!("core.hooksPath={}", no_hooks().to_string_lossy());
        git(repo, &["-c", &hooks, "worktree", "add", "--detach", "--quiet", &dir.to_string_lossy(), "HEAD"])?;
        let sandbox = Sandbox { repo: repo.to_path_buf(), dir, excludes: crate::platform::tmp_dir().join(format!("penelope-check-{}.exclude", id)) };
        sandbox.bring_ignored();
        Ok(sandbox)
    }

    /// Links ignored dependency folders and copies ignored compiled extensions from the repository, so `import
    /// sklearn` or `require("x")` works in the worktree as it does in the checkout. At most 5,000 entries.
    fn bring_ignored(&self) {
        let listed = git(&self.repo, &["ls-files", "--others", "--ignored", "--exclude-standard", "--directory", "-z"]).unwrap_or_default();
        let mut brought = Vec::new();
        for entry in listed.split('\0').filter(|e| !e.is_empty()).take(5000) {
            let rel = entry.trim_end_matches('/');
            let Some(rel) = safe_rel(rel) else { continue };
            let name = rel.rsplit('/').next().unwrap_or("");
            let (from, to) = (self.repo.join(&rel), self.dir.join(&rel));
            if to.exists() || rel.starts_with(".penelope") || rel.starts_with(".git/") {
                continue;
            }
            if LINKED.contains(&name) && from.is_dir() {
                if let Some(parent) = to.parent() {
                    let _ = fs::create_dir_all(parent);
                }
                #[cfg(unix)]
                let linked = std::os::unix::fs::symlink(&from, &to).is_ok();
                #[cfg(windows)]
                let linked = std::os::windows::fs::symlink_dir(&from, &to).is_ok();
                if linked {
                    brought.push(rel);
                }
            } else if BUILT.iter().any(|b| name.ends_with(b)) && from.is_file() {
                if let Some(parent) = to.parent() {
                    let _ = fs::create_dir_all(parent);
                }
                if fs::hard_link(&from, &to).is_ok() || fs::copy(&from, &to).is_ok() {
                    brought.push(rel);
                }
            }
        }
        let lines: String = brought.iter().map(|r| format!("/{}\n", r)).collect();
        let _ = fs::write(&self.excludes, lines);
    }

    /// Stages everything except the test and returns the diff against HEAD with its count of changed lines.
    fn staged_diff(&self, test_rel: &str) -> Result<(usize, String), String> {
        let exclude = format!(":(exclude){}", test_rel);
        let ignore = format!("core.excludesFile={}", self.excludes.to_string_lossy());
        git(&self.dir, &["-c", &ignore, "add", "-A", "--", ".", &exclude])?;
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
        let _ = fs::remove_file(&self.excludes);
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
/// `allowed` lists the files model candidates may change (empty: any file but the risky ones). `default_run` is true
/// when `run` came from the file ending. `deadline` ends the whole check; `sandboxed` runs every command in bubblewrap.
pub struct Job {
    top: PathBuf,
    sub: PathBuf,
    test_rel: String,
    test_arg: String,
    run: String,
    module: String,
    timeout: u64,
    allowed: Vec<String>,
    default_run: bool,
    deadline: Instant,
    sandboxed: bool,
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
        let by_ending = run.trim().is_empty();
        let run = if by_ending {
            default_run(&test_arg).ok_or_else(|| format!("no default command for {}. Pass run, for example `cargo test repro_issue` or `pytest {{test}}`.", test_arg))?.to_string()
        } else {
            run.trim().to_string()
        };
        let quoted = crate::platform::shell_quote(&test_arg);
        let run = if run.contains("{test}") { run.replace("{test}", &quoted) } else { run };
        Ok(Job { top, sub, test_rel, test_arg, run, module: module.trim().to_string(), timeout, allowed: Vec::new(), default_run: by_ending,
                 deadline: Instant::now() + Duration::from_secs(1200), sandboxed: false })
    }

    /// Sets the time budget of the whole check, counted from now.
    pub fn budget(mut self, seconds: u64) -> Job {
        self.deadline = Instant::now() + Duration::from_secs(seconds);
        self
    }

    fn seconds_left(&self) -> u64 {
        self.deadline.saturating_duration_since(Instant::now()).as_secs()
    }

    fn workdir(&self, sandbox: &Sandbox) -> PathBuf {
        sandbox.dir.join(&self.sub)
    }

    /// Runs a command of the job in the working folder of a sandbox, with the sandbox (and its `src` folder, for a src
    /// layout) first on PYTHONPATH. A command that would start after the deadline is skipped.
    fn exec(&self, command: &str, sandbox: &Sandbox) -> Run {
        let left = self.seconds_left();
        if left == 0 {
            return Run { ok: false, code: -1, timed_out: false, skipped: true, tail: "the time budget of the check (check.budget) is used up".to_string() };
        }
        let work = self.workdir(sandbox);
        let mut roots = vec![work.clone()];
        if work != sandbox.dir {
            roots.push(sandbox.dir.clone());
        }
        if sandbox.dir.join("src").is_dir() {
            roots.push(sandbox.dir.join("src"));
        }
        run_command_with(command, &work, &roots, self.timeout.min(left), if self.sandboxed { Some(&sandbox.dir) } else { None })
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
                    let patch = crate::platform::tmp_dir().join(format!("penelope-check-{}.diff", unique()));
                    fs::write(&patch, &diff).map_err(|e| e.to_string())?;
                    let applied = git(&sandbox.dir, &["apply", "--whitespace=nowarn", &patch.to_string_lossy()]);
                    let _ = fs::remove_file(&patch);
                    applied.map_err(|e| format!("the working-tree diff does not apply to HEAD: {}", e))?;
                }
                let others = git(&job.top, &["ls-files", "--others", "--exclude-standard", "-z"])?;
                let others: Vec<&str> = others.split('\0').filter(|r| !r.is_empty() && *r != job.test_rel && !r.starts_with(".penelope/")).collect();
                if let Some(rel) = others.iter().find(|r| runs_first(r)) {
                    return Err(format!("the working tree adds {}, which Python or pytest runs before every test, so the check refuses it", rel));
                }
                for rel in others {
                    copy_into(&job.top.join(rel), &sandbox.dir.join(rel))?;
                }
                Ok(())
            }
            Candidate::Blocks(_, blocks) => apply_blocks(&sandbox.dir, blocks, &job.allowed),
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
    /// The test fails with an import, syntax or collection error before and after the change: the environment of the
    /// worktree is broken, so the test shows nothing about the issue.
    EnvError,
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
            Status::EnvError => "env_error",
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

/// True when a failed run looks like a broken environment and not like the issue: an import, syntax or collection
/// error, or a pytest exit code for an interrupted run, an internal error, a usage error or no collected test (2 to 5).
pub fn env_failure(command: &str, r: &Run) -> bool {
    if r.ok || r.timed_out || r.skipped {
        return false;
    }
    const MARKS: &[&str] = &["ModuleNotFoundError", "ImportError", "SyntaxError", "ERROR collecting", "errors during collection",
                             "Cannot find module", "ERR_MODULE_NOT_FOUND"];
    MARKS.iter().any(|m| r.tail.contains(m)) || (command.contains("pytest") && matches!(r.code, 2..=5))
}

/// Decides one candidate from the runs. Kept apart from the runs so the rule has its own tests. `env` is true when
/// the test failed with an environment error both on HEAD and after the change.
pub fn decide(before_ok: bool, diff_lines: usize, after_ok: Option<bool>, module_after: Option<bool>, module_before: Option<bool>, env: bool) -> Status {
    if before_ok {
        return Status::PassesBefore;
    }
    if diff_lines == 0 {
        return Status::NoChange;
    }
    if after_ok != Some(true) {
        return if env { Status::EnvError } else { Status::StillFails };
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
    if before.skipped {
        return Verdict { before: Some(before.clone()), ..Verdict::error(before.tail.clone()) };
    }
    let after = match job.after(candidate) {
        Ok(a) => a,
        Err(e) => return Verdict { before: Some(before.clone()), ..Verdict::error(e) },
    };
    if let Some(r) = after.test.iter().chain(after.module.iter()).find(|r| r.skipped) {
        return Verdict { before: Some(before.clone()), ..Verdict::error(r.tail.clone()) };
    }
    let module_ok = after.module.as_ref().map(|r| r.ok);
    if module_ok == Some(false) && module_before.is_none() {
        *module_before = Some(job.module_before().ok());
    }
    let mb = module_before.clone().flatten();
    let env = env_failure(&job.run, before) && after.test.as_ref().map(|r| env_failure(&job.run, r)).unwrap_or(false);
    let status = decide(false, after.diff_lines, after.test.as_ref().map(|r| r.ok), module_ok, mb.as_ref().map(|r| r.ok), env);
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
        Status::EnvError => format!("CHECK FAILED: env_error. `{}` fails with an import, syntax or collection error on the original code ({}) and after \
the change ({}), so it shows nothing about the issue. Likely cause: a dependency or build output the throwaway worktree lacks, or a broken \
test file. Fix the test or pass run with the right interpreter.{}", job.run, code(&v.before), code(&v.after), tail(&v.after)),
        Status::Error => format!("CHECK NOT RUN: {}", v.error),
    }
}

// ---------------------------------------------------------------------------------------------------------------
// The record of verdicts.

fn dir(ctx: &Ctx) -> PathBuf {
    ctx.root.join("check")
}

/// The session of this process. The MCP server that the claude command starts gets it in `PENELOPE_CHECK_SESSION`, so
/// a check there and the verdict line here read the same record.
pub fn session() -> String {
    static S: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    S.get_or_init(|| std::env::var("PENELOPE_CHECK_SESSION").ok().filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| format!("{}-{}", std::process::id(), now_ms()))).clone()
}

/// The record folder of one task: a hash of the repository root and the session. Two eval tasks in parallel, or two
/// sessions on one machine, never read each other's last check.
fn task_record(ctx: &Ctx, top: &Path) -> PathBuf {
    let top = top.canonicalize().unwrap_or_else(|_| top.to_path_buf());
    let mut h: u64 = 0xcbf29ce484222325;
    for b in format!("{}\n{}", top.to_string_lossy(), session()).bytes() {
        h = (h ^ b as u64).wrapping_mul(0x100000001b3);
    }
    dir(ctx).join(format!("{:016x}", h))
}

/// The repository root of the task this thread works on, or the folder itself outside git.
fn current_top() -> PathBuf {
    let d = task_dir("");
    git(&d, &["rev-parse", "--show-toplevel"]).map(|t| PathBuf::from(t.trim())).unwrap_or(d)
}

fn repo_name(top: &Path) -> String {
    top.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
}

/// Appends one line to `log.jsonl`; a check (not a turn note) also becomes `last.json` of its task.
fn record(ctx: &Ctx, top: &Path, mut entry: Value, is_check: bool) {
    if let Some(o) = entry.as_object_mut() {
        o.insert("repo".into(), json!(repo_name(top)));
        o.insert("turn_ms".into(), json!(turn_start()));
    }
    let d = dir(ctx);
    let _ = fs::create_dir_all(&d);
    if is_check {
        let t = task_record(ctx, top);
        let _ = fs::create_dir_all(&t);
        let _ = fs::write(t.join("last.json"), serde_json::to_string_pretty(&entry).unwrap_or_default());
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

fn last_for(ctx: &Ctx, top: &Path) -> Option<Value> {
    fs::read_to_string(task_record(ctx, top).join("last.json")).ok().and_then(|t| serde_json::from_str(&t).ok())
}

// ---------------------------------------------------------------------------------------------------------------
// Best-of-N.

/// Files that Python or pytest loads before any test runs. A new one can make every test pass: a one-line
/// `sitecustomize.py` with `import os; os._exit(0)` turns exit 1 into exit 0.
fn runs_first(rel: &str) -> bool {
    let name = rel.rsplit('/').next().unwrap_or(rel).to_ascii_lowercase();
    matches!(name.as_str(), "sitecustomize.py" | "usercustomize.py" | "conftest.py") || name.ends_with(".pth")
}

/// Files that change how tests are collected or configured. A candidate may change one only when it is listed.
fn configures_tests(rel: &str) -> bool {
    let name = rel.rsplit('/').next().unwrap_or(rel).to_ascii_lowercase();
    runs_first(rel) || matches!(name.as_str(), "setup.cfg" | "pytest.ini" | "tox.ini" | "pyproject.toml")
}

/// Why a candidate may not write `rel`, or None when it may. `allowed` empty means any file but the risky ones.
fn refused(rel: &str, allowed: &[String]) -> Option<String> {
    let listed = allowed.iter().any(|a| safe_rel(a).as_deref() == Some(rel));
    if configures_tests(rel) && !listed {
        return Some(format!("{} changes how every test runs; a candidate may change it only when files lists it", rel));
    }
    if !allowed.is_empty() && !listed {
        return Some(format!("{} is outside the files the candidates may change", rel));
    }
    None
}

fn crlf(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\n', "\r\n")
}

/// One SEARCH/REPLACE edit. An empty `search` creates a file that does not exist yet.
#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub path: String,
    pub search: String,
    pub replace: String,
}

/// Reads SEARCH/REPLACE blocks. The file path is the last non-empty line before `<<<<<<< SEARCH` and after the
/// previous block, without fences, backticks or a leading `###`. A block without its own path line keeps the path of
/// the block before it.
pub fn parse_blocks(text: &str) -> Vec<Block> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out: Vec<Block> = Vec::new();
    let (mut i, mut from) = (0, 0);
    while i < lines.len() {
        if lines[i].trim_end() != "<<<<<<< SEARCH" {
            i += 1;
            continue;
        }
        let own = lines[from..i].iter().rev().map(|l| l.trim()).find(|l| !l.is_empty() && !l.starts_with("```"))
            .map(|l| l.trim_start_matches('#').trim().trim_matches('`').trim().to_string());
        let path = own.or_else(|| out.last().map(|b| b.path.clone())).unwrap_or_default();
        let Some(mid) = (i + 1..lines.len()).find(|&j| lines[j].trim_end() == "=======") else { break };
        let Some(end) = (mid + 1..lines.len()).find(|&j| lines[j].trim_end() == ">>>>>>> REPLACE") else { break };
        let join = |part: &[&str]| if part.is_empty() { String::new() } else { format!("{}\n", part.join("\n")) };
        out.push(Block { path, search: join(&lines[i + 1..mid]), replace: join(&lines[mid + 1..end]) });
        i = end + 1;
        from = i;
    }
    out
}

/// Applies blocks under `root`. A search text must occur in the file; the first occurrence is replaced. A file with
/// CRLF line endings keeps them. Every path must pass `refused` against `allowed`.
pub fn apply_blocks(root: &Path, blocks: &[Block], allowed: &[String]) -> Result<(), String> {
    if blocks.is_empty() {
        return Err("the answer holds no SEARCH/REPLACE block".to_string());
    }
    for b in blocks {
        let rel = safe_rel(&b.path).ok_or_else(|| format!("bad path in a block: {:?}", b.path))?;
        if let Some(why) = refused(&rel, allowed) {
            return Err(why);
        }
        let path = root.join(&rel);
        let old = fs::read_to_string(&path).ok();
        let new = match (&old, b.search.is_empty()) {
            (None, true) => b.replace.clone(),
            (Some(_), true) => return Err(format!("a block has an empty SEARCH, but {} exists; quote the lines to replace", rel)),
            (None, false) => return Err(format!("{} does not exist", rel)),
            (Some(text), false) => {
                let (search, replace) = if text.contains("\r\n") { (crlf(&b.search), crlf(&b.replace)) } else { (b.search.clone(), b.replace.clone()) };
                match text.find(&search) {
                    Some(at) => format!("{}{}{}", &text[..at], replace, &text[at + search.len()..]),
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

/// One file shown to the candidate models: its path, the text (whole, or the kept ranges with cut markers) and a note
/// that names the ranges when lines were cut.
struct Shown {
    path: String,
    text: String,
    note: String,
}

fn candidate_prompt(issue: &str, test_arg: &str, test_text: &str, files: &[Shown], hint: &str) -> String {
    let mut p = format!("Issue:\n{}\n\nReproduction test {}:\n```\n{}\n```\n\nFiles at the original commit:\n", issue.trim(), test_arg, test_text.trim_end());
    for f in files {
        p.push_str(&format!("\n{}{}\n```\n{}\n```\n", f.path, f.note, f.text.trim_end()));
    }
    p.push_str(&format!("\n{}\nDo not change the test. SEARCH text must come from the lines shown.", hint));
    p
}

const CANDIDATE_SYSTEM: &str = "You fix one issue in a code base. Answer only with SEARCH/REPLACE blocks in this format:\n\
path/to/file.py\n<<<<<<< SEARCH\nexact lines from the file\n=======\nnew lines\n>>>>>>> REPLACE\n\
The SEARCH lines must match the file exactly, including indentation. Use as few blocks as needed. No other text.";

/// Characters of source the candidate prompt may hold.
const SOURCE_BUDGET: usize = 40_000;
/// Lines of context kept around each hunk of the lead's change when a file is too long to send whole.
const CONTEXT: usize = 80;

/// The line ranges (1-based, inclusive) at HEAD that the lead's change touches in `rel`, from `git diff -U0`.
fn changed_ranges(job: &Job, rel: &str) -> Vec<(usize, usize)> {
    let diff = git(&job.top, &["diff", "-U0", "HEAD", "--", rel]).unwrap_or_default();
    diff.lines().filter(|l| l.starts_with("@@ -")).filter_map(|l| {
        let old = l.trim_start_matches("@@ -").split(' ').next()?;
        let mut it = old.split(',');
        let start: usize = it.next()?.parse().ok()?;
        let count: usize = it.next().map(|c| c.parse().unwrap_or(1)).unwrap_or(1);
        let start = start.max(1);
        Some((start, start + count.max(1) - 1))
    }).collect()
}

/// Cuts `lines` down to the ranges `keep` (1-based, inclusive, merged here) and at most `budget` characters. Returns
/// the text with a marker line for every cut, and the ranges kept.
fn excerpt(lines: &[&str], keep: &[(usize, usize)], budget: usize) -> (String, Vec<(usize, usize)>) {
    let mut ranges: Vec<(usize, usize)> = keep.iter().map(|&(a, b)| (a.max(1), b.min(lines.len()))).filter(|(a, b)| a <= b).collect();
    ranges.sort();
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for r in ranges {
        match merged.last_mut() {
            Some(last) if r.0 <= last.1 + 1 => last.1 = last.1.max(r.1),
            _ => merged.push(r),
        }
    }
    let (mut out, mut kept, mut used, mut next) = (String::new(), Vec::new(), 0usize, 1usize);
    let mut full = false;
    for (a, b) in merged {
        if a > next {
            out.push_str(&format!("... (lines {} to {} cut) ...\n", next, a - 1));
        }
        let mut last = a - 1;
        for line in &lines[a - 1..b] {
            if used + line.len() + 1 > budget {
                full = true;
                break;
            }
            out.push_str(line);
            out.push('\n');
            used += line.len() + 1;
            last += 1;
        }
        if last >= a {
            kept.push((a, last));
        }
        next = last + 1;
        if full {
            break;
        }
    }
    if next <= lines.len() {
        out.push_str(&format!("... (lines {} to {} cut) ...\n", next, lines.len()));
    }
    (out, kept)
}

/// Reads the files the candidates may change, at HEAD, at most 40,000 characters in all. A file that fits goes whole.
/// A longer one goes as the lead's changed hunks with 80 lines around each, or from its first line when the lead did
/// not change it, and the prompt names the ranges shown. A file that no longer fits is named in `notes`.
fn head_files(job: &Job, files: &[String], notes: &mut Vec<String>) -> Vec<Shown> {
    let mut out = Vec::new();
    let mut budget = SOURCE_BUDGET;
    for f in files {
        let Some(rel) = safe_rel(f) else { continue };
        let Ok(text) = git(&job.top, &["show", &format!("HEAD:{}", rel)]) else { continue };
        if text.len() <= budget {
            budget -= text.len();
            out.push(Shown { path: rel, text, note: String::new() });
            continue;
        }
        if budget < 2_000 {
            notes.push(format!("{}: not shown to the candidates, the source budget of {} characters is used up", rel, SOURCE_BUDGET));
            continue;
        }
        let lines: Vec<&str> = text.lines().collect();
        let hunks = changed_ranges(job, &rel);
        let keep: Vec<(usize, usize)> = if hunks.is_empty() { vec![(1, lines.len())] } else {
            hunks.iter().map(|&(a, b)| (a.saturating_sub(CONTEXT).max(1), b + CONTEXT)).collect()
        };
        let (shown, kept) = excerpt(&lines, &keep, budget);
        budget -= shown.len().min(budget);
        let ranges = kept.iter().map(|(a, b)| format!("{}-{}", a, b)).collect::<Vec<_>>().join(", ");
        notes.push(format!("{}: lines {} of {} shown to the candidates", rel, ranges, lines.len()));
        out.push(Shown { path: rel, text: shown, note: format!(" (only lines {} of {} are shown; the rest is cut)", ranges, lines.len()) });
    }
    out
}

fn changed_files(job: &Job) -> Vec<String> {
    let exclude = format!(":(exclude){}", job.test_rel);
    git(&job.top, &["diff", "HEAD", "--name-only", "--", ".", &exclude]).unwrap_or_default().lines().map(str::to_string).collect()
}

/// Best-of-N: the working tree (when it has a change) and `n` candidates from the candidate models. The working tree
/// wins when it passes; otherwise the first model candidate in diff-size order that passes wins. The winning diff goes
/// to `winner.diff` in the record folder of the task. Candidates may change only `files` (empty: the files the lead
/// changed).
pub fn best_of(ctx: &Ctx, job: Job, issue: &str, files: &[String], n: usize) -> String {
    let s = settings(ctx);
    let files: Vec<String> = if files.is_empty() { changed_files(&job) } else { files.iter().filter_map(|f| safe_rel(f)).collect() };
    let job = Job { allowed: files.clone(), ..job };
    let mut notes = Vec::new();
    let sources = head_files(&job, &files, &mut notes);
    let test_text = fs::read_to_string(job.top.join(&job.test_rel)).unwrap_or_default();
    let mut candidates = Vec::new();
    if !changed_files(&job).is_empty() || has_untracked(&job) {
        candidates.push(Candidate::WorkingTree);
    }
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
    best_of_candidates(ctx, &job, candidates, notes)
}

fn has_untracked(job: &Job) -> bool {
    git(&job.top, &["ls-files", "--others", "--exclude-standard", "-z"]).unwrap_or_default()
        .split('\0').any(|r| !r.is_empty() && r != job.test_rel && !r.starts_with(".penelope/"))
}

/// The order of the candidates: the working tree first when it has a change, then the others by diff size, smallest
/// first, the earlier one on a tie. Candidates without a diff (`None`: did not apply, or empty) are left out.
pub fn selection_order(sizes: &[Option<usize>], working_tree: Option<usize>) -> Vec<usize> {
    let mut idx: Vec<usize> = Vec::new();
    if let Some(w) = working_tree.filter(|&w| matches!(sizes.get(w), Some(Some(n)) if *n > 0)) {
        idx.push(w);
    }
    let rest: Vec<Option<usize>> = sizes.iter().enumerate().map(|(i, s)| if Some(i) == working_tree { None } else { *s }).collect();
    idx.extend(order(&rest));
    idx
}

/// The selection part of best-of-N, without model calls, so tests can hand in fixed candidates. When the working tree
/// passes, it wins, and the model candidates still run for the report while the time budget lasts.
pub fn best_of_candidates(ctx: &Ctx, job: &Job, candidates: Vec<Candidate>, mut notes: Vec<String>) -> String {
    let record_dir = task_record(ctx, &job.top);
    let before = match job.before() {
        Ok(r) => r,
        Err(e) => return format!("CHECK NOT RUN: {}", e),
    };
    if before.skipped {
        return format!("CHECK NOT RUN: {}", before.tail);
    }
    if before.ok {
        let v = Verdict { status: Status::PassesBefore, before: Some(before), after: None, module_after: None, module_before: None,
                          diff_lines: 0, diff: String::new(), error: String::new() };
        record(ctx, &job.top, entry("best", job, &v, json!({"candidates": candidates.len()})), true);
        return describe(job, &v);
    }
    // Diff sizes first: applying is cheap, the tests are not.
    let sizes: Vec<Result<usize, String>> = candidates.iter().map(|c| {
        let sandbox = Sandbox::new(&job.top)?;
        c.apply(job, &sandbox)?;
        sandbox.staged_diff(&job.test_rel).map(|(n, _)| n)
    }).collect();
    for (i, c) in candidates.iter().enumerate() {
        match &sizes[i] {
            Err(e) => notes.push(format!("{}: does not apply ({})", c.label(), e)),
            Ok(0) => notes.push(format!("{}: no change", c.label())),
            Ok(n) => notes.push(format!("{}: {} lines", c.label(), n)),
        }
    }
    let plain: Vec<Option<usize>> = sizes.iter().map(|r| r.as_ref().ok().copied()).collect();
    let wt = candidates.iter().position(|c| matches!(c, Candidate::WorkingTree));
    let mut module_before = None;
    let mut tried = Vec::new();
    let mut winner: Option<(usize, Verdict)> = None;
    let mut env_seen = false;
    for i in selection_order(&plain, wt) {
        if job.seconds_left() == 0 {
            tried.push(format!("{}: not run, the time budget is used up", candidates[i].label()));
            continue;
        }
        let v = check_one(job, &before, &candidates[i], &mut module_before);
        env_seen |= v.status == Status::EnvError;
        let report_only = winner.is_some();
        tried.push(format!("{}: {}{}", candidates[i].label(), v.status.word(), if report_only { " (report only)" } else { "" }));
        if v.status == Status::Passed && winner.is_none() {
            let wins_working_tree = matches!(candidates[i], Candidate::WorkingTree);
            winner = Some((i, v));
            if !wins_working_tree {
                break;
            }
        }
    }
    if let Some((i, v)) = winner {
        let _ = fs::create_dir_all(&record_dir);
        let _ = fs::write(record_dir.join("winner.diff"), &v.diff);
        record(ctx, &job.top, entry("best", job, &v, json!({"candidates": candidates.len(), "winner": candidates[i].label()})), true);
        return format!("BEST OF {}: {} wins. {}\nWinner diff: {}{}\nTried: {}\nCandidates: {}", candidates.len(), candidates[i].label(),
                       describe(job, &v), record_dir.join("winner.diff").display(),
                       if matches!(candidates[i], Candidate::WorkingTree) { " (already in the working tree)" } else { "; apply with git apply" },
                       tried.join("; "), notes.join("; "));
    }
    let status = if env_seen && env_failure(&job.run, &before) { Status::EnvError } else { Status::StillFails };
    let v = Verdict { status, before: Some(before), after: None, module_after: None, module_before: None,
                      diff_lines: 0, diff: String::new(), error: String::new() };
    record(ctx, &job.top, entry("best", job, &v, json!({"candidates": candidates.len()})), true);
    format!("BEST OF {}: no candidate passed ({}). The test fails on the original code. Tried: {}\nCandidates: {}",
            candidates.len(), status.word(), if tried.is_empty() { "none".to_string() } else { tried.join("; ") }, notes.join("; "))
}

// ---------------------------------------------------------------------------------------------------------------
// Entry points: the tool, the command and the turn.

fn task_dir(cwd: &str) -> PathBuf {
    let cwd = if cwd.trim().is_empty() { crate::eval::task_cwd() } else { cwd.to_string() };
    if cwd.trim().is_empty() { std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")) } else { PathBuf::from(cwd) }
}

/// A yes from the person for the default interpreter run of a model-written test. It lasts for the process, which is
/// the session.
static INTERPRETER_OK: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The gate in front of every command of a check, as strict as the `run` tool. A named `run` or `module_tests` command
/// passes when it is on the safe list or the project allow list; otherwise the person is asked once and a yes adds it
/// to the allow list. The default interpreter run (`python3 TEST`) executes code the model wrote, so it needs one yes
/// per session. `run_policy` `all` lets everything pass; a scout never runs a check outside the safe list. `ask` stands
/// for `tools::ask_person`, so the tests can answer.
fn permit_with(ctx: &Ctx, job: &Job, ask: &dyn Fn(&str, &str) -> Option<bool>) -> Option<String> {
    if ctx.cfg["run_policy"].as_str().unwrap_or("safe") == "all" {
        return None;
    }
    let mut named: Vec<&str> = Vec::new();
    if !job.default_run {
        named.push(&job.run);
    }
    if !job.module.is_empty() {
        named.push(&job.module);
    }
    let unsafe_named: Vec<&str> = named.into_iter().filter(|c| !crate::tools::is_safe(c) && !crate::tools::is_allowed(ctx, c)).collect();
    let interpreter = job.default_run && !INTERPRETER_OK.load(std::sync::atomic::Ordering::SeqCst);
    if unsafe_named.is_empty() && !interpreter {
        return None;
    }
    if crate::scout::inside() {
        return Some("CHECK NOT RUN. A scout runs commands of the safe list only.".to_string());
    }
    let mut question = Vec::new();
    if interpreter {
        question.push(format!("verify_fix runs the model-written test with `{}`", job.run));
    }
    for c in &unsafe_named {
        question.push(format!("verify_fix runs `{}`", c));
    }
    match ask(&question.join("; "), &job.top.to_string_lossy()) {
        Some(true) => {
            for c in unsafe_named {
                crate::tools::allow_command(ctx, c);
            }
            if interpreter {
                INTERPRETER_OK.store(true, std::sync::atomic::Ordering::SeqCst);
            }
            None
        }
        Some(false) => Some("CHECK NOT RUN. The user declined the commands of the check.".to_string()),
        None => Some(format!("CHECK NOT RUN. {} is not on the safe list and nobody can confirm it here. Pass a run command from the \
safe list (for example `pytest {{test}}`), or the user allows every command with \"run_policy\": \"all\" in .penelope/config.json.",
                             question.join("; ").replace("verify_fix runs ", ""))),
    }
}

/// Builds a job from the settings and passes the gate: the time budget, bubblewrap when `sandbox` is `bwrap`, and the
/// permission of the commands.
fn prepared(ctx: &Ctx, cwd: &str, test: &str, run: &str, module: &str) -> Result<Job, String> {
    let s = settings(ctx);
    let mut job = Job::new(&task_dir(cwd), test, run, module, s.timeout)?.budget(s.budget);
    if ctx.cfg["sandbox"].as_str().unwrap_or("off") == "bwrap" {
        if crate::config::which("bwrap").is_none() {
            return Err("the sandbox needs bubblewrap (bwrap) on the PATH.".to_string());
        }
        job.sandboxed = true;
    }
    if let Some(refusal) = permit_with(ctx, &job, &|q, cwd| crate::tools::ask_person(q, cwd, "command", "")) {
        return Err(refusal.trim_start_matches("CHECK NOT RUN. ").to_string());
    }
    Ok(job)
}

/// The tool `verify_fix`. Test output passes the privacy guard before it reaches the model.
pub fn verify(ctx: &Ctx, args: &Value) -> String {
    let s = settings(ctx);
    let str_arg = |k: &str| args[k].as_str().unwrap_or("").to_string();
    let module = { let m = str_arg("module_tests"); if m.trim().is_empty() { s.module_tests.clone() } else { m } };
    let run = { let r = str_arg("run"); if r.trim().is_empty() { s.run.clone() } else { r } };
    let job = match prepared(ctx, &str_arg("cwd"), &str_arg("test"), &run, &module) {
        Ok(j) => j,
        Err(e) => return format!("CHECK NOT RUN: {}", e),
    };
    let n = args["best_of"].as_u64().unwrap_or(0) as usize;
    if n > 0 {
        let files: Vec<String> = args["files"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default();
        return crate::tools::guard(ctx, &best_of(ctx, job, &str_arg("issue"), &files, n.min(8)));
    }
    let v = check_working_tree(&job);
    record(ctx, &job.top, entry("tool", &job, &v, json!({})), true);
    crate::tools::guard(ctx, &describe(&job, &v))
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
    match last_for(ctx, &current_top()) {
        Some(l) => out.push_str(&format!("last check in this session: {} on {}, diff {} lines\n", l["status"].as_str().unwrap_or("?"), l["test"].as_str().unwrap_or("?"), l["diff_lines"])),
        None => out.push_str("last check in this session: none\n"),
    }
    out.push_str(&log_summary(&fs::read_to_string(dir(ctx).join("log.jsonl")).unwrap_or_default()));
    out.push_str("usage: /check TEST [--run CMD] [--module CMD] [--cwd DIR]; /check best TEST [N] [--files a,b] -- ISSUE; /check gate|report|off\n");
    out
}

fn bump(counts: &mut Vec<(String, usize)>, key: String) {
    match counts.iter_mut().find(|(k, _)| *k == key) {
        Some(slot) => slot.1 += 1,
        None => counts.push((key, 1)),
    }
}

/// The counts of the log: by source and verdict, the verdicts that point at the sandbox or the test per repository
/// (env_error, test_passes_before, still_fails), and the turns where a failed check was followed by a passed one.
fn log_summary(log: &str) -> String {
    let (mut counts, mut per_repo) = (Vec::new(), Vec::new());
    let mut turns: Vec<(String, bool, bool)> = Vec::new();   // (repo and turn, a failed check seen, a pass after it)
    for line in log.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        let (source, status) = (v["source"].as_str().unwrap_or("?"), v["status"].as_str().unwrap_or("?"));
        bump(&mut counts, format!("{}:{}", source, status));
        let repo = v["repo"].as_str().unwrap_or("?");
        if matches!(status, "env_error" | "test_passes_before" | "still_fails") {
            bump(&mut per_repo, format!("{} {}", repo, status));
        }
        if source == "turn" || v["turn_ms"].as_u64().unwrap_or(0) == 0 {
            continue;   // a check outside a model turn (the CLI command) has no turn to repair in
        }
        let turn = format!("{}@{}", repo, v["turn_ms"].as_u64().unwrap_or(0));
        let slot = match turns.iter().position(|t| t.0 == turn) {
            Some(i) => i,
            None => { turns.push((turn, false, false)); turns.len() - 1 }
        };
        if status == "passed" && turns[slot].1 {
            turns[slot].2 = true;
        } else if status != "passed" {
            turns[slot].1 = true;
        }
    }
    let mut out = String::new();
    if !counts.is_empty() {
        out.push_str(&format!("log: {}\n", counts.iter().map(|(k, n)| format!("{} {}", k, n)).collect::<Vec<_>>().join(", ")));
    }
    if !per_repo.is_empty() {
        out.push_str(&format!("per repository: {}\n", per_repo.iter().map(|(k, n)| format!("{} {}", k, n)).collect::<Vec<_>>().join(", ")));
    }
    let repaired = turns.iter().filter(|t| t.2).count();
    if repaired > 0 {
        out.push_str(&format!("repairs: {} turns where a failed check was followed by a passed one\n", repaired));
    }
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
    let job = match prepared(ctx, &cwd, &test, &run, &module) {
        Ok(j) => j,
        Err(e) => return format!("CHECK NOT RUN: {}", e),
    };
    if best {
        return crate::tools::guard(ctx, &best_of(ctx, job, &issue, &files, n));
    }
    let v = check_working_tree(&job);
    record(ctx, &job.top, entry("command", &job, &v, json!({})), true);
    crate::tools::guard(ctx, &describe(&job, &v))
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
    let record_dir = task_record(ctx, &current_top());
    ROOT.with(|r| *r.borrow_mut() = Some(record_dir));
}

/// True when the record folder of the task holds a check from `start` on.
fn checked_since(record_dir: &Path, start: u64) -> bool {
    fs::read_to_string(record_dir.join("last.json")).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok())
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
    turn_note_for(ctx, trace, start, &current_top())
}

/// `turn_note` for the task in the repository `top`.
fn turn_note_for(ctx: &Ctx, trace: &[crate::tools::Call], start: u64, top: &Path) -> Option<String> {
    if settings(ctx).mode == "off" {
        return None;
    }
    let edited = ARMED.with(|a| a.get())
        || trace.iter().any(|(name, detail, ok, _)| *ok && edit_name(name) && detail.split(|c: char| c.is_whitespace() || "\"',:{}".contains(c)).any(is_code));
    if !edited {
        return None;
    }
    let line = match last_for(ctx, top).filter(|l| l["time_ms"].as_u64().unwrap_or(0) >= start) {
        Some(l) if l["status"] == "passed" => "Checker: verified. The reproduction test failed on the original code and passes after the fix.".to_string(),
        Some(l) => format!("Checker: NOT verified ({}). See /check.", l["status"].as_str().unwrap_or("?")),
        None => {
            record(ctx, top, json!({"time_ms": now_ms(), "source": "turn", "status": "unverified"}), false);
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
        assert_eq!(decide(true, 3, Some(true), Some(true), None, false), Status::PassesBefore);
        assert_eq!(decide(false, 0, None, None, None, false), Status::NoChange);
        assert_eq!(decide(false, 2, Some(false), None, None, false), Status::StillFails);
        assert_eq!(decide(false, 2, Some(false), None, None, true), Status::EnvError, "an import error before and after shows nothing");
        assert_eq!(decide(false, 2, Some(true), Some(false), Some(true), false), Status::BreaksModule);
        assert_eq!(decide(false, 2, Some(true), Some(false), Some(false), false), Status::Passed, "module tests that failed on HEAD do not count");
        assert_eq!(decide(false, 2, Some(true), None, None, false), Status::Passed);
        let run = |code: i32, tail: &str| Run { ok: code == 0, code, timed_out: false, tail: tail.to_string(), skipped: false };
        assert!(env_failure("python3 t.py", &run(1, "ModuleNotFoundError: No module named 'sklearn.utils._cython'")));
        assert!(env_failure("pytest t.py", &run(2, "1 error in 0.1s")) && env_failure("pytest t.py", &run(3, "INTERNALERROR>")) && env_failure("pytest t.py", &run(4, "")));
        assert!(!env_failure("python3 t.py", &run(1, "AssertionError: 3 != 5")) && !env_failure("sh t.sh", &run(2, "")));
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
        let mut ctx = ctx;
        ctx.cfg["run_policy"] = json!("all");   // `git grep` is not on the safe list
        let said = verify(&ctx, &json!({"test": "repro_test.txt", "run": REPRO, "module_tests": MODULE, "cwd": repo.to_string_lossy()}));
        assert!(said.starts_with("CHECK PASSED"), "{}", said);
        assert_eq!(last_for(&ctx, &j.top).unwrap()["status"], "passed");
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
        let two = parse_blocks("calc.txt\n<<<<<<< SEARCH\na\n=======\nb\n>>>>>>> REPLACE\n<<<<<<< SEARCH\nc\n=======\nd\n>>>>>>> REPLACE\n");
        assert_eq!(two.iter().map(|b| b.path.as_str()).collect::<Vec<_>>(), vec!["calc.txt", "calc.txt"], "a second block keeps the path");
        let dir = std::env::temp_dir().join(format!("penelope-checker-blocks-{}", unique()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("calc.txt"), "def add(a, b):\r\n    return a - b\r\n").unwrap();
        apply_blocks(&dir, &blocks, &[]).unwrap();
        assert_eq!(fs::read_to_string(dir.join("calc.txt")).unwrap(), "def add(a, b):\r\n    return a + b\r\n", "CRLF stays CRLF");
        assert_eq!(fs::read_to_string(dir.join("new/file.txt")).unwrap(), "hello\n");
        let miss = vec![Block { path: "calc.txt".into(), search: "nothing\n".into(), replace: "x\n".into() }];
        assert!(apply_blocks(&dir, &miss, &[]).unwrap_err().contains("does not occur"));
        let append = vec![Block { path: "calc.txt".into(), search: String::new(), replace: "x\n".into() }];
        assert!(apply_blocks(&dir, &append, &[]).unwrap_err().contains("empty SEARCH"), "an empty SEARCH never appends to a file");
        assert!(apply_blocks(&dir, &[Block { path: "../evil".into(), search: String::new(), replace: "x".into() }], &[]).is_err());
        let site = vec![Block { path: "sitecustomize.py".into(), search: String::new(), replace: "import os; os._exit(0)\n".into() }];
        assert!(apply_blocks(&dir, &site, &[]).unwrap_err().contains("every test"));
        assert!(!dir.join("sitecustomize.py").exists());
        let other = vec![Block { path: "new/other.txt".into(), search: String::new(), replace: "x\n".into() }];
        assert!(apply_blocks(&dir, &other, &["calc.txt".to_string()]).unwrap_err().contains("outside the files"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_long_file_is_cut_around_the_change_and_the_cut_is_named() {
        let lines: Vec<String> = (1..=1000).map(|n| format!("line {}", n)).collect();
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let (text, kept) = excerpt(&refs, &[(400, 420), (10, 20), (15, 30)], 100_000);
        assert_eq!(kept, vec![(10, 30), (400, 420)]);
        assert!(text.starts_with("... (lines 1 to 9 cut) ...\nline 10\n") && text.contains("... (lines 31 to 399 cut) ...") && text.ends_with("... (lines 421 to 1000 cut) ...\n"), "{}", text);
        let (small, kept) = excerpt(&refs, &[(1, 1000)], 80);
        assert_eq!(kept, vec![(1, 11)], "11 lines of 7 or 8 characters fit in 80; the budget ends the excerpt: {}", small);
    }

    #[test]
    fn the_order_is_smallest_diff_first_and_skips_empty_ones() {
        assert_eq!(order(&[Some(5), Some(2), None, Some(2), Some(0)]), vec![1, 3, 0]);
        assert!(order(&[None, Some(0)]).is_empty());
        assert_eq!(selection_order(&[Some(9), Some(2), Some(5)], Some(0)), vec![0, 1, 2], "the working tree goes first");
        assert_eq!(selection_order(&[None, Some(2), Some(1)], Some(0)), vec![2, 1], "a working tree that does not apply is left out");
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
        // One line that makes every Python test exit 0, and a change outside the allowed files: both are refused.
        let evil = Candidate::Blocks("evil".into(), blocks("sitecustomize.py\n<<<<<<< SEARCH\n=======\nimport os; os._exit(0)\n>>>>>>> REPLACE\n"));
        let outside = Candidate::Blocks("outside".into(), blocks("other.txt\n<<<<<<< SEARCH\nkeep\n=======\nkeep\na + b\n>>>>>>> REPLACE\n"));
        let j = Job { allowed: vec!["calc.txt".into()], ..j };
        let said = best_of_candidates(&ctx, &j, vec![big, wrong, broken, evil, outside, small], Vec::new());
        assert!(said.contains("small wins"), "{}", said);
        assert!(said.contains("broken: does not apply"), "{}", said);
        assert!(said.contains("evil: does not apply (sitecustomize.py changes how every test runs"), "{}", said);
        assert!(said.contains("outside: does not apply (other.txt is outside"), "{}", said);
        let winner = fs::read_to_string(task_record(&ctx, &j.top).join("winner.diff")).unwrap();
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
        assert!(t.elapsed() < Duration::from_secs(5), "killed near the 1 s limit, after {:?}", t.elapsed());
    }

    #[test]
    fn the_time_budget_ends_a_check() {
        let (repo, _ctx) = fixture("budget");
        fs::write(repo.join("calc.txt"), "def add(a, b):\n    return a + b\n").unwrap();
        let j = job(&repo).budget(0);
        let v = check_working_tree(&j);
        assert_eq!(v.status, Status::Error, "{}", describe(&j, &v));
        assert!(v.error.contains("time budget"), "{}", v.error);
        let _ = fs::remove_dir_all(repo.parent().unwrap());
    }

    fn python() -> Option<&'static str> {
        let python = if cfg!(windows) { "python" } else { "python3" };
        crate::config::which(python).map(|_| python)
    }

    /// The editable-install pitfall, run once with the path order and once without. A `.pth` file in a site folder
    /// points at the checkout of the user, which holds the fix, as `pip install -e .` would. Without the worktree first
    /// on PYTHONPATH the "original code" run imports the fixed file and passes; with it, it fails as it must.
    #[test]
    fn the_worktree_beats_an_editable_install_on_the_import_path() {
        let Some(python) = python() else { return };
        let (repo, _ctx) = fixture("python");
        fs::write(repo.join("calc.py"), "def add(a, b):\n    return a - b\n").unwrap();
        sh_git(&repo, &["add", "calc.py"]);
        sh_git(&repo, &["commit", "-q", "-m", "calc"]);
        fs::write(repo.join("calc.py"), "def add(a, b):\n    return a + b\n").unwrap();
        let site = repo.parent().unwrap().join("site");
        fs::create_dir_all(&site).unwrap();
        fs::write(site.join("editable.pth"), format!("{}\n", repo.display())).unwrap();
        fs::create_dir_all(repo.join("tests")).unwrap();
        fs::write(repo.join("tests").join("test_issue.py"), format!(
            "import site, sys\nsite.addsitedir({:?})\nfrom calc import add\nsys.exit(0 if add(2, 3) == 5 else 1)\n", site.to_string_lossy())).unwrap();
        let j = Job::new(&repo, "tests/test_issue.py", &format!("{} tests/test_issue.py", python), "", 60).unwrap();
        let v = check_working_tree(&j);
        assert_eq!(v.status, Status::Passed, "{}", describe(&j, &v));
        assert_eq!(v.before.as_ref().map(|r| r.code), Some(1), "the original code fails: {}", describe(&j, &v));
        let sandbox = Sandbox::new(&j.top).unwrap();
        copy_into(&repo.join("tests").join("test_issue.py"), &sandbox.dir.join("tests").join("test_issue.py")).unwrap();
        let without = run_command_in(&j.run, &sandbox.dir, &[], 60);
        assert!(without.ok, "without the path order the copy imports the fixed checkout: {:?}", without);
        drop(sandbox);
        let _ = fs::remove_dir_all(repo.parent().unwrap());
    }

    #[test]
    fn ignored_build_output_comes_along_and_hooks_stay_off() {
        let (repo, _ctx) = fixture("ignored");
        fs::write(repo.join(".gitignore"), "*.so\nnode_modules/\n").unwrap();
        sh_git(&repo, &["add", ".gitignore"]);
        sh_git(&repo, &["commit", "-q", "-m", "ignore"]);
        fs::create_dir_all(repo.join("pkg")).unwrap();
        fs::write(repo.join("pkg").join("_ext.so"), "binary").unwrap();
        fs::create_dir_all(repo.join("node_modules").join("dep")).unwrap();
        fs::write(repo.join("node_modules").join("dep").join("index.js"), "module.exports = 1\n").unwrap();
        let hooks = repo.join(".git").join("hooks");
        fs::create_dir_all(&hooks).unwrap();
        let marker = repo.parent().unwrap().join("hook-ran");
        fs::write(hooks.join("post-checkout"), format!("#!/bin/sh\ntouch '{}'\n", marker.display())).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(hooks.join("post-checkout"), fs::Permissions::from_mode(0o755)).unwrap();
        }
        let sandbox = Sandbox::new(&repo).unwrap();
        assert!(sandbox.dir.join("pkg").join("_ext.so").is_file(), "an in-place extension is copied");
        // Windows makes a folder link only with developer mode, so only Unix asserts it.
        assert!(cfg!(windows) || sandbox.dir.join("node_modules").join("dep").join("index.js").is_file(), "node_modules is linked");
        assert_eq!(sandbox.staged_diff("none.txt").unwrap().0, 0, "what the sandbox brought in is no change");
        assert!(!marker.exists(), "the post-checkout hook of the user did not run");
        drop(sandbox);
        assert!(repo.join("node_modules").join("dep").join("index.js").is_file(), "removing the sandbox keeps the real folder");
        let _ = fs::remove_dir_all(repo.parent().unwrap());
    }

    #[test]
    fn a_new_sitecustomize_in_the_working_tree_is_refused() {
        let (repo, _ctx) = fixture("site");
        fs::write(repo.join("calc.txt"), "def add(a, b):\n    return a + b\n").unwrap();
        fs::write(repo.join("sitecustomize.py"), "import os; os._exit(0)\n").unwrap();
        let j = job(&repo);
        let v = check_working_tree(&j);
        assert_eq!(v.status, Status::Error, "{}", describe(&j, &v));
        assert!(v.error.contains("sitecustomize.py"), "{}", v.error);
        let _ = fs::remove_dir_all(repo.parent().unwrap());
    }

    #[test]
    fn a_test_that_cannot_import_gives_env_error() {
        let Some(python) = python() else { return };
        let (repo, _ctx) = fixture("env");
        fs::write(repo.join("calc.py"), "def add(a, b):\n    return a - b\n").unwrap();
        sh_git(&repo, &["add", "calc.py"]);
        sh_git(&repo, &["commit", "-q", "-m", "calc"]);
        fs::write(repo.join("calc.py"), "def add(a, b):\n    return a + b\n").unwrap();
        fs::write(repo.join("test_issue.py"), "import penelope_missing_build_output\nfrom calc import add\nassert add(2, 3) == 5\n").unwrap();
        let j = Job::new(&repo, "test_issue.py", &format!("{} test_issue.py", python), "", 60).unwrap();
        let v = check_working_tree(&j);
        assert_eq!(v.status, Status::EnvError, "{}", describe(&j, &v));
        assert!(describe(&j, &v).contains("env_error"));
        let _ = fs::remove_dir_all(repo.parent().unwrap());
    }

    #[test]
    fn commands_off_the_safe_list_do_not_run_without_a_yes() {
        let (repo, mut ctx) = fixture("permit");
        fs::write(repo.join("repro_test.txt"), "x\n").unwrap();
        ctx.cfg["run_policy"] = json!("safe");
        let nobody = |_: &str, _: &str| None;
        let j = Job::new(&repo, "repro_test.txt", "rm -rf x; true", "", 60).unwrap();
        assert!(permit_with(&ctx, &j, &nobody).unwrap().starts_with("CHECK NOT RUN"));
        assert!(permit_with(&ctx, &j, &|_: &str, _: &str| Some(false)).unwrap().contains("declined"));
        let safe = Job::new(&repo, "repro_test.txt", "git diff --stat", "", 60).unwrap();
        assert!(permit_with(&ctx, &safe, &nobody).is_none(), "a safe-list command runs");
        fs::write(repo.join("t.py"), "print(1)\n").unwrap();
        let interp = Job::new(&repo, "t.py", "", "", 60).unwrap();
        assert!(interp.default_run && permit_with(&ctx, &interp, &nobody).unwrap().contains("model-written test"));
        ctx.cfg["run_policy"] = json!("all");
        assert!(permit_with(&ctx, &j, &nobody).is_none());
        let _ = fs::remove_dir_all(repo.parent().unwrap());
    }

    #[test]
    fn two_tasks_do_not_see_each_other_s_check() {
        let (repo_a, ctx) = fixture("key-a");
        let (repo_b, _) = fixture("key-b");
        let start = now_ms();
        let (ctx_a, a) = (ctx.clone(), repo_a.clone());
        std::thread::spawn(move || record(&ctx_a, &a, json!({"time_ms": now_ms(), "source": "tool", "status": "passed"}), true)).join().unwrap();
        let (ctx_b, b) = (ctx.clone(), repo_b.clone());
        let seen = std::thread::spawn(move || (checked_since(&task_record(&ctx_b, &b), start), turn_note_for(&ctx_b, &[], start, &b))).join().unwrap();
        assert!(!seen.0, "the pass of task A does not silence the reminder of task B");
        assert_eq!(seen.1, None, "nothing armed in that thread, so no line");
        assert!(checked_since(&task_record(&ctx, &repo_a), start));
        assert_ne!(task_record(&ctx, &repo_a), task_record(&ctx, &repo_b));
        let _ = fs::remove_dir_all(repo_a.parent().unwrap());
        let _ = fs::remove_dir_all(repo_b.parent().unwrap());
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
        record(&ctx, &current_top(), json!({"time_ms": now_ms(), "source": "tool", "status": "passed"}), true);
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
    fn the_log_counts_verdicts_per_repository_and_repairs_inside_a_turn() {
        let log = [
            json!({"source": "tool", "status": "still_fails", "repo": "a", "turn_ms": 5}),
            json!({"source": "tool", "status": "passed", "repo": "a", "turn_ms": 5}),
            json!({"source": "tool", "status": "env_error", "repo": "b", "turn_ms": 6}),
            json!({"source": "command", "status": "still_fails", "repo": "c", "turn_ms": 0}),
            json!({"source": "command", "status": "passed", "repo": "c", "turn_ms": 0}),
        ].iter().map(|v| v.to_string()).collect::<Vec<_>>().join("\n");
        let out = log_summary(&log);
        assert!(out.contains("per repository: a still_fails 1, b env_error 1, c still_fails 1"), "{}", out);
        assert!(out.contains("repairs: 1 turns"), "only the model turn counts, not the two CLI calls: {}", out);
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

    /// The hit rate when one reproduction test serves all N candidates of a task, as the checker does. With chance
    /// `none` there is no failing test and the first candidate stays; with chance `good` the test passes every correct
    /// candidate (t = 1); otherwise it passes no correct one (t = 0). A wrong candidate passes with chance f either way.
    fn expected_hit_shared(p: f64, f: f64, n: i32, none: f64, good: f64) -> f64 {
        let q = p + (1.0 - p) * f;
        let with_good = if q == 0.0 { 0.0 } else { p * (1.0 - (1.0 - q).powi(n)) / q };
        let with_bad = p * (p + (1.0 - p) * (1.0 - f)).powi(n - 1);
        none * p + good * with_good + (1.0 - none - good) * with_bad
    }

    #[test]
    fn the_shared_test_formula_matches_a_simulation_and_the_doc_table() {
        let mut seed: u64 = 11;
        let mut rnd = || { seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407); (seed >> 11) as f64 / (1u64 << 53) as f64 };
        let (none, good) = (0.29, 0.71 * 0.44);
        for &(p, f, n) in &[(0.5, 0.2, 3), (0.1, 0.2, 5), (0.97, 0.0, 3)] {
            let rounds = 100_000;
            let mut hits = 0;
            for _ in 0..rounds {
                let u = rnd();
                let good_cands: Vec<bool> = (0..n).map(|_| rnd() < p).collect();
                let pick = if u < none { 0 } else {
                    let t = if u < none + good { 1.0 } else { 0.0 };
                    good_cands.iter().map(|&g| rnd() < if g { t } else { f }).position(|x| x).unwrap_or(0)
                };
                hits += good_cands[pick] as usize;
            }
            let sim = hits as f64 / rounds as f64;
            assert!((sim - expected_hit_shared(p, f, n, none, good)).abs() < 0.01, "p {} f {} n {}: {} vs {}", p, f, n, sim, expected_hit_shared(p, f, n, none, good));
        }
        // The mixture of docs/CHECKER.md: 8 tasks at 0.97, 1 at 0.5, 3 at 0.1, 16 at 0.
        let mix: Vec<f64> = [vec![0.97; 8], vec![0.5], vec![0.1; 3], vec![0.0; 16]].concat();
        let hits = |f: f64, n: i32| mix.iter().map(|&p| expected_hit_shared(p, f, n, none, good)).sum::<f64>();
        assert!((hits(0.2, 3) - 8.709).abs() < 0.001 && (hits(0.2, 5) - 8.690).abs() < 0.001, "{} {}", hits(0.2, 3), hits(0.2, 5));
        assert!((hits(0.0, 3) - 8.912).abs() < 0.001 && (hits(0.0, 5) - 9.071).abs() < 0.001, "{} {}", hits(0.0, 3), hits(0.0, 5));
        assert!(hits(0.2, 5) < hits(0.2, 3), "with f = 0.2 a fifth candidate costs more than it adds");
    }
}
