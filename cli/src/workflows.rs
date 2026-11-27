//! Workflows: a small script that drives several agents deterministically.
//!
//! The script runs in Rhai with no file, network or process access. Every agent runs with the normal
//! Penelope tools, Eurykleia, the gates and the route. The journal keeps every call, so a crash does
//! not repeat a finished call. The script can ask the user before and during the run.

use crate::config::Ctx;
use rhai::{Array, Dynamic, Engine, EvalAltResult, FnPtr, ImmutableString, Map, NativeCallContext, Position, Scope};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// What the caller allows the workflow to do.
pub struct RunOptions {
    pub args: Value,
    pub budget_eur: f64,
    pub budget_tokens: u64,
    pub max_parallel: usize,
    pub confirm: bool,
    pub cancel: Arc<AtomicBool>,
    pub ask: Option<Box<dyn FnMut(&[Value]) -> Result<Vec<Value>, String>>>,
    /// Continue the run with this id and take finished calls from its journal.
    pub resume_id: Option<String>,
}

impl Default for RunOptions {
    fn default() -> Self {
        RunOptions {
            args: json!({}),
            budget_eur: 0.0,
            budget_tokens: 0,
            max_parallel: default_max_parallel(),
            confirm: true,
            cancel: Arc::new(AtomicBool::new(false)),
            ask: None,
            resume_id: None,
        }
    }
}

/// The default number of parallel agents from the machine, not from a wish: half the CPUs, at most 4, and at most
/// one agent per 1.5 GB of available memory. On the owner's machine the memory bound is the one that matters.
pub fn default_max_parallel() -> usize {
    let cpus = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    let by_cpu = (cpus / 2).max(1).min(4);
    let free_gb = fs::read_to_string("/proc/meminfo").ok()
        .and_then(|text| text.lines().find(|l| l.starts_with("MemAvailable:"))
            .and_then(|l| l.split_whitespace().nth(1)).and_then(|k| k.parse::<u64>().ok()))
        .map(|kib| kib as f64 / 1024.0 / 1024.0);
    match free_gb {
        Some(gb) => by_cpu.min((gb / 1.5).floor().max(1.0) as usize),
        None => by_cpu,
    }
}

/// What a run produced.
#[derive(Debug)]
pub struct RunResult {
    pub id: String,
    pub output: Value,
    pub eur: f64,
    pub tokens: u64,
    pub journal: PathBuf,
}

struct State {
    ctx: Ctx,
    id: String,
    dir: PathBuf,
    spent_eur: f64,
    spent_tokens: u64,
    budget_eur: f64,
    budget_tokens: u64,
    max_parallel: usize,
    depth: usize,
    cancel: Arc<AtomicBool>,
    resume: HashMap<String, Value>,
    ask: Option<Box<dyn FnMut(&[Value]) -> Result<Vec<Value>, String>>>,
}

fn fail(msg: impl Into<String>) -> Box<EvalAltResult> {
    Box::new(EvalAltResult::ErrorRuntime(msg.into().into(), Position::NONE))
}

fn dynamic(v: Value) -> Result<Dynamic, Box<EvalAltResult>> {
    rhai::serde::to_dynamic(v.clone()).map_err(|e| fail(format!("cannot hand {} to the script: {}", v, e)))
}

fn value(v: &Dynamic) -> Result<Value, Box<EvalAltResult>> {
    rhai::serde::from_dynamic(v).map_err(|e| fail(format!("cannot read {} from the script: {}", v, e)))
}

/// A stable key for the journal. FNV-1a over the prompt and the options.
fn key_text(prompt: &str, opts_text: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in prompt.as_bytes().iter().chain(opts_text.as_bytes()) {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{:016x}", h)
}

fn key(prompt: &str, opts: &Map) -> String {
    key_text(prompt, &format!("{:?}", opts))
}

fn journal_line(dir: &Path, entry: &Value) -> Result<(), String> {
    let path = dir.join("journal.jsonl");
    let line = serde_json::to_string(entry).map_err(|e| e.to_string())?;
    let mut file = fs::OpenOptions::new().create(true).append(true).open(&path)
        .map_err(|e| format!("cannot write {}: {}", path.display(), e))?;
    writeln!(file, "{}", line).map_err(|e| format!("cannot write {}: {}", path.display(), e))
}

/// True when the user asked this run to stop. `/workflow stop ID` writes the marker beside the journal, in this or
/// another process, so a running workflow sees it before its next agent.
fn stopped(st: &State) -> bool {
    st.cancel.load(Ordering::SeqCst) || st.dir.join("stop").is_file()
}

fn ensure_running(st: &State) -> Result<(), Box<EvalAltResult>> {
    if stopped(st) { Err(fail("workflow stopped")) } else { Ok(()) }
}

/// A workflow may not raise its own rights. The cluster and tool layer sees the user's safe policy, so `git push`,
/// `git merge` and package installs wait for a question that a workflow cannot answer.
fn workflow_ctx(ctx: &Ctx) -> Ctx {
    let mut c = ctx.clone();
    c.cfg["run_policy"] = json!("safe");
    c
}


fn load_resume(dir: &Path) -> HashMap<String, Value> {
    let mut out = HashMap::new();
    let Ok(text) = fs::read_to_string(dir.join("journal.jsonl")) else { return out };
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        if let (Some(k), Some(value)) = (v["key"].as_str(), v.get("value")) {
            out.insert(k.to_string(), value.clone());
        }
    }
    out
}

