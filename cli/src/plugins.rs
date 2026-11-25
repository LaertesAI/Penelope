//! Plugins in the format of Claude Code: a folder with `.claude-plugin/plugin.json`, `skills/<name>/SKILL.md` and
//! `.mcp.json`. Penelope registers the folder, offers the MCP servers to the main model and lists the skills by name
//! and description. The body of a skill is read only when the model asks for it. The tools of a plugin's MCP servers
//! run only after `penelope plugin allow NAME` set `"allow": true` for that plugin in `plugins.json`.

use crate::config::Ctx;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

fn registry(ctx: &Ctx) -> PathBuf {
    ctx.root.join("plugins.json")
}

pub fn load(ctx: &Ctx) -> Value {
    fs::read_to_string(registry(ctx)).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(json!({"plugins": []}))
}

fn front(text: &str, key: &str) -> String {
    // value of one key of the YAML front matter, single line
    let mut inside = false;
    for line in text.lines() {
        if line.trim() == "---" {
            if inside { break; }
            inside = true;
            continue;
        }
        if inside {
            if let Some(v) = line.strip_prefix(&format!("{}:", key)) {
                return v.trim().trim_matches('"').to_string();
            }
        }
    }
    String::new()
}

/// Registers a plugin folder. Returns a report line.
pub fn add(ctx: &Ctx, dir: &str) -> String {
    let dir = match fs::canonicalize(dir) { Ok(d) => d, Err(_) => return format!("no folder {}", dir) };
    let manifest: Value = fs::read_to_string(dir.join(".claude-plugin").join("plugin.json")).ok()
        .and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(json!({}));
    let name = manifest["name"].as_str().map(String::from).unwrap_or_else(|| dir.file_name().unwrap().to_string_lossy().to_string());
    let mut skills: Vec<Value> = Vec::new();
    for file in skill_files(&dir) {
        let fallback = file.parent().and_then(|p| p.file_name()).map(|p| p.to_string_lossy().to_string()).unwrap_or_else(|| name.clone());
        if let Ok(text) = fs::read_to_string(&file) {
            let n = front(&text, "name");
            let entry = json!({"name": if n.is_empty() { fallback } else { n }, "description": front(&text, "description"), "file": file.to_string_lossy()});
            if !skills.iter().any(|s| s["name"] == entry["name"]) {
                skills.push(entry);
            }
        }
    }
    let mut servers = json!({});
    for src in [dir.join(".mcp.json")] {
        if let Some(v) = fs::read_to_string(src).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok()) {
            let m = if v["mcpServers"].is_object() { v["mcpServers"].clone() } else { v };
            if let Some(obj) = m.as_object() {
                for (k, s) in obj {
                    servers[format!("{}-{}", name, k)] = s.clone();
                }
            }
        }
    }
    if let Some(obj) = manifest["mcpServers"].as_object() {
        for (k, s) in obj {
            servers[format!("{}-{}", name, k)] = s.clone();
        }
    }
    let mut reg = load(ctx);
    let list = reg["plugins"].as_array_mut().unwrap();
    let allow = list.iter().any(|p| p["name"].as_str() == Some(&name) && p["allow"].as_bool().unwrap_or(false));      // a plugin added again keeps its state
    list.retain(|p| p["name"].as_str() != Some(&name));
    let (ns, nm) = (skills.len(), servers.as_object().map(|o| o.len()).unwrap_or(0));
    list.push(json!({"name": name, "dir": dir.to_string_lossy(), "allow": allow, "skills": skills, "mcpServers": servers}));
    let _ = fs::write(registry(ctx), serde_json::to_string_pretty(&reg).unwrap());
    format!("plugin {} registered: {} skills, {} MCP servers, tools {}", name, ns, nm, if allow { "allowed" } else { "denied (penelope plugin allow NAME)" })
}

