//! Tool search: finds a capability that no connected tool has, and installs it only after a yes in the TUI.
//!
//! Step 1, local: the tools of the running MCP servers (`mcpclient::cached_tools`, no server is started for this),
//! the names of every registered server, the installed plugins (name, skills, commands) and the standalone skills.
//! Each is one document of name plus first sentence, ranked with BM25.
//!
//! Step 2, registry: the official MCP Registry, `GET https://registry.modelcontextprotocol.io/v0.1/servers`
//! (OpenAPI 1.0.0 of that host, read 2026-11-27). The `search` parameter is a substring match on the server name with
//! at least 3 characters, so at most `KEYWORDS` single words go out, one request each, with `limit=30&version=latest`.
//! The answer is cached in `tool-search-cache.json` in the project root for 24 hours, keyed by keyword. No key is
//! needed. An unreachable registry gives the local results and one line that says so.
//!
//! Privacy: only the registry step sends anything. `scrub` removes home paths, emails and token-like strings first,
//! and every report names the exact keywords that were sent.
//!
//! The model tool `tool_search` only reads and suggests. Installing happens in `/tools install N` followed by
//! `/tools yes`, both typed by the person; `plugins::install` (vet scan, removal of a refused clone) or
//! `plugins::mcp_add` (tools denied until allowed) does the work.

use crate::config::Ctx;
use regex::Regex;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

pub const REGISTRY: &str = "https://registry.modelcontextprotocol.io/v0.1/servers";
/// Registry keywords per search. Each one is one request.
pub const KEYWORDS: usize = 2;
/// Seconds a cached registry answer stays valid.
pub const TTL: u64 = 24 * 3600;
const STOP: &[&str] = &["the", "and", "for", "with", "that", "this", "from", "into", "what", "which", "need", "needs",
    "tool", "tools", "server", "servers", "mcp", "can", "could", "would", "should", "have", "has", "get", "use", "using",
    "want", "make", "some", "any", "all", "my", "our", "your", "you", "are", "was", "how", "to", "of", "in", "on", "a", "an", "is", "it", "or", "me"];

/// How a candidate gets installed. `Local` is already here.
#[derive(Clone, Debug, PartialEq)]
pub enum Install {
    Local,
    /// A stdio server started by a package runner: command and arguments for `plugins::mcp_add`.
    Package { command: String, args: Vec<String> },
    /// A git link for `plugins::install`.
    Repo(String),
    /// A remote server. Penelope starts stdio servers only, so it is shown and not installed.
    Remote(String),
}

#[derive(Clone, Debug)]
pub struct Candidate {
    pub name: String,
    pub line: String,
    /// "local" or "registry".
    pub source: String,
    pub install: Install,
    pub repo: String,
    pub score: f64,
}

impl Candidate {
    /// The install command or package as one line.
    pub fn install_text(&self) -> String {
        match &self.install {
            Install::Local => "already installed".to_string(),
            Install::Package { command, args } => format!("{} {}", command, args.join(" ")),
            Install::Repo(url) => format!("/plugins install {}", url),
            Install::Remote(url) => format!("remote {} (not installable, stdio only)", url),
        }
    }
}

#[derive(Debug, Default)]
pub struct Report {
    pub candidates: Vec<Candidate>,
    /// The keywords that went to the registry, each as one request. Empty when nothing left the machine.
    pub sent: Vec<String>,
    pub note: Option<String>,
}

pub fn tokens(text: &str) -> Vec<String> {
    text.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|t| t.chars().count() >= 2 && !STOP.contains(t))
        .map(String::from).collect()
}

