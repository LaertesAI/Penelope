//! Persistent memory as a map of waypoints.
//!
//! The map is a tree under `.penelope/memory/`. Every node is a waypoint with a title, signpost words and a note file.
//! A lookup starts at the root and follows the child whose signposts match the request best, one level at a time.
//! It never searches all notes. Every finished walk is stored in `paths.json`: the same request jumps to its
//! node the next time without walking.
//!
//! Every write goes through a lock file (`memory.lock`) and a temporary file with a rename, so two processes never
//! lose each other's notes and a killed writer leaves the old file. A damaged `map.json` is kept as
//! `map.json.corrupt-TIME` and the map is rebuilt from the note files. A note file with bytes that are not UTF-8 is
//! read with replacement characters and a copy of the original stays next to it as `NAME.md.bad-TIME`.

use serde_json::{json, Map, Value};
use std::fs;
use std::path::{Path, PathBuf};

pub struct Memory {
    pub dir: PathBuf,
    map: Value,
    paths: Value,
    /// Set when the map exists but cannot be read (permission, I/O). A write is refused then, so the map is never
    /// replaced by an empty one.
    map_error: Option<String>,
    map_stamp: Option<(u128, u64)>,
    paths_stamp: Option<(u128, u64)>,
}

/// At most this many stored walks stay in `paths.json`; the least used are dropped first.
const MAX_WALKS: usize = 800;

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

/// How many `.corrupt-` and `.bad-` copies of one file stay; older ones are removed.
const KEEP_COPIES: usize = 5;

fn stamp_of(path: &Path) -> Option<(u128, u64)> {
    let m = fs::metadata(path).ok()?;
    let t = m.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_nanos();
    Some((t, m.len()))
}

fn secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// The text of a note file. Bytes that are not UTF-8 become replacement characters, so one bad byte never hides the
/// other lines of the file.
fn read_text(path: &Path) -> Option<String> {
    fs::read(path).ok().map(|b| String::from_utf8_lossy(&b).into_owned())
}

fn valid_map(v: &Value) -> bool {
    v.get("").and_then(|r| r.get("children")).map(|c| c.is_array()).unwrap_or(false)
}

fn default_map() -> Value {
    json!({"": {"title": "start", "signs": [], "children": []}})
}

/// The clean id of a path: lower case, spaces as dashes, empty parts dropped.
fn clean_id(path: &str) -> String {
    path.split('/').map(|p| p.trim().to_lowercase().replace(' ', "-")).filter(|p| !p.is_empty()).collect::<Vec<_>>().join("/")
}

/// Creates the node and every missing parent in a map. Returns the clean id.
fn ensure_node(map: &mut Value, path: &str) -> String {
    let mut id = String::new();
    for p in clean_id(path).split('/').filter(|p| !p.is_empty()) {
        let child = if id.is_empty() { p.to_string() } else { format!("{}/{}", id, p) };
        if map.get(&child).is_none() {
            map[&child] = json!({"title": p, "signs": words(p), "children": []});
            if let Some(kids) = map[&id]["children"].as_array_mut() {
                kids.push(json!(child));
            }
        }
        id = child;
    }
    id
}

/// A map made again from the note files: one waypoint per file, the words of its lines as signposts.
fn rebuild_map(dir: &Path) -> Value {
    let mut map = default_map();
    let Ok(read) = fs::read_dir(dir.join("notes")) else { return map };
    let mut names: Vec<String> = read.flatten().filter_map(|e| e.file_name().to_str().and_then(|n| n.strip_suffix(".md")).map(String::from)).collect();
    names.sort();
    for name in names {
        if name == "start" || name.contains(".bad-") {
            continue;
        }
        let id = ensure_node(&mut map, &name.replace("__", "/"));
        if id.is_empty() {
            continue;
        }
        let mut signs: Vec<String> = Vec::new();
        for line in read_text(&dir.join("notes").join(format!("{}.md", name))).unwrap_or_default().lines().filter(|l| l.starts_with("- ")) {
            for w in words(line) {
                if !signs.contains(&w) && signs.len() < 40 {
                    signs.push(w);
                }
            }
        }
        map[&id]["signs"] = json!(signs);
    }
    map
}

