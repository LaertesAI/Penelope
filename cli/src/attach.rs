//! Mitos, version 1: tool results as attachments by reference.
//!
//! A result over `mitos.attach_over_bytes` (default 4,096) is stored once, content-addressed, below
//! `<root>/attachments/<sha256 prefix>/<sha256>`. The model gets one compact line with the name, type, size, a summary of
//! at most 200 characters and the number of links, and reads ranges with `attachment_read`. An MCP client that announced
//! the `attachments` feature gets an MCP `resource_link` item with the same facts in `_meta["laertes/mitos"]`, and reads
//! ranges with `mitos/fetch`. The summary is cut from the text itself, never written by a model.
//!
//! The same store serves three paths: Penelope's own tool loop (`for_model`), Penelope as an MCP server (`for_client`
//! and `fetch`) and Penelope as a client of another Mitos server (`compact_from_block`, `remember`).

use crate::config::Ctx;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

pub const URI_PREFIX: &str = "mitos://attachment/";
pub const FEATURE: &str = "attachments";
pub const TOOL: &str = "attachment_read";
pub const DEFAULT_ATTACH_OVER: u64 = 4_096;
pub const DEFAULT_FETCH_CAP: u64 = 16_384;
pub const DEFAULT_STORE_MAX: u64 = 500 * 1024 * 1024;
pub const SUMMARY_MAX: usize = 200;
pub const LINKS_MAX: usize = 50;

fn setting(ctx: &Ctx, key: &str, default: u64) -> u64 {
    ctx.cfg["mitos"][key].as_u64().unwrap_or(default)
}

/// Results over this many bytes become attachments. 0 switches attachments off.
pub fn threshold(ctx: &Ctx) -> u64 {
    setting(ctx, "attach_over_bytes", DEFAULT_ATTACH_OVER)
}

/// The most bytes one `mitos/fetch` or `attachment_read` call returns.
pub fn fetch_cap(ctx: &Ctx) -> usize {
    setting(ctx, "fetch_cap_bytes", DEFAULT_FETCH_CAP).max(256) as usize
}

fn store_max(ctx: &Ctx) -> u64 {
    setting(ctx, "store_max_bytes", DEFAULT_STORE_MAX)
}

/// One stored result and the facts a reader gets without reading it.
#[derive(Clone, Debug, PartialEq)]
pub struct Attachment {
    pub sha256: String,
    pub bytes: usize,
    pub mime: String,
    pub lines: Option<usize>,
    pub summary: String,
    pub level: u8,
    pub links: Vec<String>,
    pub name: String,
}

impl Attachment {
    pub fn uri(&self) -> String {
        format!("{}{}", URI_PREFIX, self.sha256)
    }

    /// `_meta["laertes/mitos"]` of the resource link. Text of level 3 (the person) keeps its summary and links to
    /// itself: the reader sees hash, size and type, and reads the content with a fetch.
    pub fn meta(&self) -> Value {
        let person = self.level >= 3;
        let mut m = json!({
            "sha256": self.sha256, "bytes": self.bytes, "mime": self.mime, "l": self.level,
            "summary": if person { "level 3 content, read it with attachment_read".to_string() } else { self.summary.clone() },
            "links": if person { Vec::<String>::new() } else { self.links.clone() },
        });
        if let Some(n) = self.lines {
            m["lines"] = json!(n);
        }
        m
    }

    /// The MCP content item that replaces the text.
    pub fn resource_link(&self) -> Value {
        json!({"type": "resource_link", "uri": self.uri(), "name": self.name, "mimeType": self.mime,
               "_meta": {crate::mitos::EXTENSION_ID: self.meta()}})
    }

    /// The one line a model sees.
    pub fn compact_line(&self, server: Option<&str>) -> String {
        let meta = self.meta();
        line_from(&self.uri(), &self.name, &meta, server)
    }
}

/// The compact line from a uri, a name and the Mitos meta of a link.
fn line_from(uri: &str, name: &str, meta: &Value, server: Option<&str>) -> String {
    let mime = meta["mime"].as_str().unwrap_or("application/octet-stream");
    let mut line = format!("[attachment {} · {} · {} bytes", name, mime, meta["bytes"].as_u64().unwrap_or(0));
    if let Some(n) = meta["lines"].as_u64() {
        line.push_str(&format!(" · {} lines", n));
    }
    let links = meta["links"].as_array().map(|l| l.len()).unwrap_or(0);
    line.push_str(&format!(" · {} links · l{}]", links, meta["l"].as_u64().unwrap_or(0)));
    let summary = meta["summary"].as_str().unwrap_or("");
    if !summary.is_empty() {
        line.push(' ');
        line.push_str(summary);
    }
    line.push_str(&format!(" → {} uri={}", TOOL, uri));
    if let Some(s) = server {
        line.push_str(&format!(" server={}", s));
    }
    line
}

// ---------- summary, links, hints ----------

/// At most `n` characters; a longer text ends in an ellipsis that counts toward `n`.
fn cut(text: &str, n: usize) -> String {
    if text.chars().count() <= n {
        return text.to_string();
    }
    let head: String = text.chars().take(n.saturating_sub(1)).collect();
    format!("{}…", head.trim_end())
}

fn is_text_mime(mime: &str) -> bool {
    mime.starts_with("text/") || mime.contains("json") || mime.contains("xml") || mime.contains("yaml") || mime.contains("javascript")
}

fn line_count(text: &str) -> usize {
    if text.is_empty() { 0 } else { text.matches('\n').count() + if text.ends_with('\n') { 0 } else { 1 } }
}

