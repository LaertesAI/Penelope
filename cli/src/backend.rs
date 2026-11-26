//! The main model. Three ways in: the official claude command (uses the user's own login), an endpoint in the chat
//! completions format of OpenAI, or the Messages API of Anthropic. The two HTTP formats share the tool loop in `llm.rs`.

use crate::config::{which, Ctx};
use crate::mcp;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

/// What happens inside one turn, reported while it runs.
pub enum Event {
    /// A few characters of text, as the main model writes them.
    Delta(String),
    /// A few characters of the model's thinking, when the provider streams it. Shown while it runs, never stored.
    Thinking(String),
    /// A finished block of text the main model wrote, also between two tool calls. It replaces the deltas before it.
    Text(String),
    ToolStart { id: String, name: String, detail: String },
    ToolEnd { id: String, ok: bool, summary: String },
    /// The privacy filter masked this many items in the request before the main model read it.
    Masked(usize),
    /// The privacy filter masked this many items in the text block that was reported just before.
    MaskedAnswer(usize),
}

/// The text blocks of one turn after the privacy filter. The result of a turn repeats its last block. A block that
/// passed the filter once is taken from this list, so its masked items are counted once.
#[derive(Default)]
struct Answers {
    seen: Vec<(String, String)>,
}

impl Answers {
    /// The block as the screen and the transcript get it, and the number of items newly masked in it.
    fn block(&mut self, ctx: &Ctx, text: &str) -> (String, usize) {
        if let Some((_, shown)) = self.seen.iter().find(|(raw, _)| raw == text.trim()) {
            return (shown.clone(), 0);
        }
        let (shown, hits) = crate::tools::guard_answer(ctx, text);
        self.seen.push((text.trim().to_string(), shown.clone()));
        (shown, hits)
    }
}

/// `mcp__penelope__run` becomes `run`, `mcp__screencodec__open` becomes `screen open`.
fn short_name(name: &str) -> String {
    let rest = name.strip_prefix("mcp__").unwrap_or(name);
    match rest.split_once("__") {
        Some(("penelope", tool)) => tool.to_string(),
        Some(("screencodec", tool)) => format!("screen {}", tool),
        Some((server, tool)) => format!("{} {}", server, tool),
        None => rest.to_string(),
    }
}

/// The one argument of a tool call a person wants to read: the command, the address, the query.
fn tool_detail(input: &Value) -> String {
    if let Some(items) = input["items"].as_array() {          // the plan: how far it is and the step in work
        let done = items.iter().filter(|i| i["state"] == "done").count();
        let now = items.iter().find(|i| i["state"] == "doing").or_else(|| items.iter().find(|i| i["state"] == "pending"));
        return format!("{} of {} done{}", done, items.len(), now.and_then(|i| i["text"].as_str()).map(|t| format!(" · {}", t)).unwrap_or_default());
    }
    for key in ["command", "url", "query", "task", "question", "path", "name", "text", "ref", "pattern"] {
        if let Some(s) = input[key].as_str().filter(|s| !s.is_empty()) {
            return s.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(160).collect();
        }
    }
    match input.as_object() {
        Some(o) if !o.is_empty() => input.to_string().chars().take(160).collect(),
        _ => String::new(),
    }
}

fn first_line(text: &str) -> String {
    text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").chars().take(160).collect()
}

/// The rules the main model works by. They depend on the setting "delegate": who does the searching.
pub fn rules(ctx: &Ctx) -> String {
    let look = match ctx.cfg["delegate"].as_str().unwrap_or("auto") {
        "off" => "2. Use web_search to find pages and fetch to read one. Use find, grep and read to look at files.",
        "offer" => "2. Hand every lookup that needs more than one step to scout: research on the web, the question where something is in the \
project. scout runs on a weaker model with its own tools and returns a short report. For a single quick lookup use web_search, fetch, find, grep or read yourself.",
        "web" => "2. Research on the web goes to scout: give it the full question and say what you need back. scout runs on a weaker model with \
its own tools and returns a short report. You have no web tools of your own. For files use tree, find, grep, outline and read yourself.",
        _ => "2. Every search goes to scout: research on the web, reading pages, finding files, the question where something is in the project. \
Give scout the full question and say what you need back. scout runs on a weaker model with its own tools and returns a short report. \
You have no search tools of your own, only tree and outline for a first look.",
    };
    let home = crate::platform::home_str();
    let cwd = std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default();
    let cwd = match cwd.strip_prefix(&home) {
        Some(rest) if home.len() > 1 => format!("~{}", rest),
        _ => cwd,
    };
    let tests = match ctx.cfg["test_command"].as_str().map(str::trim).filter(|c| !c.is_empty()) {
        Some(command) => format!(" After a successful write or edit, Penelope runs `{}` and appends the last 20 lines to the tool result. \
When a result says TEST LOOP STOPPED, call no more tools in that turn: answer with the failure, what you changed and what remains open.", command),
        None => String::new(),
    };
    format!("You work inside Penelope, a small agent harness. The working directory is {cwd}. It is the project, also when a folder \
above it holds a repository. Paths are relative to it, and ~ stands for the home folder. Rules: \
1. Use the tool run for every shell command. Long output comes back condensed by a weaker model. Ask run_detail for raw lines only when you need them. \
{} \
3. Use read for a file you are about to change, then edit for a part of it or write for the whole file. A change waits for the user's yes.{tests} \
4. For a task with three steps or more, keep the list of steps current with plan. \
5. When the user states a rule, a preference or a fact to keep, store it with memory_note. At the start of a task, look up the topic with memory_go. \
6. To look at a web page as it is drawn or at a picture use the tool screen, never a screenshot of the terminal. It drives ScreenCodec and moves Penelope's cursor before a click; cursor moves her pointer without a click. \
7. When two readings of a request lead to different work, ask with ask_user before you guess. \
8. A rule that binds every later session goes to the shared memory: memory_note with scope global. A fact of this project stays in the project. \
9. When a fixed command would save steps next time, write it as a tool with tool_new; a call runs through run and asks like run. \
10. After a change, hand the claim and the command output to check. Report what passed and what is still open. \
11. Answer briefly. Today is {today} (UTC).", look, cwd = cwd, tests = tests,
            today = &crate::sessions::stamp(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0))[..10])
}

