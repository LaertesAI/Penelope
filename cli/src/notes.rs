//! Notes with links and a status.
//!
//! A note is Markdown text. It may start with a header between two `---` lines holding `source:`, `date:` and
//! `status:` (`confirmed`, `open` or `stale`). A missing status counts as `open`. `[[name]]` in the body links to
//! another note by name. `within` returns the names of all notes at most `hops` links from a start note, following
//! links in both directions, without stale notes, sorted by name.
//!
//! Not wired into a command yet: nothing outside the tests calls this module.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Confirmed,
    Open,
    Stale,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Note {
    pub name: String,
    pub source: String,
    pub date: String,
    pub status: Status,
    pub body: String,
    pub links: Vec<String>,
}

fn parse_status(s: &str) -> Result<Status, String> {
    match s.trim() {
        "confirmed" => Ok(Status::Confirmed),
        "open" | "" => Ok(Status::Open),
        "stale" => Ok(Status::Stale),
        other => Err(format!("unknown status {}", other)),
    }
}

/// All `[[name]]` links of a text, in order, without duplicates. `[[name|label]]` links to `name`.
pub fn links(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("[[") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("]]") else { break };
        let inner = &after[..end];
        let name = inner.split('|').next().unwrap_or("").trim();
        if !name.is_empty() && !inner.contains('\n') && !out.iter().any(|n| n == name) {
            out.push(name.to_string());
        }
        rest = &after[end + 2..];
    }
    out
}

/// Reads one note. `name` comes from the caller (the file name without `.md`).
pub fn parse(name: &str, text: &str) -> Result<Note, String> {
    let (mut source, mut date, mut status) = (String::new(), String::new(), Status::Open);
    let mut body = text;
    if text.starts_with("---") {
        let mut lines = text.lines();
        lines.next();
        let mut consumed = text.lines().next().map(|l| l.len() + 1).unwrap_or(0);
        let mut closed = false;
        for line in lines {
            consumed += line.len() + 1;
            if line.trim() == "---" {
                closed = true;
                break;
            }
            if let Some((k, v)) = line.split_once(':') {
                match k.trim() {
                    "source" => source = v.trim().to_string(),
                    "date" => date = v.trim().to_string(),
                    "status" => status = parse_status(v).map_err(|e| format!("note {}: {}", name, e))?,
                    _ => {}
                }
            }
        }
        if !closed {
            return Err(format!("note {}: header is not closed with ---", name));
        }
        body = text.get(consumed.min(text.len())..).unwrap_or("");
    }
    Ok(Note { name: name.to_string(), source, date, status, body: body.to_string(), links: links(body) })
}

/// Loads every `*.md` file of a folder. The file name without `.md` is the note name. A file that does not parse is an error.
pub fn load_dir(dir: &Path) -> Result<Vec<Note>, String> {
    let mut notes = Vec::new();
    let entries = fs::read_dir(dir).map_err(|e| format!("cannot read {}: {}", dir.display(), e))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
        let text = fs::read_to_string(&path).map_err(|e| format!("cannot read {}: {}", path.display(), e))?;
        notes.push(parse(&name, &text)?);
    }
    notes.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(notes)
}

