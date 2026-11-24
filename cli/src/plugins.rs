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
    let mut skills = Vec::new();
    let mut scan = |file: PathBuf, fallback: String| {
        if let Ok(text) = fs::read_to_string(&file) {
            let n = front(&text, "name");
            skills.push(json!({"name": if n.is_empty() { fallback } else { n }, "description": front(&text, "description"), "file": file.to_string_lossy()}));
        }
    };
    scan(dir.join("SKILL.md"), name.clone());
    if let Ok(entries) = fs::read_dir(dir.join("skills")) {
        for e in entries.flatten() {
            scan(e.path().join("SKILL.md"), e.file_name().to_string_lossy().to_string());
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
    if out.is_empty() { "no plugins registered\n".to_string() } else { out }
}

/// One line per skill for the system prompt.
pub fn skill_index(ctx: &Ctx) -> String {
    let reg = load(ctx);
    let mut out = String::new();
    for p in reg["plugins"].as_array().unwrap() {
        for s in p["skills"].as_array().unwrap() {
            out.push_str(&format!("- {}: {}\n", s["name"].as_str().unwrap_or("?"), s["description"].as_str().unwrap_or("").chars().take(160).collect::<String>()));
        }
    }
    out
}

/// The body of a skill, at most 12,000 characters.
pub fn skill_body(ctx: &Ctx, name: &str) -> String {
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

/// MCP servers of all plugins, for the configuration of the main model.
pub fn servers(ctx: &Ctx) -> Value {
    let reg = load(ctx);
    let mut out = json!({});
    for p in reg["plugins"].as_array().unwrap() {
        for (k, s) in p["mcpServers"].as_object().unwrap() {
            out[k] = s.clone();
        }
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
}