/// What the text looks like, in a few words: JSON, a diff, a log with errors, source code, or plain text.
fn structure_hint(text: &str, lines: usize) -> String {
    let t = text.trim_start();
    if (t.starts_with('{') || t.starts_with('[')) && text.len() < 5_000_000 {
        if let Ok(v) = serde_json::from_str::<Value>(text) {
            return match v {
                Value::Object(o) => format!("{} lines, json object, {} keys", lines, o.len()),
                Value::Array(a) => format!("{} lines, json array, {} items", lines, a.len()),
                _ => format!("{} lines, json", lines),
            };
        }
    }
    let files = text.matches("diff --git").count();
    if files > 0 {
        return format!("{} lines, diff, {} files", lines, files);
    }
    let (mut errors, mut warnings, mut defs) = (0usize, 0usize, 0usize);
    for l in text.lines() {
        let low = l.trim_start();
        if l.contains("error") || l.contains("FAILED") || l.contains("FAIL ") || l.contains("panicked") { errors += 1; }
        if l.starts_with("warning") { warnings += 1; }
        if low.starts_with("fn ") || low.starts_with("pub fn ") || low.starts_with("def ") || low.starts_with("class ")
            || low.starts_with("function ") || low.starts_with("struct ") || low.starts_with("impl ") { defs += 1; }
    }
    if errors + warnings >= 3 {
        return format!("{} lines, log, {} error lines, {} warnings", lines, errors, warnings);
    }
    if defs >= 3 {
        return format!("{} lines, source, {} definitions", lines, defs);
    }
    format!("{} lines, text", lines)
}

/// The summary of a text: the first non-empty lines, then the structure hint, at most `SUMMARY_MAX` characters.
pub fn summarize_text(text: &str) -> String {
    let lines = line_count(text);
    let hint = cut(&structure_hint(text, lines), 70);
    let room = SUMMARY_MAX.saturating_sub(hint.chars().count() + 3).max(40);
    let mut head = String::new();
    for l in text.lines().map(str::trim).filter(|l| !l.is_empty()).take(3) {
        if !head.is_empty() { head.push_str(" | "); }
        head.push_str(&cut(l, 70));
        if head.chars().count() >= room { break; }
    }
    let head = cut(&head, room);
    if head.is_empty() { cut(&hint, SUMMARY_MAX) } else { cut(&format!("{} · {}", head, hint), SUMMARY_MAX) }
}

/// The summary of binary content: type, size and, for a PNG, the picture size.
pub fn summarize_binary(mime: &str, data: &[u8]) -> String {
    if data.len() >= 24 && data.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]) {
        let w = u32::from_be_bytes([data[16], data[17], data[18], data[19]]);
        let h = u32::from_be_bytes([data[20], data[21], data[22], data[23]]);
        return format!("{} {}x{}, {} bytes", mime, w, h, data.len());
    }
    format!("{}, {} bytes", mime, data.len())
}

fn url_regex() -> &'static regex::Regex {
    static R: OnceLock<regex::Regex> = OnceLock::new();
    R.get_or_init(|| regex::Regex::new(r#"https?://[^\s<>"'`)\]}]+"#).unwrap())
}

fn path_regex() -> &'static regex::Regex {
    static R: OnceLock<regex::Regex> = OnceLock::new();
    R.get_or_init(|| regex::Regex::new(r#"(?:^|[\s"'(\[=:])((?:\.{1,2}/|/)?(?:[\w.\-]+/)+[\w.\-]*\.[A-Za-z0-9]{1,6}|/(?:[\w.\-]+/)+[\w.\-]+)(?::\d+)*"#).unwrap())
}

/// URLs and file paths in a text, in the order they first occur, without repeats, at most `LINKS_MAX`.
pub fn extract_links(text: &str) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    let mut push = |s: String, out: &mut Vec<String>| {
        if out.len() < LINKS_MAX && !s.is_empty() && seen.insert(s.clone()) { out.push(s); }
    };
    for m in url_regex().find_iter(text) {
        push(m.as_str().trim_end_matches(['.', ',', ';', ':', '!', '?']).to_string(), &mut out);
    }
    let without_urls = url_regex().replace_all(text, " ");
    for c in path_regex().captures_iter(&without_urls) {
        let p = c[1].trim_end_matches(['.', ',', ';']).to_string();
        if p.contains("//") { continue; }
        push(p, &mut out);
    }
    out
}

// ---------- store ----------

fn dir_of(ctx: &Ctx) -> PathBuf {
    ctx.root.join("attachments")
}

fn is_sha(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn data_path(ctx: &Ctx, sha: &str) -> PathBuf {
    dir_of(ctx).join(&sha[..2]).join(sha)
}

fn meta_path(ctx: &Ctx, sha: &str) -> PathBuf {
    dir_of(ctx).join(&sha[..2]).join(format!("{}.json", sha))
}

fn now_nanos() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0)
}

fn safe_name(tool: &str) -> String {
    let n: String = tool.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).take(30).collect();
    if n.trim_matches('-').is_empty() { "result".to_string() } else { n }
}

/// Stores bytes content-addressed and returns the facts about them. The same bytes give the same attachment and one
/// file; a second store raises the level to the higher of the two. Oldest entries are removed when the store is over
/// its limit; the entry just written stays.
pub fn store_bytes(ctx: &Ctx, tool: &str, mime: &str, level: u8, data: &[u8]) -> Result<Attachment, String> {
    let sha = crate::brief::sha256_hex(data);
    let text = if is_text_mime(mime) { std::str::from_utf8(data).ok() } else { None };
    let (lines, summary, links) = match text {
        Some(t) => (Some(line_count(t)), summarize_text(t), extract_links(t)),
        None => (None, summarize_binary(mime, data), Vec::new()),
    };
    let mut att = Attachment { sha256: sha.clone(), bytes: data.len(), mime: mime.to_string(), lines, summary, level,
                               links, name: format!("{}-{}", safe_name(tool), &sha[..8]) };
    let file = data_path(ctx, &sha);
    let sidecar = meta_path(ctx, &sha);
    let have = fs::metadata(&file).map(|m| m.len() == data.len() as u64).unwrap_or(false) && sidecar.exists();
    if have {
        // Deduplicated: one file. The stricter level of the two stays, and the first name and summary stay.
        if let Some(old) = read_sidecar(ctx, &sha) {
            att.level = att.level.max(old.level);
            if old.level != att.level {
                write_sidecar(ctx, &att, old_created(ctx, &sha))?;
            }
            att.name = old.name;
        }
        return Ok(att);
    }
    let dir = file.parent().ok_or("no folder")?;
    fs::create_dir_all(dir).map_err(|e| format!("attachment store: {}", e))?;
    let tmp = dir.join(format!("{}.tmp{}", sha, std::process::id()));
    fs::write(&tmp, data).map_err(|e| format!("attachment store: {}", e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // The person's own content stays readable by the owner only.
        let mode = if att.level >= 3 { 0o600 } else { 0o644 };
        let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(mode));
    }
    fs::rename(&tmp, &file).map_err(|e| format!("attachment store: {}", e))?;
    write_sidecar(ctx, &att, now_nanos())?;
    let _ = cleanup(ctx, Some(&sha));
    Ok(att)
}