/// Every `SKILL.md` below a plugin folder, at most three levels deep. This covers `skills/NAME/SKILL.md`, a single
/// `SKILL.md`, and the flat `skills/NAME.md` some Claude plugins ship. Folders of build output are skipped.
fn skill_files(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![(dir.to_path_buf(), 0usize)];
    while let Some((at, depth)) = stack.pop() {
        let Ok(entries) = fs::read_dir(&at) else { continue };
        for e in entries.flatten() {
            let path = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            if path.is_dir() {
                if depth < 3 && !matches!(name.as_str(), ".git" | "node_modules" | ".claude-plugin" | "dist" | "build" | "target") {
                    stack.push((path, depth + 1));
                }
            } else if name == "SKILL.md" || (name.ends_with(".md") && at.file_name().map(|p| p == "skills").unwrap_or(false)) {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

/// The plugins of a Claude Code marketplace file: `{"plugins":[{"name":"x","source":"./x"}]}`. Each source is added
/// as a plugin of its own. Returns one line per plugin.
fn install_marketplace(ctx: &Ctx, dir: &std::path::Path) -> Option<String> {
    let text = fs::read_to_string(dir.join(".claude-plugin").join("marketplace.json")).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    let items: Vec<(String, String)> = match &v["plugins"] {
        Value::Array(a) => a.iter().filter_map(|p| Some((p["name"].as_str()?.to_string(), p["source"].as_str()?.to_string()))).collect(),
        Value::Object(o) => o.iter().filter_map(|(name, p)| Some((name.clone(), p["source"].as_str()?.to_string()))).collect(),
        _ => Vec::new(),
    };
    if items.is_empty() {
        return None;
    }
    let mut lines = vec![format!("marketplace {}: {} plugins", dir.file_name().unwrap_or_default().to_string_lossy(), items.len())];
    for (name, source) in items {
        let target = if source.starts_with('/') { PathBuf::from(&source) } else { dir.join(source.trim_start_matches("./")) };
        if !target.is_dir() {
            lines.push(format!("{}: source {} is missing", name, source));
            continue;
        }
        lines.push(format!("{}: {}", name, add(ctx, &target.to_string_lossy())));
    }
    Some(lines.join("\n"))
}

/// Sets `"allow"` of one plugin. True puts the tools of its MCP servers on the allow list of the claude command.
pub fn set_allow(ctx: &Ctx, name: &str, allow: bool) -> String {
    let mut reg = load(ctx);
    let Some(p) = reg["plugins"].as_array_mut().and_then(|l| l.iter_mut().find(|p| p["name"].as_str() == Some(name))) else {
        return format!("no plugin named {} (penelope plugin list)", name);
    };
    p["allow"] = json!(allow);
    let servers = p["mcpServers"].as_object().map(|o| o.len()).unwrap_or(0);
    let _ = fs::write(registry(ctx), serde_json::to_string_pretty(&reg).unwrap());
    format!("plugin {}: the tools of its {} MCP servers are {} from the next request on", name, servers, if allow { "allowed" } else { "denied" })
}

/// The name of a server inside a tool name of the claude command. Its documentation says that characters such as `.`
/// or spaces become `_`. Letters, digits, `_` and `-` stay.
fn tool_server(name: &str) -> String {
    name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' }).collect()
}

/// Allow rules for the claude command: one rule per MCP server of a plugin with `"allow": true`. The claude command
/// names a tool `mcp__SERVER__TOOL`, and its documentation gives `mcp__SERVER__*` as the rule for every tool of a server.
pub fn allowed(ctx: &Ctx) -> Vec<String> {
    let reg = load(ctx);
    let mut out = Vec::new();
    for p in reg["plugins"].as_array().unwrap().iter().filter(|p| p["allow"].as_bool().unwrap_or(false)) {
        for (k, _) in p["mcpServers"].as_object().unwrap() {
            out.push(format!("mcp__{}__*", tool_server(k)));
        }
    }
    for (k, s) in mcp_load(ctx)["servers"].as_object().cloned().unwrap_or_default() {
        if s["allow"].as_bool().unwrap_or(false) {
            out.push(format!("mcp__{}__*", tool_server(&k)));
        }
    }
    out
}

fn plugin_home(ctx: &Ctx) -> PathBuf {
    match ctx.cfg["plugin_home"].as_str().filter(|p| !p.trim().is_empty()) {
        Some(p) => PathBuf::from(p),
        None => crate::platform::home().join(".penelope").join("plugins"),
    }
}

fn safe_name(name: &str) -> String {
    name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '-' }).collect()
}

fn zip_target(target: &str) -> bool {
    let path = target.split(['?', '#']).next().unwrap_or(target).to_lowercase();
    path.ends_with(".zip")
}

fn install_zip_file(ctx: &Ctx, archive: &Path, name: &str) -> String {
    let base = plugin_home(ctx);
    if let Err(e) = fs::create_dir_all(&base) {
        return format!("NOT installed. The plugin folder could not be made: {}", e);
    }
    let dir = base.join(name);
    if dir.exists() {
        return format!("NOT installed. {} already exists. Remove it or register it with penelope plugin add.", dir.display());
    }
    // Unpack into a hidden staging folder first, so a single wrapping folder can become the plugin folder itself.
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let staging = base.join(format!(".{}-unpack-{}", name, stamp));
    let unpacked = match crate::files::extract_zip(archive, &staging) {
        Ok(u) => u,
        Err(e) => return e,
    };
    let root = unpacked.top.clone().unwrap_or_else(|| staging.clone());
    let moved = fs::rename(&root, &dir);
    if moved.is_err() {
        let _ = fs::remove_dir_all(&staging);
        return format!("NOT installed. The unpacked folder could not be moved to {}: {}", dir.display(), moved.unwrap_err());
    }
    let _ = fs::remove_dir_all(&staging);
    if let Some(report) = install_marketplace(ctx, &dir) {
        return format!("unpacked {} files into {} · {}", unpacked.files, dir.display(), report);
    }
    let report = add(ctx, &dir.to_string_lossy());
    format!("unpacked {} files into {} · {}", unpacked.files, dir.display(), report)
}

fn install_zip_url(ctx: &Ctx, target: &str, name: &str) -> String {
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let tmp = std::env::temp_dir().join(format!("penelope-plugin-{}-{}.zip", std::process::id(), stamp));
    if let Some(path) = target.strip_prefix("file://") {
        let path = PathBuf::from(path);
        let report = install_zip_file(ctx, &path, name);
        return report;
    }
    use std::io::Read;
    let response = match crate::net::agent(60).get(target).call() {
        Ok(r) if (200..300).contains(&r.status()) => r,
        Ok(r) => return format!("NOT installed. The server answered with {}.", r.status()),
        Err(e) => return format!("NOT installed. The ZIP could not be loaded: {}", e),
    };
    let mut bytes = Vec::new();
    if let Err(e) = response.into_reader().take(50 * 1024 * 1024).read_to_end(&mut bytes) {
        return format!("NOT installed. The ZIP could not be read: {}", e);
    }
    if bytes.len() >= 50 * 1024 * 1024 {
        return "NOT installed. The ZIP is over 50 MB.".to_string();
    }
    if let Err(e) = fs::write(&tmp, &bytes) {
        return format!("NOT installed. The ZIP could not be stored: {}", e);
    }
    let report = install_zip_file(ctx, &tmp, name);
    let _ = fs::remove_file(&tmp);
    report
}

/// Installs a plugin from a link, a folder or a ZIP: a git address is cloned into `~/.penelope/plugins/NAME`, a folder
/// is registered as it is, a ZIP is unpacked there first. The plugin then waits with its tools denied until
/// `plugin allow NAME` or `/plugins allow NAME`.
pub fn install(ctx: &Ctx, target: &str) -> String {
    let target = target.trim().trim_matches(|c| c == '"' || c == '\'');
    if target.is_empty() {
        return "NOT installed. Give a link or a folder: /plugins install https://host/owner/plugin".to_string();
    }
    let local = Path::new(target);
    if local.is_file() && zip_target(target) {
        let name = safe_name(local.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default().as_str());
        if name.is_empty() { return "NOT installed. The ZIP has no usable name.".to_string(); }
        return install_zip_file(ctx, local, &name);
    }
    let remote = target.starts_with("https://") || target.starts_with("http://") || target.starts_with("git@") || target.starts_with("ssh://") || target.starts_with("file://");
    if remote && zip_target(target) {
        let name = target.split(['?', '#']).next().unwrap_or(target).trim_end_matches('/');
        let name = safe_name(name.rsplit('/').next().unwrap_or("plugin").trim_end_matches(".zip"));
        if name.is_empty() { return "NOT installed. The link has no usable name.".to_string(); }
        return install_zip_url(ctx, target, &name);
    }
    if !remote {
        let path = Path::new(target);
        if !path.exists() {
            return format!("NOT installed. {} is no folder and no link.", target);
        }
        if let Some(report) = install_marketplace(ctx, path) {
            return report;
        }
        return add(ctx, target);
    }
    let name = target.trim_end_matches('/').trim_end_matches(".git").rsplit(&['/', ':'][..]).next().unwrap_or("plugin");
    let name: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '-' }).collect();
    if name.is_empty() {
        return "NOT installed. The link has no name at its end.".to_string();
    }
    let base = plugin_home(ctx);
    let dir = base.join(&name);
    if let Some(parent) = dir.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Some(report) = install_marketplace(ctx, &dir) {
        return report;
    }
    if dir.join(".git").is_dir() {
        let out = std::process::Command::new("git").args(["-C", &dir.to_string_lossy(), "pull", "--ff-only"]).output();
        match out {
            Ok(o) if o.status.success() => { let _ = add(ctx, &dir.to_string_lossy()); return format!("plugin {} updated from {} and registered", name, target); }
            Ok(o) => return format!("NOT updated. git pull said: {}", String::from_utf8_lossy(&o.stderr).trim().chars().take(300).collect::<String>()),
            Err(e) => return format!("NOT updated. git could not start: {}", e),
        }
    }
    let out = std::process::Command::new("git").args(["clone", "--depth", "1", target, &dir.to_string_lossy()]).output();
    match out {
        Ok(o) if o.status.success() => {
            let report = add(ctx, &dir.to_string_lossy());
            format!("cloned into {} · {}", dir.display(), report)
        }
        Ok(o) => format!("NOT installed. git clone said: {}", String::from_utf8_lossy(&o.stderr).trim().chars().take(300).collect::<String>()),
        Err(e) => format!("NOT installed. git could not start: {}", e),
    }
}

