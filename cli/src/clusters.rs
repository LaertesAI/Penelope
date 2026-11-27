//! Clusters instead of agents: an orchestrator starts groups of parts that share one job and one memory file.
//!
//! A cluster profile names its parts: workers, an optional validator, the model or role of every part, and the limits.
//! A running cluster writes one JSON state file and one `memory.md` under `.penelope/clusters/<id>/`, and reports
//! `S:working|P:1/2|T:12s|F:backend|E:...` after every part. The orchestrator can start several clusters, let them
//! run in parallel, and order them with `after` (a live-test cluster can wait for backend and frontend).
//!
//! The parts are deliberately not subagents. A part is a model call with a role, a focus and the tools of that role.
//! The same cluster can be built from Laertes families, Claude, an OpenCode model or a local model by editing its
//! profile. A part that names a family in training stays visible and blocks only the cluster that included it.

use crate::config::Ctx;
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Serializes the small registry writes; clusters run in threads.
static WRITE_LOCK: Mutex<()> = Mutex::new(());
/// Two clusters may merge at the same time after their validators. One patch lands, the second sees a conflict.
static MERGE_LOCK: Mutex<()> = Mutex::new(());
/// The zombie watch reads the report history and parallel parts may add to it; one writer at a time.
static REPORT_LOCK: Mutex<()> = Mutex::new(());
/// Parallel parts append to one `memory.md`; one writer at a time keeps the sections whole.
static MEMORY_LOCK: Mutex<()> = Mutex::new(());
/// Two clusters may finish a validation at the same moment; the score table is read, changed and written under one lock.
static SCORE_LOCK: Mutex<()> = Mutex::new(());
/// Shadow runs append to one `.penelope/clusters/shadow.json`; one writer at a time.
static SHADOW_LOCK: Mutex<()> = Mutex::new(());
/// The cancel flag of every cluster that is running in this process. `kill()` sets it at once; a watcher on the kill
/// file reaches a cluster started by another process within 100 ms.
static CANCELS: Mutex<Vec<(String, Arc<AtomicBool>)>> = Mutex::new(Vec::new());
/// The live child of every Claude CLI part, by `cluster id/part name`. A kill signals the whole tree.
static CHILDREN: Mutex<Vec<(String, u32)>> = Mutex::new(Vec::new());
/// Two parts may ask to spawn at the same moment. The tree limit is checked and the child created under one lock.
static SPAWN_LOCK: Mutex<()> = Mutex::new(());

thread_local! {
    /// The cluster whose part is running on this thread. A tool call resolves it here; the MCP server of a Claude part
    /// finds the same id in `PENELOPE_CLUSTER`.
    static ACTIVE_CLUSTER: RefCell<Option<String>> = const { RefCell::new(None) };
    /// The origin level of the running part. A worker is L:1, a validator L:2, a person outside a cluster L:3.
    static ACTIVE_LEVEL: RefCell<Option<u8>> = const { RefCell::new(None) };
}

/// Runs a block of work as a part of one cluster. Nested clusters restore the parent id when they return.
pub fn with_cluster<T>(id: &str, f: impl FnOnce() -> T) -> T {
    let before = ACTIVE_CLUSTER.with(|c| c.replace(Some(id.to_string())));
    let out = f();
    ACTIVE_CLUSTER.with(|c| *c.borrow_mut() = before);
    out
}

/// Runs a block at one origin level. A file written inside inherits this level.
pub fn with_level<T>(level: u8, f: impl FnOnce() -> T) -> T {
    let before = ACTIVE_LEVEL.with(|c| c.replace(Some(level.min(3))));
    let out = f();
    ACTIVE_LEVEL.with(|c| *c.borrow_mut() = before);
    out
}

/// The origin level of the running part: the explicit level, otherwise 1 inside a cluster and 3 outside.
pub fn current_level() -> u8 {
    if let Some(level) = ACTIVE_LEVEL.with(|c| *c.borrow()) { return level; }
    if current_cluster().is_some() { 1 } else { 3 }
}

/// The cluster of the running part, or `None` outside a cluster. A Claude CLI part reaches this through its MCP server.
pub fn current_cluster() -> Option<String> {
    if let Some(id) = ACTIVE_CLUSTER.with(|c| c.borrow().clone()) {
        if !id.trim().is_empty() { return Some(id); }
    }
    std::env::var("PENELOPE_CLUSTER").ok().filter(|id| !id.trim().is_empty())
}


pub const BUILTIN_PROFILES: &[&str] = &["coding", "backend", "frontend", "live-test", "research", "solo", "prompt"];

#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    pub name: String,
    pub role: String,
    pub slot: String,
    pub task: String,
    /// The kind of task this part does, for the score table (for example `rust-fix`). Empty means the profile name.
    pub task_type: String,
    /// A model setting, a role name (`main`, `weak`, `scout`, `judge`, `worker`, ...), or empty for the role's model.
    pub model: Value,
    pub mcp: Vec<String>,
    /// Names of worker parts that must finish before this part starts. Empty means it may start at once.
    pub depends_on: Vec<String>,
    /// Independent answers before this part's result is accepted. 0 inherits the profile: 1 runs once, 2 runs the
    /// part on its model and on the profile's `second_model`. A disagreement escalates once to `decider_model`.
    pub quorum: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Profile {
    pub name: String,
    pub workers: Vec<Part>,
    pub validator: Option<Part>,
    pub max_models: usize,
    pub max_depth: usize,
    /// Worker parts that may run at the same time. Writes to the memory file stay serialized.
    pub max_parallel: usize,
    pub budget_eur: f64,
    pub max_steps: usize,
    pub retries: usize,
    /// True sends a one-worker, no-retry profile through one model and skips the final validator. The validator cannot
    /// correct a result when there is no retry; the caller's own check is the judge. Off by default.
    pub single_model: bool,
    /// Seconds between two reports of a running part. 0 reports after every model answer.
    pub report_every: u64,
    /// Consecutive reports with one `P:` and `F:`, or repeated equal tool calls, before the cluster is stuck.
    pub stuck_after: usize,
    /// A cluster that may write gets its own git worktree. Research and live-test profiles turn this off.
    pub worktree: bool,
    /// A named profile that rewrites the task before the workers of this profile start. The named profile must be
    /// read-only and must not name a `before` of its own, so there is no hidden chain of prompt clusters.
    pub before: Option<String>,
    /// A factory schedule: `every` repeats after a duration such as `3h`. The two fields are exclusive.
    pub every: Option<String>,
    /// A factory schedule: `at` is a time of day, `HH:MM`, once a day.
    pub at: Option<String>,
    /// The one purpose of a factory profile. A factory uses it as the cluster goal when its registry entry has none.
    pub purpose: Option<String>,
    /// Independent answers for every worker part that does not set its own `quorum`. 1 (default) runs once, 2 runs
    /// the part and a second opinion; a disagreement asks the decider model once.
    pub quorum: usize,
    /// The model setting of the second quorum answer. Default `weak`; it may be a full model object.
    pub second_model: Value,
    /// The model setting that decides a disagreement. Default `main`; it is called once per disagreement.
    pub decider_model: Value,
    /// This profile runs as a shadow beside the profile it names: same task, own worktree, files never merged.
    pub shadow_of: Option<String>,
}

/// A number that may arrive as `3`, `"3"` or `"3 reports"`.
fn count_of(value: &Value) -> Option<usize> {
    if let Some(n) = value.as_u64() { return Some(n as usize); }
    let text = value.as_str()?.trim();
    let digits: String = text.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// Seconds that may arrive as `30`, `"30s"`, `"5m"` or `"1h"`.
fn seconds_of(value: &Value) -> Option<u64> {
    if let Some(n) = value.as_u64() { return Some(n); }
    let text = value.as_str()?.trim().to_lowercase();
    let digits: String = text.chars().take_while(|c| c.is_ascii_digit()).collect();
    let n: u64 = digits.parse().ok()?;
    Some(match text.chars().last() {
        Some('m') => n * 60,
        Some('h') => n * 3600,
        _ => n,
    })
}

#[derive(Clone, Debug, Default)]
pub struct PartRun {
    pub name: String,
    pub slot: String,
    pub role: String,
    pub model: String,
    pub state: String,
    pub output: String,
    pub error: String,
}

#[derive(Clone, Debug, Default)]
pub struct Cluster {
    pub id: String,
    pub name: String,
    pub profile: String,
    pub task: String,
    pub parent: Option<String>,
    pub after: Vec<String>,
    pub depth: usize,
    pub state: String,
    pub done: usize,
    pub total: usize,
    pub started: u64,
    pub updated: u64,
    pub output: String,
    pub error: String,
    pub memory: String,
    /// The check the validator ran and its result, for `V:` in the report.
    pub evidence: String,
    /// The git root of the project, when this cluster has a worktree.
    pub project: String,
    /// The worktree of this cluster; empty for a research or live-test cluster.
    pub worktree: String,
    pub branch: String,
    /// The working directory for every tool call of this cluster: the worktree, or empty for the live tree.
    pub cwd: String,
    /// How often this cluster was already started again from a dump. The limit is 2.
    pub respawns: usize,
    /// The cluster this one was respawned from.
    pub respawned_from: String,
    /// Parts that agreed with their independent second answer, and parts escalated to the decider model.
    pub quorum_agreed: usize,
    pub quorum_escalated: usize,
    /// A shadow cluster runs beside a main cluster. Its files are never merged into the live tree.
    pub shadow: bool,
    /// The main cluster id when this cluster is a shadow; empty otherwise.
    pub shadow_of: String,
    /// The match of a shadow run against its main run, 0.0 to 1.0. `None` until the pair was compared.
    pub match_rate: Option<f64>,
    pub parts: Vec<PartRun>,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn stamp() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)
}

fn safe(name: &str) -> String {
    name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '-' }).collect()
}

fn base(ctx: &Ctx) -> PathBuf {
    ctx.root.join("clusters")
}

fn profile_dir(ctx: &Ctx) -> PathBuf {
    base(ctx).join("profiles")
}

fn cluster_dir(ctx: &Ctx, id: &str) -> PathBuf {
    base(ctx).join(safe(id))
}

fn cluster_file(ctx: &Ctx, id: &str) -> PathBuf {
    base(ctx).join(format!("{}.json", safe(id)))
}

pub fn profile_names(ctx: &Ctx) -> Vec<String> {
    let mut out: Vec<String> = BUILTIN_PROFILES.iter().map(|s| s.to_string()).collect();
    if let Ok(entries) = fs::read_dir(profile_dir(ctx)) {
        for e in entries.flatten() {
            let path = e.path();
            let ext = path.extension().and_then(|x| x.to_str());
            if !matches!(ext, Some("json") | Some("cluster")) { continue; }
            if let Some(stem) = path.file_stem().and_then(|x| x.to_str()) {
                if !out.iter().any(|n| n == stem) { out.push(stem.to_string()); }
            }
        }
    }
    out.sort();
    out
}

fn part(value: &Value, fallback: &str) -> Part {
    let name = value["name"].as_str().filter(|s| !s.trim().is_empty()).unwrap_or(fallback).trim().to_string();
    let role = value["role"].as_str().filter(|s| !s.trim().is_empty()).unwrap_or("worker").trim().to_string();
    let model = value.get("model").cloned().unwrap_or(json!(""));
    let slot = value["slot"].as_str().map(str::to_string)
        .or_else(|| crate::crew::family_of(&model).map(str::to_string))
        .unwrap_or_else(|| "part".to_string());
    Part {
        name,
        role,
        slot,
        task: value["task"].as_str().unwrap_or("").trim().to_string(),
        task_type: value["task_type"].as_str().unwrap_or("").trim().to_string(),
        model,
        mcp: value["mcp"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default(),
        depends_on: ["needs", "after"].iter()
            .find_map(|key| value[*key].as_array())
            .map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.trim().to_string())).filter(|s| !s.is_empty()).collect())
            .unwrap_or_default(),
        quorum: count_of(&value["quorum"]).unwrap_or(0).min(2),
    }
}

/// A model setting may be a name such as `weak` or a full setting object. It is not resolved here, so a broken
/// setting fails at the part that really needs it.
fn model_setting(value: &Value, fallback: &str) -> Value {
    if value.is_object() {
        return value.clone();
    }
    match value.as_str().map(str::trim).filter(|s| !s.is_empty()) {
        Some(name) => json!(name),
        None => json!(fallback),
    }
}

/// A profile name in `before` or `shadow_of`: lower case letters, digits, `_` or `-`, at most 32 characters.
fn profile_name_ok(name: &str) -> bool {
    !name.is_empty() && name.len() <= 32
        && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

/// A part role. `human` is a cluster-only role: it makes no model call and waits for a yes or no.
fn part_role_known(role: &str) -> bool {
    role == "human" || crate::route::role_names().iter().any(|r| *r == role)
}

fn parse_profile(value: &Value, fallback_name: &str) -> Result<Profile, String> {
    let name = value["name"].as_str().filter(|s| !s.trim().is_empty()).unwrap_or(fallback_name).trim().to_string();
    let workers = value["workers"].as_array().ok_or_else(|| format!("profile {} has no workers list", name))?;
    if workers.is_empty() { return Err(format!("profile {} has no worker part", name)); }
    let mut list = Vec::new();
    for (i, w) in workers.iter().enumerate() {
        let p = part(w, &format!("worker{}", i + 1));
        if !part_role_known(&p.role) {
            return Err(format!("profile {}: part {} has unknown role {} (use {})", name, p.name, p.role, crate::route::role_names().join(", ")));
        }
        if list.iter().any(|x: &Part| x.name == p.name) { return Err(format!("profile {}: part name {} is used twice", name, p.name)); }
        list.push(p);
    }
    let validator = value["validator"].as_object().map(|v| part(&Value::Object(v.clone()), "validator"));
    if let Some(v) = &validator {
        if v.role == "human" {
            return Err(format!("profile {}: the validator cannot be a human part; a validator must answer with a check and evidence. Put the human part in workers.", name));
        }
        if !crate::route::role_names().iter().any(|r| *r == v.role) {
            return Err(format!("profile {}: validator has unknown role {}", name, v.role));
        }
    }
    let max_models = value["max_models"].as_u64().unwrap_or(8) as usize;
    let count = list.len() + validator.iter().count();
    if max_models == 0 || count > max_models { return Err(format!("profile {}: {} parts, max_models is {}", name, count, max_models)); }
    // A part may name other worker parts it waits for. Unknown names and cycles are refused before anything runs.
    let names: Vec<&str> = list.iter().map(|p| p.name.as_str()).collect();
    for p in &list {
        for need in &p.depends_on {
            if !names.contains(&need.as_str()) {
                return Err(format!("profile {}: part {} needs unknown part {}", name, p.name, need));
            }
            if need == &p.name {
                return Err(format!("profile {}: part {} needs itself", name, p.name));
            }
        }
    }
    let mut remaining: Vec<&str> = names.clone();
    while !remaining.is_empty() {
        let ready: Vec<&str> = remaining.iter().copied().filter(|part| {
            list.iter().find(|p| p.name == *part).map(|p| p.depends_on.iter().all(|need| !remaining.contains(&need.as_str()))).unwrap_or(false)
        }).collect();
        if ready.is_empty() { return Err(format!("profile {}: the parts need each other in a cycle", name)); }
        remaining.retain(|part| !ready.contains(part));
    }
    let max_depth = value["max_depth"].as_u64().unwrap_or(2).min(3) as usize;
    let max_parallel = value["max_parallel"].as_u64().unwrap_or(1).clamp(1, 8) as usize;
    let budget_eur = value["budget_eur"].as_f64().unwrap_or(0.0).max(0.0);
    let max_steps = value["max_steps"].as_u64().unwrap_or(12).clamp(1, 30) as usize;
    let retries = value["retries"].as_u64().unwrap_or(1).min(3) as usize;
    let single_model = value["single_model"].as_bool().unwrap_or(false);
    let report_every = seconds_of(&value["report_every"]).unwrap_or(0);
    let stuck_after = count_of(&value["stuck_after"]).unwrap_or(3).max(1);
    // A profile with a worker part writes, so it gets a worktree unless it says otherwise. A research or live-test
    // profile has no writer and can be set to false explicitly.
    let worktree = value["worktree"].as_bool().unwrap_or_else(|| list.iter().any(|p| p.role == "worker"));
    // `before` names another profile that rewrites the task. It is checked at start, so this parse stays free of file
    // reads; the chain and the read-only rule are enforced before any model call.
    let before = match value["before"].as_str().map(str::trim).filter(|s| !s.is_empty()) {
        Some(before) => {
            if !profile_name_ok(before) {
                return Err(format!("profile {}: before {} is no profile name (lower case letters, digits, _ or -, at most 32)", name, before));
            }
            Some(before.to_string())
        }
        None => None,
    };
    // A shadow profile runs beside the profile it names. The main run is never changed; the shadow's files are never
    // merged. The profile must exist when the pair starts, not at parse time.
    let shadow_of = match value["shadow_of"].as_str().map(str::trim).filter(|s| !s.is_empty()) {
        Some(main) => {
            if !profile_name_ok(main) {
                return Err(format!("profile {}: shadow_of {} is no profile name (lower case letters, digits, _ or -, at most 32)", name, main));
            }
            if main == name {
                return Err(format!("profile {}: shadow_of names the profile itself. A shadow runs beside another profile.", name));
            }
            Some(main.to_string())
        }
        None => None,
    };
    // The quorum settings are numbers and two model settings. A quorum of 2 is the only useful vote; a third answer
    // would need a tie rule, so the profile is refused instead of silently clamping.
    let quorum = match value.get("quorum") {
        None | Some(Value::Null) => 1,
        Some(v) => match count_of(v) {
            Some(1) => 1,
            Some(2) => 2,
            _ => return Err(format!("profile {}: quorum is 1 or 2, got {}", name, v)),
        },
    };
    let second_model = model_setting(&value["second_model"], "weak");
    let decider_model = model_setting(&value["decider_model"], "main");
    // A factory profile carries its schedule. The two forms are exclusive and are validated here, so a timer is never
    // written from a typo. The runner only reads the fields; the schedule itself lives in the operating system.
    let every = match value.get("every") {
        Some(Value::String(text)) => Some(crate::factories::normalize_every(text)
            .map_err(|e| format!("profile {}: {}", name, e))?),
        Some(value) if value.as_u64().is_some() => {
            let seconds = value.as_u64().unwrap_or(0);
            if seconds == 0 { return Err(format!("profile {}: every needs a duration such as 3h or 30m", name)); }
            Some(crate::factories::normalize_every(&format!("{}s", seconds))
                .map_err(|e| format!("profile {}: {}", name, e))?)
        }
        Some(Value::Null) | None => None,
        Some(_) => return Err(format!("profile {}: every needs a duration such as 3h or 30m", name)),
    };
    let at = match value.get("at") {
        Some(Value::String(text)) => Some(crate::factories::normalize_at(text)
            .map_err(|e| format!("profile {}: {}", name, e))?),
        Some(Value::Null) | None => None,
        Some(_) => return Err(format!("profile {}: at needs a time of day such as 09:30", name)),
    };
    if every.is_some() && at.is_some() {
        return Err(format!("profile {}: use every or at, not both", name));
    }
    let purpose = value["purpose"].as_str().map(str::trim).filter(|s| !s.is_empty()).map(|s| s.chars().take(1000).collect::<String>());
    Ok(Profile { name, workers: list, validator, max_models, max_depth, max_parallel, budget_eur, max_steps, retries, single_model, report_every, stuck_after, worktree, before, every, at, purpose, quorum, second_model, decider_model, shadow_of })
}

fn builtin(name: &str) -> Option<Profile> {
    let value = match name {
        "solo" => json!({
            "name": "solo",
            "workers": [{"name": "coder", "role": "worker", "slot": "Telemach", "model": "main",
                         "task": "Do the task. Read the project first, change only what the task needs, and run the check."}],
            "validator": {"name": "check", "role": "judge", "slot": "Argos", "model": "judge",
                          "task": "Check the coder's evidence against the task. Answer PASS or FAIL with one reason."},
            "max_models": 2, "max_depth": 1, "max_steps": 12, "retries": 1
        }),
        "backend" => json!({
            "name": "backend",
            "workers": [
                {"name": "api", "role": "worker", "slot": "Telemach", "model": "main",
                 "task": "Own the backend API. Read the existing routes and data model first, then implement and test the server side."},
                {"name": "data", "role": "worker", "slot": "Telemach", "model": "main",
                 "task": "Own persistence and migrations. Check the schema and the data path, then make the smallest safe change."}
            ],
            "validator": {"name": "check", "role": "judge", "slot": "Argos", "model": "judge",
                          "task": "Check the backend work against the task and the project's tests. Answer PASS or FAIL with one reason."},
            "max_models": 8, "max_depth": 2, "max_steps": 14, "retries": 1
        }),
        "frontend" => json!({
            "name": "frontend",
            "workers": [
                {"name": "ui", "role": "worker", "slot": "Telemach", "model": "main",
                 "task": "Own the interface. Read the components and styles first, then implement the screen and its states."},
                {"name": "interaction", "role": "worker", "slot": "Telemach", "model": "main",
                 "task": "Own interaction and accessibility. Check keyboard, focus, empty and error states."}
            ],
            "validator": {"name": "check", "role": "judge", "slot": "Argos", "model": "judge",
                          "task": "Check the frontend work against the task. Answer PASS or FAIL with one reason."},
            "max_models": 8, "max_depth": 2, "max_steps": 14, "retries": 1
        }),
        "live-test" => json!({
            "name": "live-test",
            "workers": [
                {"name": "tester", "role": "worker", "slot": "Nostos", "model": "worker",
                 "task": "Start the service and test the running system with commands. Report the exact requests and responses."},
                {"name": "screen", "role": "tool", "slot": "Nostos", "model": "tool",
                 "task": "Inspect the live screen with the ScreenCodec tools. Report what is visible and what is broken."}
            ],
            "validator": {"name": "check", "role": "judge", "slot": "Argos", "model": "judge",
                          "task": "Check the live evidence against the task. PASS only when the running system was really tested."},
            "max_models": 4, "max_depth": 1, "max_steps": 16, "retries": 1, "worktree": false
        }),
        "research" => json!({
            "name": "research",
            "workers": [
                {"name": "research1", "role": "scout", "slot": "Nostos", "model": "scout",
                 "task": "Search the web and the project. Collect facts with URLs and exact names."},
                {"name": "research2", "role": "scout", "slot": "Nostos", "model": "scout",
                 "task": "Search from the other side: primary sources, counterexamples and missing context."}
            ],
            "validator": {"name": "check", "role": "judge", "slot": "Argos", "model": "judge",
                          "task": "Check that the report has sources and answers the question. PASS or FAIL with one reason."},
            "max_models": 8, "max_depth": 2, "max_steps": 12, "retries": 1, "worktree": false
        }),
        "coding" => json!({
            "name": "coding",
            "workers": [
                {"name": "backend", "role": "worker", "slot": "Telemach", "model": "main",
                 "task": "Own the backend part of the task: routes, data, migrations and server tests."},
                {"name": "frontend", "role": "worker", "slot": "Telemach", "model": "main",
                 "task": "Own the frontend part of the task: screens, state, accessibility and browser tests."}
            ],
            "validator": {"name": "check", "role": "judge", "slot": "Argos", "model": "judge",
                          "task": "Check both parts against the task and the project's checks. Answer PASS or FAIL with one reason."},
            "max_models": 8, "max_depth": 2, "max_steps": 14, "retries": 1
        }),
        "prompt" => json!({
            "name": "prompt",
            "workers": [
                {"name": "rewrite", "role": "condenser", "slot": "Aiolos", "model": "weak",
                 "task": "Rewrite the cluster task so a worker can do it directly. Keep every fact, file, limit and check. Answer with the rewritten task only."}
            ],
            "validator": {"name": "check", "role": "judge", "slot": "Argos", "model": "judge",
                          "task": "Check that the rewritten task keeps every fact of the original and is directly doable. Answer PASS or FAIL with one reason."},
            "max_models": 2, "max_depth": 1, "max_steps": 6, "retries": 0, "worktree": false
        }),
        _ => return None,
    };
    parse_profile(&value, name).ok()
}

/// The fixed parts a cluster file may name. The file cannot name a model setting, a tool or a task; it picks one of
/// these templates, so parsing never evaluates anything and no text from the file reaches a shell.
const CLUSTER_PART_NAMES: &[&str] = &["lead", "researcher", "scout", "coder", "worker", "validator", "judge", "tool", "tester", "human", "condenser"];

fn cluster_template(base: &str) -> Option<(&'static str, &'static str, &'static str, &'static str)> {
    Some(match base {
        "lead" => ("lead", "Tiresias", "main", "Plan the cluster task. Name the files, the steps and the check that proves the result."),
        "researcher" | "scout" => ("scout", "Nostos", "scout", "Search the project and the web. Collect facts with exact names, paths and URLs; change nothing."),
        "coder" | "worker" => ("worker", "Telemach", "main", "Do the change in your worktree. Run the project's check and report its exact output."),
        "validator" | "judge" => ("judge", "Argos", "judge", "Check the work against the cluster task and the evidence in the memory. Answer PASS or FAIL with one reason."),
        "tool" => ("tool", "Nostos", "tool", "Use the screen and MCP tools. Report what is visible and what is broken."),
        "tester" => ("worker", "Telemach", "worker", "Start the service and test the running system with commands. Report the exact requests and responses."),
        "human" => ("human", "you", "", "Answer this approval question with yes or no."),
        "condenser" => ("condenser", "Aiolos", "weak", "Rewrite the task so a worker can do it directly. Keep every fact, limit and check; answer with the rewritten task only."),
        _ => return None,
    })
}

fn cluster_part_value(name: &str, t: (&'static str, &'static str, &'static str, &'static str)) -> Value {
    json!({"name": name, "role": t.0, "slot": t.1, "model": t.2, "task": t.3})
}

fn valid_cluster_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase())
        && name.len() <= 32
        && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

/// Splits the inside of `$name(...)` at commas outside `[parts]`. Quotes and code are not part of the language.
fn split_cluster_args(text: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut depth = 0i32;
    for c in text.chars() {
        match c {
            '[' => { depth += 1; cur.push(c); }
            ']' => {
                depth -= 1;
                if depth < 0 { return Err("NOT parsed. parts=[...] has a ] without a [.".to_string()); }
                cur.push(c);
            }
            ',' if depth == 0 => out.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    if depth != 0 { return Err("NOT parsed. parts=[...] is not closed.".to_string()); }
    out.push(cur);
    Ok(out.into_iter().map(|p| p.trim().to_string()).collect())
}

/// Parses `parts=[researcher, coder*3, validator, tool]`. A repeated part gets the names coder, coder2, coder3; a
/// name may be used only once. There is exactly one validator.
fn parse_cluster_parts(text: &str) -> Result<(Vec<Value>, Option<Value>), String> {
    let inner = text.strip_prefix('[').and_then(|s| s.strip_suffix(']'))
        .ok_or_else(|| "NOT parsed. parts=[...] needs square brackets, for example parts=[coder, validator].".to_string())?;
    if inner.trim().is_empty() {
        return Err("NOT parsed. parts=[...] has no part; name at least one part beside the validator.".to_string());
    }
    let mut workers = Vec::new();
    let mut validator: Option<Value> = None;
    let mut used: Vec<String> = Vec::new();
    for item in inner.split(',') {
        let item = item.trim();
        if item.is_empty() { return Err("NOT parsed. There is an empty part in parts=[...].".to_string()); }
        let (base, count) = match item.split_once('*') {
            Some((b, c)) => {
                if c.contains('*') { return Err(format!("NOT parsed. {} has more than one *.", item)); }
                let n: usize = c.trim().parse().map_err(|_| format!("NOT parsed. {} needs a count after *, for example coder*3.", item))?;
                (b.trim(), n)
            }
            None => (item, 1),
        };
        if !valid_cluster_name(base) {
            return Err(format!("NOT parsed. {} is no part name (lower case letters, digits, _ or -, at most 32).", base));
        }
        if count == 0 || count > 8 {
            return Err(format!("NOT parsed. {} repeats a part {} times; use 1 to 8.", item, count));
        }
        let template = cluster_template(base)
            .ok_or_else(|| format!("NOT parsed. Unknown part {}. The parts are: {}.", base, CLUSTER_PART_NAMES.join(", ")))?;
        if base == "validator" || base == "judge" {
            if count != 1 { return Err(format!("NOT parsed. There is one validator; {} names {} of them.", item, count)); }
            if validator.is_some() { return Err("NOT parsed. The validator is named twice; use validator or judge once.".to_string()); }
            validator = Some(cluster_part_value(base, template));
            continue;
        }
        for n in 1..=count {
            let name = if count == 1 || n == 1 { base.to_string() } else { format!("{}{}", base, n) };
            if used.iter().any(|u| u == &name) {
                return Err(format!("NOT parsed. The part name {} is used twice.", name));
            }
            used.push(name.clone());
            workers.push(cluster_part_value(&name, template));
        }
    }
    if workers.is_empty() {
        return Err("NOT parsed. parts=[...] has no worker part; name at least one part beside the validator.".to_string());
    }
    Ok((workers, validator))
}

/// One numeric or duration setting of a cluster file, with the same ranges `parse_profile` expects.
fn cluster_setting(key: &str, raw: &str) -> Result<Value, String> {
    let whole = |low: u64, high: u64| -> Result<u64, String> {
        let n: u64 = raw.parse().map_err(|_| format!("NOT parsed. {} needs a whole number, got {}.", key, raw))?;
        if n < low || n > high { return Err(format!("NOT parsed. {} is between {} and {}, got {}.", key, low, high, n)); }
        Ok(n)
    };
    match key {
        "max_models" => Ok(json!(whole(1, 64)?)),
        "max_depth" => Ok(json!(whole(0, 3)?)),
        "max_parallel" => Ok(json!(whole(1, 8)?)),
        "max_steps" => Ok(json!(whole(1, 30)?)),
        "retries" => Ok(json!(whole(0, 3)?)),
        "budget_eur" => {
            let n: f64 = raw.parse().map_err(|_| format!("NOT parsed. budget_eur needs a number, got {}.", raw))?;
            if !(0.0..=1000.0).contains(&n) { return Err(format!("NOT parsed. budget_eur is between 0 and 1000, got {}.", n)); }
            Ok(json!(n))
        }
        "report_every" => {
            let n = seconds_of(&Value::String(raw.to_string())).ok_or_else(|| format!("NOT parsed. report_every needs seconds such as 30s, 5m or 1h, got {}.", raw))?;
            if n > 86_400 { return Err(format!("NOT parsed. report_every is at most 24h, got {}.", raw)); }
            Ok(json!(raw))
        }
        "stuck_after" => {
            let n = count_of(&Value::String(raw.to_string())).ok_or_else(|| format!("NOT parsed. stuck_after needs a number of reports, got {}.", raw))?;
            if n == 0 || n > 100 { return Err(format!("NOT parsed. stuck_after is between 1 and 100, got {}.", n)); }
            Ok(json!(raw))
        }
        "quorum" => Ok(json!(whole(1, 2)?)),
        _ => Err(format!("NOT parsed. Unknown setting {}.", key)),
    }
}

const CLUSTER_FILE_KEYS: &[&str] = &["parts", "before", "shadow_of", "quorum", "max_models", "max_depth", "max_parallel", "max_steps", "retries", "budget_eur", "report_every", "stuck_after", "worktree"];

/// Parses one cluster file into a `Profile`:
/// `$dev(parts=[researcher, coder*3, validator, tool], max_models=8, max_depth=3, budget_eur=2)`.
/// The language is deliberately small: named built-in parts, repetitions, and numbers. No quotes, no code, no model
/// setting and no tool name can enter through it.
pub fn parse_cluster(text: &str) -> Result<Profile, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err("NOT parsed. A cluster file is empty. Example: $dev(parts=[coder, validator], max_models=2)".to_string());
    }
    for c in trimmed.chars() {
        if !(c.is_ascii_alphanumeric() || c.is_whitespace() || " $()[],.=*_-".contains(c)) {
            return Err(format!("NOT parsed. The character {:?} is not allowed in a cluster file; the language has only part names, numbers and the keys {}.", c, CLUSTER_FILE_KEYS.join(", ")));
        }
    }
    let rest = trimmed.strip_prefix('$')
        .ok_or_else(|| "NOT parsed. A cluster file starts with $, for example $dev(parts=[coder, validator]).".to_string())?;
    let open = rest.find('(')
        .ok_or_else(|| "NOT parsed. A cluster file needs (...), for example $dev(parts=[coder, validator]).".to_string())?;
    let inside = rest[open + 1..].strip_suffix(')')
        .ok_or_else(|| "NOT parsed. A cluster file ends with ).".to_string())?;
    let name = rest[..open].trim();
    if !valid_cluster_name(name) {
        return Err("NOT parsed. The name of a cluster is 1 to 32 lower case letters, digits, _ or -, starting with a letter.".to_string());
    }
    let mut parts: Option<(Vec<Value>, Option<Value>)> = None;
    let mut settings: Vec<(String, Value)> = Vec::new();
    for arg in split_cluster_args(inside)? {
        if arg.is_empty() { return Err("NOT parsed. There is an empty setting in (...).".to_string()); }
        let (key, raw) = arg.split_once('=')
            .ok_or_else(|| format!("NOT parsed. {} is no key=value pair.", arg))?;
        let key = key.trim().to_lowercase();
        let raw = raw.trim();
        if !CLUSTER_FILE_KEYS.contains(&key.as_str()) {
            return Err(format!("NOT parsed. Unknown setting {}. The settings are: {}.", key, CLUSTER_FILE_KEYS.join(", ")));
        }
        if settings.iter().any(|(k, _)| k == &key) || (key == "parts" && parts.is_some()) {
            return Err(format!("NOT parsed. The setting {} appears twice.", key));
        }
        if key == "parts" {
            parts = Some(parse_cluster_parts(raw)?);
        } else if key == "worktree" {
            let on = match raw.to_lowercase().as_str() {
                "true" | "yes" | "on" => true,
                "false" | "no" | "off" => false,
                _ => return Err(format!("NOT parsed. worktree is true or false, got {}.", raw)),
            };
            settings.push((key, json!(on)));
        } else if key == "before" {
            if !valid_cluster_name(raw) {
                return Err(format!("NOT parsed. before needs a profile name (lower case letters, digits, _ or -, at most 32), got {}.", raw));
            }
            settings.push((key, json!(raw)));
        } else if key == "shadow_of" {
            if !valid_cluster_name(raw) {
                return Err(format!("NOT parsed. shadow_of needs a profile name (lower case letters, digits, _ or -, at most 32), got {}.", raw));
            }
            if name == raw {
                return Err("NOT parsed. shadow_of names the profile itself.".to_string());
            }
            settings.push((key, json!(raw)));
        } else {
            let item = cluster_setting(&key, raw)?;
            settings.push((key, item));
        }
    }
    let (workers, validator) = parts
        .ok_or_else(|| "NOT parsed. A cluster file needs parts=[...], for example parts=[coder, validator].".to_string())?;
    let mut value = json!({"name": name, "workers": workers});
    if let Some(v) = validator { value["validator"] = v; }
    for (key, item) in settings { value[key.as_str()] = item; }
    parse_profile(&value, name)
}