pub fn store_text(ctx: &Ctx, tool: &str, level: u8, text: &str) -> Result<Attachment, String> {
    store_bytes(ctx, tool, "text/plain", level, text.as_bytes())
}

fn write_sidecar(ctx: &Ctx, a: &Attachment, created: u64) -> Result<(), String> {
    let v = json!({"sha256": a.sha256, "bytes": a.bytes, "mime": a.mime, "lines": a.lines, "summary": a.summary,
                   "l": a.level, "links": a.links, "name": a.name, "created": created});
    let path = meta_path(ctx, &a.sha256);
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    fs::write(&tmp, v.to_string()).map_err(|e| format!("attachment store: {}", e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(if a.level >= 3 { 0o600 } else { 0o644 }));
    }
    fs::rename(&tmp, &path).map_err(|e| format!("attachment store: {}", e))
}

fn old_created(ctx: &Ctx, sha: &str) -> u64 {
    fs::read_to_string(meta_path(ctx, sha)).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| v["created"].as_u64()).unwrap_or_else(now_nanos)
}

fn read_sidecar(ctx: &Ctx, sha: &str) -> Option<Attachment> {
    let v: Value = serde_json::from_str(&fs::read_to_string(meta_path(ctx, sha)).ok()?).ok()?;
    Some(Attachment {
        sha256: sha.to_string(),
        bytes: v["bytes"].as_u64()? as usize,
        mime: v["mime"].as_str()?.to_string(),
        lines: v["lines"].as_u64().map(|n| n as usize),
        summary: v["summary"].as_str().unwrap_or("").to_string(),
        level: v["l"].as_u64().unwrap_or(0) as u8,
        links: v["links"].as_array().map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_string)).collect()).unwrap_or_default(),
        name: v["name"].as_str().unwrap_or("attachment").to_string(),
    })
}

/// What a cleanup removed. `local` counts everything; `export` leaves level 3 out, so a report that leaves the
/// machine never says that the person's content existed or how big it was.
#[derive(Debug, Default, PartialEq)]
pub struct CleanupReport {
    pub removed: usize,
    pub freed_bytes: u64,
    pub level3_removed: usize,
    pub level3_freed_bytes: u64,
}

impl CleanupReport {
    pub fn local(&self) -> Value {
        json!({"removed": self.removed + self.level3_removed, "freed_bytes": self.freed_bytes + self.level3_freed_bytes,
               "level3_removed": self.level3_removed})
    }

    /// For a report that leaves the machine: no level 3 entry, not even in the totals.
    #[allow(dead_code)]
    pub fn export(&self) -> Value {
        json!({"removed": self.removed, "freed_bytes": self.freed_bytes})
    }
}

/// Removes the oldest attachments until the store fits `mitos.store_max_bytes`. The entry named in `keep` stays.
pub fn cleanup(ctx: &Ctx, keep: Option<&str>) -> CleanupReport {
    let mut report = CleanupReport::default();
    let max = store_max(ctx);
    let mut entries: Vec<(u64, String, u64, u8)> = Vec::new();
    let mut total = 0u64;
    let Ok(top) = fs::read_dir(dir_of(ctx)) else { return report };
    for sub in top.flatten() {
        let Ok(files) = fs::read_dir(sub.path()) else { continue };
        for f in files.flatten() {
            let name = f.file_name().to_string_lossy().to_string();
            if !is_sha(&name) { continue; }
            let size = f.metadata().map(|m| m.len()).unwrap_or(0);
            total += size;
            let side = read_sidecar(ctx, &name);
            let created = old_created(ctx, &name);
            entries.push((created, name, size, side.map(|s| s.level).unwrap_or(0)));
        }
    }
    if total <= max { return report; }
    entries.sort();
    for (_, sha, size, level) in entries {
        if total <= max { break; }
        if keep == Some(sha.as_str()) { continue; }
        let _ = fs::remove_file(data_path(ctx, &sha));
        let _ = fs::remove_file(meta_path(ctx, &sha));
        total = total.saturating_sub(size);
        if level >= 3 {
            report.level3_removed += 1;
            report.level3_freed_bytes += size;
        } else {
            report.removed += 1;
            report.freed_bytes += size;
        }
    }
    if report.removed + report.level3_removed > 0 {
        // The local log keeps the counts; it stays in the project folder.
        if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(dir_of(ctx).join("cleanup.log")) {
            use std::io::Write;
            let _ = writeln!(f, "{}", report.local());
        }
    }
    report
}