/// BM25 over small documents. Returns one score per document, in order.
pub fn bm25(query: &[String], docs: &[Vec<String>]) -> Vec<f64> {
    let n = docs.len() as f64;
    if docs.is_empty() { return Vec::new(); }
    let avg = (docs.iter().map(|d| d.len()).sum::<usize>() as f64 / n).max(1.0);
    let (k1, b) = (1.5, 0.75);
    let uniq: HashSet<&String> = query.iter().collect();
    let mut df: HashMap<&String, f64> = HashMap::new();
    for q in &uniq {
        df.insert(q, docs.iter().filter(|d| d.contains(q)).count() as f64);
    }
    docs.iter().map(|d| {
        let len = d.len() as f64;
        uniq.iter().map(|q| {
            let tf = d.iter().filter(|t| t == q).count() as f64;
            if tf == 0.0 { return 0.0; }
            let f = df[q];
            let idf = ((n - f + 0.5) / (f + 0.5) + 1.0).ln();
            idf * tf * (k1 + 1.0) / (tf + k1 * (1.0 - b + b * len / avg))
        }).sum()
    }).collect()
}

fn first_sentence(text: &str) -> String {
    let text = text.trim();
    let end = text.find(". ").map(|i| i + 1).unwrap_or(text.len());
    let short: String = text[..end].chars().take(120).collect();
    short.replace('\n', " ").trim().to_string()
}

/// Removes what must not leave the machine: paths under a home folder, emails and token-like strings.
pub fn scrub(query: &str) -> String {
    let mut text = query.to_string();
    let rules = [
        r"(?i)\b(?:bearer|token|key|secret|password)\s*[:=]\s*\S+",
        r"[\w.+-]+@[\w-]+(?:\.[\w-]+)+",
        r"(?i)(?:~|/home/[^\s/]+|/Users/[^\s/]+|[a-z]:\\Users\\[^\s\\]+)(?:[/\\]\S*)?",
        r"\b(?:sk|pk|ghp|gho|ghs|github_pat|xox[a-z]|glpat|AKIA|AIza|hf|npm)[-_A-Za-z0-9]{8,}",
        r"\b[A-Fa-f0-9]{24,}\b",
        r"\b[A-Za-z0-9+/_-]{28,}={0,2}",
    ];
    for rule in rules {
        if let Ok(re) = Regex::new(rule) {
            text = re.replace_all(&text, " ").to_string();
        }
    }
    if let Some(home) = std::env::var("HOME").ok().or_else(|| std::env::var("USERPROFILE").ok()).filter(|h| h.len() > 1) {
        text = text.replace(&home, " ");
    }
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The words that go to the registry: the longest distinct words of the scrubbed query, at most `KEYWORDS`.
pub fn keywords(scrubbed: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut words: Vec<String> = tokens(scrubbed).into_iter().filter(|t| t.chars().count() >= 3 && seen.insert(t.clone())).collect();
    words.sort_by_key(|w| std::cmp::Reverse(w.chars().count()));
    words.truncate(KEYWORDS);
    words
}

// ----- local

struct Doc { cand: Candidate, tokens: Vec<String> }

fn doc(name: &str, line: &str, install: Install) -> Doc {
    let mut t = tokens(name);
    t.extend(tokens(name));
    t.extend(tokens(name));
    t.extend(tokens(line));
    Doc { cand: Candidate { name: name.to_string(), line: line.to_string(), source: "local".to_string(), install, repo: String::new(), score: 0.0 }, tokens: t }
}

fn local_docs(ctx: &Ctx) -> Vec<Doc> {
    let mut out = Vec::new();
    for (server, t) in crate::mcpclient::cached_tools(ctx) {
        out.push(doc(&format!("{}/{}", server, t.name), &first_sentence(&t.description), Install::Local));
    }
    if let Some(servers) = crate::plugins::servers(ctx).as_object() {
        for name in servers.keys() {
            out.push(doc(&format!("mcp server {}", name), "MCP server, run mcp_servers with probe to list its tools", Install::Local));
        }
    }
    for p in crate::plugins::entries(ctx) {
        out.push(doc(&format!("plugin {}", p.name), &format!("{} skills, {} commands", p.skills.len(), p.commands.len()), Install::Local));
        for s in p.skills.iter().chain(p.commands.iter()) {
            out.push(doc(&format!("{}:{}", p.name, s.name), &first_sentence(&s.description), Install::Local));
        }
    }
    for (name, description) in crate::skills::list(ctx) {
        out.push(doc(&format!("skill {}", name), &first_sentence(&description), Install::Local));
    }
    out
}

/// Ranks the local documents. Documents with score 0 are dropped.
pub fn local(ctx: &Ctx, query: &str, limit: usize) -> Vec<Candidate> {
    rank(local_docs(ctx), query, limit)
}

fn rank(docs: Vec<Doc>, query: &str, limit: usize) -> Vec<Candidate> {
    let q = tokens(query);
    if q.is_empty() { return Vec::new(); }
    let scores = bm25(&q, &docs.iter().map(|d| d.tokens.clone()).collect::<Vec<_>>());
    let mut out: Vec<Candidate> = docs.into_iter().zip(scores).filter(|(_, s)| *s > 0.0)
        .map(|(d, s)| Candidate { score: s, ..d.cand }).collect();
    out.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal).then(a.name.cmp(&b.name)));
    out.truncate(limit);
    out
}