/// Reads a profile from a JSON or a cluster file. A cluster file must name the profile the same way its file is called.
fn read_profile_file(path: &Path, fallback: &str) -> Result<Profile, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("cannot read {}: {}", path.display(), e))?;
    match path.extension().and_then(|x| x.to_str()) {
        Some("cluster") => parse_cluster(&text),
        _ => {
            let value: Value = serde_json::from_str(&text).map_err(|e| format!("profile {} is not JSON: {}", fallback, e))?;
            parse_profile(&value, fallback)
        }
    }
}

pub fn profile(ctx: &Ctx, name: &str) -> Result<Profile, String> {
    let name = name.trim();
    // `--profile ./dev.cluster` and `--profile /path/dev.json` work directly, without copying a file into the folder.
    let direct = Path::new(name);
    if (name.ends_with(".cluster") || name.ends_with(".json")) && direct.is_file() {
        let fallback = direct.file_stem().and_then(|x| x.to_str()).unwrap_or(name);
        return read_profile_file(direct, fallback);
    }
    let path = profile_dir(ctx).join(format!("{}.json", safe(name)));
    if let Ok(text) = fs::read_to_string(&path) {
        let value: Value = serde_json::from_str(&text).map_err(|e| format!("profile {} is not JSON: {}", name, e))?;
        return parse_profile(&value, name);
    }
    let cluster_path = profile_dir(ctx).join(format!("{}.cluster", safe(name)));
    if let Ok(text) = fs::read_to_string(&cluster_path) {
        let p = parse_cluster(&text)?;
        if p.name != name {
            return Err(format!("cluster file {} names ${} but the profile is called {}. Rename one of them.",
                               cluster_path.display(), p.name, name));
        }
        return Ok(p);
    }
    if let Some(p) = builtin(name) { return Ok(p); }
    // A user profile may name a built-in profile as its base and add or remove parts.
    Err(format!("no cluster profile {}. profiles: {}. Put one under .penelope/clusters/profiles/{}.json or {}.cluster.",
                 name, profile_names(ctx).join(", "), name, name))
}

pub fn profiles_text(ctx: &Ctx) -> String {
    let mut out = String::from("Cluster profiles\n\n");
    for name in profile_names(ctx) {
        match profile(ctx, &name) {
            Ok(p) => {
                let parts: Vec<String> = p.workers.iter().map(|w| format!("{} ({})", w.name, w.role))
                    .chain(p.validator.iter().map(|v| format!("{} (validator)", v.name))).collect();
                let quorum_parts = p.workers.iter().filter(|w| quorum_of(&p, w) >= 2).count();
                let extra = format!("{}{}{}{}{}",
                    if p.worktree { " · worktree" } else { "" },
                    p.before.as_deref().map(|b| format!(" · before {}", b)).unwrap_or_default(),
                    p.shadow_of.as_deref().map(|s| format!(" · shadow of {}", s)).unwrap_or_default(),
                    if quorum_parts > 0 { format!(" · quorum 2 on {} parts (second {}, decider {})", quorum_parts,
                                                  p.second_model.as_str().unwrap_or("a model setting"),
                                                  p.decider_model.as_str().unwrap_or("a model setting")) } else { String::new() },
                    if p.budget_eur > 0.0 { format!(" · {:.2} EUR budget", p.budget_eur) } else { " · no euro limit".to_string() });
                out.push_str(&format!("{:<12} {} · max {} parts, depth {}, {} parallel, {} retries, report every {}s, stuck after {}{}\n  {}\n",
                                      name, parts.join(", "), p.max_models, p.max_depth, p.max_parallel, p.retries,
                                      p.report_every, p.stuck_after, extra,
                                      p.every.as_deref().map(|e| format!("every {}", e))
                                          .or_else(|| p.at.as_deref().map(|a| format!("daily at {}", a))).unwrap_or_default()));
            }
            Err(e) => out.push_str(&format!("{:<12} {}\n", name, e)),
        }
    }
    out.push_str("\nA profile is a file under .penelope/clusters/profiles/NAME.json or NAME.cluster. A part names a role, a slot and a\nmodel: \"main\", \"weak\", \"scout\", \"judge\", \"worker\" or a full model setting such as an OpenCode model.\n");
    out.push_str("A cluster file is one line: $dev(parts=[researcher, coder*3, validator, tool], max_models=8, max_depth=3, budget_eur=2).\nIt names built-in parts and repetitions only; it cannot carry a model, a tool or code.\n");
    out.push_str("A part with role \"human\" waits for yes or no. Without an interface set \"human\": \"yes\" or \"no\" in\nconfig.json; Penelope never assumes yes. A profile field \"before\": \"PROFILE\" sends the task through that read-only\nprompt cluster before its workers start; the prompt profile must not name a before of its own.\n");
    out.push_str("A part may set \"task_type\" (for example \"rust-fix\"); validator results are scored per part and task type,\nwith the profile name as the default. `penelope cluster advise [NAME]` prints the parts that fail often;\n`penelope cluster advise NAME --apply` writes the best recorded model into the profile for the next run.\n");
    out.push_str("A part or profile may set \"quorum\": 2: the part runs twice, the second answer uses the profile's\n\"second_model\" (default weak), and a disagreement asks \"decider_model\" (default main) once. A profile with\n\"shadow_of\": \"MAIN\" runs as a shadow beside MAIN on the same task; its worktree is never merged.\n");
    out
}

fn estimate_part(ctx: &Ctx, part: &Part, steps: usize) -> (u64, u64, Option<f64>, String) {
    if part.role == "human" {
        return (0, 0, None, "you (a person)".to_string());
    }
    // A rough basis for the estimate: system, task and memory at the start, then one step of tool text per step.
    let tokens_in = 1_500 + 800 * steps as u64;
    let tokens_out = 250 * steps as u64;
    match model_for_part(ctx, part) {
        Ok(model) => {
            let name = model["model"].as_str().unwrap_or("").to_string();
            let euro = crate::cost::euro_for(ctx, &name, tokens_in, tokens_out);
            (tokens_in, tokens_out, euro, part_model_name(part, &model))
        }
        Err(e) => (tokens_in, tokens_out, None, format!("(no model: {})", e)),
    }
}

fn dry_cluster_text(ctx: &Ctx, p: &Profile, goal: &str, note: &str) -> String {
    let mut out = format!("\ncluster {}{}\n", p.name, if note.is_empty() { String::new() } else { format!(" · {}", note) });
    let task = if goal.trim().is_empty() { "(no goal given; the real run needs GOAL)" } else { goal.trim() };
    out.push_str(&format!("  task: {}\n", task.chars().take(200).collect::<String>()));
    out.push_str(&format!("  limits: max {} parts, depth {}, {} parallel, {} steps per part, {} retries{}{}{}\n",
                          p.max_models, p.max_depth, p.max_parallel, p.max_steps, p.retries,
                          if p.worktree { " · worktree" } else { "" },
                          p.before.as_deref().map(|b| format!(" · before {}", b)).unwrap_or_default(),
                          if p.budget_eur > 0.0 { format!(" · {:.2} EUR budget", p.budget_eur) } else { String::new() }));
    let mut tokens_in = 0u64;
    let mut tokens_out = 0u64;
    let mut euro = 0.0;
    let mut unpriced = 0usize;
    let quorum_parts = p.workers.iter().filter(|w| quorum_of(p, w) >= 2).count();
    for part in p.workers.iter().chain(p.validator.iter()) {
        let steps = if part.role == "judge" { 4 } else { p.max_steps };
        let (tin, tout, cost, model) = estimate_part(ctx, part, steps);
        tokens_in += tin;
        tokens_out += tout;
        let price = match cost {
            Some(value) => { euro += value; format!(" · {:.4} EUR", value) }
            None if part.role == "human" => " · no model call".to_string(),
            None => { unpriced += 1; " · no price in config.json".to_string() }
        };
        out.push_str(&format!("  {:<12} {:<10} {:<10} model {:<28} ~{:>7} in / {:>6} out{}\n",
                              part.name, part.role, part.slot, model, tin, tout, price));
    }
    out.push_str(&format!("  estimate: ~{} tokens in / {} tokens out · {:.4} EUR", tokens_in, tokens_out, euro));
    if unpriced > 0 { out.push_str(&format!(" (partial: {} parts have no price in config.json)", unpriced)); }
    out.push('\n');
    if quorum_parts > 0 {
        out.push_str(&format!("  quorum: {} part(s) run a second answer (model {}); a disagreement adds one decider call (model {}). The estimate does not include those calls.\n",
                              quorum_parts, p.second_model.as_str().unwrap_or("a model setting"),
                              p.decider_model.as_str().unwrap_or("a model setting")));
    }
    if let Some(main) = &p.shadow_of {
        out.push_str(&format!("  shadow: this profile runs beside {} in its own worktree; its files are never merged.\n", main));
    }
    out
}

/// `penelope cluster run --dry`: prints the prompt cluster and the worker cluster, the model of every part and a rough
/// token and euro estimate. It reads the profiles and the price table; it starts no model and writes no cluster file.
pub fn dry_run(ctx: &Ctx, goal: &str, hint: Option<&str>) -> Result<String, String> {
    let mut out = String::from("Dry run · no model call, no file change.\n");
    if let Some(name) = hint.map(str::trim).filter(|s| !s.is_empty()) {
        let p = profile(ctx, name)?;
        if let Some(before) = before_profile(ctx, &p)? {
            out.push_str(&dry_cluster_text(ctx, &before, goal, "runs first and rewrites the task"));
        }
        out.push_str(&dry_cluster_text(ctx, &p, goal, ""));
        out.push_str("\nThe estimate comes from the step limit and the price table; it is a rough number, not a bill.\n");
        return Ok(out);
    }
    out.push_str("No --profile: a real run asks the router for 1 to 4 clusters, which is a model call. This dry run starts none and estimates every profile instead.\n");
    for name in profile_names(ctx) {
        match profile(ctx, &name) {
            Ok(p) => out.push_str(&dry_cluster_text(ctx, &p, goal, if p.before.is_some() { "has a before prompt cluster" } else { "" })),
            Err(e) => out.push_str(&format!("\nprofile {}: {}\n", name, e)),
        }
    }
    out.push_str("\nChoose one with --profile NAME. The estimate comes from the step limit and the price table; it is a rough number, not a bill.\n");
    Ok(out)
}

fn resolve_model(ctx: &Ctx, what: &str, model: &Value, fallback_role: &str) -> Result<Value, String> {
    if model.is_object() && model["model"].is_string() {
        return Ok(model.clone());
    }
    let key = model.as_str().unwrap_or("").trim();
    let resolved = match key {
        "main" => Some(ctx.cfg["main"].clone()),
        "weak" => Some(ctx.cfg["weak"].clone()),
        "" => crate::route::model_for(ctx, fallback_role).cloned(),
        role if crate::route::role_names().iter().any(|r| *r == role) => crate::route::model_for(ctx, role).cloned(),
        other => return Err(format!("{} names unknown model {}; use main, weak, scout, judge, worker or a model setting", what, other)),
    };
    resolved.filter(|m| m["model"].is_string()).ok_or_else(|| format!("{} has no model for its role", what))
}

fn model_for_part(ctx: &Ctx, p: &Part) -> Result<Value, String> {
    resolve_model(ctx, &format!("part {}", p.name), &p.model, &p.role)
}

fn part_model_name(p: &Part, model: &Value) -> String {
    model["label"].as_str().or_else(|| model["model"].as_str()).unwrap_or(&p.slot).to_string()
}

fn save(ctx: &Ctx, c: &Cluster) -> Result<(), String> {
    let _guard = WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // `kill` writes `killing` while the run may write `killed` at the same moment. A late kill must never downgrade
    // a final state; the run is the authority on how it stopped.
    if c.state == "killing" {
        if let Ok(text) = fs::read_to_string(cluster_file(ctx, &c.id)) {
            if let Ok(value) = serde_json::from_str::<Value>(&text) {
                if matches!(value["state"].as_str().unwrap_or(""), "done" | "failed" | "killed" | "stuck") {
                    return Ok(());
                }
            }
        }
    }
    let dir = cluster_dir(ctx, &c.id);
    fs::create_dir_all(&dir).map_err(|e| format!("cannot make {}: {}", dir.display(), e))?;
    let parts: Vec<Value> = c.parts.iter().map(|p| json!({"name": p.name, "slot": p.slot, "role": p.role, "model": p.model,
        "state": p.state, "output": p.output, "error": p.error})).collect();
    let value = json!({
        "id": c.id, "name": c.name, "profile": c.profile, "task": c.task, "parent": c.parent, "after": c.after,
        "depth": c.depth, "state": c.state, "done": c.done, "total": c.total, "started": c.started, "updated": c.updated,
        "output": c.output, "error": c.error, "memory": c.memory, "parts": parts,
        "evidence": c.evidence,
        "project": c.project, "worktree": c.worktree, "branch": c.branch, "cwd": c.cwd,
        "respawns": c.respawns, "respawned_from": c.respawned_from,
        "quorum": {"agreed": c.quorum_agreed, "escalated": c.quorum_escalated},
        "shadow": c.shadow, "shadow_of": c.shadow_of, "match_rate": c.match_rate
    });
    let path = cluster_file(ctx, &c.id);
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_string_pretty(&value).unwrap()).map_err(|e| format!("cannot write {}: {}", tmp.display(), e))?;
    fs::rename(&tmp, &path).map_err(|e| format!("cannot move {}: {}", tmp.display(), e))
}

fn load(ctx: &Ctx, id: &str) -> Result<Cluster, String> {
    let path = cluster_file(ctx, id);
    let text = fs::read_to_string(&path).map_err(|_| format!("no cluster {} (penelope cluster list)", id))?;
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("cluster {} is not JSON: {}", id, e))?;
    Ok(cluster_from(&v))
}

fn cluster_from(v: &Value) -> Cluster {
    Cluster {
        id: v["id"].as_str().unwrap_or("").to_string(),
        name: v["name"].as_str().unwrap_or("").to_string(),
        profile: v["profile"].as_str().unwrap_or("").to_string(),
        task: v["task"].as_str().unwrap_or("").to_string(),
        parent: v["parent"].as_str().filter(|s| !s.is_empty()).map(str::to_string),
        after: v["after"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default(),
        depth: v["depth"].as_u64().unwrap_or(0) as usize,
        state: v["state"].as_str().unwrap_or("queued").to_string(),
        done: v["done"].as_u64().unwrap_or(0) as usize,
        total: v["total"].as_u64().unwrap_or(0) as usize,
        started: v["started"].as_u64().unwrap_or(0),
        updated: v["updated"].as_u64().unwrap_or(0),
        output: v["output"].as_str().unwrap_or("").to_string(),
        error: v["error"].as_str().unwrap_or("").to_string(),
        memory: v["memory"].as_str().unwrap_or("").to_string(),
        evidence: v["evidence"].as_str().unwrap_or("").to_string(),
        project: v["project"].as_str().unwrap_or("").to_string(),
        worktree: v["worktree"].as_str().unwrap_or("").to_string(),
        branch: v["branch"].as_str().unwrap_or("").to_string(),
        cwd: v["cwd"].as_str().unwrap_or("").to_string(),
        respawns: v["respawns"].as_u64().unwrap_or(0) as usize,
        respawned_from: v["respawned_from"].as_str().unwrap_or("").to_string(),
        quorum_agreed: v["quorum"]["agreed"].as_u64().unwrap_or(0) as usize,
        quorum_escalated: v["quorum"]["escalated"].as_u64().unwrap_or(0) as usize,
        shadow: v["shadow"].as_bool().unwrap_or(false),
        shadow_of: v["shadow_of"].as_str().unwrap_or("").to_string(),
        match_rate: v["match_rate"].as_f64(),
        parts: v["parts"].as_array().map(|a| a.iter().map(|p| PartRun {
            name: p["name"].as_str().unwrap_or("").to_string(), slot: p["slot"].as_str().unwrap_or("").to_string(),
            role: p["role"].as_str().unwrap_or("").to_string(), model: p["model"].as_str().unwrap_or("").to_string(),
            state: p["state"].as_str().unwrap_or("queued").to_string(),
            output: p["output"].as_str().unwrap_or("").to_string(), error: p["error"].as_str().unwrap_or("").to_string(),
        }).collect()).unwrap_or_default(),
    }
}

/// The context limit of the single-model path: Qwen3-1.7B with the desktop running holds about 28,665 KV tokens
/// (Paper 35 §7). A task that fits, or that is sequential, goes to one model; only a larger or parallel task goes to
/// a cluster.
pub const SWITCH_KV_TOKENS_DESKTOP: usize = 28_665;

/// True means one model, false means a cluster. `tokens` is the estimated context of the task, `sequential` says the
/// steps cannot run in parallel.
pub fn switch_decision(tokens: usize, sequential: bool, kv_limit: usize) -> (bool, String) {
    if sequential {
        return (true, format!("single model: the task is sequential; a cluster adds orchestration without parallel gain"));
    }
    if tokens <= kv_limit {
        return (true, format!("single model: {} tokens fit the {} token KV limit; a cluster adds tokens without a gain", tokens, kv_limit));
    }
    (false, format!("cluster: {} tokens exceed the {} token KV limit", tokens, kv_limit))
}

/// A rough token estimate for a task text: 4 characters per token, at least one token per non-empty word.
pub fn estimate_tokens(text: &str) -> usize {
    let chars = text.chars().count();
    let words = text.split_whitespace().count();
    (chars / 4).max(words)
}

pub fn switch_text(tokens: Option<usize>, text: &str, sequential: bool, kv_limit: usize) -> String {
    let tokens = tokens.unwrap_or_else(|| estimate_tokens(text));
    let (single, why) = switch_decision(tokens, sequential, kv_limit);
    format!("{}\nroute: {}\ntokens: {} (estimated)\nkv limit: {}", why, if single { "single" } else { "cluster" }, tokens, kv_limit)
}

pub fn list(ctx: &Ctx) -> Vec<Cluster> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(base(ctx)) else { return out };
    for e in entries.flatten() {
        let path = e.path();
        if path.extension().and_then(|x| x.to_str()) != Some("json") { continue; }
        if let Ok(text) = fs::read_to_string(&path) {
            if let Ok(v) = serde_json::from_str::<Value>(&text) {
                if let Some(id) = v["id"].as_str() { if !id.is_empty() { out.push(cluster_from(&v)); } }
            }
        }
    }
    out.sort_by(|a, b| a.depth.cmp(&b.depth).then(b.started.cmp(&a.started)).then(a.name.cmp(&b.name)));
    out
}

pub fn working(ctx: &Ctx) -> usize {
    list(ctx).iter().filter(|c| matches!(c.state.as_str(), "queued" | "working" | "validating" | "killing")).count()
}

fn error_code(error: &str) -> &'static str {
    let e = error.to_lowercase();
    if e.is_empty() || e == "-" { return "Ok"; }
    if e.contains("budget") { return "Budget"; }
    if e.contains("timeout") || e.contains("timed out") { return "Timeout"; }
    if e.contains("loop") || e.contains("repeat") { return "NoTermination"; }
    if e.contains("ask") && e.contains("loop") { return "AskLoop"; }
    if e.contains("grammar") { return "StatusGrammar"; }
    if e.contains("stale") { return "Stale"; }
    if e.contains("permission") || e.contains("denied") { return "Permission"; }
    if e.contains("missing") { return "MissingInput"; }
    if e.contains("conflict") { return "Conflict"; }
    if e.contains("network") || e.contains("connection") { return "Network"; }
    if e.contains("not found") || e.contains("gone") { return "NotFound"; }
    if e.contains("dependency") { return "Dependency"; }
    if e.contains("test") || e.contains("check") || e.contains("validator") { return "Tests"; }
    if e.contains("build") || e.contains("compile") { return "Build"; }
    if e.contains("contract") { return "InvalidContract"; }
    if e.contains("blocked") { return "Blocked"; }
    if e.contains("stuck") { return "Stuck"; }
    "Unknown"
}

fn kerkis_line(c: &Cluster, focus: &str, error: &str) -> Result<crate::proof::KerkisLine, String> {
    let state = match c.state.as_str() {
        "queued" | "waiting" => "waiting",
        "working" | "validating" => "working",
        "done" => "done",
        "stuck" => "stuck",
        _ => "blocked",
    };
    // F is a file id. A focus that is not a file id is not inventing one; f0000:0 means no file.
    let file = if focus.starts_with('f') && focus.contains(':') { focus } else { "f0000:0" };
    let v = if c.evidence.trim().is_empty() { "-".to_string() } else { format!("e{}", &result_hash(&c.evidence)[..8]) };
    crate::proof::KerkisLine::new(&c.id, state, c.done, c.total, now().saturating_sub(c.started), file,
                                  error_code(error), &v)
}

fn report(ctx: &Ctx, c: &Cluster, focus: &str, error: &str) -> String {
    let elapsed = now().saturating_sub(c.started);
    // proof::render keeps the fields and the escape rules. V is added only when the validator ran a check.
    let r = crate::proof::Report {
        s: c.state.clone(),
        p: format!("{}/{}", c.done, c.total),
        t: format!("{}s", elapsed),
        f: if focus.is_empty() { "-".to_string() } else { focus.to_string() },
        e: if error.is_empty() { "-".to_string() } else { error.to_string() },
        v: if c.evidence.trim().is_empty() { None } else { Some(c.evidence.clone()) },
    };
    let line = format!("{}\n", crate::proof::render(&r));
    let dir = cluster_dir(ctx, &c.id);
    let _guard = REPORT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _ = fs::create_dir_all(&dir);
    let _ = fs::write(dir.join("report.txt"), &line);
    // The Kerkis line is the protocol line. report.txt keeps the older form for the human view and existing consumers.
    if let Ok(krk) = kerkis_line(c, focus, error) {
        let _ = fs::write(dir.join("report.krk"), format!("{}\n", crate::proof::render_kerkis(&krk)));
    }
    // The history is what the zombie watch reads: one line per report, with the time it was written.
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(dir.join("reports.jsonl")) {
        let _ = writeln!(file, "{} {}", now(), line.trim_end());
    }
    line
}

fn taint_path(ctx: &Ctx) -> PathBuf {
    ctx.root.join("taint.json")
}

fn content_hash(bytes: &[u8]) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{:016x}", h)
}

/// Records the writer level of a file after a write or edit. The key is the canonical path, the value the level and
/// the hash of the bytes. A later read with the same hash inherits the level; a changed hash is visible as changed.
pub fn note_file(ctx: &Ctx, path: &Path, level: u8) {
    let Ok(bytes) = fs::read(path) else { return };
    let key = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()).to_string_lossy().to_string();
    let mut map: serde_json::Map<String, Value> = fs::read_to_string(taint_path(ctx)).ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| v.as_object().cloned()).unwrap_or_default();
    map.insert(key, json!({"L": level.min(3), "hash": content_hash(&bytes)}));
    let _ = fs::write(taint_path(ctx), Value::Object(map).to_string());
}

/// The recorded level of a file and whether its bytes still match the recorded hash.
pub fn file_level(ctx: &Ctx, path: &Path) -> Option<(u8, bool)> {
    let key = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()).to_string_lossy().to_string();
    let map: Value = fs::read_to_string(taint_path(ctx)).ok().and_then(|s| serde_json::from_str(&s).ok())?;
    let entry = map.get(&key)?;
    let level = entry["L"].as_u64()? as u8;
    let hash = entry["hash"].as_str()?.to_string();
    let same = fs::read(path).map(|b| content_hash(&b) == hash).unwrap_or(false);
    Some((level, same))
}

fn memory_path(ctx: &Ctx, id: &str) -> PathBuf {
    cluster_dir(ctx, id).join("memory.md")
}

fn notes_dir(ctx: &Ctx, id: &str) -> PathBuf {
    cluster_dir(ctx, id).join("notes")
}

/// The note name of a part. The profile may carry any text in a name; the file stem is made safe first.
fn note_name(part: &Part) -> String {
    let name = safe(&part.name);
    if name.is_empty() { "part".to_string() } else { name }
}

/// The task a part works on: its own task, or the cluster task when the part names none.
fn part_task<'a>(c: &'a Cluster, part: &'a Part) -> &'a str {
    if part.task.trim().is_empty() { &c.task } else { &part.task }
}

fn write_part_note(ctx: &Ctx, c: &Cluster, profile: &Profile, part: &Part, result: Option<&str>,
                   status: crate::notes::Status) {
    let mut body = format!("Task: {}", part_task(c, part).trim());
    if let Some(text) = result {
        let text = text.trim();
        if !text.is_empty() { body.push_str(&format!("\n\nResult:\n{}", text)); }
    }
    let mut note = crate::notes::Note {
        name: note_name(part),
        source: format!("cluster {} · {} · {}", c.name, part.role,
                        if part.slot.is_empty() { &part.role } else { &part.slot }),
        date: crate::notes::today(),
        status,
        links: crate::notes::links(&body),
        body,
        level: 1,
        hash: String::new(),
    };
    let mut add_link = |name: String| {
        if !name.is_empty() && !note.links.iter().any(|l| l == &name) { note.links.push(name); }
    };
    // The parts this part waits for are links in both directions.
    for need in &part.depends_on { add_link(safe(need)); }
    // The worker list is one chain. A part sees the notes of its neighbours, so an independent part still gets the
    // context of the part before it without loading the whole cluster.
    if let Some(i) = profile.workers.iter().position(|p| p.name == part.name) {
        if i > 0 { add_link(note_name(&profile.workers[i - 1])); }
        if i + 1 < profile.workers.len() { add_link(note_name(&profile.workers[i + 1])); }
    }
    // The links must be in the body: `parse` reads `[[links]]` from the Markdown, so a link only in memory would be
    // lost on the next load. The line is also readable in the note itself.
    if !note.links.is_empty() {
        let drawn: Vec<String> = note.links.iter().map(|l| format!("[[{}]]", l)).collect();
        note.body.push_str(&format!("\n\nLinks: {}", drawn.join(" ")));
    }
    let _guard = MEMORY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _ = crate::notes::save(&notes_dir(ctx, &c.id), &note);
}

/// The prompt text of the notes at most `hops` links from `start`, stale notes left out. Public so a test can check the
/// selection of a part without a model call.
pub fn prompt_notes_text(dir: &Path, start: &str, hops: usize) -> String {
    match crate::notes::load_dir(dir) {
        Ok(notes) => crate::notes::context(&notes, start, hops),
        Err(_) => String::new(),
    }
}

/// The validator sees every open or confirmed note, never a stale one. The full `memory.md` stays the human log.
fn validator_notes_text(ctx: &Ctx, id: &str) -> String {
    let notes = match crate::notes::load_dir(&notes_dir(ctx, id)) { Ok(n) => n, Err(_) => return String::new() };
    let live: Vec<&crate::notes::Note> = notes.iter().filter(|n| n.status != crate::notes::Status::Stale).collect();
    crate::notes::text(&live)
}

fn scores_path(ctx: &Ctx) -> PathBuf {
    base(ctx).join("scores.json")
}

/// The score table groups results by part and task type. A part may name its task type in the profile; otherwise the
/// profile name is the task type, so `backend` and `frontend` runs stay apart.
fn task_type_of(profile: &Profile, part: &Part) -> String {
    if part.task_type.trim().is_empty() { profile.name.clone() } else { part.task_type.trim().to_string() }
}

/// The key of a part in the model column of the score table. A setting name such as `main` or `weak` can be written
/// back into a profile; a full model object falls back to its label, which is useful in the report but not a setting.
fn model_key_of(ctx: &Ctx, part: &Part) -> String {
    if let Some(name) = part.model.as_str().map(str::trim).filter(|s| !s.is_empty()) {
        return name.to_string();
    }
    model_for_part(ctx, part).ok().map(|m| part_model_name(part, &m)).unwrap_or_default()
}

/// Writes the outcome of the validator into `.penelope/clusters/scores.json`, one result per worker part. The model
/// setting of the part is counted beside the totals, so `penelope cluster advise --apply` can suggest a better one.
fn record_scores(ctx: &Ctx, profile: &Profile, accepted: bool) {
    let _guard = SCORE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let path = scores_path(ctx);
    let mut table = crate::advise::load(&path).unwrap_or_else(|_| crate::advise::new_table());
    for part in profile.workers.iter().filter(|p| p.role != "human") {
        let task_type = task_type_of(profile, part);
        crate::advise::record_model(&mut table, &part.name, &task_type, &model_key_of(ctx, part), accepted);
    }
    let _ = crate::advise::save(&path, &table);
}

/// Adds one Kerkis event for a part to the score table.
fn record_event(ctx: &Ctx, part: &str, kind: &str) {
    let _guard = SCORE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let path = scores_path(ctx);
    let mut table = crate::advise::load(&path).unwrap_or_else(|_| crate::advise::new_table());
    crate::advise::record_event(&mut table, part, kind);
    let _ = crate::advise::save(&path, &table);
}

fn memory_line(ctx: &Ctx, c: &Cluster, part: &Part, model: &Value, text: &str, state: &str) {
    let _guard = MEMORY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = cluster_dir(ctx, &c.id);
    let _ = fs::create_dir_all(&dir);
    let entry = format!("\n## {} · {} · {}\nmodel: {} · state: {}\n\n{}\n", part.name, part.role,
                        if part.slot.is_empty() { "part" } else { &part.slot }, part_model_name(part, model), state, text.trim());
    let _ = fs::OpenOptions::new().create(true).append(true).open(memory_path(ctx, &c.id)).and_then(|mut f| f.write_all(entry.as_bytes()));
}

fn kill_requested(ctx: &Ctx, id: &str) -> bool {
    cluster_dir(ctx, id).join("kill").is_file()
}

fn budget_spent(ctx: &Ctx, before: f64) -> f64 {
    (crate::cost::total_eur(ctx) - before).max(0.0)
}

fn register_cancel(id: &str, cancel: &Arc<AtomicBool>) {
    let mut all = CANCELS.lock().unwrap_or_else(|e| e.into_inner());
    all.retain(|(key, _)| key != id);
    all.push((id.to_string(), cancel.clone()));
}

fn unregister_cancel(id: &str) {
    CANCELS.lock().unwrap_or_else(|e| e.into_inner()).retain(|(key, _)| key != id);
}

/// Sets the flag of a cluster in this process and signals its running Claude children. A cluster started by another
/// process sees the kill file through its watcher.
fn signal_cancel(id: &str) {
    let flag = CANCELS.lock().unwrap_or_else(|e| e.into_inner()).iter()
        .find(|(key, _)| key == id).map(|(_, flag)| flag.clone());
    if let Some(flag) = flag {
        flag.store(true, Ordering::SeqCst);
    }
    kill_children(id);
}

/// Kills one process and its descendants. `pgrep` and `kill` are the smallest portable tools for a tree without a new
/// crate; a missing tool leaves the direct child kill as the fallback.
fn kill_tree(pid: u32) {
    let children: Vec<u32> = Command::new("pgrep").args(["-P", &pid.to_string()]).output().ok()
        .map(|out| String::from_utf8_lossy(&out.stdout).lines().filter_map(|line| line.trim().parse().ok()).collect())
        .unwrap_or_default();
    for child in children {
        kill_tree(child);
    }
    let _ = Command::new("kill").args(["-KILL", &pid.to_string()]).status();
}

