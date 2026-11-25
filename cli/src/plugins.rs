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

/// One skill or one command of a plugin, as the plugins screen shows it.
#[derive(Clone, Default)]
pub struct Item {
    pub name: String,
    pub description: String,
    pub file: String,
    pub argument_hint: String,
}

/// One registered plugin with everything the screen and the slash commands need.
#[derive(Clone, Default)]
pub struct Entry {
    pub name: String,
    pub dir: String,
    pub allow: bool,
    pub skills: Vec<Item>,
    pub commands: Vec<Item>,
    pub servers: Vec<String>,
}

fn items(value: &Value) -> Vec<Item> {
    value.as_array().map(|list| list.iter().map(|i| Item {
        name: i["name"].as_str().unwrap_or("").to_string(),
        description: i["description"].as_str().unwrap_or("").to_string(),
        file: i["file"].as_str().unwrap_or("").to_string(),
        argument_hint: i["argumentHint"].as_str().or_else(|| i["argument_hint"].as_str()).unwrap_or("").to_string(),
    }).collect()).unwrap_or_default()
}

/// The plugins as typed rows. A registry from an older version gets its command folder read here, without a write.
pub fn entries(ctx: &Ctx) -> Vec<Entry> {
    let reg = load(ctx);
    let mut out = Vec::new();
    for p in reg["plugins"].as_array().cloned().unwrap_or_default() {
        let dir = p["dir"].as_str().unwrap_or("").to_string();
        let commands = if p["commands"].is_array() { items(&p["commands"]) } else { items(&json!(scan_commands(Path::new(&dir)))) };
        out.push(Entry {
            name: p["name"].as_str().unwrap_or("?").to_string(),
            dir,
            allow: p["allow"].as_bool().unwrap_or(false),
            skills: items(&p["skills"]),
            commands,
            servers: p["mcpServers"].as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default(),
        });
    }
    out
}

/// One line per plugin command, for /help and for the plugins screen.
pub fn command_index(ctx: &Ctx, only: Option<&str>) -> String {
    let mut out = String::new();
    for e in entries(ctx).into_iter().filter(|e| only.map(|name| name == e.name).unwrap_or(true)) {
        for c in e.commands {
            let hint = if c.argument_hint.is_empty() { String::new() } else { format!(" {}", c.argument_hint) };
            out.push_str(&format!("/{}:{}{} · {}\n", e.name, c.name, hint, c.description.chars().take(100).collect::<String>()));
        }
    }
    out
}

/// The body of a command without its YAML front matter.
fn command_body(text: &str) -> String {
    if let Some(rest) = text.strip_prefix("---") {
        if let Some((_, body)) = rest.split_once("\n---") {
            return body.trim_start_matches(['\r', '\n']).to_string();
        }
    }
    text.to_string()
}

fn expand_args(template: &str, args: &str) -> String {
    let mut out = template.replace("$ARGUMENTS", args).replace("$@", args);
    let parts: Vec<&str> = args.split_whitespace().collect();
    for (i, part) in parts.iter().enumerate().take(9) {
        out = out.replace(&format!("${}", i + 1), part);
    }
    let has_placeholder = template.contains("$ARGUMENTS") || template.contains("$@")
        || (1..=9).any(|i| template.contains(&format!("${}", i)));
    if !args.is_empty() && !has_placeholder {
        out.push_str(&format!("\n\nArguments: {}", args));
    }
    out
}