/// Reads `map.json`. Returns the map, an error for a file that exists but cannot be read, and whether it was damaged.
fn load_map(dir: &Path) -> (Value, Option<String>, bool) {
    let file = dir.join("map.json");
    match fs::read(&file) {
        Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
            Ok(v) if valid_map(&v) => (v, None, false),
            _ => (rebuild_map(dir), None, true),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let has_notes = fs::read_dir(dir.join("notes")).map(|r| r.flatten().any(|e| e.file_name().to_string_lossy().ends_with(".md"))).unwrap_or(false);
            if has_notes { (rebuild_map(dir), None, true) } else { (default_map(), None, false) }
        }
        Err(e) => (default_map(), Some(format!("cannot read {}: {}", file.display(), e)), false),
    }
}

fn load_paths(dir: &Path) -> Value {
    fs::read(dir.join("paths.json")).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()).filter(|v| v.is_object()).unwrap_or_else(|| json!({}))
}

impl Memory {
    pub fn open(root: &Path) -> Memory {
        Memory::open_at(&root.join("memory"))
    }

    pub fn open_at(dir: &Path) -> Memory {
        let dir = dir.to_path_buf();
        let _ = fs::create_dir_all(dir.join("notes"));
        crate::fslock::sweep(&dir);
        crate::fslock::sweep(&dir.join("notes"));
        let (mut map, mut map_error, damaged) = load_map(&dir);
        if damaged {
            // Repair under the lock, so two processes do not both rename and write. The first one wins; the second
            // reads the repaired map.
            if let Ok(_lock) = crate::fslock::Lock::take(&dir, "memory.lock") {
                {
                    let (again, err, still) = load_map(&dir);
                    if still {
                        let kept = dir.join(format!("map.json.corrupt-{}", secs()));
                        let had = dir.join("map.json").exists();
                        let renamed = !had || fs::rename(dir.join("map.json"), &kept).is_ok();
                        let written = crate::fslock::write_atomic(&dir.join("map.json"), serde_json::to_string_pretty(&again).unwrap().as_bytes()).is_ok();
                        crate::fslock::prune(&dir, "map.json.corrupt-", KEEP_COPIES);
                        eprintln!("penelope: the memory map in {} was damaged; rebuilt it from {} note files{}", dir.display(),
                                  again.as_object().map(|m| m.len().saturating_sub(1)).unwrap_or(0),
                                  if had && renamed && written { format!("; the old file is {}", kept.display()) } else if !written { " (could not write it back: the folder is read-only?)".to_string() } else { String::new() });
                    }
                    map = again;
                    map_error = err;
                }
            }
        }
        let paths = load_paths(&dir);
        let (map_stamp, paths_stamp) = (stamp_of(&dir.join("map.json")), stamp_of(&dir.join("paths.json")));
        Memory { dir, map, paths, map_error, map_stamp, paths_stamp }
    }

    /// Takes the write lock and reads map and walks again if another process changed them since this value was opened.
    fn locked(&mut self) -> Result<crate::fslock::Lock, String> {
        if let Some(e) = &self.map_error {
            return Err(e.clone());
        }
        let lock = crate::fslock::Lock::take(&self.dir, "memory.lock")?;
        if stamp_of(&self.dir.join("map.json")) != self.map_stamp {
            let (map, err, _) = load_map(&self.dir);
            if let Some(e) = err {
                return Err(e);
            }
            self.map = map;
        }
        if stamp_of(&self.dir.join("paths.json")) != self.paths_stamp {
            self.paths = load_paths(&self.dir);
        }
        Ok(lock)
    }

