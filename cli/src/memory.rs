//! Persistent memory as a map of waypoints.
//!
//! The map is a tree under `.penelope/memory/`. Every node is a waypoint with a title, signpost words and a note file.
//! A lookup starts at the root and follows the child whose signposts match the request best, one level at a time.
//! It never searches all notes. Every finished walk is stored in `paths.json`: the same request jumps to its
//! node the next time without walking.

use serde_json::{json, Map, Value};
use std::fs;
use std::path::{Path, PathBuf};

pub struct Memory {
    pub dir: PathBuf,
    map: Value,
    paths: Value,
}

/// The memory every project of this PC shares: rules and facts that outlive one session. It lies outside the project
/// folder, so a session in another repository reads the same notes.
pub fn global_dir() -> PathBuf {
    let home = crate::platform::home();
    let dir = home.join(".penelope").join("global-memory");
    let _ = fs::create_dir_all(&dir);
    dir
}

/// A memory folder that does not belong to one project.
pub fn open_global() -> Memory {
    Memory::open_at(&global_dir())
}

/// The shared memory of this PC, or the folder named by `global_memory` in the settings. A test and a session with a
/// separate home both stay inside their own folder.
pub fn shared(ctx: &crate::config::Ctx) -> Memory {
    match ctx.cfg["global_memory"].as_str().filter(|p| !p.trim().is_empty()) {
        Some(p) => Memory::open_at(Path::new(p)),
        None => Memory::open_at(&global_dir()),
    }
}

/// Languages and topics that get their own branch when a note is filed without a path.
const BRANCHES: &[(&str, &[&str])] = &[
    ("rust", &["rust", "cargo", "crate", "borrow"]),
    ("python", &["python", "pip", "venv", "pytest", "uv"]),
    ("java", &["java", "maven", "gradle", "spring", "jvm"]),
    ("javascript", &["javascript", "typescript", "node", "npm", "bun", "react", "js", "ts"]),
    ("go", &["golang", "goroutine"]),
    ("cpp", &["c++", "cpp", "cmake"]),
    ("shell", &["bash", "zsh", "shell", "fish"]),
    ("git", &["git", "commit", "branch", "push", "github"]),
    ("web", &["html", "css", "browser", "http", "api"]),
    ("ai", &["llm", "model", "prompt", "token", "lora", "mcp"]),
];

fn words(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    for c in text.chars() {
        if c.is_alphanumeric() || c == '+' || c == '#' {
            cur.extend(c.to_lowercase());
        } else if !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out.retain(|w| w.chars().count() > 1 && !STOP.contains(&w.as_str()));
    out
}

const STOP: &[&str] = &[
    "the", "and", "for", "with", "that", "this", "from", "into", "what", "how", "when", "where", "which", "are", "was", "is",
    "der", "die", "das", "und", "oder", "ein", "eine", "mit", "von", "wie", "was", "wo", "ist", "sind", "fuer", "auf", "im", "in",
    "zu", "den", "dem", "des", "nicht", "auch", "ich", "wir", "soll", "bitte", "to", "of", "on", "at", "it", "be", "do", "we", "my",
];

/// True for notes that read like a lasting rule: words that bind every later session.
pub fn scope_for(text: &str) -> &'static str {
    let t = text.to_lowercase();
    let markers = ["always", "never", "from now on", "by default", "whenever", "prefer", "remember that", "keep in mind",
                   "immer", "nie ", "ab jetzt", "grundsätzlich", "standardmäßig", "bevorzuge", "regel:", "rule:", "convention", "style guide"];
    if markers.iter().any(|m| t.contains(m)) { "global" } else { "project" }
}

fn signature(text: &str) -> String {
    let mut w = words(text);
    w.sort();
    w.dedup();
    w.join(" ")
}

impl Memory {
    pub fn open(root: &Path) -> Memory {
        Memory::open_at(&root.join("memory"))
    }

