//! The brief extension `io.github.laertesai/brief`.
//!
//! A brief is one ordinary MCP argument. A model writes the short form
//! `{"task": "...", "attach": ["link:URL", "file:NAME", "image:NAME", "context:NAME"]}`. The client of a Laertes
//! server that announced the extension replaces every name by an item with size and SHA-256. A small text file travels
//! as `text`, a small binary as `data`, a file over 256 KB as `upload` (sent before the call through `brief_put`), and
//! a link or a file reference as `uri`.
//!
//! The server side reads a file only inside the project and plugin folders it was given, and checks an HTTP host
//! against its allow list before it opens a link. Foreign MCP servers never see this module: without the announced
//! extension the client leaves every argument untouched.

use crate::config::Ctx;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Keeps two uploads of the same bytes apart in one session.
static NEXT_UPLOAD: AtomicU64 = AtomicU64::new(0);

/// The reverse-DNS name of the extension.
pub const EXTENSION_ID: &str = "io.github.laertesai/brief";
/// The version of the brief format.
pub const VERSION: u64 = 1;
/// The largest content that travels inline in a tool call.
pub const MAX_INLINE: usize = 256 * 1024;
/// The largest content of one item, inline or uploaded.
pub const MAX_ITEM: usize = 4 * 1024 * 1024;
/// The most attachments of one brief.
pub const MAX_ITEMS: usize = 16;
/// The longest task text.
pub const MAX_TASK: usize = 2_000;
/// The bytes of one upload chunk before base64.
pub const CHUNK: usize = 96 * 1024;
/// The kinds an attachment may have.
pub const KINDS: &[&str] = &["image", "file", "context", "link"];

/// The settings a server may announce next to the extension name.
#[derive(Clone, Debug)]
pub struct Extension {
    pub version: u64,
    pub max_inline: usize,
    pub max_item: usize,
    pub max_items: usize,
    pub upload_tool: String,
}

impl Default for Extension {
    fn default() -> Self {
        Extension { version: VERSION, max_inline: MAX_INLINE, max_item: MAX_ITEM, max_items: MAX_ITEMS,
                    upload_tool: "brief_put".to_string() }
    }
}

/// The announcement a server puts under `capabilities.experimental`.
pub fn manifest() -> Value {
    json!({
        "version": VERSION,
        "kinds": KINDS,
        "maxInlineBytes": MAX_INLINE,
        "maxItemBytes": MAX_ITEM,
        "maxItems": MAX_ITEMS,
        "ways": ["text", "data", "upload", "uri"],
        "uploadTool": "brief_put"
    })
}

/// The capabilities of Penelope's own MCP server. The same map is offered under `extensions`, the name the later
/// MCP revisions use.
pub fn capabilities() -> Value {
    let mut experimental = Map::new();
    experimental.insert(EXTENSION_ID.to_string(), manifest());
    let mut extensions = Map::new();
    extensions.insert(EXTENSION_ID.to_string(), manifest());
    json!({"tools": {"listChanged": false}, "experimental": experimental, "extensions": extensions})
}

/// Reads the extension from the answer to `initialize`. A server that does not announce it returns `None`; every
/// caller keeps its old path in that case.
pub fn extension_of(initialize: &Value) -> Option<Extension> {
    let caps = initialize.get("capabilities")?;
    let raw = caps.get("experimental").and_then(|m| m.get(EXTENSION_ID))
        .or_else(|| caps.get("extensions").and_then(|m| m.get(EXTENSION_ID)))?;
    if raw.is_null() || raw == &Value::Bool(false) {
        return None;
    }
    Some(Extension {
        version: raw["version"].as_u64().unwrap_or(VERSION),
        max_inline: raw["maxInlineBytes"].as_u64().map(|n| n as usize).unwrap_or(MAX_INLINE).min(MAX_INLINE),
        max_item: raw["maxItemBytes"].as_u64().map(|n| n as usize).unwrap_or(MAX_ITEM).min(MAX_ITEM),
        max_items: raw["maxItems"].as_u64().map(|n| n as usize).unwrap_or(MAX_ITEMS).min(MAX_ITEMS),
        upload_tool: raw["uploadTool"].as_str().map(str::to_string).unwrap_or_else(|| "brief_put".to_string()),
    })
}

/// One item that is too large for the inline limit. The client sends it in chunks through the upload tool before the
/// real tool call.
#[derive(Clone, Debug)]
pub struct Upload {
    pub id: String,
    pub data: Vec<u8>,
}

/// The packed argument and the uploads that must reach the server first.
#[derive(Clone, Debug)]
pub struct Packed {
    pub argument: Value,
    pub uploads: Vec<Upload>,
}

/// `image:shot.png` -> `("image", "shot.png")`. A link keeps its own colons.
pub fn split_ref(entry: &str) -> Result<(&str, &str), String> {
    let (kind, rest) = entry.split_once(':')
        .ok_or_else(|| format!("attachment '{}' is not kind:name", entry))?;
    let kind = kind.trim();
    if !KINDS.contains(&kind) {
        return Err(format!("attachment '{}' is not kind:name with kind in {}", entry, KINDS.join(", ")));
    }
    let rest = rest.trim();
    if rest.is_empty() {
        return Err(format!("attachment '{}' names no file or link", entry));
    }
    Ok((kind, rest))
}

