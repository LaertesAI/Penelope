//! The two wire formats for models behind an HTTP address: `openai` (chat completions) and `anthropic` (the Messages
//! API). Both run the same tool loop: send the history, run the tool calls of the answer, send the results, until an
//! answer comes without a tool call. The history keeps the format of its kind, so it can go back unchanged.

use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;

/// The version header of the Messages API, from the API reference on 2026-11-24. The setting `api_version` replaces it.
pub const ANTHROPIC_VERSION: &str = "2026-10-21";
/// Upper limit of one answer in tokens for the `anthropic` kind, where the field is required. The setting `max_tokens` replaces it.
pub const MAX_TOKENS: u64 = 16000;

pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub args: Value,
}

/// One answer of a model.
pub struct Reply {
    pub text: String,
    pub calls: Vec<ToolCall>,
    /// The message as the history keeps it: every block the provider sent, in its order.
    pub message: Value,
    pub tokens_in: u64,
    pub tokens_out: u64,
    /// `stop_reason` or `finish_reason` of the provider.
    pub stop: String,
}

/// What a tool loop used.
#[derive(Default)]
pub struct Usage {
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub steps: u64,
    /// The largest input of one request in the loop: the context at its fullest.
    pub max_in: u64,
}

pub fn is_api(kind: &str) -> bool {
    kind == "openai" || kind == "anthropic"
}

pub fn kind_of(model: &Value) -> &str {
    model["kind"].as_str().unwrap_or("claude-cli")
}

pub fn user_message(text: &str) -> Value {
    json!({"role": "user", "content": text})
}

/// The messages that carry tool results back, in the format of the kind. The Messages API takes all results of one
/// answer in a single user message. Chat completions take one message with the role `tool` per call.
pub fn result_messages(kind: &str, results: &[(String, String, bool)]) -> Vec<Value> {
    if kind == "text" {
        // Text tool calls (`"tool_format": "text"`): the results go back as one user message of <tool_response> blocks.
        let body: Vec<String> = results.iter().map(|(id, text, ok)| format!("<tool_response id=\"{}\"{}>\n{}\n</tool_response>", id,
                                                                            if *ok { "" } else { " error=\"true\"" }, text)).collect();
        return vec![user_message(&body.join("\n"))];
    }
    if kind == "anthropic" {
        let blocks: Vec<Value> = results.iter().map(|(id, text, ok)| {
            let mut b = json!({"type": "tool_result", "tool_use_id": id, "content": text});
            if !ok {
                b["is_error"] = json!(true);
            }
            b
        }).collect();
        return vec![json!({"role": "user", "content": blocks})];
    }
    results.iter().map(|(id, text, _)| json!({"role": "tool", "tool_call_id": id, "content": text})).collect()
}

/// A text from a provider with the key taken out. A provider can repeat a key in an error message.
fn scrub(text: &str, key: &str) -> String {
    if key.len() < 6 { text.to_string() } else { text.replace(key, "[key]") }
}

/// One raw HTTP answer. The body stays text until the key is scrubbed out of it.
struct RawReply {
    status: u16,
    location: Option<String>,
    retry_after: Option<u64>,
    body: String,
}

fn send_raw(url: &str, headers: &[(String, String)], body: &Value, seconds: u64) -> Result<RawReply, String> {
    let mut req = crate::net::agent(seconds).post(url);
    for (name, value) in headers {
        req = req.set(name, value);
    }
    let read = |resp: ureq::Response| -> Result<RawReply, String> {
        let status = resp.status();
        let location = resp.header("location").map(str::to_string);
        let retry_after = resp.header("retry-after").and_then(|s| s.parse::<u64>().ok());
        let body = resp.into_string().map_err(|e| format!("the answer of {} could not be read: {}", url, e))?;
        Ok(RawReply { status, location, retry_after, body })
    };
    match req.send_json(body.clone()) {
        Ok(resp) => read(resp),
        Err(ureq::Error::Status(_, resp)) => read(resp),
        Err(e) => Err(format!("no answer from {}: {}", url, e)),
    }
}

/// Sends one request in a thread and returns as soon as the cancel flag is set. The socket of the abandoned request
/// closes at its own timeout; the part itself is over at once, which is what a kill promises.
fn send_cancel(url: &str, headers: &[(String, String)], body: &Value, seconds: u64, cancel: &AtomicBool) -> Result<RawReply, String> {
    if cancel.load(Ordering::SeqCst) {
        return Err("stopped".to_string());
    }
    let (tx, rx) = mpsc::channel();
    let label = url.to_string();
    let (url, headers, body) = (url.to_string(), headers.to_vec(), body.clone());
    std::thread::spawn(move || { let _ = tx.send(send_raw(&url, &headers, &body, seconds)); });
    loop {
        if cancel.load(Ordering::SeqCst) {
            return Err("stopped".to_string());
        }
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(result) => return result,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => return Err(format!("no answer from {}", label)),
        }
    }
}