/// The stored bytes of a uri this store issued, after the hash check. A uri without the prefix, with a bad hash or
/// without an entry in the store is refused: the store reads only what `store_bytes` wrote.
pub fn load(ctx: &Ctx, uri: &str) -> Result<(Attachment, Vec<u8>), String> {
    let sha = uri.strip_prefix(URI_PREFIX).unwrap_or("");
    if !is_sha(sha) {
        return Err(format!("NOT fetched. {} is not an attachment uri of this server.", cut(uri, 80)));
    }
    let Some(att) = read_sidecar(ctx, sha) else {
        return Err(format!("NOT fetched. This server did not issue {}.", uri));
    };
    let data = fs::read(data_path(ctx, sha)).map_err(|_| format!("NOT fetched. The content of {} was removed from the store.", uri))?;
    if crate::brief::sha256_hex(&data) != sha {
        return Err(format!("NOT fetched. The stored bytes of {} do not match their hash.", uri));
    }
    Ok((att, data))
}

/// The origin level of a stored attachment, or `None` for an unknown uri.
pub fn level_of(ctx: &Ctx, uri: &str) -> Option<u8> {
    let sha = uri.strip_prefix(URI_PREFIX)?;
    if !is_sha(sha) { return None; }
    read_sidecar(ctx, sha).map(|a| a.level)
}

// ---------- fetch ----------

fn range_of(params: &Value, key: &str) -> Result<Option<(u64, u64)>, String> {
    let Some(v) = params.get(key).filter(|v| !v.is_null()) else { return Ok(None) };
    let a = v.as_array().filter(|a| a.len() == 2);
    match a.and_then(|a| Some((a[0].as_u64()?, a[1].as_u64()?))) {
        Some((s, e)) if e >= s => Ok(Some((s, e))),
        _ => Err(format!("NOT fetched. {} must be [start, end] with two integers and end not before start.", key)),
    }
}

/// `mitos/fetch`: the requested range of one attachment, at most `fetch_cap` bytes per call. `lines` is [first, last],
/// counted from 1, both included. `bytes` is [start, end), counted from 0. Without a range the call returns the first
/// `fetch_cap` bytes. A cut answer says so and names the next range.
pub fn fetch(ctx: &Ctx, params: &Value) -> Result<Value, String> {
    let uri = params["uri"].as_str().unwrap_or("");
    let (att, data) = load(ctx, uri)?;
    let lines = range_of(params, "lines")?;
    let bytes = range_of(params, "bytes")?;
    if lines.is_some() && bytes.is_some() {
        return Err("NOT fetched. Give lines or bytes, not both.".to_string());
    }
    let cap = fetch_cap(ctx);
    let total = data.len();
    let is_text = is_text_mime(&att.mime) && std::str::from_utf8(&data).is_ok();
    let (start, end, truncated, next, range) = if let Some((first, last)) = lines {
        if !is_text { return Err(format!("NOT fetched. {} is binary and has no lines; ask for bytes.", att.mime)); }
        if first == 0 { return Err("NOT fetched. Lines count from 1.".to_string()); }
        let text = std::str::from_utf8(&data).unwrap_or("");
        let mut offset = 0usize;
        let mut start: Option<usize> = None;
        let mut end = 0usize;
        let mut next: Option<Value> = None;
        for (i, l) in text.split_inclusive('\n').enumerate() {
            let n = (i + 1) as u64;
            if n > last { break; }
            if n >= first {
                let got = start.map(|s| end - s).unwrap_or(0);
                if got + l.len() > cap {
                    if start.is_none() {
                        // One line longer than the cap: cut it at a character boundary; the rest is a byte range.
                        let mut c = cap.min(l.len());
                        while c > 0 && !l.is_char_boundary(c) { c -= 1; }
                        start = Some(offset);
                        end = offset + c;
                        next = Some(json!({"bytes": [offset + c, offset + l.len()]}));
                    } else {
                        next = Some(json!({"lines": [n, last]}));
                    }
                    break;
                }
                if start.is_none() { start = Some(offset); }
                end = offset + l.len();
            }
            offset += l.len();
        }
        let s = start.unwrap_or(total);
        let e = if start.is_some() { end } else { total };
        (s, e, next.is_some(), next, json!({"lines": [first, last]}))
    } else {
        let (s, e) = bytes.unwrap_or((0, cap as u64));
        let s = (s as usize).min(total);
        let want_end = (e as usize).min(total);
        let e = want_end.min(s + cap);
        let next = if e < want_end { Some(json!({"bytes": [e, want_end]})) } else if bytes.is_none() && e < total { Some(json!({"bytes": [e, total]})) } else { None };
        (s, e, e < want_end || (bytes.is_none() && e < total), next, json!({"bytes": [s, e]}))
    };
    let slice = &data[start.min(total)..end.min(total)];
    let content = if is_text {
        json!([{"type": "text", "text": String::from_utf8_lossy(slice)}])
    } else {
        json!([{"type": "resource", "resource": {"uri": uri, "mimeType": att.mime, "blob": crate::brief::b64_encode(slice)}}])
    };
    let mut meta = json!({"sha256": att.sha256, "bytes": total, "mime": att.mime, "l": att.level, "range": range,
                          "returned": slice.len(), "truncated": truncated});
    if let Some(n) = next { meta["next"] = n; }
    Ok(json!({"content": content, "isError": false, "_meta": {crate::mitos::EXTENSION_ID: meta}}))
}

/// The text of a fetch answer with one trailer line: what range came back and where the next one starts.
pub fn render_fetch(answer: &Value) -> String {
    let meta = &answer["_meta"][crate::mitos::EXTENSION_ID];
    let mut text = String::new();
    for block in answer["content"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
        if let Some(t) = block["text"].as_str() { text.push_str(t); }
        if block["type"] == "resource" { text.push_str(&format!("[binary range, {} bytes, base64 in the answer]", meta["returned"])); }
    }
    let mut trailer = format!("[{} of {} bytes", meta["returned"], meta["bytes"]);
    if meta["truncated"] == json!(true) {
        trailer.push_str(&format!(" · cut at the cap · next: {}", meta["next"]));
    }
    trailer.push(']');
    if !text.ends_with('\n') && !text.is_empty() { text.push('\n'); }
    format!("{}{}", text, trailer)
}

// ---------- the model's tool ----------

static REMOTE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();