    pub fn open_at(dir: &Path) -> Memory {
        let dir = dir.to_path_buf();
        let _ = fs::create_dir_all(dir.join("notes"));
        let read = |name: &str, default: Value| -> Value {
            fs::read_to_string(dir.join(name)).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(default)
        };
        let map = read("map.json", json!({"": {"title": "start", "signs": [], "children": []}}));
        let paths = read("paths.json", json!({}));
        Memory { dir, map, paths }
    }

    /// The children of a waypoint, sorted, for the graph and the Obsidian vault.
    pub fn kids(&self, id: &str) -> Vec<String> {
        let mut kids = self.children(id);
        kids.sort();
        kids
    }

    /// The title of a waypoint: the last part of its id.
    pub fn title(&self, id: &str) -> String {
        if id.is_empty() { "start".to_string() } else { id.rsplit('/').next().unwrap_or(id).to_string() }
    }

    /// The note lines of one waypoint, without the markdown dash.
    pub fn notes_at(&self, id: &str) -> Vec<String> {
        fs::read_to_string(self.note_file(id)).map(|b| b.lines().filter_map(|l| l.strip_prefix("- ").map(str::to_string)).collect()).unwrap_or_default()
    }

    /// Every waypoint, parents before children, for the gate of a recall.
    pub fn waypoints(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut stack = vec![String::new()];
        while let Some(id) = stack.pop() {
            for c in self.children(&id).into_iter().rev() {
                stack.push(c);
            }
            if !id.is_empty() {
                out.push(id);
            }
        }
        out
    }

    /// The top of the map as a few lines: what a system prompt needs to know where notes exist. No note text.
    pub fn index(&self, max: usize) -> String {
        let mut rows: Vec<(String, usize)> = Vec::new();
        for id in self.waypoints() {
            if id.matches('/').count() > 1 {
                continue;
            }
            let notes = fs::read_to_string(self.note_file(&id)).map(|b| b.lines().filter(|l| l.starts_with("- ")).count()).unwrap_or(0);
            if notes > 0 {
                rows.push((id, notes));
            }
        }
        rows.sort();
        let mut out = String::new();
        for (id, n) in rows.into_iter().take(max) {
            out.push_str(&format!("{} ({} notes) · memory_go \"{}\"\n", id, n, id.replace('/', " ")));
        }
        out
    }

    /// Counts one successful walk for every waypoint on the way. The brain shows a hot waypoint with `×n`, so the map
    /// learns which paths lead to answers that worked.
    pub fn mark_used(&self, trail: &[String]) {
        if trail.is_empty() {
            return;
        }
        let path = self.dir.join("uses.json");
        let mut v: serde_json::Value = fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_else(|| json!({}));
        for id in trail {
            v[id] = json!(v[id].as_u64().unwrap_or(0) + 1);
        }
        let _ = fs::write(path, serde_json::to_string_pretty(&v).unwrap());
    }

    /// How often a successful turn went through this waypoint.
    pub fn uses(&self, id: &str) -> u64 {
        fs::read_to_string(self.dir.join("uses.json")).ok().and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .map(|v| v[id].as_u64().unwrap_or(0)).unwrap_or(0)
    }

    /// Removes one note by its text. Returns the waypoint it was filed at.
    pub fn forget(&mut self, text: &str) -> Option<String> {
        let text = text.trim();
        if text.is_empty() {
            return None;
        }
        for id in self.waypoints() {
            let file = self.note_file(&id);
            let Ok(body) = fs::read_to_string(&file) else { continue };
            let target = format!("- {}", text);
            if !body.lines().any(|l| l == target) {
                continue;
            }
            let kept: Vec<&str> = body.lines().filter(|l| *l != target).collect();
            let _ = fs::write(&file, kept.join("\n") + "\n");
            let mut journal = self.journal();
            if let Some(at) = journal.iter().rposition(|v| v["id"].as_str() == Some(id.as_str()) && v["line"].as_str() == Some(text)) {
                journal.remove(at);
                let _ = fs::write(self.dir.join("journal.json"), serde_json::to_string_pretty(&journal).unwrap());
            }
            self.paths = json!({});
            self.save();
            return Some(id);
        }
        None
    }

