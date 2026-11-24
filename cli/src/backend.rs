//! The main model. Two ways in: the official claude command (uses the user's own login) or an OpenAI-compatible
//! endpoint with an API key or a local model.

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
    /// A finished block of text the main model wrote, also between two tool calls. It replaces the deltas before it.
    Text(String),
    ToolStart { id: String, name: String, detail: String },
    ToolEnd { id: String, ok: bool, summary: String },
    /// The privacy filter masked this many items in the request before the main model read it.
    Masked(usize),
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
    for key in ["command", "url", "query", "task", "path", "name", "text", "ref", "pattern"] {
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
        _ => "2. Every search goes to scout: research on the web, reading pages, finding files, the question where something is in the project. \
Give scout the full question and say what you need back. scout runs on a weaker model with its own tools and returns a short report. \
You have no search tools of your own.",
    };
    let home = std::env::var("HOME").unwrap_or_default();
    let cwd = std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default();
    let cwd = match cwd.strip_prefix(&home) {
        Some(rest) if home.len() > 1 => format!("~{}", rest),
        _ => cwd,
    };
    format!("You work inside Penelope, a small agent harness. The working directory is {cwd}. It is the project, also when a folder \
above it holds a repository. Paths are relative to it, and ~ stands for the home folder. Rules: \
1. Use the tool run for every shell command. Long output comes back condensed by a weaker model. Ask run_detail for raw lines only when you need them. \
{} \
3. Use read for a file you are about to change, then edit for a part of it or write for the whole file. A change waits for the user's yes. \
4. For a task with three steps or more, keep the list of steps current with plan. \
5. When the user states a rule, a preference or a fact to keep, store it with memory_note. At the start of a task, look up the topic with memory_go. \
6. To look at a web page as it is drawn or at a picture use the screencodec tools, never a screenshot. \
7. Answer briefly.", look, cwd = cwd)
}

pub struct Chat {
    pub session: Option<String>,
    history: Vec<Value>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub turns: u64,
    /// Set by another thread to stop the running turn.
    pub cancel: Arc<AtomicBool>,
    /// Process id of the running claude command, 0 when none runs.
    pub pid: Arc<AtomicU32>,
}

impl Chat {
    pub fn new() -> Chat {
        Chat { session: None, history: Vec::new(), input_tokens: 0, output_tokens: 0, turns: 0,
               cancel: Arc::new(AtomicBool::new(false)), pid: Arc::new(AtomicU32::new(0)) }
    }

    /// Goes on with a stored conversation of the claude command.
    pub fn resume(entry: &Value) -> Chat {
        let mut chat = Chat::new();
        chat.session = entry["id"].as_str().map(String::from);
        chat.turns = entry["turns"].as_u64().unwrap_or(0);
        chat.input_tokens = entry["in"].as_u64().unwrap_or(0);
        chat.output_tokens = entry["out"].as_u64().unwrap_or(0);
        chat
    }

    pub fn turn(&mut self, ctx: &Ctx, user: &str, memory: &str) -> Result<String, String> {
        self.turn_with(ctx, user, memory, &mut |_| {})
    }

    /// One turn. `on` receives the text and the tool calls while the turn runs.
    pub fn turn_with(&mut self, ctx: &Ctx, user: &str, memory: &str, on: &mut dyn FnMut(Event)) -> Result<String, String> {
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
        let out = match ctx.cfg["main"]["kind"].as_str().unwrap_or("claude-cli") {
            "openai" => self.turn_openai(ctx, user, &system, on),
            _ => self.turn_claude(ctx, user, &system, on),
        };
        if out.is_ok() {
            self.turns += 1;
            if let Some(id) = &self.session {
                crate::sessions::record(ctx, id, user, self.turns, (self.input_tokens, self.output_tokens));
            }
        }
        out
    }

