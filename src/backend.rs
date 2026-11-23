//! The main model. Two ways in: the official claude command (uses the user's own login) or an OpenAI-compatible
//! endpoint with an API key or a local model.

use crate::config::{which, Ctx};
use crate::mcp;
use serde_json::{json, Value};
use std::process::{Command, Stdio};

pub const RULES: &str = "You work inside Kate, a small agent harness. Rules: \
1. Use the tool run for every shell command. Long output comes back condensed by a weaker model. Ask run_detail for raw lines only when you need them. \
2. Use fetch for web pages and give it the question you want answered. \
3. When the user states a rule, a preference or a fact to keep, store it with memory_note. At the start of a task, look up the topic with memory_go. \
4. To look at a web page or a picture use the screencodec tools, never a screenshot. \
5. Answer briefly.";

pub struct Chat {
    session: Option<String>,
    history: Vec<Value>,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

impl Chat {
    pub fn new() -> Chat {
        Chat { session: None, history: Vec::new(), input_tokens: 0, output_tokens: 0 }
    }

    pub fn turn(&mut self, ctx: &Ctx, user: &str, memory: &str) -> Result<String, String> {
        let system = if memory.is_empty() { RULES.to_string() } else { format!("{}\n\nNotes from the memory map for this request:\n{}", RULES, memory) };
        match ctx.cfg["main"]["kind"].as_str().unwrap_or("claude-cli") {
            "openai" => self.turn_openai(ctx, user, &system),
            _ => self.turn_claude(ctx, user, &system),
        }
    }

    fn turn_claude(&mut self, ctx: &Ctx, user: &str, system: &str) -> Result<String, String> {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let mut servers = json!({"kate": {"command": exe.to_string_lossy(), "args": ["mcp", "--root", ctx.root.to_string_lossy()]}});
        let mut allowed: Vec<String> = ["run", "run_detail", "fetch", "memory_go", "memory_note", "memory_map"].iter().map(|t| format!("mcp__kate__{}", t)).collect();
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
        cmd.args(["-p", user, "--output-format", "json", "--model", ctx.cfg["main"]["model"].as_str().unwrap_or("sonnet"),
                  "--append-system-prompt", system, "--strict-mcp-config", "--mcp-config", &cfg, "--tools", "", "--allowedTools", &allowed.join(",")]);
        if let Some(s) = &self.session {
            cmd.args(["--resume", s]);
        }
        let out = cmd.stdin(Stdio::null()).stderr(Stdio::null()).output().map_err(|e| format!("cannot start the claude command: {}", e))?;
        let text = String::from_utf8_lossy(&out.stdout);
        let v: Value = serde_json::from_str(text.trim()).map_err(|_| format!("unexpected answer of the claude command: {}", text.chars().take(200).collect::<String>()))?;
        if let Some(s) = v["session_id"].as_str() {
            self.session = Some(s.to_string());
        }
        self.input_tokens += v["usage"]["input_tokens"].as_u64().unwrap_or(0) + v["usage"]["cache_creation_input_tokens"].as_u64().unwrap_or(0);
        self.output_tokens += v["usage"]["output_tokens"].as_u64().unwrap_or(0);
        v["result"].as_str().map(|s| s.trim().to_string()).ok_or_else(|| "the claude command returned no result text".to_string())
    }

    fn turn_openai(&mut self, ctx: &Ctx, user: &str, system: &str) -> Result<String, String> {
        let m = &ctx.cfg["main"];
        let base = m["base_url"].as_str().unwrap_or("http://localhost:11434/v1").trim_end_matches('/').to_string();
        let key = m["api_key_env"].as_str().and_then(|n| std::env::var(n).ok()).unwrap_or_default();
        let tools: Vec<Value> = mcp::tool_specs().as_array().unwrap().iter()
            .map(|t| json!({"type": "function", "function": {"name": t["name"], "description": t["description"], "parameters": t["inputSchema"]}})).collect();
        self.history.push(json!({"role": "user", "content": user}));
        for _ in 0..8 {
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
            for c in calls {
                let name = c["function"]["name"].as_str().unwrap_or("");
                let args: Value = serde_json::from_str(c["function"]["arguments"].as_str().unwrap_or("{}")).unwrap_or(json!({}));
                let text = mcp::call(ctx, name, &args).unwrap_or_else(|e| e);
                self.history.push(json!({"role": "tool", "tool_call_id": c["id"], "content": text}));
            }
        }
        Err("stopped after 8 rounds of tool calls".to_string())
    }
}
