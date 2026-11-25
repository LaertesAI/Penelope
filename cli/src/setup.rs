//! The setup: a few questions on the plain terminal, each with numbered answers. Enter keeps the current value.

use serde_json::{json, Value};
use std::io::{BufRead, Write};

/// Asks one question. Returns the value of the chosen answer. An empty line, an unknown answer or the end of the input keeps `current`.
fn choose(input: &mut dyn BufRead, out: &mut dyn Write, question: &str, options: &[(&str, &str)], current: &str) -> String {
    let _ = writeln!(out, "\n{}", question);
    for (i, (value, help)) in options.iter().enumerate() {
        let _ = writeln!(out, "  {} {:<12} {}{}", i + 1, value, help, if *value == current { "   (now)" } else { "" });
    }
    let _ = write!(out, "  number, or enter to keep {}: ", current);
    let _ = out.flush();
    let mut line = String::new();
    if input.read_line(&mut line).unwrap_or(0) == 0 {
        return current.to_string();
    }
    match line.trim().parse::<usize>() {
        Ok(n) if n >= 1 && n <= options.len() => options[n - 1].0.to_string(),
        _ => options.iter().find(|(v, _)| *v == line.trim()).map(|(v, _)| v.to_string()).unwrap_or_else(|| current.to_string()),
    }
}

fn text(input: &mut dyn BufRead, out: &mut dyn Write, question: &str, current: &str) -> String {
    let _ = write!(out, "  {} (enter keeps \"{}\"): ", question, current);
    let _ = out.flush();
    let mut line = String::new();
    let _ = input.read_line(&mut line);
    if line.trim().is_empty() { current.to_string() } else { line.trim().to_string() }
}

/// One model: a provider of the table, or another address in the format of OpenAI. The setting gets the name of the
/// environment variable with the key. A key typed here is refused and is not stored.
fn model(input: &mut dyn BufRead, out: &mut dyn Write, title: &str, cfg: &Value, names: &[(&str, &str)]) -> Value {
    use crate::providers::{self, TABLE};
    let now = providers::of(cfg).map(|p| p.name).unwrap_or("custom");
    let help: Vec<String> = TABLE.iter().map(|p| format!("{} ({}, {})", p.label, p.kind, providers::state_of(p))).collect();
    let mut options: Vec<(&str, &str)> = TABLE.iter().zip(help.iter()).map(|(p, h)| (p.name, h.as_str())).collect();
    options.push(("custom", "another address in the format of OpenAI"));
    let chosen = choose(input, out, &format!("{}: where does it run?", title), &options, now);
    if chosen == "claude-cli" {
        let current = if now == "claude-cli" { cfg["model"].as_str().unwrap_or(names[0].0) } else { names[0].0 };
        return json!({"kind": "claude-cli", "model": choose(input, out, &format!("{}: which model?", title), names, current)});
    }
    let Some(p) = providers::get(&chosen) else {
        let was_custom = now == "custom";
        let address = text(input, out, "address of the endpoint", if was_custom { cfg["base_url"].as_str().unwrap_or("http://localhost:11434/v1") } else { "http://localhost:11434/v1" });
        let name = text(input, out, "name of the model", if was_custom { cfg["model"].as_str().unwrap_or("") } else { "" });
        let mut env = text(input, out, "name of the environment variable with the key, empty for none", if was_custom { cfg["api_key_env"].as_str().unwrap_or("") } else { "" });
        if !env.is_empty() && !providers::is_env_name(&env) {
            let _ = writeln!(out, "  That is no name of an environment variable, so nothing was stored. Put the key into a variable and give its name.");
            env.clear();
        }
        return json!({"kind": "openai", "base_url": address, "model": name, "api_key_env": env});
    };
    let same = now == p.name;
    let name = text(input, out, "name of the model", if same { cfg["model"].as_str().unwrap_or(p.model) } else { p.model });
    let _ = writeln!(out, "  key: {}{}", providers::state_of(p), if p.key_env.is_empty() { String::new() } else { format!(" in {}", p.key_env) });
    if same {          // the same provider keeps its other fields, such as max_tokens
        let mut kept = cfg.clone();
        kept["model"] = json!(name);
        return kept;
    }
    let workspace = if p.base.contains(providers::WORKSPACE) { text(input, out, "workspace ID of your account, as in the column API Host of the console", "") } else { String::new() };
    match providers::setting(p, Some(&name), Some(&workspace), None) {
        Ok(v) => v,
        Err(e) => {
            let _ = writeln!(out, "  {} The setting stays as it was.", e);
            cfg.clone()
        }
    }
}