    fn write(&self, name: &str, data: &[u8]) -> Result<(), String> {
        let path = self.dir.join(name);
        crate::fslock::write_atomic(&path, data).map_err(|e| format!("cannot write {}: {}", path.display(), e))
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
        read_text(&self.note_file(id)).map(|b| b.lines().filter_map(|l| l.strip_prefix("- ").map(str::to_string)).collect()).unwrap_or_default()
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
            let notes = read_text(&self.note_file(&id)).map(|b| b.lines().filter(|l| l.starts_with("- ")).count()).unwrap_or(0);
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
        let Ok(_lock) = crate::fslock::Lock::take(&self.dir, "memory.lock") else { return };
        let path = self.dir.join("uses.json");
        let mut v: serde_json::Value = fs::read(&path).ok().and_then(|s| serde_json::from_slice(&s).ok()).unwrap_or_else(|| json!({}));
        for id in trail {
            v[id] = json!(v[id].as_u64().unwrap_or(0) + 1);
        }
        let _ = crate::fslock::write_atomic(&path, serde_json::to_string_pretty(&v).unwrap().as_bytes());
    }

    /// How often a successful turn went through this waypoint.
    pub fn uses(&self, id: &str) -> u64 {
        fs::read(self.dir.join("uses.json")).ok().and_then(|s| serde_json::from_slice::<serde_json::Value>(&s).ok())
            .map(|v| v[id].as_u64().unwrap_or(0)).unwrap_or(0)
    }

    /// Removes one note by its text. Returns the waypoint it was filed at. A failure to write is reported on stderr and
    /// leaves the note where it is.
    pub fn forget(&mut self, text: &str) -> Option<String> {
        let text = text.trim();
        if text.is_empty() {
            return None;
        }
        let _lock = match self.locked() {
            Ok(l) => l,
            Err(e) => {
                eprintln!("penelope: NOT forgotten. {}", e);
                return None;
            }
        };
        for id in self.waypoints() {
            let file = self.note_file(&id);
            let Some(body) = read_text(&file) else { continue };
            let target = format!("- {}", text);
            if !body.lines().any(|l| l == target) {
                continue;
            }
            let kept: Vec<&str> = body.lines().filter(|l| *l != target).collect();
            if let Err(e) = self.put_note(&file, &(kept.join("\n") + "\n")) {
                eprintln!("penelope: NOT forgotten. {}", e);
                return None;
            }
            let mut journal = self.journal();
            if let Some(at) = journal.iter().rposition(|v| v["id"].as_str() == Some(id.as_str()) && v["line"].as_str() == Some(text)) {
                journal.remove(at);
                let _ = self.write("journal.json", serde_json::to_string_pretty(&journal).unwrap().as_bytes());
            }
            self.paths = json!({});
            if let Err(e) = self.save() {
                eprintln!("penelope: {}", e);
            }
            return Some(id);
        }
        None
    }

    /// Writes map and walks. The map first: a note file without a signpost is found again by a rebuild.
    fn save(&mut self) -> Result<(), String> {
        self.write("map.json", serde_json::to_string_pretty(&self.map).unwrap().as_bytes())?;
        self.map_stamp = stamp_of(&self.dir.join("map.json"));
        self.save_paths()
    }

    fn save_paths(&mut self) -> Result<(), String> {
        if let Some(m) = self.paths.as_object_mut() {
            if m.len() > MAX_WALKS {
                let mut by_use: Vec<(u64, String)> = m.iter().map(|(k, v)| (v["hits"].as_u64().unwrap_or(0), k.clone())).collect();
                by_use.sort();
                for (_, k) in by_use.into_iter().take(m.len() - MAX_WALKS) {
                    m.remove(&k);
                }
            }
        }
        self.write("paths.json", serde_json::to_string_pretty(&self.paths).unwrap().as_bytes())?;
        self.paths_stamp = stamp_of(&self.dir.join("paths.json"));
        Ok(())
    }

