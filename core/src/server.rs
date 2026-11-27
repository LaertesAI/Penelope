//! The small HTTP API of Penelope: search, read, embeddings, rerank and redaction for other programs on this PC.
//!
//! It is deliberately one binary and one thread per request. There is no framework and no listening port unless the
//! person starts `penelope serve`. The default bind is loopback; a remote bind needs `--allow-remote` and a bearer
//! token in `PENELOPE_SERVER_TOKEN`. The server never calls itself: `search.api_url`, `search.rerank` and `reader.url`
//! are cleared in its own copy of the settings.

use crate::config::Ctx;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

const MAX_BODY: usize = 2 * 1024 * 1024;
const MAX_HEADER: usize = 64 * 1024;
const REQUESTS_PER_MINUTE: u32 = 120;
const EMBED_DIM: usize = 256;

struct Window {
    start: Instant,
    count: u32,
}

pub struct State {
    ctx: Arc<Ctx>,
    token: Option<String>,
    started: Instant,
    limits: Mutex<HashMap<String, Window>>,
}

/// A request after the HTTP layer has read it. Tests build one without a socket.
struct Request {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
    peer: String,
}

impl State {
    fn new(ctx: &Ctx, token: Option<String>) -> State {
        let mut own = ctx.clone();
        // These three would make the server call itself. A search API does not need a search API.
        own.cfg["search"]["api_url"] = json!("");
        own.cfg["search"]["rerank"]["enabled"] = json!(false);
        own.cfg["reader"]["url"] = json!("");
        own.cfg["search"]["jina"] = json!(false);
        // A privacy model would be a model call inside a search. The server uses the fixed rules.
        own.cfg["privacy"] = json!("rules");
        own.cfg["roles"] = json!({});
        if own.cfg["search"]["kind"].as_str() == Some("api") {
            own.cfg["search"]["kind"] = json!("auto");
        }
        State { ctx: Arc::new(own), token, started: Instant::now(), limits: Mutex::new(HashMap::new()) }
    }

    fn allow(&self, peer: &str) -> bool {
        let mut map = self.limits.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        if map.len() > 512 {
            map.retain(|_, w| now.duration_since(w.start) < Duration::from_secs(60));
        }
        let window = map.entry(peer.to_string()).or_insert_with(|| Window { start: now, count: 0 });
        if now.duration_since(window.start) >= Duration::from_secs(60) {
            *window = Window { start: now, count: 0 };
        }
        if window.count >= REQUESTS_PER_MINUTE {
            return false;
        }
        window.count += 1;
        true
    }
}

/// Starts the server and blocks until the process is stopped.
pub fn run(ctx: &Ctx, args: &[String]) -> Result<(), String> {
    let mut bind = "127.0.0.1:8765".to_string();
    let mut allow_remote = false;
    let mut quiet = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--bind" | "-b" => {
                i += 1;
                bind = args.get(i).cloned().ok_or("--bind needs an address such as 127.0.0.1:8765")?;
                if !valid_bind(&bind) {
                    return Err(format!("--bind {} is not HOST:PORT (for example 127.0.0.1:8765 or [::1]:8765, port 1 to 65535)", bind));
                }
            }
            "--allow-remote" => allow_remote = true,
            "--quiet" => quiet = true,
            "--print-config" => {
                println!("{}", serde_json::to_string_pretty(&config_hint(&bind)).unwrap());
                return Ok(());
            }
            "--help" | "-h" => {
                println!("penelope serve [--bind 127.0.0.1:8765] [--allow-remote] [--quiet] [--print-config]\n\
                          POST /search /read /embed /rerank /redact · GET /health /models\n\
                          auth: PENELOPE_SERVER_TOKEN as a bearer token; a remote bind is refused without it.");
                return Ok(());
            }
            other => return Err(format!("unknown option {} for penelope serve", other)),
        }
        i += 1;
    }
    let token = std::env::var("PENELOPE_SERVER_TOKEN").ok().filter(|t| !t.trim().is_empty());
    if !loopback(&bind) {
        if !allow_remote {
            return Err("the server binds to loopback by default. Use --allow-remote and PENELOPE_SERVER_TOKEN for another address.".to_string());
        }
        if token.is_none() {
            return Err("a remote bind needs PENELOPE_SERVER_TOKEN; start it with the token in the environment.".to_string());
        }
    }
    let state = Arc::new(State::new(ctx, token));
    let listener = TcpListener::bind(&bind).map_err(|e| format!("cannot listen on {}: {}", bind, e))?;
    let actual = listener.local_addr().map(|a| a.to_string()).unwrap_or(bind.clone());
    println!("penelope server on http://{}", actual);
    println!("endpoints: GET /health /models · POST /search /read /embed /rerank /redact");
    println!("auth: {} · limit: {} requests per minute per client", if state.token.is_some() { "bearer token" } else { "none (loopback)" }, REQUESTS_PER_MINUTE);
    if !quiet {
        println!("client settings: penelope serve --print-config");
    }
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let state = state.clone();
                std::thread::spawn(move || handle_connection(stream, state));
            }
            Err(e) => eprintln!("penelope server: accept failed: {}", e),
        }
    }
    Ok(())
}