fn register_child(key: &str, pid: u32) {
    let mut all = CHILDREN.lock().unwrap_or_else(|e| e.into_inner());
    all.retain(|(slot, _)| slot != key);
    all.push((key.to_string(), pid));
}

fn unregister_child(key: &str) {
    CHILDREN.lock().unwrap_or_else(|e| e.into_inner()).retain(|(slot, _)| slot != key);
}

fn kill_children(id: &str) {
    let prefix = format!("{}/", id);
    let pids: Vec<u32> = CHILDREN.lock().unwrap_or_else(|e| e.into_inner()).iter()
        .filter(|(key, _)| key.starts_with(&prefix)).map(|(_, pid)| *pid).collect();
    for pid in pids {
        kill_tree(pid);
    }
}

/// Watches the kill file of one cluster while it runs. 100 ms is short enough to end a part inside the two-second
/// promise and long enough not to spin on disk.
struct KillWatch {
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl KillWatch {
    fn start(ctx: &Ctx, id: &str, cancel: Arc<AtomicBool>) -> KillWatch {
        let stop = Arc::new(AtomicBool::new(false));
        let idle = stop.clone();
        let dir = cluster_dir(ctx, id);
        let id = id.to_string();
        let handle = std::thread::spawn(move || {
            while !idle.load(Ordering::SeqCst) {
                if dir.join("kill").is_file() {
                    cancel.store(true, Ordering::SeqCst);
                    kill_children(&id);
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        });
        KillWatch { stop, handle: Some(handle) }
    }
}

impl Drop for KillWatch {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

struct CancelGuard(String);

impl Drop for CancelGuard {
    fn drop(&mut self) {
        unregister_cancel(&self.0);
    }
}

/// Writes the periodic report of a running cluster and hands it to the zombie watch. A report interval of 0 turns the
/// watch off, which is the default for a short cluster: only a cluster that asked for reports can be idle that way.
struct ZombieThread {
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl ZombieThread {
    fn start(ctx: &Ctx, id: &str, report_every: u64, watch: Arc<Mutex<Watch>>, focus: Arc<Mutex<String>>,
             cancel: Arc<AtomicBool>) -> ZombieThread {
        let stop = Arc::new(AtomicBool::new(false));
        let idle = stop.clone();
        let handle = if report_every == 0 {
            None
        } else {
            let ctx = ctx.clone();
            let id = id.to_string();
            Some(std::thread::spawn(move || {
                let mut last = now();
                while !idle.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(250));
                    if now().saturating_sub(last) < report_every { continue; }
                    last = now();
                    let Ok(c) = load(&ctx, &id) else { continue };
                    if c.state != "working" && c.state != "validating" { continue; }
                    let focus = focus.lock().unwrap_or_else(|e| e.into_inner()).clone();
                    if focus.is_empty() { continue; }
                    report(&ctx, &c, &focus, "");
                    let cause = watch.lock().unwrap_or_else(|e| e.into_inner()).note_report(&c, &focus);
                    if cause.is_some() {
                        cancel.store(true, Ordering::SeqCst);
                        kill_children(&id);
                        break;
                    }
                }
            }))
        };
        ZombieThread { stop, handle }
    }
}

impl Drop for ZombieThread {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// The two hard limits of a part: the budget of its cluster and the budget of the whole session. Both are checked
/// after every model answer with the usage of the loop that is still running, so the call that crosses a limit is the
/// last call of the part.
#[derive(Clone, Copy)]
struct Limits {
    cluster: f64,
    cluster_before: f64,
    session: f64,
    session_before: f64,
}

impl Limits {
    fn check(&self, ctx: &Ctx, model: &str, usage: &crate::llm::Usage) -> Option<String> {
        let pending = crate::cost::euro_for(ctx, model, usage.tokens_in, usage.tokens_out).unwrap_or(0.0);
        self.at(ctx, pending)
    }

    fn over(&self, ctx: &Ctx) -> Option<String> {
        self.at(ctx, 0.0)
    }

    fn at(&self, ctx: &Ctx, pending: f64) -> Option<String> {
        let cluster_spent = (crate::cost::total_eur(ctx) - self.cluster_before).max(0.0) + pending;
        if self.cluster > 0.0 && cluster_spent >= self.cluster {
            return Some(format!("BUDGET: the cluster used {:.4} of {:.2} EUR in the call that crossed its limit", cluster_spent, self.cluster));
        }
        let session_spent = (crate::cost::total_eur(ctx) - self.session_before).max(0.0) + pending;
        if self.session > 0.0 && session_spent >= self.session {
            return Some(format!("BUDGET: the session used {:.4} of {:.2} EUR in the call that crossed its limit", session_spent, self.session));
        }
        None
    }
}

/// A stable 64-bit FNV-1a hash of a tool result, so the event log does not keep the result text.
fn result_hash(text: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in text.trim().as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{:016x}", h)
}

/// The zombie watch of one cluster. It counts reports with one `P:` and `F:` per focus and equal tool calls per part.
/// `stuck_after` of either marks the cluster stuck; the cause goes into the dump that the next prompt names.
struct Watch {
    stuck_after: usize,
    reports: HashMap<String, (String, String, usize, u64)>,
    calls: HashMap<String, Vec<String>>,
    cause: Option<String>,
}

impl Watch {
    fn new(stuck_after: usize) -> Watch {
        Watch { stuck_after: stuck_after.max(1), reports: HashMap::new(), calls: HashMap::new(), cause: None }
    }

    fn cause(&self) -> Option<String> {
        self.cause.clone()
    }

    fn note_report(&mut self, c: &Cluster, focus: &str) -> Option<String> {
        if self.cause.is_some() { return self.cause.clone(); }
        let p = format!("{}/{}", c.done, c.total);
        let f = focus.to_string();
        let at = now();
        let entry = self.reports.entry(f.clone()).or_insert((p.clone(), f.clone(), 0, at));
        if entry.0 == p && entry.1 == f {
            entry.2 += 1;
        } else {
            *entry = (p.clone(), f.clone(), 1, at);
        }
        entry.3 = at;
        if entry.2 >= self.stuck_after {
            let cause = format!("repeated report {} times at P:{} F:{}", entry.2, p, f);
            self.cause = Some(cause.clone());
            return Some(cause);
        }
        None
    }

    /// Every tool call of a part goes into `calls.jsonl` with a hash of the normalized result. `stuck_after` equal
    /// name, arguments and result in a row are a zombie. The ring keeps 8 entries, so period 1 to 3 is visible.
    fn note_call(&mut self, ctx: &Ctx, c: &Cluster, part: &Part, name: &str, args: &Value, result: &str) -> Option<String> {
        let sig = format!("{} {} {}", name, serde_json::to_string(args).unwrap_or_else(|_| "null".to_string()),
                          result_hash(result));
        let dir = cluster_dir(ctx, &c.id);
        let _ = fs::create_dir_all(&dir);
        if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(dir.join("calls.jsonl")) {
            let _ = writeln!(file, "{} {} {}", now(), part.name, sig.chars().take(500).collect::<String>());
        }
        if self.cause.is_some() { return self.cause.clone(); }
        let repeated = {
            let list = self.calls.entry(part.name.clone()).or_default();
            list.push(sig.clone());
            while list.len() > 8 { list.remove(0); }
            let n = self.stuck_after.min(8);
            list.len() >= n && list[list.len() - n..].iter().all(|item| item == &sig)
        };
        if repeated {
            let cause = format!("repeated tool call {} times: {}", self.stuck_after, sig.chars().take(200).collect::<String>());
            self.cause = Some(cause.clone());
            return Some(cause);
        }
        None
    }

    /// A Claude part writes its calls to a log; the same rule applies after the command ended.
    fn note_checks(&mut self, ctx: &Ctx, c: &Cluster, part: &Part, checks: &[Check]) -> Option<String> {
        for check in checks {
            let args = json!({"target": check.target});
            if let Some(cause) = self.note_call(ctx, c, part, &check.name, &args, if check.ok { "ok" } else { "fail" }) {
                return Some(cause);
            }
        }
        None
    }
}

#[derive(Clone, Debug)]
struct Check {
    name: String,
    target: String,
    ok: bool,
}

#[derive(Default)]
struct Done {
    text: String,
    checks: Vec<Check>,
}

/// What a tool call checked: the command of `run`, otherwise the path, folder or text of the call.
fn check_target(name: &str, args: &Value) -> String {
    match name {
        "run" => args["command"].as_str().unwrap_or("").trim().to_string(),
        _ => ["path", "dir", "text", "query"].iter().find_map(|k| args[*k].as_str()).unwrap_or("").trim().to_string(),
    }
}

/// `tools::run` returns `exit code N` as its first line. A check is ok only when that code is zero.
fn check_ok(name: &str, text: &str, dispatched: bool) -> bool {
    if !dispatched { return false; }
    let start = text.trim_start();
    if start.starts_with("NOT ") || start.starts_with("cannot ") || start.starts_with("the command could not start") {
        return false;
    }
    if name == "run" {
        if let Some(code) = start.lines().next().and_then(|l| l.strip_prefix("exit code ")) {
            return code.trim().parse::<i64>().map(|c| c == 0).unwrap_or(true);
        }
    }
    true
}

/// One line of evidence: the first successful check, or the first check when all failed. The expected form for a
/// command check is `cat hello.txt ok`.
fn evidence_of(checks: &[Check]) -> String {
    let chosen = checks.iter().find(|c| c.ok && c.name == "run")
        .or_else(|| checks.iter().find(|c| c.ok))
        .or_else(|| checks.first());
    let Some(c) = chosen else { return String::new(); };
    let status = if c.ok { "ok" } else { "failed" };
    if c.target.is_empty() {
        format!("{} {}", c.name, status)
    } else if c.name == "run" {
        format!("{} {}", c.target, status)
    } else {
        format!("{} {} {}", c.name, c.target, status)
    }
}

/// Reads the tool log of a Claude CLI part. Every line is one call that reached `mcp::call_raw`.
fn checks_from_log(path: &Path) -> Vec<Check> {
    let Ok(text) = fs::read_to_string(path) else { return Vec::new(); };
    text.lines().filter_map(|line| serde_json::from_str::<Value>(line).ok()).filter_map(|v| {
        let name = v["name"].as_str()?;
        let args = v["args"].clone();
        let result = v["result"].as_str().unwrap_or("");
        Some(Check { name: name.to_string(), target: check_target(name, &args),
                     ok: check_ok(name, result, v["ok"].as_bool().unwrap_or(false)) })
    }).collect()
}

/// Fills the cwd of a tool call that takes one and did not name it. A call for a tool without cwd stays as it is.
fn args_with_cwd(name: &str, args: &Value, cwd: &str) -> Value {
    let mut args = args.clone();
    if cwd.trim().is_empty() { return args; }
    let takes = crate::spec::schema(name).map(|s| s["properties"].get("cwd").is_some()).unwrap_or(false);
    if !takes { return args; }
    if let Some(obj) = args.as_object_mut() {
        let given = obj.get("cwd").and_then(Value::as_str).map(|s| !s.trim().is_empty()).unwrap_or(false);
        if !given { obj.insert("cwd".to_string(), json!(cwd)); }
    }
    args
}

fn run_model(ctx: &Ctx, c: &Cluster, part: &Part, system: &str, task: &str, max_steps: usize,
             cancel: &AtomicBool, limits: Limits, watch: &Arc<Mutex<Watch>>) -> Result<Done, String> {
    let model = model_for_part(ctx, part)?;
    if let Some(family) = crate::crew::family_of(&model) {
        return Err(format!("{} slot is in training and cannot answer yet", family));
    }
    let kind = crate::llm::kind_of(&model).to_string();
    if crate::llm::is_api(&kind) {
        let tools: Vec<Value> = crate::mcp::tool_specs_for(ctx, &part.role).as_array().cloned().unwrap_or_default();
        let mut history = vec![crate::llm::user_message(task)];
        let mut usage = crate::llm::Usage::default();
        let role = part.role.clone();
        let cluster_cwd = c.cwd.clone();
        let model_name = model["model"].as_str().unwrap_or("").to_string();
        let mut checks: Vec<Check> = Vec::new();
        let mut tick = |usage: &crate::llm::Usage| {
            if let Some(stop) = limits.check(ctx, &model_name, usage) { return Some(stop); }
            if let Some(cause) = watch.lock().unwrap_or_else(|e| e.into_inner()).cause() {
                cancel.store(true, Ordering::SeqCst);
                return Some(format!("STUCK: {}", cause));
            }
            None
        };
        let out = crate::agents::with_mcp_scope(&part.mcp, || crate::route::with_role(&role, || with_cluster(&c.id, || {
            crate::llm::run_loop_with(&model, system, &mut history, &tools, max_steps, Some(cancel), &mut usage, &mut tick,
                &mut |call| {
                    // A part works in its cluster folder. A tool call without a cwd gets the worktree, never the live tree.
                    let args = args_with_cwd(&call.name, &call.args, &cluster_cwd);
                    let (text, ok) = crate::tools::dispatch_retry(ctx, &role, &call.name, &args);
                    if watch.lock().unwrap_or_else(|e| e.into_inner()).note_call(ctx, c, part, &call.name, &args, &text).is_some() {
                        record_event(ctx, &part.name, "loop_kill");
                        cancel.store(true, Ordering::SeqCst);
                    }
                    checks.push(Check { name: call.name.clone(), target: check_target(&call.name, &args), ok: check_ok(&call.name, &text, ok) });
                    (text, ok)
                }, &mut |_| {})
        })));
        crate::cost::record(ctx, &format!("cluster:{}", c.name), &model_name, usage.tokens_in, usage.tokens_out);
        if let Some(cause) = watch.lock().unwrap_or_else(|e| e.into_inner()).cause() {
            return Err(format!("STUCK: {}", cause));
        }
        let out = out?;
        if let Some(stop) = limits.over(ctx) { return Err(stop); }
        return Ok(Done { text: out, checks });
    }
    if kind != "claude-cli" {
        return Err(format!("the cluster cannot call the model kind {}", kind));
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let log = cluster_dir(ctx, &c.id).join(format!("tools-{}.jsonl", safe(&part.name)));
    let _ = fs::remove_file(&log);
    let mut env = json!({"PENELOPE_ROLE": part.role, "PENELOPE_CLUSTER": c.id});
    let task_cwd = crate::eval::task_cwd();
    if !task_cwd.is_empty() {
        env["PENELOPE_TASK_CWD"] = json!(task_cwd);
    }
    if let Ok(dir) = std::env::var("PENELOPE_APPROVE_DIR") { env["PENELOPE_APPROVE_DIR"] = json!(dir); }
    // The validator's MCP server writes every tool call to this file; the cluster reads it after the command ends.
    env["PENELOPE_TOOL_LOG"] = json!(log.to_string_lossy());
    let cfg = json!({"mcpServers": {"penelope": {"command": exe.to_string_lossy(), "args": ["mcp", "--root", ctx.root.to_string_lossy()], "env": env}}}).to_string();
    let allowed: Vec<String> = crate::route::allowed_for(&part.role).iter().map(|t| format!("mcp__penelope__{}", t)).collect();
    let mut cmd = Command::new("claude");
    cmd.args(["-p", task, "--output-format", "json", "--model", model["model"].as_str().unwrap_or("sonnet"),
              "--append-system-prompt", system, "--strict-mcp-config", "--mcp-config", &cfg, "--tools", "", "--allowedTools", &allowed.join(",")]);
    if let Some(level) = crate::llm::effort(&model) { cmd.args(["--effort", level]); }
    // Every tool of the part runs inside the worktree of its cluster. The Claude command itself also starts there,
    // so a tool that writes without a cwd cannot reach the live tree.
    if !c.cwd.trim().is_empty() { cmd.current_dir(&c.cwd); }
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn()
        .map_err(|e| format!("cannot start the claude command: {}", e))?;
    let pid = child.id();
    let slot = format!("{}/{}", c.id, part.name);
    register_child(&slot, pid);
    let mut stdout = child.stdout.take().ok_or_else(|| "the claude command has no output pipe".to_string())?;
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stdout.read_to_end(&mut bytes);
        bytes
    });
    let mut cancelled = false;
    loop {
        if cancel.load(Ordering::SeqCst) || kill_requested(ctx, &c.id) {
            cancelled = true;
            break;
        }
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => {
                unregister_child(&slot);
                let _ = child.kill();
                let _ = reader.join();
                return Err(format!("the claude command failed: {}", e));
            }
        }
    }
    if cancelled {
        // The command starts one tree: the model, its tool servers and their children. All of it goes.
        kill_tree(pid);
        let _ = child.kill();
        let _ = child.wait();
    }
    let bytes = reader.join().unwrap_or_default();
    unregister_child(&slot);
    if cancelled { return Err("stopped".to_string()); }
    let status = child.wait().map_err(|e| format!("the claude command failed: {}", e))?;
    let v: Value = serde_json::from_slice(&bytes).map_err(|_| format!("the claude command ended with {} and no result", status))?;
    if v["is_error"].as_bool().unwrap_or(false) {
        return Err(v["result"].as_str().unwrap_or("the claude command reported an error").trim().to_string());
    }
    let tokens_in = v["usage"]["input_tokens"].as_u64().unwrap_or(0) + v["usage"]["cache_creation_input_tokens"].as_u64().unwrap_or(0);
    let tokens_out = v["usage"]["output_tokens"].as_u64().unwrap_or(0);
    crate::cost::record(ctx, &format!("cluster:{}", c.name), model["model"].as_str().unwrap_or(""), tokens_in, tokens_out);
    let checks = checks_from_log(&log);
    let _ = fs::remove_file(&log);
    if let Some(stop) = limits.over(ctx) { return Err(stop); }
    if let Some(cause) = watch.lock().unwrap_or_else(|e| e.into_inner()).note_checks(ctx, c, part, &checks) {
        return Err(format!("STUCK: {}", cause));
    }
    Ok(Done { text: v["result"].as_str().unwrap_or("").trim().to_string(), checks })
}

fn worker_system(c: &Cluster, p: &Part, model: &Value, feedback: &str) -> String {
    format!("You are the {} part of the Penelope cluster {} (slot {}). This is not a subagent: you are one part of a cluster, \\
and the cluster has one memory file. The cluster task: {}.\nYour focus: {}.\nModel: {}.\nWork only on your part, report facts, file paths and command output. \\
Do not start separate agents. If a tool refuses, say so. The validator may send the result back.{}",
            p.name, c.name, p.slot, c.task, if p.task.is_empty() { "the whole task" } else { &p.task },
            part_model_name(p, model),
            if feedback.is_empty() { String::new() } else { format!("\nThe validator sent this back: {}", feedback) })
}

fn validator_system(c: &Cluster, p: &Part, model: &Value) -> String {
    format!("You are the validator part of the Penelope cluster {} (slot {}). This is not a subagent. Check the cluster task \\
against the one memory file and the project's checks. You must run at least one check with your tools (run, read, grep, \\
tree, outline, json_query or git_diff) before you answer; a PASS without a tool call is no evidence and the cluster fails. \\
Answer on the first line PASS or FAIL: one short reason, then the check and its result. Model: {}.", c.name, p.slot, part_model_name(p, model))
}

fn validator_prompt(c: &Cluster, memory: &str) -> String {
    format!("Cluster task: {}\n\nThe parts wrote these notes:\n{}\n\nCheck it. PASS only when the task is really done and the evidence is in the notes. \\
Otherwise FAIL with the smallest exact correction.", c.task, memory.chars().take(12_000).collect::<String>())
}

/// The answers of two independent runs agree when their whitespace and letter case are the same. The compare is
/// deliberately strict: a different sentence is a disagreement, and a nearly equal answer costs one decider call.
fn normalized_answer(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ").trim_end_matches(['.', '!', '?']).to_lowercase()
}

fn answers_agree(first: &str, second: &str) -> bool {
    let first = normalized_answer(first);
    !first.is_empty() && first == normalized_answer(second)
}

/// The number of answers for a part: its own `quorum`, else the profile's. 2 is the only vote the profile parser keeps.
fn quorum_of(profile: &Profile, part: &Part) -> usize {
    let from_part = if part.quorum > 0 { part.quorum } else { profile.quorum };
    from_part.clamp(1, 2)
}

/// What a quorum run did. The first answer is the normal part result; `decided` is the answer that replaces it only
/// after an escalation.
#[derive(Clone, Debug)]
struct QuorumOutcome {
    agreed: bool,
    escalated: bool,
    second_model: String,
    decider_model: String,
    second: String,
    decided: String,
}

/// Asks one model without tools, records its cost and checks the hard budgets. Used for the second quorum answer and
/// for the decider, so a quorum can never run without its tokens reaching `usage.json`.
fn quorum_ask(ctx: &Ctx, c: &Cluster, model: &Value, what: &str, system: &str, task: &str,
              limits: Limits, cancel: &AtomicBool) -> Result<(String, String), String> {
    if cancel.load(Ordering::SeqCst) { return Err("stopped".to_string()); }
    let name = model["model"].as_str().unwrap_or("").to_string();
    let (text, tokens_in, tokens_out) = crate::weak::ask_count(model, system, task)
        .map_err(|e| {
            if cancel.load(Ordering::SeqCst) {
                "stopped".to_string()
            } else {
                let shown = model["label"].as_str().or_else(|| model["model"].as_str()).unwrap_or("model");
                format!("QUORUM: the {} {} did not answer: {}", what, shown, e)
            }
        })?;
    crate::cost::record(ctx, &format!("cluster:{}:quorum", c.name), &name, tokens_in, tokens_out);
    if let Some(stop) = limits.over(ctx) { return Err(stop); }
    if cancel.load(Ordering::SeqCst) { return Err("stopped".to_string()); }
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err(format!("QUORUM: the {} returned no text", what));
    }
    Ok((text, name))
}

/// The second answer and, only on a disagreement, the one decider call. The second model never sees the first answer,
/// so the two answers are independent. The decider sees both answers and returns the result the part keeps.
fn quorum_check(ctx: &Ctx, c: &Cluster, part: &Part, profile: &Profile, task: &str, first: &str,
                primary: &Value, limits: Limits, cancel: &AtomicBool) -> Result<QuorumOutcome, String> {
    let second_model = resolve_model(ctx, &format!("profile {} second_model", profile.name), &profile.second_model, "worker")?;
    let second_system = format!("You are the second model of the Penelope cluster {} for part {} (slot {}). The cluster task: {}.\nYour focus: {}.\nYou have no tools: answer from the task and its context only. The first answer is not shown to you; give your own answer in the form the part would report.\nModel: {}.",
                                c.name, part.name, part.slot, c.task,
                                if part.task.is_empty() { "the whole task" } else { &part.task },
                                part_model_name(part, &second_model));
    let (second, second_name) = quorum_ask(ctx, c, &second_model, "second model", &second_system, task, limits, cancel)?;
    let mut outcome = QuorumOutcome {
        agreed: answers_agree(first, &second), escalated: false,
        second_model: second_name, decider_model: String::new(), second: second.clone(), decided: String::new(),
    };
    if outcome.agreed { return Ok(outcome); }
    let decider_model = resolve_model(ctx, &format!("profile {} decider_model", profile.name), &profile.decider_model, "worker")?;
    let decider_system = "You are the deciding model of a Penelope cluster. Two parts answered the same task independently and disagree. \
Decide which answer is correct, or give the correct answer yourself. You have no tools. Answer with the final result only, no explanation.";
    let decider_task = format!("Task:\n{}\n\nFirst answer ({}):\n{}\n\nSecond answer ({}):\n{}\n\nGive the final result only.",
                               task, part_model_name(part, primary), first.chars().take(6_000).collect::<String>(),
                               outcome.second_model, second.chars().take(6_000).collect::<String>());
    let (decided, decider_name) = quorum_ask(ctx, c, &decider_model, "decider model", decider_system, &decider_task, limits, cancel)?;
    outcome.escalated = true;
    outcome.decider_model = decider_name;
    outcome.decided = decided;
    Ok(outcome)
}

/// The question a human part puts to the person. An empty task asks for the whole cluster task.
fn human_question(c: &Cluster, part: &Part) -> String {
    if part.task.trim().is_empty() {
        format!("Cluster {} asks for approval: {} — yes or no?", c.name, c.task)
    } else {
        part.task.trim().to_string()
    }
}

/// Asks the person a yes/no question through the approval files of the interface. Without an interface the answer comes
/// from config `human`; when that is not set the part refuses instead of assuming yes.
fn human_answer(ctx: &Ctx, c: &Cluster, part: &Part, question: &str) -> Result<bool, String> {
    if std::env::var("PENELOPE_APPROVE_DIR").ok().filter(|d| !d.trim().is_empty()).is_some() {
        if let Some(yes) = crate::tools::ask_person(question, &c.cwd, "human", "") {
            return Ok(yes);
        }
    }
    match ctx.cfg["human"].as_str().map(|s| s.trim().to_lowercase()).as_deref() {
        Some("yes") => Ok(true),
        Some("no") => Ok(false),
        _ => Err(format!("the human part {} needs a yes or no, but no person can answer here. Set PENELOPE_APPROVE_DIR to the interface folder, or set \"human\": \"yes\" or \"human\": \"no\" in config.json for a non-interactive run; Penelope never assumes yes.", part.name)),
    }
}

fn run_workers(ctx: &Ctx, c: &mut Cluster, profile: &Profile, feedback: &str, limits: Limits, cancel: &Arc<AtomicBool>,
               watch: &Arc<Mutex<Watch>>, focus: &Arc<Mutex<String>>) -> Result<bool, String> {
    let mut all_ok = true;
    c.error.clear();
    c.evidence.clear();
    let mut pending: Vec<usize> = (0..profile.workers.len()).collect();
    let mut finished: Vec<usize> = Vec::new();
    while !pending.is_empty() {
        if kill_requested(ctx, &c.id) || cancel.load(Ordering::SeqCst) { return Err("killed".to_string()); }
        if let Some(cause) = watch.lock().unwrap_or_else(|e| e.into_inner()).cause() {
            return Err(format!("STUCK: {}", cause));
        }
        if let Some(stop) = limits.over(ctx) {
            c.error = stop;
            return Err(c.error.clone());
        }
        // A part is ready when every part it needs has finished. A failed dependency keeps its dependants closed.
        let ready: Vec<usize> = pending.iter().copied().filter(|i| {
            profile.workers[*i].depends_on.iter().all(|need| {
                profile.workers.iter().position(|p| &p.name == need).map(|j| finished.contains(&j)).unwrap_or(false)
            })
        }).collect();
        if ready.is_empty() {
            for i in &pending {
                c.parts[*i].state = "failed".to_string();
                c.parts[*i].error = "a part it needs did not finish".to_string();
                all_ok = false;
            }
            c.error = "a part waits for a dependency that did not finish".to_string();
            c.updated = now();
            save(ctx, c)?;
            report(ctx, c, "dependency", &c.error);
            break;
        }
        // A human part always runs alone: one person answers one question at a time and no model may guess in parallel.
        let human = ready.iter().copied().find(|i| profile.workers[*i].role == "human");
        let wave: Vec<usize> = if let Some(i) = human { vec![i] } else { ready.into_iter().take(profile.max_parallel.max(1)).collect() };
        if let Some(i) = human {
            let part = profile.workers[i].clone();
            pending.retain(|index| *index != i);
            c.parts[i].state = "working".to_string();
            c.parts[i].model = if part.slot.is_empty() { "you".to_string() } else { part.slot.clone() };
            c.updated = now();
            c.state = "working".to_string();
            save(ctx, c)?;
            *focus.lock().unwrap_or_else(|e| e.into_inner()) = part.name.clone();
            report(ctx, c, &part.name, "");
            let question = human_question(c, &part);
            let human_model = json!("human");
            match human_answer(ctx, c, &part, &question) {
                Ok(true) => {
                    c.parts[i].state = "done".to_string();
                    c.parts[i].output = "yes".to_string();
                    c.parts[i].error.clear();
                    c.output.push_str(&format!("\n--- {} (human) ---\nyes\n", part.name));
                    memory_line(ctx, c, &part, &human_model, &format!("question: {}\nanswer: yes", question), "done");
                    c.done = (c.done + 1).min(c.total);
                    finished.push(i);
                    c.updated = now();
                    save(ctx, c)?;
                    report(ctx, c, &part.name, "");
                }
                Ok(false) => {
                    c.parts[i].state = "failed".to_string();
                    c.parts[i].output = "no".to_string();
                    c.parts[i].error = "the human said no".to_string();
                    memory_line(ctx, c, &part, &human_model, &format!("question: {}\nanswer: no\nthe cluster stops here", question), "failed");
                    c.error = format!("human part {} said no; the cluster stops before the parts that need it", part.name);
                    c.state = "blocked".to_string();
                    c.updated = now();
                    save(ctx, c)?;
                    report(ctx, c, &part.name, &c.error);
                    return Err(c.error.clone());
                }
                Err(e) => {
                    c.parts[i].state = "failed".to_string();
                    c.parts[i].error = e.clone();
                    memory_line(ctx, c, &part, &human_model, &format!("question: {}\nNOT answered: {}", question, e), "failed");
                    c.error = format!("human part {}: {}", part.name, e);
                    c.state = "blocked".to_string();
                    c.updated = now();
                    save(ctx, c)?;
                    report(ctx, c, &part.name, &c.error);
                    return Err(c.error.clone());
                }
            }
            continue;
        }
        let mut jobs: Vec<(usize, Part, Value, String, String, usize)> = Vec::new();
        for i in &wave {
            let part = profile.workers[*i].clone();
            let model = match model_for_part(ctx, &part) {
                Ok(model) => model,
                Err(e) => {
                    c.parts[*i].state = "failed".to_string();
                    c.parts[*i].error = e;
                    all_ok = false;
                    pending.retain(|index| index != i);
                    continue;
                }
            };
            c.parts[*i].model = part_model_name(&part, &model);
            c.parts[*i].state = "working".to_string();
            let system = worker_system(c, &part, &model, feedback);
            // The part's own task note starts the walk. Only notes at most two links away reach the prompt, and a stale
            // note is left out by `notes::context`.
            write_part_note(ctx, c, profile, &part, None, crate::notes::Status::Open);
            let notes = prompt_notes_text(&notes_dir(ctx, &c.id), &note_name(&part), 2);
            let task = format!("Cluster task: {}\n\nYour part: {}\n\nNotes within two links of your task note (stale notes left out):\n{}\n\nDo your part and report the evidence.",
                               c.task, if part.task.is_empty() { &c.task } else { &part.task },
                               if notes.trim().is_empty() { "(none yet)" } else { notes.trim() });
            let quorum = quorum_of(profile, &part);
            jobs.push((*i, part, model, system, task, quorum));
        }
        for (i, _, _, _, _, _) in &jobs { pending.retain(|index| index != i); }
        if jobs.is_empty() { continue; }
        c.done = c.done.min(c.total);
        c.updated = now();
        c.state = "working".to_string();
        save(ctx, c)?;
        let wave_focus = if jobs.len() > 1 { "parallel".to_string() } else { jobs[0].1.name.clone() };
        *focus.lock().unwrap_or_else(|e| e.into_inner()) = wave_focus.clone();
        report(ctx, c, &wave_focus, "");
        let mut handles = Vec::new();
        for (i, part, model, system, task, quorum) in jobs {
            let ctx = ctx.clone();
            let snapshot = c.clone();
            let profile_snapshot = profile.clone();
            let cancel = cancel.clone();
            let watch = watch.clone();
            let steps = profile.max_steps;
            handles.push(std::thread::spawn(move || {
                let out = run_model(&ctx, &snapshot, &part, &system, &task, steps, &cancel, limits, &watch);
                let (out, vote) = match out {
                    Ok(done) if quorum >= 2 => {
                        match quorum_check(&ctx, &snapshot, &part, &profile_snapshot, &task, &done.text,
                                           &model, limits, &cancel) {
                            Ok(vote) => (Ok(done), Some(vote)),
                            Err(e) => (Err(e), None),
                        }
                    }
                    other => (other, None),
                };
                (i, part, model, out, vote)
            }));
        }
        let mut results = Vec::new();
        for handle in handles {
            match handle.join() {
                Ok(result) => results.push(result),
                Err(_) => return Err("a worker part panicked".to_string()),
            }
        }
        for (i, part, model, out, vote) in results {
            // A cancel that came from the zombie watch belongs to `stuck`, not to `killed`.
            let out = match out {
                Err(e) if e == "stopped" => match watch.lock().unwrap_or_else(|e| e.into_inner()).cause() {
                    Some(cause) => Err(format!("STUCK: {}", cause)),
                    None => Err(e),
                },
                other => other,
            };
            match out {
                Ok(done) => {
                    let text = vote.as_ref().map(|v| if v.escalated { v.decided.clone() } else { done.text.clone() })
                        .unwrap_or_else(|| done.text.clone());
                    if let Some(v) = &vote {
                        if v.agreed { c.quorum_agreed += 1; }
                        if v.escalated { c.quorum_escalated += 1; }
                    }
                    c.parts[i].state = "done".to_string();
                    c.parts[i].output = text.clone();
                    c.parts[i].error.clear();
                    c.output.push_str(&format!("\n--- {} ({}) ---\n{}\n", part.name, part.slot, text));
                    write_part_note(ctx, c, profile, &part, Some(&text), crate::notes::Status::Open);
                    memory_line(ctx, c, &part, &model, &text, "done");
                    if c.evidence.trim().is_empty() {
                        let evidence = evidence_of(&done.checks);
                        if !evidence.is_empty() { c.evidence = evidence; }
                    }
                    if let Some(v) = &vote {
                        let result = if v.escalated { format!("escalated to {}", v.decider_model) } else { "agreed".to_string() };
                        append_memory(ctx, c, &format!("\n## quorum · {}\n\nfirst answer ({}):\n{}\n\nsecond answer ({}):\n{}\n\n{}: {}\n",
                                                      part.name, part_model_name(&part, &model),
                                                      done.text.chars().take(4_000).collect::<String>(),
                                                      v.second_model, v.second.chars().take(4_000).collect::<String>(),
                                                      result, if v.escalated { &v.decided } else { &text }));
                        stream_note(ctx, &c.id, &format!("QUORUM {} · {} · second {} · {}",
                                                          part.name, if v.agreed { "agreed" } else { "disagree" },
                                                          v.second_model, if v.escalated { &v.decider_model } else { "no decider call" }));
                    }
                    c.done = (c.done + 1).min(c.total);
                    finished.push(i);
                    if let Some(stop) = limits.over(ctx) {
                        // The answer is not merged: the call that crossed the limit is the last call of the cluster.
                        c.parts[i].state = "failed".to_string();
                        c.parts[i].error = stop.clone();
                        c.error = stop;
                        c.updated = now();
                        save(ctx, c)?;
                        report(ctx, c, &part.name, &c.error);
                        return Err(c.error.clone());
                    }
                }
                Err(e) if e.starts_with("BUDGET: ") => {
                    c.parts[i].state = "failed".to_string();
                    c.parts[i].error = e.clone();
                    write_part_note(ctx, c, profile, &part, Some(&format!("NOT done: {}", e)), crate::notes::Status::Open);
                    memory_line(ctx, c, &part, &model, &format!("NOT done: {}", e), "failed");
                    c.error = e;
                    c.updated = now();
                    save(ctx, c)?;
                    report(ctx, c, &part.name, &c.error);
                    return Err(c.error.clone());
                }
                Err(e) if e.starts_with("STUCK: ") => {
                    c.parts[i].state = "failed".to_string();
                    c.parts[i].error = e.clone();
                    write_part_note(ctx, c, profile, &part, Some(&format!("NOT done: {}", e)), crate::notes::Status::Open);
                    memory_line(ctx, c, &part, &model, &format!("NOT done: {}", e), "failed");
                    c.error = e;
                    c.updated = now();
                    save(ctx, c)?;
                    report(ctx, c, &part.name, &c.error);
                    return Err(c.error.clone());
                }
                Err(e) if e == "stopped" && (cancel.load(Ordering::SeqCst) || kill_requested(ctx, &c.id)) => {
                    return Err("killed".to_string());
                }
                Err(e) => {
                    c.parts[i].state = "failed".to_string();
                    c.parts[i].error = e.clone();
                    all_ok = false;
                    write_part_note(ctx, c, profile, &part, Some(&format!("NOT done: {}", e)), crate::notes::Status::Open);
                    memory_line(ctx, c, &part, &model, &format!("NOT done: {}", e), "failed");
                    c.error = format!("{}: {}", part.name, e);
                }
            }
            if let Some(cause) = watch.lock().unwrap_or_else(|e| e.into_inner()).cause() {
                c.updated = now();
                save(ctx, c)?;
                report(ctx, c, &part.name, &c.error);
                return Err(format!("STUCK: {}", cause));
            }
            c.updated = now();
            save(ctx, c)?;
            report(ctx, c, &part.name, &c.error);
        }
    }
    Ok(all_ok)
}

fn validate(ctx: &Ctx, c: &mut Cluster, profile: &Profile, limits: Limits, cancel: &AtomicBool,
            watch: &Arc<Mutex<Watch>>, focus: &Arc<Mutex<String>>) -> Result<bool, String> {
    let Some(part) = &profile.validator else { return Ok(true); };
    if kill_requested(ctx, &c.id) || cancel.load(Ordering::SeqCst) { return Err("killed".to_string()); }
    if let Some(cause) = watch.lock().unwrap_or_else(|e| e.into_inner()).cause() { return Err(format!("STUCK: {}", cause)); }
    if let Some(stop) = limits.over(ctx) { return Err(stop); }
    let model = match model_for_part(ctx, part) {
        Ok(m) => m,
        Err(e) => { c.error = e; c.evidence.clear(); return Ok(false); }
    };
    c.state = "validating".to_string();
    c.updated = now();
    *focus.lock().unwrap_or_else(|e| e.into_inner()) = "validator".to_string();
    let idx = profile.workers.len();
    if let Some(p) = c.parts.get_mut(idx) { p.state = "working".to_string(); p.model = part_model_name(part, &model); }
    save(ctx, c)?;
    report(ctx, c, "validator", "");
    // The validator checks every open or confirmed note; a stale note is not evidence. The full memory.md is the
    // human log and is read only when no note exists yet (for example an older cluster from before notes).
    let notes = validator_notes_text(ctx, &c.id);
    let memory = if notes.trim().is_empty() { fs::read_to_string(memory_path(ctx, &c.id)).unwrap_or_default() } else { notes };
    let out = with_level(2, || run_model(ctx, c, part, &validator_system(c, part, &model), &validator_prompt(c, &memory), 4, cancel, limits, watch));
    let done = match out {
        Ok(d) => d,
        Err(e) => {
            if e.starts_with("BUDGET: ") || e.starts_with("STUCK: ") { return Err(e); }
            if e == "stopped" && (cancel.load(Ordering::SeqCst) || kill_requested(ctx, &c.id)) { return Err("killed".to_string()); }
            c.error = format!("validator: {}", e);
            c.evidence.clear();
            if let Some(p) = c.parts.get_mut(idx) { p.state = "failed".to_string(); p.error = e.clone(); }
            memory_line(ctx, c, part, &model, &format!("NOT checked: {}", e), "failed");
            save(ctx, c)?;
            return Ok(false);
        }
    };
    let evidence = evidence_of(&done.checks);
    let checked = done.checks.iter().any(|c| c.ok);
    let text = done.text;
    c.evidence = evidence.clone();
    let note = if evidence.is_empty() { text.clone() } else { format!("{}\n\nevidence: {}", text, evidence) };
    memory_line(ctx, c, part, &model, &note, "validated");
    let said_pass = text.trim_start().to_uppercase().starts_with("PASS");
    // A PASS counts only when the validator really ran a check that passed. `done` without V is not accepted later.
    let ok = said_pass && checked;
    // The validator owns the status of the worker notes: accepted notes become `confirmed`; a rejected result (and a
    // confirmed note that a later validation contradicts) becomes `stale` and never reaches another prompt.
    let status = if ok { crate::notes::Status::Confirmed } else { crate::notes::Status::Stale };
    for worker in profile.workers.iter().filter(|p| p.role != "human") {
        let _ = crate::notes::set_status(&notes_dir(ctx, &c.id), &note_name(worker), status);
    }
    // One result per worker part and task type, for `penelope cluster advise`.
    record_scores(ctx, profile, ok);
    if let Some(p) = c.parts.get_mut(idx) { p.state = if ok { "done" } else { "failed" }.to_string(); p.output = text.clone(); p.error = if ok { String::new() } else { text.clone() }; }
    if ok {
        c.done = (c.done + 1).min(c.total);
        c.error.clear();
    } else if said_pass {
        c.error = if done.checks.is_empty() {
            "the validator said PASS but ran no check: no V field, the cluster fails".to_string()
        } else {
            format!("the validator said PASS but its check failed: {}", evidence)
        };
    } else {
        c.error = text.lines().next().unwrap_or("validator said FAIL").chars().take(300).collect();
    }
    c.updated = now();
    save(ctx, c)?;
    Ok(ok)
}

fn cleanup_worktree(c: &mut Cluster) -> String {
    if c.worktree.is_empty() || c.project.is_empty() { return String::new(); }
    match crate::work::remove_with_branch(Path::new(&c.project), &c.id) {
        Ok(note) => {
            c.worktree.clear();
            c.branch.clear();
            c.cwd.clear();
            note
        }
        Err(e) => format!("the worktree could not be removed: {}", e),
    }
}

/// Applies the patch of the cluster's worktree to the live tree. Called only after the validator answered PASS.
fn apply_worktree(c: &Cluster) -> Result<(), String> {
    let project = PathBuf::from(&c.project);
    if !project.is_dir() { return Err(format!("the project folder {} is gone", project.display())); }
    let bytes = crate::work::patch_of(&project, &c.id)?;
    if bytes.is_empty() { return Ok(()); }
    let _guard = MERGE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    crate::work::apply_bytes(&project, &bytes).map_err(|e| {
        format!("merge conflict: the patch did not apply to the live tree ({}). Nothing was resolved automatically; the worktree {} stays for inspection.", e, c.worktree)
    })
}

/// The profile a `before` field names, checked before anything runs. It must be read-only and must not name a `before`
/// of its own: a task never travels through a hidden chain of prompt clusters.
fn before_profile(ctx: &Ctx, p: &Profile) -> Result<Option<Profile>, String> {
    let Some(name) = p.before.as_deref() else { return Ok(None) };
    if name == p.name {
        return Err(format!("profile {}: before names the profile itself. A prompt cluster must be another profile.", p.name));
    }
    let before = profile(ctx, name).map_err(|e| format!("profile {}: before {}: {}", p.name, name, e))?;
    if let Some(next) = before.before.as_deref() {
        return Err(format!("profile {}: before profile {} names its own before {}. A chain of prompt clusters is not allowed; a before cluster only rewrites the task.", p.name, before.name, next));
    }
    if let Some(w) = before.workers.iter().chain(before.validator.iter())
        .find(|w| !matches!(w.role.as_str(), "condenser" | "scout" | "judge")) {
        return Err(format!("profile {}: before profile {} has the part {} with role {}, which may write or act. A before cluster only rewrites the task; use condenser, scout or judge parts.", p.name, before.name, w.name, w.role));
    }
    if before.worktree {
        return Err(format!("profile {}: before profile {} may write (worktree true). A before cluster only rewrites the task; set worktree false.", p.name, before.name));
    }
    Ok(Some(before))
}

/// Runs the named prompt cluster on the task and returns its text. The prompt cluster is a child of `c`, so its report
/// and memory stay in the same tree. `max_depth` and `max_models` are checked before the child is created.
fn prompt_rewrite(ctx: &Ctx, c: &Cluster, p: &Profile, session_before: Option<f64>) -> Result<Option<(String, String)>, String> {
    let Some(before) = before_profile(ctx, p)? else { return Ok(None) };
    let root = root_of(ctx, c)?;
    let root_profile = profile(ctx, &root.profile)?;
    if c.depth + 1 > root_profile.max_depth {
        return Err(format!("the prompt cluster would be at depth {} but the root profile {} allows max_depth {}.",
                           c.depth + 1, root_profile.name, root_profile.max_depth));
    }
    let need = part_count(&before);
    let child = {
        let _guard = SPAWN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let used = tree_models(ctx, &root.id);
        if used + need > root_profile.max_models {
            return Err(format!("max_models: the tree of root {} already uses {} of {} parts, and the prompt profile {} needs {} more.",
                               root.id, used, root_profile.max_models, before.name, need));
        }
        let name = format!("{}-prompt", c.name);
        new_cluster(ctx, &before, &name, &c.task, Some(&c.id), Vec::new(), c.depth + 1)?
    };
    let _ = run_cluster_inner(ctx, &child.id, session_before)?;
    let finished = load(ctx, &child.id)?;
    if finished.state != "done" {
        return Err(format!("the prompt cluster {} ended {}: {}. The worker cluster did not start.", finished.id, finished.state,
                           if finished.error.is_empty() { "no error line" } else { &finished.error }));
    }
    let text = finished.parts.iter().filter(|x| x.role != "judge" && x.role != "human")
        .map(|x| x.output.trim()).filter(|x| !x.is_empty()).collect::<Vec<_>>().join("\n\n");
    if text.trim().is_empty() {
        return Err(format!("the prompt cluster {} ended done but wrote no task text.", finished.id));
    }
    // A model answer can be long. A task beyond this cap is cut with a visible note instead of filling every prompt.
    let mut text = text.trim().to_string();
    if text.chars().count() > 6000 {
        text = text.chars().take(6000).collect::<String>();
        text.push_str("\n[the prompt cluster answer was cut after 6000 characters]");
    }
    Ok(Some((text, before.name.clone())))
}

/// Runs one cluster: every worker part, then the validator. A FAIL sends the result back to the workers up to `retries`.
pub fn run_cluster(ctx: &Ctx, id: &str) -> Result<String, String> {
    run_cluster_inner(ctx, id, None)
}

/// One model call with the tool loop, for a workflow part. The part has no cluster state on disk.
pub(crate) fn run_once(ctx: &Ctx, prompt: &str, model: Value, effort: &str) -> Result<String, String> {
    let name = model.as_str().unwrap_or("");
    let (role, model) = match name {
        "main" | "weak" => ("worker".to_string(), Value::String(name.to_string())),
        "worker" | "judge" | "scout" | "router" => (name.to_string(), Value::Null),
        _ if model.is_string() => ("worker".to_string(), model),
        _ => ("worker".to_string(), json!({"model": model, "effort": effort})),
    };
    let part = Part { name: "workflow".to_string(), role, slot: "workflow".to_string(),
                      task: String::new(), task_type: String::new(), model, mcp: Vec::new(), depends_on: Vec::new(), quorum: 0 };
    let c = Cluster { id: "workflow".to_string(), name: "workflow".to_string(), profile: "workflow".to_string(),
                      task: prompt.to_string(), parent: None, after: Vec::new(), depth: 0, state: "working".to_string(),
                      done: 0, total: 1, started: now(), updated: now(), output: String::new(), error: String::new(),
                      memory: String::new(), evidence: String::new(), project: String::new(), worktree: String::new(),
                      branch: String::new(), cwd: String::new(), respawns: 0, respawned_from: String::new(),
                      quorum_agreed: 0, quorum_escalated: 0, shadow: false, shadow_of: String::new(),
                      match_rate: None, parts: Vec::new() };
    let system = "You are a worker part of a Penelope workflow. Do the task and report the evidence.";
    let limits = Limits { cluster: 0.0, cluster_before: 0.0, session: 0.0, session_before: 0.0 };
    let watch = Arc::new(Mutex::new(Watch::new(usize::MAX)));
    let done = run_model(ctx, &c, &part, system, prompt, 20, &AtomicBool::new(false), limits, &watch)?;
    Ok(done.text)
}

/// `run_cluster` with the euro baseline of the whole orchestration. Every cluster of one session therefore shares one
/// `session_budget_eur`, beside its own `budget_eur`.
fn run_cluster_inner(ctx: &Ctx, id: &str, session_before: Option<f64>) -> Result<String, String> {
    let mut c = load(ctx, id)?;
    if c.state == "killed" { return Ok(format!("cluster {} was already killed", id)); }
    let profile = profile(ctx, &c.profile)?;
    if kill_requested(ctx, &c.id) {
        let note = cleanup_worktree(&mut c);
        c.state = "killed".to_string();
        c.error = "killed by the orchestrator".to_string();
        c.updated = now();
        save(ctx, &c)?;
        report(ctx, &c, "kill", "killed");
        return Ok(format!("cluster {} killed{}", id, if note.is_empty() { String::new() } else { format!(" · {}", note) }));
    }
    if profile.worktree && (c.worktree.is_empty() || !Path::new(&c.worktree).is_dir()) {
        c.state = "blocked".to_string();
        c.error = format!("the profile {} writes, but the cluster has no worktree. Nothing was run in the live tree.", profile.name);
        c.updated = now();
        save(ctx, &c)?;
        report(ctx, &c, "worktree", &c.error);
        return Ok(format!("cluster {} blocked: {}", id, c.error));
    }
    // A `before` profile rewrites the task before any worker of this cluster starts. The rewrite runs as a child cluster
    // with its own report; a failed rewrite blocks this cluster instead of starting the workers with the old task.
    let rewrite = match prompt_rewrite(ctx, &c, &profile, session_before) {
        Ok(Some((task, before))) => {
            let original = c.task.clone();
            c.task = task;
            let dir = cluster_dir(ctx, &c.id);
            let _ = fs::create_dir_all(&dir);
            let _ = fs::write(dir.join("prompt.txt"),
                format!("before: {}\n\noriginal task:\n{}\n\nrewritten task:\n{}\n", before, original, c.task));
            Some((before, original))
        }
        Ok(None) => None,
        Err(e) => {
            c.state = "blocked".to_string();
            c.error = format!("the task was not rewritten: {}", e);
            c.updated = now();
            save(ctx, &c)?;
            report(ctx, &c, "before", &c.error);
            return Ok(format!("cluster {} blocked: {}", id, c.error));
        }
    };
    let cancel = Arc::new(AtomicBool::new(false));
    register_cancel(&c.id, &cancel);
    let _cancel_guard = CancelGuard(c.id.clone());
    let _kill_watch = KillWatch::start(ctx, &c.id, cancel.clone());
    c.started = if c.started == 0 { now() } else { c.started };
    c.state = "working".to_string();
    c.done = 0;
    c.quorum_agreed = 0;
    c.quorum_escalated = 0;
    // A profile with one worker and no retry has no second round. Its validator can only report, not correct, so the
    // result goes to the one model; a caller that wants the validator sets retries to 1. This is the switch from Paper 35
    // for a sequential task.
    let single_model = profile.single_model && profile.workers.len() == 1 && profile.retries == 0 && profile.validator.is_some();
    c.total = profile.workers.len() + if single_model { 0 } else { profile.validator.iter().count() };
    c.error.clear();
    c.evidence.clear();
    c.updated = now();
    save(ctx, &c)?;
    let _ = fs::write(memory_path(ctx, &c.id), format!("# Cluster {} · {}\n\nTask: {}\nProfile: {} · max {} parts, depth {}\n",
                                                         c.name, c.profile, c.task, c.profile, profile.max_models, profile.max_depth));
    if let Some((before, _)) = &rewrite {
        append_memory(ctx, &c, &format!("\n## before prompt · {}\n\nthe task was rewritten by the {} cluster before the worker parts started. The original and the rewritten text are in prompt.txt.\n", before, before));
    }
    if !c.worktree.is_empty() { append_memory(ctx, &c, &format!("\nWorktree: {} · branch {}\n", c.worktree, c.branch)); }
    report(ctx, &c, "start", "");
    let before = crate::cost::total_eur(ctx);
    let limits = Limits { cluster: profile.budget_eur, cluster_before: before,
                          session: crate::cost::session_budget(ctx), session_before: session_before.unwrap_or(before) };
    let watch = Arc::new(Mutex::new(Watch::new(profile.stuck_after)));
    let focus = Arc::new(Mutex::new(String::new()));
    let _zombie = ZombieThread::start(ctx, &c.id, profile.report_every, watch.clone(), focus.clone(), cancel.clone());
    let mut feedback = String::new();
    let mut ok = false;
    let mut workers_ok = true;
    for attempt in 0..=profile.retries {
        if kill_requested(ctx, &c.id) || cancel.load(Ordering::SeqCst) {
            let note = cleanup_worktree(&mut c);
            c.state = "killed".to_string();
            c.error = "killed by the orchestrator".to_string();
            c.updated = now();
            save(ctx, &c)?;
            report(ctx, &c, "kill", "killed");
            return Ok(format!("cluster {} killed{}", id, if note.is_empty() { String::new() } else { format!(" · {}", note) }));
        }
        workers_ok = match run_workers(ctx, &mut c, &profile, &feedback, limits, &cancel, &watch, &focus) {
            Ok(v) => v,
            Err(e) if e.starts_with("STUCK: ") => {
                let cause = e.trim_start_matches("STUCK: ").to_string();
                return stop_stuck(ctx, &mut c, &cause);
            }
            Err(e) if e == "killed" => {
                let note = cleanup_worktree(&mut c);
                c.state = "killed".to_string(); c.error = "killed by the orchestrator".to_string(); c.updated = now(); save(ctx, &c)?;
                return Ok(format!("cluster {} killed{}", id, if note.is_empty() { String::new() } else { format!(" · {}", note) }));
            }
            Err(e) => { c.state = "blocked".to_string(); c.error = e; c.updated = now(); save(ctx, &c)?; return Ok(format!("cluster {} blocked: {}", id, c.error)); }
        };
        if !workers_ok { break; }
        if single_model { ok = workers_ok; break; }
        if profile.validator.is_none() { ok = true; break; }
        match validate(ctx, &mut c, &profile, limits, &cancel, &watch, &focus) {
            Ok(true) => { ok = true; break; }
            Ok(false) => {
                feedback = c.error.clone();
                c.done = 0;
                if attempt < profile.retries {
                    let note = format!("\n## validator · retry {}\n\n{}\n", attempt + 1, feedback);
                    let _ = fs::OpenOptions::new().create(true).append(true).open(memory_path(ctx, &c.id)).and_then(|mut f| std::io::Write::write_all(&mut f, note.as_bytes()));
                }
            }
            Err(e) if e.starts_with("STUCK: ") => {
                let cause = e.trim_start_matches("STUCK: ").to_string();
                return stop_stuck(ctx, &mut c, &cause);
            }
            Err(e) if e == "killed" => {
                let note = cleanup_worktree(&mut c);
                c.state = "killed".to_string(); c.error = "killed by the orchestrator".to_string(); c.updated = now(); save(ctx, &c)?;
                return Ok(format!("cluster {} killed{}", id, if note.is_empty() { String::new() } else { format!(" · {}", note) }));
            }
            Err(e) => { c.state = "blocked".to_string(); c.error = e; c.updated = now(); save(ctx, &c)?; return Ok(format!("cluster {} blocked: {}", id, c.error)); }
        }
    }
    let mut blocked = false;
    if ok {
        if let Some(stop) = limits.over(ctx) {
            ok = false;
            blocked = true;
            c.error = stop;
        }
    }
    if ok && profile.worktree {
        if c.shadow {
            // A shadow is a measurement, not a second writer. Its worktree stays for inspection and its patch never
            // reaches the live tree, not even after a PASS.
            append_memory(ctx, &c, &format!("\n## shadow\n\nworktree {} was not merged into {}. A shadow only measures the main run.\n", c.worktree, c.project));
        } else if profile.validator.is_none() {
            ok = false;
            blocked = true;
            c.error = "the worktree was not applied: a profile without a validator cannot answer PASS".to_string();
        } else if let Err(e) = apply_worktree(&c) {
            ok = false;
            blocked = true;
            c.error = e;
        } else {
            append_memory(ctx, &c, &format!("\n## merge\n\nworktree {} applied to {}\n", c.worktree, c.project));
        }
    }
    if ok {
        // The orchestrator accepts only a done line with a non-empty V. The check is the same proof::accept used by tests.
        let done = crate::proof::Report {
            s: "done".to_string(), p: format!("{}/{}", c.done, c.total), t: format!("{}s", now().saturating_sub(c.started)),
            f: "-".to_string(), e: "-".to_string(),
            v: if c.evidence.trim().is_empty() { None } else { Some(c.evidence.clone()) },
        };
        if let Err(e) = crate::proof::accept(&done) {
            ok = false;
            c.error = e;
        }
    }
    c.state = if ok { "done" } else if blocked { "blocked" } else { "failed" }.to_string();
    if !ok && c.error.is_empty() {
        c.error = if workers_ok { "the validator did not pass the cluster".to_string() } else { "at least one part failed".to_string() };
    }
    c.updated = now();
    save(ctx, &c)?;
    report(ctx, &c, "end", &c.error);
    Ok(format!("cluster {} {} · {}/{} parts · {:.4} EUR", id, c.state, c.done, c.total, budget_spent(ctx, before)))
}

fn memory_log_path(ctx: &Ctx, id: &str) -> PathBuf {
    cluster_dir(ctx, id).join("memory.log")
}

/// Appends one event to memory.md (the prompt view) and to memory.log (the append-only event log). `level` is the
/// origin level L: 0 for a file, 1 for an agent, 2 for a checked tool, 3 for a person. The weakest level of a derived
/// claim is passed by the caller; the default is 1.
fn mem_put(ctx: &Ctx, c: &Cluster, text: &str, level: u8) -> Result<(), String> {
    let _guard = MEMORY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    fs::OpenOptions::new().create(true).append(true).open(memory_path(ctx, &c.id))
        .and_then(|mut f| f.write_all(text.as_bytes()))
        .map_err(|e| format!("cannot write memory.md: {}", e))?;
    let event = serde_json::json!({"t": now(), "C": c.id, "L": level.min(3), "text": text});
    fs::OpenOptions::new().create(true).append(true).open(memory_log_path(ctx, &c.id))
        .and_then(|mut f| writeln!(f, "{}", event))
        .map_err(|e| format!("cannot write memory.log: {}", e))
}

fn append_memory(ctx: &Ctx, c: &Cluster, text: &str) {
    let _ = mem_put(ctx, c, text, 1);
}

fn new_cluster(ctx: &Ctx, profile: &Profile, name: &str, task: &str, parent: Option<&str>, after: Vec<String>, depth: usize) -> Result<Cluster, String> {
    // Two clusters or a respawn may start in the same millisecond; the file name must stay unique.
    let mut id = format!("{}-{}", safe(name), stamp());
    let mut suffix = 1;
    while cluster_file(ctx, &id).is_file() {
        id = format!("{}-{}-{}", safe(name), stamp(), suffix);
        suffix += 1;
    }
    let parts: Vec<PartRun> = profile.workers.iter().map(|p| PartRun { name: p.name.clone(), slot: p.slot.clone(), role: p.role.clone(), model: String::new(), state: "queued".into(), output: String::new(), error: String::new() })
        .chain(profile.validator.iter().map(|p| PartRun { name: p.name.clone(), slot: p.slot.clone(), role: p.role.clone(), model: String::new(), state: "queued".into(), output: String::new(), error: String::new() }))
        .collect();
    // The project is the folder above `.penelope`. A profile that may write needs a git worktree: without git the
    // cluster is refused here, so a coding part can never fall back to the live tree.
    let root = ctx.root.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| ctx.root.clone());
    let (project, worktree, branch, cwd) = if profile.worktree {
        let project = crate::work::project(&root.to_string_lossy()).map_err(|e| {
            format!("NOT created. cluster {} needs a git worktree, but {} is no git project: {}", name, root.display(), e)
        })?;
        let (path, branch) = crate::work::create(&project, &id).map_err(|e| {
            format!("NOT created. cluster {} needs a git worktree: {}", name, e)
        })?;
        let path = path.to_string_lossy().to_string();
        (project.to_string_lossy().to_string(), path.clone(), branch, path)
    } else {
        (String::new(), String::new(), String::new(), String::new())
    };
    let c = Cluster { id: id.clone(), name: name.to_string(), profile: profile.name.clone(), task: task.to_string(),
                      parent: parent.map(str::to_string), after, depth, state: "queued".to_string(), done: 0,
                      total: parts.len(), started: now(), updated: now(), output: String::new(), error: String::new(),
                      memory: memory_path(ctx, &id).to_string_lossy().to_string(), evidence: String::new(),
                      project, worktree, branch, cwd, respawns: 0, respawned_from: String::new(),
                      quorum_agreed: 0, quorum_escalated: 0, shadow: false, shadow_of: String::new(),
                      match_rate: None, parts };
    save(ctx, &c)?;
    let _ = fs::write(memory_path(ctx, &id), format!("# Cluster {} · {}\n\nTask: {}\n\n", name, profile.name, task));
    Ok(c)
}

/// Follows `parent` links up to the cluster of the whole tree. The root profile owns `max_depth` and `max_models`.
fn root_of(ctx: &Ctx, c: &Cluster) -> Result<Cluster, String> {
    let mut current = c.clone();
    for _ in 0..32 {
        let Some(parent) = current.parent.clone() else { return Ok(current); };
        current = load(ctx, &parent)?;
    }
    Err(format!("cluster {} has a parent chain deeper than 32", c.id))
}

/// Parts of every cluster in one tree, including clusters that already finished. A spawned child stays part of the
/// tree, so the root profile's `max_models` cannot be exceeded by a series of small spawns.
fn tree_models(ctx: &Ctx, root_id: &str) -> usize {
    let all = list(ctx);
    let mut ids = vec![root_id.to_string()];
    let mut i = 0;
    while i < ids.len() {
        let current = ids[i].clone();
        for c in &all {
            if c.parent.as_deref() == Some(current.as_str()) && !ids.contains(&c.id) {
                ids.push(c.id.clone());
            }
        }
        i += 1;
    }
    let mut total = 0;
    let mut found_root = false;
    for c in &all {
        if ids.contains(&c.id) {
            total += c.total;
            if c.id == root_id { found_root = true; }
        }
    }
    if !found_root {
        if let Ok(root) = load(ctx, root_id) { total += root.total; }
    }
    total
}

fn part_count(p: &Profile) -> usize {
    p.workers.len() + p.validator.iter().count()
}

/// Checks the limits of the whole tree and creates a child without running it. `spawn_inner` runs it; the tests use
/// this half to prove a refusal without a model call.
fn spawn_create(ctx: &Ctx, parent_id: &str, profile_name: &str, name: Option<&str>, task: &str) -> Result<Cluster, String> {
    let parent = load(ctx, parent_id)?;
    let root = root_of(ctx, &parent)?;
    let root_profile = profile(ctx, &root.profile)?;
    let next_depth = parent.depth + 1;
    if next_depth > root_profile.max_depth {
        return Err(format!("NOT spawned. A part at depth {} of {} cannot start a subcluster: the root profile {} allows max_depth {}. \
The child would be at depth {}.", parent.depth, root_profile.max_depth, root_profile.name, root_profile.max_depth, next_depth));
    }
    if task.trim().is_empty() {
        return Err("NOT spawned. The subcluster task is empty, so the child would not know what to do.".to_string());
    }
    let child_profile = profile(ctx, profile_name)?;
    let name = name.map(str::trim).filter(|n| !n.is_empty()).unwrap_or(&child_profile.name).to_string();
    let need = part_count(&child_profile);
    let _guard = SPAWN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let used = tree_models(ctx, &root.id);
    if used + need > root_profile.max_models {
        return Err(format!("NOT spawned. max_models: the tree of root {} already uses {} of {} parts, and profile {} needs {} more. \
Raise max_models of {} or reuse the clusters that are already in the tree.", root.id, used, root_profile.max_models,
                            child_profile.name, need, root_profile.name));
    }
    new_cluster(ctx, &child_profile, &name, task.trim(), Some(&parent.id), Vec::new(), next_depth)
}

/// Writes one child report line into the parent's memory, children log and report stream. The orchestrator reads the
/// tree through the parent; the child never reports past it.
fn record_child(ctx: &Ctx, parent: &Cluster, child: &Cluster) -> String {
    let report = fs::read_to_string(cluster_dir(ctx, &child.id).join("report.txt")).unwrap_or_default();
    let line = report.lines().next().unwrap_or("").trim().to_string();
    let entry = format!("\n## child {} · {}\n\nid: {} · parent: {} · depth: {}\nstate: {} · P:{}/{}\nreport: {}\n",
                        child.name, child.profile, child.id, parent.id, child.depth, child.state, child.done, child.total,
                        if line.is_empty() { "(no report)" } else { &line });
    append_memory(ctx, parent, &entry);
    let dir = cluster_dir(ctx, &parent.id);
    let _ = fs::create_dir_all(&dir);
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(dir.join("children.jsonl")) {
        let _ = writeln!(file, "{}", json!({"id": child.id, "name": child.name, "profile": child.profile,
                                            "parent": parent.id, "depth": child.depth, "state": child.state,
                                            "report": line}));
    }
    let _guard = REPORT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(dir.join("reports.jsonl")) {
        let _ = writeln!(file, "{} CHILD {} {}", now(), child.id, line);
    }
    line
}

/// Creates a subcluster of the running cluster, runs it, and gives its report line to the parent. The tool calls this
/// with the id of the part's own cluster; a part outside a cluster gets a refusal.
fn spawn_inner(ctx: &Ctx, parent_id: &str, profile_name: &str, name: Option<&str>, task: &str) -> Result<String, String> {
    let parent = load(ctx, parent_id)?;
    let child = spawn_create(ctx, parent_id, profile_name, name, task)?;
    let run = run_cluster_inner(ctx, &child.id, None);
    let finished = load(ctx, &child.id).unwrap_or_else(|_| child.clone());
    let line = record_child(ctx, &parent, &finished);
    match run {
        Ok(_) => Ok(format!("cluster {} spawned as {} · parent {} · depth {} · state {}\nreport: {}",
                            child.name, child.id, parent.id, child.depth, finished.state,
                            if line.is_empty() { "(no report)" } else { &line })),
        Err(e) => Err(format!("cluster {} was created as {} at depth {} but did not finish: {}",
                              child.name, child.id, child.depth, e)),
    }
}

/// The tool `cluster_spawn`: only a part of a running cluster may start a subcluster.
pub fn spawn(ctx: &Ctx, profile_name: &str, name: Option<&str>, task: &str) -> Result<String, String> {
    let parent = current_cluster().ok_or_else(|| {
        "NOT spawned. cluster_spawn belongs to a part inside a running cluster. Use cluster_start for a new orchestration.".to_string()
    })?;
    spawn_inner(ctx, &parent, profile_name, name, task)
}

#[derive(Clone, Debug)]
pub struct PlanCluster {
    pub name: String,
    pub profile: String,
    pub task: String,
    pub after: Vec<String>,
    pub parent: Option<String>,
}

pub fn parse_plan(text: &str, ctx: &Ctx) -> Result<Vec<PlanCluster>, String> {
    let start = text.find('{').ok_or_else(|| "the orchestrator wrote no JSON".to_string())?;
    let end = text.rfind('}').ok_or_else(|| "the orchestrator wrote no closed JSON".to_string())?;
    let v: Value = serde_json::from_str(&text[start..=end]).map_err(|e| format!("the orchestrator wrote no valid JSON: {}", e))?;
    let clusters = v["clusters"].as_array().ok_or_else(|| "the orchestrator wrote no clusters list".to_string())?;
    if clusters.is_empty() || clusters.len() > 4 { return Err("the orchestrator must name 1 to 4 clusters".to_string()); }
    let names = profile_names(ctx);
    let mut out: Vec<PlanCluster> = Vec::new();
    for (i, c) in clusters.iter().enumerate() {
        let profile = c["profile"].as_str().unwrap_or("").trim();
        if !names.iter().any(|n| n == profile) { return Err(format!("unknown cluster profile {}", if profile.is_empty() { "(empty)" } else { profile })); }
        let name = c["name"].as_str().unwrap_or("").trim();
        let name = if name.is_empty() { format!("cluster{}", i + 1) } else { name.to_string() };
        if out.iter().any(|x| x.name == name) { return Err(format!("cluster name {} is used twice", name)); }
        let task = c["task"].as_str().unwrap_or("").trim();
        if task.is_empty() { return Err(format!("cluster {} has no task", name)); }
        let after: Vec<String> = c["after"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default();
        if after.iter().any(|a| !clusters.iter().any(|x| x["name"].as_str() == Some(a.as_str()))) {
            return Err(format!("cluster {} waits for a name that is not in the plan", name));
        }
        out.push(PlanCluster { name, profile: profile.to_string(), task: task.to_string(), after,
                               parent: c["parent"].as_str().filter(|s| !s.is_empty()).map(str::to_string) });
    }
    // A cycle in `after` would leave every cluster waiting.
    let mut remaining: Vec<&str> = out.iter().map(|c| c.name.as_str()).collect();
    let mut removed: Vec<&str> = Vec::new();
    while !remaining.is_empty() {
        let ready: Vec<&str> = remaining.iter().copied()
            .filter(|name| out.iter().find(|c| c.name == *name).map(|c| c.after.iter().all(|a| removed.contains(&a.as_str()) || !out.iter().any(|x| x.name == *a))).unwrap_or(false))
            .collect();
        if ready.is_empty() { return Err("the orchestrator plan has a cycle in after".to_string()); }
        for name in &ready { removed.push(name); }
        remaining.retain(|n| !ready.contains(n));
    }
    Ok(out)
}

fn plan(ctx: &Ctx, goal: &str, hint: Option<&str>) -> Vec<PlanCluster> {
    if let Some(profile) = hint {
        return vec![PlanCluster { name: profile.to_string(), profile: profile.to_string(), task: goal.to_string(), after: Vec::new(), parent: None }];
    }
    let model = crate::route::model_for(ctx, "router").or_else(|| Some(&ctx.cfg["weak"])).cloned();
    if let Some(model) = model.filter(|m| m["model"].is_string()) {
        let system = "You are the orchestrator of Penelope, a cluster framework. Split the task into 1 to 4 clusters. \\
A cluster is a group of parts with one job, not a subagent. Available profiles: coding, backend, frontend, live-test, research, solo. \\
Answer ONLY one JSON object: {\"clusters\":[{\"name\":\"backend\",\"profile\":\"backend\",\"task\":\"one concrete job\",\"after\":[]}]}. \\
Use after names when one cluster needs another (a live-test cluster comes after coding). Keep names short and unique.";
        let user = format!("Task: {}\nProfiles: {}\n", goal, profile_names(ctx).join(", "));
        if let Ok((answer, tin, tout)) = crate::weak::ask_count(&model, system, &user) {
            crate::cost::record(ctx, "orchestrator", model["model"].as_str().unwrap_or(""), tin, tout);
            if let Ok(plan) = parse_plan(&answer, ctx) { return plan; }
        }
    }
    vec![PlanCluster { name: "coding".to_string(), profile: "coding".to_string(), task: goal.to_string(), after: Vec::new(), parent: None }]
}

fn create_plan(ctx: &Ctx, plan: &[PlanCluster]) -> Result<Vec<(String, PlanCluster)>, String> {
    let mut created: Vec<(String, PlanCluster)> = Vec::new();
    let mut pending: Vec<PlanCluster> = plan.to_vec();
    let mut guard = 0;
    while !pending.is_empty() && guard < 12 {
        guard += 1;
        let mut progressed = false;
        for c in pending.clone() {
            if let Some(parent_name) = &c.parent {
                let Some((parent_id, _)) = created.iter().find(|(_, x)| x.name == *parent_name) else { continue; };
                let parent = load(ctx, parent_id)?;
                let parent_profile = profile(ctx, &parent.profile)?;
                if parent.depth + 1 > parent_profile.max_depth {
                    return Err(format!("cluster {} would be deeper than the {} profile allows", c.name, parent.profile));
                }
                let p = profile(ctx, &c.profile)?;
                let child = new_cluster(ctx, &p, &c.name, &c.task, Some(parent_id), Vec::new(), parent.depth + 1)?;
                created.push((child.id, c.clone()));
                pending.retain(|x| x.name != c.name);
                progressed = true;
            } else {
                let p = profile(ctx, &c.profile)?;
                let child = new_cluster(ctx, &p, &c.name, &c.task, None, Vec::new(), 0)?;
                created.push((child.id, c.clone()));
                pending.retain(|x| x.name != c.name);
                progressed = true;
            }
        }
        if !progressed {
            return Err("a cluster names a parent that is not in the plan".to_string());
        }
    }
    // `after` is stored as ids, so the run can look at the state of a dependency, not only at its name.
    for (id, plan_cluster) in &created {
        let mut child = load(ctx, id)?;
        child.after = plan_cluster.after.iter().filter_map(|name| created.iter().find(|(_, x)| x.name == *name).map(|(id, _)| id.clone())).collect();
        // A subcluster waits for its parent: the parent owns the task the child is part of.
        if let Some(parent) = &plan_cluster.parent {
            if let Some((parent_id, _)) = created.iter().find(|(_, x)| x.name == *parent) {
                if !child.after.contains(parent_id) { child.after.push(parent_id.clone()); }
            }
        }
        save(ctx, &child)?;
    }
    Ok(created)
}

/// The orchestrator: make a plan, create the clusters and run them, respecting `after` and `parent`.
pub fn orchestrate(ctx: &Ctx, goal: &str, hint: Option<&str>, parallel: bool) -> Result<String, String> {
    orchestrate_with(ctx, goal, hint, parallel, None)
}

/// `orchestrate` with an optional shadow profile. The shadow runs after the main clusters, on the same task, in its own
/// worktree. Its result is compared with the main run and its files are never merged.
pub fn orchestrate_with(ctx: &Ctx, goal: &str, hint: Option<&str>, parallel: bool, shadow: Option<&str>) -> Result<String, String> {
    if goal.trim().is_empty() { return Err("NOT started. The cluster goal is empty.".to_string()); }
    let plan = plan(ctx, goal.trim(), hint);
    // A bad explicit shadow is refused before a cluster file is written or a model is called.
    if let Some(name) = shadow.map(str::trim).filter(|s| !s.is_empty()) {
        profile(ctx, name).map_err(|e| format!("--shadow {}: {}", name, e))?;
        if plan.iter().any(|p| p.parent.is_none() && p.profile == name) {
            return Err(format!("--shadow {} is the main profile itself; name another profile.", name));
        }
    }
    let created = create_plan(ctx, &plan)?;
    // Resolve the shadow pairs before the first model call, so a bad --shadow name costs nothing. A profile that names
    // `shadow_of: P` runs beside every top-level plan cluster with profile P.
    let explicit_target = created.iter().find(|(_, p)| p.parent.is_none()).map(|(id, _)| id.clone());
    let mut shadow_plan: Vec<(String, Vec<String>)> = Vec::new();
    for (id, p) in &created {
        if p.parent.is_some() { continue; }
        let explicit_for = if Some(id) == explicit_target.as_ref() { shadow } else { None };
        let targets = shadow_targets(ctx, &p.profile, explicit_for)?;
        if !targets.is_empty() { shadow_plan.push((id.clone(), targets)); }
    }
    let mut done: Vec<String> = Vec::new();
    let mut pending: Vec<(String, PlanCluster)> = created.clone();
    // One euro baseline for the whole orchestration: every cluster also answers to the session budget.
    let session_before = crate::cost::total_eur(ctx);
    let mut guard = 0;
    while !pending.is_empty() && guard < 8 {
        guard += 1;
        let ready: Vec<(String, PlanCluster)> = pending.iter().filter(|(_, p)| p.after.iter().all(|a| done.contains(a))).cloned().collect();
        if ready.is_empty() { break; }
        if parallel && ready.len() > 1 {
            let mut handles = Vec::new();
            for (id, _) in &ready {
                let ctx = ctx.clone(); let id = id.clone();
                handles.push(std::thread::spawn(move || { let _ = run_cluster_inner(&ctx, &id, Some(session_before)); }));
            }
            for h in handles { let _ = h.join(); }
        } else {
            for (id, _) in &ready { let _ = run_cluster_inner(ctx, id, Some(session_before)); }
        }
        // A zombie is dumped and started again with the cause of its dump, at most twice. Other clusters wait for the
        // name, so the respawn finishes inside this step.
        for (id, _) in &ready {
            let mut victim = id.clone();
            for _ in 0..2 {
                let Ok(current) = load(ctx, &victim) else { break };
                if current.state != "stuck" || current.respawns >= 2 { break; }
                match respawn_inner(ctx, &victim, "", Some(session_before)) {
                    Ok((next, _)) => victim = next,
                    Err(_) => break,
                }
            }
        }
        for (_, p) in &ready { done.push(p.name.clone()); }
        pending.retain(|(_, p)| !ready.iter().any(|(_, r)| r.name == p.name));
    }
    let mut shadow_lines = Vec::new();
    for (main_id, targets) in shadow_plan {
        for name in targets {
            match run_shadow(ctx, &main_id, &name, Some(session_before)) {
                Ok((run, _)) => shadow_lines.push(run.line()),
                Err(e) => shadow_lines.push(format!("SHADOW {} beside {}: NOT run: {}", name, main_id, e)),
            }
        }
    }
    let ids: Vec<String> = created.iter().map(|(id, _)| id.clone()).collect();
    let summary = status_text(ctx, None);
    let shadows = if shadow_lines.is_empty() { String::new() } else { format!("\n\n{}", shadow_lines.join("\n")) };
    Ok(format!("{} clusters started: {}\n\n{}{}", created.len(), ids.join(", "), summary, shadows))
}

fn shadow_path(ctx: &Ctx) -> PathBuf {
    base(ctx).join("shadow.json")
}

/// A role that may change files. A shadow with such a role always gets a worktree, even if its profile says otherwise,
/// because a shadow must never reach the live tree.
fn writes_files(p: &Profile) -> bool {
    p.workers.iter().chain(p.validator.iter()).any(|w| matches!(w.role.as_str(), "worker" | "lead"))
}

/// Profiles that name `main` in `shadow_of`. A malformed profile file is ignored; it is not a reason to stop a run.
fn declared_shadows(ctx: &Ctx, main: &str) -> Vec<String> {
    let mut out = Vec::new();
    for name in profile_names(ctx) {
        if name == main { continue; }
        if let Ok(p) = profile(ctx, &name) {
            if p.shadow_of.as_deref() == Some(main) { out.push(name); }
        }
    }
    out
}

/// The explicit `--shadow` profile plus every profile declared for this main profile.
fn shadow_targets(ctx: &Ctx, main: &str, explicit: Option<&str>) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    if let Some(name) = explicit.map(str::trim).filter(|s| !s.is_empty()) {
        if name == main { return Err(format!("--shadow {} is the main profile itself; name another profile.", name)); }
        profile(ctx, name).map_err(|e| format!("--shadow {}: {}", name, e))?;
        out.push(name.to_string());
    }
    for name in declared_shadows(ctx, main) {
        if !out.iter().any(|known| known == &name) { out.push(name); }
    }
    Ok(out)
}

/// One comparison of a main cluster and its shadow. Outputs are compared worker by worker; checks compare the verdict
/// and, when one side has evidence, the evidence line.
#[derive(Clone, Debug)]
struct ShadowRun {
    main: String,
    shadow: String,
    main_profile: String,
    shadow_profile: String,
    main_state: String,
    shadow_state: String,
    output_matched: usize,
    output_total: usize,
    check_matched: usize,
    check_total: usize,
    match_rate: f64,
}

impl ShadowRun {
    fn value(&self) -> Value {
        json!({
            "main": self.main, "shadow": self.shadow,
            "main_profile": self.main_profile, "shadow_profile": self.shadow_profile,
            "main_state": self.main_state, "shadow_state": self.shadow_state,
            "outputs": {"matched": self.output_matched, "total": self.output_total},
            "checks": {"matched": self.check_matched, "total": self.check_total},
            "match_rate": self.match_rate, "at": now()
        })
    }

    fn line(&self) -> String {
        format!("SHADOW {} · {} · match {:.1}% · outputs {}/{} · checks {}/{} · files not merged",
                self.shadow, self.shadow_profile, self.match_rate * 100.0,
                self.output_matched, self.output_total, self.check_matched, self.check_total)
    }
}

/// The worker outputs in profile order. The validator is left out; it is compared through the verdict and evidence.
fn compared_outputs(ctx: &Ctx, c: &Cluster) -> Vec<String> {
    let count = profile(ctx, &c.profile).map(|p| p.workers.len()).unwrap_or(c.parts.len());
    c.parts.iter().take(count).filter(|p| p.role != "human").map(|p| p.output.clone()).collect()
}

fn compare_shadows(ctx: &Ctx, main: &Cluster, shadow: &Cluster) -> ShadowRun {
    let first = compared_outputs(ctx, main);
    let second = compared_outputs(ctx, shadow);
    let output_total = first.len().max(second.len());
    let output_matched = (0..output_total).filter(|i| {
        i < &first.len() && i < &second.len() && answers_agree(&first[*i], &second[*i])
    }).count();
    let mut check_total = 1;
    let mut check_matched = usize::from(main.state == shadow.state);
    if !main.evidence.trim().is_empty() || !shadow.evidence.trim().is_empty() {
        check_total += 1;
        if normalized_answer(&main.evidence) == normalized_answer(&shadow.evidence) { check_matched += 1; }
    }
    let total = output_total + check_total;
    let matched = output_matched + check_matched;
    ShadowRun {
        main: main.id.clone(), shadow: shadow.id.clone(),
        main_profile: main.profile.clone(), shadow_profile: shadow.profile.clone(),
        main_state: main.state.clone(), shadow_state: shadow.state.clone(),
        output_matched, output_total, check_matched, check_total,
        match_rate: if total == 0 { 0.0 } else { matched as f64 / total as f64 },
    }
}

fn shadow_detail(run: &ShadowRun) -> String {
    format!("Shadow run\n\nmain: {} · state {}\nshadow: {} · state {}\nmatch rate: {:.1}% ({}/{} comparisons)\noutputs: {}/{}\nchecks: {}/{}\n\nThe shadow worked in its own worktree. Its files were not merged into the live tree.\n",
            run.main, run.main_state, run.shadow, run.shadow_state, run.match_rate * 100.0,
            run.output_matched + run.check_matched, run.output_total + run.check_total,
            run.output_matched, run.output_total, run.check_matched, run.check_total)
}

/// Appends one run to `.penelope/clusters/shadow.json`, the evidence the owner reads across projects.
fn record_shadow(ctx: &Ctx, run: &ShadowRun) -> Result<(), String> {
    let _guard = SHADOW_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let path = shadow_path(ctx);
    let mut table = fs::read_to_string(&path).ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({"runs": []}));
    if !table["runs"].is_array() { table["runs"] = json!([]); }
    table["updated"] = json!(now());
    if let Some(runs) = table["runs"].as_array_mut() {
        runs.push(run.value());
        while runs.len() > 100 { runs.remove(0); }
    }
    fs::create_dir_all(base(ctx)).map_err(|e| format!("cannot make {}: {}", base(ctx).display(), e))?;
    fs::write(&path, serde_json::to_string_pretty(&table).unwrap())
        .map_err(|e| format!("cannot write {}: {}", path.display(), e))
}

/// Runs one shadow profile beside a finished main cluster, compares the two and records the match rate. The shadow is
/// a top-level sibling, not a child: it does not change the main tree's max_models and it never merges a patch.
fn run_shadow(ctx: &Ctx, main_id: &str, shadow_name: &str, session_before: Option<f64>) -> Result<(ShadowRun, Cluster), String> {
    let main = load(ctx, main_id)?;
    let mut p = profile(ctx, shadow_name)?;
    if writes_files(&p) { p.worktree = true; }
    let label = format!("{}-shadow-{}", main.name, shadow_name);
    let mut s = new_cluster(ctx, &p, &label, &main.task, None, Vec::new(), 0)?;
    s.shadow = true;
    s.shadow_of = main.id.clone();
    save(ctx, &s)?;
    let _ = run_cluster_inner(ctx, &s.id, session_before);
    let s = load(ctx, &s.id)?;
    let main = load(ctx, main_id)?;
    let run = compare_shadows(ctx, &main, &s);
    record_shadow(ctx, &run)?;
    let mut main = main;
    main.match_rate = Some(run.match_rate);
    save(ctx, &main)?;
    let mut shadow = s;
    shadow.match_rate = Some(run.match_rate);
    save(ctx, &shadow)?;
    let detail = shadow_detail(&run);
    let dir = cluster_dir(ctx, &main.id);
    let _ = fs::create_dir_all(&dir);
    let _ = fs::write(dir.join("shadow.txt"), &detail);
    append_memory(ctx, &main, &format!("\n## shadow · {}\n\n{}", shadow.name, detail));
    append_memory(ctx, &shadow, &format!("\n## shadow match\n\n{}", detail));
    stream_note(ctx, &main.id, &run.line());
    stream_note(ctx, &shadow.id, &run.line());
    Ok((run, shadow))
}


/// The parts a name refers to: a cluster id or name, a profile, or one part name. Used by `cluster advise NAME`.
fn part_names_for(ctx: &Ctx, name: &str) -> Vec<String> {
    let cluster = load(ctx, name).ok().or_else(|| list(ctx).into_iter().find(|c| c.name == name));
    if let Some(c) = cluster {
        let mut names: Vec<String> = c.parts.iter().map(|p| p.name.clone()).collect();
        if let Ok(p) = profile(ctx, &c.profile) {
            names.extend(p.workers.iter().map(|w| w.name.clone()));
            names.extend(p.validator.iter().map(|v| v.name.clone()));
        }
        return names;
    }
    if let Ok(p) = profile(ctx, name) {
        return p.workers.iter().map(|w| w.name.clone()).chain(p.validator.iter().map(|v| v.name.clone())).collect();
    }
    vec![name.to_string()]
}

/// `penelope cluster advise [NAME]`: one sentence per part and task type that the validator rejected often enough.
/// Without a name every pair is shown; a name narrows it to a cluster, a profile or one part. Worst share first.
pub fn advise_text(ctx: &Ctx, name: Option<&str>) -> Result<String, String> {
    let path = scores_path(ctx);
    if !path.exists() {
        return Ok(format!("no cluster scores yet. A validator writes them to {} after a cluster run.\n", path.display()));
    }
    let table = crate::advise::load(&path)?;
    let all = crate::advise::suggestions(&table);
    let filter = name.map(str::trim).filter(|n| !n.is_empty());
    let picked: Vec<&crate::advise::Suggestion> = match filter {
        None => all.iter().collect(),
        Some(n) => {
            let names = part_names_for(ctx, n);
            all.iter().filter(|s| names.iter().any(|known| known == &s.part)).collect()
        }
    };
    if picked.is_empty() {
        let scope = filter.map(|n| format!(" for {}", n)).unwrap_or_default();
        return Ok(format!("no suggestion{}: a pair needs at least {} results and {} % rejected.\n",
                           scope, crate::advise::MIN_RESULTS, crate::advise::MIN_REJECTED_PERCENT));
    }
    let mut out = String::new();
    for suggestion in picked {
        let mut line = suggestion.sentence();
        if let Some(best) = crate::advise::best_model(&table, &suggestion.part, &suggestion.task_type) {
            if let Some((accepted, total)) = crate::advise::model_acceptance(&table, &suggestion.part, &suggestion.task_type, &best) {
                line.push_str(&format!(" Best recorded model: {} ({} of {} accepted).", best, accepted, total));
            }
        }
        out.push_str(&format!("{}\n", line));
    }
    Ok(out)
}

/// The profile an advice name points at: a profile name, a cluster id, or the newest cluster with that name.
fn profile_for_advice(ctx: &Ctx, name: &str) -> Result<Profile, String> {
    if let Ok(p) = profile(ctx, name) { return Ok(p); }
    let cluster = load(ctx, name).ok().or_else(|| {
        let mut matches: Vec<Cluster> = list(ctx).into_iter().filter(|c| c.name == name).collect();
        matches.sort_by_key(|c| (c.started, c.updated));
        matches.pop()
    });
    match cluster {
        Some(c) => profile(ctx, &c.profile).map_err(|e| format!("cluster {}: {}", c.id, e)),
        None => profile(ctx, name),
    }
}

/// One model setting a score-table key can mean. Only names the profile parser accepts are written back: `main`,
/// `weak` or a role. A label such as `test-cluster` is mapped back to the setting that carries it.
fn setting_for_model(ctx: &Ctx, name: &str) -> Option<String> {
    if name == "main" || name == "weak" || crate::route::role_names().iter().any(|r| *r == name) {
        return Some(name.to_string());
    }
    let mut settings: Vec<(&str, Value)> = vec![("main", ctx.cfg["main"].clone()), ("weak", ctx.cfg["weak"].clone())];
    for role in crate::route::role_names() {
        if let Some(model) = crate::route::model_for(ctx, role) {
            settings.push((role, model.clone()));
        }
    }
    settings.into_iter().find(|(_, model)| {
        model["label"].as_str() == Some(name) || model["model"].as_str() == Some(name)
    }).map(|(setting, _)| setting.to_string())
}

fn model_settings_text(ctx: &Ctx, part: &Part) -> String {
    let key = model_key_of(ctx, part);
    if key.is_empty() { "(no model)".to_string() } else { key }
}

/// `penelope cluster advise NAME --apply`: writes the suggested model setting into the profile for the next run.
/// A running cluster is refused, because it would keep the profile it started with and the change would look silent.
pub fn advise_apply(ctx: &Ctx, name: Option<&str>) -> Result<String, String> {
    let name = name.map(str::trim).filter(|s| !s.is_empty())
        .ok_or_else(|| "NOT applied. advise --apply needs a profile or cluster name; it never changes every profile at once.".to_string())?;
    let p = profile_for_advice(ctx, name)?;
    let running: Vec<String> = list(ctx).into_iter()
        .filter(|c| c.profile == p.name && matches!(c.state.as_str(), "queued" | "working" | "validating" | "killing"))
        .map(|c| c.id).collect();
    if !running.is_empty() {
        return Err(format!("NOT applied. {} running cluster(s) use profile {} ({}). A running cluster keeps the profile it started with; stop it or wait, then apply for the next run.",
                           running.len(), p.name, running.join(", ")));
    }
    let path = scores_path(ctx);
    if !path.exists() {
        return Err(format!("NOT applied. no cluster scores yet at {}. A validator writes them after a cluster run.", path.display()));
    }
    let table = crate::advise::load(&path)?;
    let all = crate::advise::suggestions(&table);
    let picked: Vec<&crate::advise::Suggestion> = all.iter().filter(|s| {
        p.workers.iter().any(|w| w.name == s.part && task_type_of(&p, w) == s.task_type)
    }).collect();
    if picked.is_empty() {
        return Ok(format!("profile {}: no part has enough rejected results for a change. Nothing was written.\n", p.name));
    }
    let json_path = profile_dir(ctx).join(format!("{}.json", safe(&p.name)));
    let cluster_path = profile_dir(ctx).join(format!("{}.cluster", safe(&p.name)));
    if !json_path.exists() && cluster_path.exists() {
        return Err(format!("NOT applied. profile {} is a .cluster file, which cannot carry a model. Create {}.json with the same parts and set the model there.",
                           p.name, p.name));
    }
    let mut value = profile_json(&p, &p.name);
    let mut changes: Vec<String> = Vec::new();
    for suggestion in picked {
        let Some(worker) = p.workers.iter().find(|w| w.name == suggestion.part) else { continue; };
        let current = model_settings_text(ctx, worker);
        let fallback = if current == "main" { "weak".to_string() } else { "main".to_string() };
        let recorded = crate::advise::best_model(&table, &suggestion.part, &suggestion.task_type)
            .and_then(|name| setting_for_model(ctx, &name));
        let best = match recorded {
            Some(name) if name == current => continue,
            Some(name) => name,
            None => fallback,
        };
        if best == current { continue; }
        if let Some(workers) = value["workers"].as_array_mut() {
            for w in workers {
                if w["name"].as_str() == Some(suggestion.part.as_str()) {
                    w["model"] = json!(best);
                }
            }
        }
        changes.push(format!("{}: {} -> {} · rejected {} of {} on {}",
                             suggestion.part, current, best, suggestion.rejected, suggestion.total, suggestion.task_type));
    }
    if changes.is_empty() {
        return Ok(format!("profile {}: the suggested model is already set. Nothing was written.\n", p.name));
    }
    let checked = parse_profile(&value, &p.name)?;
    fs::create_dir_all(profile_dir(ctx)).map_err(|e| format!("cannot make {}: {}", profile_dir(ctx).display(), e))?;
    fs::write(&json_path, serde_json::to_string_pretty(&value).unwrap())
        .map_err(|e| format!("cannot write {}: {}", json_path.display(), e))?;
    let mut out = format!("profile {}: {} change(s) for the next run\n", checked.name, changes.len());
    for change in changes { out.push_str(&format!("  {}\n", change)); }
    out.push_str("No running cluster was changed: a cluster reads its profile when it starts. Start a new run to use the new model.\n");
    Ok(out)
}

pub fn status_text(ctx: &Ctx, id: Option<&str>) -> String {
    if let Some(id) = id.filter(|s| !s.is_empty()) {
        let c = match load(ctx, id) { Ok(c) => c, Err(e) => return e };
        let mut out = format!("cluster {} · {} · profile {} · state {}\nTask: {}\nProgress: {}/{}\nMemory: {}\n",
                              c.id, c.name, c.profile, c.state, c.task, c.done, c.total, c.memory);
        if !c.worktree.is_empty() { out.push_str(&format!("Worktree: {} · branch {}\n", c.worktree, c.branch)); }
        if c.shadow { out.push_str(&format!("Shadow: yes · shadow of {} · files never merged\n", if c.shadow_of.is_empty() { "?" } else { &c.shadow_of })); }
        if let Some(rate) = c.match_rate { out.push_str(&format!("Shadow match: {:.1}%\n", rate * 100.0)); }
        if c.quorum_agreed + c.quorum_escalated > 0 {
            out.push_str(&format!("Quorum: {} agreed · {} escalated\n", c.quorum_agreed, c.quorum_escalated));
        }
        if !c.error.is_empty() { out.push_str(&format!("Error: {}\n", c.error)); }
        for p in &c.parts {
            out.push_str(&format!("  {:<12} {:<10} {:<12} {:<10} {}\n", p.name, p.role, p.slot, p.state, p.model));
            if !p.error.is_empty() { out.push_str(&format!("    {}\n", p.error.chars().take(200).collect::<String>())); }
        }
        if let Ok(report) = fs::read_to_string(cluster_dir(ctx, id).join("report.txt")) { out.push_str(&format!("\nReport: {}", report)); }
        // Every question and answer is in the report stream; show the last links next to the report.
        if let Ok(history) = fs::read_to_string(cluster_dir(ctx, id).join("reports.jsonl")) {
            let asks: Vec<&str> = history.lines().filter(|line| line.contains(" ASK ")).collect();
            if !asks.is_empty() {
                out.push_str("\nLinks:\n");
                for line in asks.iter().rev().take(4).rev() {
                    let text = line.splitn(2, ' ').nth(1).unwrap_or(line);
                    out.push_str(&format!("  {}\n", text));
                }
            }
        }
        if let Ok(memory) = fs::read_to_string(memory_path(ctx, id)) { out.push_str(&format!("\n{}", memory.chars().rev().take(4000).collect::<String>().chars().rev().collect::<String>())); }
        return out;
    }
    let clusters = list(ctx);
    if clusters.is_empty() { return "no cluster yet. /cluster GOAL starts one, /cluster profiles shows the profiles.".to_string(); }
    let mut out = String::from("Clusters\n\n");
    for c in clusters {
        let indent = "  ".repeat(c.depth);
        out.push_str(&format!("{}{:<22} {:<10} P:{}/{} {:.0}s  {:<24} {}\n", indent, c.name, c.state, c.done, c.total,
                              now().saturating_sub(c.started), c.id, c.task.chars().take(60).collect::<String>()));
        if !c.error.is_empty() { out.push_str(&format!("{}  ! {}\n", indent, c.error.chars().take(160).collect::<String>())); }
    }
    out.push_str("\n/cluster GOAL starts an orchestration. /cluster status ID dumps one cluster. /cluster profiles shows the parts of every profile.\n");
    out
}

fn dump_text(c: &Cluster, cause: &str) -> String {
    let parts: String = c.parts.iter().map(|p| format!("{} · {} · {} · {}\n", p.name, p.role, p.state,
                                                       if p.error.is_empty() { &p.model } else { &p.error })).collect();
    let memory = fs::read_to_string(&c.memory).unwrap_or_default();
    format!("Cluster: {} · {}\nState: {}\nTask: {}\nProgress: {}/{}\nCause: {}\nError: {}\n\nParts:\n{}\nMemory (tail):\n{}\n",
            c.id, c.name, c.state, c.task, c.done, c.total, cause, c.error, parts,
            memory.chars().rev().take(6000).collect::<String>().chars().rev().collect::<String>())
}

/// Writes the state dump of a zombie. The cause is the one line the respawned prompt must name.
fn write_dump(ctx: &Ctx, c: &Cluster, cause: &str) {
    let dir = cluster_dir(ctx, &c.id);
    let _ = fs::create_dir_all(&dir);
    let _ = fs::write(dir.join("dump.txt"), dump_text(c, cause));
    let _ = fs::write(dir.join("dump.json"), serde_json::to_string_pretty(&json!({"cause": cause, "state": c.state, "at": now()})).unwrap());
}

fn cause_from_dump(ctx: &Ctx, id: &str) -> Option<String> {
    let text = fs::read_to_string(cluster_dir(ctx, id).join("dump.json")).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    value["cause"].as_str().filter(|cause| !cause.trim().is_empty()).map(str::to_string)
}

/// The end of a zombie: the worktree is removed, no patch is merged, and the dump carries the cause.
fn stop_stuck(ctx: &Ctx, c: &mut Cluster, cause: &str) -> Result<String, String> {
    let note = cleanup_worktree(c);
    c.state = "stuck".to_string();
    c.error = cause.to_string();
    c.updated = now();
    write_dump(ctx, c, cause);
    save(ctx, c)?;
    report(ctx, c, "zombie", cause);
    Ok(format!("cluster {} stuck: {}{}", c.id, cause, if note.is_empty() { String::new() } else { format!(" · {}", note) }))
}

pub fn dump(ctx: &Ctx, id: &str) -> String {
    let c = match load(ctx, id) { Ok(c) => c, Err(e) => return e };
    let mut out = status_text(ctx, Some(id));
    out.push_str(&format!("\n\nOutput\n{}\n", if c.output.is_empty() { "(none)" } else { &c.output }));
    if let Ok(stored) = fs::read_to_string(cluster_dir(ctx, id).join("dump.txt")) {
        out.push_str(&format!("\n\nDump\n{}", stored));
    }
    out
}

/// Starts a killed cluster again with the task and the cause of its dump. The dump must exist first; a respawn
/// without a cause is refused. A cluster has at most two respawns.
pub fn respawn(ctx: &Ctx, id: &str, extra: &str) -> String {
    match respawn_inner(ctx, id, extra, None) {
        Ok((_, said)) => said,
        Err(e) => format!("NOT respawned. {}", e),
    }
}

fn respawn_inner(ctx: &Ctx, id: &str, extra: &str, session_before: Option<f64>) -> Result<(String, String), String> {
    let mut old = load(ctx, id)?;
    let cause = cause_from_dump(ctx, id).ok_or_else(|| {
        format!("cluster {} has no dump cause. Run cluster_dump {} first; a respawn without the cause from the dump is not allowed.", id, id)
    })?;
    if old.respawns >= 2 {
        return Err(format!("cluster {} was already respawned {} times; the limit is 2", id, old.respawns));
    }
    // Kill first, then start again. The old worktree goes before the new one is made.
    let _ = kill(ctx, id);
    old = load(ctx, id).unwrap_or(old);
    old.state = "killed".to_string();
    old.error = format!("respawned: {}", cause);
    old.updated = now();
    save(ctx, &old)?;
    let profile = profile(ctx, &old.profile)?;
    let task = format!("{}\n\nCause from the dump: {}\nCAUSE:{}|AT:{}|CLAIMS:-\n{}", old.task, cause,
                       error_code(&cause), old.id, extra.trim());
    let mut new = new_cluster(ctx, &profile, &old.name, &task, old.parent.as_deref(), old.after.clone(), old.depth)?;
    new.respawns = old.respawns + 1;
    new.respawned_from = old.id.clone();
    save(ctx, &new)?;
    let said = run_cluster_inner(ctx, &new.id, session_before)?;
    Ok((new.id.clone(), format!("cluster {} respawned ({} of 2) as {}\n{}", old.id, new.respawns, new.id, said)))
}

/// A profile as JSON again. A factory uses it to copy a base profile into its own profile file, which then carries the
/// schedule and one purpose. Every part field that `parse_profile` reads is written back, so the copy is complete.
pub fn profile_json(p: &Profile, name: &str) -> Value {
    let part_value = |part: &Part| {
        let mut value = json!({"name": part.name, "role": part.role, "slot": part.slot, "task": part.task, "model": part.model});
        if !part.task_type.is_empty() { value["task_type"] = json!(part.task_type); }
        if !part.mcp.is_empty() { value["mcp"] = json!(part.mcp); }
        if !part.depends_on.is_empty() { value["needs"] = json!(part.depends_on); }
        if part.quorum > 0 { value["quorum"] = json!(part.quorum); }
        value
    };
    let mut value = json!({
        "name": name,
        "workers": p.workers.iter().map(part_value).collect::<Vec<_>>(),
        "max_models": p.max_models, "max_depth": p.max_depth, "max_parallel": p.max_parallel,
        "budget_eur": p.budget_eur, "max_steps": p.max_steps, "retries": p.retries,
        "report_every": p.report_every, "stuck_after": p.stuck_after, "worktree": p.worktree,
        "quorum": p.quorum, "second_model": p.second_model, "decider_model": p.decider_model
    });
    if let Some(validator) = &p.validator { value["validator"] = part_value(validator); }
    if let Some(before) = &p.before { value["before"] = json!(before); }
    if let Some(shadow_of) = &p.shadow_of { value["shadow_of"] = json!(shadow_of); }
    if let Some(every) = &p.every { value["every"] = json!(every); }
    if let Some(at) = &p.at { value["at"] = json!(at); }
    if let Some(purpose) = &p.purpose { value["purpose"] = json!(purpose); }
    value
}

/// Creates or changes a profile file. The result is parsed by the same `parse_profile` as a profile loaded from disk,
/// so a broken profile is refused before it is written.
pub fn profile_upsert(ctx: &Ctx, name: &str, fields: &Value) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() { return Err("the profile needs a name".to_string()); }
    let Some(given) = fields.as_object() else {
        return Err("fields must be a JSON object with profile keys such as workers, max_parallel or budget_eur".to_string());
    };
    let path = profile_dir(ctx).join(format!("{}.json", safe(name)));
    let mut value = match fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str::<Value>(&text).map_err(|e| format!("profile {} is not JSON: {}", name, e))?,
        Err(_) => json!({"name": name}),
    };
    for (key, item) in given {
        value[key] = item.clone();
    }
    value["name"] = json!(name);
    let profile = parse_profile(&value, name)?;
    fs::create_dir_all(profile_dir(ctx)).map_err(|e| format!("cannot make {}: {}", profile_dir(ctx).display(), e))?;
    fs::write(&path, serde_json::to_string_pretty(&value).unwrap()).map_err(|e| format!("cannot write {}: {}", path.display(), e))?;
    Ok(format!("profile {} saved: {} parts, max {} parts, report every {}s, stuck after {}{}", name,
               profile.workers.len() + profile.validator.iter().count(), profile.max_models, profile.report_every, profile.stuck_after,
               if profile.budget_eur > 0.0 { format!(", {:.2} EUR budget", profile.budget_eur) } else { String::new() }))
}