/// Standard base64 with padding. No crate is added for it.
pub fn b64_encode(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        out.push(ALPHABET[(b0 >> 2) as usize] as char);
        out.push(ALPHABET[(((b0 & 0x03) << 4) | (b1 >> 4)) as usize] as char);
        out.push(if chunk.len() > 1 { ALPHABET[(((b1 & 0x0f) << 2) | (b2 >> 6)) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { ALPHABET[(b2 & 0x3f) as usize] as char } else { '=' });
    }
    out
}

/// Strict base64 decoding. Whitespace is ignored; every other character has to belong to the alphabet, and the
/// padding has to be right. `None` means the text is not base64.
pub fn b64_decode(text: &str) -> Option<Vec<u8>> {
    let clean: Vec<u8> = text.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    if clean.is_empty() {
        return Some(Vec::new());
    }
    if clean.len() % 4 != 0 {
        return None;
    }
    let padding = clean.iter().rev().take_while(|b| **b == b'=').count();
    if padding > 2 {
        return None;
    }
    let body = &clean[..clean.len() - padding];
    if body.iter().any(|b| *b == b'=') {
        return None;
    }
    if (padding == 0 && body.len() % 4 != 0)
        || (padding == 1 && body.len() % 4 != 3)
        || (padding == 2 && body.len() % 4 != 2)
    {
        return None;
    }
    let value = |b: u8| -> Option<u32> {
        match b {
            b'A'..=b'Z' => Some((b - b'A') as u32),
            b'a'..=b'z' => Some((b - b'a' + 26) as u32),
            b'0'..=b'9' => Some((b - b'0' + 52) as u32),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    };
    let mut out = Vec::with_capacity(clean.len() / 4 * 3);
    let mut buffer: u32 = 0;
    let mut bits = 0u32;
    for byte in body.iter().copied() {
        buffer = (buffer << 6) | value(byte)?;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    if buffer != 0 {
        return None;
    }
    Some(out)
}

/// SHA-256 as lower-case hex. The few lines keep the crate list unchanged.
pub fn sha256_hex(data: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
        0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
        0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
        0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
        0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
        0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
        0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
    ];
    let mut state: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
        0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
    ];
    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut message = data.to_vec();
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in message.chunks(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([chunk[i * 4], chunk[i * 4 + 1], chunk[i * 4 + 2], chunk[i * 4 + 3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let mut a = state[0];
        let mut b = state[1];
        let mut c = state[2];
        let mut d = state[3];
        let mut e = state[4];
        let mut f = state[5];
        let mut g = state[6];
        let mut h = state[7];
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = h.wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        state[0] = state[0].wrapping_add(a);
        state[1] = state[1].wrapping_add(b);
        state[2] = state[2].wrapping_add(c);
        state[3] = state[3].wrapping_add(d);
        state[4] = state[4].wrapping_add(e);
        state[5] = state[5].wrapping_add(f);
        state[6] = state[6].wrapping_add(g);
        state[7] = state[7].wrapping_add(h);
    }
    state.iter().map(|word| format!("{:08x}", word)).collect()
}

/// The MIME type of a file name. Unknown endings are binary.
pub fn mime_for(name: &str) -> &'static str {
    let ext = Path::new(name).extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "bmp" => "image/bmp",
        "pdf" => "application/pdf",
        "json" => "application/json",
        "txt" => "text/plain",
        "md" | "markdown" => "text/markdown",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "js" | "mjs" => "text/javascript",
        "ts" => "text/typescript",
        "rs" => "text/rust",
        "py" => "text/x-python",
        "toml" => "text/toml",
        "yaml" | "yml" => "text/yaml",
        "csv" => "text/csv",
        "xml" => "application/xml",
        "zip" => "application/zip",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        _ => "application/octet-stream",
    }
}

/// The scheme and host of an absolute URL. The port and a user info part are dropped.
#[derive(Clone, Debug)]
pub struct UrlParts {
    pub scheme: String,
    pub host: String,
}

pub fn url_parts(uri: &str) -> Option<UrlParts> {
    let (scheme, rest) = uri.split_once("://")?;
    if scheme.trim().is_empty() {
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let hostport = authority.rsplit('@').next().unwrap_or(authority);
    let host = if hostport.starts_with('[') {
        hostport.split(']').next().map(|h| format!("{}]", h)).unwrap_or_default()
    } else {
        hostport.split(':').next().unwrap_or("").to_string()
    };
    Some(UrlParts { scheme: scheme.trim().to_lowercase(), host: host.trim().to_lowercase() })
}

/// The path of a `file://` URL, with percent escapes decoded.
///
/// Windows paths come in two forms: `file:///C:/dir/a.txt` (the standard) and `file://C:\dir\a.txt` (a path pasted
/// after the scheme). Both give `C:/dir/a.txt` or `C:\dir\a.txt`, which Windows opens alike.
pub fn file_uri_path(uri: &str) -> Option<String> {
    let rest = uri.strip_prefix("file://")?;
    let path = if rest.starts_with('/') || starts_with_drive(rest) {
        rest
    } else {
        &rest[rest.find('/')?..]
    };
    let path = percent_decode(path);
    if cfg!(windows) && path.starts_with('/') && starts_with_drive(&path[1..]) {
        return Some(path[1..].to_string());
    }
    Some(path)
}

/// True for `C:`, `C:/...` and `C:\...`: a drive letter of Windows.
fn starts_with_drive(text: &str) -> bool {
    let b = text.as_bytes();
    b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b.len() == 2 || b[2] == b'/' || b[2] == b'\\')
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(value) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(value);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

fn file_uri(path: &Path) -> String {
    // Windows: C:\dir\a.txt becomes file:///C:/dir/a.txt. The prefix \\?\ of a canonical path is not part of the URL.
    let text = path.to_string_lossy();
    let text = if cfg!(windows) { text.strip_prefix(r"\\?\").unwrap_or(&text).replace('\\', "/") } else { text.to_string() };
    let text = if text.starts_with('/') { text } else { format!("/{}", text) };
    let mut out = String::from("file://");
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{:02X}", byte));
        }
    }
    out
}

/// True when the real path is one of the roots or lies below one. Symbolic links are resolved first.
pub fn inside(path: &Path, roots: &[PathBuf]) -> bool {
    let Ok(real) = std::fs::canonicalize(path) else { return false };
    roots.iter().any(|root| {
        std::fs::canonicalize(root).map(|r| real == r || real.starts_with(&r)).unwrap_or(false)
    })
}

fn push_dir(out: &mut Vec<PathBuf>, dir: PathBuf) {
    if let Ok(real) = std::fs::canonicalize(&dir) {
        if real.is_dir() && !out.contains(&real) {
            out.push(real);
        }
    }
}

/// The folders a brief may read. They are the project folder, the registered plugin folders, the plugin home and the
/// cwd of the named server. `brief.roots` in the config adds more; nothing else is ever read.
pub fn allowed_roots(ctx: &Ctx, server: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let project = if ctx.root.file_name().map(|n| n == ".penelope").unwrap_or(false) {
        ctx.root.parent().map(Path::to_path_buf).unwrap_or_else(|| ctx.root.clone())
    } else {
        ctx.root.clone()
    };
    push_dir(&mut out, project);
    if let Some(list) = ctx.cfg["brief"]["roots"].as_array() {
        for item in list.iter().filter_map(Value::as_str) {
            push_dir(&mut out, PathBuf::from(item));
        }
    }
    if let Some(list) = ctx.cfg["plugin_dirs"].as_array() {
        for item in list.iter().filter_map(Value::as_str) {
            push_dir(&mut out, PathBuf::from(item));
        }
    }
    let plugin_home = ctx.cfg["plugin_home"].as_str().filter(|p| !p.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::platform::home().join(".penelope").join("plugins"));
    push_dir(&mut out, plugin_home);
    let reg = crate::plugins::load(ctx);
    for plugin in reg["plugins"].as_array().cloned().unwrap_or_default() {
        if let Some(dir) = plugin["dir"].as_str() {
            push_dir(&mut out, PathBuf::from(dir));
        }
    }
    if let Some((entry, _)) = crate::plugins::server_entry(ctx, server) {
        if let Some(cwd) = entry["cwd"].as_str().filter(|c| !c.trim().is_empty()) {
            push_dir(&mut out, PathBuf::from(cwd));
        }
    }
    out
}

/// The hosts a brief may open. Only the config names them; the default is an empty list.
pub fn allowed_hosts(ctx: &Ctx) -> Vec<String> {
    ctx.cfg["brief"]["hosts"].as_array().cloned().unwrap_or_default().into_iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .map(|h| h.trim().to_lowercase())
        .filter(|h| !h.is_empty())
        .collect()
}

fn host_allowed(host: &str, hosts: &[String]) -> bool {
    !host.is_empty() && hosts.iter().any(|allowed| allowed.eq_ignore_ascii_case(host))
}

/// Finds a named file below one of the roots. A relative name is tried in every root; an absolute path has to be
/// inside a root as well. A `..` that leaves a root is refused, and so is a symbolic link that points out of it.
fn local_file(name: &str, roots: &[PathBuf]) -> Result<PathBuf, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("an attachment names an empty file".to_string());
    }
    let raw = PathBuf::from(name);
    let mut candidates: Vec<PathBuf> = Vec::new();
    if raw.is_absolute() {
        candidates.push(raw.clone());
    } else {
        for root in roots {
            candidates.push(root.join(&raw));
        }
    }
    for candidate in &candidates {
        if candidate.is_file() {
            let real = std::fs::canonicalize(candidate).map_err(|e| format!("cannot read {}: {}", candidate.display(), e))?;
            if !inside(&real, roots) {
                return Err(format!("the path {} is outside the folders this server may read", candidate.display()));
            }
            return Ok(real);
        }
    }
    if raw.is_absolute() {
        return Err(format!("the path {} is outside the folders this server may read or does not exist", raw.display()));
    }
    Err(format!("no file named {} below the folders this server may read", name))
}

fn read_local(name: &str, roots: &[PathBuf], max: usize) -> Result<(PathBuf, Vec<u8>), String> {
    let path = local_file(name, roots)?;
    let size = std::fs::metadata(&path).map_err(|e| format!("cannot read {}: {}", path.display(), e))?.len();
    if size > max as u64 {
        return Err(format!("{} is {} bytes, larger than the item limit of {} bytes", path.display(), size, max));
    }
    let data = std::fs::read(&path).map_err(|e| format!("cannot read {}: {}", path.display(), e))?;
    Ok((path, data))
}

/// Packs the short form into the argument the server expects. A file over the inline limit becomes an upload; a
/// server that announces no upload tool gets a clear refusal instead of a half-packed brief.
pub fn pack(short: &Value, roots: &[PathBuf], ext: &Extension) -> Result<Packed, String> {
    if ext.version != VERSION {
        return Err(format!("brief version {} is not supported by this client, it speaks version {}", ext.version, VERSION));
    }
    if !short.is_object() {
        return Err("brief must be an object".to_string());
    }
    if short.get("items").is_some() {
        return Err("brief is already packed".to_string());
    }
    let task = short["task"].as_str().map(str::trim).unwrap_or("");
    if task.is_empty() || task.chars().count() > MAX_TASK {
        return Err(format!("brief.task must be a text of 1 to {} characters", MAX_TASK));
    }
    let attach = match short.get("attach") {
        Some(value) => value.as_array().cloned().ok_or_else(|| "brief.attach must be a list".to_string())?,
        None => Vec::new(),
    };
    if attach.len() > ext.max_items.min(MAX_ITEMS) {
        return Err(format!("a brief holds at most {} attachments", ext.max_items.min(MAX_ITEMS)));
    }
    let mut items: Vec<Value> = Vec::new();
    let mut uploads: Vec<Upload> = Vec::new();
    for (n, entry) in attach.iter().enumerate() {
        let entry = entry.as_str().ok_or_else(|| format!("attachment {} is not a text", n + 1))?;
        let (kind, rest) = split_ref(entry)?;
        let id = format!("a{}", n + 1);
        if kind == "link" {
            let parts = url_parts(rest).ok_or_else(|| format!("attachment {}: {} is not an absolute URL", id, rest))?;
            match parts.scheme.as_str() {
                "http" | "https" => items.push(json!({"id": id, "kind": kind, "uri": rest})),
                "file" => {
                    let path = file_uri_path(rest).ok_or_else(|| format!("attachment {}: {} is not a file URL", id, rest))?;
                    if !Path::new(&path).is_file() {
                        return Err(format!("attachment {}: no file at {}", id, path));
                    }
                    if !inside(Path::new(&path), roots) {
                        return Err(format!("attachment {}: the path {} is outside the folders this client may read", id, path));
                    }
                    items.push(json!({"id": id, "kind": kind, "uri": rest}));
                }
                other => return Err(format!("attachment {}: only file, http and https links are allowed, not {}", id, other)),
            }
            continue;
        }
        let path = local_file(rest, roots)?;
        let size = std::fs::metadata(&path).map_err(|e| format!("cannot read {}: {}", path.display(), e))?.len();
        if size > ext.max_item as u64 {
            return Err(format!("{} is {} bytes, larger than the item limit of {} bytes", path.display(), size, ext.max_item));
        }
        if size > ext.max_inline as u64 && ext.upload_tool.trim().is_empty() {
            return Err(format!("{} is {} bytes, larger than the inline limit of {} bytes, and this server announces no upload tool",
                               path.display(), size, ext.max_inline));
        }
        let data = std::fs::read(&path).map_err(|e| format!("cannot read {}: {}", path.display(), e))?;
        if data.len() > ext.max_item {
            return Err(format!("{} is {} bytes, larger than the item limit of {} bytes", path.display(), data.len(), ext.max_item));
        }
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| rest.to_string());
        let mime = mime_for(&name);
        let hash = sha256_hex(&data);
        let mut item = json!({"id": id, "kind": kind, "name": name, "mime": mime, "bytes": data.len(), "sha256": hash});
        if data.len() > ext.max_inline {
            let sequence = NEXT_UPLOAD.fetch_add(1, Ordering::Relaxed);
            let upload_id = format!("u-{}-{}", &hash[..12], sequence);
            item["upload"] = json!(upload_id);
            uploads.push(Upload { id: upload_id, data });
        } else if kind == "image" {
            item["data"] = json!(b64_encode(&data));
        } else if let Ok(text) = String::from_utf8(data.clone()) {
            item["text"] = json!(text);
        } else {
            item["data"] = json!(b64_encode(&data));
        }
        items.push(item);
    }
    Ok(Packed { argument: json!({"v": VERSION, "task": task, "items": items}), uploads })
}