/// Takes a plugin out of the registry. The folder is left on disk on purpose: a person can add it again or delete it.
pub fn remove(ctx: &Ctx, name: &str) -> String {
    let mut reg = load(ctx);
    let Some(list) = reg["plugins"].as_array_mut() else { return "no plugin registry".to_string() };
    let Some(at) = list.iter().position(|p| p["name"].as_str() == Some(name)) else {
        return format!("no plugin named {}. penelope plugin list shows the names.", name);
    };
    let dir = list[at]["dir"].as_str().unwrap_or("").to_string();
    list.remove(at);
    let _ = fs::write(registry(ctx), serde_json::to_string_pretty(&reg).unwrap());
    format!("plugin {} removed from the registry; the folder stays at {}", name, dir)
}

/// Re-reads a registered plugin, or pulls a git install first. A ZIP install has no source to pull; it is re-scanned.
pub fn update(ctx: &Ctx, name: &str) -> String {
    let reg = load(ctx);
    let Some(entry) = reg["plugins"].as_array().and_then(|a| a.iter().find(|p| p["name"].as_str() == Some(name))) else {
        return format!("no plugin named {}. penelope plugin list shows the names.", name);
    };
    let dir = entry["dir"].as_str().unwrap_or("");
    if dir.is_empty() || !Path::new(dir).is_dir() {
        return format!("NOT updated. The folder of {} is gone: {}", name, dir);
    }
    if Path::new(dir).join(".git").is_dir() {
        let out = std::process::Command::new("git").args(["-C", dir, "pull", "--ff-only"]).output();
        match out {
            Ok(o) if o.status.success() => format!("pulled {} · {}", dir, add(ctx, dir)),
            Ok(o) => format!("NOT updated. git pull said: {}", String::from_utf8_lossy(&o.stderr).trim().chars().take(300).collect::<String>()),
            Err(e) => format!("NOT updated. git could not start: {}", e),
        }
    } else {
        format!("re-read {} · {}", dir, add(ctx, dir))
    }
}

