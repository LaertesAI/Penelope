//! The brain view: the memory map as a graph and as an Obsidian vault.
//!
//! `ascii` draws the map in the terminal: one node per line, the connectors of a tree, the number of notes and a small
//! bar. `vault` writes the same map as Markdown files with `[[wikilinks]]` under `.penelope/brain/`, so Obsidian can
//! open the folder and draw its own graph. Both read only the map, never a model.

use crate::config::Ctx;
use crate::memory::Memory;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone)]
pub struct Node {
    pub id: String,
    pub title: String,
    pub depth: usize,
    pub notes: Vec<String>,
}

/// The map as a flat list in the order a reader walks it.
pub fn nodes(memory: &Memory) -> Vec<Node> {
    fn go(m: &Memory, id: &str, depth: usize, out: &mut Vec<Node>) {
        if !id.is_empty() {
            out.push(Node { id: id.to_string(), title: m.title(id), depth, notes: m.notes_at(id) });
        }
        for kid in m.kids(id) {
            go(m, &kid, if id.is_empty() { 0 } else { depth + 1 }, out);
        }
    }
    let mut out = Vec::new();
    go(memory, "", 0, &mut out);
    out
}

pub fn counts(ctx: &Ctx) -> (usize, usize, usize) {
    let m = Memory::open(&ctx.root);
    let list = nodes(&m);
    let notes = list.iter().map(|n| n.notes.len()).sum();
    let edges = list.len().saturating_sub(1) + list.iter().filter(|n| n.depth == 0).count().saturating_sub(1);
    (list.len(), notes, edges)
}

/// The graph in the terminal. An open circle is a waypoint without notes, a full one has notes. The bar shows how full
/// the waypoint is against the fullest one, so a long branch stands out.
pub fn ascii(ctx: &Ctx) -> String {
    let m = Memory::open(&ctx.root);
    let list = nodes(&m);
    if list.is_empty() {
        return "the brain is empty. A note with memory_note starts the first waypoint.\n".to_string();
    }
    let max = list.iter().map(|n| n.notes.len()).max().unwrap_or(0).max(1);
    let (waypoints, notes, _) = counts(ctx);
    let mut out = format!("{} waypoints · {} notes · {} stored walks\n\n", waypoints, notes,
                          fs::read_to_string(m.dir.join("paths.json")).ok().and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                              .and_then(|v| v.as_object().map(|o| o.len())).unwrap_or(0));
    for (i, n) in list.iter().enumerate() {
        let last_at_depth = list[i + 1..].iter().find(|x| x.depth <= n.depth).map(|x| x.depth < n.depth).unwrap_or(true);
        let pad = "│  ".repeat(n.depth.saturating_sub(1)) + if n.depth == 0 { "" } else if last_at_depth { "└─ " } else { "├─ " };
        let bar = if n.notes.is_empty() { String::new() } else {
            let full = (n.notes.len() * 8 / max).max(1);
            format!(" {}{}", "█".repeat(full), "·".repeat(8 - full))
        };
        let uses = m.uses(&n.id);
        out.push_str(&format!("{}{} {}{} ({} notes{})\n", pad, if n.notes.is_empty() { "○" } else { "●" }, n.title, bar, n.notes.len(),
                              if uses > 0 { format!(" · ×{}", uses) } else { String::new() }));
        if n.depth <= 1 {
            for note in n.notes.iter().take(2) {
                out.push_str(&format!("{}    {}\n", "│  ".repeat(n.depth), note.chars().take(96).collect::<String>()));
            }
        }
    }
    out.push_str("\nThe same map as an Obsidian vault: penelope brain --write (or /brain write in the interface).\n");
    out
}

fn file_name(id: &str) -> String {
    if id.is_empty() { "start".to_string() } else { id.replace('/', " · ") }
}

fn clean(text: &str) -> String {
    text.replace('|', "\\|").replace('\n', " ").trim().to_string()
}