/// Walks through the questions and returns the new settings.
pub fn wizard(input: &mut dyn BufRead, out: &mut dyn Write, mut cfg: Value) -> Value {
    let _ = writeln!(out, "Penelope setup. Seven questions, enter keeps what is set.");
    let claude = crate::config::which("claude");
    let _ = writeln!(out, "  claude command: {}", claude.map(|p| p.display().to_string()).unwrap_or_else(|| "not found, choose another provider below or install it".into()));
    cfg["main"] = model(input, out, "Main model (answers, writes code)", &cfg["main"].clone(),
                        &[("sonnet", "balanced"), ("opus", "strongest, most expensive"), ("haiku", "fastest, cheapest")]);
    cfg["weak"] = model(input, out, "Weaker model (condenses output, runs searches as scout)", &cfg["weak"].clone(),
                        &[("haiku", "fastest, cheapest"), ("sonnet", "balanced")]);
    let cur = |cfg: &Value, key: &str, default: &str| cfg[key].as_str().unwrap_or(default).to_string();
    cfg["delegate"] = json!(choose(input, out, "Who searches (web, files, the project)?",
        &[("auto", "always the weaker model as scout, the main model has no search tools"),
          ("web", "web research goes to the scout, the main model searches files itself"),
          ("offer", "the main model may search itself or send a scout"), ("off", "the main model searches itself")], &cur(&cfg, "delegate", "auto")));
    cfg["privacy"] = json!(choose(input, out, "What does the privacy filter mask before a model reads a text?",
        &[("rules", "keys, passwords, tokens, private key blocks"), ("pii", "the same plus e-mail addresses and IBANs"), ("off", "nothing")], &cur(&cfg, "privacy", "rules")));
    cfg["run_policy"] = json!(choose(input, out, "Which commands run without a question?",
        &[("safe", "read-only and build commands, every other command asks"), ("all", "every command")], &cur(&cfg, "run_policy", "safe")));
    cfg["edit_policy"] = json!(choose(input, out, "May the main model change files without a question?",
        &[("ask", "every change asks, one answer can allow all in the session"), ("all", "every change happens at once")], &cur(&cfg, "edit_policy", "ask")));
    let search = choose(input, out, "Which source answers a web search?",
        &[("auto", "the search tool of the claude command if the weaker model runs there, else DuckDuckGo"), ("claude", "the search tool of the claude command"),
          ("duckduckgo", "no key, refuses after a few queries"), ("searxng", "your own SearXNG instance"), ("brave", "the Brave Search API with a key"), ("off", "no web search")],
        cfg["search"]["kind"].as_str().unwrap_or("auto"));
    let mut search_cfg = cfg["search"].clone();
    search_cfg["kind"] = json!(search);
    match search.as_str() {
        "searxng" => { search_cfg["url"] = json!(text(input, out, "address of the instance", cfg["search"]["url"].as_str().unwrap_or("http://localhost:8080"))); }
        "brave" => { search_cfg["api_key_env"] = json!(text(input, out, "environment variable with the key", cfg["search"]["api_key_env"].as_str().unwrap_or("BRAVE_API_KEY"))); }
        _ => {}
    }
    cfg["search"] = search_cfg;
    let _ = writeln!(out, "\nSettings:\n{}", serde_json::to_string_pretty(&cfg).unwrap());
    cfg
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_become_settings_and_enter_keeps_them() {
        let start = crate::config::default_config();
        let mut out = Vec::new();
        // main: claude-cli, opus. weaker: a custom address with three texts. delegate offer, privacy pii, run kept, edit all, search searxng with an address.
        let answers = "1\n2\ncustom\nhttp://localhost:1234/v1\nqwen2.5:0.5b\n\n3\n2\n\n2\n4\nhttp://localhost:8888\n";
        let cfg = wizard(&mut answers.as_bytes(), &mut out, start.clone());
        assert_eq!(cfg["main"], json!({"kind": "claude-cli", "model": "opus"}));
        assert_eq!(cfg["weak"], json!({"kind": "openai", "base_url": "http://localhost:1234/v1", "model": "qwen2.5:0.5b", "api_key_env": ""}));
        assert_eq!((cfg["delegate"].as_str(), cfg["privacy"].as_str(), cfg["run_policy"].as_str(), cfg["edit_policy"].as_str()),
                   (Some("offer"), Some("pii"), Some("safe"), Some("all")));
        assert_eq!((cfg["search"]["kind"].as_str(), cfg["search"]["url"].as_str()), (Some("searxng"), Some("http://localhost:8888")));
        // The table: groq by name with its example model, Alibaba with a model and a workspace. Enter keeps the other five answers.
        let cfg = wizard(&mut "groq\n\nalibaba-intl\nqwen-plus\nllm-abc\n".as_bytes(), &mut out, start.clone());
        assert_eq!(cfg["main"], json!({"kind": "openai", "base_url": "https://api.groq.com/openai/v1", "model": "llama-3.3-70b-versatile", "api_key_env": "GROQ_API_KEY"}));
        assert_eq!(cfg["weak"]["base_url"], "https://llm-abc.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1");
        assert_eq!((cfg["weak"]["model"].as_str(), cfg["weak"]["api_key_env"].as_str(), cfg["delegate"].as_str()), (Some("qwen-plus"), Some("DASHSCOPE_API_KEY"), Some("auto")));
        assert_eq!(wizard(&mut "".as_bytes(), &mut out, cfg.clone()), cfg);
        // A key typed in place of a variable name is refused: the settings and the printed text do not hold it.
        let mut shown = Vec::new();
        let cfg = wizard(&mut "custom\nhttp://localhost:9/v1\nm\nsk-typed-by-mistake-123456\n".as_bytes(), &mut shown, start.clone());
        assert_eq!(cfg["main"]["api_key_env"], "");
        assert!(!cfg.to_string().contains("sk-typed") && !String::from_utf8_lossy(&shown).contains("sk-typed"));
        let kept = wizard(&mut "".as_bytes(), &mut out, start.clone());
        assert_eq!(kept, start);
    }
}