fn plugin_dirs(ctx: &Ctx) -> Vec<PathBuf> {
    if let Some(list) = ctx.cfg["plugin_dirs"].as_array() {
        return list.iter().filter_map(|v| v.as_str()).map(PathBuf::from).collect();
    }
    let home = crate::platform::home();
    vec![home.join(".claude/plugins"), home.join(".claude/skills"), home.join(".config/opencode/skills"),
         home.join(".config/opencode/plugin"), home.join(".codex/skills"), home.join(".cursor/skills"), home.join(".agents/skills"),
         home.join(".penelope/plugins")]
}

fn looks_like_plugin(dir: &Path) -> bool {
    dir.join(".claude-plugin").join("plugin.json").is_file() || dir.join("SKILL.md").is_file() || dir.join("skills").is_dir() || dir.join(".mcp.json").is_file()
}

/// On the first start Penelope takes over the plugins that another harness already installed on this PC. A marker file
/// keeps it to one import per folder. Returns one line per new plugin.
pub fn import_known(ctx: &Ctx) -> Vec<String> {
    let marker = ctx.root.join("imported.json");
    let mut done: Vec<String> = fs::read_to_string(&marker).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    let mut out = Vec::new();
    for root in plugin_dirs(ctx) {
        if !root.is_dir() || done.contains(&root.to_string_lossy().to_string()) {
            continue;
        }
        // Claude Code keeps the real install paths in installed_plugins.json, one entry per scope and version.
        let registered: Vec<PathBuf> = fs::read_to_string(root.join("installed_plugins.json")).ok()
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .and_then(|v| v["plugins"].as_object().map(|o| o.values().flat_map(|list| list.as_array().cloned().unwrap_or_default())
                .filter_map(|e| e["installPath"].as_str().map(PathBuf::from)).collect()))
            .unwrap_or_default();
        for path in &registered {
            if path.is_dir() && looks_like_plugin(&path) {
                let report = if path.join(".claude-plugin/marketplace.json").is_file() {
                    install_marketplace(ctx, &path).unwrap_or_else(|| add(ctx, &path.to_string_lossy()))
                } else {
                    add(ctx, &path.to_string_lossy())
                };
                out.push(format!("{}: {}", crate::files::show(&path), report));
            }
        }
        if registered.is_empty() {
            let mut found: Vec<PathBuf> = Vec::new();
            if looks_like_plugin(&root) {
                found.push(root.clone());
            } else if let Ok(entries) = fs::read_dir(&root) {
                for e in entries.flatten() {
                    if e.path().is_dir() && looks_like_plugin(&e.path()) {
                        found.push(e.path());
                    }
                }
            }
            for dir in found {
                let report = if dir.join(".claude-plugin/marketplace.json").is_file() {
                    install_marketplace(ctx, &dir).unwrap_or_else(|| add(ctx, &dir.to_string_lossy()))
                } else {
                    add(ctx, &dir.to_string_lossy())
                };
                if report.starts_with("plugin ") || report.starts_with("marketplace ") {
                    out.push(format!("{}: {}", crate::files::show(&dir), report));
                }
            }
        }
        done.push(root.to_string_lossy().to_string());
    }
    let _ = fs::write(marker, serde_json::to_string_pretty(&done).unwrap());
    out
}

