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

/// One model: through the claude command, or an endpoint in the format of OpenAI (API key or local model).
fn model(input: &mut dyn BufRead, out: &mut dyn Write, title: &str, cfg: &Value, names: &[(&str, &str)]) -> Value {
    let kind = choose(input, out, &format!("{}: where does it run?", title),
                      &[("claude-cli", "the claude command with your own login"), ("openai", "an endpoint in the format of OpenAI: API key, Ollama, llama.cpp")],
                      cfg["kind"].as_str().unwrap_or("claude-cli"));
    if kind == "openai" {
        return json!({"kind": "openai",
            "base_url": text(input, out, "address of the endpoint", cfg["base_url"].as_str().unwrap_or("http://localhost:11434/v1")),
            "model": text(input, out, "name of the model", if cfg["kind"] == "openai" { cfg["model"].as_str().unwrap_or("") } else { "" }),
            "api_key_env": text(input, out, "environment variable with the key, empty for none", cfg["api_key_env"].as_str().unwrap_or(""))});
    }
    let current = if cfg["kind"] == "openai" { names[0].0 } else { cfg["model"].as_str().unwrap_or(names[0].0) };
    json!({"kind": "claude-cli", "model": choose(input, out, &format!("{}: which model?", title), names, current)})
}

/// Walks through the questions and returns the new settings.
pub fn wizard(input: &mut dyn BufRead, out: &mut dyn Write, mut cfg: Value) -> Value {
    let _ = writeln!(out, "Penelope setup. Seven questions, enter keeps what is set.");
    let claude = crate::config::which("claude");
    let _ = writeln!(out, "  claude command: {}", claude.map(|p| p.display().to_string()).unwrap_or_else(|| "not found, choose openai below or install it".into()));
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
    cfg["search"] = match search.as_str() {
        "searxng" => json!({"kind": "searxng", "url": text(input, out, "address of the instance", cfg["search"]["url"].as_str().unwrap_or("http://localhost:8080"))}),
        "brave" => json!({"kind": "brave", "api_key_env": text(input, out, "environment variable with the key", cfg["search"]["api_key_env"].as_str().unwrap_or("BRAVE_API_KEY"))}),
        kind => json!({"kind": kind}),
    };
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
        // main: claude-cli, opus. weaker: openai with three texts. delegate offer, privacy pii, run kept, edit all, search searxng with an address.
        let answers = "1\n2\n2\nhttp://localhost:1234/v1\nqwen2.5:0.5b\n\n3\n2\n\n2\n4\nhttp://localhost:8888\n";
        let cfg = wizard(&mut answers.as_bytes(), &mut out, start.clone());
        assert_eq!(cfg["main"], json!({"kind": "claude-cli", "model": "opus"}));
        assert_eq!(cfg["weak"], json!({"kind": "openai", "base_url": "http://localhost:1234/v1", "model": "qwen2.5:0.5b", "api_key_env": ""}));
        assert_eq!((cfg["delegate"].as_str(), cfg["privacy"].as_str(), cfg["run_policy"].as_str(), cfg["edit_policy"].as_str()),
                   (Some("offer"), Some("pii"), Some("safe"), Some("all")));
        assert_eq!(cfg["search"], json!({"kind": "searxng", "url": "http://localhost:8888"}));
        let kept = wizard(&mut "".as_bytes(), &mut out, start.clone());
        assert_eq!(kept, start);
    }
}