fn remote() -> &'static Mutex<HashMap<String, String>> {
    REMOTE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Remembers which MCP server issued a uri, so `attachment_read` asks that server.
pub fn remember(uri: &str, server: &str) {
    remote().lock().unwrap_or_else(|e| e.into_inner()).insert(uri.to_string(), server.to_string());
}

fn server_of(uri: &str) -> Option<String> {
    remote().lock().unwrap_or_else(|e| e.into_inner()).get(uri).cloned()
}

/// `attachment_read`: a range of an attachment. A uri that a plugin server issued goes to that server through
/// `mitos/fetch`; every other uri is read from the local store.
pub fn read_tool(ctx: &Ctx, a: &Value) -> Result<String, String> {
    let uri = a["uri"].as_str().unwrap_or("");
    let mut params = json!({"uri": uri});
    for k in ["lines", "bytes"] {
        if !a[k].is_null() { params[k] = a[k].clone(); }
    }
    let server = a["server"].as_str().filter(|s| !s.is_empty()).map(str::to_string).or_else(|| server_of(uri));
    let answer = match server {
        Some(s) => crate::mcpclient::fetch_attachment(ctx, &s, &params)?,
        None => fetch(ctx, &params)?,
    };
    Ok(crate::tools::guard(ctx, &render_fetch(&answer)))
}

/// The origin level of the answer of a call. `attachment_read` answers with the level of the attachment it reads.
pub fn result_level(ctx: &Ctx, tool: &str, args: &Value) -> u8 {
    if tool == TOOL {
        if let Some(l) = args["uri"].as_str().and_then(|u| level_of(ctx, u)) { return l; }
    }
    crate::mitos::origin_level(tool)
}

// ---------- the three paths ----------

/// The tools whose results are never replaced: the read tool itself, and answers that are short by design.
fn never(tool: &str) -> bool {
    matches!(tool, "attachment_read" | "brief_put" | "ask_user" | "plan" | "write" | "edit" | "undo")
}

/// The tools of Penelope's own loop whose long results become attachments. They produce long text and the model
/// rarely needs all of it; a report that the model must read whole (a cluster report, a scout answer) stays inline.
fn local_tool(tool: &str) -> bool {
    matches!(tool, "run" | "run_detail" | "read" | "grep" | "find" | "tree" | "outline" | "git_diff" | "json_query" | "http_request"
                   | "job_read" | "lsp")
}

fn over_threshold(ctx: &Ctx, text: &str) -> bool {
    let t = threshold(ctx);
    t > 0 && text.len() as u64 > t
}

/// A refusal and the stop line of the test loop reach the model whole.
fn must_stay_inline(text: &str) -> bool {
    text.starts_with("NOT ") || text.contains(crate::tools::TEST_STOP)
}

/// Penelope's own tool loop: a long result of a built-in tool becomes one compact line. The text is stored
/// under the origin level of the tool. Any failure to store leaves the text as it is.
pub fn for_model(ctx: &Ctx, tool: &str, text: String) -> String {
    if !local_tool(tool) || !over_threshold(ctx, &text) || must_stay_inline(&text) {
        return text;
    }
    match store_text(ctx, tool, crate::mitos::origin_level(tool), &text) {
        Ok(att) => att.compact_line(None),
        Err(_) => text,
    }
}

/// Penelope as an MCP server for a client that announced `attachments`: the content of a `tools/call` result.
pub fn for_client(ctx: &Ctx, tool: &str, text: &str, attach: bool) -> Value {
    if attach && !never(tool) && over_threshold(ctx, text) {
        if let Ok(att) = store_text(ctx, tool, crate::mitos::origin_level(tool), text) {
            return json!([att.resource_link()]);
        }
    }
    json!([{"type": "text", "text": text}])
}