pub struct Chat {
    pub session: Option<String>,
    history: Vec<Value>,
    /// The wire format the history is written in: `openai` or `anthropic`. Empty before the first turn through an endpoint.
    history_kind: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub turns: u64,
    /// Set by another thread to stop the running turn.
    pub cancel: Arc<AtomicBool>,
    /// Process id of the running claude command, 0 when none runs.
    pub pid: Arc<AtomicU32>,
    /// The tool calls of the last turn, for the lessons the weaker model draws from them.
    pub trace: Vec<crate::tools::Call>,
}

impl Chat {
    pub fn new() -> Chat {
        Chat { session: None, history: Vec::new(), history_kind: String::new(), input_tokens: 0, output_tokens: 0, turns: 0,
               cancel: Arc::new(AtomicBool::new(false)), pid: Arc::new(AtomicU32::new(0)), trace: Vec::new() }
    }

    /// Goes on with a stored conversation. A conversation through an HTTP endpoint gets its message history back from disk.
    pub fn resume(ctx: &Ctx, entry: &Value) -> Chat {
        let mut chat = Chat::new();
        chat.session = entry["id"].as_str().map(String::from);
        if let Some((kind, messages)) = chat.session.as_deref().and_then(|id| crate::sessions::load_history(ctx, id)) {
            chat.history = messages;
            chat.history_kind = kind;
        }
        chat.turns = entry["turns"].as_u64().unwrap_or(0);
        chat.input_tokens = entry["in"].as_u64().unwrap_or(0);
        chat.output_tokens = entry["out"].as_u64().unwrap_or(0);
        chat
    }

    /// One turn. `on` receives the text and the tool calls while the turn runs.
    pub fn turn_with(&mut self, ctx: &Ctx, user: &str, memory: &str, on: &mut dyn FnMut(Event)) -> Result<String, String> {
        if let Some(family) = crate::crew::family_of(&ctx.cfg["main"]) {
            return Err(format!("The main model is {} 1, one of the Laertes families. It is in training and cannot answer yet. \
Choose a hosted model: penelope provider use NAME --role main.", family));
        }
        crate::tools::begin_turn();
        self.cancel.store(false, Ordering::SeqCst);
        let (user, masked) = crate::tools::guard_count(ctx, user);
        let user = user.as_str();
        if masked > 0 {
            on(Event::Masked(masked));
        }
        let mut system = if memory.is_empty() { rules(ctx) } else { format!("{}\n\nNotes from the memory map for this request:\n{}", rules(ctx), memory) };
        let skills = crate::plugins::skill_index(ctx);
        if !skills.is_empty() {
            system.push_str(&format!("\n\nInstalled skills (read one with the tool skill before you use it):\n{}", skills));
        }
        let lessons = crate::memory::Memory::open(&ctx.root).branch("tools", 900);
        if !lessons.is_empty() {
            system.push_str(&format!("\n\nLessons from earlier tool use, learned in this folder:\n{}", lessons));
        }
        // Only the names of the waypoints, not their notes: a lookup pulls the text when it is needed.
        let index = crate::context::memory_index(ctx, 14);
        if !index.is_empty() {
            system.push_str(&format!("\n\nMemory map, names only. Read a way with memory_go when the request needs it:\n{}", index));
        }
        let (mut trace, mut ids): (Vec<crate::tools::Call>, Vec<String>) = (Vec::new(), Vec::new());
        let mut answers = Answers::default();
        let mut watch = |e: Event| {
            if let Event::Text(text) = &e {          // a finished block passes the privacy filter before anyone draws or stores it
                let (shown, hits) = answers.block(ctx, text);
                on(Event::Text(shown));
                if hits > 0 {
                    on(Event::MaskedAnswer(hits));
                }
                return;
            }
            match &e {
                Event::ToolStart { id, name, detail } => {
                    trace.push((name.clone(), detail.clone(), true, String::new()));
                    ids.push(id.clone());
                }
                Event::ToolEnd { id, ok, summary } => {
                    if let Some(i) = ids.iter().position(|x| x == id) {
                        trace[i].2 = *ok;
                        trace[i].3 = summary.clone();
                    }
                }
                _ => {}
            }
            on(e)
        };
        let out = match ctx.cfg["main"]["kind"].as_str().unwrap_or("claude-cli") {
            kind if crate::llm::is_api(kind) => self.turn_api(ctx, user, &system, &mut watch),
            _ => self.turn_claude(ctx, user, &system, &mut watch),
        };
        self.trace = trace;
        let out = out.map(|answer| {
            let (shown, hits) = answers.block(ctx, &answer);
            if hits > 0 {
                on(Event::MaskedAnswer(hits));
            }
            shown
        });
        if out.is_ok() {
            self.turns += 1;
            if let Some(id) = &self.session {
                crate::sessions::record(ctx, id, user, self.turns, (self.input_tokens, self.output_tokens));
            }
        }
        out
    }