pub fn kill(ctx: &Ctx, id: &str) -> String {
    let mut c = match load(ctx, id) { Ok(c) => c, Err(e) => return e };
    if c.state == "done" || c.state == "failed" || c.state == "killed" { return format!("cluster {} is already {}", id, c.state); }
    let _ = fs::create_dir_all(cluster_dir(ctx, id));
    let _ = fs::write(cluster_dir(ctx, id).join("kill"), "1");
    // A cluster in this process stops at once, including a model call that is waiting for its answer.
    signal_cancel(id);
    // The worktree and its branch go now. A running cluster repeats the removal when it sees the kill flag.
    let note = cleanup_worktree(&mut c);
    // The run may have stopped between the load above and the kill file. Never downgrade its final state to `killing`.
    if let Ok(latest) = load(ctx, id) {
        if matches!(latest.state.as_str(), "done" | "failed" | "killed") {
            return format!("cluster {} stopped{}", id, if note.is_empty() { String::new() } else { format!(" · {}", note) });
        }
    }
    c.state = "killing".to_string();
    c.updated = now();
    let _ = save(ctx, &c);
    format!("cluster {} will stop before its next part{}", id, if note.is_empty() { String::new() } else { format!(" · {}", note) })
}

/// The tool and CLI entry: run an orchestration and return its summary.
pub fn start(ctx: &Ctx, goal: &str, profile: Option<&str>, parallel: bool) -> String {
    match orchestrate(ctx, goal, profile, parallel) {
        Ok(text) => text,
        Err(e) => format!("NOT started. {}", e),
    }
}