/// The `brief_put` arguments for one upload, in order. An empty file still gets one empty chunk.
pub fn chunk_arguments(id: &str, data: &[u8]) -> Vec<Value> {
    let total = if data.is_empty() { 1 } else { (data.len() + CHUNK - 1) / CHUNK };
    (0..total).map(|seq| {
        let start = seq * CHUNK;
        let end = usize::min(start + CHUNK, data.len());
        let part = if start <= data.len() { &data[start..end] } else { &[] };
        json!({"id": id, "seq": seq, "data": b64_encode(part), "last": seq + 1 == total})
    }).collect()
}

/// One upload that waits for its brief.
struct UploadState {
    next_seq: u64,
    data: Vec<u8>,
    done: bool,
}

impl Default for UploadState {
    fn default() -> Self {
        UploadState { next_seq: 0, data: Vec::new(), done: false }
    }
}

/// The uploads and briefs of one server process. A brief id such as `b1` can be used by a later call, so the content
/// does not travel twice.
#[derive(Default)]
pub struct Store {
    uploads: HashMap<String, UploadState>,
    briefs: HashMap<String, Brief>,
    next_brief: usize,
}

impl Store {
    /// Takes one chunk of an upload. Sequence numbers start at 0 and may not skip or repeat.
    pub fn put(&mut self, id: &str, seq: u64, data_b64: &str, last: bool) -> Result<usize, String> {
        if id.trim().is_empty() {
            return Err("brief_put.id must name an upload".to_string());
        }
        let estimate = data_b64.len().saturating_mul(3) / 4;
        let state = self.uploads.entry(id.to_string()).or_default();
        if state.done {
            return Err(format!("upload {} is already complete", id));
        }
        if state.data.len().saturating_add(estimate) > MAX_ITEM + 2 {
            self.uploads.remove(id);
            return Err(format!("upload {} is larger than {} bytes", id, MAX_ITEM));
        }
        let chunk = b64_decode(data_b64).ok_or_else(|| format!("upload {}: data is not base64", id))?;
        if seq != state.next_seq {
            return Err(format!("upload {}: chunk {} is out of order, expected {}", id, seq, state.next_seq));
        }
        if state.data.len() + chunk.len() > MAX_ITEM {
            self.uploads.remove(id);
            return Err(format!("upload {} is larger than {} bytes", id, MAX_ITEM));
        }
        state.data.extend_from_slice(&chunk);
        state.next_seq += 1;
        state.done = last;
        Ok(state.data.len())
    }