fn validate(schema: &Value, value: &Value) -> Result<(), String> {
    let validator = jsonschema::validator_for(schema).map_err(|e| format!("schema: {}", e))?;
    validator.validate(value).map_err(|e| format!("answer does not match the schema: {}", e))
}

fn opts_model(opts: &Map) -> (Value, String) {
    let model = opts.get("model").map(|v| v.to_string()).filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "worker".to_string());
    let effort = opts.get("effort").map(|v| v.to_string()).unwrap_or_else(|| "high".to_string());
    (Value::String(model), effort)
}

/// The schema is either a map or a JSON string. Rhai's `to_string()` of a map is not JSON, so the map goes through
/// serde instead of the textual path.
fn schema_of(opts: &Map) -> Result<Option<Value>, Box<EvalAltResult>> {
    let Some(raw) = opts.get("schema") else { return Ok(None) };
    let schema = if let Some(text) = raw.clone().try_cast::<ImmutableString>() {
        serde_json::from_str::<Value>(text.as_str())
            .map_err(|e| fail(format!("schema is not JSON: {}", e)))?
    } else {
        rhai::serde::from_dynamic::<Value>(raw)
            .map_err(|e| fail(format!("schema cannot be read: {}", e)))?
    };
    Ok(Some(schema))
}

fn budget_left(st: &State) -> Result<(), Box<EvalAltResult>> {
    if st.budget_eur > 0.0 && st.spent_eur >= st.budget_eur {
        return Err(fail(format!("budget of {:.4} EUR is used", st.budget_eur)));
    }
    if st.budget_tokens > 0 && st.spent_tokens >= st.budget_tokens {
        return Err(fail(format!("budget of {} tokens is used", st.budget_tokens)));
    }
    Ok(())
}

fn project(ctx: &Ctx) -> PathBuf {
    ctx.root.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| ctx.root.clone())
}

fn run_agent(st: &mut State, prompt: &str, opts: &Map) -> Result<Dynamic, Box<EvalAltResult>> {
    ensure_running(st)?;
    let label = opts.get("label").map(|v| v.to_string()).unwrap_or_default();
    let phase = opts.get("phase").map(|v| v.to_string()).unwrap_or_default();
    let worktree = opts.get("worktree").map(|v| v.as_bool().unwrap_or(false)).unwrap_or(false);
    let schema = schema_of(opts)?;
    let k = key(prompt, opts);
    if let Some(v) = st.resume.get(&k) {
        return dynamic(v.clone());
    }
    budget_left(st)?;
    let before = crate::cost::total_eur(&st.ctx);
    let before_tokens = crate::cost::total_tokens(&st.ctx);
    let ctx = workflow_ctx(&st.ctx);
    let (model, effort) = opts_model(opts);
    let mut kept = Value::Null;
    let text = if worktree {
        let name = format!("wf-{}-{}-{}", st.id, if label.is_empty() { "part" } else { &label }, &k[..8]);
        let (path, branch) = crate::work::create(&project(&ctx), &name)
            .map_err(|e| fail(format!("worktree: {}", e)))?;
        let goal = format!("{}\n\nWork in this worktree. The check is hidden.", prompt);
        let out = crate::eval::with_task_cwd(&path, || crate::clusters::run_once(&ctx, &goal, model.clone(), &effort));
        // The worktree stays: a build workflow must be able to inspect and use its result. The journal names it.
        kept = json!({"worktree": path.to_string_lossy(), "branch": branch});
        out.map_err(|e| fail(e))?
    } else {
        crate::clusters::run_once(&ctx, prompt, model, &effort).map_err(|e| fail(e))?
    };
    let after = crate::cost::total_eur(&st.ctx);
    let after_tokens = crate::cost::total_tokens(&st.ctx);
    let eur = (after - before).max(0.0);
    st.spent_eur += eur;
    st.spent_tokens += after_tokens.saturating_sub(before_tokens);
    let value = match schema {
        Some(schema) => {
            let v: Value = serde_json::from_str(&text).map_err(|e| fail(format!("answer is not JSON: {}", e)))?;
            validate(&schema, &v).map_err(|e| fail(e))?;
            v
        }
        None => Value::String(text),
    };
    let mut entry = json!({"key": k.clone(), "phase": phase, "label": label, "prompt": prompt, "value": value, "eur": eur});
    if !kept.is_null() { entry["worktree"] = kept; }
    journal_line(&st.dir, &entry).map_err(|e| fail(e))?;
    if let Some(v) = entry.get("value") {
        st.resume.insert(k, v.clone());
    }
    dynamic(entry["value"].clone())
}