/// HOST:PORT with a non-empty host without spaces or slashes and a port from 1 to 65535.
pub fn valid_bind(bind: &str) -> bool {
    let Some((host, port)) = bind.rsplit_once(':') else { return false };
    let host = host.trim_matches(['[', ']']);
    !host.is_empty() && !host.contains(|c: char| c.is_whitespace() || c == '/') && port.parse::<u16>().map(|p| p > 0).unwrap_or(false)
}

fn loopback(bind: &str) -> bool {
    let host = bind.rsplit_once(':').map(|(h, _)| h).unwrap_or(bind).trim_matches(['[', ']']);
    host == "localhost" || host == "127.0.0.1" || host == "::1"
}

fn config_hint(bind: &str) -> Value {
    let render = if bind.starts_with("0.0.0.0:") || bind.starts_with("[::]:") { bind.replacen("0.0.0.0", "127.0.0.1", 1).replacen("[::]", "127.0.0.1", 1) } else { bind.to_string() };
    let base = format!("http://{}", render);
    json!({
        "search": {
            "kind": "api",
            "api_url": format!("{}/search", base),
            "server_api_key_env": "PENELOPE_SERVER_TOKEN",
            "rerank": {"enabled": true, "url": format!("{}/rerank", base), "api_key_env": "PENELOPE_SERVER_TOKEN"}
        },
        "reader": {"enabled": true, "url": format!("{}/read", base), "api_key_env": "PENELOPE_SERVER_TOKEN"}
    })
}

fn handle_connection(mut stream: TcpStream, state: Arc<State>) {
    let peer = stream.peer_addr().map(|a| a.ip().to_string()).unwrap_or_else(|_| "unknown".to_string());
    let request = match read_request(&mut stream, peer) {
        Ok(r) => r,
        Err((status, message)) => { write_json(&mut stream, status, &json!({"error": message})); return; }
    };
    let (status, body) = route(&state, &request);
    if status == 204 {
        let _ = stream.write_all(b"HTTP/1.1 204 No Content\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Headers: authorization, content-type\r\nAccess-Control-Allow-Methods: GET, POST, OPTIONS\r\nConnection: close\r\n\r\n");
    } else {
        write_json(&mut stream, status, &body);
    }
}

