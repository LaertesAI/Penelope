//! The doctor: one line per program or setting Penelope depends on.
//!
//! The command line prints this text, the desktop shows the same lines in its settings screen. No key is read into
//! the text, only the name of the variable and whether it is set.

use crate::config::{self, Ctx};
use crate::{cursor, llm, memory, providers, route, search, spec, tools};

/// One line per program or setting Penelope depends on.
pub fn text(ctx: &Ctx) -> String {
    let mut out = String::new();
    let mut line = |name: &str, ok: bool, detail: String| {
        out.push_str(&format!(
            "{} {}{}\n",
            if ok { "ok   " } else { "FAIL " },
            name,
            if detail.is_empty() {
                String::new()
            } else {
                format!(": {}", detail)
            }
        ));
    };
    line("folder", true, ctx.root.display().to_string());
    let claude = config::which("claude");
    let needs_claude = ["main", "weak"]
        .iter()
        .any(|r| !llm::is_api(llm::kind_of(&ctx.cfg[*r])));
    line(
        "claude command (login of the user)",
        claude.is_some() || !needs_claude,
        claude.map(|p| p.display().to_string()).unwrap_or_else(|| {
            if needs_claude {
                "not on the PATH".into()
            } else {
                "not on the PATH, no model setting needs it".into()
            }
        }),
    );
    for role in ["main", "weak", "scout"] {
        let m = &ctx.cfg[role];
        if !m["model"].is_string() {
            continue;
        }
        // The line names the variable and says whether it is set. The key itself is never read into this text.
        let (table, env) = (providers::of(m), m["api_key_env"].as_str().unwrap_or(""));
        let local = ["http://localhost", "http://127.0.0.1", "http://[::1]"]
            .iter()
            .any(|h| m["base_url"].as_str().unwrap_or("").starts_with(h));
        let optional = table.map(|p| p.key_optional).unwrap_or(env.is_empty()) || local;
        let state = table
            .map(providers::state_of)
            .unwrap_or_else(|| providers::key_state(env, optional));
        line(
            &format!("{} model", role),
            state != "no key",
            format!(
                "{} on {}, {}{}",
                m["model"].as_str().unwrap_or("?"),
                table
                    .map(|p| p.name)
                    .unwrap_or("an address outside the table"),
                state,
                if env.is_empty() {
                    String::new()
                } else {
                    format!(" in {}", env)
                }
            ),
        );
    }
    let sc = ctx.cfg["screencodec"]["command"]
        .as_str()
        .filter(|c| !c.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| config::which("screencodec-mcp"));
    line(
        "screencodec-mcp (eyes: pages and pictures as text)",
        sc.is_some(),
        sc.map(|p| p.display().to_string())
            .unwrap_or_else(|| "not on the PATH, Penelope runs without it".into()),
    );
    line(
        "run policy",
        true,
        ctx.cfg["run_policy"].as_str().unwrap_or("safe").to_string(),
    );
    line(
        "edit policy (file changes)",
        true,
        ctx.cfg["edit_policy"].as_str().unwrap_or("ask").to_string(),
    );
    line(
        "delegate (who searches)",
        true,
        format!(
            "{}: {}",
            ctx.cfg["delegate"].as_str().unwrap_or("auto"),
            match ctx.cfg["delegate"].as_str().unwrap_or("auto") {
                "off" => "the main model searches itself",
                "offer" => "the main model may search itself or send a scout",
                "web" => "web research runs on the scout, the main model searches files itself",
                _ => "every search runs on the weaker model as scout",
            }
        ),
    );
    line("web search", true, search::kind(ctx).to_string());
    line("search API", true, search::api_status(ctx));
    let reader = ctx.cfg["reader"]["url"].as_str().filter(|u| !u.is_empty());
    line(
        "reader API",
        true,
        reader
            .map(|u| u.to_string())
            .unwrap_or_else(|| "not set; static HTML and local PDF only".to_string()),
    );
    line(
        "pdftotext (local PDF)",
        config::which("pdftotext").is_some(),
        "a scanned PDF still needs OCR or the reader API".to_string(),
    );
    line(
        "memory map",
        true,
        format!("{} lines", tools::memory_map(ctx).lines().count()),
    );
    let shared = memory::open_global().index(200);
    line(
        "shared memory",
        true,
        format!(
            "{} waypoints (rules for every project)",
            if shared.is_empty() {
                0
            } else {
                shared.lines().count()
            }
        ),
    );
    line(
        "tool contracts",
        true,
        format!(
            "{} built-in tools, every call checked by route::dispatch",
            spec::TOOLS.len()
        ),
    );
    let (cursor, ok) = cursor::backend(ctx);
    line("cursor (Penelope's own pointer)", ok, cursor);
    let desktop = ctx.cfg["screencodec"]["desktop"].as_str().unwrap_or("off");
    line(
        "ScreenCodec desktop",
        true,
        format!(
            "{} (SDL_MCP_DESKTOP; ask needs your confirmation, on captures at once)",
            desktop
        ),
    );
    for (role, _) in route::ROLES {
        if matches!(*role, "router" | "filter") {
            line(
                &format!("role {}", role),
                true,
                route::model_name(ctx, role),
            );
        }
    }
    out
}