    fn save(&self) {
        let _ = fs::write(self.dir.join("map.json"), serde_json::to_string_pretty(&self.map).unwrap());
        let _ = fs::write(self.dir.join("paths.json"), serde_json::to_string_pretty(&self.paths).unwrap());
    }

    fn note_file(&self, id: &str) -> PathBuf {
        let name = if id.is_empty() { "start".to_string() } else { id.replace('/', "__") };
        self.dir.join("notes").join(format!("{}.md", name))
    }

    fn node(&self, id: &str) -> Option<&Map<String, Value>> {
        self.map.get(id).and_then(|v| v.as_object())
    }

    fn children(&self, id: &str) -> Vec<String> {
        self.node(id)
            .and_then(|n| n.get("children"))
            .and_then(|c| c.as_array())
            .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
            .unwrap_or_default()
    }

    /// Creates the node and every missing parent. Returns the clean id.
    fn ensure(&mut self, path: &str) -> String {
        let parts: Vec<String> = path.split('/').map(|p| p.trim().to_lowercase().replace(' ', "-")).filter(|p| !p.is_empty()).collect();
        let mut id = String::new();
        for p in parts {
            let child = if id.is_empty() { p.clone() } else { format!("{}/{}", id, p) };
            if self.map.get(&child).is_none() {
                self.map[&child] = json!({"title": p, "signs": words(&p), "children": []});
                let kids = self.map[&id]["children"].as_array_mut().unwrap();
                kids.push(json!(child));
            }
            id = child;
        }
        id
    }

    /// Picks a branch for a note without a path: the first language or topic whose words occur in the text.
    pub fn auto_path(text: &str) -> String {
        let w = words(text);
        for (branch, keys) in BRANCHES {
            if w.iter().any(|x| keys.contains(&x.as_str())) {
                return format!("{}/notes", branch);
            }
        }
        "general/notes".to_string()
    }