/// Sleeps in short pieces so a kill does not wait for the retry pause. False means cancelled.
fn sleep_cancel(seconds: u64, cancel: Option<&AtomicBool>) -> bool {
    let until = std::time::Instant::now() + Duration::from_secs(seconds);
    while std::time::Instant::now() < until {
        if cancel.map(|c| c.load(Ordering::SeqCst)).unwrap_or(false) {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    cancel.map(|c| !c.load(Ordering::SeqCst)).unwrap_or(true)
}

/// Sends one request and returns the body as JSON. Status 429 and 5xx are tried again twice. A set cancel flag ends
/// the wait at once, including the pause between two tries.
fn post(url: &str, headers: &[(&str, String)], body: &Value, key: &str, seconds: u64, cancel: Option<&AtomicBool>) -> Result<Value, String> {
    let headers: Vec<(String, String)> = headers.iter().map(|(name, value)| (name.to_string(), value.clone())).collect();
    let mut wait = 2;
    for attempt in 0..3 {
        if cancel.map(|c| c.load(Ordering::SeqCst)).unwrap_or(false) {
            return Err("stopped".to_string());
        }
        let raw = match cancel {
            Some(cancel) => send_cancel(url, &headers, body, seconds, cancel)?,
            None => send_raw(url, &headers, body, seconds)?,
        };
        if (300..400).contains(&raw.status) {
            let to = raw.location.as_deref().unwrap_or("an address the answer did not name");
            return Err(format!("{} answered with a redirect to {}. Redirects are not followed, so the key cannot travel to another host.", url, to));
        }
        let text = scrub(&raw.body, key);
        if (200..300).contains(&raw.status) {
            return serde_json::from_str(&text).map_err(|_| format!("{} answered with text that is no JSON", url));
        }
        let code = raw.status;
        if (code == 429 || code >= 500) && attempt < 2 {
            let pause = raw.retry_after.unwrap_or(wait).min(20);
            if !sleep_cancel(pause, cancel) {
                return Err("stopped".to_string());
            }
            wait *= 3;
            continue;
        }
        let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        let said = v["error"]["message"].as_str().or(v["message"].as_str()).or(v["error"].as_str()).map(String::from)
            .unwrap_or_else(|| text.split_whitespace().collect::<Vec<_>>().join(" "));
        let hint = match code {
            401 | 403 => ". Check the environment variable with the key (penelope providers shows whether it is set)",
            404 => ". Check the address and the model name in config.json",
            _ => "",
        };
        return Err(format!("{} answered with status {}: {}{}", url, code, said.chars().take(300).collect::<String>(), hint));
    }
    Err(format!("{} gave no answer in 3 tries", url))
}

/// One request to the model of a setting. `history` holds messages in the format of the setting's kind. `tools` are
/// tool descriptions in the MCP form with `name`, `description` and `inputSchema`.
/// The effort level of a setting, when the provider accepts one.
pub fn effort(model: &Value) -> Option<&str> {
    let name = model["model"].as_str().unwrap_or("");
    let provider = match kind_of(model) {
        "claude-cli" => "claude-cli",
        "anthropic" => "anthropic",
        kind => crate::providers::of(model).map(|p| p.name).unwrap_or(match kind { "openai" => "openai", _ => "" }),
    };
    // A model without an effort field gets none, even when an old setting still names a level.
    if !crate::providers::supports_effort(provider, name) {
        return None;
    }
    model["effort"].as_str().filter(|e| !e.trim().is_empty()).or_else(|| Some(crate::providers::default_effort(provider, name)))
}

pub fn chat(model: &Value, system: &str, history: &[Value], tools: &[Value]) -> Result<Reply, String> {
    chat_cancel(model, system, history, tools, None)
}

/// `chat` with a cancel flag. The flag ends an HTTP request that is still waiting for its answer.
pub fn chat_cancel(model: &Value, system: &str, history: &[Value], tools: &[Value], cancel: Option<&AtomicBool>) -> Result<Reply, String> {
    let key = crate::providers::key(model);
    let name = model["model"].as_str().unwrap_or("");
    let seconds = model["timeout"].as_u64().unwrap_or(300);
    if kind_of(model) == "anthropic" {
        let base = model["base_url"].as_str().unwrap_or("https://api.anthropic.com").trim_end_matches('/');
        let limit = model["max_tokens"].as_u64().unwrap_or(MAX_TOKENS);
        let mut body = json!({"model": name, "max_tokens": limit, "messages": history});
        if !system.is_empty() {
            body["system"] = json!(system);
        }
        if let Some(level) = effort(model) {
            // Claude 5 turns thinking on with adaptive thinking and takes its depth from output_config.effort.
            // The older budget_tokens form is no longer accepted by the Claude 5 models.
            body["thinking"] = json!({"type": "adaptive"});
            body["output_config"] = json!({"effort": level});
        }
        if !tools.is_empty() {
            body["tools"] = tools.iter().map(|t| json!({"name": t["name"], "description": t["description"], "input_schema": t["inputSchema"]})).collect();
        }
        let mut headers = vec![("anthropic-version", model["api_version"].as_str().unwrap_or(ANTHROPIC_VERSION).to_string())];
        if let Some(extra) = model["headers"].as_object() {
            for (name, value) in extra {
                if let Some(v) = value.as_str() { headers.push((name.as_str(), v.to_string())); }
            }
        }
        if !key.is_empty() {
            headers.push(("x-api-key", key.clone()));
        }
        if !model["fallbacks"].is_null() {          // a second model takes over inside the same request when the first declines
            body["fallbacks"] = model["fallbacks"].clone();
            headers.push(("anthropic-beta", "server-side-fallback-2026-07-01".to_string()));
        }
        let v = post(&format!("{}/v1/messages", base), &headers, &body, &key, seconds, cancel)?;
        let stop = v["stop_reason"].as_str().unwrap_or("").to_string();
        let blocks = v["content"].as_array().cloned().unwrap_or_default();
        if blocks.is_empty() && stop.is_empty() {
            return Err(format!("the answer holds no content: {}", v.to_string().chars().take(200).collect::<String>()));
        }
        let text = blocks.iter().filter(|b| b["type"] == "text").filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join("\n").trim().to_string();
        let mut calls: Vec<ToolCall> = blocks.iter().filter(|b| b["type"] == "tool_use").map(|b| ToolCall {
            id: b["id"].as_str().unwrap_or("").to_string(), name: b["name"].as_str().unwrap_or("").to_string(), args: b["input"].clone() }).collect();
        let mut text = text;
        if calls.is_empty() && model["tool_format"].as_str() == Some("text") && (text.contains("<tool_call>") || text.contains(DSML)) {
            let (found, rest) = text_tool_calls(&text);
            calls = found;
            text = rest;
        }
        let u = &v["usage"];
        let tokens_in = ["input_tokens", "cache_creation_input_tokens", "cache_read_input_tokens"].iter().map(|k| u[*k].as_u64().unwrap_or(0)).sum();
        let detail = v["stop_details"]["category"].as_str().map(|c| format!(" (category {})", c)).unwrap_or_default();
        return Ok(Reply { text, calls, message: json!({"role": "assistant", "content": blocks}), tokens_in, tokens_out: u["output_tokens"].as_u64().unwrap_or(0),
                          stop: format!("{}{}", stop, if stop == "refusal" { detail } else { String::new() }) });
    }
    let base = model["base_url"].as_str().unwrap_or("http://localhost:11434/v1").trim_end_matches('/');
    let mut messages = Vec::new();
    if !system.is_empty() {
        messages.push(json!({"role": "system", "content": system}));
    }
    messages.extend(history.iter().cloned());
    let mut body = json!({"model": name, "messages": messages});
    if !tools.is_empty() {
        body["tools"] = tools.iter().map(|t| json!({"type": "function", "function": {"name": t["name"], "description": t["description"], "parameters": t["inputSchema"]}})).collect();
    }
    for field in ["temperature", "max_tokens"] {          // sent only when the setting names them, some models refuse a temperature
        if !model[field].is_null() {
            body[field] = model[field].clone();
        }
    }
    if let Some(level) = effort(model) {
        body["reasoning_effort"] = json!(level);
    }
    let mut headers = Vec::new();
    if let Some(extra) = model["headers"].as_object() {
        for (name, value) in extra {
            if let Some(v) = value.as_str() { headers.push((name.as_str(), v.to_string())); }
        }
    }
    if !key.is_empty() {
        headers.push(("Authorization", format!("Bearer {}", key)));
    }
    let v = post(&format!("{}/chat/completions", base), &headers, &body, &key, seconds, cancel)?;
    let choice = &v["choices"][0];
    if !choice["message"].is_object() {
        return Err(format!("the answer holds no message: {}", v.to_string().chars().take(200).collect::<String>()));
    }
    let message = choice["message"].clone();
    let mut calls: Vec<ToolCall> = message["tool_calls"].as_array().cloned().unwrap_or_default().iter().map(|c| ToolCall {
        id: c["id"].as_str().unwrap_or("").to_string(), name: c["function"]["name"].as_str().unwrap_or("").to_string(),
        // The arguments arrive as a JSON text. A text that does not parse becomes Null and the loop reports it to the model.
        args: match &c["function"]["arguments"] {
            Value::String(s) if s.trim().is_empty() => json!({}),
            Value::String(s) => serde_json::from_str(s).unwrap_or(Value::Null),
            Value::Object(o) => Value::Object(o.clone()),
            _ => json!({}),
        } }).collect();
    let mut text = message["content"].as_str().unwrap_or("").trim().to_string();
    // A server without a tool-call parser hands Qwen's <tool_call> blocks back as text. They become calls here. The
    // DSML and GLM templates count only in the text tool format: in native mode an answer that quotes such markup
    // (from a README or a fetched page) stays text.
    let templates = model["tool_format"].as_str() == Some("text");
    if calls.is_empty() && (text.contains("<tool_call>") || (templates && text.contains(DSML))) {
        let (found, rest) = text_tool_calls_with(&text, templates);
        calls = found;
        text = rest;
    }
    Ok(Reply { text, calls, message,
               tokens_in: v["usage"]["prompt_tokens"].as_u64().unwrap_or(0), tokens_out: v["usage"]["completion_tokens"].as_u64().unwrap_or(0),
               stop: choice["finish_reason"].as_str().unwrap_or("").to_string() })
}

/// Tool calls written as text in the Qwen and Hermes format: one JSON object with `name` and `arguments` between
/// `<tool_call>` and `</tool_call>`, any number per answer. A `<think>` block is dropped. Returns the calls and the text
/// that is left, for example the reason after `none:` when no tool fits. A block that does not parse stays in the text.
/// GLM's `<arg_key>` bodies and DeepSeek's DSML blocks count too.
pub fn text_tool_calls(content: &str) -> (Vec<ToolCall>, String) {
    text_tool_calls_with(content, true)
}

/// `text_tool_calls`; with `templates` false only the JSON block counts, the GLM and DSML templates stay text.
pub fn text_tool_calls_with(content: &str, templates: bool) -> (Vec<ToolCall>, String) {
    let mut rest = content.to_string();
    if let (Some(start), Some(end)) = (rest.find("<think>"), rest.find("</think>")) {
        if end > start {
            rest.replace_range(start..end + "</think>".len(), "");
        }
    }
    let mut calls = Vec::new();
    let mut left = String::new();
    let mut cursor = rest.as_str();
    while let Some(open) = cursor.find("<tool_call>") {
        left.push_str(&cursor[..open]);
        let after = &cursor[open + "<tool_call>".len()..];
        let Some(close) = after.find("</tool_call>") else {
            left.push_str(&cursor[open..]);
            cursor = "";
            break;
        };
        let body = after[..close].trim();
        match serde_json::from_str::<Value>(body) {
            Ok(v) if v["name"].as_str().map(|n| !n.is_empty()).unwrap_or(false) => {
                let args = match &v["arguments"] {
                    Value::Object(o) => Value::Object(o.clone()),
                    Value::String(s) => serde_json::from_str(s).unwrap_or(Value::Null),
                    Value::Null => json!({}),
                    _ => Value::Null,
                };
                calls.push(ToolCall { id: format!("text-call-{}", calls.len() + 1), name: v["name"].as_str().unwrap().to_string(), args });
            }
            _ => match glm_call(body).filter(|_| templates) {
                Some((name, args)) => calls.push(ToolCall { id: format!("text-call-{}", calls.len() + 1), name, args }),
                None => left.push_str(&cursor[open..open + "<tool_call>".len() + close + "</tool_call>".len()]),
            },
        }
        cursor = &after[close + "</tool_call>".len()..];
    }
    left.push_str(cursor);
    if templates && calls.is_empty() && left.contains(DSML) {
        let (found, rest) = dsml_tool_calls(&left);
        calls = found;
        left = rest;
    }
    let text = left.trim();
    let text = text.strip_prefix("none:").map(str::trim).unwrap_or(text);
    (calls, text.to_string())
}

/// One call in GLM's own template, the body between `<tool_call>` and `</tool_call>`: the tool name, then pairs of
/// `<arg_key>K</arg_key><arg_value>V</arg_value>`. GLM-5.3 mixes that template with JSON when the prompt shows JSON
/// blocks, so the reader also takes `<arg_key>K": V</arg_value>`, attributes `K="V"`, and `name {json}` or
/// `name({json})`: in 39 stored first answers it reads 37, the strict template 7 of 20. Every value stays text (a JSON
/// string is unescaped), so `{"a": 1}` written as file content stays that text; `calibrate::typed_call` turns a value
/// into a number, bool, array or object where the tool's schema asks for one. None when the body does not start with
/// a tool name or holds text that is no pair.
pub fn glm_call(body: &str) -> Option<(String, Value)> {
    let name_re = regex::Regex::new(r"^\s*([A-Za-z_][A-Za-z0-9_.-]*)").unwrap();
    let m = name_re.captures(body)?;
    let name = m[1].to_string();
    let mut rest = body[m.get(0)?.end()..].trim();
    if let Some(inner) = rest.strip_prefix('(') {
        rest = inner.trim_end().strip_suffix(')').unwrap_or(inner).trim();
    }
    if rest.starts_with('{') {
        if let Some(Ok(Value::Object(o))) = serde_json::Deserializer::from_str(rest).into_iter::<Value>().next() {
            return Some((name, Value::Object(o)));
        }
    }
    let key = regex::Regex::new(r#"(?:^|<arg_key>|</arg_value>|"\s+)\s*"?([A-Za-z_][A-Za-z0-9_-]*)"?\s*(?:</arg_key>\s*<arg_value>|"\s*:\s*|:\s*|=\s*)"#).unwrap();
    let keys: Vec<regex::Captures> = key.captures_iter(rest).collect();
    if keys.is_empty() {
        return rest.trim_matches(|c: char| c == '{' || c == '}' || c.is_whitespace()).is_empty().then(|| (name, json!({})));
    }
    if keys[0].get(0)?.start() > 0 && !rest[..keys[0].get(0)?.start()].trim().is_empty() {
        return None;
    }
    let mut args = serde_json::Map::new();
    for (i, k) in keys.iter().enumerate() {
        let start = k.get(0)?.end();
        let end = keys.get(i + 1).and_then(|n| n.get(0)).map(|n| n.start()).unwrap_or(rest.len());
        args.insert(k[1].to_string(), json!(loose_value(&rest[start..end.max(start)])));
    }
    Some((name, Value::Object(args)))
}

/// The text of one loosely written value: closing tags and a trailing `}` or `)` go, a JSON string is unescaped, a
/// JSON value keeps its text, other text loses its surrounding quotes.
fn loose_value(raw: &str) -> String {
    let v = raw.replace("</arg_value>", "").replace("</arg_key>", "");
    let v = v.trim();
    let mut stream = serde_json::Deserializer::from_str(v).into_iter::<Value>();
    if let Some(Ok(x)) = stream.next() {
        let end = stream.byte_offset();
        if v[end..].trim().trim_matches(|c| c == '}' || c == ')' || c == '"').trim().is_empty() {
            return match x {
                Value::String(s) => s,
                _ => v[..end].to_string(),
            };
        }
    }
    let v = v.strip_prefix('"').unwrap_or(v);
    v.strip_suffix('"').unwrap_or(v).to_string()
}

/// The marker of DeepSeek's own text template for tool calls (fullwidth bars).
pub const DSML: &str = "\u{ff5c}DSML\u{ff5c}";

/// Tool calls in DeepSeek's DSML template, which DeepSeek V4 writes when no API tool field is set:
/// `<｜DSML｜ invoke name="run">` with one `<｜DSML｜ parameter name="command" string="true">VALUE</｜DSML｜ parameter>`
/// per argument, inside a `calls` or `function_calls` block. `string="true"` keeps the value as text; any other value is
/// read as JSON and stays text when it does not parse. DeepSeek often closes a tag short (`</｜DSML｜>`), repeats a
/// closing tag or leaves one out, so an invoke runs to the next invoke and a value to the next DSML tag. The text from
/// the first to the last DSML tag leaves the answer; everything else stays.
pub fn dsml_tool_calls(content: &str) -> (Vec<ToolCall>, String) {
    let bar = "\u{ff5c}";
    let open = format!("<{}DSML{}", bar, bar);
    let close = format!("</{}DSML{}", bar, bar);
    let invoke = regex::Regex::new(&format!(r#"<{b}DSML{b}\s*invoke\s+name="([^"]+)"\s*>"#, b = bar)).unwrap();
    let param = regex::Regex::new(&format!(r#"<{b}DSML{b}\s*parameter\s+name="([^"]+)"(?:\s+string="(true|false)")?\s*>"#, b = bar)).unwrap();
    let starts: Vec<regex::Captures> = invoke.captures_iter(content).collect();
    let mut calls = Vec::new();
    for (i, m) in starts.iter().enumerate() {
        let from = m.get(0).map(|x| x.end()).unwrap_or(0);
        let to = starts.get(i + 1).and_then(|n| n.get(0)).map(|n| n.start()).unwrap_or(content.len());
        let body = &content[from..to];
        let mut args = serde_json::Map::new();
        for p in param.captures_iter(body) {
            let vstart = p.get(0).map(|x| x.end()).unwrap_or(0);
            let tail = &body[vstart..];
            // The value ends at the next DSML tag, open or close.
            let vend = [tail.find(&close), tail.find(&open)].into_iter().flatten().min().unwrap_or(tail.len());
            let raw = &tail[..vend];
            let value = if p.get(2).map(|s| s.as_str()) == Some("true") {
                json!(raw)
            } else {
                serde_json::from_str(raw.trim()).unwrap_or_else(|_| json!(raw))
            };
            args.insert(p[1].to_string(), value);
        }
        calls.push(ToolCall { id: format!("text-call-{}", calls.len() + 1), name: m[1].to_string(), args: Value::Object(args) });
    }
    if calls.is_empty() {
        return (Vec::new(), content.to_string());
    }
    let first = [content.find(&open), content.find(&close)].into_iter().flatten().min().unwrap_or(0);
    let last_tag = content.rfind(&open).max(content.rfind(&close)).unwrap_or(first);
    let last = content[last_tag..].find('>').map(|i| last_tag + i + 1).unwrap_or(content.len());
    let rest = format!("{}{}", &content[..first], &content[last..]);
    (calls, rest.trim().to_string())
}

/// The tool loop. `history` grows by every message of the turn. `exec` runs one tool call and returns its text and
/// whether it worked. `on_text` gets the text a model writes next to tool calls. Returns the final answer.
#[allow(clippy::too_many_arguments)]
pub fn run_loop(model: &Value, system: &str, history: &mut Vec<Value>, tools: &[Value], rounds: usize, cancel: Option<&AtomicBool>, usage: &mut Usage,
                exec: &mut dyn FnMut(&ToolCall) -> (String, bool), on_text: &mut dyn FnMut(&str)) -> Result<String, String> {
    run_loop_with(model, system, history, tools, rounds, cancel, usage, &mut |_| None, exec, on_text)
}

/// `run_loop` with a tick after every model answer. The tick sees the usage of the whole loop; a returned text ends
/// the loop with that error. Clusters use it to check a budget and the zombie watch while a part is still running.
#[allow(clippy::too_many_arguments)]
pub fn run_loop_with(model: &Value, system: &str, history: &mut Vec<Value>, tools: &[Value], rounds: usize, cancel: Option<&AtomicBool>, usage: &mut Usage,
                     tick: &mut dyn FnMut(&Usage) -> Option<String>,
                     exec: &mut dyn FnMut(&ToolCall) -> (String, bool), on_text: &mut dyn FnMut(&str)) -> Result<String, String> {
    run_loop_full(model, system, history, tools, rounds, cancel, usage, tick, &mut || None, exec, on_text)
}

/// `run_loop_with` plus `after_round`, called once after the tools of a round ran and their results joined the history.
/// A returned text ends the turn with that answer and no further model call (calibration variant `stop-green`).
#[allow(clippy::too_many_arguments)]
pub fn run_loop_full(model: &Value, system: &str, history: &mut Vec<Value>, tools: &[Value], rounds: usize, cancel: Option<&AtomicBool>, usage: &mut Usage,
                     tick: &mut dyn FnMut(&Usage) -> Option<String>, after_round: &mut dyn FnMut() -> Option<String>,
                     exec: &mut dyn FnMut(&ToolCall) -> (String, bool), on_text: &mut dyn FnMut(&str)) -> Result<String, String> {
    let kind = if model["tool_format"].as_str() == Some("text") { "text".to_string() } else { kind_of(model).to_string() };
    for round in 0..rounds {
        if cancel.map(|c| c.load(Ordering::SeqCst)).unwrap_or(false) {
            return Err("stopped".to_string());
        }
        if let Some(stop) = tick(usage) {
            return Err(stop);
        }
        let reply = chat_cancel(model, system, history, tools, cancel)?;
        usage.tokens_in += reply.tokens_in;
        usage.max_in = usage.max_in.max(reply.tokens_in);
        usage.tokens_out += reply.tokens_out;
        if let Some(stop) = tick(usage) {
            return Err(stop);
        }
        let stop = reply.stop.as_str();
        if stop.starts_with("refusal") || stop == "content_filter" {
            return Err(format!("the model declined this request: {}. The request is not part of the stored history.", stop));
        }
        if stop == "model_context_window_exceeded" {
            return Err("the conversation is longer than the context window of the model. Start a new session.".to_string());
        }
        if stop == "max_tokens" || stop == "length" {
            if !reply.text.is_empty() {
                on_text(&reply.text);
            }
            return Err(format!("the answer stopped at the token limit ({}). Raise \"max_tokens\" in the model setting.", stop));
        }
        history.push(reply.message);
        if stop == "pause_turn" {          // the provider paused a long turn: the same history goes back and the turn goes on
            continue;
        }
        if reply.calls.is_empty() {
            // A code edit without a check gets one reminder, but only while a round is left for the answer to it.
            if round + 1 < rounds {
                if let Some(reminder) = crate::checker::before_final() {
                    history.push(user_message(&reminder));
                    continue;
                }
            }
            return Ok(reply.text);
        }
        if !reply.text.is_empty() {
            on_text(&reply.text);
        }
        let mut results = Vec::new();
        let mut stop_text: Option<String> = None;
        for call in &reply.calls {
            let (text, ok) = if call.args.is_null() {
                ("the arguments of this call were no valid JSON. Send the call again with a JSON object.".to_string(), false)
            } else {
                exec(call)
            };
            usage.steps += 1;
            if crate::tools::is_test_stop(&text) {
                if stop_text.is_none() {
                    stop_text = Some(text.clone());
                }
                // The model gets one more answer, without tools: it explains the failure instead of trying another edit.
                results.push((call.id.clone(), format!("{}\n\nDo not call another tool in this turn. Write the final answer now: name the test command, \
                                                       the failure and what remains open.", text), false));
            } else {
                results.push((call.id.clone(), text, ok));
            }
        }
        history.extend(result_messages(&kind, &results));
        if let Some(stop) = stop_text {
            let final_reply = match chat_cancel(model, system, history, &[], cancel) {
                Ok(reply) => reply,
                Err(e) if e == "stopped" => return Err(e),
                Err(e) => return Ok(format!("{}\n\nThe final answer could not be written: {}", stop, e)),
            };
            usage.tokens_in += final_reply.tokens_in;
            usage.tokens_out += final_reply.tokens_out;
            history.push(final_reply.message);
            if !final_reply.text.trim().is_empty() {
                return Ok(final_reply.text);
            }
            return Ok(stop);
        }
        if let Some(done) = after_round() {
            return Ok(done);
        }
    }
    Err(format!("stopped after {} rounds of tool calls", rounds))
}

#[cfg(test)]
mod text_call_tests {
    use super::*;

    #[test]
    fn glm_arg_key_blocks_become_calls() {
        // GLM-5.3 in a calibration run with the text tool format (hard-fix-median), plus a typed value.
        let answer = "<tool_call>read<arg_key>path</arg_key><arg_value>app/mathutil.py</arg_value></tool_call>\
<tool_call>run<arg_key>command</arg_key><arg_value>python3 tests/test_median.py</arg_value><arg_key>timeout</arg_key><arg_value>30</arg_value></tool_call>";
        let (calls, rest) = text_tool_calls(answer);
        assert_eq!(calls.len(), 2);
        assert_eq!((calls[0].name.as_str(), calls[0].args["path"].as_str()), ("read", Some("app/mathutil.py")));
        assert_eq!((calls[1].args["command"].as_str(), &calls[1].args["timeout"]), (Some("python3 tests/test_median.py"), &json!("30")));
        assert!(rest.is_empty());
        // A string that looks like JSON stays the string: write gets the file content as written.
        let (write, _) = text_tool_calls("<tool_call>write<arg_key>path</arg_key><arg_value>x.json</arg_value><arg_key>text</arg_key><arg_value>{\"a\": 1}</arg_value></tool_call>");
        assert_eq!(write[0].args["text"], json!("{\"a\": 1}"));
        let (edit, _) = text_tool_calls("<tool_call>edit<arg_key>old</arg_key><arg_value>30</arg_value><arg_key>new</arg_key><arg_value>true</arg_value></tool_call>");
        assert_eq!((&edit[0].args["old"], &edit[0].args["new"]), (&json!("30"), &json!("true")));
        // Without templates (native mode) the GLM body stays text.
        let (none, _) = text_tool_calls_with("<tool_call>read<arg_key>path</arg_key><arg_value>a</arg_value></tool_call>", false);
        assert!(none.is_empty());
        // GLM's mixed forms from the calibration runs: a JSON key inside arg_key, attributes, name plus JSON.
        let mixed = [
            ("read<arg_key>path\": \"config/settings.json\"</arg_value>", "read", json!({"path": "config/settings.json"})),
            ("run<arg_key>command=\"ls logs; wc -l logs/*.log\"</arg_value>cwd=\"/w\"", "run", json!({"command": "ls logs; wc -l logs/*.log", "cwd": "/w"})),
            ("read cwd=\"/w\" path=\"data/sales.csv\"", "read", json!({"cwd": "/w", "path": "data/sales.csv"})),
            ("run {\"command\": \"cat a.py\"}</arg_value><arg_key>cwd\": \"/w\"}</arg_value>", "run", json!({"command": "cat a.py"})),
            ("read({\"path\": \"data/sales.csv\"})", "read", json!({"path": "data/sales.csv"})),
            ("tree<arg_key>depth\": 4}</arg_value>", "tree", json!({"depth": "4"})),
            ("edit<arg_key>edits\":[{\"new\":\"b\",\"old\":\"a\"}]</arg_value><arg_key>path</arg_key><arg_value>c.json</arg_value>", "edit",
             json!({"edits": "[{\"new\":\"b\",\"old\":\"a\"}]", "path": "c.json"})),
            ("tree{\"depth\": 3}{}</arg_value>", "tree", json!({"depth": 3})),
        ];
        for (body, name, args) in mixed {
            let (calls, _) = text_tool_calls(&format!("<tool_call>{}</tool_call>", body));
            assert_eq!(calls.len(), 1, "{}", body);
            assert_eq!((calls[0].name.as_str(), &calls[0].args), (name, &args), "{}", body);
        }
        // Prose in a block is no call.
        let (none, _) = text_tool_calls("<tool_call>I would now read the file to see what it holds.</tool_call>");
        assert!(none.is_empty());
    }

    #[test]
    fn deepseek_dsml_blocks_become_calls() {
        // The answer DeepSeek V4.1 Flash wrote in a calibration run with the text tool format (hard-fix-median).
        let answer = "I'll start by looking at the project.\n\n<\u{ff5c}DSML\u{ff5c} calls>\n<\u{ff5c}DSML\u{ff5c} invoke name=\"run\">\n\
<\u{ff5c}DSML\u{ff5c} parameter name=\"command\" string=\"true\">cat app/mathutil.py && echo \"---\"</\u{ff5c}DSML\u{ff5c} parameter>\n\
<\u{ff5c}DSML\u{ff5c} parameter name=\"timeout\" string=\"false\">30</\u{ff5c}DSML\u{ff5c} parameter>\n</\u{ff5c}DSML\u{ff5c} invoke>\n\
<\u{ff5c}DSML\u{ff5c}invoke name=\"read\">\n<\u{ff5c}DSML\u{ff5c}parameter name=\"path\" string=\"true\">tests/test_median.py</\u{ff5c}DSML\u{ff5c}parameter>\n\
</\u{ff5c}DSML\u{ff5c}invoke>\n</\u{ff5c}DSML\u{ff5c} calls>";
        let (calls, rest) = text_tool_calls(answer);
        assert_eq!(calls.len(), 2);
        assert_eq!((calls[0].name.as_str(), calls[0].args["command"].as_str()), ("run", Some("cat app/mathutil.py && echo \"---\"")));
        assert_eq!(calls[0].args["timeout"], json!(30));
        assert_eq!((calls[1].name.as_str(), calls[1].args["path"].as_str()), ("read", Some("tests/test_median.py")));
        assert_eq!(rest, "I'll start by looking at the project.");
        // Text with the marker and no invoke block stays as it is.
        let (none, same) = text_tool_calls("a \u{ff5c}DSML\u{ff5c} marker");
        assert!(none.is_empty() && same == "a \u{ff5c}DSML\u{ff5c} marker");
        // Short and missing closing tags, as DeepSeek wrote them in the second calibration round (hard-log-warn, hard-pipe-p2).
        let b = "\u{ff5c}";
        let sloppy = format!("Next.\n<{b}DSML{b} calls>\n<{b}DSML{b} invoke name=\"read\">\n<{b}DSML{b} parameter name=\"limit\" string=\"false\">200</{b}DSML{b}>\n\
<{b}DSML{b} parameter name=\"path\" string=\"true\">logs/app-3.log</{b}DSML{b}>\n<{b}DSML{b} invoke name=\"read\">\n\
<{b}DSML{b} parameter name=\"path\" string=\"true\">data/numbers.txt</{b}DSML{b}>\n</{b}DSML{b}>\n</{b}DSML{b} calls>", b = b);
        let (calls, rest) = text_tool_calls(&sloppy);
        assert_eq!(calls.len(), 2);
        assert_eq!((&calls[0].args["limit"], calls[0].args["path"].as_str()), (&json!(200), Some("logs/app-3.log")));
        assert_eq!(calls[1].args["path"], "data/numbers.txt");
        assert_eq!(rest, "Next.");
    }

    #[test]
    fn two_blocks_after_a_thought_become_two_calls() {
        let answer = "<think>\n- host: penelope, memory yes\n- tool: read, grep\n- args: path; pattern, path\n</think>\n\n<tool_call>\n{\"name\": \"read\", \"arguments\": {\"path\": \"src/main.rs\"}}\n</tool_call>\n<tool_call>\n{\"name\": \"grep\", \"arguments\": {\"pattern\": \"TODO\", \"path\": \"src\"}}\n</tool_call>";
        let (calls, rest) = text_tool_calls(answer);
        assert_eq!(calls.len(), 2);
        assert_eq!((calls[0].name.as_str(), calls[0].args["path"].as_str()), ("read", Some("src/main.rs")));
        assert_eq!((calls[1].name.as_str(), calls[1].args["pattern"].as_str()), ("grep", Some("TODO")));
        assert_ne!(calls[0].id, calls[1].id);
        assert_eq!(rest, "");
    }

    #[test]
    fn no_block_keeps_the_reason_after_none() {
        let answer = "<think>\n- host: claude-code, memory no\n- tool: none, memory needs penelope\n</think>\n\nnone: memory needs penelope";
        let (calls, rest) = text_tool_calls(answer);
        assert!(calls.is_empty());
        assert_eq!(rest, "memory needs penelope");
    }

    #[test]
    fn a_broken_block_stays_text_and_a_good_one_still_counts() {
        let answer = "<tool_call>\n{not json}\n</tool_call>\n<tool_call>\n{\"name\": \"read\", \"arguments\": \"{\\\"path\\\": \\\"a\\\"}\"}\n</tool_call>";
        let (calls, rest) = text_tool_calls(answer);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].args["path"].as_str(), Some("a"));
        assert!(rest.contains("{not json}"), "{}", rest);
    }

    #[test]
    fn an_open_block_without_end_is_left_as_text() {
        let (calls, rest) = text_tool_calls("<tool_call>\n{\"name\": \"read\"");
        assert!(calls.is_empty());
        assert!(rest.starts_with("<tool_call>"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn effort_is_sent_only_to_a_model_that_has_the_field() {
        let claude = json!({"kind": "claude-cli", "model": "sonnet"});
        assert_eq!(effort(&claude), Some("high"));                                  // an old setting gets the default
        assert_eq!(effort(&json!({"kind": "anthropic", "model": "claude-opus-5-5", "effort": " "})), Some("high"));
        assert_eq!(effort(&json!({"kind": "anthropic", "model": "claude-opus-5-5", "effort": "low"})), Some("low"));
        assert_eq!(effort(&json!({"kind": "openai", "base_url": "https://api.openai.com/v1", "model": "gpt-6.2-astra"})), Some("high"));
        assert_eq!(effort(&json!({"kind": "openai", "base_url": "https://api.openai.com/v1", "model": "gpt-4o", "effort": "high"})), None);
        assert_eq!(effort(&json!({"kind": "anthropic", "model": "claude-3-5-sonnet", "effort": "high"})), None);
        assert_eq!(effort(&json!({"kind": "openai", "base_url": "https://opencode.ai/zen/go/v1", "model": "mimo-v2.6-flash", "effort": "high"})), None);
    }

    #[test]
    fn a_test_loop_stop_ends_the_loop_with_one_answer_without_tools() {
        std::env::set_var("PENELOPE_TEST_KEY_STOP", "test-key-stop-0123456789");
        let (address, seen) = fake::serve(vec![
            (200, json!({"choices": [{"finish_reason": "tool_calls", "message": {"role": "assistant", "content": null, "tool_calls": [
                {"id": "call_1", "type": "function", "function": {"name": "edit", "arguments": "{}"}}]}}],
                "usage": {"prompt_tokens": 10, "completion_tokens": 2}}).to_string()),
            (200, json!({"choices": [{"finish_reason": "stop", "message": {"role": "assistant", "content": "Tests failed three times; the parser still fails."}}],
                "usage": {"prompt_tokens": 20, "completion_tokens": 5}}).to_string()),
        ]);
        let model = json!({"kind": "openai", "base_url": format!("{}/v1", address), "model": "gpt-6.2-astra", "api_key_env": "PENELOPE_TEST_KEY_STOP"});
        let mut history = vec![json!({"role": "user", "content": "fix it"})];
        let tools = vec![json!({"name": "edit", "description": "", "inputSchema": {"type": "object"}})];
        let mut usage = Usage::default();
        let mut calls = 0;
        let answer = run_loop(&model, "", &mut history, &tools, 4, None, &mut usage, &mut |_| {
            calls += 1;
            (format!("{}: STOP. `cargo test` failed 3 times.", crate::tools::TEST_STOP), false)
        }, &mut |_| {}).unwrap();
        assert_eq!(answer, "Tests failed three times; the parser still fails.");
        assert_eq!(calls, 1);
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 2);
        assert!(seen[1].body.get("tools").is_none(), "{}", seen[1].body);
        let messages = seen[1].body["messages"].as_array().unwrap();
        let last = messages.iter().rev().find(|m| m["role"] == "tool").unwrap();
        let result = last["content"].as_str().unwrap();
        assert!(result.contains(crate::tools::TEST_STOP) && result.contains("Write the final answer"), "{}", result);
    }

    #[test]
    fn effort_reaches_the_anthropic_and_the_openai_body() {
        std::env::set_var("PENELOPE_TEST_KEY_EFFORT", "test-key-effort-0123456789");
        let (address, seen) = fake::serve(vec![
            (200, json!({"content": [{"type": "text", "text": "a"}], "stop_reason": "end_turn"}).to_string()),
            (200, json!({"choices": [{"finish_reason": "stop", "message": {"role": "assistant", "content": "b"}}]}).to_string()),
            (200, json!({"choices": [{"finish_reason": "stop", "message": {"role": "assistant", "content": "c"}}]}).to_string()),
        ]);
        let history = vec![json!({"role": "user", "content": "hello"})];
        let anthropic = json!({"kind": "anthropic", "base_url": address, "model": "claude-opus-5-5",
                               "api_key_env": "PENELOPE_TEST_KEY_EFFORT", "effort": "medium"});
        chat(&anthropic, "", &history, &[]).unwrap();
        let openai = json!({"kind": "openai", "base_url": format!("{}/v1", address), "model": "gpt-6.2-astra",
                            "api_key_env": "PENELOPE_TEST_KEY_EFFORT"});
        chat(&openai, "", &history, &[]).unwrap();
        let openai_high = json!({"kind": "openai", "base_url": format!("{}/v1", address), "model": "gpt-6.2-astra",
                                 "api_key_env": "PENELOPE_TEST_KEY_EFFORT", "effort": "max"});
        chat(&openai_high, "", &history, &[]).unwrap();
        let seen = seen.lock().unwrap();
        assert_eq!(seen[0].body["thinking"], json!({"type": "adaptive"}));
        assert_eq!(seen[0].body["output_config"], json!({"effort": "medium"}));
        assert_eq!(seen[1].body["reasoning_effort"], json!("high"));
        assert_eq!(seen[2].body["reasoning_effort"], json!("max"));
        assert!(seen.iter().all(|s| !s.body.to_string().contains("test-key-effort")));
    }
}

/// A small HTTP server for tests. It plays an endpoint: every request gets the next prepared body, and the server
/// keeps what it received. No test talks to a real provider.
#[cfg(any(test, feature = "test-fake"))]
pub mod fake {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    pub struct Seen {
        pub path: String,
        /// Header lines in lower case.
        pub headers: Vec<String>,
        pub body: serde_json::Value,
    }

    /// Starts the server on a free port. Returns its address and the list of received requests.
    pub fn serve(replies: Vec<(u16, String)>) -> (String, Arc<Mutex<Vec<Seen>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        std::thread::spawn(move || {
            for (status, reply) in replies {
                let Ok((stream, _)) = listener.accept() else { return };
                let mut reader = BufReader::new(stream);
                let (mut first, mut headers, mut length) = (String::new(), Vec::new(), 0usize);
                let _ = reader.read_line(&mut first);
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
                        break;
                    }
                    let line = line.trim().to_lowercase();
                    if let Some(n) = line.strip_prefix("content-length:") {
                        length = n.trim().parse().unwrap_or(0);
                    }
                    headers.push(line);
                }
                let mut body = vec![0u8; length];
                let _ = reader.read_exact(&mut body);
                log.lock().unwrap().push(Seen { path: first.split_whitespace().nth(1).unwrap_or("").to_string(), headers,
                                                body: serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null) });
                let mut stream = reader.into_inner();
                let _ = write!(stream, "HTTP/1.1 {} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", status, reply.len(), reply);
                let _ = stream.flush();
            }
        });
        (address, seen)
    }

    /// Like `serve`, but every answer waits its own delay in milliseconds before it is written, and each connection
    /// has its own thread. Tests use it for a model call that sleeps and for parts that must overlap.
    pub fn serve_parallel(replies: Vec<(u16, String, u64)>) -> (String, Arc<Mutex<Vec<Seen>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        let queue = Arc::new(Mutex::new(replies));
        std::thread::spawn(move || {
            let mut handlers = Vec::new();
            loop {
                let Ok((stream, _)) = listener.accept() else { return };
                let job = queue.clone();
                let log = log.clone();
                handlers.push(std::thread::spawn(move || {
                    let (status, reply, delay) = {
                        let mut queue = job.lock().unwrap();
                        if queue.is_empty() { return; }
                        queue.remove(0)
                    };
                    let mut reader = BufReader::new(stream);
                    let (mut first, mut headers, mut length) = (String::new(), Vec::new(), 0usize);
                    let _ = reader.read_line(&mut first);
                    loop {
                        let mut line = String::new();
                        if reader.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
                            break;
                        }
                        let line = line.trim().to_lowercase();
                        if let Some(n) = line.strip_prefix("content-length:") {
                            length = n.trim().parse().unwrap_or(0);
                        }
                        headers.push(line);
                    }
                    let mut body = vec![0u8; length];
                    let _ = reader.read_exact(&mut body);
                    log.lock().unwrap().push(Seen { path: first.split_whitespace().nth(1).unwrap_or("").to_string(), headers,
                                                    body: serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null) });
                    if delay > 0 {
                        std::thread::sleep(std::time::Duration::from_millis(delay));
                    }
                    let mut stream = reader.into_inner();
                    let _ = write!(stream, "HTTP/1.1 {} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", status, reply.len(), reply);
                    let _ = stream.flush();
                }));
                if queue.lock().unwrap().is_empty() {
                    break;
                }
            }
            for handler in handlers {
                let _ = handler.join();
            }
        });
        (address, seen)
    }
}