/// Writes `.penelope/brain/` as an Obsidian vault: `index.md`, one file per waypoint, `[[links]]` between parent and
/// children, the notes below a heading. Returns the folder and the number of files.
pub fn write_vault(ctx: &Ctx) -> Result<(PathBuf, usize), String> {
    let m = Memory::open(&ctx.root);
    let mut list = nodes(&m);
    if !list.iter().any(|n| n.id.is_empty()) {
        list.insert(0, Node { id: String::new(), title: "start".to_string(), depth: 0, notes: m.notes_at("") });
    }
    let dir = ctx.root.join("brain");
    fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {}", dir.display(), e))?;
    // Old files of a waypoint that was renamed would stay and make the graph lie.
    if let Ok(entries) = fs::read_dir(&dir) {
        for e in entries.flatten() {
            if e.path().extension().and_then(|x| x.to_str()) == Some("md") {
                let _ = fs::remove_file(e.path());
            }
        }
    }
    let mut by_id: BTreeMap<String, Node> = BTreeMap::new();
    for n in &list {
        by_id.insert(n.id.clone(), n.clone());
    }
    for n in &list {
        let parent = n.id.rsplit_once('/').map(|(p, _)| p.to_string()).unwrap_or_default();
        let mut body = String::new();
        body.push_str("---\n");
        body.push_str(&format!("penelope_waypoint: \"{}\"\nnotes: {}\n---\n\n", n.id, n.notes.len()));
        body.push_str(&format!("# {}\n\n", if n.id.is_empty() { "start".to_string() } else { n.id.clone() }));
        if !n.id.is_empty() {
            body.push_str(&format!("Up: [[{}]]  \n", file_name(&parent)));
        }
        let kids = m.kids(&n.id);
        if !kids.is_empty() {
            body.push_str("Down: ");
            body.push_str(&kids.iter().map(|k| format!("[[{}]]", file_name(k))).collect::<Vec<_>>().join(" · "));
            body.push_str("  \n");
        }
        body.push_str("\n## Notes\n\n");
        if n.notes.is_empty() {
            body.push_str("_No notes at this waypoint. It is a signpost._\n");
        }
        for note in &n.notes {
            body.push_str(&format!("- {}\n", clean(note)));
        }
        let path = dir.join(format!("{}.md", file_name(&n.id)));
        fs::write(&path, body).map_err(|e| format!("cannot write {}: {}", path.display(), e))?;
    }
    let mut index = String::from("---\npenelope: brain\n---\n\n# The brain of Penelope\n\n");
    index.push_str(&format!("{} waypoints, {} notes. Open this folder as a vault in Obsidian and its graph view draws the same map.\n\n",
                            list.len(), list.iter().map(|n| n.notes.len()).sum::<usize>()));
    index.push_str("## Map\n\n");
    for n in &list {
        index.push_str(&format!("{}- [[{}]] — {} notes\n", "  ".repeat(n.depth), file_name(&n.id), n.notes.len()));
    }
    index.push_str("\n## How a waypoint works\n\nA lookup follows signposts one level at a time. A finished walk is stored, so the same request jumps to its waypoint next time. `penelope memory unravel` takes a note back.\n");
    fs::write(dir.join("index.md"), index).map_err(|e| format!("cannot write index.md: {}", e))?;
    Ok((dir, list.len() + 1))
}

/// The vault path for a message.
pub fn show(p: &Path) -> String {
    crate::files::show(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_graph_and_the_vault_show_the_same_waypoints() {
        let dir = std::env::temp_dir().join(format!("penelope-brain-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let ctx = crate::config::load(&dir.join(".penelope"));
        let mut m = Memory::open(&ctx.root);
        m.note("rust/errors", "Use anyhow in the binary, thiserror in libraries.");
        m.note("rust/errors", "No unwrap in library code.");
        m.note("rust/build", "cargo test --release runs the fast tests.");
        m.note("git/commits", "One change per commit.");
        let text = ascii(&ctx);
        assert!(text.contains("5 waypoints · 4 notes") && text.contains("● errors") && text.contains("○ rust"), "{}", text);
        assert!(text.contains("○ rust") && text.contains("○ git"), "{}", text);
        let (path, files) = write_vault(&ctx).unwrap();
        assert_eq!(files, 7);          // start, rust, errors, build, git, commits, plus index
        let index = fs::read_to_string(path.join("index.md")).unwrap();
        assert!(index.contains("[[rust · errors]] — 2 notes") && index.contains("[[git]]"), "{}", index);
        let node = fs::read_to_string(path.join("rust · errors.md")).unwrap();
        assert!(node.contains("penelope_waypoint: \"rust/errors\"") && node.contains("Up: [[rust]]") && node.contains("Use anyhow"), "{}", node);
        assert!(node.contains("No unwrap") && !node.contains("cargo test"), "{}", node);
        let start = fs::read_to_string(path.join("start.md")).unwrap();
        assert!(start.contains("Down: [[git]] · [[rust]]"), "{}", start);
        let _ = fs::remove_dir_all(&dir);
    }
}