fn read_request(stream: &mut TcpStream, peer: String) -> Result<Request, (u16, String)> {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(20)));
    let mut data = Vec::new();
    let mut buf = [0u8; 8192];
    let header_end = loop {
        if let Some(at) = find(&data, b"\r\n\r\n") {
            break at;
        }
        if data.len() > MAX_HEADER {
            return Err((431, "request header too large".to_string()));
        }
        let n = stream.read(&mut buf).map_err(|e| (400, format!("request could not be read: {}", e)))?;
        if n == 0 {
            return Err((400, "empty request".to_string()));
        }
        data.extend_from_slice(&buf[..n]);
    };
    let head = String::from_utf8_lossy(&data[..header_end]);
    let mut lines = head.split("\r\n");
    let request_line = lines.next().ok_or((400, "no request line".to_string()))?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_uppercase();
    let path = parts.next().unwrap_or("").split('?').next().unwrap_or("").to_string();
    if method.is_empty() || path.is_empty() {
        return Err((400, "bad request line".to_string()));
    }
    let mut headers = HashMap::new();
    for line in lines {
        if let Some((key, value)) = line.split_once(':') {
            headers.insert(key.trim().to_lowercase(), value.trim().to_string());
        }
    }
    if headers.get("transfer-encoding").map(|v| !v.is_empty()).unwrap_or(false) {
        return Err((411, "send a Content-Length, not a chunked body".to_string()));
    }
    let length: usize = headers.get("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
    if length > MAX_BODY {
        return Err((413, format!("body over {} MB", MAX_BODY / 1_048_576)));
    }
    if headers.get("expect").map(|v| v.eq_ignore_ascii_case("100-continue")).unwrap_or(false) {
        let _ = stream.write_all(b"HTTP/1.1 100 Continue\r\n\r\n");
        let _ = stream.flush();
    }
    let start = header_end + 4;
    while data.len() < start + length {
        let n = stream.read(&mut buf).map_err(|e| (400, format!("request body could not be read: {}", e)))?;
        if n == 0 {
            break;
        }
        data.extend_from_slice(&buf[..n]);
    }
    if data.len() < start + length {
        return Err((400, "request body is shorter than Content-Length".to_string()));
    }
    Ok(Request { method, path, headers, body: data[start..start + length].to_vec(), peer })
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn write_json(stream: &mut TcpStream, status: u16, body: &Value) {
    let text = serde_json::to_string(body).unwrap_or_else(|_| "{\"error\":\"cannot encode the answer\"}".to_string());
    let _ = write!(stream, "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nCache-Control: no-store\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Headers: authorization, content-type\r\nAccess-Control-Allow-Methods: GET, POST, OPTIONS\r\nConnection: close\r\n\r\n{}",
                 status, reason(status), text.len(), text);
    let _ = stream.flush();
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK", 204 => "No Content", 400 => "Bad Request", 401 => "Unauthorized", 404 => "Not Found",
        405 => "Method Not Allowed", 411 => "Length Required", 413 => "Payload Too Large", 429 => "Too Many Requests",
        431 => "Request Header Fields Too Large", 500 => "Internal Server Error", 502 => "Bad Gateway",
        _ => "Error",
    }
}

fn authorized(state: &State, headers: &HashMap<String, String>) -> bool {
    match &state.token {
        None => true,
        Some(token) => headers.get("authorization").map(|h| h == &format!("Bearer {}", token)).unwrap_or(false),
    }
}