    fn take(&mut self, id: &str) -> Result<Vec<u8>, String> {
        let state = self.uploads.remove(id)
            .ok_or_else(|| format!("upload {} is missing or not finished", id))?;
        if !state.done {
            return Err(format!("upload {} is missing or not finished", id));
        }
        Ok(state.data)
    }

    fn keep(&mut self, mut brief: Brief) -> Brief {
        self.next_brief += 1;
        let id = format!("b{}", self.next_brief);
        brief.id = id.clone();
        self.briefs.insert(id, brief.clone());
        brief
    }

    /// One brief by the id the server returned.
    pub fn get(&self, id: &str) -> Option<Brief> {
        self.briefs.get(id).cloned()
    }
}

/// One checked attachment. `content` is `None` for a link that was only checked against the host list.
#[derive(Clone, Debug)]
pub struct Item {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub mime: String,
    pub uri: Option<String>,
    pub content: Option<Vec<u8>>,
}

/// A checked brief.
#[derive(Clone, Debug)]
pub struct Brief {
    pub id: String,
    pub task: String,
    pub items: Vec<Item>,
}

impl Brief {
    pub fn of(&self, kind: &str) -> Vec<&Item> {
        self.items.iter().filter(|item| item.kind == kind).collect()
    }

    pub fn first(&self, kind: &str) -> Option<&Item> {
        self.items.iter().find(|item| item.kind == kind)
    }
}

