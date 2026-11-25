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
}

/// Runs a block of work as a part of one cluster. Nested clusters restore the parent id when they return.
pub fn with_cluster<T>(id: &str, f: impl FnOnce() -> T) -> T {
    let before = ACTIVE_CLUSTER.with(|c| c.replace(Some(id.to_string())));
    let out = f();
    ACTIVE_CLUSTER.with(|c| *c.borrow_mut() = before);
    out
}

/// The cluster of the running part, or `None` outside a cluster. A Claude CLI part reaches this through its MCP server.
pub fn current_cluster() -> Option<String> {
    if let Some(id) = ACTIVE_CLUSTER.with(|c| c.borrow().clone()) {
        if !id.trim().is_empty() { return Some(id); }
    }
    std::env::var("PENELOPE_CLUSTER").ok().filter(|id| !id.trim().is_empty())
}


pub const BUILTIN_PROFILES: &[&str] = &["coding", "backend", "frontend", "live-test", "research", "solo"];

#[derive(Clone, Debug)]
pub struct Part {
    pub name: String,
    pub role: String,
    pub slot: String,
    pub task: String,
    /// A model setting, a role name (`main`, `weak`, `scout`, `judge`, `worker`, ...), or empty for the role's model.
    pub model: Value,
    pub mcp: Vec<String>,
    /// Names of worker parts that must finish before this part starts. Empty means it may start at once.
    pub depends_on: Vec<String>,
}

#[derive(Clone, Debug)]
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
    /// Seconds between two reports of a running part. 0 reports after every model answer.
    pub report_every: u64,
    /// Consecutive reports with one `P:` and `F:`, or repeated equal tool calls, before the cluster is stuck.
    pub stuck_after: usize,
    /// A cluster that may write gets its own git worktree. Research and live-test profiles turn this off.
    pub worktree: bool,
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
            if path.extension().and_then(|x| x.to_str()) != Some("json") { continue; }
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
        model,
        mcp: value["mcp"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default(),
        depends_on: ["needs", "after"].iter()
            .find_map(|key| value[*key].as_array())
            .map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.trim().to_string())).filter(|s| !s.is_empty()).collect())
            .unwrap_or_default(),
    }
}

fn parse_profile(value: &Value, fallback_name: &str) -> Result<Profile, String> {
    let name = value["name"].as_str().filter(|s| !s.trim().is_empty()).unwrap_or(fallback_name).trim().to_string();
    let workers = value["workers"].as_array().ok_or_else(|| format!("profile {} has no workers list", name))?;
    if workers.is_empty() { return Err(format!("profile {} has no worker part", name)); }
    let mut list = Vec::new();
    for (i, w) in workers.iter().enumerate() {
        let p = part(w, &format!("worker{}", i + 1));
        if !crate::route::role_names().iter().any(|r| *r == p.role) {
            return Err(format!("profile {}: part {} has unknown role {} (use {})", name, p.name, p.role, crate::route::role_names().join(", ")));
        }
        if list.iter().any(|x: &Part| x.name == p.name) { return Err(format!("profile {}: part name {} is used twice", name, p.name)); }
        list.push(p);
    }
    let validator = value["validator"].as_object().map(|v| part(&Value::Object(v.clone()), "validator"));
    if let Some(v) = &validator {
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
    let report_every = seconds_of(&value["report_every"]).unwrap_or(0);
    let stuck_after = count_of(&value["stuck_after"]).unwrap_or(3).max(1);
    // A profile with a worker part writes, so it gets a worktree unless it says otherwise. A research or live-test
    // profile has no writer and can be set to false explicitly.
    let worktree = value["worktree"].as_bool().unwrap_or_else(|| list.iter().any(|p| p.role == "worker"));
    Ok(Profile { name, workers: list, validator, max_models, max_depth, max_parallel, budget_eur, max_steps, retries, report_every, stuck_after, worktree })
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
        _ => return None,
    };
    parse_profile(&value, name).ok()
}