fn route(state: &State, req: &Request) -> (u16, Value) {
    if req.method == "OPTIONS" {
        return (204, Value::Null);
    }
    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/") => (200, json!({
            "service": "penelope-server", "version": env!("CARGO_PKG_VERSION"),
            "endpoints": ["GET /health", "GET /models", "GET /whoami", "POST /search", "POST /read", "POST /embed", "POST /rerank", "POST /redact"]
        })),
        ("GET", "/health") => (200, health(state)),
        ("GET", "/whoami") => {
            if !authorized(state, &req.headers) {
                return (401, json!({"error": "a bearer token is required"}));
            }
            (200, json!({"service": "penelope-server", "version": env!("CARGO_PKG_VERSION"),
                         "auth": if state.token.is_some() { "bearer" } else { "none" }, "token_valid": true}))
        }
        ("GET", "/models") => (200, json!({"models": [
            {"name": "penelope-hash-256", "kind": "embedding", "dimensions": EMBED_DIM, "note": "deterministic token and bigram features, no model download"},
            {"name": "penelope-bm25", "kind": "rerank", "note": "lexical BM25 over the supplied documents"}
        ]})),
        ("POST", path) => {
            if !authorized(state, &req.headers) {
                return (401, json!({"error": "a bearer token is required"}));
            }
            if !state.allow(&req.peer) {
                return (429, json!({"error": format!("more than {} requests per minute", REQUESTS_PER_MINUTE)}));
            }
            match path {
                "/search" => post_json(&req.body, |body| search(state, body)),
                "/read" => post_json(&req.body, |body| read(state, body)),
                "/embed" => post_json(&req.body, embed),
                "/rerank" => post_json(&req.body, rerank),
                "/redact" => post_json(&req.body, redact),
                _ => (404, json!({"error": "no such endpoint"})),
            }
        }
        ("GET", "/search") | ("GET", "/read") | ("GET", "/embed") | ("GET", "/rerank") | ("GET", "/redact") => {
            (405, json!({"error": "use POST with a JSON body"}))
        }
        _ => (404, json!({"error": "no such endpoint"})),
    }
}

fn post_json(body: &[u8], f: impl FnOnce(&Value) -> (u16, Value)) -> (u16, Value) {
    let value: Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return (400, json!({"error": format!("the body is no JSON: {}", e)})),
    };
    if !value.is_object() {
        return (400, json!({"error": "the body must be one JSON object"}));
    }
    f(&value)
}

fn health(state: &State) -> Value {
    json!({
        "ok": true,
        "service": "penelope-server",
        "version": env!("CARGO_PKG_VERSION"),
        "uptime_s": state.started.elapsed().as_secs(),
        "endpoints": ["/search", "/read", "/embed", "/rerank", "/redact"],
        "auth": if state.token.is_some() { "bearer" } else { "none" },
        "rate_limit": {"requests_per_minute": REQUESTS_PER_MINUTE},
        "search": {"sources": crate::search::sources(&state.ctx), "cache": state.ctx.cfg["search"]["cache"].as_bool().unwrap_or(true)},
        "reader": {"local_pdf": crate::config::which("pdftotext").is_some(), "server": false},
        "embedding": {"model": "penelope-hash-256", "dimensions": EMBED_DIM}
    })
}

fn search(state: &State, body: &Value) -> (u16, Value) {
    let query = body["query"].as_str().unwrap_or("").trim();
    if query.is_empty() {
        return (400, json!({"error": "query is required"}));
    }
    if query.chars().count() > 1_000 {
        return (400, json!({"error": "query is over 1,000 characters"}));
    }
    let limit = body["limit"].as_u64().unwrap_or(8).clamp(1, 20) as usize;
    let page = body["page"].as_u64().unwrap_or(1).clamp(1, 5) as usize;
    let recency = body["recency"].as_str().unwrap_or("").to_string();
    let mut ctx = (*state.ctx).clone();
    if let Some(language) = body["language"].as_str().filter(|l| !l.is_empty()) {
        ctx.cfg["search"]["language"] = json!(language);
    }
    if let Some(safe) = body["safe"].as_bool() {
        ctx.cfg["search"]["safe"] = json!(safe);
    }
    let opts = crate::search::Options {
        limit: (limit * page).min(40),
        engine: None,
        fresh: body["fresh"].as_bool().unwrap_or(false),
        recency,
    };
    match crate::search::search_hits(&ctx, query, &opts) {
        Ok((hits, engines)) => {
            let total = hits.len();
            let results: Vec<Value> = hits.into_iter().skip((page - 1) * limit).take(limit).map(|h| json!({
                "title": h.title, "url": h.url, "snippet": h.snippet, "engine": h.engine,
                "published": h.published, "score": h.score
            })).collect();
            (200, json!({"query": query, "page": page, "limit": limit, "number_of_results": total,
                         "engines": engines, "results": results}))
        }
        Err(e) => (502, json!({"error": e, "query": query})),
    }
}