/// Names of the notes at most `hops` links away from `start`, links followed both ways, stale notes left out
/// (a stale note also does not pass the walk on), sorted by name. The start note is included when it is not stale.
/// Links to notes that do not exist are ignored. An unknown or stale start returns an empty list.
pub fn within(notes: &[Note], start: &str, hops: usize) -> Vec<String> {
    let by_name: BTreeMap<&str, &Note> = notes.iter().map(|n| (n.name.as_str(), n)).collect();
    match by_name.get(start) {
        Some(n) if n.status != Status::Stale => {}
        _ => return Vec::new(),
    }
    let mut neighbours: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for n in notes.iter().filter(|n| n.status != Status::Stale) {
        for l in &n.links {
            if let Some(t) = by_name.get(l.as_str()).filter(|t| t.status != Status::Stale) {
                neighbours.entry(n.name.as_str()).or_default().insert(t.name.as_str());
                neighbours.entry(t.name.as_str()).or_default().insert(n.name.as_str());
            }
        }
    }
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut queue: VecDeque<(&str, usize)> = VecDeque::new();
    seen.insert(start);
    queue.push_back((start, 0));
    while let Some((name, depth)) = queue.pop_front() {
        if depth == hops {
            continue;
        }
        for next in neighbours.get(name).into_iter().flatten() {
            if seen.insert(next) {
                queue.push_back((next, depth + 1));
            }
        }
    }
    seen.into_iter().map(|s| s.to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(name: &str, status: &str, links: &[&str]) -> Note {
        let body: String = links.iter().map(|l| format!("see [[{}]]\n", l)).collect();
        parse(name, &format!("---\nsource: test\ndate: 2026-11-25\nstatus: {}\n---\n{}", status, body)).unwrap()
    }

    #[test]
    fn parses_header_and_body() {
        let n = parse("a", "---\nsource: validator\ndate: 2026-11-25\nstatus: confirmed\n---\nText with [[b]] and [[c]].\n").unwrap();
        assert_eq!(n.source, "validator");
        assert_eq!(n.date, "2026-11-25");
        assert_eq!(n.status, Status::Confirmed);
        assert_eq!(n.links, vec!["b", "c"]);
        assert!(n.body.starts_with("Text with"));
    }

    #[test]
    fn note_without_header_is_open() {
        let n = parse("a", "Just text [[b]]").unwrap();
        assert_eq!(n.status, Status::Open);
        assert_eq!(n.links, vec!["b"]);
    }

    #[test]
    fn bad_status_and_open_header_are_errors() {
        assert!(parse("a", "---\nstatus: maybe\n---\nx").is_err());
        assert!(parse("a", "---\nstatus: open\nno end").is_err());
    }

    #[test]
    fn links_skip_duplicates_labels_and_broken_brackets() {
        assert_eq!(links("[[a]] [[a]] [[b|label]] [[ ]] [[open"), vec!["a", "b"]);
    }

    #[test]
    fn ten_linked_notes_within_two_hops_without_stale() {
        // chain n0 - n1 - n2 - n3 - n4 - n5, a branch n1 - n6, n6 - n7 (stale), n2 - n8, n8 - n9
        let notes = vec![
            note("n0", "open", &["n1"]),
            note("n1", "confirmed", &["n2", "n6"]),
            note("n2", "open", &["n3", "n8"]),
            note("n3", "open", &["n4"]),
            note("n4", "open", &["n5"]),
            note("n5", "open", &[]),
            note("n6", "open", &["n7"]),
            note("n7", "stale", &[]),
            note("n8", "open", &["n9"]),
            note("n9", "open", &[]),
        ];
        // from n1: n0, n2, n6 at 1 hop; n3, n8, n7 at 2 hops. n7 is stale. n4, n9 are 3 hops.
        assert_eq!(within(&notes, "n1", 2), vec!["n0", "n1", "n2", "n3", "n6", "n8"]);
    }

    #[test]
    fn follows_links_in_both_directions() {
        let notes = vec![note("a", "open", &[]), note("b", "open", &["a"])];
        assert_eq!(within(&notes, "a", 1), vec!["a", "b"]);
    }

    #[test]
    fn cycles_and_missing_targets_do_not_hang() {
        let notes = vec![note("a", "open", &["b", "ghost"]), note("b", "open", &["c"]), note("c", "open", &["a"])];
        assert_eq!(within(&notes, "a", 5), vec!["a", "b", "c"]);
    }

    #[test]
    fn zero_hops_and_unknown_or_stale_start() {
        let notes = vec![note("a", "open", &["b"]), note("b", "stale", &[])];
        assert_eq!(within(&notes, "a", 0), vec!["a"]);
        assert!(within(&notes, "b", 2).is_empty());
        assert!(within(&notes, "zzz", 2).is_empty());
    }

    #[test]
    fn stale_note_does_not_pass_the_walk_on() {
        let notes = vec![note("a", "open", &["s"]), note("s", "stale", &["c"]), note("c", "open", &[])];
        assert_eq!(within(&notes, "a", 3), vec!["a"]);
    }

    #[test]
    fn loads_markdown_files_of_a_folder() {
        let dir = std::env::temp_dir().join(format!("penelope-notes-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("one.md"), "---\nstatus: confirmed\n---\n[[two]]").unwrap();
        fs::write(dir.join("two.md"), "plain").unwrap();
        fs::write(dir.join("skip.txt"), "[[one]]").unwrap();
        let notes = load_dir(&dir).unwrap();
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(notes.iter().map(|n| n.name.as_str()).collect::<Vec<_>>(), vec!["one", "two"]);
        assert_eq!(within(&notes, "one", 1), vec!["one", "two"]);
    }
}