pub fn list(ctx: &Ctx) -> String {
    let reg = load(ctx);
    let mut out = String::new();
    for p in reg["plugins"].as_array().unwrap() {
        out.push_str(&format!("{} ({}) · tools {}\n", p["name"].as_str().unwrap_or("?"), crate::files::show(Path::new(p["dir"].as_str().unwrap_or(""))),
                              if p["allow"].as_bool().unwrap_or(false) { "allowed" } else { "denied" }));
        for s in p["skills"].as_array().unwrap() {
            out.push_str(&format!("  skill {}: {}\n", s["name"].as_str().unwrap_or("?"), s["description"].as_str().unwrap_or("").chars().take(90).collect::<String>()));
        }
        for (k, _) in p["mcpServers"].as_object().unwrap() {
            out.push_str(&format!("  mcp {}\n", k));
        }
    }
    let learned = crate::skills::list(ctx);
    if !learned.is_empty() {
        out.push_str("\nskills learned here\n");
        for (name, description) in learned {
            out.push_str(&format!("  {}: {}\n", name, description.chars().take(90).collect::<String>()));
        }
    }
    if out.is_empty() { "no plugins registered\n".to_string() } else { out }
}

/// One line per skill for the system prompt.
pub fn skill_index(ctx: &Ctx) -> String {
    let reg = load(ctx);
    let mut out = String::new();
    for (name, description) in crate::skills::list(ctx) {
        out.push_str(&format!("- {}: {} (learned here)\n", name, description));
    }
    for p in reg["plugins"].as_array().unwrap() {
        for s in p["skills"].as_array().unwrap() {
            out.push_str(&format!("- {}: {}\n", s["name"].as_str().unwrap_or("?"), s["description"].as_str().unwrap_or("").chars().take(160).collect::<String>()));
        }
    }
    out
}