fn engine(state: Rc<RefCell<State>>) -> Engine {
    let mut e = Engine::new();
    e.set_max_operations(10_000_000);
    e.set_max_expr_depths(64, 64);
    e.set_max_string_size(1_000_000);
    e.set_max_array_size(10_000);
    e.set_max_map_size(10_000);

    let s = state.clone();
    e.register_fn("agent", move |prompt: ImmutableString, opts: Map| -> Result<Dynamic, Box<EvalAltResult>> {
        let mut st = s.borrow_mut();
        run_agent(&mut st, prompt.as_str(), &opts)
    });

    let s = state.clone();
    e.register_fn("parallel", move |items: Array| -> Result<Array, Box<EvalAltResult>> {
        let mut st = s.borrow_mut();
        ensure_running(&st)?;
        let mut jobs = Vec::new();
        for item in items.iter() {
            let m = item.clone().try_cast::<Map>().ok_or_else(|| fail("parallel needs maps"))?;
            let prompt = m.get("prompt").map(|v| v.to_string()).unwrap_or_default();
            let opts = m.get("opts").and_then(|v| v.clone().try_cast::<Map>()).unwrap_or_default();
            let worktree = opts.get("worktree").map(|v| v.as_bool().unwrap_or(false)).unwrap_or(false);
            if worktree {
                return Err(fail("worktree is not supported inside parallel; use agent with worktree for each part"));
            }
            let schema = schema_of(&opts)?;
            let k = key(&prompt, &opts);
            let label = opts.get("label").map(|v| v.to_string()).unwrap_or_default();
            let phase = opts.get("phase").map(|v| v.to_string()).unwrap_or_default();
            jobs.push((prompt, opts, schema, k, label, phase));
        }
        if jobs.len() > st.max_parallel {
            return Err(fail(format!("parallel is limited to {} agents", st.max_parallel)));
        }
        budget_left(&st)?;
        let ctx = workflow_ctx(&st.ctx);
        let before = crate::cost::total_eur(&st.ctx);
        let before_tokens = crate::cost::total_tokens(&st.ctx);
        let mut values: Vec<Option<Value>> = vec![None; jobs.len()];
        let mut handles = Vec::new();
        for (index, (prompt, opts, _schema, k, label, phase)) in jobs.iter().enumerate() {
            if let Some(v) = st.resume.get(k) {
                values[index] = Some(v.clone());
                continue;
            }
            let (model, effort) = opts_model(opts);
            let c = ctx.clone();
            let prompt = prompt.clone();
            let label = label.clone();
            let phase = phase.clone();
            let key = k.clone();
            handles.push(std::thread::spawn(move || {
                let text = crate::clusters::run_once(&c, &prompt, model, &effort)?;
                Ok::<_, String>((index, key, label, phase, text))
            }));
        }
        let mut out = vec![Value::Null; jobs.len()];
        for h in handles {
            let (index, key, label, phase, text) = h.join().map_err(|_| fail("a parallel agent panicked"))?.map_err(|e| fail(e))?;
            let schema = jobs[index].2.as_ref();
            let value = match schema {
                Some(schema) => {
                    let v: Value = serde_json::from_str(&text).map_err(|e| fail(format!("answer is not JSON: {}", e)))?;
                    validate(schema, &v).map_err(|e| fail(e))?;
                    v
                }
                None => Value::String(text),
            };
            journal_line(&st.dir, &json!({"key": key, "phase": phase, "label": label, "value": value}))
                .map_err(|e| fail(e))?;
            st.resume.insert(key, value.clone());
            out[index] = value;
        }
        for (index, slot) in values.into_iter().enumerate() {
            if let Some(v) = slot { out[index] = v; }
        }
        let after = crate::cost::total_eur(&st.ctx);
        let after_tokens = crate::cost::total_tokens(&st.ctx);
        st.spent_eur += (after - before).max(0.0);
        st.spent_tokens += after_tokens.saturating_sub(before_tokens);
        journal_line(&st.dir, &json!({"phase": "parallel", "results": out})).map_err(|e| fail(e))?;
        out.into_iter().map(dynamic).collect()
    });

    let s = state.clone();
    e.register_fn("pipeline", move |items: Array, stages: Array| -> Result<Array, Box<EvalAltResult>> {
        let mut st = s.borrow_mut();
        ensure_running(&st)?;
        let stage_maps: Vec<Map> = stages.iter()
            .map(|v| v.clone().try_cast::<Map>().ok_or_else(|| fail("pipeline stages need maps")))
            .collect::<Result<_, _>>()?;
        if items.len() > st.max_parallel {
            return Err(fail(format!("pipeline is limited to {} items", st.max_parallel)));
        }
        let mut specs = Vec::new();
        for stage in &stage_maps {
            let template = stage.get("prompt").map(|v| v.to_string()).unwrap_or_default();
            let (model, effort) = opts_model(stage);
            let schema = schema_of(stage)?;
            // Rhai maps are not Send. The thread gets plain JSON, and the key uses the same JSON text on every run.
            let opts_json = rhai::serde::from_dynamic::<Value>(&Dynamic::from_map(stage.clone()))
                .map_err(|e| fail(format!("pipeline stage: {}", e)))?;
            specs.push((template, model, effort, schema, opts_json));
        }
        budget_left(&st)?;
        let ctx = workflow_ctx(&st.ctx);
        let before = crate::cost::total_eur(&st.ctx);
        let before_tokens = crate::cost::total_tokens(&st.ctx);
        let resume = st.resume.clone();
        let mut handles = Vec::new();
        for item in items.iter() {
            let item = item.to_string();
            let specs = specs.clone();
            let c = ctx.clone();
            let resume = resume.clone();
            handles.push(std::thread::spawn(move || -> Result<Vec<(String, String, String, String)>, String> {
                let mut out = Vec::new();
                let mut current = item;
                for (template, model, effort, _schema, opts) in specs {
                    let prompt = template.replace("{item}", &current);
                    let k = key_text(&prompt, &opts.to_string());
                    let label = opts["label"].as_str().unwrap_or("").to_string();
                    let phase = opts["phase"].as_str().unwrap_or("").to_string();
                    if let Some(v) = resume.get(&k) {
                        current = v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string());
                    } else {
                        current = crate::clusters::run_once(&c, &prompt, model, &effort)?;
                    }
                    out.push((k, label, phase, current.clone()));
                }
                Ok(out)
            }));
        }
        let mut out = Array::new();
        for h in handles {
            let row = h.join().map_err(|_| fail("a pipeline agent panicked"))?.map_err(|e| fail(e))?;
            let mut values = Vec::new();
            for (stage_index, (k, label, phase, text)) in row.into_iter().enumerate() {
                if let Some(schema) = specs.get(stage_index).and_then(|s| s.3.as_ref()) {
                    let v: Value = serde_json::from_str(&text).map_err(|e| fail(format!("answer is not JSON: {}", e)))?;
                    validate(schema, &v).map_err(|e| fail(e))?;
                }
                let value = Value::String(text);
                journal_line(&st.dir, &json!({"key": k, "phase": phase, "label": label, "value": value}))
                    .map_err(|e| fail(e))?;
                st.resume.insert(k, value.clone());
                values.push(value);
            }
            out.push(dynamic(json!(values))?);
        }
        let after = crate::cost::total_eur(&st.ctx);
        let after_tokens = crate::cost::total_tokens(&st.ctx);
        st.spent_eur += (after - before).max(0.0);
        st.spent_tokens += after_tokens.saturating_sub(before_tokens);
        journal_line(&st.dir, &json!({"phase": "pipeline"})).map_err(|e| fail(e))?;
        Ok(out)
    });


    let s = state.clone();
    e.register_fn("loop_until", move |ctx: NativeCallContext, condition: FnPtr, step: FnPtr, opts: Map|
        -> Result<Dynamic, Box<EvalAltResult>> {
        let max = opts.get("max_iterations").map(|v| v.as_int().unwrap_or(4)).unwrap_or(4).max(1).min(12);
        let mut result = Dynamic::UNIT;
        for round in 0..max {
            {
                let st = s.borrow();
                ensure_running(&st)?;
            }
            result = step.call_within_context(&ctx, ())?;
            let verdict: Dynamic = condition.call_within_context(&ctx, (result.clone(),))?;
            let stop = verdict.as_bool().unwrap_or(false);
            let entry = json!({"phase": "loop", "round": round + 1, "result": value(&result)?, "stop": stop});
            let st = s.borrow();
            journal_line(&st.dir, &entry).map_err(|e| fail(e))?;
            if stop { break; }
        }
        Ok(result)
    });

    let s = state.clone();
    e.register_fn("ask", move |questions: Array| -> Result<Array, Box<EvalAltResult>> {
        let qs: Vec<Value> = questions.iter().map(value).collect::<Result<_, _>>()?;
        let mut st = s.borrow_mut();
        let Some(ask) = st.ask.as_mut() else { return Err(fail("no ask dialog is attached")); };
        let answers = ask(&qs).map_err(|e| fail(e))?;
        let entry = json!({"phase": "ask", "questions": qs, "answers": answers});
        journal_line(&st.dir, &entry).map_err(|e| fail(e))?;
        answers.iter().map(|a| dynamic(a.clone())).collect()
    });

    let s = state.clone();
    e.register_fn("phase", move |title: ImmutableString| {
        let st = s.borrow();
        let _ = journal_line(&st.dir, &json!({"phase": title.as_str()}));
    });

    let s = state.clone();
    e.register_fn("log", move |text: ImmutableString| {
        let st = s.borrow();
        let _ = journal_line(&st.dir, &json!({"log": text.as_str()}));
    });

    let s = state.clone();
    e.register_fn("workflow", move |name: ImmutableString, args: Map| -> Result<Dynamic, Box<EvalAltResult>> {
        let (ctx, depth, remaining_eur, remaining_tokens, cancel) = {
            let st = s.borrow();
            if st.depth >= 1 { return Err(fail("a workflow may call only one level of workflows")); }
            let eur = if st.budget_eur > 0.0 { (st.budget_eur - st.spent_eur).max(0.0) } else { 0.0 };
            let tokens = if st.budget_tokens > 0 { st.budget_tokens.saturating_sub(st.spent_tokens) } else { 0 };
            (workflow_ctx(&st.ctx), st.depth + 1, eur, tokens, st.cancel.clone())
        };
        let path = ctx.root.join("workflows").join(format!("{}.rhai", name.as_str()));
        if !path.is_file() { return Err(fail(format!("no workflow at {}", path.display()))); }
        let value = rhai::serde::from_dynamic::<Value>(&Dynamic::from_map(args)).map_err(|e| fail(e.to_string()))?;
        let result = run_script(&ctx, &path, RunOptions {
            args: value, confirm: false, budget_eur: remaining_eur, budget_tokens: remaining_tokens, cancel,
            ..RunOptions::default()
        }, depth).map_err(|e| fail(e))?;
        {
            let mut st = s.borrow_mut();
            st.spent_eur += result.eur;
            st.spent_tokens += result.tokens;
        }
        dynamic(result.output)
    });

    e
}

