//! The knowledge graph over the memory of Penelope.
//!
//! One function, `Graph::build`, reads three sources and makes one graph:
//! - the project memory (`.penelope/memory`) and the shared memory (`~/.penelope/global-memory`): every waypoint is a
//!   node, its notes are the text of the node;
//! - the cluster memory logs (`.penelope/clusters/ID/memory.log`): every entry is a node.
//!
//! Edges: `child` (waypoint to child waypoint), `mention` (a `[[name]]` reference, or the id or a long unique title of
//! another waypoint written in the text), `file` (a node names a file path, so it points at a file node; two nodes that
//! name the same path are two steps apart through that node) and `link` (a node contains a URL, so it points at a link
//! node). Nothing here calls a model or the network.
//!
//! Levels: a waypoint holds notes of the person (level 3, as `mitos::origin_level` says for `memory_go`). A cluster
//! entry keeps the level of its log line. A file node is level 1, a link node level 0.
//!
//! `find` is the pointer behind the `memory_find` tool: it returns ids, titles, one line and a path, never a whole note.

use crate::config::Ctx;
use crate::memory::Memory;
use regex::Regex;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

const TEXT_CAP: usize = 4000;
const LOG_TAIL: usize = 500;
const DAY: u64 = 86_400;
/// A pointer needs at least this score, see `Graph::find`.
pub const FIND_THRESHOLD: f64 = 0.2;
const FILE_EXTENSIONS: &[&str] = &["rs", "md", "toml", "json", "py", "ts", "tsx", "js", "jsx", "txt", "yaml", "yml", "sh", "html", "css",
    "lock", "cfg", "ini", "c", "h", "cpp", "go", "java", "kt", "rb", "sql", "pdf", "log", "csv", "tex", "swift", "dart", "lua"];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Note,
    Rule,
    ClusterEntry,
    File,
    Link,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Note => "note",
            Kind::Rule => "rule",
            Kind::ClusterEntry => "cluster-entry",
            Kind::File => "file",
            Kind::Link => "link",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Scope {
    Project,
    Global,
    Cluster,
    /// A file or a link node, made from the text of other nodes.
    Derived,
}