/// The tail of a text, at most `n` characters. Answers and memory sections are capped so a tool answer stays readable.
fn tail(text: &str, n: usize) -> String {
    text.chars().rev().take(n).collect::<String>().chars().rev().collect()
}

/// Finds a cluster by exact id first, then by name. An old name that occurs twice resolves to the newest cluster.
fn find_target(ctx: &Ctx, target: &str) -> Result<Cluster, String> {
    let needle = target.trim();
    if needle.is_empty() {
        return Err("NOT asked. The target cluster is empty; name its id or its name.".to_string());
    }
    if let Ok(c) = load(ctx, needle) { return Ok(c); }
    let all = list(ctx);
    let mut matches: Vec<Cluster> = all.iter().filter(|c| c.name == needle).cloned().collect();
    matches.sort_by_key(|c| (c.started, c.updated));
    if let Some(c) = matches.pop() { return Ok(c); }
    let known: Vec<String> = all.iter().take(12).map(|c| format!("{} ({}, {})", c.id, c.name, c.state)).collect();
    Err(format!("no cluster {}. known clusters: {}. Use cluster list to see the tree.",
                needle, if known.is_empty() { "none yet".to_string() } else { known.join(", ") }))
}

/// The answer of a cluster without a new model call: its output first, then its memory sections, then its state.
/// A finished cluster with neither output nor a memory section is an empty answer, not a silent success. A previous
/// question or answer in the memory is a link, not the target's own part result.
fn answer_of(target: &Cluster, memory: &str) -> Result<(String, &'static str), String> {
    let output = target.output.trim();
    if !output.is_empty() {
        return Ok((tail(output, 6000), "output"));
    }
    let has_part = memory.lines().any(|line| {
        line.starts_with("## ")
            && !line.starts_with("## question from ")
            && !line.starts_with("## answer from ")
            && !line.starts_with("## ask ")
    });
    if has_part {
        return Ok((tail(memory.trim(), 6000), "memory"));
    }
    if matches!(target.state.as_str(), "done" | "failed" | "killed" | "stuck") {
        return Err(format!("cluster {} ({}) is {} but has no answer: its output and memory hold no part result.",
                           target.id, target.name, target.state));
    }
    Ok((format!("cluster {} ({}) is {} and has no answer yet: its output and memory hold no part result.",
                target.id, target.name, target.state), "state"))
}

