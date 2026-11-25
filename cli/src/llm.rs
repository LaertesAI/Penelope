//! The two wire formats for models behind an HTTP address: `openai` (chat completions) and `anthropic` (the Messages
//! API). Both run the same tool loop: send the history, run the tool calls of the answer, send the results, until an
//! answer comes without a tool call. The history keeps the format of its kind, so it can go back unchanged.

use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};

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

/// Sends one request and returns the body as JSON. Status 429 and 5xx are tried again twice.
fn post(url: &str, headers: &[(&str, String)], body: &Value, key: &str, seconds: u64) -> Result<Value, String> {
    let mut wait = 2;
    for attempt in 0..3 {
        let mut req = crate::net::agent(seconds).post(url);
        for (name, value) in headers {
            req = req.set(name, value);
        }
        match req.send_json(body.clone()) {
            Ok(resp) => {
                if (300..400).contains(&resp.status()) {
                    let to = resp.header("location").unwrap_or("an address the answer did not name");
                    return Err(format!("{} answered with a redirect to {}. Redirects are not followed, so the key cannot travel to another host.", url, to));
                }
                let text = resp.into_string().map_err(|e| format!("the answer of {} could not be read: {}", url, e))?;
                return serde_json::from_str(&scrub(&text, key)).map_err(|_| format!("{} answered with text that is no JSON", url));
            }
            Err(ureq::Error::Status(code, resp)) => {
                let after = resp.header("retry-after").and_then(|s| s.parse::<u64>().ok());
                let text = scrub(&resp.into_string().unwrap_or_default(), key);
                if (code == 429 || code >= 500) && attempt < 2 {
                    std::thread::sleep(std::time::Duration::from_secs(after.unwrap_or(wait).min(20)));
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
            Err(e) => return Err(scrub(&format!("no answer from {}: {}", url, e), key)),
        }
    }
    Err(format!("{} gave no answer in 3 tries", url))
}

/// One request to the model of a setting. `history` holds messages in the format of the setting's kind. `tools` are
/// tool descriptions in the MCP form with `name`, `description` and `inputSchema`.
pub fn chat(model: &Value, system: &str, history: &[Value], tools: &[Value]) -> Result<Reply, String> {
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
        let v = post(&format!("{}/v1/messages", base), &headers, &body, &key, seconds)?;
        let stop = v["stop_reason"].as_str().unwrap_or("").to_string();
        let blocks = v["content"].as_array().cloned().unwrap_or_default();
        if blocks.is_empty() && stop.is_empty() {
            return Err(format!("the answer holds no content: {}", v.to_string().chars().take(200).collect::<String>()));
        }
        let text = blocks.iter().filter(|b| b["type"] == "text").filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join("\n").trim().to_string();
        let calls = blocks.iter().filter(|b| b["type"] == "tool_use").map(|b| ToolCall {
            id: b["id"].as_str().unwrap_or("").to_string(), name: b["name"].as_str().unwrap_or("").to_string(), args: b["input"].clone() }).collect();
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
    let mut headers = Vec::new();
    if let Some(extra) = model["headers"].as_object() {
        for (name, value) in extra {
            if let Some(v) = value.as_str() { headers.push((name.as_str(), v.to_string())); }
        }
    }
    if !key.is_empty() {
        headers.push(("Authorization", format!("Bearer {}", key)));
    }
    let v = post(&format!("{}/chat/completions", base), &headers, &body, &key, seconds)?;
    let choice = &v["choices"][0];
    if !choice["message"].is_object() {
        return Err(format!("the answer holds no message: {}", v.to_string().chars().take(200).collect::<String>()));
    }
    let message = choice["message"].clone();
    let calls = message["tool_calls"].as_array().cloned().unwrap_or_default().iter().map(|c| ToolCall {
        id: c["id"].as_str().unwrap_or("").to_string(), name: c["function"]["name"].as_str().unwrap_or("").to_string(),
        // The arguments arrive as a JSON text. A text that does not parse becomes Null and the loop reports it to the model.
        args: match &c["function"]["arguments"] {
            Value::String(s) if s.trim().is_empty() => json!({}),
            Value::String(s) => serde_json::from_str(s).unwrap_or(Value::Null),
            Value::Object(o) => Value::Object(o.clone()),
            _ => json!({}),
        } }).collect();
    Ok(Reply { text: message["content"].as_str().unwrap_or("").trim().to_string(), calls, message,
               tokens_in: v["usage"]["prompt_tokens"].as_u64().unwrap_or(0), tokens_out: v["usage"]["completion_tokens"].as_u64().unwrap_or(0),
               stop: choice["finish_reason"].as_str().unwrap_or("").to_string() })
}

/// The tool loop. `history` grows by every message of the turn. `exec` runs one tool call and returns its text and
/// whether it worked. `on_text` gets the text a model writes next to tool calls. Returns the final answer.
#[allow(clippy::too_many_arguments)]
pub fn run_loop(model: &Value, system: &str, history: &mut Vec<Value>, tools: &[Value], rounds: usize, cancel: Option<&AtomicBool>, usage: &mut Usage,
                exec: &mut dyn FnMut(&ToolCall) -> (String, bool), on_text: &mut dyn FnMut(&str)) -> Result<String, String> {
    let kind = kind_of(model).to_string();
    for _ in 0..rounds {
        if cancel.map(|c| c.load(Ordering::SeqCst)).unwrap_or(false) {
            return Err("stopped".to_string());
        }
        let reply = chat(model, system, history, tools)?;
        usage.tokens_in += reply.tokens_in;
        usage.tokens_out += reply.tokens_out;
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
            return Ok(reply.text);
        }
        if !reply.text.is_empty() {
            on_text(&reply.text);
        }
        let mut results = Vec::new();
        for call in &reply.calls {
            let (text, ok) = if call.args.is_null() {
                ("the arguments of this call were no valid JSON. Send the call again with a JSON object.".to_string(), false)
            } else {
                exec(call)
            };
            usage.steps += 1;
            results.push((call.id.clone(), text, ok));
        }
        history.extend(result_messages(&kind, &results));
    }
    Err(format!("stopped after {} rounds of tool calls", rounds))
}

/// A small HTTP server for tests. It plays an endpoint: every request gets the next prepared body, and the server
/// keeps what it received. No test talks to a real provider.
#[cfg(test)]
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
}