// ----- registry

/// Reads one registry answer. Only the newest entry of a server name is kept.
pub fn parse_registry(body: &str) -> Result<Vec<Candidate>, String> {
    let v: Value = serde_json::from_str(body).map_err(|e| format!("registry answer is not JSON: {}", e))?;
    let list = v["servers"].as_array().ok_or("registry answer has no servers list")?;
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for item in list {
        let s = &item["server"];
        let Some(name) = s["name"].as_str().filter(|n| !n.is_empty()) else { continue };
        if item["_meta"]["io.modelcontextprotocol.registry/official"]["status"].as_str().map(|st| st != "active").unwrap_or(false) { continue; }
        if !seen.insert(name.to_string()) { continue; }
        let repo = s["repository"]["url"].as_str().unwrap_or("").to_string();
        let package = s["packages"].as_array().and_then(|ps| ps.iter().find_map(|p| {
            let id = p["identifier"].as_str().filter(|i| !i.is_empty())?;
            let version = p["version"].as_str().filter(|v| !v.is_empty());
            match p["registryType"].as_str()? {
                "npm" => Some(Install::Package { command: "npx".to_string(), args: vec!["-y".to_string(), match version { Some(v) => format!("{}@{}", id, v), None => id.to_string() }] }),
                "pypi" => Some(Install::Package { command: "uvx".to_string(), args: vec![id.to_string()] }),
                _ => None,
            }
        }));
        let install = match (package, repo.is_empty(), s["remotes"][0]["url"].as_str()) {
            (Some(p), _, _) => p,
            (None, false, _) => Install::Repo(repo.clone()),
            (None, true, Some(url)) => Install::Remote(url.to_string()),
            _ => continue,
        };
        out.push(Candidate { name: name.to_string(), line: first_sentence(s["description"].as_str().unwrap_or("")), source: "registry".to_string(),
                             install, repo, score: 0.0 });
    }
    Ok(out)
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn cache_path(ctx: &Ctx) -> std::path::PathBuf { ctx.root.join("tool-search-cache.json") }

fn cache_get(ctx: &Ctx, keyword: &str, at: u64) -> Option<String> {
    let v: Value = serde_json::from_str(&fs::read_to_string(cache_path(ctx)).ok()?).ok()?;
    let e = &v["entries"][keyword];
    (at.saturating_sub(e["at"].as_u64()?) < TTL).then(|| e["body"].as_str().map(String::from))?
}

fn cache_put(ctx: &Ctx, keyword: &str, body: &str, at: u64) {
    let mut v: Value = fs::read_to_string(cache_path(ctx)).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_else(|| json!({"entries": {}}));
    if !v["entries"].is_object() { v["entries"] = json!({}); }
    v["entries"][keyword] = json!({"at": at, "body": body});
    let _ = fs::write(cache_path(ctx), serde_json::to_string(&v).unwrap_or_default());
}

fn http_get(keyword: &str) -> Result<String, String> {
    crate::net::agent(10).get(REGISTRY).query("search", keyword).query("limit", "30").query("version", "latest").call()
        .map_err(|e| e.to_string())?.into_string().map_err(|e| e.to_string())
}

/// Local plus registry. `fetch` gets one keyword and returns the raw registry body; tests pass a fixed one.
pub fn search_with(ctx: &Ctx, query: &str, limit: usize, fetch: &dyn Fn(&str) -> Result<String, String>) -> Report {
    let limit = limit.clamp(1, 20);
    let mut report = Report { candidates: local(ctx, query, limit), ..Default::default() };
    let words = keywords(&scrub(query));
    if words.is_empty() {
        report.note = Some("registry not asked: no keyword of 3 or more letters is left after removing paths, emails and tokens".to_string());
        return report;
    }
    let at = now();
    let mut remote: Vec<Candidate> = Vec::new();
    let mut failed = None;
    for w in &words {
        let body = match cache_get(ctx, w, at) {
            Some(b) => b,
            None => match fetch(w) {
                Ok(b) => {
                    report.sent.push(w.clone());
                    if parse_registry(&b).is_ok() { cache_put(ctx, w, &b, at); }
                    b
                }
                Err(e) => { report.sent.push(w.clone()); failed = Some(e); continue; }
            },
        };
        match parse_registry(&body) {
            Ok(list) => remote.extend(list),
            Err(e) => failed = Some(e),
        }
    }
    if remote.is_empty() {
        if let Some(e) = failed {
            report.note = Some(format!("the MCP Registry could not be reached ({}); local results only", e.chars().take(120).collect::<String>()));
        }
    }
    let mut seen = HashSet::new();
    remote.retain(|c| seen.insert(c.name.clone()));
    let docs: Vec<Doc> = remote.into_iter().map(|c| {
        let mut t = tokens(&c.name.replace(['/', '.', '-', '_'], " "));
        t.extend(t.clone());
        t.extend(tokens(&c.line));
        Doc { cand: c, tokens: t }
    }).collect();
    // Registry names are matched by the keywords, so rank them by the whole query and keep the unmatched tail.
    let mut ranked = rank(docs, query, limit);
    report.candidates.append(&mut ranked);
    report
}

pub fn search(ctx: &Ctx, query: &str, limit: usize) -> Report {
    search_with(ctx, query, limit, &http_get)
}

pub fn render(report: &Report) -> String {
    let mut out = String::new();
    for (i, c) in report.candidates.iter().enumerate() {
        out.push_str(&format!("{}. {} [{}] {}\n   install: {}", i + 1, c.name, c.source, c.line, c.install_text()));
        if !c.repo.is_empty() { out.push_str(&format!("\n   repository: {}", c.repo)); }
        out.push('\n');
    }
    if report.candidates.is_empty() { out.push_str("no candidate found\n"); }
    if report.sent.is_empty() {
        out.push_str("sent to the registry: nothing\n");
    } else {
        out.push_str(&format!("sent to the registry: {}\n", report.sent.iter().map(|w| format!("search={}", w)).collect::<Vec<_>>().join(", ")));
    }
    if let Some(n) = &report.note { out.push_str(&format!("{}\n", n)); }
    out.trim_end().to_string()
}

// ----- model tool

/// The model tool. It reads and suggests; any argument that asks for an install is refused.
pub fn tool(ctx: &Ctx, args: &Value) -> String {
    if ["install", "confirm", "yes", "approve"].iter().any(|k| args.get(*k).map(|v| !matches!(v, Value::Null | Value::Bool(false))).unwrap_or(false)) {
        return "NOT installed. tool_search only suggests. The user installs with /tools install N and /tools yes.".to_string();
    }
    let query = args["query"].as_str().unwrap_or("").trim();
    if query.is_empty() {
        return "give the capability to look for, for example: query \"read a PDF\"".to_string();
    }
    let limit = args["limit"].as_u64().unwrap_or(8) as usize;
    let report = search(ctx, query, limit);
    save_last(ctx, &report.candidates);
    format!("{}\nTo install, ask the user to run /tools search {} and choose a number.", render(&report), scrub(query))
}

// ----- TUI side: last list, pending install

fn last_path(ctx: &Ctx) -> std::path::PathBuf { ctx.root.join("tool-search-last.json") }

fn encode(c: &Candidate) -> Value {
    let (kind, command, args, url) = match &c.install {
        Install::Local => ("local", String::new(), vec![], String::new()),
        Install::Package { command, args } => ("package", command.clone(), args.clone(), String::new()),
        Install::Repo(u) => ("repo", String::new(), vec![], u.clone()),
        Install::Remote(u) => ("remote", String::new(), vec![], u.clone()),
    };
    json!({"name": c.name, "line": c.line, "source": c.source, "repo": c.repo, "kind": kind, "command": command, "args": args, "url": url})
}

fn decode(v: &Value) -> Candidate {
    let s = |k: &str| v[k].as_str().unwrap_or("").to_string();
    let install = match v["kind"].as_str().unwrap_or("") {
        "package" => Install::Package { command: s("command"), args: v["args"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default() },
        "repo" => Install::Repo(s("url")),
        "remote" => Install::Remote(s("url")),
        _ => Install::Local,
    };
    Candidate { name: s("name"), line: s("line"), source: s("source"), install, repo: s("repo"), score: 0.0 }
}

pub fn save_last(ctx: &Ctx, list: &[Candidate]) {
    let _ = fs::write(last_path(ctx), serde_json::to_string(&list.iter().map(encode).collect::<Vec<_>>()).unwrap_or_default());
}

pub fn last(ctx: &Ctx) -> Vec<Candidate> {
    fs::read_to_string(last_path(ctx)).ok().and_then(|s| serde_json::from_str::<Vec<Value>>(&s).ok())
        .map(|l| l.iter().map(decode).collect()).unwrap_or_default()
}

/// One install that waits for the person's `/tools yes`. Only `request` sets it, and only the TUI calls `request`.
static PENDING: Mutex<Option<Candidate>> = Mutex::new(None);

fn server_name(name: &str) -> String {
    let tail = name.rsplit('/').next().unwrap_or(name);
    let s: String = tail.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '-' }).collect();
    s.trim_matches('-').to_string()
}