fn check_size_hash(item: &Item, raw: &Value) -> Result<(), String> {
    let Some(content) = &item.content else { return Ok(()) };
    if content.len() > MAX_ITEM {
        return Err(format!("attachment {} is larger than {} bytes", item.id, MAX_ITEM));
    }
    if let Some(announced) = raw.get("bytes") {
        let announced = announced.as_u64()
            .ok_or_else(|| format!("attachment {}: bytes must be an integer", item.id))?;
        if announced != content.len() as u64 {
            return Err(format!("attachment {}: {} bytes arrived, {} were announced", item.id, content.len(), announced));
        }
    }
    if let Some(announced) = raw.get("sha256") {
        let announced = announced.as_str()
            .ok_or_else(|| format!("attachment {}: sha256 must be a text", item.id))?;
        if announced.to_lowercase() != sha256_hex(content) {
            return Err(format!("attachment {}: the SHA-256 of the content does not match", item.id));
        }
    }
    Ok(())
}

/// Checks one brief and loads the content of every item. A short form without a harness is accepted too: a local name
/// is read inside the roots and an HTTP link is checked against the hosts. The host is checked here; the content of an
/// HTTP link is read by the tool that carries out the work, or by `check`.
pub fn open(argument: &Value, store: &mut Store, roots: &[PathBuf], hosts: &[String]) -> Result<Brief, String> {
    if !argument.is_object() {
        return Err("brief must be an object".to_string());
    }
    let version = argument.get("v").and_then(Value::as_u64).unwrap_or(VERSION);
    if version != VERSION {
        return Err(format!("brief version {} is not supported, this server speaks version {}", version, VERSION));
    }
    let task = argument.get("task").and_then(Value::as_str).map(str::trim).unwrap_or("");
    if task.is_empty() || task.chars().count() > MAX_TASK {
        return Err(format!("brief.task must be a text of 1 to {} characters", MAX_TASK));
    }
    let mut items: Vec<Item> = Vec::new();

    // A client with a harness sends `items`. A client without one may send the short form; the server resolves it.
    if let Some(raw_items) = argument.get("items") {
        let list = raw_items.as_array().ok_or_else(|| "brief.items must be a list".to_string())?;
        for raw in list {
            if items.len() >= MAX_ITEMS {
                return Err(format!("a brief holds at most {} attachments", MAX_ITEMS));
            }
            let id = raw["id"].as_str().filter(|id| !id.trim().is_empty())
                .ok_or_else(|| "every attachment needs an id".to_string())?.to_string();
            let kind = raw["kind"].as_str().unwrap_or("");
            if !KINDS.contains(&kind) {
                return Err(format!("attachment {}: kind must be one of {}", id, KINDS.join(", ")));
            }
            let ways: Vec<&str> = ["text", "data", "upload", "uri"].iter().copied().filter(|k| raw.get(*k).is_some()).collect();
            if ways.len() != 1 {
                return Err(format!("attachment {} must carry its content in exactly one way, found {}", id, ways.len()));
            }
            let mut name = raw["name"].as_str().unwrap_or("").to_string();
            let mime = raw["mime"].as_str().unwrap_or("").to_string();
            let mut uri = None;
            let mut content = None;
            match ways[0] {
                "text" => {
                    let text = raw["text"].as_str().ok_or_else(|| format!("attachment {}: text must be a text", id))?;
                    content = Some(text.as_bytes().to_vec());
                }
                "data" => {
                    let data = raw["data"].as_str().ok_or_else(|| format!("attachment {}: data must be a text", id))?;
                    let bytes = b64_decode(data).ok_or_else(|| format!("attachment {}: data is not base64", id))?;
                    if bytes.len() > MAX_INLINE {
                        return Err(format!("attachment {} is larger than the inline limit of {} bytes, upload it", id, MAX_INLINE));
                    }
                    content = Some(bytes);
                }
                "upload" => {
                    let upload = raw["upload"].as_str().ok_or_else(|| format!("attachment {}: upload must name an upload", id))?;
                    content = Some(store.take(upload)?);
                }
                "uri" => {
                    let raw_uri = raw["uri"].as_str().ok_or_else(|| format!("attachment {}: uri must be a text", id))?;
                    let parts = url_parts(raw_uri).ok_or_else(|| format!("attachment {}: {} is not an absolute URL", id, raw_uri))?;
                    match parts.scheme.as_str() {
                        "file" => {
                            let path = file_uri_path(raw_uri).ok_or_else(|| format!("attachment {}: {} is not a file URL", id, raw_uri))?;
                            let path = std::fs::canonicalize(&path)
                                .map_err(|e| format!("attachment {}: no file at this path: {}", id, e))?;
                            if !inside(&path, roots) {
                                return Err(format!("attachment {}: the path {} is outside the folders this server may read", id, path.display()));
                            }
                            if !path.is_file() {
                                return Err(format!("attachment {}: no file at this path", id));
                            }
                            let size = std::fs::metadata(&path).map_err(|e| format!("attachment {}: cannot read {}: {}", id, path.display(), e))?.len();
                            if size > MAX_ITEM as u64 {
                                return Err(format!("attachment {} is larger than {} bytes", id, MAX_ITEM));
                            }
                            if name.is_empty() {
                                name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                            }
                            content = Some(std::fs::read(&path).map_err(|e| format!("attachment {}: cannot read {}: {}", id, path.display(), e))?);
                        }
                        "http" | "https" => {
                            if !host_allowed(&parts.host, hosts) {
                                return Err(format!("attachment {}: the host {} is not on the allow list of this server", id, parts.host));
                            }
                        }
                        other => return Err(format!("attachment {}: only file, http and https links are allowed, not {}", id, other)),
                    }
                    uri = Some(raw_uri.to_string());
                }
                _ => unreachable!(),
            }
            let item = Item { id: id.clone(), kind: kind.to_string(), name, mime, uri, content };
            check_size_hash(&item, raw)?;
            items.push(item);
        }
    }
    if let Some(attach) = argument.get("attach") {
        let list = attach.as_array().ok_or_else(|| "brief.attach must be a list".to_string())?;
        for entry in list {
            if items.len() >= MAX_ITEMS {
                return Err(format!("a brief holds at most {} attachments", MAX_ITEMS));
            }
            let entry = entry.as_str().ok_or_else(|| "every attachment of brief.attach must be a text".to_string())?;
            let (kind, rest) = split_ref(entry)?;
            let id = format!("a{}", items.len() + 1);
            if kind == "link" {
                let parts = url_parts(rest).ok_or_else(|| format!("attachment {}: {} is not an absolute URL", id, rest))?;
                match parts.scheme.as_str() {
                    "http" | "https" => {
                        if !host_allowed(&parts.host, hosts) {
                            return Err(format!("attachment {}: the host {} is not on the allow list of this server", id, parts.host));
                        }
                    }
                    "file" => {
                        let path = file_uri_path(rest).ok_or_else(|| format!("attachment {}: {} is not a file URL", id, rest))?;
                        let path = std::fs::canonicalize(&path)
                            .map_err(|e| format!("attachment {}: no file at this path: {}", id, e))?;
                        if !inside(&path, roots) {
                            return Err(format!("attachment {}: the path {} is outside the folders this server may read", id, path.display()));
                        }
                        if !path.is_file() {
                            return Err(format!("attachment {}: no file at this path", id));
                        }
                        let size = std::fs::metadata(&path).map_err(|e| format!("attachment {}: cannot read {}: {}", id, path.display(), e))?.len();
                        if size > MAX_ITEM as u64 {
                            return Err(format!("attachment {} is larger than {} bytes", id, MAX_ITEM));
                        }
                        let data = std::fs::read(&path).map_err(|e| format!("attachment {}: cannot read {}: {}", id, path.display(), e))?;
                        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                        items.push(Item { id, kind: kind.to_string(), mime: mime_for(&name).to_string(), name, uri: Some(rest.to_string()), content: Some(data) });
                        continue;
                    }
                    other => return Err(format!("attachment {}: only file, http and https links are allowed, not {}", id, other)),
                }
                items.push(Item { id, kind: kind.to_string(), name: String::new(), mime: String::new(), uri: Some(rest.to_string()), content: None });
            } else {
                let (path, data) = read_local(rest, roots, MAX_ITEM)
                    .map_err(|e| format!("attachment {}: {}", id, e))?;
                let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| rest.to_string());
                let item = Item { id, kind: kind.to_string(), mime: mime_for(&name).to_string(), name, uri: Some(file_uri(&path)), content: Some(data) };
                items.push(item);
            }
        }
    }
    if items.len() > MAX_ITEMS {
        return Err(format!("a brief holds at most {} attachments", MAX_ITEMS));
    }
    Ok(store.keep(Brief { id: String::new(), task: task.to_string(), items }))
}