/// One line in the report stream of a cluster. The orchestrator reads `reports.jsonl` and `cluster_status`.
fn stream_ask(ctx: &Ctx, id: &str, line: &str) {
    let dir = cluster_dir(ctx, id);
    let _ = fs::create_dir_all(&dir);
    let _guard = REPORT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(dir.join("reports.jsonl")) {
        let _ = writeln!(file, "{} ASK {}", now(), line);
    }
}

/// A report-stream entry for a quorum or shadow result. It is not a `S:` line; the cluster state stays in `report.txt`.
fn stream_note(ctx: &Ctx, id: &str, line: &str) {
    let dir = cluster_dir(ctx, id);
    let _ = fs::create_dir_all(&dir);
    let _guard = REPORT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(dir.join("reports.jsonl")) {
        let _ = writeln!(file, "{} {}", now(), line);
    }
}

/// The fixed question codes of the Kerkis answer channel. A free question text stays in the memories.
fn ask_code(question: &str) -> &'static str {
    let q = question.to_lowercase();
    if q.contains("design") || q.contains("option") || q.contains("layout") { "design_options" }
    else if q.contains("status") || q.contains("state") || q.contains("progress") { "status" }
    else if q.contains("file") || q.contains("path") || q.contains("where") { "file_ref" }
    else if q.contains("test") || q.contains("check") { "test_result" }
    else if q.contains("error") || q.contains("fail") || q.contains("why") { "failure_reason" }
    else { "other" }
}

fn ask_inner(ctx: &Ctx, asker_id: &str, target_name: &str, question: &str) -> Result<String, String> {
    let question = question.trim();
    if question.is_empty() {
        return Err("NOT asked. The question is empty.".to_string());
    }
    let asker = load(ctx, asker_id)?;
    let target = find_target(ctx, target_name)?;
    if target.id == asker.id {
        return Err(format!("NOT asked. Cluster {} cannot ask itself; ask another cluster in the tree.", asker.id));
    }
    // The question is part of the target's memory before any answer is read, so the exchange stays auditable.
    let before = fs::read_to_string(memory_path(ctx, &target.id)).unwrap_or_default();
    append_memory(ctx, &target, &format!("\n## question from {} · {}\n\n{}\n", asker.name, asker.id, question));
    let (answer, source) = answer_of(&target, &before)?;
    append_memory(ctx, &asker, &format!("\n## answer from {} · {}\n\nquestion: {}\nanswer (from {}, {}): {}\n",
                                       target.name, target.id, question, target.state, source, answer));
    // The report stream carries codes and ids only. The question text stays in the memories, where the asking part
    // reads it; the orchestrator sees CODE, Q, HOPS, VISITED and REF, not free text.
    let code = ask_code(question);
    let qid = format!("q{}", now());
    let event = format!("{} -> {} CODE:{} Q:{} HOPS:1 VISITED:{} REF:{} L:1", asker.id, target.id, code, qid, asker.id, target.id);
    stream_ask(ctx, &asker.id, &event);
    stream_ask(ctx, &target.id, &event);
    let block = format!("\n## ask {} -> {}\n\nquestion: {}\nanswer (from {}, {}): {}\n",
                       asker.id, target.id, question, target.state, source, answer);
    let dir = cluster_dir(ctx, &asker.id);
    let _ = fs::create_dir_all(&dir);
    let _ = fs::OpenOptions::new().create(true).append(true).open(dir.join("ask.txt"))
        .and_then(|mut f| f.write_all(block.as_bytes()));
    Ok(format!("Q (to {} · {}): {}\nA (from {}, {}):\n{}", target.name, target.id, question, target.state, source, answer))
}