fn run_script(ctx: &Ctx, path: &Path, options: RunOptions, depth: usize) -> Result<RunResult, String> {
    let script = fs::read_to_string(path).map_err(|e| format!("cannot read {}: {}", path.display(), e))?;
    let name = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "workflow".to_string());
    let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
    let id = options.resume_id.clone().filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| format!("{}-{}", sanitize(&name), crate::sessions::stamp(ms)));
    let id = sanitize(&id);
    let dir = ctx.root.join("workflows").join(&id);
    fs::create_dir_all(&dir).map_err(|e| format!("cannot make {}: {}", dir.display(), e))?;
    // A stop marker from an earlier run is stale. A new run of the same id starts clean.
    let _ = fs::remove_file(dir.join("stop"));
    let resume = load_resume(&dir);
    let state = Rc::new(RefCell::new(State {
        ctx: ctx.clone(),
        id: id.clone(),
        dir: dir.clone(),
        spent_eur: 0.0,
        spent_tokens: 0,
        budget_eur: options.budget_eur,
        budget_tokens: options.budget_tokens,
        max_parallel: options.max_parallel.max(1).min(8),
        depth,
        cancel: options.cancel,
        resume,
        ask: options.ask,
    }));
    if options.confirm {
        let text = plan_text(path, options.budget_eur)?;
        let mut st = state.borrow_mut();
        let Some(ask) = st.ask.as_mut() else { return Err("workflow needs a confirmation, but no ask dialog is attached".to_string()); };
        let answers = ask(&[json!({"question": format!("{}\nStart?", text), "kind": "yes_no"})])?;
        let yes = answers.first().and_then(|v| v.as_bool()).unwrap_or(false);
        if !yes { return Err("workflow not started: the user said no".to_string()); }
    }
    let engine = engine(state.clone());
    let mut scope = Scope::new();
    scope.push_constant("args", dynamic(options.args).map_err(|e| e.to_string())?);
    scope.push_constant("budget", dynamic(json!({"eur": options.budget_eur, "tokens": options.budget_tokens})).map_err(|e| e.to_string())?);
    let ast = engine.compile(&script).map_err(|e| format!("script {}: {}", path.display(), e))?;
    let out = engine.eval_ast_with_scope::<Dynamic>(&mut scope, &ast)
        .map_err(|e| format!("workflow {}: {}", name, e))?;
    let st = state.borrow();
    let result = RunResult {
        id,
        output: value(&out).map_err(|e| e.to_string())?,
        eur: st.spent_eur,
        tokens: st.spent_tokens,
        journal: dir.join("journal.jsonl"),
    };
    fs::write(dir.join("result.json"), serde_json::to_string_pretty(&json!({
        "id": result.id, "output": result.output, "eur": result.eur, "tokens": result.tokens
    })).map_err(|e| e.to_string())?).map_err(|e| format!("cannot write result: {}", e))?;
    Ok(result)
}