    fn turn_claude(&mut self, ctx: &Ctx, user: &str, system: &str, on: &mut dyn FnMut(Event)) -> Result<String, String> {
        if !self.history.is_empty() {
            return Err(format!("this session ran through an endpoint in the format {}. The claude command cannot go on with it. Start a new session.", self.history_kind));
        }
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let mut servers = json!({"penelope": {"command": exe.to_string_lossy(), "args": ["mcp", "--root", ctx.root.to_string_lossy()]}});
        let task_cwd = crate::eval::task_cwd();
        if !task_cwd.is_empty() {
            servers["penelope"]["env"] = json!({"PENELOPE_TASK_CWD": task_cwd});   // an eval task: the MCP server uses this folder
        }
        if let Ok(dir) = std::env::var("PENELOPE_APPROVE_DIR") {
            servers["penelope"]["env"]["PENELOPE_APPROVE_DIR"] = json!(dir);       // the interface answers questions about unsafe commands
        }
        let mut allowed: Vec<String> = mcp::main_tools(ctx).iter().map(|t| format!("mcp__penelope__{}", t)).collect();
        for t in crate::forge::specs(ctx) {          // a tool Penelope wrote is offered to the claude command too
            if let Some(name) = t["name"].as_str() { allowed.push(format!("mcp__penelope__{}", name)); }
        }
        if let Some(obj) = crate::plugins::servers(ctx).as_object() {
            for (k, s) in obj {
                servers[k] = s.clone();        // the model sees the tools of every plugin server
            }
        }
        allowed.extend(crate::plugins::allowed(ctx));        // a call works for plugins with "allow": true only
        let sc_cmd = ctx.cfg["screencodec"]["command"].as_str().filter(|c| !c.is_empty()).map(std::path::PathBuf::from).or_else(|| which("screencodec-mcp"));
        if let Some(sc) = sc_cmd {
            servers["screencodec"] = json!({"command": sc.to_string_lossy(), "args": [], "env": {
                "SDL_MCP_ALLOW_HOSTS": ctx.cfg["screencodec"]["hosts"].as_str().unwrap_or(""),
                "SDL_MCP_IMAGE_DIRS": ctx.cfg["screencodec"]["images"].as_str().unwrap_or("")}});
            for t in ["open", "screen", "zoom", "click", "type", "select", "key", "scroll", "image"] {
                allowed.push(format!("mcp__screencodec__{}", t));
            }
        }
        let cfg = json!({"mcpServers": servers}).to_string();
        let mut cmd = Command::new("claude");
        cmd.args(["-p", user, "--output-format", "stream-json", "--verbose", "--include-partial-messages", "--model", ctx.cfg["main"]["model"].as_str().unwrap_or("sonnet"),
                  "--append-system-prompt", system, "--strict-mcp-config", "--mcp-config", &cfg, "--tools", "", "--allowedTools", &allowed.join(",")]);
        if let Some(level) = crate::llm::effort(&ctx.cfg["main"]) {
            cmd.args(["--effort", level]);
        }
        if let Some(s) = &self.session {
            cmd.args(["--resume", s]);
        }
        let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn()
            .map_err(|e| format!("cannot start the claude command: {}", e))?;
        self.pid.store(child.id(), Ordering::SeqCst);
        let mut result: Option<Value> = None;
        let mut seen = String::new();
        for line in BufReader::new(child.stdout.take().unwrap()).lines() {
            let line = match line { Ok(l) => l, Err(_) => break };
            let v: Value = match serde_json::from_str(line.trim()) { Ok(v) => v, Err(_) => { seen = line; continue } };
            match v["type"].as_str().unwrap_or("") {
                "assistant" => {
                    for c in v["message"]["content"].as_array().cloned().unwrap_or_default() {
                        match c["type"].as_str().unwrap_or("") {
                            "text" => {
                                let text = c["text"].as_str().unwrap_or("").trim().to_string();
                                if !text.is_empty() {
                                    on(Event::Text(text));
                                }
                            }
                            "tool_use" => on(Event::ToolStart { id: c["id"].as_str().unwrap_or("").to_string(),
                                                                name: short_name(c["name"].as_str().unwrap_or("")), detail: tool_detail(&c["input"]) }),
                            _ => {}
                        }
                    }
                }
                "user" => {
                    for c in v["message"]["content"].as_array().cloned().unwrap_or_default() {
                        if c["type"].as_str() == Some("tool_result") {
                            let text = match &c["content"] {
                                Value::String(s) => s.clone(),
                                Value::Array(a) => a.iter().filter_map(|x| x["text"].as_str()).collect::<Vec<_>>().join("\n"),
                                _ => String::new(),
                            };
                            on(Event::ToolEnd { id: c["tool_use_id"].as_str().unwrap_or("").to_string(),
                                                ok: !c["is_error"].as_bool().unwrap_or(false), summary: first_line(&text) });
                        }
                    }
                }
                "stream_event" => {
                    let e = &v["event"];
                    if e["type"].as_str() == Some("content_block_delta") && e["delta"]["type"].as_str() == Some("text_delta") {
                        if let Some(t) = e["delta"]["text"].as_str().filter(|t| !t.is_empty()) {
                            on(Event::Delta(t.to_string()));
                        }
                    }
                    if e["type"].as_str() == Some("content_block_delta") && e["delta"]["type"].as_str() == Some("thinking_delta") {
                        if let Some(t) = e["delta"]["thinking"].as_str().filter(|t| !t.is_empty()) {
                            on(Event::Thinking(t.to_string()));
                        }
                    }
                }
                "result" => result = Some(v),
                _ => {}
            }
        }
        let _ = child.wait();
        self.pid.store(0, Ordering::SeqCst);
        if self.cancel.load(Ordering::SeqCst) {
            return Err("stopped".to_string());
        }
        let v = result.ok_or_else(|| format!("the claude command ended without a result: {}", seen.chars().take(200).collect::<String>()))?;
        if let Some(s) = v["session_id"].as_str() {
            self.session = Some(s.to_string());
        }
        let tin = v["usage"]["input_tokens"].as_u64().unwrap_or(0) + v["usage"]["cache_creation_input_tokens"].as_u64().unwrap_or(0);
        let tout = v["usage"]["output_tokens"].as_u64().unwrap_or(0);
        self.input_tokens += tin;
        self.output_tokens += tout;
        crate::cost::record(ctx, "lead", ctx.cfg["main"]["model"].as_str().unwrap_or(""), tin, tout);
        if v["is_error"].as_bool().unwrap_or(false) {
            return Err(v["result"].as_str().unwrap_or("the claude command reported an error").trim().to_string());
        }
        v["result"].as_str().map(|s| s.trim().to_string()).ok_or_else(|| "the claude command returned no result text".to_string())
    }