/// `/tools install N [repo]`: names exactly what would run and waits. Nothing is installed here.
pub fn request(ctx: &Ctx, n: usize, force_repo: bool) -> String {
    let list = last(ctx);
    let Some(c) = n.checked_sub(1).and_then(|i| list.get(i)).cloned() else {
        return format!("no candidate {}. /tools search QUERY lists them first.", n);
    };
    let c = match (&c.install, force_repo, c.repo.is_empty()) {
        (_, true, false) => Candidate { install: Install::Repo(c.repo.clone()), ..c },
        (_, true, true) => return format!("{} has no repository link.", c.name),
        _ => c,
    };
    let plan = match &c.install {
        Install::Local => return format!("{} is already installed.", c.name),
        Install::Remote(_) => return format!("{} is a remote server. Penelope starts stdio servers only; nothing to install.", c.name),
        Install::Package { command, args } => format!("register the MCP server {} as `{} {}`, tools denied until /mcp allow {}. The package code is downloaded by {} when the server starts and is NOT scanned by vet; /tools install {} repo scans the repository instead.",
            server_name(&c.name), command, args.join(" "), server_name(&c.name), command, n),
        Install::Repo(url) => format!("clone {} into the plugin folder, scan it with vet, delete the clone if the scan refuses it, register it with its tools denied.", url),
    };
    *PENDING.lock().unwrap_or_else(|e| e.into_inner()) = Some(c.clone());
    format!("Will {}\nType /tools yes to do it, /tools no to cancel.", plan)
}

