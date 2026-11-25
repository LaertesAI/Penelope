//! A small client for the MCP servers of plugins.
//!
//! The claude command starts the servers of a plugin itself. A model behind an HTTP endpoint does not see them, so this
//! module starts one server per call, speaks the handshake and returns the text of a tool. A call is allowed only when
//! the plugin that owns the server says `"allow": true`; the same rule as the allow list of the claude command.
//!
//! One process per call keeps the code small and cannot leave a server running after a crash. A server that needs
//! state between calls (a browser, a database session) must be reached through its own tool or the claude command.

use crate::config::Ctx;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc::channel;
use std::time::Duration;

#[derive(Debug)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
}

fn spawn(server: &Value) -> Result<std::process::Child, String> {
    let Some(command) = server["command"].as_str().filter(|c| !c.trim().is_empty()) else {
        return Err("this server has no command (only stdio servers are supported)".to_string());
    };
    let mut cmd = Command::new(command);
    if let Some(args) = server["args"].as_array() {
        cmd.args(args.iter().filter_map(|a| a.as_str()));
    }
    if let Some(env) = server["env"].as_object() {
        for (k, v) in env {
            if let Some(v) = v.as_str() {
                cmd.env(k, v);
            }
        }
    }
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().map_err(|e| format!("cannot start {}: {}", command, e))
}

fn rpc(mut child: std::process::Child, method: &str, params: Value, seconds: u64) -> Result<Value, String> {
    let mut input = child.stdin.take().ok_or("the server has no stdin")?;
    let stdout = child.stdout.take().ok_or("the server has no stdout")?;
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if let Ok(v) = serde_json::from_str::<Value>(line.trim()) {
                if v["id"].is_u64() {
                    let _ = tx.send(v);
                }
            }
        }
    });
    let write = |input: &mut std::process::ChildStdin, v: &Value| -> Result<(), String> {
        writeln!(input, "{}", v).map_err(|e| e.to_string())?;
        input.flush().map_err(|e| e.to_string())
    };
    write(&mut input, &json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "penelope", "version": env!("CARGO_PKG_VERSION")}}}))?;
    if rx.recv_timeout(Duration::from_secs(20)).is_err() {
        let _ = child.kill();
        let _ = child.wait();
        return Err("the server did not answer initialize".to_string());
    }
    write(&mut input, &json!({"jsonrpc": "2.0", "method": "notifications/initialized", "params": {}}))?;
    write(&mut input, &json!({"jsonrpc": "2.0", "id": 2, "method": method, "params": params}))?;
    let answer = loop {
        let v = match rx.recv_timeout(Duration::from_secs(seconds)) {
            Ok(v) => v,
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("the server did not answer {} within {} s", method, seconds));
            }
        };
        if v["id"] == 2 {
            break v;
        }
    };
    let _ = child.kill();
    let _ = child.wait();
    if let Some(e) = answer.get("error") {
        return Err(e["message"].as_str().unwrap_or(&e.to_string()).to_string());
    }
    Ok(answer["result"].clone())
}

/// The tools of one server. The plugin must be registered; `allow` is not needed to read the list.
pub fn probe(ctx: &Ctx, server: &str) -> Result<Vec<ToolInfo>, String> {
    if !crate::agents::mcp_allowed(server) {
        return Err(format!("NOT listed. This agent run may use the MCP servers: {}.", crate::agents::mcp_scope().unwrap_or_default().join(", ")));
    }
    let (entry, _) = crate::plugins::server_entry(ctx, server).ok_or_else(|| format!("no MCP server named {}", server))?;
    let child = spawn(&entry)?;
    let result = rpc(child, "tools/list", json!({}), 30)?;
    Ok(result["tools"].as_array().cloned().unwrap_or_default().into_iter().map(|t| ToolInfo {
        name: t["name"].as_str().unwrap_or("").to_string(),
        description: t["description"].as_str().unwrap_or("").to_string(),
    }).collect())
}

/// Calls one tool of one plugin server. A denied plugin changes nothing.
pub fn call(ctx: &Ctx, server: &str, tool: &str, args: &Value) -> Result<String, String> {
    if !crate::agents::mcp_allowed(server) {
        return Err(format!("NOT called. This agent run may use the MCP servers: {}. Ask the lead to pass {} in the mcp list of the agent.", 
                           crate::agents::mcp_scope().unwrap_or_default().join(", "), server));
    }
    let (entry, allow) = crate::plugins::server_entry(ctx, server).ok_or_else(|| format!("NOT called. no MCP server named {}", server))?;
    if !allow {
        return Err(format!("NOT called. The plugin that holds {} is denied. Allow it with /plugins allow NAME.", server));
    }
    if tool.is_empty() {
        return Err("NOT called. Name the tool of the server.".to_string());
    }
    let child = spawn(&entry)?;
    let result = rpc(child, "tools/call", json!({"name": tool, "arguments": args}), 120)?;
    let text = result["content"].as_array().map(|a| a.iter().filter_map(|c| c["text"].as_str()).collect::<Vec<_>>().join("\n")).unwrap_or_default();
    if result["isError"].as_bool().unwrap_or(false) {
        return Err(format!("NOT done. {}", text));
    }
    Ok(if text.trim().is_empty() { format!("{} answered without text", tool) } else { crate::tools::guard(ctx, &text) })
}