/// Expands `/plugin:command args` or an unambiguous `/command args` into the prompt body. None means no such command.
pub fn expand_command(ctx: &Ctx, name: &str, args: &str) -> Option<String> {
    let wanted = name.trim().trim_start_matches('/');
    let mut exact: Vec<(Entry, Item)> = Vec::new();
    let mut bare: Vec<(Entry, Item)> = Vec::new();
    for entry in entries(ctx) {
        for command in &entry.commands {
            let full = format!("{}:{}", entry.name, command.name);
            if full == wanted { exact.push((entry.clone(), command.clone())); }
            else if command.name == wanted && !wanted.contains(':') { bare.push((entry.clone(), command.clone())); }
        }
    }
    let (entry, command) = match exact.into_iter().next().or_else(|| if bare.len() == 1 { bare.pop() } else { None }) {
        Some(found) => found,
        None => return None,
    };
    let text = fs::read_to_string(&command.file).ok()?;
    Some(format!("Plugin command /{}:{}.\n\n{}", entry.name, command.name, expand_args(&command_body(&text), args.trim())))
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
    let commands = scan_commands(&dir);
    let mut reg = load(ctx);
    let list = reg["plugins"].as_array_mut().unwrap();
    let allow = list.iter().any(|p| p["name"].as_str() == Some(&name) && p["allow"].as_bool().unwrap_or(false));      // a plugin added again keeps its state
    list.retain(|p| p["name"].as_str() != Some(&name));
    let (nc, ns, nm) = (commands.len(), skills.len(), servers.as_object().map(|o| o.len()).unwrap_or(0));
    list.push(json!({"name": name, "dir": dir.to_string_lossy(), "allow": allow, "commands": commands, "skills": skills, "mcpServers": servers}));
    let _ = fs::write(registry(ctx), serde_json::to_string_pretty(&reg).unwrap());
    format!("plugin {} registered: {} commands, {} skills, {} MCP servers, tools {}{}", name, nc, ns, nm,
            if allow { "allowed" } else { "denied (penelope plugin allow NAME)" },
            if nc > 0 { format!(" · run a command as /{}:NAME", name) } else { String::new() })
}