    /// One turn through an HTTP endpoint. The harness runs the tool loop itself and writes the history to disk afterwards.
    fn turn_api(&mut self, ctx: &Ctx, user: &str, system: &str, on: &mut dyn FnMut(Event)) -> Result<String, String> {
        let m = &ctx.cfg["main"];
        let kind = crate::llm::kind_of(m).to_string();
        if self.history.is_empty() && self.session.is_some() && self.turns > 0 {
            return Err("this session ran through the claude command, so its history is not stored here. Start a new session.".to_string());
        }
        if !self.history.is_empty() && self.history_kind != kind {
            return Err(format!("this session is stored in the format {} and the main model now uses {}. Start a new session.", self.history_kind, kind));
        }
        let mine = mcp::main_tools(ctx);
        let tools: Vec<Value> = mcp::tool_specs().as_array().unwrap().iter().filter(|t| mine.contains(&t["name"].as_str().unwrap_or(""))).cloned().collect();
        let before = self.history.len();
        self.history.push(crate::llm::user_message(user));
        self.history_kind = kind.clone();
        let mut usage = crate::llm::Usage::default();
        let mut history = std::mem::take(&mut self.history);
        // Both closures report to the interface, so the events go through one shared cell.
        let events = std::cell::RefCell::new(on);
        let out = crate::llm::run_loop(m, system, &mut history, &tools, 12, Some(&*self.cancel), &mut usage,
            &mut |call| {
                (*events.borrow_mut())(Event::ToolStart { id: call.id.clone(), name: short_name(&call.name), detail: tool_detail(&call.args) });
                let called = if mine.contains(&call.name.as_str()) {
                    crate::tools::dispatch_retry(ctx, "lead", &call.name, &call.args)
                } else {
                    (format!("NOT called. unknown tool {} — it is not in the tool list of this turn.", call.name), false)
                };
                (*events.borrow_mut())(Event::ToolEnd { id: call.id.clone(), ok: called.1, summary: first_line(&called.0) });
                called
            },
            &mut |text| (*events.borrow_mut())(Event::Text(text.to_string())));
        self.history = history;
        self.input_tokens += usage.tokens_in;
        self.output_tokens += usage.tokens_out;
        crate::cost::record(ctx, "lead", m["model"].as_str().unwrap_or(""), usage.tokens_in, usage.tokens_out);
        if out.is_err() {
            self.history.truncate(before);          // a failed turn leaves no half exchange behind
            return out;
        }
        let id = self.session.get_or_insert_with(crate::sessions::new_id).clone();
        crate::sessions::save_history(ctx, &id, &kind, m["model"].as_str().unwrap_or(""), &self.history);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_text_block_is_masked_once_and_counted_once() {
        let root = std::env::temp_dir().join(format!("penelope-answers-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut ctx = crate::config::load(&root);
        assert_eq!(ctx.cfg["privacy_answers"], true);
        let raw = "The file sets API_TOKEN=q7Lw2ZxP91aaBBcc and PORT=8080.";
        let mut answers = Answers::default();
        let (shown, hits) = answers.block(&ctx, raw);
        assert!(hits == 1 && shown.contains("API_TOKEN=[masked:secret]") && shown.contains("PORT=8080") && !shown.contains("q7Lw2"), "{}", shown);
        assert_eq!(answers.block(&ctx, &format!("{}\n", raw)), (shown, 0));          // the result of the turn repeats the block
        let stats: Value = serde_json::from_str(&std::fs::read_to_string(root.join("stats.json")).unwrap()).unwrap();
        assert_eq!(stats["privacy"]["masked"], 1);
        ctx.cfg["privacy_answers"] = json!(false);
        assert_eq!(Answers::default().block(&ctx, raw), (raw.to_string(), 0));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A folder with one file, and settings whose main model is an endpoint of the given kind at `address`.
    fn endpoint(name: &str, kind: &str, address: &str, key_env: &str) -> (Ctx, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("penelope-llm-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut ctx = crate::config::load(&dir.join(".penelope"));
        std::fs::write(dir.join("note.txt"), "hello from the note\n").unwrap();
        let model = match kind { "anthropic" => "claude-opus-5-5", "openai" => "gpt-6.2-astra", _ => "test-model" };
        ctx.cfg["main"] = json!({"kind": kind, "base_url": address, "model": model, "api_key_env": key_env});
        ctx.cfg["auto_learn"] = json!(false);
        (ctx, dir)
    }

    fn names(events: &[String]) -> String {
        events.join(" | ")
    }

    fn collect(events: &mut Vec<String>) -> impl FnMut(Event) + '_ {
        |e| events.push(match e {
            Event::Text(t) => format!("text {}", t),
            Event::ToolStart { name, detail, .. } => format!("start {} {}", name, detail),
            Event::ToolEnd { ok, summary, .. } => format!("end {} {}", ok, summary),
            Event::Delta(_) | Event::Thinking(_) | Event::Masked(_) | Event::MaskedAnswer(_) => "other".to_string(),
        })
    }

    #[test]
    fn openai_shape_runs_one_round_of_tool_calls_and_resumes_from_disk() {
        std::env::set_var("PENELOPE_TEST_KEY_OPENAI", "test-key-openai-0123456789");
        let dir = std::env::temp_dir().join(format!("penelope-llm-openai-{}", std::process::id()));
        let call = json!({"path": "note.txt", "cwd": dir.to_string_lossy()}).to_string();
        let (address, seen) = crate::llm::fake::serve(vec![
            (200, json!({"choices": [{"finish_reason": "tool_calls", "message": {"role": "assistant", "content": null, "tool_calls": [
                {"id": "call_1", "type": "function", "function": {"name": "read", "arguments": call}}]}}], "usage": {"prompt_tokens": 100, "completion_tokens": 20}}).to_string()),
            (200, json!({"choices": [{"finish_reason": "stop", "message": {"role": "assistant", "content": "The note says hello."}}], "usage": {"prompt_tokens": 150, "completion_tokens": 8}}).to_string()),
            (200, json!({"choices": [{"finish_reason": "stop", "message": {"role": "assistant", "content": "Second answer."}}], "usage": {"prompt_tokens": 170, "completion_tokens": 3}}).to_string()),
        ]);
        let (ctx, dir) = endpoint("openai", "openai", &format!("{}/v1", address), "PENELOPE_TEST_KEY_OPENAI");
        let mut chat = Chat::new();
        let mut events = Vec::new();
        let answer = chat.turn_with(&ctx, "What does note.txt say?", "", &mut collect(&mut events)).unwrap();
        assert_eq!(answer, "The note says hello.");
        assert!(names(&events).starts_with("start read note.txt | end true ") && events[1].contains("lines 1 to 1 of 1"), "{}", names(&events));
        assert_eq!((chat.input_tokens, chat.output_tokens, chat.turns), (250, 28, 1));
        {
            let seen = seen.lock().unwrap();
            assert_eq!((seen.len(), seen[0].path.as_str()), (2, "/v1/chat/completions"));
            assert!(seen[0].headers.contains(&"authorization: bearer test-key-openai-0123456789".to_string()));
            let first = &seen[0].body;
            assert_eq!((first["model"].as_str(), first["messages"][0]["role"].as_str(), first["messages"][1]["content"].as_str()),
                       (Some("gpt-6.2-astra"), Some("system"), Some("What does note.txt say?")));
            assert!(first["temperature"].is_null() && first["tools"][0]["type"] == "function" && first["tools"][0]["function"]["parameters"]["type"] == "object");
            assert_eq!(first["reasoning_effort"], json!("high"));          // an old setting without the field gets the default too
            let second = seen[1].body["messages"].as_array().unwrap();
            assert_eq!(second.len(), 4);
            assert_eq!(second[2]["tool_calls"][0]["id"], "call_1");
            assert_eq!((second[3]["role"].as_str(), second[3]["tool_call_id"].as_str()), (Some("tool"), Some("call_1")));
            assert!(second[3]["content"].as_str().unwrap().contains("hello from the note"));
        }
        // The session is in the list and its messages are on disk, so another process can go on with it.
        let entry = crate::sessions::pick(&ctx, "1").unwrap();
        let id = entry["id"].as_str().unwrap();
        assert!(id.starts_with("api-") && chat.session.as_deref() == Some(id));
        let (kind, stored) = crate::sessions::load_history(&ctx, id).unwrap();
        assert_eq!((kind.as_str(), stored.len()), ("openai", 4));
        let mut again = Chat::resume(&ctx, &entry);
        assert_eq!(again.turn_with(&ctx, "And once more?", "", &mut |_| {}).unwrap(), "Second answer.");
        let third = seen.lock().unwrap()[2].body["messages"].as_array().unwrap().clone();
        assert_eq!((third.len(), third[1]["content"].as_str(), third[5]["content"].as_str()), (6, Some("What does note.txt say?"), Some("And once more?")));
        assert_eq!((again.turns, crate::sessions::load_history(&ctx, id).unwrap().1.len()), (2, 6));
        // The same session cannot go on in the other wire format or through the claude command.
        let mut other = ctx.clone();
        other.cfg["main"]["kind"] = json!("anthropic");
        assert!(Chat::resume(&ctx, &entry).turn_with(&other, "x", "", &mut |_| {}).unwrap_err().contains("stored in the format openai"));
        other.cfg["main"] = json!({"kind": "claude-cli", "model": "haiku"});
        assert!(Chat::resume(&ctx, &entry).turn_with(&other, "x", "", &mut |_| {}).unwrap_err().contains("claude command cannot go on"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn anthropic_shape_runs_one_round_of_tool_calls_and_reports_a_refusal() {
        std::env::set_var("PENELOPE_TEST_KEY_ANTHROPIC", "test-key-anthropic-0123456789");
        let dir = std::env::temp_dir().join(format!("penelope-llm-anthropic-{}", std::process::id()));
        let blocks = json!([{"type": "thinking", "thinking": "", "signature": "sig-1"}, {"type": "text", "text": "I will read the note."},
                            {"type": "tool_use", "id": "toolu_1", "name": "read", "input": {"path": "note.txt", "cwd": dir.to_string_lossy()}}]);
        let (address, seen) = crate::llm::fake::serve(vec![
            (200, json!({"id": "msg_1", "type": "message", "role": "assistant", "content": blocks, "stop_reason": "tool_use", "usage": {"input_tokens": 90, "output_tokens": 30}}).to_string()),
            (200, json!({"content": [{"type": "text", "text": "The note says hello."}], "stop_reason": "end_turn", "usage": {"input_tokens": 140, "cache_read_input_tokens": 10, "output_tokens": 9}}).to_string()),
            (200, json!({"content": [], "stop_reason": "refusal", "stop_details": {"type": "refusal", "category": "cyber"}, "usage": {"input_tokens": 5, "output_tokens": 0}}).to_string()),
        ]);
        let (ctx, dir) = endpoint("anthropic", "anthropic", &address, "PENELOPE_TEST_KEY_ANTHROPIC");
        let mut chat = Chat::new();
        let mut events = Vec::new();
        let answer = chat.turn_with(&ctx, "What does note.txt say?", "", &mut collect(&mut events)).unwrap();
        assert_eq!(answer, "The note says hello.");
        assert!(names(&events).starts_with("text I will read the note. | start read note.txt | end true "), "{}", names(&events));
        assert_eq!((chat.input_tokens, chat.output_tokens), (240, 39));
        {
            let seen = seen.lock().unwrap();
            assert_eq!((seen.len(), seen[0].path.as_str()), (2, "/v1/messages"));
            let h = &seen[0].headers;
            assert!(h.contains(&"x-api-key: test-key-anthropic-0123456789".to_string()) && h.contains(&format!("anthropic-version: {}", crate::llm::ANTHROPIC_VERSION)), "{:?}", h);
            assert!(!h.iter().any(|l| l.starts_with("authorization:") || l.starts_with("anthropic-beta:")));
            let first = &seen[0].body;
            assert!(first["system"].as_str().unwrap().starts_with("You work inside Penelope") && first["max_tokens"] == 16000 && first["temperature"].is_null());
            assert_eq!((&first["thinking"], &first["output_config"]), (&json!({"type": "adaptive"}), &json!({"effort": "high"})));
            assert_eq!(first["messages"], json!([{"role": "user", "content": "What does note.txt say?"}]));
            let tool = first["tools"].as_array().unwrap().iter().find(|t| t["name"] == "read").unwrap();
            assert!(tool["input_schema"]["properties"]["path"].is_object() && tool["type"].is_null());
            let second = seen[1].body["messages"].as_array().unwrap();
            assert_eq!(second.len(), 3);
            assert_eq!(second[1], json!({"role": "assistant", "content": blocks}));          // every block goes back unchanged, the thinking block too
            let result = &second[2]["content"][0];
            assert_eq!((second[2]["role"].as_str(), result["type"].as_str(), result["tool_use_id"].as_str()), (Some("user"), Some("tool_result"), Some("toolu_1")));
            assert!(result["content"].as_str().unwrap().contains("hello from the note") && result["is_error"].is_null());
        }
        let id = chat.session.clone().unwrap();
        assert_eq!(crate::sessions::load_history(&ctx, &id).map(|(k, m)| (k, m.len())), Some(("anthropic".to_string(), 4)));
        // A refusal ends the turn with its category. The stored history stays as it was.
        let refused = chat.turn_with(&ctx, "Next request", "", &mut |_| {}).unwrap_err();
        assert!(refused.contains("declined this request: refusal (category cyber)"), "{}", refused);
        assert_eq!((chat.turns, chat.history.len(), crate::sessions::load_history(&ctx, &id).unwrap().1.len()), (1, 4, 4));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_scout_and_the_weaker_model_work_through_the_messages_api() {
        let (address, seen) = crate::llm::fake::serve(vec![
            (200, json!({"content": [{"type": "tool_use", "id": "toolu_1", "name": "run", "input": {"command": "rm -rf build"}},
                                     {"type": "tool_use", "id": "toolu_2", "name": "write", "input": {"path": "x.txt", "text": "x"}}],
                         "stop_reason": "tool_use", "usage": {"input_tokens": 50, "output_tokens": 12}}).to_string()),
            (200, json!({"content": [{"type": "text", "text": "Nothing was removed."}], "stop_reason": "end_turn", "usage": {"input_tokens": 70, "output_tokens": 6}}).to_string()),
            (200, json!({"content": [{"type": "text", "text": "three lines"}], "stop_reason": "end_turn", "usage": {"input_tokens": 9, "output_tokens": 2}}).to_string()),
        ]);
        let (mut ctx, dir) = endpoint("scout", "anthropic", &address, "");
        ctx.cfg["scout"] = ctx.cfg["main"].clone();
        let report = crate::scout::run(&ctx, "Remove the build folder.", &dir.to_string_lossy());
        assert!(report.starts_with("report of the scout on claude-opus-5-5: 2 steps, 120 tokens in, 18 out") && report.ends_with("Nothing was removed."), "{}", report);
        assert!(!crate::scout::inside());
        {
            let seen = seen.lock().unwrap();
            assert_eq!(seen[0].body["tools"].as_array().unwrap().len(), crate::scout::TOOLS.len());
            assert!(!seen[0].headers.iter().any(|l| l.starts_with("x-api-key:")));          // no variable named, no key header
            let results = seen[1].body["messages"][2]["content"].as_array().unwrap().clone();
            assert_eq!(results.len(), 2);          // both results travel in one user message
            assert!(results[0]["content"].as_str().unwrap().starts_with("NOT run. A scout runs commands of the safe list only"));
            assert!(results[1]["content"].as_str().unwrap().starts_with("NOT called. A scout has no tool write") && results[1]["is_error"] == true && !dir.join("x.txt").exists());
        }
        assert_eq!(crate::weak::ask(&ctx.cfg["main"], "Condense.", "a\nb\nc").unwrap(), "three lines");
        assert!(seen.lock().unwrap()[2].body["tools"].is_null());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_key_reaches_the_settings_the_transcript_or_any_stored_file() {
        const KEY: &str = "sk-test-7c1e9a4b2d6f8031-not-a-real-key";
        std::env::set_var("PENELOPE_TEST_KEY_STORE", KEY);
        let dir = std::env::temp_dir().join(format!("penelope-llm-key-{}", std::process::id()));
        let call = json!({"path": "note.txt", "cwd": dir.to_string_lossy()}).to_string();
        // This endpoint repeats the key wherever it can: in an error, next to a tool call and in the answer.
        let (address, seen) = crate::llm::fake::serve(vec![
            (401, json!({"error": {"message": format!("Incorrect API key provided: {}", KEY)}}).to_string()),
            (200, json!({"choices": [{"finish_reason": "tool_calls", "message": {"role": "assistant", "content": format!("Your key is {}.", KEY), "tool_calls": [
                {"id": "call_1", "type": "function", "function": {"name": "read", "arguments": call}}]}}], "usage": {"prompt_tokens": 1, "completion_tokens": 1}}).to_string()),
            (200, json!({"choices": [{"finish_reason": "stop", "message": {"role": "assistant", "content": format!("Done with {}.", KEY)}}]}).to_string()),
        ]);
        let (mut ctx, dir) = endpoint("key", "openai", "http://unused", "");
        let set = crate::providers::use_provider(&mut ctx, &["openai".to_string(), "--base-url".to_string(), format!("{}/v1", address), "--model".to_string(), "test-model".to_string()]);
        assert!(set.starts_with("set: main model test-model on openai") && set.contains("in OPENAI_API_KEY"), "{}", set);
        ctx.cfg["main"]["api_key_env"] = json!("PENELOPE_TEST_KEY_STORE");
        std::fs::write(ctx.root.join("config.json"), serde_json::to_string_pretty(&ctx.cfg).unwrap()).unwrap();
        let mut chat = Chat::new();
        let failed = chat.turn_with(&ctx, "First try", "", &mut |_| {}).unwrap_err();
        assert!(failed.contains("status 401") && failed.contains("Incorrect API key provided: [key]") && !failed.contains(KEY), "{}", failed);
        let mut events = Vec::new();
        let answer = chat.turn_with(&ctx, "What does note.txt say?", "", &mut collect(&mut events)).unwrap();
        assert_eq!(answer, "Done with [key].");
        // The interface stores what it showed as the transcript of the session.
        let mut items: Vec<Value> = events.iter().map(|e| json!({"shown": e})).collect();
        items.push(json!({"reply": answer, "error": failed}));
        let id = chat.session.clone().unwrap();
        crate::sessions::save_transcript(&ctx, &id, &items);
        let screen = format!("{}{}{}{}", names(&events), crate::providers::list(&ctx), crate::doctor_text(&ctx), set);
        assert!(!screen.contains(KEY) && screen.contains("PENELOPE_TEST_KEY_STORE") && screen.contains("text Your key is [key]."), "{}", screen);
        // Every request carried the key in its header and nowhere in its body.
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 3);
        assert!(seen.iter().all(|r| r.headers.contains(&format!("authorization: bearer {}", KEY)) && !r.body.to_string().contains(KEY)));
        // No file below the settings folder holds the key: config.json, sessions.json, the transcript, the history.
        fn files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for e in std::fs::read_dir(dir).unwrap().flatten() {
                if e.path().is_dir() { files(&e.path(), out) } else { out.push(e.path()) }
            }
        }
        let mut all = Vec::new();
        files(&ctx.root, &mut all);
        let names: Vec<String> = all.iter().map(|p| p.strip_prefix(&ctx.root).unwrap().to_string_lossy().replace('\\', "/")).collect();
        for needed in ["config.json", "sessions.json", &format!("sessions/{}.json", id), &format!("sessions/{}.history.json", id)] {
            assert!(names.iter().any(|n| n == needed), "{} is missing in {:?}", needed, names);
        }
        for path in &all {
            assert!(!String::from_utf8_lossy(&std::fs::read(path).unwrap()).contains(KEY), "{} holds the key", path.display());
        }
        assert!(std::fs::read_to_string(ctx.root.join("config.json")).unwrap().contains("\"api_key_env\": \"PENELOPE_TEST_KEY_STORE\""));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