pub fn cancel() -> String {
    match PENDING.lock().unwrap_or_else(|e| e.into_inner()).take() {
        Some(c) => format!("cancelled: {} not installed", c.name),
        None => "nothing waits for a yes".to_string(),
    }
}

/// `/tools yes`: runs the waiting install through the existing paths.
pub fn confirm(ctx: &Ctx) -> String {
    let Some(c) = PENDING.lock().unwrap_or_else(|e| e.into_inner()).take() else {
        return "nothing waits for a yes. /tools install N first.".to_string();
    };
    match c.install {
        Install::Package { command, args } => crate::plugins::mcp_add(ctx, &server_name(&c.name), &command, &args, false),
        Install::Repo(url) => crate::plugins::install(ctx, &url),
        _ => format!("{} cannot be installed.", c.name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{"servers":[
      {"server":{"name":"com.example/pdf-reader","description":"Read text and tables from PDF files. Fast.","repository":{"url":"https://github.com/example/pdf-reader","source":"github"},"version":"1.2.0",
        "packages":[{"registryType":"npm","identifier":"pdf-reader-mcp","version":"1.2.0","transport":{"type":"stdio"}}]},
       "_meta":{"io.modelcontextprotocol.registry/official":{"status":"active","isLatest":true}}},
      {"server":{"name":"io.foo/sheet","description":"Spreadsheet access.","repository":{"url":"https://github.com/foo/sheet"},"version":"0.1.0"},
       "_meta":{"io.modelcontextprotocol.registry/official":{"status":"active"}}},
      {"server":{"name":"ai.cloud/only-remote","description":"Hosted thing.","version":"1","remotes":[{"type":"streamable-http","url":"https://mcp.cloud.ai/mcp"}]}},
      {"server":{"name":"old/gone","description":"pdf old","version":"1","repository":{"url":"https://github.com/old/gone"}},"_meta":{"io.modelcontextprotocol.registry/official":{"status":"deleted"}}},
      {"server":{"name":"py/pdf-tool","description":"PDF in Python","version":"3","packages":[{"registryType":"pypi","identifier":"pdf-tool","version":"3"}]}}
    ],"metadata":{"nextCursor":"x","count":5}}"#;

    static LOCK: Mutex<()> = Mutex::new(());

    fn ctx(tag: &str) -> Ctx {
        let root = std::env::temp_dir().join(format!("penelope-toolsearch-{}-{}", tag, std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        crate::config::load(&root)
    }

    #[test]
    fn bm25_ranks_the_name_match_first() {
        let docs = vec![
            doc("fs/read_file", "Read a text file", Install::Local),
            doc("db/query", "Run a SQL query on a database", Install::Local),
            doc("web/fetch", "Fetch a web page", Install::Local),
        ];
        let ranked = rank(docs, "query my sql database", 5);
        assert_eq!(ranked[0].name, "db/query");
        assert!(ranked.iter().all(|c| c.name != "web/fetch"), "a zero score is dropped");
    }

    #[test]
    fn local_search_reads_skills_of_the_project() {
        let c = ctx("local");
        crate::skills::write(&c, "pdf-notes", "Summarize a PDF into notes. Use for papers.", "A body of enough characters for the writer.").unwrap();
        crate::skills::write(&c, "git-tidy", "Clean up merged branches.", "A body of enough characters for the writer.").unwrap();
        let hits = local(&c, "summarize pdf", 5);
        assert_eq!(hits[0].name, "skill pdf-notes");
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn scrub_removes_paths_emails_and_tokens() {
        let q = "convert /home/rene/work/secret.pdf for rene@example.com with ghp_abcdefghijklmnop1234 and ~/docs/a.txt and C:\\Users\\Rene\\x.doc using sk-ant1234567890abcdef plus token=abc123 about spreadsheets";
        let s = scrub(q);
        for bad in ["rene", "example.com", "ghp_", "secret", "~/", "Users", "sk-ant", "abc123"] {
            assert!(!s.contains(bad), "{} left in {:?}", bad, s);
        }
        assert!(s.contains("convert") && s.contains("spreadsheets"), "{:?}", s);
        assert_eq!(keywords(&s), vec!["spreadsheets", "convert"]);
    }

    #[test]
    fn the_registry_sample_is_parsed() {
        let list = parse_registry(SAMPLE).unwrap();
        let names: Vec<&str> = list.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["com.example/pdf-reader", "io.foo/sheet", "ai.cloud/only-remote", "py/pdf-tool"], "the deleted entry is dropped");
        assert_eq!(list[0].install, Install::Package { command: "npx".into(), args: vec!["-y".into(), "pdf-reader-mcp@1.2.0".into()] });
        assert_eq!(list[0].repo, "https://github.com/example/pdf-reader");
        assert_eq!(list[0].line, "Read text and tables from PDF files.");
        assert_eq!(list[1].install, Install::Repo("https://github.com/foo/sheet".into()));
        assert_eq!(list[2].install, Install::Remote("https://mcp.cloud.ai/mcp".into()));
        assert_eq!(list[3].install, Install::Package { command: "uvx".into(), args: vec!["pdf-tool".into()] });
        assert!(parse_registry("not json").is_err());
    }

    #[test]
    fn the_registry_step_sends_scrubbed_words_and_caches() {
        let c = ctx("net");
        let calls = std::cell::RefCell::new(Vec::new());
        let fetch = |w: &str| -> Result<String, String> { calls.borrow_mut().push(w.to_string()); Ok(SAMPLE.to_string()) };
        let r = search_with(&c, "read pdf in /home/rene/a.pdf for me@x.org", 5, &fetch);
        assert_eq!(r.sent, vec!["read".to_string(), "pdf".to_string()]);
        assert!(r.sent.iter().all(|w| !w.contains("rene") && !w.contains('@')), "{:?}", r.sent);
        assert_eq!(r.candidates[0].name, "com.example/pdf-reader");
        let text = render(&r);
        assert!(text.contains("repository: https://github.com/example/pdf-reader") && text.contains("sent to the registry: search="), "{}", text);
        let first = calls.borrow().len();
        let again = search_with(&c, "read pdf in /home/rene/a.pdf for me@x.org", 5, &fetch);
        assert_eq!(calls.borrow().len(), first, "the second search is answered from the cache");
        assert!(again.sent.is_empty());
        assert!(render(&again).contains("sent to the registry: nothing"));
    }

    #[test]
    fn an_unreachable_registry_gives_local_results_and_a_note() {
        let c = ctx("down");
        crate::skills::write(&c, "pdf-notes", "Summarize a PDF into notes.", "A body of enough characters for the writer.").unwrap();
        let r = search_with(&c, "summarize pdf", 5, &|_| Err("connection refused".to_string()));
        assert_eq!(r.candidates.len(), 1);
        assert!(r.note.unwrap().contains("could not be reached"));
    }

    #[test]
    fn a_query_of_only_secrets_sends_nothing() {
        let c = ctx("none");
        let r = search_with(&c, "/home/rene/x ghp_abcdefghijklmnop1234", 5, &|_| panic!("must not be called"));
        assert!(r.sent.is_empty() && r.note.is_some());
    }

    #[test]
    fn the_model_tool_never_installs() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let c = ctx("model");
        save_last(&c, &parse_registry(SAMPLE).unwrap());
        let said = tool(&c, &json!({"query": "pdf", "install": true}));
        assert!(said.starts_with("NOT installed"), "{}", said);
        let said = tool(&c, &json!({"query": "pdf", "confirm": "yes"}));
        assert!(said.starts_with("NOT installed"), "{}", said);
        assert!(PENDING.lock().unwrap().is_none(), "the model tool sets no pending install");
        assert!(!c.root.join("mcp.json").exists() && !c.root.join("plugins.json").exists());
        assert!(tool(&c, &json!({})).contains("give the capability"));
    }

    #[test]
    fn an_install_waits_for_the_yes_of_the_person() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let c = ctx("yes");
        save_last(&c, &parse_registry(SAMPLE).unwrap());
        let plan = request(&c, 1, false);
        assert!(plan.contains("npx -y pdf-reader-mcp@1.2.0") && plan.contains("NOT scanned by vet"), "{}", plan);
        assert!(!c.root.join("mcp.json").exists(), "asking changes nothing");
        assert!(request(&c, 3, false).contains("remote"));
        assert!(request(&c, 9, false).contains("no candidate"));
        let _ = request(&c, 1, false);
        assert!(cancel().starts_with("cancelled"));
        assert!(confirm(&c).contains("nothing waits"));
        let _ = request(&c, 1, false);
        let said = confirm(&c);
        assert!(said.contains("pdf-reader added") && said.contains("denied"), "{}", said);
        assert_eq!(crate::plugins::mcp_load(&c)["servers"]["pdf-reader"]["allow"], false);
        let repo = request(&c, 1, true);
        assert!(repo.contains("scan it with vet"), "{}", repo);
        let _ = cancel();
    }
}