/// Reads one HTTP link from a host on the allow list, at most `max` bytes. Redirects are not followed.
pub fn read_link(uri: &str, hosts: &[String], max: usize) -> Result<Vec<u8>, String> {
    let parts = url_parts(uri).ok_or_else(|| format!("{} is not an absolute URL", uri))?;
    if !matches!(parts.scheme.as_str(), "http" | "https") {
        return Err("only http and https links can be read".to_string());
    }
    if !host_allowed(&parts.host, hosts) {
        return Err(format!("the host {} is not on the allow list of this server", parts.host));
    }
    let response = crate::net::agent(10).get(uri).call().map_err(|e| format!("cannot read {}: {}", uri, e))?;
    let mut data = Vec::new();
    response.into_reader().take(max as u64 + 1).read_to_end(&mut data)
        .map_err(|e| format!("cannot read {}: {}", uri, e))?;
    if data.len() > max {
        return Err(format!("{} is larger than {} bytes", uri, max));
    }
    Ok(data)
}

fn looks_like_image(data: &[u8], mime: &str) -> bool {
    if mime.starts_with("image/svg") {
        return std::str::from_utf8(data).map(|text| {
            let text = text.trim_start();
            text.starts_with("<svg") || text.starts_with("<?xml")
        }).unwrap_or(false);
    }
    match mime {
        "image/png" => data.starts_with(&[0x89, b'P', b'N', b'G']),
        "image/jpeg" | "image/jpg" => data.starts_with(&[0xFF, 0xD8, 0xFF]),
        "image/gif" => data.starts_with(b"GIF8"),
        "image/bmp" => data.starts_with(b"BM"),
        "image/webp" => data.len() >= 12 && data.starts_with(b"RIFF") && &data[8..12] == b"WEBP",
        _ => true,
    }
}

fn next_call(brief: &Brief) -> Value {
    if brief.items.is_empty() {
        return Value::Null;
    }
    if let Some(link) = brief.first("link") {
        if let Some(uri) = &link.uri {
            return json!({"tool": "fetch", "arguments": {"url": uri, "question": brief.task}});
        }
    }
    for item in &brief.items {
        if let Some(uri) = &item.uri {
            if let Some(path) = file_uri_path(uri) {
                if item.kind == "image" {
                    return json!({"tool": "screen", "arguments": {"action": "image", "path": path}});
                }
                return json!({"tool": "read", "arguments": {"path": path}});
            }
        }
    }
    json!({"tool": "brief_check", "arguments": {"brief_id": brief.id}})
}

