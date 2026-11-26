//! Notes with links and a status.
//!
//! A note is Markdown text. It may start with a header between two `---` lines holding `source:`, `date:` and
//! `status:` (`confirmed`, `open` or `stale`). A missing status counts as `open`. `[[name]]` in the body links to
//! another note by name. `within` returns the names of all notes at most `hops` links from a start note, following
//! links in both directions, without stale notes, sorted by name. `context` turns that selection into prompt text.
//!
//! A cluster keeps its notes under `.penelope/clusters/<id>/notes/`. The cluster validator sets the notes it accepts
//! to `confirmed` and a later rejection to `stale`; a part's prompt loads only the notes at most two links from its own
//! task note, so a stale fact never reaches a model.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// The longest body of one note that reaches a prompt, and the longest text of all selected notes together. A small
/// model gets a small context; the file itself keeps the full text.
const MAX_NOTE_TEXT: usize = 4_000;
const MAX_CONTEXT_TEXT: usize = 12_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Confirmed,
    Open,
    Stale,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Confirmed => "confirmed",
            Status::Open => "open",
            Status::Stale => "stale",
        }
    }
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

/// The date of today in UTC as `YYYY-MM-DD`, without a clock crate.
pub fn today() -> String {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let days = (secs / 86_400) as i64;
    let (year, month, day) = civil_from_days(days);
    format!("{:04}-{:02}-{:02}", year, month, day)
}

/// Howard Hinnant's `civil_from_days`: days since 1970-01-01 to a proleptic Gregorian date.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (year + if month <= 2 { 1 } else { 0 }, month, day)
}

/// The file of a note. A note name is one plain file name, never a path, so a note cannot write outside its folder.
fn file_of(dir: &Path, name: &str) -> Result<PathBuf, String> {
    let name = name.trim();
    if name.is_empty() || name == "." || name == ".." || name.contains('/') || name.contains('\\') {
        return Err(format!("note name {:?} is not a plain file name", name));
    }
    Ok(dir.join(format!("{}.md", name)))
}

/// The Markdown form of a note: header between two `---` lines, then the body.
pub fn render(note: &Note) -> String {
    format!("---\nsource: {}\ndate: {}\nstatus: {}\n---\n{}\n",
            note.source, note.date, note.status.as_str(), note.body.trim_end_matches('\n'))
}

/// Writes one note as `<name>.md` under `dir` and creates the folder when needed.
pub fn save(dir: &Path, note: &Note) -> Result<(), String> {
    let path = file_of(dir, &note.name)?;
    fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {}", dir.display(), e))?;
    fs::write(&path, render(note)).map_err(|e| format!("cannot write {}: {}", path.display(), e))
}

/// Changes only the status of an existing note and keeps source, date, body and links.
pub fn set_status(dir: &Path, name: &str, status: Status) -> Result<(), String> {
    let path = file_of(dir, name)?;
    let text = fs::read_to_string(&path).map_err(|e| format!("cannot read {}: {}", path.display(), e))?;
    let mut note = parse(name, &text)?;
    note.status = status;
    save(dir, &note)
}

/// The notes at most `hops` links away from `start`, links followed both ways, stale notes left out (a stale note
/// also does not pass the walk on), sorted by name. The start note is included when it is not stale. Links to notes
/// that do not exist are ignored. An unknown or stale start returns an empty list.
pub fn within_notes<'a>(notes: &'a [Note], start: &str, hops: usize) -> Vec<&'a Note> {
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
    seen.into_iter().filter_map(|name| by_name.get(name).copied()).collect()
}

/// Names of the notes at most `hops` links away from `start`, in the order `within_notes` returns them.
pub fn within(notes: &[Note], start: &str, hops: usize) -> Vec<String> {
    within_notes(notes, start, hops).into_iter().map(|n| n.name.clone()).collect()
}

/// The prompt text of a set of notes: one block per note, each body capped. Only the caller decides which notes;
/// `within_notes` and `context` already leave stale notes out.
pub fn text(notes: &[&Note]) -> String {
    let mut out = String::new();
    for note in notes {
        let block = format!("[note {} · {} · {} · {}]\n{}\n\n", note.name, note.source, note.date,
                            note.status.as_str(), note.body.trim().chars().take(MAX_NOTE_TEXT).collect::<String>());
        if out.len() + block.len() > MAX_CONTEXT_TEXT {
            break;
        }
        out.push_str(&block);
    }
    out
}

/// The prompt text of the notes at most `hops` links from `start`. Stale notes never appear and never carry the walk
/// past themselves; an unknown or stale start gives an empty text.
pub fn context(notes: &[Note], start: &str, hops: usize) -> String {
    text(&within_notes(notes, start, hops))
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
    fn context_holds_three_of_ten_notes_and_no_stale_text() {
        // n1 links n2 and n3; n2 links the stale n7, which links n8. n4, n5, n6 and n9 are a second island.
        let mut notes = vec![
            note("n1", "open", &["n2", "n3"]),
            note("n2", "confirmed", &["n7"]),
            note("n3", "open", &[]),
            note("n4", "open", &["n5"]),
            note("n5", "open", &["n6"]),
            note("n6", "open", &["n9"]),
            note("n7", "stale", &["n8"]),
            note("n8", "open", &[]),
            note("n9", "open", &[]),
        ];
        notes.push(note("n0", "open", &["n4"]));
        let picked = within_notes(&notes, "n1", 2);
        assert_eq!(picked.iter().map(|n| n.name.as_str()).collect::<Vec<_>>(), vec!["n1", "n2", "n3"]);
        let text = context(&notes, "n1", 2);
        assert_eq!(text.matches("[note ").count(), 3, "{}", text);
        assert!(text.contains("[note n1 ") && text.contains("[note n2 ") && text.contains("[note n3 "), "{}", text);
        assert!(!text.contains("[note n7 ") && !text.contains("[[n8]]"), "a stale note reached the prompt: {}", text);
    }

    #[test]
    fn save_and_set_status_round_trip() {
        let dir = std::env::temp_dir().join(format!("penelope-notes-save-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let note = parse("a", "---\nsource: validator\ndate: 2026-11-26\nstatus: open\n---\nsee [[b]]").unwrap();
        save(&dir, &note).unwrap();
        set_status(&dir, "a", Status::Confirmed).unwrap();
        let notes = load_dir(&dir).unwrap();
        assert_eq!((notes[0].status, notes[0].links.clone()), (Status::Confirmed, vec!["b".to_string()]));
        let before = fs::read_to_string(dir.join("a.md")).unwrap();
        assert!(before.contains("status: confirmed") && before.contains("see [[b]]"), "{}", before);
        assert!(save(&dir, &parse("../x", "x").unwrap()).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn today_is_a_date() {
        let date = today();
        let parts: Vec<&str> = date.split('-').collect();
        assert_eq!(parts.len(), 3, "{}", date);
        assert_eq!(parts[0].len(), 4);
        assert_eq!(parts[1].len(), 2);
        assert_eq!(parts[2].len(), 2);
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
