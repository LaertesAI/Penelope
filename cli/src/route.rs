//! Tool routing: which role may call which tool, which model answers for a role, and one gate that every call passes.
//!
//! The interface between a model and a tool is checked here and nowhere else. A call is refused before the tool runs
//! when the role may not use it or when an argument does not match the schema in `spec.rs`. A refusal starts with
//! `NOT called.` and names the exact field, so a model can repair its call in the next round.

use crate::config::Ctx;
use crate::spec;
use serde_json::Value;

/// The roles of Penelope. Each role is a place where one model family will work later.
pub const ROLES: &[(&str, &str)] = &[
    ("lead", "talks to the person, plans, decides"),
    ("scout", "looks things up, changes nothing"),
    ("worker", "makes one change in the worktree of a task"),
    ("judge", "says whether a claim holds, with one reason"),
    ("tool", "drives the computer through ScreenCodec and MCP; it changes nothing else"),
    ("condenser", "shortens long tool output"),
    ("filter", "masks secrets and personal data before another model reads them"),
    ("router", "picks the role and the model for a step"),
];

pub fn role_names() -> Vec<&'static str> {
    ROLES.iter().map(|(r, _)| *r).collect()
}

thread_local! {
    /// The role of the tool loop on this thread. Set by `with_role` around a scout or an agent step. The MCP server
    /// of a scout, a worker or a judge carries its role in `PENELOPE_ROLE`.
    static ACTIVE: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// Runs a block of tool calls under one role on this thread. The env var stays the fallback for the MCP server process.
pub fn with_role<T>(role: &str, f: impl FnOnce() -> T) -> T {
    let before = ACTIVE.with(|r| r.replace(Some(role.to_string())));
    let out = f();
    ACTIVE.with(|r| *r.borrow_mut() = before);
    out
}

pub fn current_role() -> String {
    if let Some(role) = ACTIVE.with(|r| r.borrow().clone()) {
        return role;
    }
    if crate::scout::inside() {
        return "scout".to_string();
    }
    let role = std::env::var("PENELOPE_ROLE").unwrap_or_default();
    if role_names().contains(&role.as_str()) {
        return role;
    }
    if std::env::var("PENELOPE_SCOUT").is_ok() {
        return "scout".to_string();
    }
    "lead".to_string()
}

/// The tools one role may call. A role without tools, such as router or filter, gets an empty list.
pub fn allowed_for(role: &str) -> Vec<&'static str> {
    let all = spec::names();
    let only: &[&str] = match role {
        "lead" => return all,
        // A scout changes nothing outside its own reading: no screen action that clicks or types, no HTTP method
        // beyond GET, no MCP call.
        "scout" => &["web_search", "research", "fetch", "read", "find", "grep", "tree", "outline", "run", "run_detail", "memory_go", "memory_find", "session_search", "json_query"],
        "worker" => &["read", "write", "edit", "unzip", "undo", "pr_create", "run", "run_detail", "job_start", "job_read", "job_stop", "find", "grep", "tree", "outline", "lsp", "memory_go", "memory_find",
                      "json_query", "git_diff", "mcp_servers", "mcp_call", "cluster_spawn", "cluster_ask"],
        "judge" => &["read", "grep", "tree", "outline", "lsp", "run", "run_detail", "json_query", "git_diff", "cluster_ask"],
        "tool" => &["screen", "cursor", "mcp_servers", "mcp_call", "brief_check", "brief_put", "read", "outline", "tree"],
        _ => &[],
    };
    only.to_vec()
}

pub fn may_call(role: &str, tool: &str) -> bool {
    allowed_for(role).contains(&tool)
}

/// The model of a role. `None` means the role runs on fixed rules, as router and filter do until a model is set.
pub fn model_for<'a>(ctx: &'a Ctx, role: &str) -> Option<&'a Value> {
    let own = &ctx.cfg["roles"][role];
    if own["model"].is_string() {
        return Some(own);
    }
    match role {
        "lead" => Some(&ctx.cfg["main"]),
        "scout" => Some(crate::scout::model(ctx)),
        "condenser" => Some(&ctx.cfg["weak"]),
        "judge" | "worker" => Some(&ctx.cfg["weak"]),
        "tool" => Some(if ctx.cfg["roles"]["tool"]["model"].is_string() { &ctx.cfg["roles"]["tool"] } else { &ctx.cfg["weak"] }),
        _ => None,
    }
}

