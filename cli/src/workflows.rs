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
            max_parallel: 4,
            confirm: true,
            cancel: Arc::new(AtomicBool::new(false)),
            ask: None,
            resume_id: None,
        }
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
    start_eur: f64,
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
fn key(prompt: &str, opts: &Map) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    let opts_text = format!("{:?}", opts);
    for b in prompt.as_bytes().iter().chain(opts_text.as_bytes()) {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{:016x}", h)
}

fn journal_line(dir: &Path, entry: &Value) -> Result<(), String> {
    let path = dir.join("journal.jsonl");
    let line = serde_json::to_string(entry).map_err(|e| e.to_string())?;
    let mut text = fs::read_to_string(&path).unwrap_or_default();
    text.push_str(&line);
    text.push('\n');
    fs::write(&path, text).map_err(|e| format!("cannot write {}: {}", path.display(), e))
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

fn project(ctx: &Ctx) -> PathBuf {
    ctx.root.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| ctx.root.clone())
}

fn run_agent(st: &mut State, prompt: &str, opts: &Map) -> Result<Dynamic, Box<EvalAltResult>> {
    if st.cancel.load(Ordering::SeqCst) {
        return Err(fail("workflow stopped"));
    }
    let label = opts.get("label").map(|v| v.to_string()).unwrap_or_default();
    let phase = opts.get("phase").map(|v| v.to_string()).unwrap_or_default();
    let worktree = opts.get("worktree").map(|v| v.as_bool().unwrap_or(false)).unwrap_or(false);
    let schema = opts.get("schema").map(|v| serde_json::from_str::<Value>(&v.to_string())).transpose()
        .map_err(|e| fail(format!("schema: {}", e)))?;
    let k = key(prompt, opts);
    if let Some(v) = st.resume.get(&k) {
        return dynamic(v.clone());
    }
    if st.budget_eur > 0.0 && st.spent_eur >= st.budget_eur {
        return Err(fail(format!("budget of {:.4} EUR is used", st.budget_eur)));
    }
    if st.budget_tokens > 0 && st.spent_tokens >= st.budget_tokens {
        return Err(fail(format!("budget of {} tokens is used", st.budget_tokens)));
    }
    let before = crate::cost::total_eur(&st.ctx);
    let before_tokens = crate::cost::total_tokens(&st.ctx);
    let (model, effort) = opts_model(opts);
    let text = if worktree {
        let name = format!("wf-{}-{}", st.id, if label.is_empty() { "part" } else { &label });
        let (path, _branch) = crate::work::create(&project(&st.ctx), &name)
            .map_err(|e| fail(format!("worktree: {}", e)))?;
        let goal = format!("{}\n\nWork in this worktree. The check is hidden.", prompt);
        let out = crate::eval::with_task_cwd(&path, || crate::clusters::run_once(&st.ctx, &goal, model.clone(), &effort));
        let _ = std::process::Command::new("git").arg("-C").arg(&path).args(["worktree", "remove", "--force"]).arg(&path).output();
        out.map_err(|e| fail(e))?
    } else {
        crate::clusters::run_once(&st.ctx, prompt, model, &effort).map_err(|e| fail(e))?
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
    let entry = json!({"key": k.clone(), "phase": phase, "label": label, "prompt": prompt, "value": value, "eur": eur});
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
        if st.cancel.load(Ordering::SeqCst) { return Err(fail("workflow stopped")); }
        let mut jobs = Vec::new();
        for item in items.iter() {
            let m = item.clone().try_cast::<Map>().ok_or_else(|| fail("parallel needs maps"))?;
            let prompt = m.get("prompt").map(|v| v.to_string()).unwrap_or_default();
            let opts = m.get("opts").and_then(|v| v.clone().try_cast::<Map>()).unwrap_or_default();
            jobs.push((prompt, opts));
        }
        if jobs.len() > st.max_parallel {
            return Err(fail(format!("parallel is limited to {} agents", st.max_parallel)));
        }
        let ctx = st.ctx.clone();
        let before = crate::cost::total_eur(&st.ctx);
        let before_tokens = crate::cost::total_tokens(&st.ctx);
        let mut handles = Vec::new();
        for (prompt, opts) in jobs {
            let (model, effort) = opts_model(&opts);
            let c = ctx.clone();
            handles.push(std::thread::spawn(move || crate::clusters::run_once(&c, &prompt, model, &effort)));
        }
        let mut out = Array::new();
        for h in handles {
            let text = h.join().map_err(|_| fail("a parallel agent panicked"))?
                .map_err(|e| fail(e))?;
            out.push(Dynamic::from(text));
        }
        let after = crate::cost::total_eur(&st.ctx);
        let after_tokens = crate::cost::total_tokens(&st.ctx);
        st.spent_eur += (after - before).max(0.0);
        st.spent_tokens += after_tokens.saturating_sub(before_tokens);
        let values: Vec<Value> = out.iter().map(value).collect::<Result<_, _>>()?;
        journal_line(&st.dir, &json!({"phase": "parallel", "results": values})).map_err(|e| fail(e))?;
        Ok(out)
    });

    let s = state.clone();
    e.register_fn("pipeline", move |items: Array, stages: Array| -> Result<Array, Box<EvalAltResult>> {
        let mut st = s.borrow_mut();
        if st.cancel.load(Ordering::SeqCst) { return Err(fail("workflow stopped")); }
        let stage_maps: Vec<Map> = stages.iter()
            .map(|v| v.clone().try_cast::<Map>().ok_or_else(|| fail("pipeline stages need maps")))
            .collect::<Result<_, _>>()?;
        let mut jobs = Vec::new();
        for item in items.iter() {
            let item = item.to_string();
            let mut specs = Vec::new();
            for stage in &stage_maps {
                let template = stage.get("prompt").map(|v| v.to_string()).unwrap_or_default();
                let (model, effort) = opts_model(stage);
                specs.push((template, model, effort));
            }
            jobs.push((item, specs));
        }
        let ctx = st.ctx.clone();
        let before = crate::cost::total_eur(&st.ctx);
        let before_tokens = crate::cost::total_tokens(&st.ctx);
        let mut handles = Vec::new();
        for (item, specs) in jobs {
            let c = ctx.clone();
            handles.push(std::thread::spawn(move || -> Result<Vec<String>, String> {
                let mut out = Vec::new();
                let mut current = item;
                for (template, model, effort) in specs {
                    let prompt = template.replace("{item}", &current);
                    current = crate::clusters::run_once(&c, &prompt, model, &effort)?;
                    out.push(current.clone());
                }
                Ok(out)
            }));
        }
        let mut out = Array::new();
        for h in handles {
            let row = h.join().map_err(|_| fail("a pipeline agent panicked"))?.map_err(|e| fail(e))?;
            out.push(dynamic(json!(row))?);
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
        let (ctx, depth) = {
            let st = s.borrow();
            if st.depth >= 1 { return Err(fail("a workflow may call only one level of workflows")); }
            (st.ctx.clone(), st.depth + 1)
        };
        let path = ctx.root.join("workflows").join(format!("{}.rhai", name.as_str()));
        if !path.is_file() { return Err(fail(format!("no workflow at {}", path.display()))); }
        let value = rhai::serde::from_dynamic::<Value>(&Dynamic::from_map(args)).map_err(|e| fail(e.to_string()))?;
        let result = run_script(&ctx, &path, RunOptions { args: value, confirm: false, ..RunOptions::default() }, depth)
            .map_err(|e| fail(e))?;
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
    let resume = load_resume(&dir);
    let state = Rc::new(RefCell::new(State {
        ctx: ctx.clone(),
        id: id.clone(),
        dir: dir.clone(),
        start_eur: crate::cost::total_eur(ctx),
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
        let agents = script.matches("agent(").count() + script.matches("parallel(").count();
        let text = format!("Workflow {}: about {} agent calls, budget {:.2} EUR. Start?", name, agents, options.budget_eur);
        let mut st = state.borrow_mut();
        let Some(ask) = st.ask.as_mut() else { return Err("workflow needs a confirmation, but no ask dialog is attached".to_string()); };
        let answers = ask(&[json!({"question": text, "kind": "yes_no"})])?;
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
        let out = run(&ctx, &path, RunOptions { confirm: false, ..RunOptions::default() }).unwrap();
        assert_eq!(out.output.as_array().map(|a| a.len()), Some(2));
        let (ctx, dir) = ctx_with_fake(vec![(200, reply.clone())]);
        let path = dir.join("pipe.rhai");
        fs::write(&path, "pipeline([\"a\"], [#{prompt: \"x {item}\", model: \"worker\"}])").unwrap();
        let out = run(&ctx, &path, RunOptions { confirm: false, ..RunOptions::default() }).unwrap();
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
}