/// The body of a skill, at most 12,000 characters.
pub fn skill_body(ctx: &Ctx, name: &str) -> String {
    if let Some(body) = crate::skills::body(ctx, name) {
        return body.chars().take(12_000).collect();
    }
    let reg = load(ctx);
    for p in reg["plugins"].as_array().unwrap() {
        for s in p["skills"].as_array().unwrap() {
            if s["name"].as_str() == Some(name) {
                let text = fs::read_to_string(Path::new(s["file"].as_str().unwrap_or(""))).unwrap_or_default();
                return text.chars().take(12_000).collect();
            }
        }
    }
    format!("no skill named {}", name)
}

/// One MCP server by name, with whether a plugin that holds it is allowed. The name is the prefixed one in
/// `plugins.json`, for example `demo-echo`.
pub fn server_entry(ctx: &Ctx, name: &str) -> Option<(Value, bool)> {
    if let Some(server) = mcp_load(ctx)["servers"].get(name) {
        if !server.is_null() {
            return Some((server.clone(), server["allow"].as_bool().unwrap_or(false)));
        }
    }
    let reg = load(ctx);
    for p in reg["plugins"].as_array().unwrap() {
        if let Some(server) = p["mcpServers"].as_object().and_then(|o| o.get(name)) {
            return Some((server.clone(), p["allow"].as_bool().unwrap_or(false)));
        }
    }
    None
}

fn mcp_registry(ctx: &Ctx) -> PathBuf {
    ctx.root.join("mcp.json")
}

pub fn mcp_load(ctx: &Ctx) -> Value {
    fs::read_to_string(mcp_registry(ctx)).ok().and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| json!({"servers": {}}))
}

fn mcp_save(ctx: &Ctx, value: &Value) -> Result<(), String> {
    fs::write(mcp_registry(ctx), serde_json::to_string_pretty(value).unwrap()).map_err(|e| format!("cannot write {}: {}", mcp_registry(ctx).display(), e))
}

/// A server that is not owned by a plugin: `penelope mcp add NAME COMMAND [ARGS...]`.
pub fn mcp_add(ctx: &Ctx, name: &str, command: &str, args: &[String], allow: bool) -> String {
    let name = name.trim();
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')) {
        return "NOT added. The server name may hold letters, digits, - _ and . only.".to_string();
    }
    if command.trim().is_empty() {
        return "NOT added. Give the command that starts the server.".to_string();
    }
    if servers(ctx).as_object().map(|o| o.contains_key(name)).unwrap_or(false) {
        return format!("NOT added. A server named {} already exists. Remove it first or choose another name.", name);
    }
    let mut reg = mcp_load(ctx);
    reg["servers"][name] = json!({"command": command, "args": args, "allow": allow});
    match mcp_save(ctx, &reg) {
        Ok(_) => format!("MCP server {} added: {} {} · tools {}", name, command, args.join(" "), if allow { "allowed" } else { "denied (penelope mcp allow NAME)" }),
        Err(e) => format!("NOT added. {}", e),
    }
}

pub fn mcp_remove(ctx: &Ctx, name: &str) -> String {
    let mut reg = mcp_load(ctx);
    let Some(servers) = reg["servers"].as_object_mut() else { return "no standalone MCP server registry".to_string() };
    if servers.remove(name).is_none() {
        return format!("no standalone MCP server named {}. penelope mcp list shows the names.", name);
    }
    match mcp_save(ctx, &reg) {
        Ok(_) => format!("MCP server {} removed", name),
        Err(e) => format!("NOT removed. {}", e),
    }
}

pub fn mcp_set_allow(ctx: &Ctx, name: &str, allow: bool) -> String {
    let mut reg = mcp_load(ctx);
    let Some(server) = reg["servers"].get_mut(name) else {
        return format!("no standalone MCP server named {}. penelope mcp list shows the names.", name);
    };
    server["allow"] = json!(allow);
    match mcp_save(ctx, &reg) {
        Ok(_) => format!("MCP server {}: tools {}", name, if allow { "allowed" } else { "denied" }),
        Err(e) => format!("NOT changed. {}", e),
    }
}