pub fn profile(ctx: &Ctx, name: &str) -> Result<Profile, String> {
    let path = profile_dir(ctx).join(format!("{}.json", safe(name)));
    if let Ok(text) = fs::read_to_string(&path) {
        let value: Value = serde_json::from_str(&text).map_err(|e| format!("profile {} is not JSON: {}", name, e))?;
        return parse_profile(&value, name);
    }
    if let Some(p) = builtin(name) { return Ok(p); }
    // A user profile may name a built-in profile as its base and add or remove parts.
    Err(format!("no cluster profile {}. profiles: {}. Put one under .penelope/clusters/profiles/{}.json.",
                 name, profile_names(ctx).join(", "), name))
}

pub fn profiles_text(ctx: &Ctx) -> String {
    let mut out = String::from("Cluster profiles\n\n");
    for name in profile_names(ctx) {
        match profile(ctx, &name) {
            Ok(p) => {
                let parts: Vec<String> = p.workers.iter().map(|w| format!("{} ({})", w.name, w.role))
                    .chain(p.validator.iter().map(|v| format!("{} (validator)", v.name))).collect();
                out.push_str(&format!("{:<12} {} · max {} parts, depth {}, {} parallel, {} retries, report every {}s, stuck after {}{}\n  {}\n", name, parts.join(", "),
                                      p.max_models, p.max_depth, p.max_parallel, p.retries, p.report_every, p.stuck_after,
                                      if p.worktree { " · worktree" } else { "" },
                                      if p.budget_eur > 0.0 { format!("{:.2} EUR budget", p.budget_eur) } else { "no euro limit".to_string() }));
            }
            Err(e) => out.push_str(&format!("{:<12} {}\n", name, e)),
        }
    }
    out.push_str("\nA profile is a file under .penelope/clusters/profiles/NAME.json. A part names a role, a slot and a\nmodel: \"main\", \"weak\", \"scout\", \"judge\", \"worker\" or a full model setting such as an OpenCode model.\n");
    out
}

fn model_for_part(ctx: &Ctx, p: &Part) -> Result<Value, String> {
    if p.model.is_object() && p.model["model"].is_string() {
        return Ok(p.model.clone());
    }
    let key = p.model.as_str().unwrap_or("").trim();
    let model = match key {
        "main" => Some(ctx.cfg["main"].clone()),
        "weak" => Some(ctx.cfg["weak"].clone()),
        "" => crate::route::model_for(ctx, &p.role).cloned(),
        role if crate::route::role_names().iter().any(|r| *r == role) => crate::route::model_for(ctx, role).cloned(),
        other => return Err(format!("part {} names unknown model {}; use main, weak, scout, judge, worker or a model setting", p.name, other)),
    };
    model.filter(|m| m["model"].is_string()).ok_or_else(|| format!("part {} has no model for role {}", p.name, p.role))
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
        "respawns": c.respawns, "respawned_from": c.respawned_from
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
        parts: v["parts"].as_array().map(|a| a.iter().map(|p| PartRun {
            name: p["name"].as_str().unwrap_or("").to_string(), slot: p["slot"].as_str().unwrap_or("").to_string(),
            role: p["role"].as_str().unwrap_or("").to_string(), model: p["model"].as_str().unwrap_or("").to_string(),
            state: p["state"].as_str().unwrap_or("queued").to_string(),
            output: p["output"].as_str().unwrap_or("").to_string(), error: p["error"].as_str().unwrap_or("").to_string(),
        }).collect()).unwrap_or_default(),
    }
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
    // The history is what the zombie watch reads: one line per report, with the time it was written.
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(dir.join("reports.jsonl")) {
        let _ = writeln!(file, "{} {}", now(), line.trim_end());
    }
    line
}