/// The list of plugin servers, with their tools when `with_tools` is true. At most 8 servers are probed, so one broken
/// server cannot hold the turn forever.
pub fn list(ctx: &Ctx, with_tools: bool) -> String {
    let servers = crate::plugins::servers(ctx);
    let Some(obj) = servers.as_object() else { return "no MCP server is registered. /mcp add NAME COMMAND adds one, or /plugins install LINK.".to_string() };
    if obj.is_empty() {
        return "no MCP server is registered. /mcp add NAME COMMAND adds one, or /plugins install LINK.".to_string();
    }
    let mut out = String::new();
    for (i, (name, _)) in obj.iter().enumerate() {
        if !crate::agents::mcp_allowed(name) {
            continue;
        }
        let allow = crate::plugins::server_entry(ctx, name).map(|(_, a)| a).unwrap_or(false);
        out.push_str(&format!("{} · tools {}\n", name, if allow { "allowed" } else { "denied (/mcp allow NAME or /plugins allow NAME)" }));
        if with_tools && i < 8 {
            match probe(ctx, name) {
                Ok(tools) => {
                    for t in tools {
                        out.push_str(&format!("  {} — {}\n", t.name, t.description.chars().take(110).collect::<String>()));
                    }
                }
                Err(e) => out.push_str(&format!("  could not list: {}\n", e)),
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn a_standalone_server_is_added_allowed_called_and_removed() {
        let dir = std::env::temp_dir().join(format!("penelope-mcp-standalone-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let script = dir.join("server");
        fs::write(&script, r#"#!/usr/bin/env python3
import sys, json
for line in sys.stdin:
    m = json.loads(line)
    if "id" not in m: continue
    if m["method"] == "initialize":
        r = {"protocolVersion": "2025-06-18", "capabilities": {"tools": {}}, "serverInfo": {"name": "echo"}}
    elif m["method"] == "tools/list":
        r = {"tools": [{"name": "echo", "description": "echo one text"}]}
    elif m["method"] == "tools/call":
        r = {"content": [{"type": "text", "text": "standalone:" + m["params"]["arguments"].get("text", "")}], "isError": False}
    else:
        r = {}
    print(json.dumps({"jsonrpc": "2.0", "id": m["id"], "result": r}), flush=True)
"#).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let ctx = crate::config::load(&dir.join(".penelope"));
        let said = crate::plugins::mcp_add(&ctx, "echo", &script.display().to_string(), &[], false);
        assert!(said.contains("echo added"), "{}", said);
        assert_eq!(probe(&ctx, "echo").unwrap()[0].name, "echo");
        assert!(call(&ctx, "echo", "echo", &json!({"text": "hi"})).unwrap_err().contains("denied"));
        assert!(crate::plugins::mcp_set_allow(&ctx, "echo", true).contains("allowed"));
        assert_eq!(call(&ctx, "echo", "echo", &json!({"text": "hi"})).unwrap(), "standalone:hi");
        assert!(crate::plugins::allowed(&ctx).contains(&"mcp__echo__*".to_string()));
        assert!(crate::plugins::mcp_remove(&ctx, "echo").contains("removed"));
        assert!(probe(&ctx, "echo").is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_plugin_server_is_probed_and_called_only_after_allow() {
        let dir = std::env::temp_dir().join(format!("penelope-mcpclient-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let script = dir.join("server");
        fs::write(&script, r#"#!/usr/bin/env python3
import sys, json
for line in sys.stdin:
    m = json.loads(line)
    if "id" not in m: continue
    if m["method"] == "initialize":
        r = {"protocolVersion": "2025-06-18", "capabilities": {"tools": {}}, "serverInfo": {"name": "echo"}}
    elif m["method"] == "tools/list":
        r = {"tools": [{"name": "echo", "description": "echo one text"}]}
    elif m["method"] == "tools/call":
        r = {"content": [{"type": "text", "text": "echo:" + m["params"]["arguments"].get("text", "")}], "isError": False}
    else:
        r = {}
    print(json.dumps({"jsonrpc": "2.0", "id": m["id"], "result": r}), flush=True)
"#).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let plugin = dir.join("demo");
        fs::create_dir_all(plugin.join(".claude-plugin")).unwrap();
        fs::write(plugin.join(".claude-plugin/plugin.json"), r#"{"name": "demo.mcp"}"#).unwrap();
        fs::write(plugin.join(".mcp.json"), json!({"mcpServers": {"echo": {"command": script.display().to_string()}}}).to_string()).unwrap();
        let ctx = crate::config::load(&dir.join(".penelope"));
        assert!(crate::plugins::add(&ctx, &plugin.to_string_lossy()).contains("demo.mcp"));
        let tools = probe(&ctx, "demo.mcp-echo").unwrap();
        assert_eq!((tools.len(), tools[0].name.as_str()), (1, "echo"));
        assert!(call(&ctx, "demo.mcp-echo", "echo", &json!({"text": "hi"})).unwrap_err().contains("denied"));
        crate::plugins::set_allow(&ctx, "demo.mcp", true);
        assert_eq!(call(&ctx, "demo.mcp-echo", "echo", &json!({"text": "hi"})).unwrap(), "echo:hi");
        assert!(list(&ctx, true).contains("echo — echo one text"));
        // A run that names other servers cannot reach this one.
        crate::agents::with_mcp_scope(&["another.server".to_string()], || {
            assert!(call(&ctx, "demo.mcp-echo", "echo", &json!({"text": "hi"})).unwrap_err().contains("may use the MCP servers: another.server"));
            assert!(probe(&ctx, "demo.mcp-echo").unwrap_err().contains("may use"));
            assert!(!list(&ctx, true).contains("echo one text"));
        });
        let _ = fs::remove_dir_all(&dir);
    }
}