/// A short plan for the confirmation: what the script starts, at least. The exact number is only known while the
/// script runs, so the number is a lower bound and the dialog says so.
pub fn plan_text(path: &Path, budget_eur: f64) -> Result<String, String> {
    let script = fs::read_to_string(path).map_err(|e| format!("cannot read {}: {}", path.display(), e))?;
    let name = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "workflow".to_string());
    let direct = script.matches("agent(").count();
    let parallel = script.matches("prompt:").count();
    let loops = script.matches("loop_until(").count();
    Ok(format!(
        "Workflow {}\n  agent calls: at least {}\n  loop steps: {}\n  budget: {}\n  script: {}",
        name, direct + parallel, loops,
        if budget_eur > 0.0 { format!("{:.2} EUR", budget_eur) } else { "no euro limit".to_string() },
        path.display()
    ))
}

/// Run a saved workflow file.
pub fn run(ctx: &Ctx, path: &Path, options: RunOptions) -> Result<RunResult, String> {
    run_script(ctx, path, options, 0)
}

/// Run a saved workflow by name under `.penelope/workflows`.
pub fn run_named(ctx: &Ctx, name: &str, options: RunOptions) -> Result<RunResult, String> {
    let path = ctx.root.join("workflows").join(format!("{}.rhai", name));
    if !path.is_file() {
        return Err(format!("no workflow named {} at {}", name, path.display()));
    }
    run(ctx, &path, options)
}