fn memory_path(ctx: &Ctx, id: &str) -> PathBuf {
    cluster_dir(ctx, id).join("memory.md")
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

    /// Every tool call of a part goes into `calls.jsonl`. `stuck_after` equal calls in a row are a zombie.
    fn note_call(&mut self, ctx: &Ctx, c: &Cluster, part: &Part, name: &str, args: &Value) -> Option<String> {
        let sig = format!("{} {}", name, serde_json::to_string(args).unwrap_or_else(|_| "null".to_string()));
        let dir = cluster_dir(ctx, &c.id);
        let _ = fs::create_dir_all(&dir);
        if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(dir.join("calls.jsonl")) {
            let _ = writeln!(file, "{} {} {}", now(), part.name, sig.chars().take(500).collect::<String>());
        }
        if self.cause.is_some() { return self.cause.clone(); }
        let repeated = {
            let list = self.calls.entry(part.name.clone()).or_default();
            list.push(sig.clone());
            while list.len() > self.stuck_after { list.remove(0); }
            list.len() >= self.stuck_after && list.iter().all(|item| item == &sig)
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
            if let Some(cause) = self.note_call(ctx, c, part, &check.name, &args) {
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
                    if watch.lock().unwrap_or_else(|e| e.into_inner()).note_call(ctx, c, part, &call.name, &args).is_some() {
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
    format!("Cluster task: {}\n\nThe parts wrote this memory:\n{}\n\nCheck it. PASS only when the task is really done and the evidence is in the memory. \\
Otherwise FAIL with the smallest exact correction.", c.task, memory.chars().take(12_000).collect::<String>())
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
        let wave: Vec<usize> = ready.into_iter().take(profile.max_parallel.max(1)).collect();
        let mut jobs: Vec<(usize, Part, Value, String, String)> = Vec::new();
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
            let memory = fs::read_to_string(memory_path(ctx, &c.id)).unwrap_or_default();
            let task = format!("Cluster task: {}\n\nYour part: {}\n\nCluster memory so far:\n{}\n\nDo your part and report the evidence.",
                               c.task, if part.task.is_empty() { &c.task } else { &part.task }, memory.chars().take(10_000).collect::<String>());
            jobs.push((*i, part, model, system, task));
        }
        for (i, _, _, _, _) in &jobs { pending.retain(|index| index != i); }
        if jobs.is_empty() { continue; }
        c.done = c.done.min(c.total);
        c.updated = now();
        c.state = "working".to_string();
        save(ctx, c)?;
        let wave_focus = if jobs.len() > 1 { "parallel".to_string() } else { jobs[0].1.name.clone() };
        *focus.lock().unwrap_or_else(|e| e.into_inner()) = wave_focus.clone();
        report(ctx, c, &wave_focus, "");
        let mut handles = Vec::new();
        for (i, part, model, system, task) in jobs {
            let ctx = ctx.clone();
            let snapshot = c.clone();
            let cancel = cancel.clone();
            let watch = watch.clone();
            let steps = profile.max_steps;
            handles.push(std::thread::spawn(move || {
                let out = run_model(&ctx, &snapshot, &part, &system, &task, steps, &cancel, limits, &watch);
                (i, part, model, out)
            }));
        }
        let mut results = Vec::new();
        for handle in handles {
            match handle.join() {
                Ok(result) => results.push(result),
                Err(_) => return Err("a worker part panicked".to_string()),
            }
        }
        for (i, part, model, out) in results {
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
                    c.parts[i].state = "done".to_string();
                    c.parts[i].output = done.text.clone();
                    c.parts[i].error.clear();
                    c.output.push_str(&format!("\n--- {} ({}) ---\n{}\n", part.name, part.slot, done.text));
                    memory_line(ctx, c, &part, &model, &done.text, "done");
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
    let memory = fs::read_to_string(memory_path(ctx, &c.id)).unwrap_or_default();
    let out = run_model(ctx, c, part, &validator_system(c, part, &model), &validator_prompt(c, &memory), 4, cancel, limits, watch);
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

/// Runs one cluster: every worker part, then the validator. A FAIL sends the result back to the workers up to `retries`.
pub fn run_cluster(ctx: &Ctx, id: &str) -> Result<String, String> {
    run_cluster_inner(ctx, id, None)
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
    let cancel = Arc::new(AtomicBool::new(false));
    register_cancel(&c.id, &cancel);
    let _cancel_guard = CancelGuard(c.id.clone());
    let _kill_watch = KillWatch::start(ctx, &c.id, cancel.clone());
    c.started = if c.started == 0 { now() } else { c.started };
    c.state = "working".to_string();
    c.done = 0;
    c.total = profile.workers.len() + profile.validator.iter().count();
    c.error.clear();
    c.evidence.clear();
    c.updated = now();
    save(ctx, &c)?;
    let _ = fs::write(memory_path(ctx, &c.id), format!("# Cluster {} · {}\n\nTask: {}\nProfile: {} · max {} parts, depth {}\n",
                                                         c.name, c.profile, c.task, c.profile, profile.max_models, profile.max_depth));
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
        if profile.validator.is_none() {
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

fn append_memory(ctx: &Ctx, c: &Cluster, text: &str) {
    let _guard = MEMORY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _ = fs::OpenOptions::new().create(true).append(true).open(memory_path(ctx, &c.id)).and_then(|mut f| f.write_all(text.as_bytes()));
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
                      project, worktree, branch, cwd, respawns: 0, respawned_from: String::new(), parts };
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
    if goal.trim().is_empty() { return Err("NOT started. The cluster goal is empty.".to_string()); }
    let plan = plan(ctx, goal.trim(), hint);
    let created = create_plan(ctx, &plan)?;
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
    let ids: Vec<String> = created.iter().map(|(id, _)| id.clone()).collect();
    let summary = status_text(ctx, None);
    Ok(format!("{} clusters started: {}\n\n{}", created.len(), ids.join(", "), summary))
}

pub fn status_text(ctx: &Ctx, id: Option<&str>) -> String {
    if let Some(id) = id.filter(|s| !s.is_empty()) {
        let c = match load(ctx, id) { Ok(c) => c, Err(e) => return e };
        let mut out = format!("cluster {} · {} · profile {} · state {}\nTask: {}\nProgress: {}/{}\nMemory: {}\n",
                              c.id, c.name, c.profile, c.state, c.task, c.done, c.total, c.memory);
        if !c.worktree.is_empty() { out.push_str(&format!("Worktree: {} · branch {}\n", c.worktree, c.branch)); }
        if !c.error.is_empty() { out.push_str(&format!("Error: {}\n", c.error)); }
        for p in &c.parts {
            out.push_str(&format!("  {:<12} {:<10} {:<12} {:<10} {}\n", p.name, p.role, p.slot, p.state, p.model));
            if !p.error.is_empty() { out.push_str(&format!("    {}\n", p.error.chars().take(200).collect::<String>())); }
        }
        if let Ok(report) = fs::read_to_string(cluster_dir(ctx, id).join("report.txt")) { out.push_str(&format!("\nReport: {}", report)); }
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
    let task = format!("{}\n\nCause from the dump: {}\n{}", old.task, cause, extra.trim());
    let mut new = new_cluster(ctx, &profile, &old.name, &task, old.parent.as_deref(), old.after.clone(), old.depth)?;
    new.respawns = old.respawns + 1;
    new.respawned_from = old.id.clone();
    save(ctx, &new)?;
    let said = run_cluster_inner(ctx, &new.id, session_before)?;
    Ok((new.id.clone(), format!("cluster {} respawned ({} of 2) as {}\n{}", old.id, new.respawns, new.id, said)))
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
        assert!(one.worktree.starts_with(&format!("{}/.penelope/worktrees/", one.project)), "{}", one.worktree);
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
        assert!(body.contains(&c.worktree), "the write went to the worktree, not the live tree: {}", body);
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
        let memory = fs::read_to_string(memory_path(&ctx, &c.id)).unwrap();
        assert!(memory.contains("evidence: cat hello.txt ok"), "{}", memory);
        let _ = fs::remove_dir_all(&dir);
    }

    fn write_profile(ctx: &Ctx, name: &str, value: Value) {
        fs::create_dir_all(profile_dir(ctx)).unwrap();
        fs::write(profile_dir(ctx).join(format!("{}.json", name)), value.to_string()).unwrap();
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
        assert!(said.contains("will stop"), "{}", said);
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
}