    /// Adds one line to the note of a waypoint. New words of the line become signposts of that waypoint.
    pub fn note(&mut self, path: &str, text: &str) -> String {
        let path = if path.trim().is_empty() { Memory::auto_path(text) } else { path.to_string() };
        let id = self.ensure(&path);
        let file = self.note_file(&id);
        let mut body = fs::read_to_string(&file).unwrap_or_else(|_| format!("# {}\n\n", id));
        let line = text.trim().replace('\n', " ");
        if !body.contains(&line) {
            body.push_str(&format!("- {}\n", line));
            let mut journal = self.journal();
            journal.push(json!({"id": id, "line": line}));
            let from = journal.len().saturating_sub(500);
            let _ = fs::write(self.dir.join("journal.json"), serde_json::to_string_pretty(&journal[from..]).unwrap());
        }
        let _ = fs::write(&file, body);
        let mut signs: Vec<String> = self.map[&id]["signs"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
        for w in words(text) {
            if !signs.contains(&w) && signs.len() < 40 {
                signs.push(w);
            }
        }
        self.map[&id]["signs"] = json!(signs);
        self.paths = json!({}); // the map changed: stored walks may end too early now
        self.save();
        id
    }

    /// The notes at a waypoint and at every waypoint below it. At most `limit` characters, the newest lines survive a cut.
    pub fn branch(&self, root: &str, limit: usize) -> String {
        let (mut text, mut stack) = (String::new(), vec![root.to_string()]);
        while let Some(id) = stack.pop() {
            if let Ok(body) = fs::read_to_string(self.note_file(&id)) {
                let lines: Vec<&str> = body.lines().filter(|l| l.starts_with("- ")).collect();
                if !lines.is_empty() {
                    text.push_str(&format!("[{}]\n{}\n", id, lines.join("\n")));
                }
            }
            stack.extend(self.children(&id));
        }
        while text.len() > limit {
            match text.find('\n') {
                Some(i) => text.drain(..=i),
                None => break,
            };
        }
        text
    }

    /// Every note in the order it was written, at most the last 500.
    fn journal(&self) -> Vec<Value> {
        fs::read_to_string(self.dir.join("journal.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    /// Takes back the last `n` notes, newest first. Returns waypoint and text of each note removed. The waypoint and its
    /// signpost words stay in the map. Notes written before version 0.5.0 are not in the journal.
    pub fn unravel(&mut self, n: usize) -> Vec<(String, String)> {
        let mut journal = self.journal();
        let mut out = Vec::new();
        for _ in 0..n {
            let Some(entry) = journal.pop() else { break };
            let (id, line) = (entry["id"].as_str().unwrap_or("").to_string(), entry["line"].as_str().unwrap_or("").to_string());
            let file = self.note_file(&id);
            let Ok(body) = fs::read_to_string(&file) else { continue };
            let target = format!("- {}", line);
            let mut removed = false;
            let kept: Vec<&str> = body.lines().filter(|l| {
                let hit = !removed && *l == target;
                removed |= hit;
                !hit
            }).collect();
            if removed {
                let _ = fs::write(&file, kept.join("\n") + "\n");
                out.push((id, line));
            }
        }
        let _ = fs::write(self.dir.join("journal.json"), serde_json::to_string_pretty(&journal).unwrap());
        self.paths = json!({});
        self.save();
        out
    }

    fn score(&self, id: &str, query: &[String]) -> usize {
        // a waypoint matches with its own signposts and with the signposts of everything below it;
        // a word in the name of a waypoint counts 3, a signpost 1, a signpost with the same first 5 letters 1
        let mut signs: Vec<String> = Vec::new();
        let mut names: Vec<String> = Vec::new();
        let mut stack = vec![id.to_string()];
        while let Some(n) = stack.pop() {
            if let Some(a) = self.map[&n]["signs"].as_array() {
                signs.extend(a.iter().filter_map(|x| x.as_str().map(String::from)));
            }
            names.extend(words(n.rsplit('/').next().unwrap_or("")));
            stack.extend(self.children(&n));
        }
        let stem = |w: &str| w.chars().take(5).collect::<String>();
        query.iter().map(|q| {
            if names.contains(q) { 3 }
            else if signs.contains(q) { 1 }
            else if q.chars().count() >= 5 && signs.iter().any(|s| stem(s) == stem(q)) { 1 }
            else { 0 }
        }).sum()
    }

    /// Walks from the start to the waypoint for a request. Returns (ids on the way, true if the walk came from the path store).
    pub fn go(&mut self, query: &str) -> (Vec<String>, bool) {
        let sig = signature(query);
        if let Some(hit) = self.paths.get(&sig).and_then(|v| v.get("node")).and_then(|v| v.as_str()).map(String::from) {
            if self.map.get(&hit).is_some() {
                let n = self.paths[&sig]["hits"].as_u64().unwrap_or(0) + 1;
                self.paths[&sig]["hits"] = json!(n);
                self.save();
                return (Memory::trail(&hit), true);
            }
        }
        let q = words(query);
        let mut at = String::new();
        loop {
            let mut best: Option<(usize, String)> = None;
            for c in self.children(&at) {
                let s = self.score(&c, &q);
                if s > 0 && best.as_ref().map(|b| s > b.0).unwrap_or(true) {
                    best = Some((s, c));
                }
            }
            match best {
                Some((_, c)) => at = c,
                None => break,
            }
        }
        if !at.is_empty() && !sig.is_empty() {
            self.paths[&sig] = json!({"node": at, "hits": 1});
            self.save();
        }
        (Memory::trail(&at), false)
    }

    fn trail(id: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut cur = String::new();
        for p in id.split('/').filter(|p| !p.is_empty()) {
            cur = if cur.is_empty() { p.to_string() } else { format!("{}/{}", cur, p) };
            out.push(cur.clone());
        }
        out
    }

    /// The notes on the way to a waypoint, general rules first. At most `limit` characters.
    pub fn recall(&mut self, query: &str, limit: usize) -> (String, Vec<String>, bool) {
        let (trail, cached) = self.go(query);
        let mut text = String::new();
        for id in std::iter::once(String::new()).chain(trail.iter().cloned()) {
            if let Ok(body) = fs::read_to_string(self.note_file(&id)) {
                let lines: Vec<&str> = body.lines().filter(|l| l.starts_with("- ")).collect();
                if !lines.is_empty() {
                    text.push_str(&format!("[{}]\n{}\n", if id.is_empty() { "start" } else { &id }, lines.join("\n")));
                }
            }
        }
        if text.len() > limit {
            let mut cut = limit;
            while !text.is_char_boundary(cut) {
                cut -= 1;
            }
            text.truncate(cut);
        }
        (text, trail, cached)
    }

    /// The map as an indented list: every waypoint with the number of note lines.
    pub fn show(&self) -> String {
        let mut out = String::new();
        let mut stack = vec![(String::new(), 0usize)];
        while let Some((id, depth)) = stack.pop() {
            let notes = fs::read_to_string(self.note_file(&id)).map(|b| b.lines().filter(|l| l.starts_with("- ")).count()).unwrap_or(0);
            let title = if id.is_empty() { "start".to_string() } else { id.rsplit('/').next().unwrap().to_string() };
            out.push_str(&format!("{}{} ({} notes)\n", "  ".repeat(depth), title, notes));
            let mut kids = self.children(&id);
            kids.sort();
            for k in kids.into_iter().rev() {
                stack.push((k, depth + 1));
            }
        }
        let n = self.paths.as_object().map(|m| m.len()).unwrap_or(0);
        out.push_str(&format!("{} stored walks\n", n));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unravel_takes_back_the_newest_note() {
        let root = std::env::temp_dir().join(format!("penelope-memory-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let mut m = Memory::open(&root);
        let id = m.note("rust/errors", "Use anyhow for errors.");
        m.note("rust/errors", "No unwrap in library code.");
        assert_eq!(m.unravel(1), vec![(id.clone(), "No unwrap in library code.".to_string())]);
        let (text, _, _) = m.recall("rust errors", 800);
        assert!(text.contains("anyhow") && !text.contains("unwrap"), "{}", text);
        assert_eq!(m.unravel(5).len(), 1);
        assert!(m.unravel(1).is_empty());
        m.note("tools/edit", "Read the file before an edit.");
        m.note("tools/run/cargo", "Tests run with cargo test --release.");
        let lessons = m.branch("tools", 800);
        assert!(lessons.contains("[tools/edit]") && lessons.contains("cargo test --release") && !lessons.contains("anyhow"), "{}", lessons);
        assert!(m.branch("tools", 50).len() <= 50 && m.branch("missing", 800).is_empty());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn forget_index_and_the_scope_of_a_rule() {
        let root = std::env::temp_dir().join(format!("penelope-memory-index-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let mut m = Memory::open(&root);
        m.note("rust/errors", "Use anyhow for errors.");
        m.note("rust/errors", "No unwrap in library code.");
        m.note("rust/build", "cargo test --release is the fast check.");
        let index = m.index(10);
        assert!(index.contains("rust/errors (2 notes)") && index.contains("rust/build (1 notes)") && !index.contains("anyhow"), "{}", index);
        assert_eq!(m.forget("No unwrap in library code.").as_deref(), Some("rust/errors"));
        assert!(!m.notes_at("rust/errors").iter().any(|n| n.contains("unwrap")));
        assert!(m.forget("not there").is_none());
        m.mark_used(&["rust/errors".to_string(), "rust/errors".to_string()]);
        assert_eq!(m.uses("rust/errors"), 2);
        assert_eq!(scope_for("Always use tabs in Rust."), "global");
        assert_eq!(scope_for("The build uses cargo."), "project");
        let _ = fs::remove_dir_all(&root);
    }
}