    fn turn_claude(&mut self, ctx: &Ctx, user: &str, system: &str, on: &mut dyn FnMut(Event)) -> Result<String, String> {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let mut servers = json!({"penelope": {"command": exe.to_string_lossy(), "args": ["mcp", "--root", ctx.root.to_string_lossy()]}});
        if let Ok(dir) = std::env::var("PENELOPE_APPROVE_DIR") {
            servers["penelope"]["env"] = json!({"PENELOPE_APPROVE_DIR": dir});       // the interface answers questions about unsafe commands
        }
        let mut allowed: Vec<String> = mcp::main_tools(ctx).iter().map(|t| format!("mcp__penelope__{}", t)).collect();
        if let Some(obj) = crate::plugins::servers(ctx).as_object() {
            for (k, s) in obj {
                servers[k] = s.clone();        // tools of plugin servers are not on the allow list: the model sees them, a call is refused
            }
        }
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
        self.input_tokens += v["usage"]["input_tokens"].as_u64().unwrap_or(0) + v["usage"]["cache_creation_input_tokens"].as_u64().unwrap_or(0);
        self.output_tokens += v["usage"]["output_tokens"].as_u64().unwrap_or(0);
        if v["is_error"].as_bool().unwrap_or(false) {
            return Err(v["result"].as_str().unwrap_or("the claude command reported an error").trim().to_string());
        }
        v["result"].as_str().map(|s| s.trim().to_string()).ok_or_else(|| "the claude command returned no result text".to_string())
    }

    fn turn_openai(&mut self, ctx: &Ctx, user: &str, system: &str, on: &mut dyn FnMut(Event)) -> Result<String, String> {
        let m = &ctx.cfg["main"];
        let base = m["base_url"].as_str().unwrap_or("http://localhost:11434/v1").trim_end_matches('/').to_string();
        let key = m["api_key_env"].as_str().and_then(|n| std::env::var(n).ok()).unwrap_or_default();
        let mine = mcp::main_tools(ctx);
        let tools: Vec<Value> = mcp::tool_specs().as_array().unwrap().iter().filter(|t| mine.contains(&t["name"].as_str().unwrap_or("")))
            .map(|t| json!({"type": "function", "function": {"name": t["name"], "description": t["description"], "parameters": t["inputSchema"]}})).collect();
        self.history.push(json!({"role": "user", "content": user}));
        for _ in 0..8 {
            if self.cancel.load(Ordering::SeqCst) {
                return Err("stopped".to_string());
            }
            let mut messages = vec![json!({"role": "system", "content": system})];
            messages.extend(self.history.iter().cloned());
            let body = json!({"model": m["model"].as_str().unwrap_or(""), "messages": messages, "tools": tools, "temperature": 0});
            let mut req = ureq::post(&format!("{}/chat/completions", base)).timeout(std::time::Duration::from_secs(300));
            if !key.is_empty() {
                req = req.set("Authorization", &format!("Bearer {}", key));
            }
            let resp: Value = req.send_json(body).map_err(|e| format!("request failed: {}", e))?.into_json().map_err(|e| e.to_string())?;
            self.input_tokens += resp["usage"]["prompt_tokens"].as_u64().unwrap_or(0);
            self.output_tokens += resp["usage"]["completion_tokens"].as_u64().unwrap_or(0);
            let msg = resp["choices"][0]["message"].clone();
            let calls = msg["tool_calls"].as_array().cloned().unwrap_or_default();
            self.history.push(msg.clone());
            if calls.is_empty() {
                return Ok(msg["content"].as_str().unwrap_or("").trim().to_string());
            }
            if let Some(text) = msg["content"].as_str().map(str::trim).filter(|s| !s.is_empty()) {
                on(Event::Text(text.to_string()));
            }
            for c in calls {
                let name = c["function"]["name"].as_str().unwrap_or("");
                let args: Value = serde_json::from_str(c["function"]["arguments"].as_str().unwrap_or("{}")).unwrap_or(json!({}));
                let id = c["id"].as_str().unwrap_or("").to_string();
                on(Event::ToolStart { id: id.clone(), name: short_name(name), detail: tool_detail(&args) });
                let called = mcp::call(ctx, name, &args);
                on(Event::ToolEnd { id, ok: called.is_ok(), summary: first_line(called.as_deref().unwrap_or_else(|e| e.as_str())) });
                let text = called.unwrap_or_else(|e| e);
                self.history.push(json!({"role": "tool", "tool_call_id": c["id"], "content": text}));
            }
        }
        Err("stopped after 8 rounds of tool calls".to_string())
    }
}