/// The tool `cluster_ask`: a running cluster asks another cluster and keeps the answer in its own memory. No model
/// call is made; the answer is the target's output, memory or state.
pub fn ask(ctx: &Ctx, target: &str, question: &str) -> Result<String, String> {
    let asker = current_cluster().ok_or_else(|| {
        "NOT asked. cluster_ask belongs to a part inside a running cluster; the asking cluster keeps the answer in its memory.".to_string()
    })?;
    ask_inner(ctx, &asker, target, question)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> (Ctx, PathBuf) {
        let dir = std::env::temp_dir().join(format!("penelope-clusters-{}-{:?}", std::process::id(), std::thread::current().id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let run = |args: &[&str]| {
            let out = Command::new("git").current_dir(&dir).args(args).output().unwrap();
            assert!(out.status.success(), "git {:?}: {}", args, String::from_utf8_lossy(&out.stderr));
        };
        run(&["init", "-q"]);
        // The Windows runners set core.autocrlf=true globally; the tests compare bytes, so this repository keeps \n.
        run(&["config", "core.autocrlf", "false"]);
        run(&["config", "user.email", "test@example.test"]);
        run(&["config", "user.name", "Test"]);
        fs::write(dir.join("hello.txt"), "base\n").unwrap();
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "base"]);
        (crate::config::load(&dir.join(".penelope")), dir)
    }

    fn text_reply(text: &str) -> String {
        json!({"choices": [{"finish_reason": "stop", "message": {"role": "assistant", "content": text}}],
               "usage": {"prompt_tokens": 10, "completion_tokens": 2}}).to_string()
    }

    fn tool_reply(name: &str, args: Value) -> String {
        tool_reply_usage(name, args, 10, 2)
    }

    fn tool_reply_usage(name: &str, args: Value, tokens_in: u64, tokens_out: u64) -> String {
        json!({"choices": [{"finish_reason": "tool_calls", "message": {"role": "assistant", "content": "",
            "tool_calls": [{"id": "call-1", "type": "function", "function": {"name": name, "arguments": args.to_string()}}]}}],
            "usage": {"prompt_tokens": tokens_in, "completion_tokens": tokens_out}}).to_string()
    }

    fn fake_models(ctx: &mut Ctx, address: &str) {
        let model = json!({"kind": "openai", "base_url": format!("{}/v1", address), "model": "test-cluster", "api_key_env": ""});
        ctx.cfg["main"] = model.clone();
        ctx.cfg["weak"] = model.clone();
        ctx.cfg["edit_policy"] = json!("all");
        ctx.cfg["roles"]["judge"] = model.clone();
        ctx.cfg["roles"]["router"] = model;
    }

    fn branch_exists(dir: &Path, branch: &str) -> bool {
        Command::new("git").arg("-C").arg(dir).args(["branch", "--list", branch]).output()
            .map(|o| !String::from_utf8_lossy(&o.stdout).trim().is_empty()).unwrap_or(false)
    }

    #[test]
    fn profiles_are_built_from_parts_and_a_missing_part_is_refused() {
        let (ctx, dir) = ctx();
        let coding = profile(&ctx, "coding").unwrap();
        assert_eq!((coding.workers.len(), coding.validator.as_ref().map(|v| v.name.as_str())), (2, Some("check")));
        assert_eq!((coding.max_models, coding.max_depth), (8, 2));
        assert!(coding.worktree, "a profile with a worker part writes in a worktree");
        assert!(!profile(&ctx, "research").unwrap().worktree && !profile(&ctx, "live-test").unwrap().worktree);
        let text = profiles_text(&ctx);
        assert!(text.contains("coding") && text.contains("backend (worker)") && text.contains("worktree"), "{}", text);
        fs::create_dir_all(profile_dir(&ctx)).unwrap();
        fs::write(profile_dir(&ctx).join("tiny.json"), r#"{"name":"tiny","workers":[{"name":"one","role":"worker","model":"main"},{"name":"two","role":"worker","model":"main"}],"max_models":1}"#).unwrap();
        assert!(profile(&ctx, "tiny").unwrap_err().contains("max_models"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_cluster_is_created_reported_and_killed_without_a_model() {
        let (ctx, dir) = ctx();
        let p = profile(&ctx, "solo").unwrap();
        let c = new_cluster(&ctx, &p, "demo", "do the thing", None, Vec::new(), 0).unwrap();
        assert!(cluster_file(&ctx, &c.id).is_file() && memory_path(&ctx, &c.id).is_file());
        assert_eq!(list(&ctx).len(), 1);
        let c = load(&ctx, &c.id).unwrap();
        assert_eq!((c.state.as_str(), c.total), ("queued", 2));
        let said = kill(&ctx, &c.id);
        assert!(said.contains("will stop") && kill_requested(&ctx, &c.id));
        assert!(status_text(&ctx, None).contains("demo"));
        assert!(dump(&ctx, &c.id).contains("Task: do the thing"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_plan_keeps_dependencies_and_refuses_an_unknown_profile() {
        let (ctx, dir) = ctx();
        let plan = parse_plan(r#"{"clusters":[{"name":"back","profile":"backend","task":"server"},{"name":"front","profile":"frontend","task":"ui"},{"name":"live","profile":"live-test","task":"test","after":["back","front"]}]}"#, &ctx).unwrap();
        assert_eq!((plan.len(), plan[2].after.len()), (3, 2));
        assert!(parse_plan(r#"{"clusters":[{"name":"x","profile":"nope","task":"y"}]}"#, &ctx).unwrap_err().contains("unknown cluster profile"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_orchestrator_plan_runs_dependent_clusters_in_order() {
        let (address, _seen) = crate::llm::fake::serve(vec![
            (200, json!({"choices": [{"finish_reason": "stop", "message": {"role": "assistant", "content":
                "{\"clusters\":[{\"name\":\"one\",\"profile\":\"solo\",\"task\":\"first\"},{\"name\":\"two\",\"profile\":\"solo\",\"task\":\"second\",\"after\":[\"one\"]}]}"}}],
                "usage": {"prompt_tokens": 10, "completion_tokens": 20}}).to_string()),
            (200, text_reply("one done")),
            (200, tool_reply("run", json!({"command": "cat hello.txt"}))),
            (200, text_reply("PASS: one is done")),
            (200, text_reply("two done")),
            (200, tool_reply("run", json!({"command": "cat hello.txt"}))),
            (200, text_reply("PASS: two is done")),
        ]);
        let (mut ctx, dir) = ctx();
        fake_models(&mut ctx, &address);
        let said = orchestrate(&ctx, "do two things", None, true).unwrap();
        assert!(said.contains("2 clusters started"), "{}", said);
        let clusters = list(&ctx);
        assert_eq!((clusters.len(), clusters.iter().filter(|c| c.state == "done").count()), (2, 2));
        let two = clusters.iter().find(|c| c.name == "two").unwrap();
        let one = clusters.iter().find(|c| c.name == "one").unwrap();
        assert_eq!(two.after, vec![one.id.clone()]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn one_cluster_runs_worker_parts_and_its_validator() {
        let (address, _seen) = crate::llm::fake::serve(vec![
            (200, text_reply("backend done")),
            (200, text_reply("frontend done")),
            (200, tool_reply("run", json!({"command": "cat hello.txt"}))),
            (200, text_reply("PASS: both parts are in the memory")),
        ]);
        let (mut ctx, dir) = ctx();
        fake_models(&mut ctx, &address);
        let said = orchestrate(&ctx, "build the thing", Some("coding"), true).unwrap();
        assert!(said.contains("2 clusters") == false && said.contains("clusters started"), "{}", said);
        let c = list(&ctx).into_iter().next().unwrap();
        assert_eq!((c.state.as_str(), c.done, c.total), ("done", 3, 3));
        assert!(c.parts.iter().all(|p| p.state == "done"), "{:?}", c.parts);
        assert!(c.output.contains("backend done") && c.output.contains("frontend done"));
        let memory = fs::read_to_string(memory_path(&ctx, &c.id)).unwrap();
        assert!(memory.contains("## backend") && memory.contains("## check") && memory.contains("PASS"), "{}", memory);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn two_clusters_keep_their_edits_apart_and_kill_removes_one_worktree() {
        let (address, _seen) = crate::llm::fake::serve(vec![
            (200, text_reply("done")),
            (200, tool_reply("run", json!({"command": "cat hello.txt"}))),
            (200, text_reply("PASS: hello.txt is in the worktree")),
        ]);
        let (mut ctx, dir) = ctx();
        fake_models(&mut ctx, &address);
        let p = profile(&ctx, "solo").unwrap();
        let one = new_cluster(&ctx, &p, "one", "edit hello.txt", None, Vec::new(), 0).unwrap();
        let two = new_cluster(&ctx, &p, "two", "edit hello.txt", None, Vec::new(), 0).unwrap();
        assert_ne!(one.worktree, two.worktree);
        assert_eq!(one.cwd, one.worktree);
        // Path::starts_with compares parts, so C:/work/.penelope\worktrees\one on Windows matches as well.
        assert!(Path::new(&one.worktree).starts_with(Path::new(&one.project).join(".penelope").join("worktrees")), "{}", one.worktree);
        assert!(one.branch.starts_with("agent/") && branch_exists(&dir, &one.branch));
        fs::write(Path::new(&one.worktree).join("hello.txt"), "edit one\n").unwrap();
        fs::write(Path::new(&two.worktree).join("hello.txt"), "edit two\n").unwrap();
        assert_eq!(fs::read_to_string(dir.join("hello.txt")).unwrap(), "base\n");

        let said = kill(&ctx, &one.id);
        assert!(said.contains("will stop") && said.contains("removed"), "{}", said);
        assert!(!Path::new(&one.worktree).exists());
        assert!(!branch_exists(&dir, &one.branch));
        assert!(Path::new(&two.worktree).is_dir(), "the other worktree stays");

        let out = run_cluster(&ctx, &two.id).unwrap();
        assert!(out.contains("done"), "{}", status_text(&ctx, Some(&two.id)));
        assert_eq!(fs::read_to_string(dir.join("hello.txt")).unwrap(), "edit two\n");
        assert!(Path::new(&two.worktree).is_dir(), "the worktree stays for inspection");
        assert!(branch_exists(&dir, &two.branch));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_tool_call_without_cwd_from_an_api_part_writes_in_its_worktree() {
        let (address, seen) = crate::llm::fake::serve(vec![
            (200, tool_reply("write", json!({"path": "part.txt", "text": "made by the part\n"}))),
            (200, text_reply("wrote part.txt")),
            (200, tool_reply("run", json!({"command": "cat hello.txt"}))),
            (200, text_reply("PASS: the part wrote its file")),
        ]);
        let (mut ctx, dir) = ctx();
        fake_models(&mut ctx, &address);
        let p = profile(&ctx, "solo").unwrap();
        let c = new_cluster(&ctx, &p, "cwd", "write part.txt", None, Vec::new(), 0).unwrap();
        assert!(!dir.join("part.txt").exists());
        let out = run_cluster(&ctx, &c.id).unwrap();
        assert!(out.contains("done"), "{}", out);
        let body = seen.lock().unwrap()[1].body.to_string();
        // The body is JSON: a Windows path arrives there with every \ escaped as \\.
        let in_json = json!(c.worktree).to_string();
        assert!(body.contains(in_json.trim_matches('"')), "the write went to the worktree, not the live tree: {}", body);
        assert_eq!(fs::read_to_string(dir.join("part.txt")).unwrap(), "made by the part\n");
        assert_eq!(fs::read_to_string(Path::new(&c.worktree).join("part.txt")).unwrap(), "made by the part\n");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_worktree_profile_is_refused_in_a_folder_without_git() {
        let dir = std::env::temp_dir().join(format!("penelope-clusters-nogit-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let ctx = crate::config::load(&dir.join(".penelope"));
        let p = profile(&ctx, "coding").unwrap();
        let err = new_cluster(&ctx, &p, "write", "edit a file", None, Vec::new(), 0).unwrap_err();
        assert!(err.contains("git worktree") && err.contains("no git project"), "{}", err);
        let research = profile(&ctx, "research").unwrap();
        let c = new_cluster(&ctx, &research, "look", "read only", None, Vec::new(), 0).unwrap();
        assert!(c.worktree.is_empty() && c.cwd.is_empty() && c.branch.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_merge_conflict_is_reported_and_the_live_file_is_untouched() {
        let (address, _seen) = crate::llm::fake::serve(vec![
            (200, text_reply("done")),
            (200, tool_reply("run", json!({"command": "cat hello.txt"}))),
            (200, text_reply("PASS: the worktree holds the edit")),
        ]);
        let (mut ctx, dir) = ctx();
        fake_models(&mut ctx, &address);
        let p = profile(&ctx, "solo").unwrap();
        let c = new_cluster(&ctx, &p, "conflict", "edit hello.txt", None, Vec::new(), 0).unwrap();
        fs::write(Path::new(&c.worktree).join("hello.txt"), "cluster edit\n").unwrap();
        fs::write(dir.join("hello.txt"), "live edit\n").unwrap();
        let out = run_cluster(&ctx, &c.id).unwrap();
        assert!(out.contains("blocked"), "{}", out);
        let c = load(&ctx, &c.id).unwrap();
        assert!(c.error.contains("merge conflict"), "{}", c.error);
        assert_eq!(fs::read_to_string(dir.join("hello.txt")).unwrap(), "live edit\n");
        assert!(Path::new(&c.worktree).join("hello.txt").is_file(), "the worktree stays for inspection");
        assert!(branch_exists(&dir, &c.branch));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_validator_that_runs_no_check_fails_the_cluster() {
        let (address, _seen) = crate::llm::fake::serve(vec![
            (200, text_reply("done")),
            (200, text_reply("PASS: everything looks good")),
            (200, text_reply("done")),
            (200, text_reply("PASS: everything looks good")),
        ]);
        let (mut ctx, dir) = ctx();
        fake_models(&mut ctx, &address);
        let p = profile(&ctx, "solo").unwrap();
        let c = new_cluster(&ctx, &p, "nocheck", "do the task", None, Vec::new(), 0).unwrap();
        let out = run_cluster(&ctx, &c.id).unwrap();
        assert!(out.contains("failed"), "{}", out);
        let c = load(&ctx, &c.id).unwrap();
        assert_eq!((c.state.as_str(), c.evidence.as_str()), ("failed", ""));
        assert!(c.error.contains("ran no check"), "{}", c.error);
        let line = fs::read_to_string(cluster_dir(&ctx, &c.id).join("report.txt")).unwrap();
        let r = crate::proof::parse(&line).unwrap();
        assert_eq!(r.s, "failed");
        assert!(crate::proof::accept(&r).is_ok());

        // A done line without V is refused by the same gate the orchestrator uses.
        let mut done = c.clone();
        done.state = "done".to_string();
        done.evidence.clear();
        report(&ctx, &done, "-", "-");
        let line = fs::read_to_string(cluster_dir(&ctx, &c.id).join("report.txt")).unwrap();
        let r = crate::proof::parse(&line).unwrap();
        assert!(crate::proof::accept(&r).is_err(), "{}", line);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_one_worker_profile_without_retry_skips_the_final_validator() {
        let (address, seen) = crate::llm::fake::serve(vec![
            (200, tool_reply("write", json!({"path": "hello.txt", "text": "hello\n"}))),
            (200, tool_reply("run", json!({"command": "cat hello.txt"}))),
            (200, text_reply("created hello.txt")),
        ]);
        let (mut ctx, dir) = ctx();
        fake_models(&mut ctx, &address);
        write_profile(&ctx, "single", json!({
            "name": "single",
            "workers": [{"name": "coder", "role": "worker", "slot": "small", "model": "main", "task": "Do the task."}],
            "validator": {"name": "check", "role": "judge", "slot": "check", "model": "main", "task": "Check."},
            "max_models": 2, "max_depth": 1, "max_steps": 8, "retries": 0, "single_model": true
        }));
        let p = profile(&ctx, "single").unwrap();
        let c = new_cluster(&ctx, &p, "single-fixture", "write hello.txt", None, Vec::new(), 0).unwrap();
        let out = run_cluster(&ctx, &c.id).unwrap();
        assert!(out.contains("done"), "{}", out);
        let c = load(&ctx, &c.id).unwrap();
        assert_eq!((c.state.as_str(), c.done, c.total), ("done", 1, 1));
        assert_eq!(c.evidence, "cat hello.txt ok");
        assert_eq!(seen.lock().unwrap().len(), 3, "the validator must not run");
        let line = fs::read_to_string(cluster_dir(&ctx, &c.id).join("report.txt")).unwrap();
        assert!(line.contains("P:1/1"), "{}", line);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_solo_fixture_ends_with_the_check_and_its_result() {
        let (address, _seen) = crate::llm::fake::serve(vec![
            (200, tool_reply("write", json!({"path": "hello.txt", "text": "hello\n"}))),
            (200, text_reply("created hello.txt")),
            (200, tool_reply("run", json!({"command": "cat hello.txt"}))),
            (200, text_reply("PASS: hello.txt holds the expected line")),
        ]);
        let (mut ctx, dir) = ctx();
        fake_models(&mut ctx, &address);
        let p = profile(&ctx, "solo").unwrap();
        let c = new_cluster(&ctx, &p, "solo-fixture", "write hello.txt", None, Vec::new(), 0).unwrap();
        let out = run_cluster(&ctx, &c.id).unwrap();
        assert!(out.contains("done"), "{}", out);
        let c = load(&ctx, &c.id).unwrap();
        assert_eq!((c.state.as_str(), c.evidence.as_str()), ("done", "cat hello.txt ok"));
        assert_eq!(fs::read_to_string(dir.join("hello.txt")).unwrap(), "hello\n");
        let line = fs::read_to_string(cluster_dir(&ctx, &c.id).join("report.txt")).unwrap();
        let r = crate::proof::parse(&line).unwrap();
        assert_eq!(r.s, "done");
        assert_eq!(r.v.as_deref(), Some("cat hello.txt ok"));
        assert!(crate::proof::accept(&r).is_ok());
        let krk = fs::read_to_string(cluster_dir(&ctx, &c.id).join("report.krk")).unwrap();
        let k = crate::proof::parse_kerkis(&krk).unwrap();
        assert_eq!((k.c.as_str(), k.s.as_str()), (c.id.as_str(), "done"));
        assert!(k.v.starts_with('e') && k.v != "-", "{}", krk);
        assert!(crate::proof::accept_kerkis(&k).is_ok());
        let memory = fs::read_to_string(memory_path(&ctx, &c.id)).unwrap();
        assert!(memory.contains("evidence: cat hello.txt ok"), "{}", memory);
        let log = fs::read_to_string(memory_log_path(&ctx, &c.id)).unwrap();
        assert!(log.lines().all(|line| serde_json::from_str::<Value>(line).unwrap()["L"].as_u64() == Some(1)), "{}", log);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_watch_compares_name_arguments_and_a_hash_of_the_result() {
        let (ctx, dir) = ctx();
        let p = profile(&ctx, "solo").unwrap();
        let c = new_cluster(&ctx, &p, "watch", "do the task", None, Vec::new(), 0).unwrap();
        let part = p.workers.first().unwrap();
        let args = json!({"command": "date"});
        let mut watch = Watch::new(3);
        for result in ["10:00", "10:01", "10:02", "10:03"] {
            assert!(watch.note_call(&ctx, &c, part, "run", &args, result).is_none());
        }
        for _ in 0..2 {
            assert!(watch.note_call(&ctx, &c, part, "run", &args, "10:00").is_none());
        }
        assert!(watch.note_call(&ctx, &c, part, "run", &args, "10:00").is_some());
        let log = fs::read_to_string(cluster_dir(&ctx, &c.id).join("calls.jsonl")).unwrap();
        assert!(log.contains("run") && !log.contains("10:00"), "the log keeps the hash, not the result: {}", log);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_switch_sends_small_or_sequential_tasks_to_one_model() {
        assert_eq!(switch_decision(28_665, false, SWITCH_KV_TOKENS_DESKTOP).0, true);
        assert_eq!(switch_decision(28_666, false, SWITCH_KV_TOKENS_DESKTOP).0, false);
        assert_eq!(switch_decision(1_000_000, true, SWITCH_KV_TOKENS_DESKTOP).0, true);
        assert!(switch_text(Some(28_666), "", false, SWITCH_KV_TOKENS_DESKTOP).contains("route: cluster"));
        assert!(switch_text(None, "short task", false, SWITCH_KV_TOKENS_DESKTOP).contains("route: single"));
    }

    fn write_profile(ctx: &Ctx, name: &str, value: Value) {
        fs::create_dir_all(profile_dir(ctx)).unwrap();
        fs::write(profile_dir(ctx).join(format!("{}.json", name)), value.to_string()).unwrap();
    }

    fn template_value(name: &str, base: &str) -> Value {
        let t = cluster_template(base).unwrap();
        json!({"name": name, "role": t.0, "slot": t.1, "model": t.2, "task": t.3})
    }

    fn priced_models(ctx: &mut Ctx, address: &str) {
        fake_models(ctx, address);
        ctx.cfg["prices"] = json!({"test-cluster": [10.0, 10.0]});
    }

    #[test]
    fn a_kill_ends_a_sleeping_api_call_within_two_seconds() {
        let (address, _seen) = crate::llm::fake::serve_parallel(vec![(200, text_reply("too late"), 5_000)]);
        let (mut ctx, dir) = ctx();
        fake_models(&mut ctx, &address);
        let p = profile(&ctx, "solo").unwrap();
        let c = new_cluster(&ctx, &p, "sleepy", "do the thing", None, Vec::new(), 0).unwrap();
        let id = c.id.clone();
        let runner_ctx = ctx.clone();
        let handle = std::thread::spawn(move || run_cluster(&runner_ctx, &id));
        std::thread::sleep(Duration::from_millis(400));
        let started = std::time::Instant::now();
        let said = kill(&ctx, &c.id);
        // The cancel reaches the waiting call at once. Where git needs longer to remove the worktree (Windows), the
        // run has already saved "killed" when kill looks again, and kill reports "stopped" in place of "will stop".
        assert!(said.contains("will stop") || said.contains("stopped"), "{}", said);
        let out = handle.join().unwrap().unwrap();
        let elapsed = started.elapsed();
        assert!(elapsed < Duration::from_secs(2), "the kill took {:?}: {}", elapsed, out);
        assert!(out.contains("killed"), "{}", out);
        let c = load(&ctx, &c.id).unwrap();
        assert_eq!(c.state.as_str(), "killed");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_call_that_crosses_the_cluster_budget_blocks_without_a_merge() {
        let dir_text = std::env::temp_dir().join("penelope-budget-target");
        let (address, seen) = crate::llm::fake::serve_parallel(vec![
            (200, tool_reply_usage("write", json!({"path": "hello.txt", "text": "spent\n", "cwd": dir_text.to_string_lossy()}),
                                   1_000_000, 1_000_000), 0),
        ]);
        let (mut ctx, dir) = ctx();
        priced_models(&mut ctx, &address);
        write_profile(&ctx, "budget", json!({
            "name": "budget",
            "workers": [{"name": "one", "role": "scout", "model": "main"}],
            "validator": {"name": "check", "role": "judge", "model": "judge"},
            "budget_eur": 0.01, "worktree": false, "retries": 0
        }));
        let p = profile(&ctx, "budget").unwrap();
        let c = new_cluster(&ctx, &p, "budget", "try a big call", None, Vec::new(), 0).unwrap();
        let out = run_cluster(&ctx, &c.id).unwrap();
        assert!(out.contains("blocked"), "{}", out);
        let c = load(&ctx, &c.id).unwrap();
        assert_eq!(c.state.as_str(), "blocked");
        assert!(c.error.starts_with("BUDGET: "), "{}", c.error);
        assert_eq!(seen.lock().unwrap().len(), 1, "the call that crossed the limit was the last one");
        assert_eq!(fs::read_to_string(dir.join("hello.txt")).unwrap(), "base\n", "nothing was merged");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_session_budget_stops_a_cluster_that_has_no_budget_of_its_own() {
        let (address, seen) = crate::llm::fake::serve_parallel(vec![
            (200, tool_reply_usage("read", json!({"path": "hello.txt"}), 1_000_000, 1_000_000), 0),
        ]);
        let (mut ctx, dir) = ctx();
        priced_models(&mut ctx, &address);
        ctx.cfg["session_budget_eur"] = json!(0.01);
        write_profile(&ctx, "nobudget", json!({
            "name": "nobudget",
            "workers": [{"name": "one", "role": "scout", "model": "main"}],
            "validator": {"name": "check", "role": "judge", "model": "judge"},
            "worktree": false, "retries": 0
        }));
        let p = profile(&ctx, "nobudget").unwrap();
        let c = new_cluster(&ctx, &p, "session", "read one file", None, Vec::new(), 0).unwrap();
        let out = run_cluster(&ctx, &c.id).unwrap();
        assert!(out.contains("blocked"), "{}", out);
        let c = load(&ctx, &c.id).unwrap();
        assert!(c.error.starts_with("BUDGET: the session"), "{}", c.error);
        assert_eq!(seen.lock().unwrap().len(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_cluster_that_repeats_its_report_is_dumped_and_respawned_with_the_cause() {
        let (mut ctx, dir) = ctx();
        // The read calls name an absolute path, so the fake tool never depends on the test's working directory.
        let probe = dir.join("hello.txt").to_string_lossy().to_string();
        let (address, seen) = crate::llm::fake::serve_parallel(vec![
            // The first call sleeps long enough for three one-second reports with one P: and F:.
            (200, tool_reply("read", json!({"path": probe.clone()})), 5_000),
            (200, text_reply("recovered"), 0),
            (200, tool_reply("read", json!({"path": probe.clone()})), 0),
            (200, text_reply("PASS: recovered"), 0),
        ]);
        fake_models(&mut ctx, &address);
        write_profile(&ctx, "zombie", json!({
            "name": "zombie",
            "workers": [{"name": "worker", "role": "scout", "model": "main"}],
            "validator": {"name": "check", "role": "judge", "model": "judge"},
            "report_every": 1, "stuck_after": 3, "retries": 0, "worktree": false
        }));
        let said = orchestrate(&ctx, "do a thing", Some("zombie"), true).unwrap();
        assert!(said.contains("1 clusters started"), "{}", said);
        let clusters = list(&ctx);
        assert_eq!(clusters.len(), 2, "a zombie is kept and respawned: {}", said);
        let old = clusters.iter().find(|c| c.respawned_from.is_empty()).expect("the first cluster is kept");
        let new = clusters.iter().find(|c| !c.respawned_from.is_empty()).expect("the respawned cluster is kept");
        assert_eq!((old.state.as_str(), new.state.as_str()), ("killed", "done"), "{}", said);
        assert_eq!((new.respawns, new.respawned_from.as_str()), (1, old.id.as_str()));
        let stored = fs::read_to_string(cluster_dir(&ctx, &old.id).join("dump.txt")).unwrap();
        let cause = stored.lines().find_map(|line| line.strip_prefix("Cause: ")).expect("the dump names a cause").to_string();
        assert!(new.task.contains("Cause from the dump:") && new.task.contains(&cause), "{}", new.task);
        assert!(new.task.contains("CAUSE:") && new.task.contains("AT:") && new.task.contains("CLAIMS:-"), "{}", new.task);
        let bodies: Vec<String> = seen.lock().unwrap().iter().map(|s| s.body.to_string()).collect();
        assert!(bodies.iter().any(|body| body.contains(&cause)), "the second prompt names the dump cause: {:?}", bodies);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn repeated_identical_tool_calls_are_a_zombie_cause() {
        let probe = json!({"path": "hello.txt"});
        let (address, _seen) = crate::llm::fake::serve(vec![
            (200, tool_reply("read", probe.clone())),
            (200, tool_reply("read", probe.clone())),
            (200, tool_reply("read", probe.clone())),
        ]);
        let (mut ctx, dir) = ctx();
        fake_models(&mut ctx, &address);
        write_profile(&ctx, "toolzombie", json!({
            "name": "toolzombie",
            "workers": [{"name": "worker", "role": "scout", "model": "main"}],
            "report_every": 3600, "stuck_after": 3, "retries": 0, "worktree": false
        }));
        let p = profile(&ctx, "toolzombie").unwrap();
        let c = new_cluster(&ctx, &p, "toolzombie", "read the same file", None, Vec::new(), 0).unwrap();
        let out = run_cluster(&ctx, &c.id).unwrap();
        assert!(out.contains("stuck"), "{}", out);
        let c = load(&ctx, &c.id).unwrap();
        assert_eq!(c.state.as_str(), "stuck");
        assert!(c.error.contains("repeated tool call"), "{}", c.error);
        assert!(dump(&ctx, &c.id).contains("repeated tool call"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_respawn_needs_a_dump_cause_and_stops_at_two() {
        let (mut ctx, dir) = ctx();
        fake_models(&mut ctx, "http://127.0.0.1:9");
        write_profile(&ctx, "spawn", json!({
            "name": "spawn",
            "workers": [{"name": "one", "role": "scout", "model": "main"}],
            "report_every": 0, "stuck_after": 3, "worktree": false
        }));
        let p = profile(&ctx, "spawn").unwrap();
        let c = new_cluster(&ctx, &p, "spawn", "do it", None, Vec::new(), 0).unwrap();
        let refused = respawn(&ctx, &c.id, "");
        assert!(refused.contains("no dump cause") && refused.contains("cluster_dump"), "{}", refused);
        let mut c = load(&ctx, &c.id).unwrap();
        fs::create_dir_all(cluster_dir(&ctx, &c.id)).unwrap();
        fs::write(cluster_dir(&ctx, &c.id).join("dump.json"), json!({"cause": "hung at the TLS handshake"}).to_string()).unwrap();
        c.respawns = 2;
        save(&ctx, &c).unwrap();
        let refused = respawn(&ctx, &c.id, "");
        assert!(refused.contains("limit is 2"), "{}", refused);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_profile_can_be_written_by_the_tool_and_a_broken_one_is_refused() {
        let (ctx, dir) = ctx();
        let said = profile_upsert(&ctx, "tiny", &json!({
            "workers": [{"name": "one", "role": "scout", "model": "main"}],
            "report_every": "2m", "stuck_after": "4 reports", "worktree": false
        })).unwrap();
        assert!(said.contains("report every 120s, stuck after 4"), "{}", said);
        let p = profile(&ctx, "tiny").unwrap();
        assert_eq!((p.report_every, p.stuck_after, p.workers.len()), (120, 4, 1));
        assert!(profiles_text(&ctx).contains("report every 120s, stuck after 4"));
        let err = profile_upsert(&ctx, "broken", &json!({"workers": []})).unwrap_err();
        assert!(err.contains("no worker part"), "{}", err);
        assert!(!profile_dir(&ctx).join("broken.json").is_file());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_part_at_the_depth_limit_is_refused_and_a_shallow_part_spawns_a_subcluster() {
        let (mut ctx, dir) = ctx();
        let probe = dir.join("hello.txt").to_string_lossy().to_string();
        let (address, _seen) = crate::llm::fake::serve(vec![
            (200, text_reply("the leaf answer")),
            (200, tool_reply("read", json!({"path": probe}))),
            (200, text_reply("PASS: the leaf answer is in the memory")),
        ]);
        fake_models(&mut ctx, &address);
        write_profile(&ctx, "tree", json!({
            "name": "tree",
            "workers": [{"name": "root-part", "role": "scout", "model": "main"}],
            "validator": {"name": "root-check", "role": "judge", "model": "judge"},
            "max_models": 12, "max_depth": 3, "report_every": 0, "retries": 0, "worktree": false
        }));
        write_profile(&ctx, "leaf", json!({
            "name": "leaf",
            "workers": [{"name": "leaf-part", "role": "scout", "model": "main"}],
            "validator": {"name": "leaf-check", "role": "judge", "model": "judge"},
            "max_models": 2, "max_depth": 1, "report_every": 0, "retries": 0, "worktree": false
        }));
        let p = profile(&ctx, "tree").unwrap();
        let root = new_cluster(&ctx, &p, "root", "root task", None, Vec::new(), 0).unwrap();
        let one = new_cluster(&ctx, &p, "one", "depth one", Some(&root.id), Vec::new(), 1).unwrap();
        let two = new_cluster(&ctx, &p, "two", "depth two", Some(&one.id), Vec::new(), 2).unwrap();
        let three = new_cluster(&ctx, &p, "three", "depth three", Some(&two.id), Vec::new(), 3).unwrap();
        assert_eq!(three.depth, 3);

        let refused = with_cluster(&three.id, || spawn(&ctx, "leaf", Some("too-deep"), "go deeper")).unwrap_err();
        assert!(refused.contains("depth 3 of 3") && refused.contains("max_depth"), "{}", refused);
        assert!(list(&ctx).iter().all(|c| c.name != "too-deep"), "no child may be created");

        let said = with_cluster(&one.id, || spawn(&ctx, "leaf", Some("leaf1"), "answer the parent")).unwrap();
        assert!(said.contains("leaf1") && said.contains("state done"), "{}", said);
        let child = list(&ctx).into_iter().find(|c| c.name == "leaf1").expect("the child is in the tree");
        assert_eq!((child.parent.as_deref(), child.depth, child.state.as_str()), (Some(one.id.as_str()), 2, "done"));
        let parent_memory = fs::read_to_string(memory_path(&ctx, &one.id)).unwrap();
        assert!(parent_memory.contains("## child leaf1") && parent_memory.contains("depth: 2")
                && parent_memory.contains("report: S:done"), "{}", parent_memory);
        let children = fs::read_to_string(cluster_dir(&ctx, &one.id).join("children.jsonl")).unwrap();
        assert!(children.contains(&child.id) && children.contains("\"depth\":2"), "{}", children);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_root_max_models_stops_a_spawn_that_would_grow_the_tree_too_far() {
        let (ctx, dir) = ctx();
        write_profile(&ctx, "root3", json!({
            "name": "root3",
            "workers": [{"name": "root-part", "role": "scout", "model": "main"}],
            "max_models": 3, "max_depth": 3, "report_every": 0, "worktree": false
        }));
        write_profile(&ctx, "kid", json!({
            "name": "kid",
            "workers": [{"name": "kid-part", "role": "scout", "model": "main"}],
            "max_models": 1, "max_depth": 1, "report_every": 0, "worktree": false
        }));
        let p = profile(&ctx, "root3").unwrap();
        let root = new_cluster(&ctx, &p, "root3", "root task", None, Vec::new(), 0).unwrap();
        assert_eq!(part_count(&p), 1);
        spawn_create(&ctx, &root.id, "kid", Some("kid-a"), "first").expect("one of three is below the limit");
        spawn_create(&ctx, &root.id, "kid", Some("kid-b"), "second").expect("three of three is exactly the limit");
        let refused = spawn_create(&ctx, &root.id, "kid", Some("kid-c"), "third").unwrap_err();
        assert!(refused.contains("max_models") && refused.contains("3 of 3"), "{}", refused);
        assert!(list(&ctx).iter().all(|c| c.name != "kid-c"), "the refused child is not on disk");
        assert_eq!(tree_models(&ctx, &root.id), 3);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_cluster_spawn_tool_is_open_to_the_worker_and_the_lead() {
        assert!(crate::route::may_call("worker", "cluster_spawn"));
        assert!(crate::route::may_call("lead", "cluster_spawn"));
        assert!(!crate::route::may_call("judge", "cluster_spawn"));
        assert!(!crate::route::may_call("scout", "cluster_spawn"));
        assert!(crate::spec::get("cluster_spawn").is_some());
    }

    fn timed_parts(max_parallel: u64) -> Duration {
        let (mut ctx, dir) = ctx();
        let probe = dir.join("hello.txt").to_string_lossy().to_string();
        let (address, _seen) = crate::llm::fake::serve_parallel(vec![
            (200, text_reply("one done"), 1_000),
            (200, text_reply("two done"), 1_000),
            (200, text_reply("three done"), 1_000),
            (200, tool_reply("read", json!({"path": probe})), 0),
            (200, text_reply("PASS: all three parts are in the memory"), 0),
        ]);
        fake_models(&mut ctx, &address);
        write_profile(&ctx, "wide", json!({
            "name": "wide",
            "workers": [
                {"name": "one", "role": "scout", "model": "main"},
                {"name": "two", "role": "scout", "model": "main"},
                {"name": "three", "role": "scout", "model": "main"}
            ],
            "validator": {"name": "check", "role": "judge", "model": "judge"},
            "max_parallel": max_parallel, "report_every": 0, "stuck_after": 3, "worktree": false
        }));
        let p = profile(&ctx, "wide").unwrap();
        assert_eq!(p.max_parallel, max_parallel as usize);
        let c = new_cluster(&ctx, &p, "wide", "do three parts", None, Vec::new(), 0).unwrap();
        let started = std::time::Instant::now();
        let out = run_cluster(&ctx, &c.id).unwrap();
        let elapsed = started.elapsed();
        assert!(out.contains("done"), "{}", out);
        let memory = fs::read_to_string(memory_path(&ctx, &c.id)).unwrap();
        for name in ["one", "two", "three"] {
            assert!(memory.contains(&format!("## {}", name)), "memory misses {}: {}", name, memory);
        }
        let _ = fs::remove_dir_all(&dir);
        elapsed
    }

    #[test]
    fn three_parts_run_at_the_same_time_up_to_max_parallel() {
        let parallel = timed_parts(3);
        let sequential = timed_parts(1);
        assert!(parallel < Duration::from_millis(2_500), "three parts in parallel took {:?}", parallel);
        assert!(parallel + Duration::from_millis(1_200) < sequential,
                "parallel {:?} must be clearly faster than sequential {:?}", parallel, sequential);
    }

    #[test]
    fn part_dependencies_are_parsed_and_a_cycle_is_refused() {
        let (ctx, dir) = ctx();
        write_profile(&ctx, "chain", json!({
            "name": "chain",
            "workers": [
                {"name": "first", "role": "scout", "model": "main"},
                {"name": "second", "role": "scout", "model": "main", "needs": ["first"]}
            ],
            "max_parallel": 2, "worktree": false
        }));
        let p = profile(&ctx, "chain").unwrap();
        assert_eq!(p.workers[1].depends_on, vec!["first".to_string()]);
        write_profile(&ctx, "unknown-need", json!({
            "name": "unknown-need",
            "workers": [{"name": "one", "role": "scout", "model": "main", "needs": ["ghost"]}],
            "worktree": false
        }));
        assert!(profile(&ctx, "unknown-need").unwrap_err().contains("unknown part"));
        write_profile(&ctx, "cycle", json!({
            "name": "cycle",
            "workers": [
                {"name": "a", "role": "scout", "model": "main", "needs": ["b"]},
                {"name": "b", "role": "scout", "model": "main", "needs": ["a"]}
            ],
            "worktree": false
        }));
        assert!(profile(&ctx, "cycle").unwrap_err().contains("cycle"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_quorum_part_finishes_on_agreement_without_a_decider_call() {
        let (mut ctx, dir) = ctx();
        let probe = dir.join("hello.txt").to_string_lossy().to_string();
        let (address, seen) = crate::llm::fake::serve(vec![
            (200, text_reply("alpha")),
            (200, text_reply("alpha")),
            (200, tool_reply("read", json!({"path": probe}))),
            (200, text_reply("PASS: both answers are in the memory")),
        ]);
        fake_models(&mut ctx, &address);
        write_profile(&ctx, "vote", json!({
            "name": "vote",
            "workers": [{"name": "one", "role": "scout", "model": "main", "task": "Answer the question.", "quorum": 2}],
            "validator": {"name": "check", "role": "judge", "model": "judge"},
            "second_model": "weak", "decider_model": "main",
            "max_models": 2, "max_depth": 1, "retries": 0, "worktree": false
        }));
        let p = profile(&ctx, "vote").unwrap();
        assert_eq!(quorum_of(&p, &p.workers[0]), 2);
        let c = new_cluster(&ctx, &p, "vote", "answer the question", None, Vec::new(), 0).unwrap();
        let out = run_cluster(&ctx, &c.id).unwrap();
        assert!(out.contains("done"), "{}", out);
        let c = load(&ctx, &c.id).unwrap();
        assert_eq!(c.state.as_str(), "done");
        assert_eq!((c.quorum_agreed, c.quorum_escalated), (1, 0));
        assert_eq!(seen.lock().unwrap().len(), 4, "an agreement asks no decider: {:?}", seen.lock().unwrap().len());
        let bodies: Vec<String> = seen.lock().unwrap().iter().map(|s| s.body.to_string()).collect();
        assert!(bodies[1].contains("You have no tools"), "the second answer is independent: {}", bodies[1]);
        assert!(!bodies[1].contains("alpha"), "the second answer must not see the first: {}", bodies[1]);
        let memory = fs::read_to_string(memory_path(&ctx, &c.id)).unwrap();
        assert!(memory.contains("## quorum · one") && memory.contains("agreed"), "{}", memory);
        let stream = fs::read_to_string(cluster_dir(&ctx, &c.id).join("reports.jsonl")).unwrap();
        assert!(stream.contains("QUORUM one · agreed"), "{}", stream);
        assert!(status_text(&ctx, Some(&c.id)).contains("Quorum: 1 agreed · 0 escalated"));
        let stored: Value = serde_json::from_str(&fs::read_to_string(cluster_file(&ctx, &c.id)).unwrap()).unwrap();
        assert_eq!((stored["quorum"]["agreed"].as_u64(), stored["quorum"]["escalated"].as_u64()), (Some(1), Some(0)));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_quorum_disagreement_is_decided_once_by_the_large_model() {
        let (mut ctx, dir) = ctx();
        let probe = dir.join("hello.txt").to_string_lossy().to_string();
        let (address, seen) = crate::llm::fake::serve(vec![
            (200, text_reply("alpha")),
            (200, text_reply("beta")),
            (200, text_reply("alpha")),
            (200, tool_reply("read", json!({"path": probe}))),
            (200, text_reply("PASS: the decided answer is in the memory")),
        ]);
        fake_models(&mut ctx, &address);
        write_profile(&ctx, "vote2", json!({
            "name": "vote2",
            "workers": [{"name": "one", "role": "scout", "model": "main", "task": "Answer the question.", "quorum": 2}],
            "validator": {"name": "check", "role": "judge", "model": "judge"},
            "second_model": "weak", "decider_model": "main",
            "max_models": 2, "max_depth": 1, "retries": 0, "worktree": false
        }));
        let p = profile(&ctx, "vote2").unwrap();
        let c = new_cluster(&ctx, &p, "vote2", "answer the question", None, Vec::new(), 0).unwrap();
        let out = run_cluster(&ctx, &c.id).unwrap();
        assert!(out.contains("done"), "{}", out);
        let c = load(&ctx, &c.id).unwrap();
        assert_eq!(c.state.as_str(), "done");
        assert_eq!((c.quorum_agreed, c.quorum_escalated), (0, 1));
        assert_eq!(c.parts[0].output, "alpha", "the decider answer is the part result");
        assert_eq!(seen.lock().unwrap().len(), 5, "one second answer and one decider call");
        let bodies: Vec<String> = seen.lock().unwrap().iter().map(|s| s.body.to_string()).collect();
        assert!(bodies[2].contains("alpha") && bodies[2].contains("beta"),
                "the decider sees both answers: {}", bodies[2]);
        let memory = fs::read_to_string(memory_path(&ctx, &c.id)).unwrap();
        assert!(memory.contains("escalated to"), "{}", memory);
        assert!(memory.contains("alpha") && memory.contains("beta"), "{}", memory);
        let stream = fs::read_to_string(cluster_dir(&ctx, &c.id).join("reports.jsonl")).unwrap();
        assert!(stream.contains("QUORUM one · disagree"), "{}", stream);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn quorum_is_one_or_two_and_a_cluster_file_can_set_it() {
        let p = parse_cluster("$vote(parts=[scout, validator], quorum=2, worktree=false)").unwrap();
        assert_eq!(p.quorum, 2);
        assert_eq!(quorum_of(&p, &p.workers[0]), 2);
        for bad in ["$vote(parts=[scout], quorum=0)", "$vote(parts=[scout], quorum=3)", "$vote(parts=[scout], quorum=two)"] {
            assert!(parse_cluster(bad).is_err(), "{}", bad);
        }
        let own = parse_profile(&json!({
            "name": "own", "quorum": 2,
            "workers": [{"name": "one", "role": "scout", "model": "main", "quorum": 1}],
            "worktree": false
        }), "own").unwrap();
        assert_eq!(quorum_of(&own, &own.workers[0]), 1, "a part may turn the profile quorum off");
    }

    #[test]
    fn a_declared_shadow_runs_beside_the_main_cluster_and_records_a_full_match() {
        let (mut ctx, dir) = ctx();
        let probe = dir.join("hello.txt").to_string_lossy().to_string();
        let (address, _seen) = crate::llm::fake::serve(vec![
            (200, text_reply("same answer")),
            (200, tool_reply("read", json!({"path": probe.clone()}))),
            (200, text_reply("PASS: the main answer is checked")),
            (200, text_reply("same answer")),
            (200, tool_reply("read", json!({"path": probe}))),
            (200, text_reply("PASS: the shadow answer is checked")),
        ]);
        fake_models(&mut ctx, &address);
        write_profile(&ctx, "large", json!({
            "name": "large",
            "workers": [{"name": "one", "role": "scout", "model": "main", "task": "Answer the question."}],
            "validator": {"name": "check", "role": "judge", "model": "judge"},
            "max_models": 2, "max_depth": 1, "retries": 0, "worktree": false
        }));
        write_profile(&ctx, "small", json!({
            "name": "small",
            "workers": [{"name": "one", "role": "scout", "model": "weak", "task": "Answer the question."}],
            "validator": {"name": "check", "role": "judge", "model": "judge"},
            "shadow_of": "large", "max_models": 2, "max_depth": 1, "retries": 0, "worktree": false
        }));
        assert_eq!(profile(&ctx, "small").unwrap().shadow_of.as_deref(), Some("large"));
        let said = orchestrate_with(&ctx, "answer the question", Some("large"), false, None).unwrap();
        assert!(said.contains("SHADOW") && said.contains("match 100.0%"), "{}", said);
        let clusters = list(&ctx);
        assert_eq!(clusters.len(), 2, "{}", said);
        let main = clusters.iter().find(|c| c.profile == "large").unwrap();
        let shadow = clusters.iter().find(|c| c.shadow).expect("the shadow is stored");
        assert_eq!((main.state.as_str(), shadow.state.as_str()), ("done", "done"));
        assert_eq!((shadow.shadow_of.as_str(), main.match_rate, shadow.match_rate),
                   (main.id.as_str(), Some(1.0), Some(1.0)));
        let stored: Value = serde_json::from_str(&fs::read_to_string(shadow_path(&ctx)).unwrap()).unwrap();
        let run = &stored["runs"][0];
        assert_eq!((run["main"].as_str(), run["shadow"].as_str()), (Some(main.id.as_str()), Some(shadow.id.as_str())));
        assert_eq!((run["outputs"]["matched"].as_u64(), run["outputs"]["total"].as_u64()), (Some(1), Some(1)));
        assert_eq!((run["checks"]["matched"].as_u64(), run["checks"]["total"].as_u64()), (Some(2), Some(2)));
        assert!((run["match_rate"].as_f64().unwrap() - 1.0).abs() < 1e-9);
        assert!(cluster_dir(&ctx, &main.id).join("shadow.txt").is_file());
        let memory = fs::read_to_string(memory_path(&ctx, &main.id)).unwrap();
        assert!(memory.contains("## shadow ·") && memory.contains("not merged"), "{}", memory);
        let stream = fs::read_to_string(cluster_dir(&ctx, &main.id).join("reports.jsonl")).unwrap();
        assert!(stream.contains("SHADOW") && stream.contains("match 100.0%"), "{}", stream);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_shadow_that_differs_lowers_the_match_rate() {
        let (mut ctx, dir) = ctx();
        let probe = dir.join("hello.txt").to_string_lossy().to_string();
        let (address, _seen) = crate::llm::fake::serve(vec![
            (200, text_reply("alpha")),
            (200, tool_reply("read", json!({"path": probe.clone()}))),
            (200, text_reply("PASS: the main answer is checked")),
            (200, text_reply("beta")),
            (200, tool_reply("read", json!({"path": probe}))),
            (200, text_reply("PASS: the shadow answer is checked")),
        ]);
        fake_models(&mut ctx, &address);
        write_profile(&ctx, "large", json!({
            "name": "large",
            "workers": [{"name": "one", "role": "scout", "model": "main", "task": "Answer the question."}],
            "validator": {"name": "check", "role": "judge", "model": "judge"},
            "max_models": 2, "max_depth": 1, "retries": 0, "worktree": false
        }));
        write_profile(&ctx, "small", json!({
            "name": "small",
            "workers": [{"name": "one", "role": "scout", "model": "weak", "task": "Answer the question."}],
            "validator": {"name": "check", "role": "judge", "model": "judge"},
            "shadow_of": "large", "max_models": 2, "max_depth": 1, "retries": 0, "worktree": false
        }));
        let said = orchestrate_with(&ctx, "answer the question", Some("large"), false, None).unwrap();
        assert!(said.contains("outputs 0/1") && said.contains("checks 2/2"), "{}", said);
        let main = list(&ctx).into_iter().find(|c| c.profile == "large").unwrap();
        let rate = main.match_rate.expect("the main cluster carries the match rate");
        assert!((rate - 2.0 / 3.0).abs() < 1e-9, "match rate was {}", rate);
        let stored: Value = serde_json::from_str(&fs::read_to_string(shadow_path(&ctx)).unwrap()).unwrap();
        assert_eq!(stored["runs"][0]["outputs"]["matched"].as_u64(), Some(0));
        assert!((stored["runs"][0]["match_rate"].as_f64().unwrap() - rate).abs() < 1e-9);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_shadow_never_merges_its_files_into_the_live_tree() {
        let (mut ctx, dir) = ctx();
        let (address, _seen) = crate::llm::fake::serve(vec![
            (200, tool_reply("write", json!({"path": "hello.txt", "text": "main\n"}))),
            (200, text_reply("main wrote hello.txt")),
            (200, tool_reply("run", json!({"command": "cat hello.txt"}))),
            (200, text_reply("PASS: main wrote the file")),
            (200, tool_reply("write", json!({"path": "hello.txt", "text": "shadow\n"}))),
            (200, text_reply("shadow wrote hello.txt")),
            (200, tool_reply("run", json!({"command": "cat hello.txt"}))),
            (200, text_reply("PASS: shadow wrote the file")),
        ]);
        fake_models(&mut ctx, &address);
        write_profile(&ctx, "big", json!({
            "name": "big",
            "workers": [{"name": "coder", "role": "worker", "model": "main", "task": "Write hello.txt."}],
            "validator": {"name": "check", "role": "judge", "model": "judge"},
            "max_models": 2, "max_depth": 1, "retries": 0
        }));
        write_profile(&ctx, "little", json!({
            "name": "little",
            "workers": [{"name": "coder", "role": "worker", "model": "weak", "task": "Write hello.txt."}],
            "validator": {"name": "check", "role": "judge", "model": "judge"},
            "shadow_of": "big", "max_models": 2, "max_depth": 1, "retries": 0, "worktree": false
        }));
        let said = orchestrate_with(&ctx, "write hello.txt", Some("big"), false, None).unwrap();
        assert!(said.contains("SHADOW"), "{}", said);
        assert_eq!(fs::read_to_string(dir.join("hello.txt")).unwrap(), "main\n",
                   "only the main cluster may reach the live tree: {}", said);
        let shadow = list(&ctx).into_iter().find(|c| c.shadow).expect("the shadow ran");
        assert_eq!(shadow.state.as_str(), "done");
        assert!(!shadow.worktree.is_empty(), "a writing shadow always gets its own worktree");
        assert_eq!(fs::read_to_string(Path::new(&shadow.worktree).join("hello.txt")).unwrap(), "shadow\n");
        assert!(shadow.match_rate.is_some());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_orchestration_flag_runs_a_shadow_without_a_shadow_of_field() {
        let (mut ctx, dir) = ctx();
        let probe = dir.join("hello.txt").to_string_lossy().to_string();
        let (address, _seen) = crate::llm::fake::serve(vec![
            (200, text_reply("same answer")),
            (200, tool_reply("read", json!({"path": probe.clone()}))),
            (200, text_reply("PASS: the main answer is checked")),
            (200, text_reply("same answer")),
            (200, tool_reply("read", json!({"path": probe}))),
            (200, text_reply("PASS: the shadow answer is checked")),
        ]);
        fake_models(&mut ctx, &address);
        for (name, model) in [("large", "main"), ("small", "weak")] {
            write_profile(&ctx, name, json!({
                "name": name,
                "workers": [{"name": "one", "role": "scout", "model": model, "task": "Answer the question."}],
                "validator": {"name": "check", "role": "judge", "model": "judge"},
                "max_models": 2, "max_depth": 1, "retries": 0, "worktree": false
            }));
        }
        let said = orchestrate_with(&ctx, "answer the question", Some("large"), false, Some("small")).unwrap();
        assert!(said.contains("SHADOW") && said.contains("match 100.0%"), "{}", said);
        assert_eq!(list(&ctx).iter().filter(|c| c.shadow).count(), 1);
        let refused = orchestrate_with(&ctx, "answer the question", Some("large"), false, Some("large")).unwrap_err();
        assert!(refused.contains("main profile itself"), "{}", refused);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_tool_log_becomes_the_evidence_line() {
        let dir = std::env::temp_dir().join(format!("penelope-cluster-log-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tools.jsonl");
        fs::write(&path, [
            json!({"name": "read", "args": {"path": "memory.md"}, "ok": true, "result": "1  task"}).to_string(),
            json!({"name": "run", "args": {"command": "cargo test"}, "ok": true, "result": "exit code 0\n41 passed"}).to_string(),
        ].join("\n")).unwrap();
        let checks = checks_from_log(&path);
        assert_eq!(checks.len(), 2);
        assert_eq!(evidence_of(&checks), "cargo test ok");
        fs::write(&path, json!({"name": "run", "args": {"command": "cat missing.txt"}, "ok": true, "result": "exit code 1\nno such file"}).to_string()).unwrap();
        assert_eq!(evidence_of(&checks_from_log(&path)), "cat missing.txt failed");
        assert!(!checks_from_log(&path)[0].ok);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn asking_a_question_lands_in_both_memories_and_the_report_stream() {
        let (address, seen) = crate::llm::fake::serve(vec![]);
        let (mut ctx, dir) = ctx();
        fake_models(&mut ctx, &address);
        let p = profile(&ctx, "research").unwrap();
        let target = new_cluster(&ctx, &p, "research", "find design options", None, Vec::new(), 0).unwrap();
        let mut target = load(&ctx, &target.id).unwrap();
        target.state = "done".to_string();
        target.output = "Option A: reuse the router. Option B: a new queue.".to_string();
        save(&ctx, &target).unwrap();
        let asker = new_cluster(&ctx, &p, "dev", "build the editor", None, Vec::new(), 0).unwrap();

        let said = with_cluster(&asker.id, || ask(&ctx, &target.id, "Which design should we take?")).unwrap();
        assert!(said.contains("Option A") && said.contains("output"), "{}", said);
        let target_memory = fs::read_to_string(memory_path(&ctx, &target.id)).unwrap();
        assert!(target_memory.contains("Which design should we take?") && target_memory.contains("question from dev"),
                "{}", target_memory);
        let asker_memory = fs::read_to_string(memory_path(&ctx, &asker.id)).unwrap();
        assert!(asker_memory.contains("Option A") && asker_memory.contains("answer from research")
                && asker_memory.contains("Which design should we take?"), "{}", asker_memory);
        let stream = fs::read_to_string(cluster_dir(&ctx, &asker.id).join("reports.jsonl")).unwrap();
        assert!(stream.contains(" ASK ") && stream.contains("CODE:design_options") && stream.contains("HOPS:1") && stream.contains("VISITED:"), "{}", stream);
        assert!(!stream.contains("Which design") && !stream.contains("Option A"), "the report stream must not carry question or answer text: {}", stream);
        let status = status_text(&ctx, Some(&asker.id));
        assert!(status.contains("Links:") && status.contains("Which design"), "{}", status);
        assert!(seen.lock().unwrap().is_empty(), "cluster_ask must not start a model call");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_running_target_answers_from_memory_and_names_the_source() {
        let (ctx, dir) = ctx();
        let p = profile(&ctx, "research").unwrap();
        let target = new_cluster(&ctx, &p, "research", "find design options", None, Vec::new(), 0).unwrap();
        let mut target = load(&ctx, &target.id).unwrap();
        target.state = "working".to_string();
        target.output.clear();
        save(&ctx, &target).unwrap();
        append_memory(&ctx, &target, "\n## research1 · scout\n\nThe design options are X and Y.\n");
        let asker = new_cluster(&ctx, &p, "dev", "build the editor", None, Vec::new(), 0).unwrap();

        let said = with_cluster(&asker.id, || ask(&ctx, &target.name, "Which design?")).unwrap();
        assert!(said.contains("X and Y") && said.contains("memory") && said.contains("working"), "{}", said);
        let asker_memory = fs::read_to_string(memory_path(&ctx, &asker.id)).unwrap();
        assert!(asker_memory.contains("The design options are X and Y."), "{}", asker_memory);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn asking_an_unknown_or_a_done_empty_cluster_is_a_clear_error() {
        let (ctx, dir) = ctx();
        let p = profile(&ctx, "research").unwrap();
        let asker = new_cluster(&ctx, &p, "dev", "build the editor", None, Vec::new(), 0).unwrap();
        let unknown = with_cluster(&asker.id, || ask(&ctx, "ghost", "anything?")).unwrap_err();
        assert!(unknown.contains("no cluster ghost") && unknown.contains("known clusters"), "{}", unknown);

        let empty = new_cluster(&ctx, &p, "empty", "nothing to say", None, Vec::new(), 0).unwrap();
        let mut empty = load(&ctx, &empty.id).unwrap();
        empty.state = "done".to_string();
        empty.output.clear();
        save(&ctx, &empty).unwrap();
        let refused = with_cluster(&asker.id, || ask(&ctx, &empty.id, "what is your answer?")).unwrap_err();
        assert!(refused.contains("is done but has no answer") && refused.contains("no part result"), "{}", refused);
        assert!(fs::read_to_string(memory_path(&ctx, &empty.id)).unwrap().contains("what is your answer?"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_cluster_ask_tool_is_open_to_the_worker_the_lead_and_the_judge() {
        assert!(crate::route::may_call("worker", "cluster_ask"));
        assert!(crate::route::may_call("lead", "cluster_ask"));
        assert!(crate::route::may_call("judge", "cluster_ask"));
        assert!(!crate::route::may_call("scout", "cluster_ask"));
        assert!(crate::spec::get("cluster_ask").is_some());
    }

    #[test]
    fn a_before_prompt_cluster_rewrites_the_task_before_the_worker_starts() {
        let (mut ctx, dir) = ctx();
        let probe = dir.join("hello.txt").to_string_lossy().to_string();
        let (address, seen) = crate::llm::fake::serve(vec![
            (200, text_reply("REWRITE: build the bridge and run cargo test")),
            (200, tool_reply("read", json!({"path": probe.clone()}))),
            (200, text_reply("PASS: the rewrite keeps the task")),
            (200, text_reply("built the bridge")),
            (200, tool_reply("read", json!({"path": probe}))),
            (200, text_reply("PASS: the worker did the rewritten task")),
        ]);
        fake_models(&mut ctx, &address);
        write_profile(&ctx, "rewriter", json!({
            "name": "rewriter",
            "workers": [{"name": "rewrite", "role": "condenser", "model": "weak", "task": "Rewrite the task."}],
            "validator": {"name": "check", "role": "judge", "model": "judge"},
            "max_models": 2, "max_depth": 1, "max_steps": 4, "retries": 0, "worktree": false
        }));
        write_profile(&ctx, "dev", json!({
            "name": "dev",
            "workers": [{"name": "coder", "role": "scout", "model": "main", "task": "Do the rewritten task."}],
            "validator": {"name": "check", "role": "judge", "model": "judge"},
            "before": "rewriter", "max_models": 4, "max_depth": 2, "max_steps": 8, "retries": 0, "worktree": false
        }));
        let p = profile(&ctx, "dev").unwrap();
        assert_eq!(p.before.as_deref(), Some("rewriter"));
        let c = new_cluster(&ctx, &p, "dev", "build a bridge across the north river", None, Vec::new(), 0).unwrap();
        let out = run_cluster(&ctx, &c.id).unwrap();
        assert!(out.contains("done"), "{}", out);
        let c = load(&ctx, &c.id).unwrap();
        assert_eq!((c.state.as_str(), c.task.as_str()), ("done", "REWRITE: build the bridge and run cargo test"));
        assert_eq!(c.parts[0].output, "built the bridge");
        let prompt = fs::read_to_string(cluster_dir(&ctx, &c.id).join("prompt.txt")).unwrap();
        assert!(prompt.contains("north river") && prompt.contains("REWRITE: build the bridge"), "{}", prompt);
        let memory = fs::read_to_string(memory_path(&ctx, &c.id)).unwrap();
        assert!(memory.contains("## before prompt · rewriter") && memory.contains("Task: REWRITE: build the bridge"), "{}", memory);
        let clusters = list(&ctx);
        assert_eq!(clusters.len(), 2, "the prompt cluster stays in the tree");
        let prompt_cluster = clusters.iter().find(|x| x.name == "dev-prompt").expect("the prompt cluster is stored");
        assert_eq!((prompt_cluster.state.as_str(), prompt_cluster.parent.as_deref()), ("done", Some(c.id.as_str())));
        let bodies: Vec<String> = seen.lock().unwrap().iter().map(|s| s.body.to_string()).collect();
        assert!(bodies[0].contains("north river"), "the prompt cluster gets the original task: {}", bodies[0]);
        assert!(bodies[3].contains("REWRITE: build the bridge and run cargo test"),
                "the worker gets the rewritten task: {}", bodies[3]);
        assert!(!bodies[3].contains("north river"),
                "the worker does not get the orchestrator's original text: {}", bodies[3]);
        assert_ne!(bodies[0], bodies[3]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_before_chain_and_a_writing_prompt_profile_are_refused() {
        let (ctx, dir) = ctx();
        write_profile(&ctx, "self", json!({
            "name": "self",
            "workers": [{"name": "one", "role": "scout", "model": "main"}],
            "before": "self", "worktree": false
        }));
        let selfp = profile(&ctx, "self").unwrap();
        assert!(before_profile(&ctx, &selfp).unwrap_err().contains("profile itself"), "{}", before_profile(&ctx, &selfp).unwrap_err());

        write_profile(&ctx, "inner", json!({
            "name": "inner",
            "workers": [{"name": "one", "role": "scout", "model": "main"}],
            "before": "prompt", "worktree": false
        }));
        write_profile(&ctx, "outer", json!({
            "name": "outer",
            "workers": [{"name": "one", "role": "scout", "model": "main"}],
            "before": "inner", "worktree": false
        }));
        let outer = profile(&ctx, "outer").unwrap();
        assert!(before_profile(&ctx, &outer).unwrap_err().contains("chain of prompt clusters"),
                "{}", before_profile(&ctx, &outer).unwrap_err());

        write_profile(&ctx, "writer", json!({
            "name": "writer",
            "workers": [{"name": "coder", "role": "worker", "model": "main"}],
            "worktree": false
        }));
        write_profile(&ctx, "dev2", json!({
            "name": "dev2",
            "workers": [{"name": "one", "role": "scout", "model": "main"}],
            "before": "writer", "worktree": false
        }));
        let dev2 = profile(&ctx, "dev2").unwrap();
        let err = before_profile(&ctx, &dev2).unwrap_err();
        assert!(err.contains("role worker") && err.contains("which may write"), "{}", err);

        write_profile(&ctx, "leader", json!({
            "name": "leader",
            "workers": [{"name": "boss", "role": "lead", "model": "main"}],
            "worktree": false
        }));
        write_profile(&ctx, "dev3", json!({
            "name": "dev3",
            "workers": [{"name": "one", "role": "scout", "model": "main"}],
            "before": "leader", "worktree": false
        }));
        let dev3 = profile(&ctx, "dev3").unwrap();
        let err = before_profile(&ctx, &dev3).unwrap_err();
        assert!(err.contains("role lead") && err.contains("condenser, scout or judge"), "{}", err);
        let _ = fs::remove_dir_all(&dir);
    }

    fn human_push_profile(ctx: &Ctx) {
        write_profile(ctx, "push", json!({
            "name": "push",
            "workers": [
                {"name": "approve", "role": "human", "slot": "owner",
                 "task": "Allow the git push to origin main? Answer yes or no."},
                {"name": "push", "role": "scout", "model": "main", "needs": ["approve"],
                 "task": "Run git push to origin main."}
            ],
            "validator": {"name": "check", "role": "judge", "model": "judge"},
            "max_models": 4, "max_depth": 1, "retries": 0, "worktree": false
        }));
    }

    #[test]
    fn a_human_part_that_says_no_stops_the_cluster_before_the_push() {
        let (mut ctx, dir) = ctx();
        std::env::remove_var("PENELOPE_APPROVE_DIR");
        let (address, seen) = crate::llm::fake::serve(vec![(200, text_reply("this must not run"))]);
        fake_models(&mut ctx, &address);
        human_push_profile(&ctx);
        ctx.cfg["human"] = json!("no");
        let p = profile(&ctx, "push").unwrap();
        assert_eq!(p.workers[0].role, "human");
        let c = new_cluster(&ctx, &p, "push", "push the branch", None, Vec::new(), 0).unwrap();
        let out = run_cluster(&ctx, &c.id).unwrap();
        assert!(out.contains("blocked"), "{}", out);
        let c = load(&ctx, &c.id).unwrap();
        assert_eq!(c.state.as_str(), "blocked");
        assert!(c.error.contains("said no"), "{}", c.error);
        assert_eq!((c.parts[0].state.as_str(), c.parts[0].output.as_str()), ("failed", "no"));
        assert_eq!(c.parts[1].state.as_str(), "queued", "the push part never started");
        let memory = fs::read_to_string(memory_path(&ctx, &c.id)).unwrap();
        assert!(memory.contains("answer: no") && memory.contains("the cluster stops here"), "{}", memory);
        let line = fs::read_to_string(cluster_dir(&ctx, &c.id).join("report.txt")).unwrap();
        assert!(line.starts_with("S:blocked"), "the report names the blocked state: {}", line);
        assert!(seen.lock().unwrap().is_empty(), "a no must not start a model call");
        assert!(!dir.join("pushed.txt").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_human_part_without_an_answer_path_refuses_instead_of_assuming_yes() {
        let (mut ctx, dir) = ctx();
        std::env::remove_var("PENELOPE_APPROVE_DIR");
        let (address, seen) = crate::llm::fake::serve(vec![(200, text_reply("this must not run"))]);
        fake_models(&mut ctx, &address);
        human_push_profile(&ctx);
        ctx.cfg["human"] = Value::Null;
        let p = profile(&ctx, "push").unwrap();
        let c = new_cluster(&ctx, &p, "push", "push the branch", None, Vec::new(), 0).unwrap();
        let out = run_cluster(&ctx, &c.id).unwrap();
        assert!(out.contains("blocked"), "{}", out);
        let c = load(&ctx, &c.id).unwrap();
        assert!(c.error.contains("never assumes yes"), "{}", c.error);
        assert_eq!(c.parts[1].state.as_str(), "queued");
        assert!(seen.lock().unwrap().is_empty(), "an unanswered human part must not start a model call");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_human_part_cannot_be_the_validator() {
        let (ctx, dir) = ctx();
        write_profile(&ctx, "humancheck", json!({
            "name": "humancheck",
            "workers": [{"name": "one", "role": "scout", "model": "main"}],
            "validator": {"name": "me", "role": "human"},
            "worktree": false
        }));
        let err = profile(&ctx, "humancheck").unwrap_err();
        assert!(err.contains("validator cannot be a human"), "{}", err);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_worker_part_asks_another_cluster_and_reads_the_answer_in_its_next_turn() {
        let (mut ctx, dir) = ctx();
        let probe = dir.join("hello.txt").to_string_lossy().to_string();
        let p = profile(&ctx, "research").unwrap();
        let target = new_cluster(&ctx, &p, "research", "find design options", None, Vec::new(), 0).unwrap();
        let mut target = load(&ctx, &target.id).unwrap();
        target.state = "done".to_string();
        target.output = "Option A is the design".to_string();
        save(&ctx, &target).unwrap();
        let (address, seen) = crate::llm::fake::serve(vec![
            (200, tool_reply("cluster_ask", json!({"target": target.id, "question": "Which design?"}))),
            (200, text_reply("I take Option A")),
            (200, tool_reply("read", json!({"path": probe}))),
            (200, text_reply("PASS: the worker used the answer")),
        ]);
        fake_models(&mut ctx, &address);
        write_profile(&ctx, "asker", json!({
            "name": "asker",
            "workers": [{"name": "dev", "role": "worker", "model": "main",
                         "task": "Ask the research cluster which design to take, then say what you take."}],
            "validator": {"name": "check", "role": "judge", "model": "judge"},
            "max_models": 4, "max_depth": 1, "report_every": 0, "retries": 0, "worktree": false
        }));
        let asker_profile = profile(&ctx, "asker").unwrap();
        let asker = new_cluster(&ctx, &asker_profile, "dev", "build the editor", None, Vec::new(), 0).unwrap();
        let out = run_cluster(&ctx, &asker.id).unwrap();
        assert!(out.contains("done"), "{}", out);
        let memory = fs::read_to_string(memory_path(&ctx, &asker.id)).unwrap();
        assert!(memory.contains("Option A is the design") && memory.contains("Which design?"), "{}", memory);
        assert!(fs::read_to_string(memory_path(&ctx, &target.id)).unwrap().contains("Which design?"));
        let bodies: Vec<String> = seen.lock().unwrap().iter().map(|s| s.body.to_string()).collect();
        assert!(bodies.len() >= 2 && bodies[1].contains("Option A is the design"),
                "the answer reached the worker's next request: {:?}", bodies);
        let stream = fs::read_to_string(cluster_dir(&ctx, &asker.id).join("reports.jsonl")).unwrap();
        assert!(stream.contains(" ASK ") && stream.contains("CODE:design_options") && !stream.contains("Which design?"), "{}", stream);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_cluster_file_and_its_json_profile_are_the_same_profile() {
        let (ctx, dir) = ctx();
        let text = "$dev(parts=[researcher, coder*3, validator, tool], max_models=8, max_depth=3, budget_eur=2)";
        let from_file = parse_cluster(text).unwrap();
        assert_eq!(from_file.name, "dev");
        assert_eq!(from_file.workers.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
                   vec!["researcher", "coder", "coder2", "coder3", "tool"]);
        assert_eq!(from_file.validator.as_ref().map(|v| v.name.as_str()), Some("validator"));
        assert_eq!((from_file.max_models, from_file.max_depth, from_file.budget_eur), (8, 3, 2.0));
        assert!(from_file.worktree, "a coder part writes, so the default is a worktree");
        let from_json = parse_profile(&json!({
            "name": "dev",
            "workers": [
                template_value("researcher", "researcher"),
                template_value("coder", "coder"),
                template_value("coder2", "coder"),
                template_value("coder3", "coder"),
                template_value("tool", "tool")
            ],
            "validator": template_value("validator", "validator"),
            "max_models": 8, "max_depth": 3, "budget_eur": 2
        }), "dev").unwrap();
        assert_eq!(from_file, from_json);

        fs::create_dir_all(profile_dir(&ctx)).unwrap();
        fs::write(profile_dir(&ctx).join("dev.cluster"), text).unwrap();
        assert_eq!(profile(&ctx, "dev").unwrap(), from_file, "a .cluster file loads under its name");
        assert!(profile_names(&ctx).iter().any(|n| n == "dev"));
        assert!(profiles_text(&ctx).contains("$dev(parts=[researcher, coder*3, validator, tool]"), "{}", profiles_text(&ctx));
        // The direct path form works too.
        assert_eq!(profile(&ctx, profile_dir(&ctx).join("dev.cluster").to_str().unwrap()).unwrap(), from_file);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_cluster_file_refuses_unknown_parts_repeats_and_code() {
        for (text, want) in [
            ("$dev(parts=[coder], max_models=2", "ends with )"),
            ("$dev(parts=coder)", "square brackets"),
            ("$dev(parts=[])", "has no part"),
            ("$dev(parts=[coder*0])", "1 to 8"),
            ("$dev(parts=[coder*9])", "1 to 8"),
            ("$dev(parts=[coder, coder*2])", "used twice"),
            ("$dev(parts=[coder*2, coder2])", "Unknown part coder2"),
            ("$dev(parts=[wizard])", "Unknown part wizard"),
            ("$dev(parts=[validator, judge])", "validator is named twice"),
            ("$dev(parts=[validator*2])", "one validator"),
            ("$dev(parts=[validator])", "no worker part"),
            ("$dev(parts=[coder], nope=1)", "Unknown setting nope"),
            ("$dev(parts=[coder]; rm -rf /)", "not allowed"),
            ("$Dev(parts=[coder])", "name of a cluster"),
            ("$dev(parts=[coder], max_depth=9)", "max_depth is between"),
            ("$dev(parts=[coder], budget_eur=abc)", "budget_eur needs a number"),
            ("$dev(parts=[coder], max_models=2, max_models=3)", "appears twice"),
        ] {
            let err = parse_cluster(text).unwrap_err();
            assert!(err.contains(want), "{} -> {}", text, err);
        }
        let p = parse_cluster("$dev(parts=[coder*2, human*2, validator], before=prompt, worktree=false)").unwrap();
        assert_eq!(p.workers.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(), vec!["coder", "coder2", "human", "human2"]);
        assert_eq!(p.workers[2].role, "human");
        assert_eq!(p.before.as_deref(), Some("prompt"));
        assert!(!p.worktree);
    }

    #[test]
    fn a_dry_run_prints_parts_models_and_an_estimate_without_a_model_call() {
        let (mut ctx, dir) = ctx();
        let (address, seen) = crate::llm::fake::serve(vec![(200, text_reply("this must not run"))]);
        priced_models(&mut ctx, &address);
        fs::create_dir_all(profile_dir(&ctx)).unwrap();
        fs::write(profile_dir(&ctx).join("rewriter.cluster"),
                  "$rewriter(parts=[condenser, validator], max_models=2, max_depth=1, worktree=false)").unwrap();
        fs::write(profile_dir(&ctx).join("drydev.cluster"),
                  "$drydev(parts=[researcher, coder*2, validator, tool], before=rewriter, max_models=8, max_depth=3, budget_eur=2)").unwrap();
        let out = dry_run(&ctx, "build a bridge", Some("drydev")).unwrap();
        assert!(out.starts_with("Dry run"), "{}", out);
        assert!(out.contains("cluster rewriter · runs first and rewrites the task"), "{}", out);
        assert!(out.contains("cluster drydev"), "{}", out);
        for name in ["researcher", "coder", "coder2", "validator", "tool"] {
            assert!(out.contains(name), "{} misses {}: {}", out, name, out);
        }
        assert!(out.contains("model test-cluster"), "{}", out);
        assert!(out.contains("estimate:") && out.contains("tokens in") && out.contains("EUR"), "{}", out);
        assert!(seen.lock().unwrap().is_empty(), "the dry run must not call a model");
        assert!(list(&ctx).is_empty(), "the dry run must not create a cluster");
        assert!(!ctx.root.join("usage.json").exists(), "the dry run must not write usage");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_cluster_file_that_names_another_profile_is_refused_at_load() {
        let (ctx, dir) = ctx();
        fs::create_dir_all(profile_dir(&ctx)).unwrap();
        fs::write(profile_dir(&ctx).join("wrong.cluster"), "$other(parts=[coder, validator])").unwrap();
        let err = profile(&ctx, "wrong").unwrap_err();
        assert!(err.contains("names $other") && err.contains("called wrong"), "{}", err);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_part_prompt_holds_three_of_ten_linked_notes_and_no_stale_text() {
        let dir = std::env::temp_dir().join(format!("penelope-cluster-notes-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let write = |name: &str, status: &str, body: &str| {
            fs::write(dir.join(format!("{}.md", name)),
                      format!("---\nsource: test\ndate: 2026-11-26\nstatus: {}\n---\n{}\n", status, body)).unwrap();
        };
        // n1 links n2 and n3. n2 links the stale n7, which links n8. The other notes are a second island.
        write("n1", "open", "fact one [[n2]] [[n3]]");
        write("n2", "confirmed", "fact two [[n7]]");
        write("n3", "open", "fact three");
        write("n4", "open", "[[n5]]");
        write("n5", "open", "[[n6]]");
        write("n6", "open", "[[n9]]");
        write("n7", "stale", "STALE-SECRET [[n8]]");
        write("n8", "open", "after the stale note");
        write("n9", "open", "island");
        write("n0", "open", "[[n4]]");

        let text = prompt_notes_text(&dir, "n1", 2);
        assert_eq!(text.matches("[note ").count(), 3, "{}", text);
        assert!(text.contains("[note n1 ") && text.contains("[note n2 ") && text.contains("[note n3 "), "{}", text);
        assert!(text.contains("fact one") && text.contains("fact two") && text.contains("fact three"), "{}", text);
        assert!(!text.contains("STALE-SECRET") && !text.contains("[note n7 ") && !text.contains("[note n8 "),
                "a stale note reached the part prompt: {}", text);
        let _ = fs::remove_dir_all(&dir);
    }

    fn scoring_profile(ctx: &Ctx) {
        write_profile(ctx, "scored", json!({
            "name": "scored",
            "workers": [{"name": "coder-4", "role": "worker", "slot": "Telemach", "model": "main",
                         "task": "Do the task.", "task_type": "rust-fix"}],
            "validator": {"name": "check", "role": "judge", "slot": "Argos", "model": "judge", "task": "Check it."},
            "max_models": 2, "max_depth": 1, "retries": 0, "worktree": false
        }));
    }

    fn failed_pair() -> Vec<(u16, String)> {
        vec![(200, text_reply("worker done")), (200, text_reply("FAIL: not right"))]
    }

    fn passed_pair() -> Vec<(u16, String)> {
        vec![(200, text_reply("worker done")),
             (200, tool_reply("read", json!({"path": "Cargo.toml"}))),
             (200, text_reply("PASS: the check passed"))]
    }

    fn run_scored(ctx: &Ctx, name: &str) {
        let p = profile(ctx, "scored").unwrap();
        let c = new_cluster(ctx, &p, name, "score this part", None, Vec::new(), 0).unwrap();
        let _ = run_cluster(ctx, &c.id).unwrap();
    }

    #[test]
    fn four_rejections_of_five_in_the_validator_give_one_suggestion_naming_the_part() {
        let mut replies = Vec::new();
        for _ in 0..4 { replies.extend(failed_pair()); }
        replies.extend(passed_pair());
        let (address, _seen) = crate::llm::fake::serve(replies);
        let (mut ctx, dir) = ctx();
        fake_models(&mut ctx, &address);
        scoring_profile(&ctx);
        for i in 0..5 { run_scored(&ctx, &format!("score-{}", i)); }

        let table: Value = serde_json::from_str(&fs::read_to_string(scores_path(&ctx)).unwrap()).unwrap();
        assert_eq!(table["coder-4"]["rust-fix"]["rejected"], 4);
        assert_eq!(table["coder-4"]["rust-fix"]["accepted"], 1);
        let text = advise_text(&ctx, Some("scored")).unwrap();
        assert_eq!(text.lines().count(), 1, "{}", text);
        assert!(text.contains("coder-4") && text.contains("4 of 5") && text.contains("rust-fix"), "{}", text);
        // A cluster name, a profile name and one part name all narrow the same table.
        assert!(advise_text(&ctx, Some("score-0")).unwrap().contains("coder-4"));
        assert!(advise_text(&ctx, Some("coder-4")).unwrap().contains("coder-4"));
        assert!(advise_text(&ctx, None).unwrap().contains("coder-4"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn four_results_are_below_the_threshold_and_give_no_suggestion() {
        let mut replies = Vec::new();
        for _ in 0..4 { replies.extend(failed_pair()); }
        let (address, _seen) = crate::llm::fake::serve(replies);
        let (mut ctx, dir) = ctx();
        fake_models(&mut ctx, &address);
        scoring_profile(&ctx);
        for i in 0..4 { run_scored(&ctx, &format!("score-{}", i)); }
        let text = advise_text(&ctx, None).unwrap();
        assert!(!text.contains("was rejected"), "{}", text);
        assert!(text.contains("no suggestion") && text.contains("at least 5"), "{}", text);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn accepted_runs_do_not_create_a_false_suggestion() {
        let mut replies = Vec::new();
        for _ in 0..5 { replies.extend(passed_pair()); }
        let (address, _seen) = crate::llm::fake::serve(replies);
        let (mut ctx, dir) = ctx();
        fake_models(&mut ctx, &address);
        scoring_profile(&ctx);
        for i in 0..5 { run_scored(&ctx, &format!("score-{}", i)); }
        let table: Value = serde_json::from_str(&fs::read_to_string(scores_path(&ctx)).unwrap()).unwrap();
        assert_eq!(table["coder-4"]["rust-fix"]["accepted"], 5);
        assert_eq!(table["coder-4"]["rust-fix"]["rejected"], 0);
        let text = advise_text(&ctx, None).unwrap();
        assert!(!text.contains("was rejected"), "{}", text);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn advise_apply_writes_the_best_recorded_model_for_the_next_run() {
        let (ctx, dir) = ctx();
        write_profile(&ctx, "routed", json!({
            "name": "routed",
            "workers": [{"name": "coder-4", "role": "scout", "model": "weak", "task": "Do the task.", "task_type": "rust-fix"}],
            "validator": {"name": "check", "role": "judge", "model": "judge"},
            "max_models": 2, "max_depth": 1, "retries": 0, "worktree": false
        }));
        let scores = json!({"coder-4": {"rust-fix": {
            "accepted": 1, "rejected": 4,
            "models": {"weak": {"accepted": 1, "rejected": 4}, "main": {"accepted": 3, "rejected": 0}}
        }}});
        fs::create_dir_all(base(&ctx)).unwrap();
        fs::write(scores_path(&ctx), scores.to_string()).unwrap();

        let said = advise_apply(&ctx, Some("routed")).unwrap();
        assert!(said.contains("weak -> main") && said.contains("rejected 4 of 5 on rust-fix"), "{}", said);
        assert!(said.contains("next run"), "{}", said);
        let changed = profile(&ctx, "routed").unwrap();
        assert_eq!(changed.workers[0].model, json!("main"));
        let text = advise_text(&ctx, Some("routed")).unwrap();
        assert!(text.contains("Best recorded model: main (3 of 3 accepted)"), "{}", text);
        // A second call has nothing left to change.
        assert!(advise_apply(&ctx, Some("routed")).unwrap().contains("already set"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn advise_apply_refuses_a_running_cluster_and_leaves_its_profile_alone() {
        let (ctx, dir) = ctx();
        write_profile(&ctx, "busy", json!({
            "name": "busy",
            "workers": [{"name": "coder-4", "role": "scout", "model": "weak", "task": "Do the task.", "task_type": "rust-fix"}],
            "validator": {"name": "check", "role": "judge", "model": "judge"},
            "max_models": 2, "max_depth": 1, "retries": 0, "worktree": false
        }));
        fs::create_dir_all(base(&ctx)).unwrap();
        fs::write(scores_path(&ctx), json!({"coder-4": {"rust-fix": {
            "accepted": 1, "rejected": 4,
            "models": {"weak": {"accepted": 1, "rejected": 4}, "main": {"accepted": 3, "rejected": 0}}
        }}}).to_string()).unwrap();
        let p = profile(&ctx, "busy").unwrap();
        let c = new_cluster(&ctx, &p, "busy", "work", None, Vec::new(), 0).unwrap();
        let mut c = load(&ctx, &c.id).unwrap();
        c.state = "working".to_string();
        save(&ctx, &c).unwrap();

        let err = advise_apply(&ctx, Some("busy")).unwrap_err();
        assert!(err.contains("running cluster") && err.contains("next run"), "{}", err);
        assert_eq!(profile(&ctx, "busy").unwrap().workers[0].model, json!("weak"), "the profile was not touched");
        assert!(advise_apply(&ctx, None).unwrap_err().contains("needs a profile"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn advise_apply_refuses_a_cluster_file_that_cannot_carry_a_model() {
        let (ctx, dir) = ctx();
        fs::create_dir_all(profile_dir(&ctx)).unwrap();
        fs::write(profile_dir(&ctx).join("clust.cluster"), "$clust(parts=[scout, validator], worktree=false)").unwrap();
        fs::create_dir_all(base(&ctx)).unwrap();
        fs::write(scores_path(&ctx), json!({"scout": {"clust": {"accepted": 1, "rejected": 4}}}).to_string()).unwrap();
        let err = advise_apply(&ctx, Some("clust")).unwrap_err();
        assert!(err.contains(".cluster file") && err.contains("cannot carry a model"), "{}", err);
        assert!(!profile_dir(&ctx).join("clust.json").exists(), "the refused apply wrote no profile");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn part_notes_write_their_structural_links_into_the_markdown() {
        let (ctx, dir) = ctx();
        write_profile(&ctx, "linked", json!({
            "name": "linked",
            "workers": [
                {"name": "a", "role": "worker", "slot": "Telemach", "model": "main", "task": "Part A."},
                {"name": "b", "role": "worker", "slot": "Telemach", "model": "main", "task": "Part B.", "needs": ["a"]}
            ],
            "validator": {"name": "check", "role": "judge", "slot": "Argos", "model": "judge", "task": "Check."},
            "max_models": 3, "max_depth": 1, "retries": 0, "worktree": false
        }));
        let p = profile(&ctx, "linked").unwrap();
        let c = new_cluster(&ctx, &p, "linked-notes", "link the notes", None, Vec::new(), 0).unwrap();
        for part in &p.workers { write_part_note(&ctx, &c, &p, part, None, crate::notes::Status::Open); }
        let notes = notes_dir(&ctx, &c.id);
        let a = fs::read_to_string(notes.join("a.md")).unwrap();
        let b = fs::read_to_string(notes.join("b.md")).unwrap();
        assert!(a.contains("[[b]]"), "the neighbour link is missing: {}", a);
        assert!(b.contains("[[a]]"), "the dependency link is missing: {}", b);
        let text = prompt_notes_text(&notes, "b", 1);
        assert!(text.contains("[note a ") && text.contains("[note b "), "{}", text);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_validator_confirms_notes_and_a_later_rejection_marks_them_stale() {
        let (ctx, dir) = ctx();
        let p = profile(&ctx, "solo").unwrap();
        let c = new_cluster(&ctx, &p, "note-status", "check note status", None, Vec::new(), 0).unwrap();
        let worker = &p.workers[0];
        write_part_note(&ctx, &c, &p, worker, None, crate::notes::Status::Open);
        let note_path = notes_dir(&ctx, &c.id).join(format!("{}.md", note_name(worker)));
        assert!(fs::read_to_string(&note_path).unwrap().contains("status: open"));
        crate::notes::set_status(&notes_dir(&ctx, &c.id), &note_name(worker), crate::notes::Status::Confirmed).unwrap();
        let loaded = crate::notes::load_dir(&notes_dir(&ctx, &c.id)).unwrap();
        assert_eq!(loaded.iter().find(|n| n.name == note_name(worker)).unwrap().status, crate::notes::Status::Confirmed);
        assert!(fs::read_to_string(note_path.with_extension("events.log")).unwrap().contains("\"status\":\"confirmed\""));
        crate::notes::set_status(&notes_dir(&ctx, &c.id), &note_name(worker), crate::notes::Status::Stale).unwrap();
        let text = prompt_notes_text(&notes_dir(&ctx, &c.id), &note_name(worker), 2);
        assert!(text.is_empty(), "a stale start note must not reach a prompt: {}", text);
        let _ = fs::remove_dir_all(&dir);
    }

}