/// The name of the model a role runs on, for the side panel and the cost report. "rules" when no model is set.
pub fn model_name(ctx: &Ctx, role: &str) -> String {
    match model_for(ctx, role) {
        Some(m) => {
            let (shown, family) = crate::crew::describe(m);
            match family {
                Some(f) => format!("{} ({} 1, in training)", shown, f),
                None => format!("{} ({})", shown, m["kind"].as_str().unwrap_or("claude-cli")),
            }
        }
        None => "rules".to_string(),
    }
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(n) if n.is_i64() || n.is_u64() => "integer",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn fits(kind: &str, v: &Value) -> bool {
    match kind {
        "string" => v.is_string(),
        "integer" => v.as_i64().is_some() || v.as_u64().is_some(),
        "number" => v.is_number(),
        "boolean" => v.is_boolean(),
        "array" => v.is_array(),
        "object" => v.is_object(),
        _ => true,
    }
}

/// Checks one value against one schema. `path` is the argument name shown in the refusal.
fn check(schema: &Value, value: &Value, path: &str) -> Result<(), String> {
    if let Some(list) = schema["enum"].as_array() {
        if !list.iter().any(|v| v == value) {
            let options = list.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join(", ");
            return Err(format!("argument {} must be one of: {}. Got {}", path, options, value));
        }
    }
    let kind = schema["type"].as_str().unwrap_or("");
    if !kind.is_empty() && !fits(kind, value) {
        return Err(format!("argument {} must be {}, got {}", path, kind, type_name(value)));
    }
    match kind {
        "array" => {
            if let Some(items) = schema.get("items") {
                for (i, item) in value.as_array().cloned().unwrap_or_default().iter().enumerate() {
                    check(items, item, &format!("{}[{}]", path, i))?;
                }
            }
        }
        "object" => {
            if let Some(props) = schema["properties"].as_object() {
                if let Some(obj) = value.as_object() {
                    for key in schema["required"].as_array().cloned().unwrap_or_default().iter().filter_map(|v| v.as_str()) {
                        match obj.get(key) {
                            None | Some(Value::Null) => return Err(format!("argument {}.{} is required", path, key)),
                            Some(Value::String(s)) if s.is_empty() => return Err(format!("argument {}.{} is required and empty", path, key)),
                            _ => {}
                        }
                    }
                    for (key, sub) in props {
                        if let Some(v) = obj.get(key).filter(|v| !v.is_null()) {
                            check(sub, v, &format!("{}.{}", path, key))?;
                        }
                    }
                }
            }
        }
        _ => {}
    }
    Ok(())
}

/// Checks the arguments of a call against the schema in `spec.rs` or against a tool Penelope wrote. Returns the text a
/// model reads when the call may not run.
pub fn validate(ctx: &Ctx, name: &str, args: &Value) -> Result<(), String> {
    let schema = match spec::schema(name) {
        Some(s) => s,
        None => match crate::forge::schema(ctx, name) {
            Some(s) => s,
            None => return Err(format!("NOT called. unknown tool {}. penelope tools lists every name.", name)),
        },
    };
    let args = if args.is_null() { &serde_json::json!({}) } else { args };
    if !args.is_object() {
        return Err(format!("NOT called. tool {}: the arguments must be a JSON object, got {}", name, type_name(args)));
    }
    check(&schema, args, "argument").map_err(|e| {
        // `argument argument.command` reads badly; flatten the first label.
        let e = e.replacen("argument argument.", "argument ", 1);
        format!("NOT called. tool {}: {}", name, e)
    })
}

/// Runs a tool call under the current role. Every path into a tool goes through here: the MCP server of the lead model,
/// the tool loop of a scout or worker, a forged tool and the command line.
pub fn dispatch(ctx: &Ctx, name: &str, args: &Value) -> Result<String, String> {
    dispatch_as(ctx, &current_role(), name, args)
}

pub fn dispatch_as(ctx: &Ctx, role: &str, name: &str, args: &Value) -> Result<String, String> {
    // A tool Penelope wrote for this project belongs to the lead and the worker; every other role stays on the fixed list.
    let forged = matches!(role, "lead" | "worker") && crate::forge::schema(ctx, name).is_some();
    if !may_call(role, name) && !forged {
        let allowed = allowed_for(role);
        return Err(format!("NOT called. A {} has no tool {}. Its tools: {}.", role, name,
                           if allowed.is_empty() { "none".to_string() } else { allowed.join(", ") }));
    }
    validate(ctx, name, args)?;
    crate::mcp::call_raw(ctx, name, args)
}

/// Every tool with the roles that may call it, and the arguments of its schema.
pub fn table(ctx: &Ctx) -> String {
    let mut out = format!("{:<14} {:<34} {}\n", "tool", "roles", "arguments");
    for t in crate::spec::TOOLS {
        let roles: Vec<&str> = ROLES.iter().filter(|(r, _)| may_call(r, t.name)).map(|(r, _)| *r).collect();
        let args = crate::spec::schema(t.name).and_then(|s| s["properties"].as_object().map(|p| p.keys().cloned().collect::<Vec<_>>())).unwrap_or_default();
        out.push_str(&format!("{:<14} {:<34} {}\n", t.name, roles.join(", "), args.join(", ")));
    }
    for tool in crate::forge::specs(ctx) {
        let args = tool["inputSchema"]["properties"].as_object().map(|p| p.keys().cloned().collect::<Vec<_>>()).unwrap_or_default();
        out.push_str(&format!("{:<14} {:<34} {} (forged)\n", tool["name"].as_str().unwrap_or("?"), "lead, worker", args.join(", ")));
    }
    out.push_str(&format!("\n{} built-in tools. Every call passes route::dispatch: role first, then the schema.\n", crate::spec::TOOLS.len()));
    out
}

/// The roles, their model and their tool count, for `penelope route` and `/route`.
pub fn report(ctx: &Ctx) -> String {
    let mut out = format!("{:<10} {:<46} {}\n", "role", "model", "tools");
    for (role, what) in ROLES {
        out.push_str(&format!("{:<10} {:<46} {}\n", role, model_name(ctx, role), allowed_for(role).len()));
        out.push_str(&format!("{:<10} {}\n", "", what));
    }
    out.push_str("\npenelope provider use NAME --role router|filter|worker|judge puts a model into a role.\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_call_outside_the_role_never_reaches_the_tool() {
        let root = std::env::temp_dir().join(format!("penelope-route-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let ctx = crate::config::load(&root);
        let said = dispatch_as(&ctx, "scout", "write", &json!({"path": "x", "text": "y"})).unwrap_err();
        assert!(said.starts_with("NOT called. A scout has no tool write") && said.contains("read"), "{}", said);
        assert!(dispatch_as(&ctx, "router", "run", &json!({"command": "ls"})).unwrap_err().contains("Its tools: none"));
        assert!(may_call("lead", "tool_new") && may_call("worker", "edit") && may_call("worker", "unzip") && !may_call("judge", "write") && !may_call("scout", "unzip"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_bad_argument_names_the_field_and_the_type() {
        let root = std::env::temp_dir().join(format!("penelope-validate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let ctx = crate::config::load(&root);
        let err = validate(&ctx, "read", &json!({"path": 7})).unwrap_err();
        assert_eq!(err, "NOT called. tool read: argument path must be string, got integer");
        assert!(validate(&ctx, "read", &json!({})).unwrap_err().contains("argument path is required"));
        assert!(validate(&ctx, "memory_note", &json!({"text": "x", "scope": "everywhere"})).unwrap_err().contains("must be one of: project, global, auto"));
        assert!(validate(&ctx, "plan", &json!({"items": [{"text": "a", "state": "later"}]})).unwrap_err().contains("items[0].state must be one of"));
        assert!(validate(&ctx, "write", &json!({"path": "x"})) .unwrap_err().contains("text is required"));
        assert!(validate(&ctx, "read", &json!(["x"])).unwrap_err().contains("must be a JSON object"));
        assert!(validate(&ctx, "no_such_tool", &json!({})).unwrap_err().contains("unknown tool"));
        assert!(validate(&ctx, "read", &json!({"path": "x", "limit": "many"})).unwrap_err().contains("limit must be integer"));
        assert!(validate(&ctx, "read", &json!({"path": "x"})).is_ok());
    }

    #[test]
    fn the_roles_and_their_tools_are_well_formed() {
        assert_eq!(role_names(), vec!["lead", "scout", "worker", "judge", "tool", "condenser", "filter", "router"]);
        assert_eq!(allowed_for("lead").len(), spec::names().len());
        assert!(allowed_for("scout").iter().all(|t| spec::get(t).is_some()));
        assert_eq!(allowed_for("filter"), Vec::<&str>::new());
        for (role, _) in ROLES {
            assert!(!allowed_for(role).contains(&"__none__"), "{}", role);
        }
    }
}