fn read(state: &State, body: &Value) -> (u16, Value) {
    let url = body["url"].as_str().unwrap_or("").trim();
    if url.is_empty() {
        return (400, json!({"error": "url is required"}));
    }
    if !crate::scrape::target_is_public(url) {
        return (400, json!({"error": "the reader refuses a private, loopback or unresolved address"}));
    }
    let max_chars = body["max_chars"].as_u64().unwrap_or(60_000).clamp(1_000, 200_000) as usize;
    match crate::scrape::fetch_public(&state.ctx, url) {
        Ok(page) => {
            let markdown: String = page.markdown.chars().take(max_chars).collect();
            let links: Vec<String> = page.links.into_iter().take(200).collect();
            (200, json!({
                "url": page.url, "final_url": page.final_url, "title": page.title, "description": page.description,
                "author": page.author, "published": page.published, "markdown": markdown, "links": links,
                "words": page.words, "content_type": page.content_type, "warnings": page.warnings
            }))
        }
        Err(e) => (502, json!({"error": e, "url": url})),
    }
}

fn tokens(text: &str) -> Vec<String> {
    text.to_lowercase().split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 2).map(str::to_string).collect()
}

fn fnv(text: &str) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in text.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn embedding(text: &str) -> Vec<f64> {
    let words = tokens(text);
    let mut vector = vec![0.0f64; EMBED_DIM];
    let mut add = |word: &str, weight: f64| {
        let h = fnv(word);
        let index = (h as usize) % EMBED_DIM;
        vector[index] += if h & (1 << 63) == 0 { weight } else { -weight };
    };
    for (i, word) in words.iter().enumerate() {
        add(word, 1.0);
        if let Some(next) = words.get(i + 1) {
            add(&format!("{}_{}", word, next), 0.5);
        }
    }
    let norm = vector.iter().map(|v| v * v).sum::<f64>().sqrt();
    if norm > 0.0 {
        for value in &mut vector {
            *value = (*value / norm * 1_000_000.0).round() / 1_000_000.0;
        }
    }
    vector
}

fn embed(body: &Value) -> (u16, Value) {
    let model = body["model"].as_str().unwrap_or("penelope-hash-256");
    if !model.is_empty() && model != "penelope-hash-256" {
        return (400, json!({"error": "unknown embedding model; this server has penelope-hash-256"}));
    }
    let Some(texts) = body["texts"].as_array() else {
        return (400, json!({"error": "texts must be an array of strings"}));
    };
    if texts.is_empty() || texts.len() > 64 {
        return (400, json!({"error": "texts must hold 1 to 64 strings"}));
    }
    let mut vectors = Vec::new();
    for text in texts {
        let Some(text) = text.as_str() else {
            return (400, json!({"error": "every text must be a string"}));
        };
        if text.chars().count() > 20_000 {
            return (400, json!({"error": "one text is over 20,000 characters"}));
        }
        vectors.push(embedding(text));
    }
    (200, json!({"model": "penelope-hash-256", "dimensions": EMBED_DIM, "vectors": vectors}))
}

fn doc_text(document: &Value) -> String {
    if let Some(text) = document.as_str() {
        return text.to_string();
    }
    let mut parts = Vec::new();
    for key in ["title", "snippet", "content", "text", "url"] {
        if let Some(value) = document[key].as_str() {
            if !value.trim().is_empty() {
                parts.push(value.to_string());
            }
        }
    }
    parts.join(" ")
}

