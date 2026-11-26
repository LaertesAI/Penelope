//! Tools that Penelope writes herself.
//!
//! A forged tool is one JSON file under `.penelope/tools/`: a name, a description, a JSON schema and a shell command
//! with `{argument}` placeholders. The command is rendered with shell-quoted arguments, so a value can never break out
//! of its place. A call runs through `tools::run`, which means the same safe list, the same question to the user, the
//! same privacy filter and the same condensing as every other command.
//!
//! The thoughts behind a tool live in the memory map; `tool_new` turns such a thought into a callable contract.

use crate::config::Ctx;
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;

fn dir(ctx: &Ctx) -> PathBuf {
    ctx.root.join("tools")
}

fn valid_name(name: &str) -> bool {
    (2..=32).contains(&name.len()) && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
        && name.chars().next().map(|c| c.is_ascii_lowercase()).unwrap_or(false)
        && crate::spec::get(name).is_none()
}

/// Every argument name a command uses between braces.
pub fn placeholders(command: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = command.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            if let Some(end) = command[i + 1..].find('}') {
                let name = &command[i + 1..i + 1 + end];
                if !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !out.iter().any(|x| x == name) {
                    out.push(name.to_string());
                }
                i += end + 2;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// One value as one shell word. A single quote ends the quoted run, is escaped, and the run opens again.
pub fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r#"'\''"#))
}

fn scalar(schema: &Value) -> bool {
    matches!(schema["type"].as_str(), Some("string") | Some("integer") | Some("number") | Some("boolean"))
}

/// The contract of a new tool, checked before it is written. Returns the cleaned object or the reason it is refused.
pub fn contract(name: &str, description: &str, command: &str, schema: &Value) -> Result<Value, String> {
    if !valid_name(name) {
        return Err(format!("NOT written. {} is no free tool name: 2 to 32 lower case letters, digits, _ or -, and no built-in tool has it.", name));
    }
    let description = description.split_whitespace().collect::<Vec<_>>().join(" ");
    if description.len() < 10 || description.len() > 400 {
        return Err("NOT written. The description must be one line of 10 to 400 characters. A model reads it to choose the tool.".to_string());
    }
    if command.trim().is_empty() || command.len() > 2000 {
        return Err("NOT written. The command must be 1 to 2000 characters.".to_string());
    }
    if schema["type"].as_str() != Some("object") {
        return Err("NOT written. The schema must have \"type\": \"object\".".to_string());
    }
    let props = schema["properties"].as_object().cloned().unwrap_or_default();
    if props.len() > 12 {
        return Err("NOT written. At most 12 arguments.".to_string());
    }
    for (k, v) in &props {
        if !scalar(v) {
            return Err(format!("NOT written. Argument {} must be a string, integer, number or boolean, because it becomes one shell word.", k));
        }
    }
    for req in schema["required"].as_array().cloned().unwrap_or_default() {
        if !req.as_str().map(|r| props.contains_key(r)).unwrap_or(false) {
            return Err(format!("NOT written. Required argument {} is not declared.", req));
        }
    }
    let used = placeholders(command);
    for p in &used {
        if !props.contains_key(p) {
            return Err(format!("NOT written. The command uses {{{}}}, but the schema does not declare it.", p));
        }
    }
    if used.is_empty() {
        return Err("NOT written. The command uses no {argument}. A tool without arguments is a plain command: use run.".to_string());
    }
    Ok(json!({"name": name, "description": description, "schema": schema, "command": command.trim(),
              "origin": "penelope", "created": crate::sessions::stamp(now())}))
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Writes one tool file.
pub fn create(ctx: &Ctx, name: &str, description: &str, command: &str, schema: &Value) -> Result<String, String> {
    let value = contract(name, description, command, schema)?;
    let path = dir(ctx).join(format!("{}.json", name));
    if path.exists() {
        return Err(format!("NOT written. A tool {} already exists (tool_list shows it). Choose another name.", name));
    }
    if let Err(e) = fs::create_dir_all(dir(ctx)) {
        return Err(format!("NOT written. cannot create {}: {}", dir(ctx).display(), e));
    }
    fs::write(&path, serde_json::to_string_pretty(&value).unwrap()).map_err(|e| format!("NOT written. {}", e))?;
    Ok(format!("tool {} written: {}. Arguments: {}. A call asks like run and passes the privacy filter.", name,
               crate::files::show(&path), placeholders(command).join(", ")))
}

/// Every readable forged tool. A broken file is skipped and named in the second list.
fn load(ctx: &Ctx) -> (Vec<Value>, Vec<String>) {
    let (mut out, mut bad) = (Vec::new(), Vec::new());
    let Ok(entries) = fs::read_dir(dir(ctx)) else { return (out, bad) };
    for e in entries.flatten() {
        let path = e.path();
        if path.extension().and_then(|x| x.to_str()) != Some("json") {
            continue;
        }
        match fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok()) {
            Some(v) if v["name"].is_string() && v["command"].is_string() && v["schema"].is_object() => out.push(v),
            _ => bad.push(e.file_name().to_string_lossy().to_string()),
        }
    }
    out.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    (out, bad)
}

/// The schemas of the forged tools, for the gate in `route`.
pub fn schema(ctx: &Ctx, name: &str) -> Option<Value> {
    load(ctx).0.into_iter().find(|t| t["name"].as_str() == Some(name)).map(|t| t["schema"].clone())
}

/// The tool descriptions of a folder, for the list a model sees.
pub fn specs(ctx: &Ctx) -> Vec<Value> {
    load(ctx).0.into_iter().map(|t| json!({"name": t["name"], "description": t["description"], "inputSchema": t["schema"]})).collect()
}

fn stats_line(ctx: &Ctx, name: &str) -> String {
    let v: Value = fs::read_to_string(ctx.root.join("stats.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    let calls = v[name]["calls"].as_u64().unwrap_or(0);
    if calls == 0 { "not called yet".to_string() } else { format!("{} calls", calls) }
}

pub fn list(ctx: &Ctx) -> String {
    let (tools, bad) = load(ctx);
    let mut out = String::new();
    for t in &tools {
        let args = t["schema"]["properties"].as_object().map(|o| o.keys().cloned().collect::<Vec<_>>().join(", ")).unwrap_or_default();
        out.push_str(&format!("{} [{}] — {} · {}\n  command: {}\n", t["name"].as_str().unwrap_or("?"), args,
                              t["description"].as_str().unwrap_or(""), stats_line(ctx, t["name"].as_str().unwrap_or("")),
                              t["command"].as_str().unwrap_or("")));
    }
    if out.is_empty() {
        out.push_str("no forged tools yet. The main model writes one with tool_new, or penelope tool new.\n");
    }
    if !bad.is_empty() {
        out.push_str(&format!("broken files, skipped: {}\n", bad.join(", ")));
    }
    out
}

/// Runs one forged tool: arguments are checked by `route`, the command is rendered, then `tools::run` applies the
/// policy, the privacy filter and the condensing.
pub fn call(ctx: &Ctx, name: &str, args: &Value) -> Result<String, String> {
    let (tools, _) = load(ctx);
    let Some(t) = tools.into_iter().find(|t| t["name"].as_str() == Some(name)) else {
        return Err(format!("NOT called. unknown tool {}", name));
    };
    let schema = &t["schema"];
    let mut command = t["command"].as_str().unwrap_or("").to_string();
    for key in placeholders(&command) {
        let value = args.get(&key).cloned().unwrap_or(Value::Null);
        let text = match &value {
            Value::String(s) => s.clone(),
            Value::Number(n) => n.to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Null => return Err(format!("NOT called. tool {}: argument {} is required by the command", name, key)),
            _ => return Err(format!("NOT called. tool {}: argument {} must be one shell word", name, key)),
        };
        if value.is_null() || (value.is_string() && text.is_empty()) {
            return Err(format!("NOT called. tool {}: argument {} is required by the command", name, key));
        }
        command = command.replace(&format!("{{{}}}", key), &quote(&text));
    }
    for req in schema["required"].as_array().cloned().unwrap_or_default() {
        let key = req.as_str().unwrap_or("");
        if args.get(key).is_none() {
            return Err(format!("NOT called. tool {}: argument {} is required", name, key));
        }
    }
    let cwd = args["cwd"].as_str().unwrap_or("");
    let said = crate::tools::run(ctx, &command, &format!("forged tool {}: {}", name, t["description"].as_str().unwrap_or("")), cwd);
    // The counter of the forged tool lives next to the counters of the built-in tools.
    crate::tools::count(ctx, name, 0, 0);
    Ok(said)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn ctx(name: &str, policy: &str) -> (Ctx, PathBuf) {
        let dir = std::env::temp_dir().join(format!("penelope-forge-{}-{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let mut ctx = crate::config::load(&dir.join(".penelope"));
        ctx.cfg["run_policy"] = json!(policy);
        (ctx, dir)
    }

    #[test]
    fn a_forged_tool_quotes_every_argument_and_runs_through_the_policy() {
        let (ctx, dir) = ctx("call", "all");
        let schema = json!({"type": "object", "properties": {"text": {"type": "string"}}, "required": ["text"]});
        let made = create(&ctx, "say_it", "Print one argument through the shell", "printf '%s' {text}", &schema).unwrap();
        assert!(made.contains("tool say_it written") && list(&ctx).contains("say_it [text]"), "{}", made);
        let out = call(&ctx, "say_it", &json!({"text": "a; touch pwned"})).unwrap();
        assert!(out.contains("a; touch pwned") && !dir.join("pwned").exists(), "{}", out);
        assert!(call(&ctx, "say_it", &json!({})).unwrap_err().contains("text is required"));
        assert!(create(&ctx, "say_it", "Print one argument through the shell", "printf '%s' {text}", &schema).is_err());
        // The gate of route knows the forged tool for the lead and the worker, not for a scout.
        assert!(crate::route::dispatch_as(&ctx, "lead", "say_it", &json!({"text": "hello"})).unwrap().contains("hello"));
        assert!(crate::route::dispatch_as(&ctx, "worker", "say_it", &json!({"text": "hello"})).unwrap().contains("hello"));
        assert!(crate::route::dispatch_as(&ctx, "scout", "say_it", &json!({"text": "hello"})).unwrap_err().contains("A scout has no tool say_it"));
        assert!(crate::route::validate(&ctx, "say_it", &json!({})).unwrap_err().contains("text is required"));
        let offered = crate::mcp::tool_specs_for(&ctx, "lead");
        assert!(offered.as_array().unwrap().iter().any(|t| t["name"] == "say_it"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_contract_refuses_a_broken_tool_before_it_exists() {
        let (_ctx, _dir) = ctx("contract", "all");
        let s = |t: &str| json!({"type": "object", "properties": {"q": {"type": t}}, "required": ["q"]});
        assert!(contract("run", "A tool that shadows a built-in", "echo {q}", &s("string")).unwrap_err().contains("no free tool name"));
        assert!(contract("MyTool", "A tool with an upper case name", "echo {q}", &s("string")).unwrap_err().contains("no free tool name"));
        assert!(contract("t1", "short", "echo {q}", &s("string")).unwrap_err().contains("description"));
        assert!(contract("t1", "A tool with a nested argument", "echo {q}", &s("object")).unwrap_err().contains("one shell word"));
        assert!(contract("t1", "A tool with an undeclared argument", "echo {other}", &s("string")).unwrap_err().contains("does not declare"));
        assert!(contract("t1", "A tool without an argument", "echo hello", &s("string")).unwrap_err().contains("no {argument}"));
        assert!(contract("t1", "A tool with one good argument", "echo {q}", &s("string")).is_ok());
    }
}