    /// Writes a note file. A file whose bytes are not UTF-8 keeps a copy of the original before it is rewritten.
    fn put_note(&self, file: &Path, body: &str) -> Result<(), String> {
        if let Ok(old) = fs::read(file) {
            if std::str::from_utf8(&old).is_err() {
                let name = file.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                let _ = fs::write(file.with_file_name(format!("{}.bad-{}", name, secs())), &old);
                if let Some(parent) = file.parent() {
                    crate::fslock::prune(parent, &format!("{}.bad-", name), KEEP_COPIES);
                }
            }
        }
        crate::fslock::write_atomic(file, body.as_bytes()).map_err(|e| format!("cannot write {}: {}", file.display(), e))
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
        ensure_node(&mut self.map, path)
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

    /// Adds one line to the note of a waypoint. New words of the line become signposts of that waypoint. A write that
    /// fails is reported on stderr; the id is returned either way. Use `try_note` to get the error.
    pub fn note(&mut self, path: &str, text: &str) -> String {
        match self.try_note(path, text) {
            Ok(id) => id,
            Err(e) => {
                eprintln!("penelope: NOT noted. {}", e);
                clean_id(&if path.trim().is_empty() { Memory::auto_path(text) } else { path.to_string() })
            }
        }
    }

    /// Like `note`, and an error when the note could not be written or the map cannot be read. The note is written
    /// under the lock, after the map and the walks are read again, so a second process never loses it.
    pub fn try_note(&mut self, path: &str, text: &str) -> Result<String, String> {
        let path = if path.trim().is_empty() { Memory::auto_path(text) } else { path.to_string() };
        let _lock = self.locked()?;
        let id = self.ensure(&path);
        let file = self.note_file(&id);
        let mut body = read_text(&file).unwrap_or_else(|| format!("# {}\n\n", id));
        let line = text.trim().replace('\n', " ");
        let known = body.lines().any(|l| l == format!("- {}", line));
        if !known {
            if !body.ends_with('\n') {
                body.push('\n'); // a half-written last line stays a line of its own
            }
            body.push_str(&format!("- {}\n", line));
            self.put_note(&file, &body)?;
            let mut journal = self.journal();
            journal.push(json!({"id": id, "line": line}));
            let from = journal.len().saturating_sub(500);
            let _ = self.write("journal.json", serde_json::to_string_pretty(&journal[from..]).unwrap().as_bytes());
        }
        let mut signs: Vec<String> = self.map[&id]["signs"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
        for w in words(text) {
            if !signs.contains(&w) && signs.len() < 40 {
                signs.push(w);
            }
        }
        self.map[&id]["signs"] = json!(signs);
        self.paths = json!({}); // the map changed: stored walks may end too early now
        self.save()?;
        Ok(id)
    }

    /// The notes at a waypoint and at every waypoint below it. At most `limit` characters, the newest lines survive a cut.
    pub fn branch(&self, root: &str, limit: usize) -> String {
        let (mut text, mut stack) = (String::new(), vec![root.to_string()]);
        while let Some(id) = stack.pop() {
            if let Some(body) = read_text(&self.note_file(&id)) {
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
        fs::read(self.dir.join("journal.json")).ok().and_then(|s| serde_json::from_slice(&s).ok()).unwrap_or_default()
    }

    /// Takes back the last `n` notes, newest first. Returns waypoint and text of each note removed. The waypoint and its
    /// signpost words stay in the map. Notes written before version 0.5.0 are not in the journal.
    pub fn unravel(&mut self, n: usize) -> Vec<(String, String)> {
        let _lock = match self.locked() {
            Ok(l) => l,
            Err(e) => {
                eprintln!("penelope: NOT taken back. {}", e);
                return Vec::new();
            }
        };
        let mut journal = self.journal();
        let mut out = Vec::new();
        for _ in 0..n {
            let Some(entry) = journal.pop() else { break };
            let (id, line) = (entry["id"].as_str().unwrap_or("").to_string(), entry["line"].as_str().unwrap_or("").to_string());
            let file = self.note_file(&id);
            let Some(body) = read_text(&file) else { continue };
            let target = format!("- {}", line);
            let mut removed = false;
            let kept: Vec<&str> = body.lines().filter(|l| {
                let hit = !removed && *l == target;
                removed |= hit;
                !hit
            }).collect();
            if removed {
                match self.put_note(&file, &(kept.join("\n") + "\n")) {
                    Ok(()) => out.push((id, line)),
                    Err(e) => {
                        eprintln!("penelope: NOT taken back. {}", e);
                        break;
                    }
                }
            }
        }
        let _ = self.write("journal.json", serde_json::to_string_pretty(&journal).unwrap().as_bytes());
        self.paths = json!({});
        if let Err(e) = self.save() {
            eprintln!("penelope: {}", e);
        }
        out
    }

    /// How well a waypoint matches the words of a request. A waypoint matches with its own signposts and with the
    /// signposts of everything below it: per word a name word counts 3, a signpost 1, a signpost with the same first
    /// 5 letters 1. One walk over the subtree, no copies of the signposts.
    fn score(&self, id: &str, query: &[String]) -> usize {
        let stem = |w: &str| w.chars().take(5).collect::<String>();
        let stems: Vec<Option<String>> = query.iter().map(|q| if q.chars().count() >= 5 { Some(stem(q)) } else { None }).collect();
        let mut best = vec![0usize; query.len()];
        let mut stack = vec![id.to_string()];
        while let Some(n) = stack.pop() {
            let name = words(n.rsplit('/').next().unwrap_or(""));
            let signs = self.map[&n]["signs"].as_array();
            for (i, q) in query.iter().enumerate() {
                if best[i] == 3 {
                    continue;
                }
                if name.contains(q) {
                    best[i] = 3;
                } else if best[i] == 0 {
                    if let Some(a) = signs {
                        if a.iter().any(|s| s.as_str() == Some(q.as_str())) || stems[i].as_ref().map(|st| a.iter().any(|s| s.as_str().map(|s| stem(s) == *st).unwrap_or(false))).unwrap_or(false) {
                            best[i] = 1;
                        }
                    }
                }
            }
            if best.iter().all(|b| *b == 3) {
                break;
            }
            stack.extend(self.children(&n));
        }
        best.iter().sum()
    }

    /// Walks from the start to the waypoint for a request. Returns (ids on the way, true if the walk came from the path store).
    /// The walk is stored for the next time; a folder that cannot be written only loses that speed-up.
    pub fn go(&mut self, query: &str) -> (Vec<String>, bool) {
        let sig = signature(query);
        if let Some(hit) = self.paths.get(&sig).and_then(|v| v.get("node")).and_then(|v| v.as_str()).map(String::from) {
            if self.map.get(&hit).is_some() {
                let n = self.paths[&sig]["hits"].as_u64().unwrap_or(0) + 1;
                self.paths[&sig]["hits"] = json!(n);
                self.store_walks();
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
            self.store_walks();
        }
        (Memory::trail(&at), false)
    }

    /// Writes the walks (not the map) under the lock, after merging with what another process wrote meanwhile.
    fn store_walks(&mut self) {
        let mine = std::mem::take(&mut self.paths);
        let Ok(_lock) = crate::fslock::Lock::take(&self.dir, "memory.lock") else {
            self.paths = mine;
            return;
        };
        let mut merged = load_paths(&self.dir);
        if let (Some(m), Some(mine)) = (merged.as_object_mut(), mine.as_object()) {
            for (k, v) in mine {
                let hits = |x: &Value| x["hits"].as_u64().unwrap_or(0);
                if m.get(k).map(|old| hits(old) < hits(v)).unwrap_or(true) {
                    m.insert(k.clone(), v.clone());
                }
            }
        }
        // a note written by another process emptied the walks: its map is newer than mine, so drop my walks too
        self.paths = if stamp_of(&self.dir.join("map.json")) == self.map_stamp { merged } else { load_paths(&self.dir) };
        let _ = self.save_paths();
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
            if let Some(body) = read_text(&self.note_file(&id)) {
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
            let notes = read_text(&self.note_file(&id)).map(|b| b.lines().filter(|l| l.starts_with("- ")).count()).unwrap_or(0);
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

    fn temp_root(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("penelope-memory-{}-{}", tag, std::process::id()));
        let _ = fs::remove_dir_all(&root);
        root
    }

    #[test]
    fn eight_writers_to_one_waypoint_lose_no_note() {
        let root = temp_root("writers");
        Memory::open(&root).note("base/first", "first");
        let handles: Vec<_> = (0..8).map(|k| {
            let root = root.clone();
            std::thread::spawn(move || {
                let mut m = Memory::open(&root); // every writer holds its own copy of the map, as a process does
                for i in 0..25 {
                    m.try_note("shared/topic", &format!("writer {} note {}", k, i)).unwrap();
                    m.try_note(&format!("own/w{}", k), &format!("own {} {}", k, i)).unwrap();
                }
            })
        }).collect();
        for h in handles {
            h.join().unwrap();
        }
        let m = Memory::open(&root);
        assert_eq!(m.notes_at("shared/topic").len(), 200);
        for k in 0..8 {
            assert_eq!(m.notes_at(&format!("own/w{}", k)).len(), 25, "writer {}", k);
        }
        let ids = m.waypoints();
        assert!(ids.contains(&"base/first".to_string()) && ids.contains(&"shared/topic".to_string()) && (0..8).all(|k| ids.contains(&format!("own/w{}", k))), "{:?}", ids);
        let left: Vec<String> = fs::read_dir(&m.dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).filter(|n| n.contains(".tmp-") || n == "memory.lock").collect();
        assert!(left.is_empty(), "{:?}", left);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_short_note_inside_a_longer_line_is_still_stored() {
        let root = temp_root("substring");
        let mut m = Memory::open(&root);
        m.try_note("a/b", "use banana peels").unwrap();
        m.try_note("a/b", "banana").unwrap();
        m.try_note("a/b", "banana").unwrap(); // the same line twice is one note
        assert_eq!(m.notes_at("a/b"), vec!["use banana peels".to_string(), "banana".to_string()]);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn corrupt_copies_and_temporaries_do_not_pile_up() {
        let root = temp_root("copies");
        let mut m = Memory::open(&root);
        m.note("a/b", "fact");
        let map = root.join("memory").join("map.json");
        for t in 0..8u64 {
            fs::write(root.join("memory").join(format!("map.json.corrupt-{}", 100 + t)), b"old").unwrap();
        }
        let tmp = root.join("memory").join(".map.json.tmp-9-9");
        fs::write(&tmp, b"half").unwrap();
        fs::File::options().write(true).open(&tmp).unwrap().set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(7200)).unwrap();
        fs::write(&map, b"{ broken").unwrap();
        let m = Memory::open(&root);
        assert_eq!(m.waypoints().len(), 2);
        let kept = fs::read_dir(root.join("memory")).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().starts_with("map.json.corrupt-")).count();
        assert_eq!(kept, KEEP_COPIES);
        assert!(!tmp.exists(), "an hour-old temporary of a killed writer is removed");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_damaged_map_is_kept_and_rebuilt_from_the_notes() {
        let root = temp_root("damaged");
        let mut m = Memory::open(&root);
        let animals = ["alpaca", "badger", "coyote", "dingo", "ermine", "ferret", "gecko", "heron", "ibis", "jackal"];
        for (i, a) in animals.iter().enumerate() {
            m.note(&format!("area/topic{}", i), &format!("fact number {} about {}", i, a));
        }
        let before = m.waypoints().len();
        let map = root.join("memory").join("map.json");
        let raw = fs::read(&map).unwrap();
        fs::write(&map, &raw[..raw.len() / 2]).unwrap();
        let mut m = Memory::open(&root);
        assert_eq!(m.waypoints().len(), before, "every waypoint is back");
        let kept: Vec<String> = fs::read_dir(root.join("memory")).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).filter(|n| n.starts_with("map.json.corrupt-")).collect();
        assert_eq!(kept.len(), 1, "the damaged file is kept: {:?}", kept);
        assert!(serde_json::from_slice::<Value>(&fs::read(&map).unwrap()).is_ok(), "map.json is valid again");
        let (text, trail, _) = m.recall("ermine", 800);
        assert!(text.contains("fact number 4") && trail.last().map(String::as_str) == Some("area/topic4"), "{} {:?}", text, trail);
        m.note("area/topic3", "one more");
        assert_eq!(m.notes_at("area/topic3").len(), 2);
        fs::remove_file(&map).unwrap(); // a missing map with notes in the folder is rebuilt too
        assert_eq!(Memory::open(&root).waypoints().len(), before);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_note_file_with_bad_bytes_keeps_its_other_lines_and_a_copy() {
        let root = temp_root("badbytes");
        let mut m = Memory::open(&root);
        m.note("rust/errors", "first line");
        let file = root.join("memory").join("notes").join("rust__errors.md");
        let original = b"# rust/errors\n\n- first line\n- bad \xff\xfe bytes\n".to_vec();
        fs::write(&file, &original).unwrap();
        assert_eq!(m.notes_at("rust/errors").len(), 2, "the readable lines are still read");
        m.try_note("rust/errors", "after the damage").unwrap();
        let now = String::from_utf8(fs::read(&file).unwrap()).expect("the rewritten file is UTF-8");
        assert!(now.contains("first line") && now.contains("bad ") && now.contains("after the damage"), "{}", now);
        let copies: Vec<PathBuf> = fs::read_dir(root.join("memory").join("notes")).unwrap().flatten().map(|e| e.path()).filter(|p| p.to_string_lossy().contains(".bad-")).collect();
        assert_eq!(copies.len(), 1);
        assert_eq!(fs::read(&copies[0]).unwrap(), original, "the original bytes are kept");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_half_written_last_line_stays_a_line_of_its_own() {
        let root = temp_root("halfwritten");
        let mut m = Memory::open(&root);
        m.note("a/b", "whole");
        let file = root.join("memory").join("notes").join("a__b.md");
        let mut body = fs::read_to_string(&file).unwrap();
        body.push_str("- half writ");
        fs::write(&file, body).unwrap();
        m.note("a/b", "next");
        assert_eq!(m.notes_at("a/b"), vec!["whole".to_string(), "half writ".to_string(), "next".to_string()]);
        let _ = fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn a_read_only_folder_is_an_error_and_loses_nothing() {
        use std::os::unix::fs::PermissionsExt;
        let root = temp_root("readonly");
        let mut m = Memory::open(&root);
        m.note("rust/errors", "kept");
        let dir = root.join("memory");
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).unwrap();
        fs::set_permissions(dir.join("notes"), fs::Permissions::from_mode(0o555)).unwrap();
        if fs::write(dir.join("probe"), b"x").is_ok() {
            let _ = fs::remove_dir_all(&root); // root user: nothing is read-only
            return;
        }
        let t = std::time::Instant::now();
        let err = m.try_note("rust/errors", "refused").unwrap_err();
        assert!(err.contains("cannot") && t.elapsed().as_secs() < 3, "{} after {:?}", err, t.elapsed());
        assert_eq!(m.notes_at("rust/errors"), vec!["kept".to_string()]);
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        fs::set_permissions(dir.join("notes"), fs::Permissions::from_mode(0o755)).unwrap();
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn stored_walks_are_capped_and_the_least_used_go_first() {
        let root = temp_root("walks");
        let mut m = Memory::open(&root);
        m.note("alpha", "alpha note");
        for i in 0..900 {
            m.paths[format!("sig{}", i)] = json!({"node": "alpha", "hits": if i < 100 { 1 } else { 5 }});
        }
        m.save_paths().unwrap();
        let stored = load_paths(&m.dir);
        assert_eq!(stored.as_object().unwrap().len(), MAX_WALKS);
        assert!(stored.get("sig899").is_some() && stored.get("sig0").is_none());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_walk_with_a_word_that_is_not_in_the_map_still_finds_the_branch() {
        let root = temp_root("score");
        let mut m = Memory::open(&root);
        m.note("rust/errors", "Use anyhow for errors in parsers.");
        m.note("python/venv", "Create a venv with uv for every project.");
        let (trail, cached) = m.go("how do errors work in rust parsing");
        assert_eq!(trail.last().map(String::as_str), Some("rust/errors"));
        assert!(!cached);
        assert!(m.go("how do errors work in rust parsing").1, "the second walk comes from the store");
        let _ = fs::remove_dir_all(&root);
    }
}