fn rerank(body: &Value) -> (u16, Value) {
    let query = body["query"].as_str().unwrap_or("").trim();
    if query.is_empty() {
        return (400, json!({"error": "query is required"}));
    }
    if query.chars().count() > 1_000 {
        return (400, json!({"error": "query is over 1,000 characters"}));
    }
    let Some(documents) = body["documents"].as_array() else {
        return (400, json!({"error": "documents must be an array"}));
    };
    if documents.is_empty() || documents.len() > 100 {
        return (400, json!({"error": "documents must hold 1 to 100 entries"}));
    }
    let docs: Vec<String> = documents.iter().map(doc_text).collect();
    let query_terms: Vec<String> = {
        let mut seen = HashSet::new();
        tokens(query).into_iter().filter(|w| seen.insert(w.clone())).collect()
    };
    if query_terms.is_empty() {
        return (400, json!({"error": "the query has no searchable word"}));
    }
    let doc_tokens: Vec<Vec<String>> = docs.iter().map(|d| tokens(d)).collect();
    let avg_len = doc_tokens.iter().map(|d| d.len()).sum::<usize>() as f64 / doc_tokens.len() as f64;
    let n = doc_tokens.len() as f64;
    let mut scores = vec![0.0f64; docs.len()];
    for term in &query_terms {
        let df = doc_tokens.iter().filter(|d| d.contains(term)).count() as f64;
        let idf = (1.0 + (n - df + 0.5) / (df + 0.5)).ln();
        for (i, words) in doc_tokens.iter().enumerate() {
            let tf = words.iter().filter(|w| *w == term).count() as f64;
            if tf == 0.0 {
                continue;
            }
            let dl = words.len() as f64;
            let norm = tf + 1.2 * (1.0 - 0.75 + 0.75 * if avg_len > 0.0 { dl / avg_len } else { 1.0 });
            scores[i] += idf * (tf * 2.2) / norm;
        }
    }
    let phrase = query.to_lowercase();
    for (i, doc) in docs.iter().enumerate() {
        if doc.to_lowercase().contains(&phrase) {
            scores[i] += 0.35;
        }
    }
    let max = scores.iter().cloned().fold(0.0f64, f64::max);
    if max > 0.0 {
        for score in &mut scores {
            *score = (*score / max * 1_000_000.0).round() / 1_000_000.0;
        }
    }
    let top_k = body["top_k"].as_u64().unwrap_or(documents.len() as u64).clamp(1, 100) as usize;
    let mut order: Vec<usize> = (0..scores.len()).collect();
    order.sort_by(|a, b| scores[*b].partial_cmp(&scores[*a]).unwrap_or(std::cmp::Ordering::Equal).then(a.cmp(b)));
    let results: Vec<Value> = order.into_iter().take(top_k)
        .map(|i| json!({"index": i, "score": scores[i]})).collect();
    (200, json!({"results": results}))
}

struct Item {
    start: usize,
    end: usize,
    kind: &'static str,
}