/// The text a model reads after `brief_check`. The steps name what was really read, the findings name a real problem,
/// and `next:` is a JSON call the model can make next.
pub fn render_check(brief: &Brief) -> String {
    let mut steps: Vec<String> = Vec::new();
    let mut findings: Vec<String> = Vec::new();
    steps.push(format!("task: {}", brief.task.chars().take(160).collect::<String>()));
    steps.push(format!("read {} items: {} file, {} context, {} image, {} link",
                       brief.items.len(), brief.of("file").len(), brief.of("context").len(),
                       brief.of("image").len(), brief.of("link").len()));
    for item in &brief.items {
        let mut line = format!("{} {} {}", item.id, item.kind, item.name);
        if let Some(uri) = &item.uri {
            line.push_str(&format!(" · {}", uri));
        }
        if let Some(content) = &item.content {
            line.push_str(&format!(" · {} bytes · sha256 {}", content.len(), sha256_hex(content)));
            if content.is_empty() {
                findings.push(format!("{} {}: the content is empty", item.id, item.name));
            }
            if item.kind == "context" && std::str::from_utf8(content).is_err() {
                findings.push(format!("{} {}: context is not valid UTF-8", item.id, item.name));
            }
            if item.kind == "image" && !looks_like_image(content, &item.mime) {
                findings.push(format!("{} {}: the content does not look like {}", item.id, item.name, item.mime));
            }
        } else if item.kind == "link" {
            line.push_str(" · not read");
            findings.push(format!("{} {}: the link was checked but could not be read", item.id, item.uri.as_deref().unwrap_or("")));
        } else {
            line.push_str(" · content not loaded");
        }
        steps.push(line);
    }
    if brief.items.is_empty() {
        findings.push("no attachment: add e.g. \"file:NAME\" or \"link:URL\" to attach".to_string());
    }
    let next = next_call(brief);
    let mut out = format!("brief {}: {} steps. {}", brief.id, 2 + brief.items.len(), steps.join("; "));
    out.push_str(&format!("\n{} findings.", findings.len()));
    for (n, finding) in findings.iter().enumerate() {
        out.push_str(&format!("\nf{} {}", n + 1, finding));
    }
    out.push_str(&format!("\nnext: {}", serde_json::to_string(&next).unwrap_or_else(|_| "null".to_string())));
    out
}

/// Opens a brief (or an earlier one by id), reads its allowed links and returns the check text. A link that cannot
/// be read becomes a finding, not a refused brief: the host was already allowed.
pub fn check(ctx: &Ctx, arg: &Value, store: &mut Store) -> Result<String, String> {
    let roots = allowed_roots(ctx, "");
    let hosts = allowed_hosts(ctx);
    let mut brief = if let Some(id) = arg["brief_id"].as_str().filter(|id| !id.trim().is_empty()) {
        store.get(id).ok_or_else(|| format!("no brief with id {}", id))?
    } else if let Some(argument) = arg.get("brief").filter(|v| !v.is_null()) {
        open(argument, store, &roots, &hosts)?
    } else {
        return Err("give a brief or the id of an earlier brief".to_string());
    };
    for item in brief.items.iter_mut() {
        if item.kind == "link" && item.content.is_none() {
            if let Some(uri) = &item.uri {
                item.content = read_link(uri, &hosts, MAX_ITEM).ok();
            }
        }
    }
    Ok(render_check(&brief))
}