/// The saved workflows.
pub fn list(ctx: &Ctx) -> Vec<String> {
    let dir = ctx.root.join("workflows");
    let Ok(entries) = fs::read_dir(&dir) else { return Vec::new() };
    let mut out: Vec<String> = entries.flatten()
        .filter_map(|e| {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) == Some("rhai") {
                p.file_stem().map(|s| s.to_string_lossy().to_string())
            } else { None }
        })
        .collect();
    out.sort();
    out
}

/// The journal of one run.
pub fn status(ctx: &Ctx, id: &str) -> Result<String, String> {
    let dir = ctx.root.join("workflows").join(sanitize(id));
    let journal = dir.join("journal.jsonl");
    if !journal.is_file() { return Err(format!("no workflow run {} at {}", id, journal.display())); }
    fs::read_to_string(&journal).map_err(|e| e.to_string())
}

/// Ask a running workflow to stop before the next agent.
pub fn stop(ctx: &Ctx, id: &str) -> Result<String, String> {
    let dir = ctx.root.join("workflows").join(sanitize(id));
    if !dir.is_dir() { return Err(format!("no workflow run {}", id)); }
    fs::write(dir.join("stop"), b"1").map_err(|e| e.to_string())?;
    Ok(format!("workflow {} will stop before the next agent", id))
}