/// Penelope as a client: the compact line of a `resource_link` that carries Mitos meta, or `None` for a plain link.
pub fn compact_from_block(block: &Value, server: Option<&str>) -> Option<String> {
    let meta = block["_meta"].get(crate::mitos::EXTENSION_ID)?;
    meta["sha256"].as_str().filter(|s| is_sha(s))?;
    let uri = block["uri"].as_str().filter(|u| !u.is_empty())?;
    let name = block["name"].as_str().filter(|n| !n.is_empty()).unwrap_or("attachment");
    if let Some(s) = server { remember(uri, s); }
    Some(line_from(uri, name, meta, server))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(name: &str) -> (Ctx, PathBuf) {
        let dir = std::env::temp_dir().join(format!("penelope-attach-{}-{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        (crate::config::load(&dir.join(".penelope")), dir)
    }

    fn big(lines: usize) -> String {
        (1..=lines).map(|i| format!("line {} of the output, see src/file{}.rs and https://example.org/p{}\n", i, i % 3, i % 4)).collect()
    }

    #[test]
    fn a_result_over_the_threshold_becomes_a_resource_link_with_meta() {
        let (c, dir) = ctx("shape");
        let text = big(120);
        assert!(text.len() > 4096);
        let content = for_client(&c, "run", &text, true);
        let item = &content[0];
        assert_eq!(item["type"], "resource_link");
        let sha = crate::brief::sha256_hex(text.as_bytes());
        assert_eq!(item["uri"], format!("{}{}", URI_PREFIX, sha));
        let m = &item["_meta"][crate::mitos::EXTENSION_ID];
        assert_eq!(m["sha256"], sha);
        assert_eq!(m["bytes"], text.len());
        assert_eq!(m["mime"], "text/plain");
        assert_eq!(m["lines"], 120);
        assert_eq!(m["l"], 0, "run is outside text");
        assert!(m["summary"].as_str().unwrap().chars().count() <= SUMMARY_MAX);
        assert!(m["summary"].as_str().unwrap().starts_with("line 1 of the output"));
        let links: Vec<&str> = m["links"].as_array().unwrap().iter().filter_map(|v| v.as_str()).collect();
        assert!(links.contains(&"https://example.org/p1") && links.contains(&"src/file0.rs"), "{:?}", links);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_result_at_or_under_the_threshold_stays_inline() {
        let (c, dir) = ctx("inline");
        let exact = "x".repeat(4096);
        assert_eq!(for_client(&c, "run", &exact, true), json!([{"type": "text", "text": exact}]));
        let over = "x".repeat(4097);
        assert_eq!(for_client(&c, "run", &over, true)[0]["type"], "resource_link");
        let mut off = ctx("inline-off");
        off.0.cfg["mitos"]["attach_over_bytes"] = json!(0);
        assert_eq!(for_client(&off.0, "run", &over, true)[0]["type"], "text");
        let mut high = ctx("inline-high");
        high.0.cfg["mitos"]["attach_over_bytes"] = json!(100_000);
        assert_eq!(for_client(&high.0, "run", &over, true)[0]["type"], "text");
        for d in [dir, off.1, high.1] { let _ = fs::remove_dir_all(&d); }
    }

    #[test]
    fn a_v0_client_gets_the_text_as_before() {
        let (c, dir) = ctx("v0");
        let text = big(200);
        assert_eq!(for_client(&c, "run", &text, false), json!([{"type": "text", "text": text}]));
        assert!(!dir_of(&c).exists(), "nothing is stored for a v0 client");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_manifest_names_version_one_and_the_feature() {
        let m = crate::mitos::manifest();
        assert_eq!(m["version"], 1);
        assert!(m["features"].as_array().unwrap().contains(&json!(FEATURE)));
        assert!(crate::mitos::attachments_announced(&json!({"capabilities": {"experimental": {crate::mitos::EXTENSION_ID: m}}})));
        let v0 = json!({"capabilities": {"experimental": {crate::mitos::EXTENSION_ID: {"version": 0, "features": ["index"]}}}});
        assert!(crate::mitos::announced(&v0) && !crate::mitos::attachments_announced(&v0));
        assert!(!crate::mitos::attachments_announced(&json!({"capabilities": {}})));
    }

    #[test]
    fn the_hash_is_checked_and_a_changed_file_is_refused() {
        let (c, dir) = ctx("hash");
        let att = store_text(&c, "run", 0, &big(50)).unwrap();
        assert!(load(&c, &att.uri()).is_ok());
        fs::write(data_path(&c, &att.sha256), "tampered").unwrap();
        let err = load(&c, &att.uri()).unwrap_err();
        assert!(err.contains("do not match their hash"), "{}", err);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_line_range_returns_only_those_lines() {
        let (c, dir) = ctx("lines");
        let att = store_text(&c, "run", 0, &big(100)).unwrap();
        let answer = fetch(&c, &json!({"uri": att.uri(), "lines": [10, 12]})).unwrap();
        let text = answer["content"][0]["text"].as_str().unwrap();
        assert_eq!(text.lines().count(), 3);
        assert!(text.starts_with("line 10 of") && text.trim_end().ends_with("p0") || text.contains("line 12 of"), "{}", text);
        assert_eq!(answer["_meta"][crate::mitos::EXTENSION_ID]["truncated"], false);
        assert_eq!(answer["_meta"][crate::mitos::EXTENSION_ID]["range"]["lines"], json!([10, 12]));
        let rendered = render_fetch(&answer);
        assert!(rendered.contains("line 11 of") && rendered.contains("of "), "{}", rendered);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_byte_range_returns_those_bytes_and_a_range_past_the_end_is_empty() {
        let (c, dir) = ctx("bytes");
        let att = store_text(&c, "run", 0, &"abcdefghij".repeat(600)).unwrap();
        let a = fetch(&c, &json!({"uri": att.uri(), "bytes": [5, 15]})).unwrap();
        assert_eq!(a["content"][0]["text"], "fghijabcde");
        let past = fetch(&c, &json!({"uri": att.uri(), "bytes": [9000, 9100]})).unwrap();
        assert_eq!(past["_meta"][crate::mitos::EXTENSION_ID]["returned"], 0);
        assert!(fetch(&c, &json!({"uri": att.uri(), "bytes": [9, 3]})).is_err());
        assert!(fetch(&c, &json!({"uri": att.uri(), "bytes": [0, 3], "lines": [1, 2]})).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_cap_limits_one_call_and_names_the_next_range() {
        let (c, dir) = ctx("cap");
        let text = big(1000);
        let att = store_text(&c, "run", 0, &text).unwrap();
        let cap = fetch_cap(&c);
        assert_eq!(cap, 16_384);
        // No range: the first cap bytes.
        let first = fetch(&c, &json!({"uri": att.uri()})).unwrap();
        let m = &first["_meta"][crate::mitos::EXTENSION_ID];
        assert_eq!(m["returned"], cap);
        assert_eq!(m["truncated"], true);
        assert_eq!(m["next"]["bytes"], json!([cap, text.len()]));
        // A byte range over the cap is cut.
        let wide = fetch(&c, &json!({"uri": att.uri(), "bytes": [0, 100_000]})).unwrap();
        assert_eq!(wide["_meta"][crate::mitos::EXTENSION_ID]["returned"], cap);
        // A line range over the cap ends on a whole line and names the line to resume with.
        let lines = fetch(&c, &json!({"uri": att.uri(), "lines": [1, 1000]})).unwrap();
        let lm = &lines["_meta"][crate::mitos::EXTENSION_ID];
        let body = lines["content"][0]["text"].as_str().unwrap();
        assert!(body.len() <= cap && body.ends_with('\n'), "{}", body.len());
        assert_eq!(lm["truncated"], true);
        let resume = lm["next"]["lines"][0].as_u64().unwrap() as usize;
        assert_eq!(resume, body.lines().count() + 1);
        // The config sets the cap.
        let mut small = ctx("cap-small");
        small.0.cfg["mitos"]["fetch_cap_bytes"] = json!(1000);
        let att2 = store_text(&small.0, "run", 0, &text).unwrap();
        assert_eq!(fetch(&small.0, &json!({"uri": att2.uri()})).unwrap()["_meta"][crate::mitos::EXTENSION_ID]["returned"], 1000);
        for d in [dir, small.1] { let _ = fs::remove_dir_all(&d); }
    }

    #[test]
    fn a_foreign_uri_is_refused() {
        let (c, dir) = ctx("foreign");
        let att = store_text(&c, "run", 0, &big(10)).unwrap();
        for uri in ["file:///etc/passwd", "mitos://attachment/../../etc/passwd", "mitos://attachment/abc",
                    &format!("{}{}", URI_PREFIX, "0".repeat(64)), &format!("{}{}", URI_PREFIX, att.sha256.to_uppercase()), ""] {
            let err = fetch(&c, &json!({"uri": uri})).unwrap_err();
            assert!(err.starts_with("NOT fetched."), "{}: {}", uri, err);
        }
        // A file in the store folder that no store call wrote has no sidecar and is refused.
        let sha = crate::brief::sha256_hex(b"planted");
        fs::create_dir_all(data_path(&c, &sha).parent().unwrap()).unwrap();
        fs::write(data_path(&c, &sha), "planted").unwrap();
        assert!(fetch(&c, &json!({"uri": format!("{}{}", URI_PREFIX, sha)})).unwrap_err().contains("did not issue"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_same_content_is_stored_once() {
        let (c, dir) = ctx("dedup");
        let text = big(80);
        let a = store_text(&c, "run", 0, &text).unwrap();
        let b = store_text(&c, "grep", 0, &text).unwrap();
        assert_eq!(a.sha256, b.sha256);
        assert_eq!(b.name, a.name, "the first name stays");
        let files: Vec<_> = fs::read_dir(data_path(&c, &a.sha256).parent().unwrap()).unwrap().flatten()
            .filter(|f| !f.file_name().to_string_lossy().ends_with(".json")).collect();
        assert_eq!(files.len(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_oldest_attachments_go_first_when_the_store_is_over_its_limit() {
        let (mut c, dir) = ctx("cleanup");
        c.cfg["mitos"]["store_max_bytes"] = json!(5000);
        let texts: Vec<String> = (0..4).map(|i| format!("{}{}", i, "z".repeat(1999))).collect();
        let atts: Vec<Attachment> = texts.iter().map(|t| store_text(&c, "run", 0, t).unwrap()).collect();
        // 4 x 2000 bytes over a 5000 limit: the two oldest are gone, the two newest stay.
        assert!(load(&c, &atts[0].uri()).is_err() && load(&c, &atts[1].uri()).is_err());
        assert!(load(&c, &atts[2].uri()).is_ok() && load(&c, &atts[3].uri()).is_ok());
        // The entry just written stays even when it alone is over the limit.
        c.cfg["mitos"]["store_max_bytes"] = json!(100);
        let last = store_text(&c, "run", 0, &"q".repeat(3000)).unwrap();
        assert!(load(&c, &last.uri()).is_ok());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn level_three_content_stays_in_the_project_and_out_of_exported_reports() {
        let (mut c, dir) = ctx("level3");
        let secret = store_text(&c, "memory_go", 3, &format!("my notes {}", "n".repeat(2000))).unwrap();
        assert_eq!(secret.level, 3);
        // Below the project root only, and readable by the owner only.
        assert!(data_path(&c, &secret.sha256).starts_with(&c.root));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(data_path(&c, &secret.sha256)).unwrap().permissions().mode() & 0o777, 0o600);
            assert_eq!(fs::metadata(meta_path(&c, &secret.sha256)).unwrap().permissions().mode() & 0o777, 0o600);
        }
        // The meta a reader gets has hash, size and type, no summary and no links.
        let m = secret.meta();
        assert_eq!(m["l"], 3);
        assert_eq!(m["links"], json!([]));
        assert!(!m["summary"].as_str().unwrap().contains("my notes"));
        // Cleanup removes it; the exported report says nothing about it, the local one does.
        std::thread::sleep(std::time::Duration::from_millis(2));
        let other = store_text(&c, "run", 0, &"p".repeat(2500)).unwrap();
        c.cfg["mitos"]["store_max_bytes"] = json!(100);
        let r = cleanup(&c, None);
        assert_eq!((r.level3_removed, r.removed), (1, 1));
        assert!(load(&c, &secret.uri()).is_err() && load(&c, &other.uri()).is_err());
        assert_eq!(r.export(), json!({"removed": 1, "freed_bytes": 2500}));
        assert!(!r.export().to_string().contains("level3"));
        assert_eq!(r.local()["level3_removed"], 1);
        let log = fs::read_to_string(dir_of(&c).join("cleanup.log")).unwrap();
        assert!(log.contains("level3_removed") && !log.contains(&secret.sha256), "{}", log);
        c.cfg["mitos"]["store_max_bytes"] = json!(DEFAULT_STORE_MAX);
        // The same bytes at a lower level never lower the stored level.
        let text = "same text ".repeat(100);
        let hi = store_text(&c, "memory_go", 3, &text).unwrap();
        let lo = store_text(&c, "run", 0, &text).unwrap();
        assert_eq!((hi.level, lo.level), (3, 3));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_result_level_follows_the_tool_and_the_attachment() {
        let (c, dir) = ctx("levels");
        let att = store_text(&c, "memory_go", 3, &big(60)).unwrap();
        assert_eq!(result_level(&c, TOOL, &json!({"uri": att.uri()})), 3);
        assert_eq!(result_level(&c, "run", &json!({})), 0);
        assert_eq!(result_level(&c, "read", &json!({})), 1);
        let item = &for_client(&c, "memory_go", &big(90), true)[0];
        assert_eq!(item["_meta"][crate::mitos::EXTENSION_ID]["l"], 3);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_model_sees_one_compact_line() {
        let (c, dir) = ctx("compact");
        let text = big(150);
        let out = for_model(&c, "grep", text.clone());
        assert_eq!(out.lines().count(), 1, "{}", out);
        assert!(out.starts_with("[attachment grep-"), "{}", out);
        assert!(out.contains("text/plain") && out.contains(&format!("{} bytes", text.len())) && out.contains("150 lines"), "{}", out);
        assert!(out.contains(" links · l1]"), "{}", out);
        assert!(out.contains("line 1 of the output"), "{}", out);
        assert!(out.contains("attachment_read uri=mitos://attachment/"), "{}", out);
        assert!(out.len() < 700, "{} bytes", out.len());
        // Short results, refusals and the test stop line stay whole.
        assert_eq!(for_model(&c, "grep", "short".to_string()), "short");
        let refused = format!("NOT called. {}", "x".repeat(6000));
        assert_eq!(for_model(&c, "run", refused.clone()), refused);
        let stop = format!("{}\n{}: STOP.", "y".repeat(6000), crate::tools::TEST_STOP);
        assert_eq!(for_model(&c, "run", stop.clone()), stop);
        // A tool whose report the model must read whole stays inline.
        assert_eq!(for_model(&c, "scout", text.clone()), text);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn attachment_read_returns_the_range_of_a_local_attachment() {
        let (c, dir) = ctx("read-tool");
        let text = big(150);
        let line = for_model(&c, "run", text.clone());
        let uri = line.split("uri=").nth(1).unwrap().split_whitespace().next().unwrap().to_string();
        let got = read_tool(&c, &json!({"uri": uri, "lines": [3, 4]})).unwrap();
        assert!(got.starts_with("line 3 of the output") && got.contains("line 4 of the output"), "{}", got);
        assert!(!got.contains("line 5 of the output") && got.contains("[") , "{}", got);
        assert!(read_tool(&c, &json!({"uri": "mitos://attachment/nothing"})).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_image_and_a_pdf_are_stored_as_binary_with_size_and_type() {
        let (c, dir) = ctx("binary");
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13, b'I', b'H', b'D', b'R'];
        png.extend_from_slice(&640u32.to_be_bytes());
        png.extend_from_slice(&480u32.to_be_bytes());
        png.extend(std::iter::repeat_n(7u8, 5000));
        let att = store_bytes(&c, "screen", "image/png", 0, &png).unwrap();
        assert_eq!(att.lines, None);
        assert_eq!(att.summary, "image/png 640x480, 5024 bytes");
        assert!(att.links.is_empty());
        let pdf = store_bytes(&c, "fetch", "application/pdf", 0, &vec![b'%'; 6000]).unwrap();
        assert_eq!(pdf.summary, "application/pdf, 6000 bytes");
        // Bytes come back as a base64 blob, lines are refused.
        let got = fetch(&c, &json!({"uri": att.uri(), "bytes": [0, 8]})).unwrap();
        assert_eq!(got["content"][0]["resource"]["blob"], crate::brief::b64_encode(&png[..8]));
        assert!(fetch(&c, &json!({"uri": att.uri(), "lines": [1, 2]})).unwrap_err().contains("binary"));
        let link = att.resource_link();
        assert_eq!(link["mimeType"], "image/png");
        assert!(link["_meta"][crate::mitos::EXTENSION_ID].get("lines").is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_summary_has_the_first_lines_and_a_hint_within_200_characters() {
        let json_text = format!("{{{}}}", (0..300).map(|i| format!("\"key{}\": {}", i, i)).collect::<Vec<_>>().join(",\n"));
        let s = summarize_text(&json_text);
        assert!(s.chars().count() <= SUMMARY_MAX && s.contains("json object, 300 keys"), "{}", s);
        let log = format!("{}error: a\nerror: b\nerror: c\nwarning: d\n", "ok\n".repeat(2000));
        assert!(summarize_text(&log).contains("log, 3 error lines, 1 warnings"), "{}", summarize_text(&log));
        let diff = format!("diff --git a/x b/x\n{}", "+line\n".repeat(900));
        assert!(summarize_text(&diff).contains("diff, 1 files"));
        let long_line = "w".repeat(5000);
        assert!(summarize_text(&long_line).chars().count() <= SUMMARY_MAX);
        let wide = "ä".repeat(5000);
        assert!(summarize_text(&wide).chars().count() <= SUMMARY_MAX);
    }

    #[test]
    fn links_are_deduplicated_and_limited_to_fifty() {
        let text = "see https://a.example/x, https://a.example/x and src/main.rs:12:3 then src/main.rs again, (./docs/MITOS.md).";
        assert_eq!(extract_links(text), vec!["https://a.example/x", "src/main.rs", "./docs/MITOS.md"]);
        let many: String = (0..80).map(|i| format!("https://e.example/{}\n", i)).collect();
        let links = extract_links(&many);
        assert_eq!(links.len(), LINKS_MAX);
        assert_eq!(links[0], "https://e.example/0");
        assert!(extract_links("a plain sentence and/or nothing").is_empty());
    }

    #[test]
    fn a_resource_link_with_meta_becomes_a_compact_line_and_remembers_its_server() {
        let (c, dir) = ctx("client");
        let att = store_text(&c, "run", 0, &big(90)).unwrap();
        let block = att.resource_link();
        let line = compact_from_block(&block, Some("docs-server")).unwrap();
        assert!(line.starts_with("[attachment run-") && line.contains("90 lines") && line.contains("server=docs-server"), "{}", line);
        assert_eq!(server_of(&att.uri()).as_deref(), Some("docs-server"));
        // A link without Mitos meta is no attachment.
        assert!(compact_from_block(&json!({"type": "resource_link", "uri": "https://x.example/a", "name": "a"}), Some("s")).is_none());
        assert!(compact_from_block(&json!({"type": "resource_link", "uri": "u", "_meta": {"laertes/mitos": {"sha256": "short"}}}), None).is_none());
        let _ = fs::remove_dir_all(&dir);
    }
}