/// The MCP servers of all plugins, for the configuration of the main model.
pub fn servers(ctx: &Ctx) -> Value {
    let reg = load(ctx);
    let mut out = json!({});
    for p in reg["plugins"].as_array().unwrap() {
        for (k, s) in p["mcpServers"].as_object().unwrap() {
            out[k] = s.clone();
        }
    }
    for (k, s) in mcp_load(ctx)["servers"].as_object().cloned().unwrap_or_default() {
        out[k] = s;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_allowed_plugin_puts_its_servers_on_the_allow_list() {
        let dir = std::env::temp_dir().join(format!("penelope-plugins-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let plugin = dir.join("demo");
        fs::create_dir_all(plugin.join(".claude-plugin")).unwrap();
        fs::write(plugin.join(".claude-plugin/plugin.json"), r#"{"name": "demo.notes"}"#).unwrap();
        fs::write(plugin.join(".mcp.json"), r#"{"mcpServers": {"db": {"command": "true"}, "web search": {"command": "true"}}}"#).unwrap();
        let ctx = crate::config::load(&dir.join(".penelope"));
        assert!(add(&ctx, &plugin.to_string_lossy()).ends_with("2 MCP servers, tools denied (penelope plugin allow NAME)"));
        assert!(list(&ctx).contains("· tools denied") && allowed(&ctx).is_empty() && servers(&ctx).as_object().unwrap().len() == 2);
        assert!(set_allow(&ctx, "other", true).starts_with("no plugin named other"));
        assert!(set_allow(&ctx, "demo.notes", true).contains("2 MCP servers are allowed"));
        assert_eq!(allowed(&ctx), vec!["mcp__demo_notes-db__*", "mcp__demo_notes-web_search__*"]);
        assert!(add(&ctx, &plugin.to_string_lossy()).ends_with("tools allowed") && list(&ctx).contains("· tools allowed"));
        assert!(set_allow(&ctx, "demo.notes", false).contains("denied") && allowed(&ctx).is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_known_folder_is_taken_over_once_and_a_folder_installs() {
        let dir = std::env::temp_dir().join(format!("penelope-plugins-import-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let known = dir.join("known");
        let demo = known.join("demo");
        fs::create_dir_all(demo.join(".claude-plugin")).unwrap();
        fs::write(demo.join(".claude-plugin/plugin.json"), r#"{"name": "taken.over"}"#).unwrap();
        fs::write(demo.join("SKILL.md"), "---\nname: over\ndescription: a skill from another harness\n---\nbody\n").unwrap();
        let other = dir.join("other");
        fs::create_dir_all(&other).unwrap();
        fs::write(other.join("SKILL.md"), "---\nname: other\ndescription: a second skill\n---\nbody\n").unwrap();
        let mut ctx = crate::config::load(&dir.join(".penelope"));
        ctx.cfg["plugin_dirs"] = serde_json::json!([known]);
        ctx.cfg["plugin_home"] = serde_json::json!(dir.join("home"));
        let first = import_known(&ctx);
        assert_eq!(first.len(), 1, "{:?}", first);
        assert!(first[0].contains("taken.over") && list(&ctx).contains("skill over"), "{}", first.join("\n"));
        assert!(import_known(&ctx).is_empty());
        assert!(install(&ctx, &other.to_string_lossy()).contains("other"));
        assert!(list(&ctx).contains("skill other"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_zip_plugin_is_unpacked_into_the_plugin_home_and_registered() {
        let dir = std::env::temp_dir().join(format!("penelope-plugins-zip-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let archive = dir.join("zip-plugin.zip");
        let file = fs::File::create(&archive).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        use std::io::Write;
        zip.start_file("zip-plugin/.claude-plugin/plugin.json", opts).unwrap();
        zip.write_all(br#"{"name": "zip.plugin"}"#).unwrap();
        zip.start_file("zip-plugin/SKILL.md", opts).unwrap();
        zip.write_all(b"---\nname: zipped\ndescription: from a zip\n---\nbody\n").unwrap();
        zip.finish().unwrap();
        let mut ctx = crate::config::load(&dir.join(".penelope"));
        ctx.cfg["plugin_home"] = serde_json::json!(dir.join("home"));
        let said = install(&ctx, &archive.to_string_lossy());
        assert!(said.contains("unpacked 2 files") && said.contains("zip.plugin"), "{}", said);
        assert!(dir.join("home/zip-plugin/SKILL.md").is_file());
        assert!(list(&ctx).contains("zipped") && list(&ctx).contains("zip.plugin"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_plugin_can_be_removed_and_rescanned() {
        let dir = std::env::temp_dir().join(format!("penelope-plugins-remove-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let plugin = dir.join("demo");
        fs::create_dir_all(plugin.join(".claude-plugin")).unwrap();
        fs::write(plugin.join(".claude-plugin/plugin.json"), r#"{"name": "remove.me"}"#).unwrap();
        fs::write(plugin.join("SKILL.md"), "---\nname: gone\ndescription: before\n---\nbody\n").unwrap();
        let ctx = crate::config::load(&dir.join(".penelope"));
        assert!(add(&ctx, &plugin.to_string_lossy()).contains("remove.me"));
        fs::write(plugin.join("SKILL.md"), "---\nname: back\ndescription: after\n---\nbody\n").unwrap();
        assert!(update(&ctx, "remove.me").contains("re-read") && list(&ctx).contains("back"));
        assert!(remove(&ctx, "remove.me").contains("removed from the registry") && !list(&ctx).contains("remove.me"));
        assert!(remove(&ctx, "remove.me").contains("no plugin named"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_claude_marketplace_installs_its_plugins() {
        let dir = std::env::temp_dir().join(format!("penelope-market-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let market = dir.join("market");
        fs::create_dir_all(market.join(".claude-plugin")).unwrap();
        for name in ["alpha", "beta"] {
            let p = market.join(name);
            fs::create_dir_all(p.join(".claude-plugin")).unwrap();
            fs::write(p.join(".claude-plugin/plugin.json"), format!(r#"{{"name": "{}"}}"#, name)).unwrap();
            fs::write(p.join("SKILL.md"), format!("---\nname: {} skill\ndescription: from the market\n---\nbody\n", name)).unwrap();
        }
        fs::write(market.join(".claude-plugin/marketplace.json"), r#"{"plugins":[{"name":"alpha","source":"./alpha"},{"name":"beta","source":"./beta"}]}"#).unwrap();
        let ctx = crate::config::load(&dir.join(".penelope"));
        let said = install(&ctx, &market.to_string_lossy());
        assert!(said.contains("marketplace market: 2 plugins") && said.contains("plugin alpha registered") && said.contains("plugin beta registered"), "{}", said);
        assert!(list(&ctx).contains("alpha skill") && list(&ctx).contains("beta skill"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_git_link_is_cloned_into_the_plugin_home() {
        let dir = std::env::temp_dir().join(format!("penelope-plugins-git-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let repo = dir.join("linked");
        fs::create_dir_all(repo.join(".claude-plugin")).unwrap();
        fs::write(repo.join(".claude-plugin/plugin.json"), r#"{"name": "linked.plugin"}"#).unwrap();
        fs::write(repo.join("SKILL.md"), "---\nname: linked\ndescription: from git\n---\nbody\n").unwrap();
        let git = |args: &[&str]| std::process::Command::new("git").current_dir(&repo).args(args).output();
        if !git(&["init", "-q"]).map(|o| o.status.success()).unwrap_or(false) { return; }
        git(&["add", "."]).unwrap();
        let commit = git(&["-c", "user.email=t@e.org", "-c", "user.name=T", "commit", "-q", "-m", "first"]).unwrap();
        if !commit.status.success() { return; }
        let ctx = {
            let mut c = crate::config::load(&dir.join(".penelope"));
            c.cfg["plugin_home"] = serde_json::json!(dir.join("home"));
            c
        };
        let said = install(&ctx, &format!("file://{}", repo.display()));
        assert!(said.starts_with("cloned into "), "{}", said);
        assert!(said.contains("linked.plugin") || list(&ctx).contains("linked"));
        assert!(dir.join("home/linked/.claude-plugin/plugin.json").is_file());
        assert!(install(&ctx, &format!("file://{}", repo.display())).contains("updated"));
        let _ = fs::remove_dir_all(&dir);
    }
}