impl Scope {
    pub fn name(self) -> &'static str {
        match self {
            Scope::Project => "project",
            Scope::Global => "global",
            Scope::Cluster => "cluster",
            Scope::Derived => "derived",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum EdgeKind {
    Child,
    Mention,
    File,
    Link,
}

impl EdgeKind {
    pub fn name(self) -> &'static str {
        match self {
            EdgeKind::Child => "child",
            EdgeKind::Mention => "mention",
            EdgeKind::File => "file",
            EdgeKind::Link => "link",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Node {
    /// `p:rust/errors`, `g:rules/style`, `c:CLUSTER:LINE`, `f:src/main.rs`, `u:https://...`
    pub id: String,
    pub title: String,
    pub kind: Kind,
    pub scope: Scope,
    /// Origin level, 0 outside data to 3 person.
    pub level: u8,
    pub uses: u64,
    /// Seconds since the epoch: when the node first appeared (file creation time where the system knows it).
    pub added: u64,
    /// Seconds since the epoch: the last change.
    pub changed: u64,
    pub text: String,
    /// The notes file, the log line, the file path or the URL.
    pub path: String,
}

#[derive(Clone, Debug)]
pub struct Edge {
    pub from: String,
    pub to: String,
    pub kind: EdgeKind,
}

#[derive(Clone, Debug)]
pub struct Neighbour {
    pub id: String,
    /// True when the edge leaves the node asked about.
    pub outgoing: bool,
    pub kind: EdgeKind,
}

pub struct Stats {
    pub nodes: usize,
    pub edges: usize,
    pub by_kind: BTreeMap<&'static str, usize>,
    pub added_7d: usize,
    pub truncated: bool,
}

pub struct Pointer {
    pub id: String,
    pub title: String,
    pub line: String,
    pub path: String,
    pub level: u8,
    pub scope: &'static str,
    pub kind: &'static str,
    pub score: f64,
}

pub struct Opts {
    /// The most waypoints and entries. File and link nodes are capped at half of it.
    pub max_nodes: usize,
    pub now: u64,
}

impl Opts {
    pub fn standard() -> Opts {
        Opts { max_nodes: 3000, now: now() }
    }
}

pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[derive(Default)]
pub struct Graph {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    /// How many waypoints and entries exist before the cap cut the list.
    pub total: usize,
    pub truncated: bool,
    index: HashMap<String, usize>,
    adjacent: Vec<Vec<usize>>,
}

fn secs(t: std::time::SystemTime) -> u64 {
    t.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn cut(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

fn one_line(text: &str, max: usize) -> String {
    let flat: String = text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    cut(flat.split_whitespace().collect::<Vec<_>>().join(" ").as_str(), max)
}

struct Patterns {
    wiki: Regex,
    url: Regex,
    file: Regex,
    token: Regex,
    word: Regex,
}

impl Patterns {
    fn new() -> Patterns {
        Patterns {
            wiki: Regex::new(r"\[\[([^\[\]\n]{1,120})\]\]").unwrap(),
            url: Regex::new(r#"https?://[^\s<>"'`\]\)]+"#).unwrap(),
            file: Regex::new(r"[A-Za-z0-9_~@+.-]+(?:/[A-Za-z0-9_@+.-]+)*\.[A-Za-z][A-Za-z0-9]{0,4}").unwrap(),
            token: Regex::new(r"[\p{L}\p{N}][\p{L}\p{N}_./-]*").unwrap(),
            word: Regex::new(r"[\p{L}\p{N}+#]+").unwrap(),
        }
    }
}

fn trim_url(url: &str) -> String {
    url.trim_end_matches(|c: char| ".,;:!?)>]}'\"`".contains(c)).to_string()
}

/// File paths named in a text, URLs left out. A name without a slash needs a known extension.
fn file_paths(p: &Patterns, text: &str) -> Vec<String> {
    let blank = p.url.replace_all(text, " ");
    let mut out: Vec<String> = Vec::new();
    for m in p.file.find_iter(&blank) {
        let raw = m.as_str().trim_start_matches("./").trim_matches(|c| c == '.' || c == '-');
        if raw.is_empty() || raw.contains("..") || (raw.starts_with('~') && !raw.starts_with("~/")) {
            continue;
        }
        let ext = raw.rsplit('.').next().unwrap_or("").to_lowercase();
        let stem = raw.rsplit_once('.').map(|(s, _)| s).unwrap_or("");
        if stem.is_empty() || stem.chars().all(|c| c.is_ascii_digit() || c == '.') {
            continue;
        }
        if !raw.contains('/') && !FILE_EXTENSIONS.contains(&ext.as_str()) {
            continue;
        }
        if !out.iter().any(|x| x == raw) {
            out.push(raw.to_string());
        }
    }
    out
}

fn urls(p: &Patterns, text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for m in p.url.find_iter(text) {
        let u = trim_url(m.as_str());
        if u.len() > "https://".len() && !out.contains(&u) {
            out.push(u);
        }
    }
    out
}

fn read_json(path: &Path) -> Value {
    fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Value::Null)
}

fn notes_file(dir: &Path, id: &str) -> PathBuf {
    let name = if id.is_empty() { "start".to_string() } else { id.replace('/', "__") };
    dir.join("notes").join(format!("{}.md", name))
}

fn is_rule(id: &str, notes: &[String]) -> bool {
    (!notes.is_empty() && (id == "rules" || id.starts_with("rules/"))) || notes.iter().any(|n| crate::memory::scope_for(n) == "global")
}

/// The waypoints of one memory as nodes and child edges. `prefix` is `p` or `g`.
fn memory_nodes(m: &Memory, prefix: &str, scope: Scope, fallback_time: u64, out: &mut Vec<Node>, edges: &mut Vec<Edge>) {
    let uses = read_json(&m.dir.join("uses.json"));
    let mut ids = vec![String::new()];
    ids.extend(m.waypoints());
    for id in ids {
        let notes = m.notes_at(&id);
        let kids = m.kids(&id);
        if id.is_empty() && notes.is_empty() && kids.is_empty() {
            continue;
        }
        let file = notes_file(&m.dir, &id);
        let meta = fs::metadata(&file).ok();
        let changed = meta.as_ref().and_then(|x| x.modified().ok()).map(secs).unwrap_or(fallback_time);
        let added = meta.as_ref().and_then(|x| x.created().ok()).map(secs).unwrap_or(changed).min(changed);
        let node_id = format!("{}:{}", prefix, id);
        out.push(Node {
            title: m.title(&id),
            kind: if is_rule(&id, &notes) { Kind::Rule } else { Kind::Note },
            scope,
            level: 3,
            uses: uses[&id].as_u64().unwrap_or(0),
            added,
            changed,
            text: cut(&notes.join("\n"), TEXT_CAP),
            path: file.file_name().map(|n| format!("{}/notes/{}", if scope == Scope::Global { "global-memory" } else { "memory" }, n.to_string_lossy())).unwrap_or_default(),
            id: node_id.clone(),
        });
        for kid in kids {
            edges.push(Edge { from: node_id.clone(), to: format!("{}:{}", prefix, kid), kind: EdgeKind::Child });
        }
    }
}

/// The newest entries of every cluster log, each with its line number as the log counts it.
fn cluster_nodes(ctx: &Ctx) -> Vec<Node> {
    let dir = ctx.root.join("clusters");
    let Ok(read) = fs::read_dir(&dir) else { return Vec::new() };
    let mut names: Vec<String> = read.flatten().filter(|e| e.path().is_dir()).map(|e| e.file_name().to_string_lossy().to_string()).collect();
    names.sort();
    let mut out = Vec::new();
    for name in names {
        let Ok(bytes) = fs::read(dir.join(&name).join("memory.log")) else { continue };
        let body = String::from_utf8_lossy(&bytes).into_owned();
        let lines: Vec<(usize, &str)> = body.lines().enumerate().collect();
        let from = lines.len().saturating_sub(LOG_TAIL);
        for (i, line) in &lines[from..] {
            let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
            let Some(text) = v["text"].as_str() else { continue };
            let field = |key: &str| text.lines().find_map(|l| l.strip_prefix(format!("{}: ", key).as_str())).map(str::to_string);
            let title = field("ENT").filter(|t| !t.trim().is_empty()).unwrap_or_else(|| text.to_string());
            let t = v["t"].as_u64().unwrap_or(0);
            out.push(Node {
                id: format!("c:{}:{}", name, i + 1),
                title: one_line(&title, 60),
                kind: Kind::ClusterEntry,
                scope: Scope::Cluster,
                level: v["L"].as_u64().unwrap_or(3).min(3) as u8, // a line without L: unknown origin, treated as a person's
                uses: 0,
                added: t,
                changed: t,
                text: cut(text, TEXT_CAP),
                path: format!("clusters/{}/memory.log:{}", name, i + 1),
            });
        }
    }
    out
}

impl Graph {
    pub fn build(ctx: &Ctx) -> Graph {
        Graph::build_with(ctx, &Opts::standard())
    }

    pub fn build_with(ctx: &Ctx, opts: &Opts) -> Graph {
        let mut nodes: Vec<Node> = Vec::new();
        let mut edges: Vec<Edge> = Vec::new();
        let project = Memory::open(&ctx.root);
        let shared = crate::memory::shared(ctx);
        let stamp = |dir: &Path| fs::metadata(dir.join("map.json")).and_then(|m| m.modified()).map(secs).unwrap_or(opts.now);
        memory_nodes(&project, "p", Scope::Project, stamp(&project.dir), &mut nodes, &mut edges);
        memory_nodes(&shared, "g", Scope::Global, stamp(&shared.dir), &mut nodes, &mut edges);
        let mut entries = cluster_nodes(ctx);
        entries.sort_by(|a, b| b.changed.cmp(&a.changed).then_with(|| a.id.cmp(&b.id)));
        let total = nodes.len() + entries.len();
        let room = opts.max_nodes.saturating_sub(nodes.len());
        let truncated_entries = entries.len() > room;
        entries.truncate(room);
        nodes.truncate(opts.max_nodes);
        nodes.extend(entries);
        let mut graph = Graph { nodes, edges, total, truncated: total > opts.max_nodes || truncated_entries, ..Graph::default() };
        graph.reindex();
        // A child edge to a waypoint the cap cut would point nowhere.
        let known: HashSet<&str> = graph.nodes.iter().map(|n| n.id.as_str()).collect();
        graph.edges.retain(|e| known.contains(e.to.as_str()) && known.contains(e.from.as_str()));
        graph.link_text(opts.max_nodes);
        graph.reindex();
        graph
    }

    /// Cross-links from the text of the nodes: `[[name]]`, ids, titles, file paths and URLs.
    fn link_text(&mut self, max_nodes: usize) {
        let p = Patterns::new();
        let primary: Vec<usize> = (0..self.nodes.len()).collect();
        // names that point at a waypoint: the id without its prefix, and the vault form with " · "
        let mut by_name: HashMap<String, Vec<usize>> = HashMap::new();
        let mut by_title: HashMap<String, Vec<usize>> = HashMap::new();
        for &i in &primary {
            let n = &self.nodes[i];
            if n.kind == Kind::ClusterEntry {
                by_title.entry(n.title.to_lowercase()).or_default().push(i);
                continue;
            }
            let bare = n.id.split_once(':').map(|(_, r)| r).unwrap_or("").to_lowercase();
            if bare.is_empty() {
                continue; // the start of a memory is no pointer target
            }
            by_name.entry(bare.clone()).or_default().push(i);
            by_name.entry(bare.replace('/', " · ")).or_default().push(i);
            by_title.entry(n.title.to_lowercase()).or_default().push(i);
        }
        let mut seen: HashSet<(String, String, u8)> = HashSet::new();
        let mut extra_nodes: Vec<Node> = Vec::new();
        let mut extra_edges: Vec<Edge> = Vec::new();
        let mut derived: HashMap<String, usize> = HashMap::new();
        // per derived node: (highest level, newest change, oldest change) over the nodes below level 3 that name it,
        // and the newest change over the person nodes that name it
        let mut open: Vec<Option<(u8, u64, u64)>> = Vec::new();
        let mut person_changed: Vec<u64> = Vec::new();
        let derived_cap = (max_nodes / 2).max(1);
        let edge_cap = max_nodes.saturating_mul(4).max(100);
        for &i in &primary {
            let (id, text, scope, changed, level) = {
                let n = &self.nodes[i];
                (n.id.clone(), n.text.clone(), n.scope, n.changed, n.level)
            };
            if text.is_empty() {
                continue;
            }
            let mut push = |to: String, kind: EdgeKind, extra_edges: &mut Vec<Edge>| {
                if to != id && extra_edges.len() < edge_cap && seen.insert((id.clone(), to.clone(), kind as u8)) {
                    extra_edges.push(Edge { from: id.clone(), to, kind });
                }
            };
            // [[name]] and the plain words of a name
            for c in p.wiki.captures_iter(&text) {
                let raw = c[1].split('|').next().unwrap_or("").split('#').next().unwrap_or("").trim().to_lowercase();
                let raw = raw.replace(" · ", "/");
                let hit = by_name.get(&raw).or_else(|| by_name.get(&raw.replace('/', " · "))).or_else(|| by_title.get(&raw));
                if let Some(list) = hit {
                    // the same scope first, so a wiki link in a project note finds the project waypoint
                    let pick = list.iter().find(|&&j| self.nodes[j].scope == scope).or_else(|| list.first());
                    if let Some(&j) = pick {
                        push(self.nodes[j].id.clone(), EdgeKind::Mention, &mut extra_edges);
                    }
                }
            }
            for t in p.token.find_iter(&text) {
                let token = t.as_str().trim_end_matches(|c| "./-".contains(c)).to_lowercase();
                let long_id = token.contains('/') && token.len() >= 5;
                if let Some(list) = by_name.get(&token).filter(|_| long_id) {
                    let pick = list.iter().find(|&&j| self.nodes[j].scope == scope).or_else(|| list.first());
                    if let Some(&j) = pick {
                        push(self.nodes[j].id.clone(), EdgeKind::Mention, &mut extra_edges);
                    }
                } else if token.chars().count() >= 5 && !token.contains('/') {
                    // a title counts when it names exactly one waypoint: "errors" in two branches is not a pointer
                    if let Some(list) = by_title.get(&token).filter(|l| l.len() == 1 && self.nodes[l[0]].kind != Kind::ClusterEntry) {
                        push(self.nodes[list[0]].id.clone(), EdgeKind::Mention, &mut extra_edges);
                    }
                }
            }
            for (kind, edge, prefix, values) in [(Kind::File, EdgeKind::File, "f", file_paths(&p, &text)), (Kind::Link, EdgeKind::Link, "u", urls(&p, &text))] {
                for v in values {
                    let key = format!("{}:{}", prefix, v);
                    let at = match derived.get(&key) {
                        Some(&at) => at,
                        None if derived.len() < derived_cap => {
                            extra_nodes.push(Node {
                                id: key.clone(), title: if kind == Kind::File { v.rsplit('/').next().unwrap_or(&v).to_string() } else { cut(v.trim_start_matches("https://").trim_start_matches("http://"), 60) },
                                kind, scope: Scope::Derived, level: 3, uses: 0, added: 0, changed: 0,
                                text: String::new(), path: v.clone(),
                            });
                            derived.insert(key.clone(), extra_nodes.len() - 1);
                            open.push(None);
                            person_changed.push(0);
                            extra_nodes.len() - 1
                        }
                        None => {
                            self.truncated = true;
                            continue;
                        }
                    };
                    if level < 3 {
                        let o = open[at].get_or_insert((0, 0, u64::MAX));
                        *o = (o.0.max(level), o.1.max(changed), o.2.min(changed));
                    } else {
                        person_changed[at] = person_changed[at].max(changed);
                    }
                    push(key, edge, &mut extra_edges);
                }
            }
        }
        // A derived node has the highest level of the nodes below 3 that name it and their times; only if person nodes
        // alone name it, it is level 3 and takes their time, so it never reaches an export without them.
        for (n, (o, p)) in extra_nodes.iter_mut().zip(open.iter().zip(person_changed.iter())) {
            match o {
                Some((level, newest, oldest)) => {
                    n.level = *level;
                    n.changed = *newest;
                    n.added = *oldest;
                }
                None => {
                    n.level = 3;
                    n.changed = *p;
                    n.added = *p;
                }
            }
        }
        self.nodes.extend(extra_nodes);
        self.edges.extend(extra_edges);
    }

    fn reindex(&mut self) {
        self.index = self.nodes.iter().enumerate().map(|(i, n)| (n.id.clone(), i)).collect();
        self.adjacent = vec![Vec::new(); self.nodes.len()];
        for (e, edge) in self.edges.iter().enumerate() {
            for end in [&edge.from, &edge.to] {
                if let Some(&i) = self.index.get(end) {
                    self.adjacent[i].push(e);
                }
            }
        }
    }

    pub fn get(&self, id: &str) -> Option<&Node> {
        self.index.get(id).map(|&i| &self.nodes[i])
    }

    /// Edges in and out of a node, outgoing first, each group by edge kind and title.
    pub fn neighbours(&self, id: &str) -> Vec<Neighbour> {
        let Some(&at) = self.index.get(id) else { return Vec::new() };
        let mut out: Vec<Neighbour> = self.adjacent[at].iter().filter_map(|&e| {
            let edge = &self.edges[e];
            if edge.from == id { Some(Neighbour { id: edge.to.clone(), outgoing: true, kind: edge.kind }) }
            else if edge.to == id { Some(Neighbour { id: edge.from.clone(), outgoing: false, kind: edge.kind }) }
            else { None }
        }).collect();
        let title = |n: &Neighbour| self.get(&n.id).map(|x| x.title.to_lowercase()).unwrap_or_default();
        out.sort_by(|a, b| b.outgoing.cmp(&a.outgoing).then_with(|| (a.kind as u8).cmp(&(b.kind as u8))).then_with(|| title(a).cmp(&title(b))).then_with(|| a.id.cmp(&b.id)));
        out
    }

    /// One page of the nodes whose title, id, kind or text hold every word of the query. Newest change first, a title
    /// hit before a text hit. Returns the page and how many nodes matched in all.
    pub fn page(&self, query: &str, offset: usize, limit: usize) -> (Vec<&Node>, usize) {
        let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        let mut hits: Vec<(&Node, bool)> = self.nodes.iter().filter_map(|n| {
            let head = format!("{} {} {}", n.title, n.id, n.kind.name()).to_lowercase();
            if words.iter().all(|w| head.contains(w.as_str())) { return Some((n, true)); }
            let body = n.text.to_lowercase();
            if words.iter().all(|w| head.contains(w.as_str()) || body.contains(w.as_str())) { Some((n, false)) } else { None }
        }).collect();
        hits.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| b.0.changed.cmp(&a.0.changed)).then_with(|| a.0.id.cmp(&b.0.id)));
        let total = hits.len();
        (hits.into_iter().skip(offset).take(limit).map(|(n, _)| n).collect(), total)
    }

    pub fn stats(&self, now: u64) -> Stats {
        let mut by_kind: BTreeMap<&'static str, usize> = BTreeMap::new();
        for n in &self.nodes {
            *by_kind.entry(n.kind.name()).or_default() += 1;
        }
        let since = now.saturating_sub(7 * DAY);
        let added_7d = self.nodes.iter().filter(|n| n.added >= since && matches!(n.kind, Kind::Note | Kind::Rule | Kind::ClusterEntry)).count();
        Stats { nodes: self.nodes.len(), edges: self.edges.len(), by_kind, added_7d, truncated: self.truncated }
    }

    /// The graph without the nodes of level 3 (a person), without the edges that touch them and without the file and
    /// link nodes that only they named.
    pub fn without_person(&self) -> Graph {
        let keep = |n: &Node| n.level < 3;
        let kept: HashSet<&str> = self.nodes.iter().filter(|n| keep(n)).map(|n| n.id.as_str()).collect();
        let edges: Vec<Edge> = self.edges.iter().filter(|e| kept.contains(e.from.as_str()) && kept.contains(e.to.as_str())).cloned().collect();
        let linked: HashSet<&str> = edges.iter().flat_map(|e| [e.from.as_str(), e.to.as_str()]).collect();
        let nodes: Vec<Node> = self.nodes.iter().filter(|n| keep(n) && (n.scope != Scope::Derived || linked.contains(n.id.as_str()))).cloned().collect();
        let ids: HashSet<&str> = nodes.iter().map(|n| n.id.as_str()).collect();
        let edges: Vec<Edge> = edges.into_iter().filter(|e| ids.contains(e.from.as_str()) && ids.contains(e.to.as_str())).collect();
        let mut g = Graph { nodes, edges, total: self.total, truncated: self.truncated, ..Graph::default() };
        g.reindex();
        g
    }

    pub fn to_json(&self, now: u64) -> Value {
        let s = self.stats(now);
        json!({
            "penelope_graph": 1,
            "generated": now,
            "counts": {"nodes": s.nodes, "edges": s.edges, "by_kind": s.by_kind, "added_7d": s.added_7d},
            "truncated": self.truncated,
            "nodes": self.nodes.iter().map(|n| json!({
                "id": n.id, "title": n.title, "kind": n.kind.name(), "scope": n.scope.name(), "level": n.level, "uses": n.uses,
                "added": n.added, "changed": n.changed, "path": n.path, "text": n.text})).collect::<Vec<_>>(),
            "edges": self.edges.iter().map(|e| json!({"from": e.from, "to": e.to, "kind": e.kind.name()})).collect::<Vec<_>>(),
        })
    }

    /// The pointers for a query, best first, at most `limit`. Per query word the best place counts: the title 3, the
    /// id or path 2, a word of the text 1, a word of the text with the same first 5 letters 0.5. The score is that sum
    /// over 3 times the number of words, so 1.0 means every word is in a title. Below `FIND_THRESHOLD` nothing is
    /// returned. Level 3 nodes are left out when `person` is false.
    pub fn find(&self, query: &str, limit: usize, person: bool) -> Vec<Pointer> {
        let p = Patterns::new();
        let lower = query.to_lowercase();
        let words: Vec<String> = p.word.find_iter(&lower).map(|m| m.as_str().to_string())
            .filter(|w| w.chars().count() > 1 && !["the", "and", "for", "with", "der", "die", "das", "und", "was", "wie", "how", "what"].contains(&w.as_str())).collect();
        if words.is_empty() {
            return Vec::new();
        }
        let stem = |w: &str| w.chars().take(5).collect::<String>();
        let mut scored: Vec<(f64, &Node, String)> = Vec::new();
        for n in &self.nodes {
            if !person && n.level >= 3 {
                continue;
            }
            let title: Vec<String> = p.word.find_iter(&n.title.to_lowercase()).map(|m| m.as_str().to_string()).collect();
            let id = n.id.to_lowercase();
            let body = n.text.to_lowercase();
            let body_words: Vec<String> = p.word.find_iter(&body).map(|m| m.as_str().to_string()).collect();
            let mut sum = 0.0;
            for w in &words {
                sum += if title.contains(w) { 3.0 }
                    else if id.split(|c: char| !c.is_alphanumeric()).any(|s| s == w) || (w.len() > 3 && n.path.to_lowercase().contains(w.as_str())) { 2.0 }
                    else if body_words.contains(w) { 1.0 }
                    else if w.chars().count() >= 5 && body_words.iter().any(|b| stem(b) == stem(w)) { 0.5 }
                    else { 0.0 };
            }
            let score = sum / (3.0 * words.len() as f64);
            if score >= FIND_THRESHOLD {
                // the line that holds most words of the query
                let line = n.text.lines().max_by_key(|l| { let l = l.to_lowercase(); words.iter().filter(|w| l.contains(w.as_str())).count() }).unwrap_or("").to_string();
                scored.push((score, n, one_line(&line, 120)));
            }
        }
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal).then_with(|| b.1.uses.cmp(&a.1.uses))
            .then_with(|| b.1.changed.cmp(&a.1.changed)).then_with(|| a.1.id.cmp(&b.1.id)));
        scored.into_iter().take(limit).map(|(score, n, line)| Pointer {
            id: n.id.clone(), title: n.title.clone(), line, path: n.path.clone(), level: n.level, scope: n.scope.name(), kind: n.kind.name(),
            score: (score * 100.0).round() / 100.0,
        }).collect()
    }
}

/// How a model reads the node a pointer names.
pub fn read_hint(id: &str) -> String {
    match id.split_once(':') {
        Some(("c", rest)) => format!("memory_get {}", rest),
        Some(("p", rest)) | Some(("g", rest)) if !rest.is_empty() => format!("memory_go \"{}\"", rest.replace('/', " ")),
        _ => String::new(),
    }
}

type Cached = (Vec<(PathBuf, u128, u64)>, std::time::Instant, std::sync::Arc<Graph>);

fn stamp(path: &Path) -> (PathBuf, u128, u64) {
    let m = fs::metadata(path).ok();
    let t = m.as_ref().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_nanos()).unwrap_or(0);
    (path.to_path_buf(), t, m.map(|m| m.len()).unwrap_or(0))
}

/// The files whose change makes a built graph stale: the map and the use counts of both memories (every note writes the
/// map) and every cluster log.
fn watched(ctx: &Ctx) -> Vec<(PathBuf, u128, u64)> {
    let shared = match ctx.cfg["global_memory"].as_str().filter(|p| !p.trim().is_empty()) {
        Some(p) => PathBuf::from(p),
        None => crate::memory::global_dir(),
    };
    let mut out = Vec::new();
    for dir in [ctx.root.join("memory"), shared] {
        out.push(stamp(&dir.join("map.json")));
        out.push(stamp(&dir.join("uses.json")));
    }
    let clusters = ctx.root.join("clusters");
    out.push(stamp(&clusters));
    if let Ok(read) = fs::read_dir(&clusters) {
        let mut logs: Vec<PathBuf> = read.flatten().map(|e| e.path().join("memory.log")).collect();
        logs.sort();
        out.extend(logs.iter().map(|l| stamp(l)));
    }
    out
}

/// The graph for `ctx`, built again only when a watched file changed or 30 seconds passed. A model that calls
/// `memory_find` several times in a turn does not read every note each time.
pub fn cached(ctx: &Ctx) -> std::sync::Arc<Graph> {
    use std::sync::{Arc, Mutex, OnceLock};
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Cached>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let signature = watched(ctx);
    if let Ok(map) = cache.lock() {
        if let Some((old, at, graph)) = map.get(&ctx.root) {
            if *old == signature && at.elapsed().as_secs() < 30 {
                return Arc::clone(graph);
            }
        }
    }
    let graph = Arc::new(Graph::build(ctx));
    if let Ok(mut map) = cache.lock() {
        if map.len() >= 8 {
            map.clear();
        }
        map.insert(ctx.root.clone(), (signature, std::time::Instant::now(), Arc::clone(&graph)));
    }
    graph
}

/// The text of the built-in `memory_find` tool.
pub fn find_text(ctx: &Ctx, query: &str, limit: usize) -> String {
    let graph = cached(ctx);
    let pointers = graph.find(query, limit.clamp(1, 20), true);
    if pointers.is_empty() {
        return format!("unknown: nothing stored about \"{}\"; note it after the session with memory_add or /note", one_line(query, 80));
    }
    pointers.iter().enumerate().map(|(i, p)| {
        format!("{}. {} · {} · \"{}\" · {} · L{} {} · score {:.2}{}", i + 1, p.id, p.title, p.line, p.path, p.level, p.scope, p.score,
                match read_hint(&p.id) { h if h.is_empty() => String::new(), h => format!(" · read: {}", h) })
    }).collect::<Vec<_>>().join("\n")
}

// ---------------------------------------------------------------- export

fn safe_name(raw: &str) -> String {
    let mut out = String::new();
    for c in raw.chars() {
        let ok = c.is_alphanumeric() || c == ' ' || c == '-' || c == '_' || c == '.' || c == '·';
        out.push(if ok { c } else { '-' });
    }
    let out = out.trim_matches(|c: char| c == '.' || c == ' ' || c == '-').to_string();
    let out: String = out.chars().take(90).collect();
    let out = out.trim_end_matches(['.', ' ']).to_string();
    if out.is_empty() { "node".to_string() } else { out }
}

fn vault_name(n: &Node) -> String {
    let bare = n.id.split_once(':').map(|(_, r)| r).unwrap_or(&n.id);
    let rest = match n.kind {
        Kind::File | Kind::Link => n.path.replace("https://", "").replace("http://", "").replace('/', " · "),
        Kind::ClusterEntry => format!("{} {}", bare.replace(':', "-"), n.title),
        _ => if bare.is_empty() { "start".to_string() } else { bare.replace('/', " · ") },
    };
    let head = match n.kind { Kind::File => "file", Kind::Link => "link", _ => n.scope.name() };
    safe_name(&format!("{} · {}", head, rest))
}

const MARKER: &str = "penelope_node:";

/// Writes one Markdown file per node into `dir`. A file that exists and was not written by this export stays as it is.
/// Returns (written, kept).
pub fn write_vault(graph: &Graph, dir: &Path, now: u64) -> Result<(usize, usize), String> {
    fs::create_dir_all(dir).map_err(|e| format!("NOT exported. cannot create {}: {}", dir.display(), e))?;
    let dir = dir.canonicalize().map_err(|e| format!("NOT exported. {}: {}", dir.display(), e))?;
    let mut names: HashMap<String, String> = HashMap::new();
    let mut taken: HashSet<String> = HashSet::new();
    taken.insert("penelope · graph".to_lowercase());
    for n in &graph.nodes {
        let base = vault_name(n);
        let mut name = base.clone();
        let mut k = 2;
        while !taken.insert(name.to_lowercase()) {
            name = format!("{} ({})", base, k);
            k += 1;
        }
        names.insert(n.id.clone(), name);
    }
    let (mut written, mut kept) = (0, 0);
    let mut put = |name: &str, body: String| -> Result<(), String> {
        let file = dir.join(format!("{}.md", name));
        if file.parent() != Some(dir.as_path()) {
            return Err(format!("NOT exported. {} would leave {}", name, dir.display()));
        }
        if let Ok(meta) = fs::symlink_metadata(&file) {
            let ours = meta.is_file() && fs::read_to_string(&file).map(|b| b.starts_with("---\npenelope")).unwrap_or(false);
            if !ours {
                kept += 1;
                return Ok(());
            }
        }
        fs::write(&file, body).map_err(|e| format!("NOT exported. cannot write {}: {}", file.display(), e))?;
        written += 1;
        Ok(())
    };
    for n in &graph.nodes {
        let name = &names[&n.id];
        let mut body = format!("---\n{} {}\nkind: {}\nscope: {}\nlevel: {}\nuses: {}\nchanged: {}\npath: {}\n---\n\n# {}\n\n",
                               MARKER, json!(n.id), n.kind.name(), n.scope.name(), n.level, n.uses, n.changed, json!(n.path), n.title.replace('\n', " "));
        if !n.text.is_empty() {
            for line in n.text.lines() {
                body.push_str(&format!("- {}\n", line.trim()));
            }
            body.push('\n');
        } else if matches!(n.kind, Kind::File | Kind::Link) {
            body.push_str(&format!("`{}`\n\n", n.path));
        }
        let out: Vec<String> = graph.neighbours(&n.id).into_iter().filter(|x| x.outgoing).filter_map(|x| names.get(&x.id).map(|t| format!("- {}: [[{}]]", x.kind.name(), t))).collect();
        let back: Vec<String> = graph.neighbours(&n.id).into_iter().filter(|x| !x.outgoing).filter_map(|x| names.get(&x.id).map(|t| format!("- {}: [[{}]]", x.kind.name(), t))).collect();
        if !out.is_empty() {
            body.push_str(&format!("## Links\n\n{}\n\n", out.join("\n")));
        }
        if !back.is_empty() {
            body.push_str(&format!("## Linked from\n\n{}\n", back.join("\n")));
        }
        put(name, body)?;
    }
    let s = graph.stats(now);
    let kinds: Vec<String> = s.by_kind.iter().map(|(k, v)| format!("{} {}", v, k)).collect();
    put("penelope · graph", format!("---\npenelope_graph: 1\n---\n\n# Penelope graph\n\n{} nodes, {} edges ({}). {} new in the last 7 days.\n", s.nodes, s.edges, kinds.join(", "), s.added_7d))?;
    Ok((written, kept))
}

/// `penelope memory export --format json|obsidian [--include-person] PATH`.
pub fn export_cmd(ctx: &Ctx, args: &[String]) -> Result<String, String> {
    let (mut format, mut person, mut path) = (String::new(), false, None::<String>);
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--format" if i + 1 < args.len() => {
                format = args[i + 1].clone();
                i += 2;
            }
            "--include-person" => {
                person = true;
                i += 1;
            }
            other if other.starts_with("--") => return Err(format!("penelope memory export: unknown option {} (known: --format json|obsidian, --include-person)", other)),
            other if path.is_none() => {
                path = Some(other.to_string());
                i += 1;
            }
            other => return Err(format!("penelope memory export: one PATH only, got a second: {}", other)),
        }
    }
    let Some(path) = path else { return Err("penelope memory export --format json|obsidian [--include-person] PATH".to_string()) };
    let now = now();
    let full = Graph::build(ctx);
    let graph = if person { full } else { full.without_person() };
    let note = if person { "level 3 (person) nodes included".to_string() } else { "level 3 (person) nodes left out, add --include-person to export them".to_string() };
    match format.as_str() {
        "json" => {
            let text = serde_json::to_string_pretty(&graph.to_json(now)).unwrap_or_default();
            if path == "-" {
                return Ok(text);
            }
            let mut target = PathBuf::from(&path);
            if target.is_dir() {
                target = target.join("memory-graph.json");
            }
            if target.exists() {
                let ours = fs::read(&target).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()).map(|v| v.get("penelope_graph").is_some()).unwrap_or(false);
                if !ours {
                    return Err(format!("NOT exported. {} exists and is not a Penelope graph file", target.display()));
                }
            }
            if let Some(parent) = target.parent().filter(|p| !p.as_os_str().is_empty()) {
                fs::create_dir_all(parent).map_err(|e| format!("NOT exported. {}: {}", parent.display(), e))?;
            }
            fs::write(&target, text).map_err(|e| format!("NOT exported. cannot write {}: {}", target.display(), e))?;
            Ok(format!("{} nodes and {} edges written to {} · {}", graph.nodes.len(), graph.edges.len(), target.display(), note))
        }
        "obsidian" => {
            let (written, kept) = write_vault(&graph, Path::new(&path), now)?;
            Ok(format!("{} files written to {}{} · {}", written, path, if kept > 0 { format!(", {} existing files left as they were", kept) } else { String::new() }, note))
        }
        _ => Err("penelope memory export: --format must be json or obsidian".to_string()),
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    pub fn fixture(tag: &str) -> (PathBuf, Ctx) {
        let dir = std::env::temp_dir().join(format!("penelope-graph-{}-{}", tag, std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let mut ctx = crate::config::load(&dir.join(".penelope"));
        ctx.cfg["global_memory"] = json!(dir.join("global").to_string_lossy());
        (dir, ctx)
    }

    /// Project notes with cross-links, one global rule and two cluster logs.
    pub fn filled(tag: &str) -> (PathBuf, Ctx) {
        let (dir, ctx) = fixture(tag);
        let mut m = Memory::open(&ctx.root);
        m.note("rust/errors", "Use anyhow in the binary. See [[rust/build]] and src/main.rs.");
        m.note("rust/build", "cargo test --release is the fast check. Docs: https://doc.rust-lang.org/cargo/ and src/main.rs.");
        m.note("git/commits", "One change per commit. The notes in rust/errors say how to report failures.");
        m.mark_used(&["rust/errors".to_string()]);
        let mut g = crate::memory::shared(&ctx);
        g.note("rules/style", "Always write plain English. Never use em dashes.");
        let log = ctx.root.join("clusters").join("c1-1");
        fs::create_dir_all(&log).unwrap();
        let t = now();
        fs::write(log.join("memory.log"), format!(
            "{}\n{}\nnot json\n",
            json!({"t": t, "C": "c1-1", "L": 1, "text": "ENT: parser\nATT: owner\nVAL: Anna uses [[rust/errors]]\nSRC: mcp"}),
            json!({"t": t - 30 * DAY, "C": "c1-1", "L": 2, "text": "worker finished the backend in docs/PLAN.md"}))).unwrap();
        (dir, ctx)
    }

    fn edge(g: &Graph, from: &str, to: &str, kind: EdgeKind) -> bool {
        g.edges.iter().any(|e| e.from == from && e.to == to && e.kind == kind)
    }

    #[test]
    fn the_graph_has_nodes_of_every_kind_and_the_four_edge_kinds() {
        let (dir, ctx) = filled("build");
        let g = Graph::build(&ctx);
        let kinds = |k: Kind| g.nodes.iter().filter(|n| n.kind == k).count();
        assert_eq!(kinds(Kind::ClusterEntry), 2, "the line that is not JSON is skipped");
        assert_eq!(kinds(Kind::Rule), 1);
        assert_eq!(g.get("g:rules/style").unwrap().scope, Scope::Global);
        assert!(kinds(Kind::Note) >= 5, "rust, errors, build, git, commits: {}", kinds(Kind::Note));
        assert_eq!(kinds(Kind::File), 2, "src/main.rs once for two notes, docs/PLAN.md");
        assert_eq!(kinds(Kind::Link), 1);
        // child
        assert!(edge(&g, "p:rust", "p:rust/errors", EdgeKind::Child));
        // [[rust/build]] in errors, the id of a waypoint written in plain text, a wiki link from a cluster entry
        assert!(edge(&g, "p:rust/errors", "p:rust/build", EdgeKind::Mention));
        assert!(edge(&g, "p:git/commits", "p:rust/errors", EdgeKind::Mention));
        assert!(edge(&g, "c:c1-1:1", "p:rust/errors", EdgeKind::Mention));
        // two notes share one file node
        assert!(edge(&g, "p:rust/errors", "f:src/main.rs", EdgeKind::File) && edge(&g, "p:rust/build", "f:src/main.rs", EdgeKind::File));
        assert!(edge(&g, "c:c1-1:2", "f:docs/PLAN.md", EdgeKind::File));
        assert!(edge(&g, "p:rust/build", "u:https://doc.rust-lang.org/cargo/", EdgeKind::Link));
        assert_eq!(g.get("p:rust/errors").unwrap().uses, 1);
        assert_eq!(g.get("c:c1-1:1").unwrap().title, "parser");
        assert_eq!(g.get("c:c1-1:1").unwrap().level, 1);
        let n = g.neighbours("f:src/main.rs");
        assert_eq!(n.len(), 2);
        assert!(n.iter().all(|x| !x.outgoing));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn counts_growth_and_the_cap() {
        let (dir, ctx) = filled("stats");
        let g = Graph::build(&ctx);
        let s = g.stats(now());
        assert_eq!(s.nodes, g.nodes.len());
        assert_eq!(s.by_kind["cluster-entry"], 2);
        // the new entry and the waypoints written today count, the entry of 30 days ago does not
        assert!(g.nodes.iter().any(|n| n.id == "c:c1-1:2" && n.added < now() - 7 * DAY));
        assert_eq!(s.added_7d, g.nodes.iter().filter(|n| matches!(n.kind, Kind::Note | Kind::Rule | Kind::ClusterEntry)).count() - 1);
        assert!(!s.truncated);
        let small = Graph::build_with(&ctx, &Opts { max_nodes: 4, now: now() });
        assert!(small.truncated && small.total > 4);
        assert!(small.nodes.iter().filter(|n| n.scope != Scope::Derived).count() <= 4);
        let known: HashSet<&str> = small.nodes.iter().map(|n| n.id.as_str()).collect();
        assert!(small.edges.iter().all(|e| known.contains(e.from.as_str()) && known.contains(e.to.as_str())), "no edge points at a cut node");
        let (page, total) = g.page("", 3, 4);
        assert_eq!(page.len(), 4);
        assert_eq!(total, g.nodes.len());
        let (hits, found) = g.page("anyhow", 0, 10);
        assert_eq!((hits.len(), found), (1, 1));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn json_export_has_nodes_and_edges_and_leaves_the_person_out() {
        let (dir, ctx) = filled("json");
        let out = dir.join("out").join("graph.json");
        let report = export_cmd(&ctx, &["--format".into(), "json".into(), out.to_string_lossy().to_string()]).unwrap();
        assert!(report.contains("--include-person"), "{}", report);
        let v: Value = serde_json::from_str(&fs::read_to_string(&out).unwrap()).unwrap();
        assert_eq!(v["penelope_graph"], 1);
        let nodes = v["nodes"].as_array().unwrap();
        assert!(nodes.iter().all(|n| n["level"].as_u64().unwrap() < 3), "no person node by default");
        assert!(nodes.iter().any(|n| n["id"] == "c:c1-1:1") && nodes.iter().any(|n| n["id"] == "f:docs/PLAN.md"));
        assert!(!nodes.iter().any(|n| n["id"] == "f:src/main.rs"), "a file that only person nodes name goes with them");
        for key in ["id", "title", "kind", "scope", "level", "uses", "added", "changed", "path", "text"] {
            assert!(nodes[0].get(key).is_some(), "{}", key);
        }
        let ids: HashSet<&str> = nodes.iter().map(|n| n["id"].as_str().unwrap()).collect();
        assert!(v["edges"].as_array().unwrap().iter().all(|e| ids.contains(e["from"].as_str().unwrap()) && ids.contains(e["to"].as_str().unwrap())));
        assert_eq!(v["counts"]["nodes"].as_u64().unwrap() as usize, nodes.len());
        // with the flag the notes come along
        let all = dir.join("all.json");
        export_cmd(&ctx, &["--format".into(), "json".into(), "--include-person".into(), all.to_string_lossy().to_string()]).unwrap();
        let v: Value = serde_json::from_str(&fs::read_to_string(&all).unwrap()).unwrap();
        assert!(v["nodes"].as_array().unwrap().iter().any(|n| n["id"] == "p:rust/errors" && n["level"] == 3));
        // a file that is not a graph file is not overwritten
        let other = dir.join("notes.json");
        fs::write(&other, "{\"mine\": true}").unwrap();
        assert!(export_cmd(&ctx, &["--format".into(), "json".into(), other.to_string_lossy().to_string()]).is_err());
        assert_eq!(fs::read_to_string(&other).unwrap(), "{\"mine\": true}");
        assert!(export_cmd(&ctx, &["--format".into(), "xml".into(), "x".into()]).is_err());
        assert!(export_cmd(&ctx, &["--format".into(), "json".into(), "--oops".into(), "x".into()]).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn obsidian_export_stays_inside_the_path() {
        let (dir, ctx) = filled("vault");
        let mut m = Memory::open(&ctx.root);
        m.note("../../evil", "escape attempt with [[rust/errors]]");
        m.note("a/b", "name that is a path: ../../outside.md");
        let vault = dir.join("vault");
        fs::create_dir_all(&vault).unwrap();
        fs::write(vault.join("mine.md"), "my own file").unwrap();
        fs::write(vault.join("project · rust · errors.md"), "not written by Penelope").unwrap();
        let outside = dir.join("outside.md");
        fs::write(&outside, "keep").unwrap();
        let report = export_cmd(&ctx, &["--format".into(), "obsidian".into(), "--include-person".into(), vault.to_string_lossy().to_string()]).unwrap();
        assert!(report.contains("left as they were"), "{}", report);
        assert_eq!(fs::read_to_string(vault.join("mine.md")).unwrap(), "my own file");
        assert_eq!(fs::read_to_string(vault.join("project · rust · errors.md")).unwrap(), "not written by Penelope");
        assert_eq!(fs::read_to_string(&outside).unwrap(), "keep");
        // every file written lies directly in the vault; nothing else appeared next to it
        let mut around: Vec<String> = fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
        around.sort();
        assert!(!around.iter().any(|n| n.contains("evil")), "{:?}", around);
        for e in fs::read_dir(&vault).unwrap().flatten() {
            assert!(e.path().is_file(), "{}", e.path().display());
        }
        let build = fs::read_to_string(vault.join("project · rust · build.md")).unwrap();
        assert!(build.starts_with("---\npenelope_node: \"p:rust/build\"") && build.contains("[[file · src · main.rs]]"), "{}", build);
        let errors_links = fs::read_to_string(vault.join("project · rust · build.md")).unwrap();
        assert!(errors_links.contains("## Linked from") && errors_links.contains("[[project · rust · errors]]"), "{}", errors_links);
        assert!(vault.join("penelope · graph.md").exists());
        // the default export has no person node file
        let plain = dir.join("plain");
        export_cmd(&ctx, &["--format".into(), "obsidian".into(), plain.to_string_lossy().to_string()]).unwrap();
        assert!(!plain.join("project · rust · errors.md").exists());
        assert!(plain.join("cluster · c1-1-1 parser.md").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_cache_follows_the_files_and_derived_nodes_keep_the_level_of_their_sources() {
        let (dir, ctx) = filled("cache");
        let a = cached(&ctx);
        assert!(std::sync::Arc::ptr_eq(&a, &cached(&ctx)), "no change: the same graph");
        let mut m = Memory::open(&ctx.root);
        m.note("work/clients", "contract at ~/clients/x.pdf and docs/PLAN.md");
        let b = cached(&ctx);
        assert!(!std::sync::Arc::ptr_eq(&a, &b), "a note changed the map");
        // a file only a person's note names has level 3 and is not pointed at without the person flag
        let pdf = b.get("f:~/clients/x.pdf").unwrap();
        assert_eq!(pdf.level, 3);
        assert!(b.find("x.pdf", 5, false).is_empty());
        assert_eq!(b.find("x.pdf", 5, true)[0].id, "f:~/clients/x.pdf");
        assert!(!b.without_person().nodes.iter().any(|n| n.id == "f:~/clients/x.pdf"));
        // a file a person's note and a cluster entry both name keeps the entry's level and its time, not the note's
        let plan = b.get("f:docs/PLAN.md").unwrap();
        assert_eq!(plan.level, 2);
        assert_eq!(plan.changed, b.get("c:c1-1:2").unwrap().changed);
        assert!(b.find("PLAN.md", 5, false).iter().any(|p| p.id == "f:docs/PLAN.md"));
        // a log line without L is a person's until proven otherwise
        let log = ctx.root.join("clusters").join("c1-1").join("memory.log");
        let mut text = fs::read_to_string(&log).unwrap();
        text.push_str(&format!("{}\n", json!({"t": now(), "C": "c1-1", "text": "ENT: secret\nVAL: no level here"})));
        fs::write(&log, text).unwrap();
        let c = cached(&ctx);
        assert_eq!(c.get("c:c1-1:4").unwrap().level, 3);
        assert!(c.find("secret", 5, false).is_empty());
        assert!(!c.without_person().nodes.iter().any(|n| n.id == "c:c1-1:4"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn json_export_refuses_a_binary_file() {
        let (dir, ctx) = filled("binary");
        let file = dir.join("data.json");
        fs::write(&file, [0xffu8, 0xfe, 0x00, 0x01, 0x80]).unwrap();
        assert!(export_cmd(&ctx, &["--format".into(), "json".into(), file.to_string_lossy().to_string()]).is_err());
        assert_eq!(fs::read(&file).unwrap(), vec![0xffu8, 0xfe, 0x00, 0x01, 0x80]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn find_returns_pointers_and_the_unknown_answer() {
        let (dir, ctx) = filled("find");
        let g = Graph::build(&ctx);
        let hits = g.find("anyhow errors", 5, true);
        assert_eq!(hits[0].id, "p:rust/errors");
        assert!(hits[0].line.contains("anyhow") && hits[0].score > 0.5 && hits[0].level == 3, "{} {}", hits[0].line, hits[0].score);
        assert!(hits[0].path.ends_with("rust__errors.md"), "{}", hits[0].path);
        assert!(hits.len() <= 5);
        assert!(g.find("anyhow errors", 5, false).iter().all(|p| p.level < 3));
        assert!(g.find("owner Anna", 5, false).iter().any(|p| p.id == "c:c1-1:1"));
        assert!(g.find("quantum chromodynamics", 5, true).is_empty());
        let text = find_text(&ctx, "anyhow errors", 3);
        assert!(text.starts_with("1. p:rust/errors · errors · ") && text.contains("score ") && text.contains("memory_go \"rust errors\""), "{}", text);
        assert!(text.lines().count() <= 3);
        assert_eq!(find_text(&ctx, "quantum chromodynamics", 3),
                   "unknown: nothing stored about \"quantum chromodynamics\"; note it after the session with memory_add or /note");
        assert!(find_text(&ctx, "   ", 3).starts_with("unknown:"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_paths_and_urls_are_found_without_noise() {
        let p = Patterns::new();
        let found = file_paths(&p, "see src/tui/mod.rs, `Cargo.toml`, v1.2 and e.g. nothing. Also ./docs/A.md and https://x.org/a/b.html.");
        assert_eq!(found, vec!["src/tui/mod.rs", "Cargo.toml", "docs/A.md"]);
        assert_eq!(urls(&p, "go (https://x.org/a/b.html). Then http://y.io."), vec!["https://x.org/a/b.html", "http://y.io"]);
    }
}