/// The command Markdown files below `commands/` (and a marketplace copy under `.claude-plugin/commands`).
fn scan_commands(dir: &Path) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    let mut stack: Vec<(PathBuf, usize)> = vec![(dir.join("commands"), 0), (dir.join(".claude-plugin").join("commands"), 0)];
    while let Some((at, depth)) = stack.pop() {
        let Ok(entries) = fs::read_dir(&at) else { continue };
        for e in entries.flatten() {
            let path = e.path();
            if path.is_dir() {
                if depth < 2 { stack.push((path, depth + 1)); }
                continue;
            }
            if path.extension().map(|x| x != "md").unwrap_or(true) { continue; }
            let Ok(text) = fs::read_to_string(&path) else { continue };
            let fallback = path.file_stem().map(|x| x.to_string_lossy().to_string()).unwrap_or_else(|| "command".to_string());
            let mut name = front(&text, "name");
            if name.is_empty() { name = fallback; }
            name = name.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_' || *c == ':' || *c == '.').collect();
            if name.is_empty() || out.iter().any(|c| c["name"].as_str() == Some(name.as_str())) { continue; }
            let description = front(&text, "description");
            let description: String = if description.is_empty() {
                command_body(&text).lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").chars().take(120).collect()
            } else { description.chars().take(240).collect() };
            let argument_hint = { let h = front(&text, "argument-hint"); if h.is_empty() { front(&text, "argument_hint") } else { h } };
            out.push(json!({"name": name, "description": description, "argumentHint": argument_hint, "file": path.to_string_lossy()}));
        }
    }
    out.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    out
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
    for p in reg["plugins"].as_array().unwrap() {
        let default = p["allow"].as_bool().unwrap_or(false);
        for (k, s) in p["mcpServers"].as_object().unwrap() {
            if default {
                out.push(format!("mcp__{}__*", tool_server(k)));
            } else {
                for tool in explicitly_allowed(s) {
                    out.push(format!("mcp__{}__{}", tool_server(k), tool));
                }
            }
        }
    }
    for (k, s) in mcp_load(ctx)["servers"].as_object().cloned().unwrap_or_default() {
        if s["allow"].as_bool().unwrap_or(false) {
            out.push(format!("mcp__{}__*", tool_server(&k)));
        } else {
            for tool in explicitly_allowed(&s) {
                out.push(format!("mcp__{}__{}", tool_server(&k), tool));
            }
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
        let commands = if p["commands"].is_array() { items(&p["commands"]) } else { items(&json!(scan_commands(Path::new(p["dir"].as_str().unwrap_or(""))))) };
        out.push_str(&format!("{} ({}) · tools {} · {} commands, {} skills\n", p["name"].as_str().unwrap_or("?"), crate::files::show(Path::new(p["dir"].as_str().unwrap_or(""))),
                              if p["allow"].as_bool().unwrap_or(false) { "allowed" } else { "denied" }, commands.len(),
                              p["skills"].as_array().map(|a| a.len()).unwrap_or(0)));
        for c in &commands {
            let hint = if c.argument_hint.is_empty() { String::new() } else { format!(" {}", c.argument_hint) };
            out.push_str(&format!("  command /{}:{}{} · {}\n", p["name"].as_str().unwrap_or("?"), c.name, hint,
                                  c.description.chars().take(90).collect::<String>()));
        }
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
    if out.is_empty() { "no plugins registered. /plugins install LINK, /plugins import or penelope plugin add DIR.\n".to_string() } else {
        out.push_str("\nA plugin command runs as /PLUGIN:COMMAND ARGS. /plugins opens the screen.\n");
        out
    }
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

/// True when one tool of a server may run. `default` is the old switch of the plugin or of the server: it holds for
/// every tool that has no rule of its own. A name in `deny_tools`/`denyTools` or a false entry in `tools` wins; a name
/// in `allow_tools`/`allowTools` or a true entry allows that tool even when the default is false.
pub fn tool_allowed(server: &Value, default: bool, tool: &str) -> bool {
    let tool = tool.trim();
    if tool.is_empty() {
        return false;
    }
    let listed = |keys: &[&str]| keys.iter().any(|key| {
        server.get(*key).and_then(Value::as_array).map(|list| list.iter().any(|v| v.as_str() == Some(tool))).unwrap_or(false)
    });
    if listed(&["deny_tools", "denyTools", "denied_tools", "deniedTools"]) {
        return false;
    }
    if listed(&["allow_tools", "allowTools", "allowed_tools", "allowedTools"]) {
        return true;
    }
    for key in ["tools", "tool_rules", "toolRules"] {
        if let Some(rule) = server.get(key).and_then(|rules| rules.get(tool)) {
            if let Some(value) = tool_rule_value(rule) {
                return value;
            }
        }
    }
    default
}

/// The tools a server with the default `false` opens by name. It feeds the allow list of the claude command, so a
/// single named allow works on both paths.
pub(crate) fn explicitly_allowed(server: &Value) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for key in ["allow_tools", "allowTools", "allowed_tools", "allowedTools"] {
        if let Some(list) = server.get(key).and_then(Value::as_array) {
            names.extend(list.iter().filter_map(|v| v.as_str()).map(str::to_string));
        }
    }
    for key in ["tools", "tool_rules", "toolRules"] {
        if let Some(map) = server.get(key).and_then(Value::as_object) {
            for (name, rule) in map {
                if tool_rule_value(rule) == Some(true) {
                    names.push(name.clone());
                }
            }
        }
    }
    names.sort();
    names.dedup();
    names.retain(|name| tool_allowed(server, false, name));
    names
}

fn tool_rule_value(rule: &Value) -> Option<bool> {
    if let Some(value) = rule.as_bool() {
        return Some(value);
    }
    if let Some(value) = rule.get("allow").and_then(Value::as_bool) {
        return Some(value);
    }
    if let Some(text) = rule.as_str() {
        return match text.trim().to_lowercase().as_str() {
            "allow" | "allowed" | "true" | "yes" | "on" => Some(true),
            "deny" | "denied" | "false" | "no" | "off" => Some(false),
            _ => None,
        };
    }
    None
}

fn remove_tool_name(server: &mut Value, key: &str, tool: &str) {
    if let Some(list) = server.get_mut(key).and_then(Value::as_array_mut) {
        list.retain(|v| v.as_str() != Some(tool));
    }
}

fn set_tool_rule(server: &mut Value, tool: &str, allow: bool) {
    if !server.is_object() {
        *server = json!({});
    }
    for key in ["deny_tools", "denyTools", "denied_tools", "deniedTools"] {
        if allow {
            remove_tool_name(server, key, tool);
        }
    }
    for key in ["allow_tools", "allowTools", "allowed_tools", "allowedTools"] {
        if !allow {
            remove_tool_name(server, key, tool);
        }
    }
    if !server["tools"].is_object() {
        server["tools"] = json!({});
    }
    server["tools"][tool] = json!(allow);
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

/// Allows or denies one tool of a standalone or plugin server. The old `allow` switch of the server stays the default
/// for every other tool. The rule is kept with the server, so a standalone server keeps it in `mcp.json` and a plugin
/// server keeps it in `plugins.json` until the plugin is added again.
pub fn mcp_set_tool_allow(ctx: &Ctx, name: &str, tool: &str, allow: bool) -> String {
    let tool = tool.trim();
    if tool.is_empty() {
        return "NOT changed. Give the name of the tool.".to_string();
    }
    let state = if allow { "allowed" } else { "denied" };
    let mut standalone = mcp_load(ctx);
    if let Some(server) = standalone["servers"].get_mut(name) {
        set_tool_rule(server, tool, allow);
        return match mcp_save(ctx, &standalone) {
            Ok(_) => format!("MCP server {}: tool {} {}", name, tool, state),
            Err(e) => format!("NOT changed. {}", e),
        };
    }
    let mut reg = load(ctx);
    let mut found = false;
    if let Some(list) = reg["plugins"].as_array_mut() {
        for p in list {
            if let Some(server) = p["mcpServers"].get_mut(name) {
                set_tool_rule(server, tool, allow);
                found = true;
                break;
            }
        }
    }
    if !found {
        return format!("no MCP server named {}. penelope mcp list shows the names.", name);
    }
    match fs::write(registry(ctx), serde_json::to_string_pretty(&reg).unwrap()) {
        Ok(_) => format!("MCP server {}: tool {} {}", name, tool, state),
        Err(e) => format!("NOT changed. cannot write {}: {}", registry(ctx).display(), e),
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
        let report = add(&ctx, &plugin.to_string_lossy());
        assert!(report.contains("0 commands, 0 skills, 2 MCP servers, tools denied (penelope plugin allow NAME)"), "{}", report);
        assert!(list(&ctx).contains("· tools denied") && allowed(&ctx).is_empty() && servers(&ctx).as_object().unwrap().len() == 2);
        assert!(set_allow(&ctx, "other", true).starts_with("no plugin named other"));
        assert!(set_allow(&ctx, "demo.notes", true).contains("2 MCP servers are allowed"));
        assert_eq!(allowed(&ctx), vec!["mcp__demo_notes-db__*", "mcp__demo_notes-web_search__*"]);
        assert!(add(&ctx, &plugin.to_string_lossy()).ends_with("tools allowed") && list(&ctx).contains("· tools allowed"));
        assert!(set_allow(&ctx, "demo.notes", false).contains("denied") && allowed(&ctx).is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_tool_rule_overrides_the_switch_of_its_server() {
        let server = json!({"allow": true, "deny_tools": ["danger"], "tools": {"quiet": false, "read": {"allow": true}}});
        assert!(tool_allowed(&server, true, "other"));
        assert!(!tool_allowed(&server, true, "danger"));
        assert!(!tool_allowed(&server, true, "quiet"));
        assert!(tool_allowed(&server, true, "read"));
        let closed = json!({"allow": false, "allow_tools": ["read"]});
        assert!(tool_allowed(&closed, false, "read"));
        assert!(!tool_allowed(&closed, false, "write"));
        let named = json!({"allow": true, "tools": {"read": "deny", "write": "allow"}});
        assert!(!tool_allowed(&named, true, "read"));
        assert!(tool_allowed(&named, true, "write"));
        assert!(!tool_allowed(&server, true, ""));
        let named_only = json!({"allow": false, "allow_tools": ["read"], "tools": {"write": true, "shut": false}});
        assert_eq!(explicitly_allowed(&named_only), vec!["read".to_string(), "write".to_string()]);
    }

    #[test]
    fn a_tool_of_a_standalone_and_a_plugin_server_can_be_denied() {
        let dir = std::env::temp_dir().join(format!("penelope-plugins-tools-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let plugin = dir.join("demo");
        fs::create_dir_all(plugin.join(".claude-plugin")).unwrap();
        fs::write(plugin.join(".claude-plugin/plugin.json"), r#"{"name": "demo.notes"}"#).unwrap();
        fs::write(plugin.join(".mcp.json"), r#"{"mcpServers": {"echo": {"command": "true"}}}"#).unwrap();
        let ctx = crate::config::load(&dir.join(".penelope"));
        assert!(add(&ctx, &plugin.to_string_lossy()).contains("demo.notes"));
        set_allow(&ctx, "demo.notes", true);
        let (server, allow) = server_entry(&ctx, "demo.notes-echo").unwrap();
        assert!(tool_allowed(&server, allow, "echo"));
        assert!(mcp_set_tool_allow(&ctx, "demo.notes-echo", "echo", false).contains("denied"));
        let (server, allow) = server_entry(&ctx, "demo.notes-echo").unwrap();
        assert!(!tool_allowed(&server, allow, "echo") && tool_allowed(&server, allow, "other"));
        assert!(mcp_set_tool_allow(&ctx, "missing", "echo", false).starts_with("no MCP server"));
        assert!(mcp_add(&ctx, "solo", "true", &[], true).contains("added"));
        assert!(mcp_set_tool_allow(&ctx, "solo", "run", false).contains("denied"));
        let (server, allow) = server_entry(&ctx, "solo").unwrap();
        assert!(!tool_allowed(&server, allow, "run"));
        assert!(mcp_set_tool_allow(&ctx, "solo", "run", true).contains("allowed"));
        let (server, allow) = server_entry(&ctx, "solo").unwrap();
        assert!(tool_allowed(&server, allow, "run"));
        assert!(mcp_set_tool_allow(&ctx, "solo", "", true).starts_with("NOT changed"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn commands_are_registered_shown_and_expanded_with_their_arguments() {
        let dir = std::env::temp_dir().join(format!("penelope-plugins-commands-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let plugin = dir.join("notes");
        fs::create_dir_all(plugin.join("commands")).unwrap();
        fs::create_dir_all(plugin.join(".claude-plugin")).unwrap();
        fs::write(plugin.join(".claude-plugin/plugin.json"), r#"{"name": "notes"}"#).unwrap();
        fs::write(plugin.join("commands/summarize.md"),
                  "---\nname: summarize\ndescription: Summarize a file\nargument-hint: FILE\n---\nRead $1 and summarize it. Extra: $ARGUMENTS\n").unwrap();
        let ctx = crate::config::load(&dir.join(".penelope"));
        let report = add(&ctx, &plugin.to_string_lossy());
        assert!(report.contains("1 commands") && report.contains("/notes:NAME"), "{}", report);
        let rows = entries(&ctx);
        assert_eq!((rows.len(), rows[0].commands.len()), (1, 1));
        assert_eq!((rows[0].commands[0].name.as_str(), rows[0].commands[0].argument_hint.as_str()), ("summarize", "FILE"));
        assert!(list(&ctx).contains("command /notes:summarize FILE · Summarize a file"));
        assert!(command_index(&ctx, None).starts_with("/notes:summarize FILE ·"));
        assert!(command_index(&ctx, Some("notes")).contains("summarize") && command_index(&ctx, Some("other")).is_empty());
        let text = expand_command(&ctx, "notes:summarize", "a.txt now").unwrap();
        assert!(text.starts_with("Plugin command /notes:summarize.") && text.contains("Read a.txt") && text.contains("Extra: a.txt now"), "{}", text);
        assert!(expand_command(&ctx, "summarize", "").is_some());
        assert!(expand_command(&ctx, "other:summarize", "").is_none());
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