/// The handler of the `brief_put` tool.
pub fn put(store: &mut Store, arg: &Value) -> Result<String, String> {
    let id = arg["id"].as_str().ok_or_else(|| "brief_put.id must be a text".to_string())?;
    let seq = arg["seq"].as_u64().ok_or_else(|| "brief_put.seq must be an integer".to_string())?;
    let data = arg["data"].as_str().ok_or_else(|| "brief_put.data must be a text".to_string())?;
    let last = arg["last"].as_bool().ok_or_else(|| "brief_put.last must be a boolean".to_string())?;
    let size = store.put(id, seq, data, last)?;
    Ok(format!("upload {}: {} bytes{}", id, size, if last { ", complete" } else { "" }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("penelope-brief-unit-{}-{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn text_item(id: &str, kind: &str, name: &str, text: &str) -> Value {
        json!({"id": id, "kind": kind, "name": name, "mime": "text/plain",
               "bytes": text.len(), "sha256": sha256_hex(text.as_bytes()), "text": text})
    }

    #[test]
    fn sha256_matches_the_known_vectors() {
        assert_eq!(sha256_hex(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(sha256_hex(b"hello"), "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824");
    }

    #[test]
    fn base64_round_trips_and_refuses_broken_text() {
        assert_eq!(b64_encode(b"hello"), "aGVsbG8=");
        assert_eq!(b64_decode("aGVsbG8=").unwrap(), b"hello");
        assert_eq!(b64_decode("aGVs bG8=\n").unwrap(), b"hello");
        assert!(b64_decode("aGVsbG8").is_none());
        assert!(b64_decode("aGVsbG8*").is_none());
        assert!(b64_decode("a=VsbG8=").is_none());
    }

    #[test]
    fn the_extension_is_read_under_experimental_and_extensions() {
        let init = json!({"capabilities": {"experimental": {EXTENSION_ID: {
            "version": 1, "maxInlineBytes": 10, "maxItemBytes": 20, "maxItems": 2, "uploadTool": "put"
        }}}});
        let ext = extension_of(&init).unwrap();
        assert_eq!((ext.version, ext.max_inline, ext.max_item, ext.max_items), (1, 10, 20, 2));
        assert_eq!(ext.upload_tool, "put");
        let other = json!({"capabilities": {"extensions": {EXTENSION_ID: {"version": 1}}}});
        assert!(extension_of(&other).is_some());
        assert!(extension_of(&json!({"capabilities": {}})).is_none());
        assert!(extension_of(&json!({"capabilities": {"experimental": {EXTENSION_ID: false}}})).is_none());
    }

    #[test]
    fn a_packed_text_brief_is_checked_size_hash_and_kind() {
        let mut store = Store::default();
        let raw = json!({"v": 1, "task": "look", "items": [text_item("a1", "file", "rules.md", "rules")]});
        let brief = open(&raw, &mut store, &[], &[]).unwrap();
        assert_eq!(brief.id, "b1");
        assert_eq!(brief.task, "look");
        assert_eq!(brief.first("file").unwrap().content.as_deref(), Some(b"rules".as_slice()));
        assert_eq!(store.get("b1").unwrap().items.len(), 1);
    }

    #[test]
    fn a_wrong_hash_is_refused() {
        let mut store = Store::default();
        let mut item = text_item("a1", "file", "rules.md", "rules");
        item["text"] = json!("changed");
        let err = open(&json!({"task": "look", "items": [item]}), &mut store, &[], &[]).unwrap_err();
        assert!(err.contains("bytes arrived") || err.contains("SHA-256"), "{}", err);
    }

    #[test]
    fn two_ways_in_one_item_are_refused() {
        let mut store = Store::default();
        let err = open(&json!({"task": "look", "items": [{"id": "a1", "kind": "file", "text": "a", "data": "YQ=="}]}),
                       &mut store, &[], &[]).unwrap_err();
        assert!(err.contains("exactly one way"), "{}", err);
    }

    #[test]
    fn a_path_outside_the_roots_is_refused() {
        let dir = temp("outside-open");
        let outside = std::env::temp_dir().join(format!("penelope-brief-outside-open-{}.txt", std::process::id()));
        fs::write(&outside, "secret").unwrap();
        let uri = format!("file://{}", outside.display());
        let mut store = Store::default();
        let err = open(&json!({"task": "look", "items": [{"id": "a1", "kind": "file", "uri": uri}]}),
                       &mut store, &[dir.clone()], &[]).unwrap_err();
        assert!(err.contains("outside the folders"), "{}", err);
        let _ = fs::remove_file(&outside);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_link_to_a_foreign_host_is_refused_and_an_allowed_host_is_kept() {
        let mut store = Store::default();
        let err = open(&json!({"task": "look", "items": [{"id": "a1", "kind": "link", "uri": "https://example.org/"}]}),
                       &mut store, &[], &["127.0.0.1".to_string()]).unwrap_err();
        assert!(err.contains("allow list"), "{}", err);
        let fine = open(&json!({"task": "look", "items": [{"id": "a1", "kind": "link", "uri": "https://127.0.0.1/a"}]}),
                        &mut store, &[], &["127.0.0.1".to_string()]).unwrap();
        assert!(fine.first("link").unwrap().content.is_none());
    }

    #[test]
    fn an_inline_item_over_the_limit_is_refused() {
        let mut store = Store::default();
        let big = b64_encode(&vec![b'x'; MAX_INLINE + 1]);
        let err = open(&json!({"task": "look", "items": [{"id": "a1", "kind": "file", "data": big}]}),
                       &mut store, &[], &[]).unwrap_err();
        assert!(err.contains("inline limit"), "{}", err);
    }

    #[test]
    fn an_empty_task_and_an_unknown_kind_are_refused() {
        let mut store = Store::default();
        assert!(open(&json!({"task": " "}), &mut store, &[], &[]).unwrap_err().contains("task"));
        assert!(open(&json!({"task": "x", "attach": ["video:a.mp4"]}), &mut store, &[], &[]).unwrap_err().contains("kind"));
    }

    #[test]
    fn an_upload_is_taken_in_chunks_and_must_be_complete() {
        let data = vec![b'x'; 250_000];
        let mut store = Store::default();
        let parts = chunk_arguments("u1", &data);
        assert_eq!(parts.len(), 3);
        for part in &parts {
            store.put("u1", part["seq"].as_u64().unwrap(), part["data"].as_str().unwrap(), part["last"].as_bool().unwrap()).unwrap();
        }
        let raw = json!({"task": "look", "items": [{"id": "a1", "kind": "file", "upload": "u1",
                                                     "bytes": data.len(), "sha256": sha256_hex(&data)}]});
        let brief = open(&raw, &mut store, &[], &[]).unwrap();
        assert_eq!(brief.items[0].content.as_ref().unwrap(), &data);

        let mut store = Store::default();
        assert!(store.put("u1", 1, "YQ==", false).unwrap_err().contains("out of order"));
        store.put("u2", 0, "YQ==", false).unwrap();
        let err = open(&json!({"task": "look", "items": [{"id": "a1", "kind": "file", "upload": "u2"}]}),
                       &mut store, &[], &[]).unwrap_err();
        assert!(err.contains("not finished"), "{}", err);
    }

    #[test]
    fn the_short_form_is_accepted_without_a_harness() {
        let dir = temp("short");
        fs::write(dir.join("rules.md"), "rules").unwrap();
        let mut store = Store::default();
        let brief = open(&json!({"task": "look", "attach": ["file:rules.md"]}), &mut store, &[dir.clone()], &[]).unwrap();
        assert_eq!(brief.items[0].content.as_deref(), Some(b"rules".as_slice()));
        assert_eq!(brief.items[0].name, "rules.md");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_check_names_steps_findings_and_a_next_call() {
        let dir = temp("check");
        fs::write(dir.join("rules.md"), "rules").unwrap();
        let mut store = Store::default();
        let brief = open(&json!({"task": "check the page", "attach": ["file:rules.md", "link:https://example.org/page"]}),
                         &mut store, &[dir.clone()], &["example.org".to_string()]).unwrap();
        let text = render_check(&brief);
        assert!(text.contains("steps"), "{}", text);
        assert!(text.contains("1 findings."), "{}", text);
        assert!(text.contains("could not be read"), "{}", text);
        assert!(text.contains("next: ") && text.contains("\"tool\":\"fetch\""), "{}", text);
        let _ = fs::remove_dir_all(&dir);
    }
}