fn sanitize(name: &str) -> String {
    name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_script_cannot_read_files_or_open_a_network() {
        let engine = Engine::new();
        for script in ["read_file(\"/etc/passwd\")", "http(\"https://example.com\")", "run_command(\"rm -rf /\")"] {
            assert!(engine.eval::<Dynamic>(script).is_err(), "{} must fail", script);
        }
    }

    #[test]
    fn the_journal_key_is_stable_and_changes_with_the_prompt() {
        let opts = Map::new();
        assert_eq!(key("do it", &opts), key("do it", &opts));
        assert_ne!(key("do it", &opts), key("do it now", &opts));
    }

    #[test]
    fn a_schema_rejects_an_answer_that_does_not_match() {
        let schema = json!({"type": "object", "required": ["ok"], "properties": {"ok": {"type": "boolean"}}});
        assert!(validate(&schema, &json!({"ok": true})).is_ok());
        assert!(validate(&schema, &json!({"ok": "yes"})).is_err());
        assert!(validate(&schema, &json!({})).is_err());
    }

    #[test]
    fn a_script_without_a_confirmation_asks_first() {
        let dir = std::env::temp_dir().join(format!("penelope-workflow-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let ctx = crate::config::load(&dir.join(".penelope"));
        let path = dir.join("one.rhai");
        fs::write(&path, "phase(\"one\");\n").unwrap();
        let asked = Rc::new(std::cell::Cell::new(0));
        let counter = asked.clone();
        let options = RunOptions {
            confirm: true,
            ask: Some(Box::new(move |_q| { counter.set(counter.get() + 1); Ok(vec![json!(true)]) })),
            ..RunOptions::default()
        };
        let out = run(&ctx, &path, options).unwrap();
        assert_eq!(asked.get(), 1);
        assert!(out.journal.is_file());
        let _ = fs::remove_dir_all(&dir);
    }

    fn ctx_with_fake(replies: Vec<(u16, String)>) -> (Ctx, PathBuf) {
        let (address, _seen) = crate::llm::fake::serve(replies);
        let dir = std::env::temp_dir().join(format!("penelope-workflow-fake-{}-{:?}", std::process::id(), std::thread::current().id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let mut ctx = crate::config::load(&dir.join(".penelope"));
        let model = json!({"kind": "openai", "base_url": format!("{}/v1", address), "model": "test-model", "api_key_env": ""});
        ctx.cfg["main"] = model.clone();
        ctx.cfg["roles"]["worker"] = model;
        ctx.cfg["prices"] = json!({"test-model": [1.0, 2.0]});
        (ctx, dir)
    }

    #[test]
    fn an_agent_comes_from_the_fake_and_a_resume_takes_it_from_the_journal() {
        let reply = json!({"choices": [{"finish_reason": "stop", "message": {"role": "assistant", "content": "done"}}],
                           "usage": {"prompt_tokens": 10, "completion_tokens": 2}}).to_string();
        let (ctx, dir) = ctx_with_fake(vec![(200, reply.clone())]);
        let path = dir.join("one.rhai");
        fs::write(&path, "agent(\"do it\", #{})").unwrap();
        let first = run(&ctx, &path, RunOptions { confirm: false, ..RunOptions::default() }).unwrap();
        assert_eq!(first.output, json!("done"));
        let second = run(&ctx, &path, RunOptions { confirm: false, resume_id: Some(first.id.clone()), ..RunOptions::default() }).unwrap();
        assert_eq!(second.output, json!("done"));
        assert!(second.journal.is_file());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn parallel_and_pipeline_run_the_parts() {
        let reply = json!({"choices": [{"finish_reason": "stop", "message": {"role": "assistant", "content": "done"}}],
                           "usage": {"prompt_tokens": 10, "completion_tokens": 2}}).to_string();
        let (ctx, dir) = ctx_with_fake(vec![(200, reply.clone()), (200, reply.clone())]);
        let path = dir.join("par.rhai");
        fs::write(&path, "parallel([#{prompt: \"a\", opts: #{}}, #{prompt: \"b\", opts: #{}}])").unwrap();
        let out = run(&ctx, &path, RunOptions { confirm: false, max_parallel: 4, ..RunOptions::default() }).unwrap();
        assert_eq!(out.output.as_array().map(|a| a.len()), Some(2));
        let (ctx, dir) = ctx_with_fake(vec![(200, reply.clone())]);
        let path = dir.join("pipe.rhai");
        fs::write(&path, "pipeline([\"a\"], [#{prompt: \"x {item}\", model: \"worker\"}])").unwrap();
        let out = run(&ctx, &path, RunOptions { confirm: false, max_parallel: 4, ..RunOptions::default() }).unwrap();
        assert_eq!(out.output.as_array().map(|a| a.len()), Some(1));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_budget_stops_before_the_next_agent() {
        let reply = json!({"choices": [{"finish_reason": "stop", "message": {"role": "assistant", "content": "done"}}],
                           "usage": {"prompt_tokens": 10, "completion_tokens": 2}}).to_string();
        let (ctx, dir) = ctx_with_fake(vec![(200, reply.clone())]);
        let path = dir.join("two.rhai");
        fs::write(&path, "agent(\"one\", #{}); agent(\"two\", #{})").unwrap();
        let out = run(&ctx, &path, RunOptions { confirm: false, budget_eur: 0.0000001, ..RunOptions::default() });
        assert!(out.is_err(), "the second agent must not run");
        assert!(out.unwrap_err().contains("budget"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_cancelled_workflow_stops_before_the_agent() {
        let (ctx, dir) = ctx_with_fake(Vec::new());
        let path = dir.join("stop.rhai");
        fs::write(&path, "agent(\"one\", #{})").unwrap();
        let cancel = Arc::new(AtomicBool::new(true));
        let out = run(&ctx, &path, RunOptions { confirm: false, cancel, ..RunOptions::default() });
        assert!(out.unwrap_err().contains("stopped"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_no_stops_the_workflow_before_any_agent() {
        let dir = std::env::temp_dir().join(format!("penelope-workflow-no-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let ctx = crate::config::load(&dir.join(".penelope"));
        let path = dir.join("one.rhai");
        fs::write(&path, "phase(\"one\");\n").unwrap();
        let options = RunOptions {
            confirm: true,
            ask: Some(Box::new(|_q| Ok(vec![json!(false)]))),
            ..RunOptions::default()
        };
        assert!(run(&ctx, &path, options).unwrap_err().contains("said no"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_stop_marker_of_another_process_stops_the_next_agent() {
        let (ctx, dir) = ctx_with_fake(Vec::new());
        let run_dir = ctx.root.join("workflows").join("stop-check");
        fs::create_dir_all(&run_dir).unwrap();
        let state = State {
            ctx: ctx.clone(), id: "stop-check".to_string(), dir: run_dir.clone(),
            spent_eur: 0.0, spent_tokens: 0, budget_eur: 0.0, budget_tokens: 0,
            max_parallel: 4, depth: 0, cancel: Arc::new(AtomicBool::new(false)),
            resume: HashMap::new(), ask: None,
        };
        assert!(!stopped(&state));
        stop(&ctx, "stop-check").unwrap();
        assert!(stopped(&state), "the marker must stop the run before the next agent");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_schema_map_through_the_agent_is_json_not_the_rhai_debug_text() {
        let reply = json!({"choices": [{"finish_reason": "stop",
            "message": {"role": "assistant", "content": "{\"ok\":true}"}}],
            "usage": {"prompt_tokens": 10, "completion_tokens": 2}}).to_string();
        let (ctx, dir) = ctx_with_fake(vec![(200, reply)]);
        let path = dir.join("schema.rhai");
        fs::write(&path, "agent(\"answer\", #{schema: #{type: \"object\", required: [\"ok\"], \
                          properties: #{ok: #{type: \"boolean\"}}}})").unwrap();
        let out = run(&ctx, &path, RunOptions { confirm: false, ..RunOptions::default() }).unwrap();
        assert_eq!(out.output, json!({"ok": true}));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn parallel_refuses_more_parts_than_the_limit() {
        let (ctx, dir) = ctx_with_fake(Vec::new());
        let path = dir.join("many.rhai");
        let items: Vec<String> = (0..5).map(|i| format!("#{{prompt: \"p{}\", opts: #{{}}}}", i)).collect();
        fs::write(&path, format!("parallel([{}])", items.join(", "))).unwrap();
        let out = run(&ctx, &path, RunOptions { confirm: false, max_parallel: 4, ..RunOptions::default() });
        assert!(out.unwrap_err().contains("limited to 4"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_pipeline_resumes_finished_stages_from_the_journal() {
        let reply = json!({"choices": [{"finish_reason": "stop", "message": {"role": "assistant", "content": "done"}}],
                           "usage": {"prompt_tokens": 10, "completion_tokens": 2}}).to_string();
        let (ctx, dir) = ctx_with_fake(vec![(200, reply)]);
        let path = dir.join("pipe.rhai");
        fs::write(&path, "pipeline([\"a\"], [#{prompt: \"x {item}\", model: \"worker\"}])").unwrap();
        let first = run(&ctx, &path, RunOptions { confirm: false, ..RunOptions::default() }).unwrap();
        // No second fake reply: a resumed pipeline that calls the model again would fail.
        let second = run(&ctx, &path, RunOptions { confirm: false, resume_id: Some(first.id.clone()), ..RunOptions::default() });
        assert_eq!(second.unwrap().output, first.output);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_default_parallel_limit_comes_from_the_machine_and_stays_small() {
        let n = default_max_parallel();
        assert!((1..=4).contains(&n), "{}", n);
    }

    #[test]
    fn a_workflow_cannot_raise_the_run_policy() {        let dir = std::env::temp_dir().join(format!("penelope-workflow-policy-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let mut ctx = crate::config::load(&dir.join(".penelope"));
        ctx.cfg["run_policy"] = json!("all");
        assert_eq!(workflow_ctx(&ctx).cfg["run_policy"], json!("safe"));
        let _ = fs::remove_dir_all(&dir);
    }
}
