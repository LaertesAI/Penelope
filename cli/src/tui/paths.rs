//! Dropped files and the workspace folder: the path text a terminal pastes for a drop, and the paths of `/folder`.
//!
//! A terminal delivers a drop as pasted text. It holds one or more paths, written as plain text, in quotes, with
//! backslash-escaped spaces or as `file://` URIs with percent escapes. Everything here is pure, so the tests need no terminal.

use std::path::{Path, PathBuf};

/// Splits pasted text into words. Single and double quotes group, a backslash escapes the next character.
fn words(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let (mut quote, mut open) = (None::<char>, false);
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (None, '\\') | (Some('"'), '\\') => {
                if let Some(next) = chars.next() { cur.push(next); open = true; }
            }
            (None, '\'') | (None, '"') => { quote = Some(c); open = true; }
            (Some(q), c) if c == q => quote = None,
            (None, c) if c.is_whitespace() || c == '\0' => {
                if open { out.push(std::mem::take(&mut cur)); open = false; }
            }
            _ => { cur.push(c); open = true; }
        }
    }
    if open { out.push(cur); }
    out
}

/// The text of a `%XX` escaped string with the escapes decoded.
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = if bytes[i] == b'%' && i + 2 < bytes.len() {
            std::str::from_utf8(&bytes[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok())
        } else { None };
        match hex {
            Some(b) => { out.push(b); i += 3; }
            None => { out.push(bytes[i]); i += 1; }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

/// One word as a path text: a `file://` URI loses its scheme and its escapes, `~/` becomes the home folder.
fn word_path(word: &str, home: &str) -> String {
    let word = match word.strip_prefix("file://") {
        Some(rest) => percent_decode(rest.strip_prefix("localhost").unwrap_or(rest)),
        None => word.to_string(),
    };
    match word.strip_prefix("~/") {
        Some(rest) if !home.is_empty() => format!("{}/{}", home.trim_end_matches('/'), rest),
        _ => word,
    }
}

/// The paths of a drop. Every word must be an absolute path, a `~/` path or a `file://` URI and must exist, else the
/// text is no drop and None comes back. A bare word like `src` stays normal text.
pub fn dropped(text: &str, home: &str) -> Option<Vec<PathBuf>> {
    let list = words(text);
    if list.is_empty() {
        return None;
    }
    let mut out = Vec::new();
    for w in &list {
        let anchored = w.starts_with('/') || w.starts_with("~/") || w.starts_with("file://");
        let p = PathBuf::from(word_path(w, home));
        if !anchored || !p.is_absolute() || !p.exists() {
            return None;
        }
        out.push(p);
    }
    Some(out)
}

/// True if the text looks like a drop: quoted, escaped or a `file://` URI. A plain typed path is not.
pub fn drop_form(text: &str) -> bool {
    let t = text.trim();
    t.starts_with('\'') || t.starts_with('"') || t.starts_with("file://") || t.contains('\\')
}

/// The folder that `/folder ARG` names: absolute, `~`, or relative to the current folder. It must be an existing folder.
pub fn resolve_folder(arg: &str, cwd: &Path, home: &str) -> Result<PathBuf, String> {
    let raw = arg.trim();
    // A name with spaces may come without quotes: the whole text counts first, then the first word.
    let whole = |a: &str| { let p = PathBuf::from(word_path(a, home)); (if p.is_absolute() { p } else { cwd.join(p) }).exists() };
    let arg = if whole(raw) { raw.to_string() } else { words(raw).into_iter().next().unwrap_or_default() };
    let path = if arg == "~" && !home.is_empty() { PathBuf::from(home) } else { PathBuf::from(word_path(&arg, home)) };
    let full = if path.is_absolute() { path } else { cwd.join(path) };
    if !full.exists() {
        return Err(format!("{} does not exist", full.display()));
    }
    if !full.is_dir() {
        return Err(format!("{} is a file, not a folder", full.display()));
    }
    Ok(full.canonicalize().unwrap_or(full))
}

/// Completes the folder word after `/folder `: the longest common start of the folders that match, with a slash if one
/// folder matches. None when nothing matches or nothing is added.
pub fn complete_folder(typed: &str, cwd: &Path, home: &str) -> Option<String> {
    let arg = typed.replace("\\ ", " ");
    let arg = arg.as_str();
    let (dir_text, stem) = match arg.rfind('/') {
        Some(i) => (&arg[..=i], &arg[i + 1..]),
        None => ("", arg),
    };
    let base = if dir_text.is_empty() { cwd.to_path_buf() } else {
        let p = PathBuf::from(word_path(dir_text, home));
        if p.is_absolute() { p } else { cwd.join(p) }
    };
    let mut names: Vec<String> = std::fs::read_dir(&base).ok()?.filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.starts_with(stem) && (stem.starts_with('.') || !n.starts_with('.')))
        .collect();
    names.sort();
    let first = names.first()?.clone();
    let mut common = first.clone();
    for n in &names[1..] {
        let keep = common.chars().zip(n.chars()).take_while(|(a, b)| a == b).count();
        common = common.chars().take(keep).collect();
    }
    let done = if names.len() == 1 { format!("{}{}/", dir_text, first) } else { format!("{}{}", dir_text, common) };
    if done == arg { None } else { Some(done.replace(' ', "\\ ")) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("penelope-drop-{}-{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("my folder")).unwrap();
        fs::write(dir.join("a b.txt"), "x").unwrap();
        fs::write(dir.join("c.txt"), "y").unwrap();
        dir
    }

    #[test]
    fn a_plain_path_is_a_drop() {
        let d = scratch("plain");
        let got = dropped(&format!("{}/c.txt", d.display()), "").unwrap();
        assert_eq!(got, vec![d.join("c.txt")]);
    }

    #[test]
    fn single_quotes_and_escaped_spaces_are_one_path() {
        let d = scratch("quotes");
        assert_eq!(dropped(&format!("'{}/a b.txt'", d.display()), "").unwrap(), vec![d.join("a b.txt")]);
        assert_eq!(dropped(&format!("{}/a\\ b.txt", d.display()), "").unwrap(), vec![d.join("a b.txt")]);
        assert_eq!(dropped(&format!("\"{}/a b.txt\"", d.display()), "").unwrap(), vec![d.join("a b.txt")]);
    }

    #[test]
    fn a_file_uri_loses_its_escapes() {
        let d = scratch("uri");
        assert_eq!(dropped(&format!("file://{}/a%20b.txt", d.display()), "").unwrap(), vec![d.join("a b.txt")]);
        assert_eq!(dropped(&format!("file://localhost{}/my%20folder", d.display()), "").unwrap(), vec![d.join("my folder")]);
    }

    #[test]
    fn several_paths_split_on_spaces_and_newlines() {
        let d = scratch("many");
        let text = format!("{0}/c.txt 'file://{0}/a%20b.txt'\n{0}/my\\ folder\r\n", d.display());
        let got = dropped(&text, "").unwrap();
        assert_eq!(got, vec![d.join("c.txt"), d.join("a b.txt"), d.join("my folder")]);
    }

    #[test]
    fn text_that_is_not_all_paths_stays_text() {
        let d = scratch("text");
        assert!(dropped("please read this file", "").is_none());
        assert!(dropped("src", "").is_none());
        assert!(dropped(&format!("{}/c.txt and more", d.display()), "").is_none());
        assert!(dropped(&format!("{}/missing.txt", d.display()), "").is_none());
        assert!(dropped("   ", "").is_none());
    }

    #[test]
    fn a_home_path_resolves_against_the_home_folder() {
        let d = scratch("home");
        assert_eq!(dropped("~/c.txt", &d.display().to_string()).unwrap(), vec![d.join("c.txt")]);
    }

    #[test]
    fn folder_paths_resolve_absolute_relative_and_home() {
        let d = scratch("folder");
        let home = d.display().to_string();
        let canon = |p: PathBuf| p.canonicalize().unwrap();
        assert_eq!(resolve_folder(&d.join("my folder").display().to_string(), Path::new("/"), "").unwrap(), canon(d.join("my folder")));
        assert_eq!(resolve_folder("my\\ folder", &d, "").unwrap(), canon(d.join("my folder")));
        assert_eq!(resolve_folder("'my folder'/..", &d, "").unwrap(), canon(d.clone()));
        assert_eq!(resolve_folder("~", Path::new("/"), &home).unwrap(), canon(d.clone()));
        assert_eq!(resolve_folder("~/my folder", Path::new("/"), &home).unwrap(), canon(d.join("my folder")));
        assert!(resolve_folder("nope", &d, "").unwrap_err().contains("does not exist"));
        assert!(resolve_folder("c.txt", &d, "").unwrap_err().contains("not a folder"));
    }

    #[test]
    fn folder_completion_extends_to_the_common_start() {
        let d = scratch("complete");
        fs::create_dir_all(d.join("alpha1")).unwrap();
        fs::create_dir_all(d.join("alpha2")).unwrap();
        assert_eq!(complete_folder("al", &d, "").as_deref(), Some("alpha"));
        assert_eq!(complete_folder("my", &d, "").as_deref(), Some("my\\ folder/"));
        assert_eq!(complete_folder("alpha", &d, ""), None);
        assert_eq!(complete_folder("zzz", &d, ""), None);
    }
}