fn secret_patterns() -> &'static Vec<(regex::Regex, &'static str)> {
    static PATTERNS: OnceLock<Vec<(regex::Regex, &'static str)>> = OnceLock::new();
    PATTERNS.get_or_init(|| vec![
        (regex::Regex::new(r"(?s)-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----.*?-----END [A-Z0-9 ]*PRIVATE KEY-----").unwrap(), "private_key"),
        (regex::Regex::new(r"\b(?:sk-|ghp_|gho_|ghs_|github_pat_|glpat-|xoxb-|xoxp-|hf_)[A-Za-z0-9_-]{16,}").unwrap(), "key"),
        (regex::Regex::new(r"\bAIza[A-Za-z0-9_-]{30,}").unwrap(), "key"),
        (regex::Regex::new(r"\bAKIA[A-Z0-9]{16,}").unwrap(), "key"),
        (regex::Regex::new(r"\beyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}").unwrap(), "key"),
    ])
}

fn redact_text(text: &str, level: &str) -> (String, Vec<Item>) {
    if level == "off" {
        return (text.to_string(), Vec::new());
    }
    let mut found: Vec<Item> = Vec::new();
    for (pattern, kind) in secret_patterns() {
        for m in pattern.find_iter(text) {
            found.push(Item { start: m.start(), end: m.end(), kind });
        }
    }
    let assignment = regex::Regex::new(r#"(?i)\b(password|passwd|secret|token|api[_-]?key|access[_-]?key|private[_-]?key)\b\s*[:=]\s*["']?([^\s"',;]{6,})"#).unwrap();
    for capture in assignment.captures_iter(text) {
        if let Some(value) = capture.get(2) {
            if !value.as_str().starts_with("//") {
                found.push(Item { start: value.start(), end: value.end(), kind: "secret" });
            }
        }
    }
    let bearer = regex::Regex::new(r"(?i)\bbearer\s+([A-Za-z0-9._~+/=-]{12,})").unwrap();
    for capture in bearer.captures_iter(text) {
        if let Some(value) = capture.get(1) {
            found.push(Item { start: value.start(), end: value.end(), kind: "secret" });
        }
    }
    if level == "pii" {
        for span in crate::pii::spans(text) {
            found.push(Item { start: span.start, end: span.end, kind: span.kind });
        }
    }
    found.sort_by(|a, b| a.start.cmp(&b.start).then(b.end.cmp(&a.end)));
    let mut kept: Vec<Item> = Vec::new();
    for item in found {
        if kept.last().map(|last| item.start < last.end).unwrap_or(false) {
            continue;
        }
        kept.push(item);
    }
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for item in &kept {
        out.push_str(&text[last..item.start]);
        out.push_str(&format!("[masked:{}]", item.kind));
        last = item.end;
    }
    out.push_str(&text[last..]);
    (out, kept)
}

fn redact(body: &Value) -> (u16, Value) {
    let text = body["text"].as_str().unwrap_or("");
    if text.is_empty() {
        return (400, json!({"error": "text is required"}));
    }
    if text.len() > 1_000_000 {
        return (400, json!({"error": "text is over 1 MB"}));
    }
    let level = body["level"].as_str().unwrap_or("rules");
    if !matches!(level, "off" | "rules" | "pii") {
        return (400, json!({"error": "level must be off, rules or pii"}));
    }
    let (masked, items) = redact_text(text, level);
    let items: Vec<Value> = items.into_iter().map(|i| json!({"kind": i.kind, "start": i.start, "end": i.end})).collect();
    let masked_flag = !items.is_empty();
    (200, json!({"text": masked, "items": items, "masked": masked_flag}))
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_bind_needs_a_host_and_a_port_from_1_to_65535() {
        for ok in ["127.0.0.1:8765", "[::1]:8765", "localhost:80", "0.0.0.0:65535"] { assert!(valid_bind(ok), "{}", ok); }
        for bad in ["nonsense", ":80", "host:0", "host:70000", "host:abc", "a b:80", "http://x:80", ""] { assert!(!valid_bind(bad), "{}", bad); }
    }

    use super::*;

    fn state(name: &str, token: Option<&str>) -> (State, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("penelope-server-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let ctx = crate::config::load(&dir.join(".penelope"));
        (State::new(&ctx, token.map(str::to_string)), dir)
    }

    fn request(method: &str, path: &str, body: Value, auth: Option<&str>) -> Request {
        let mut headers = HashMap::new();
        headers.insert("content-type".to_string(), "application/json".to_string());
        if let Some(token) = auth {
            headers.insert("authorization".to_string(), format!("Bearer {}", token));
        }
        Request { method: method.to_string(), path: path.to_string(), headers, body: body.to_string().into_bytes(), peer: "test".to_string() }
    }

    #[test]
    fn health_and_models_are_readable_and_a_token_is_checked() {
        let (state, dir) = state("health", Some("secret"));
        let (code, body) = route(&state, &request("GET", "/health", json!({}), None));
        assert_eq!(code, 200);
        assert_eq!(body["service"], "penelope-server");
        assert_eq!(body["auth"], "bearer");
        let (code, _) = route(&state, &request("GET", "/whoami", json!({}), None));
        assert_eq!(code, 401);
        let (code, body) = route(&state, &request("GET", "/whoami", json!({}), Some("secret")));
        assert_eq!((code, body["token_valid"].as_bool()), (200, Some(true)));
        let (code, _) = route(&state, &request("POST", "/embed", json!({"texts": ["hello"]}), None));
        assert_eq!(code, 401);
        let (code, body) = route(&state, &request("POST", "/embed", json!({"texts": ["hello"]}), Some("secret")));
        assert_eq!((code, body["dimensions"].as_u64()), (200, Some(EMBED_DIM as u64)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn embeddings_are_deterministic_and_normalized() {
        let (state, dir) = state("embed", None);
        let body = json!({"texts": ["rust error handling", "rust error handling", "pasta"]});
        let (code, answer) = route(&state, &request("POST", "/embed", body, None));
        assert_eq!(code, 200);
        let vectors = answer["vectors"].as_array().unwrap();
        assert_eq!(vectors.len(), 3);
        assert_eq!(vectors[0], vectors[1]);
        assert_ne!(vectors[0], vectors[2]);
        let norm = vectors[0].as_array().unwrap().iter().map(|v| v.as_f64().unwrap().powi(2)).sum::<f64>().sqrt();
        assert!((norm - 1.0).abs() < 0.0001, "{}", norm);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rerank_puts_the_document_with_the_query_words_first() {
        let (state, dir) = state("rerank", None);
        let body = json!({"query": "rust error handling", "top_k": 2, "documents": [
            "cooking pasta with tomato", "Rust error handling with anyhow", "a rust game"]});
        let (code, answer) = route(&state, &request("POST", "/rerank", body, None));
        assert_eq!(code, 200);
        assert_eq!(answer["results"][0]["index"], 1);
        assert_eq!(answer["results"].as_array().unwrap().len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn redact_masks_secrets_and_keeps_byte_offsets() {
        let text = "password=hunter2 and sk-abcdefghijklmnopqrstuvwxyz";
        let (masked, items) = redact_text(text, "rules");
        assert!(masked.contains("[masked:secret]") && masked.contains("[masked:key]"), "{}", masked);
        assert_eq!(items.len(), 2);
        assert_eq!(&text[items[0].start..items[0].end], "hunter2");
        assert_eq!(&text[items[1].start..items[1].end], "sk-abcdefghijklmnopqrstuvwxyz");
        let (plain, none) = redact_text("mail a@b.com", "rules");
        assert_eq!((plain.as_str(), none.len()), ("mail a@b.com", 0));
        let (pii, some) = redact_text("mail a@b.com", "pii");
        assert!(pii.contains("[masked:email]") && some.len() == 1);
    }

    #[test]
    fn the_reader_endpoint_refuses_a_private_address_without_a_request() {
        let (state, dir) = state("reader", None);
        for url in ["http://127.0.0.1/x", "http://10.0.0.1/x", "file:///etc/passwd"] {
            let (code, answer) = route(&state, &request("POST", "/read", json!({"url": url}), None));
            assert_eq!(code, 400, "{}", url);
            assert!(answer["error"].as_str().unwrap().contains("private"));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn search_and_rerank_refuse_an_empty_body() {
        let (state, dir) = state("empty", None);
        let (code, _) = route(&state, &request("POST", "/search", json!({}), None));
        assert_eq!(code, 400);
        let (code, _) = route(&state, &request("POST", "/rerank", json!({"query": "x"}), None));
        assert_eq!(code, 400);
        let (code, _) = route(&state, &request("GET", "/search", json!({}), None));
        assert_eq!(code, 405);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
