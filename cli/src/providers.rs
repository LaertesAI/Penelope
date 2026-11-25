//! The provider table: where a model runs, in which wire format, and which environment variable holds its key.
//! Every address and variable name comes from the provider's own documentation, read on 2026-11-24. The sources are
//! listed in `docs/PROVIDERS.md`. A key is read from the environment at the moment of a request and is stored nowhere.

use crate::config::Ctx;
use serde_json::{json, Value};

pub struct Provider {
    /// The name the commands take, for example `penelope provider use groq`.
    pub name: &'static str,
    pub label: &'static str,
    /// `openai` (chat completions), `anthropic` (Messages API) or `claude-cli` (the official claude command).
    pub kind: &'static str,
    /// Base address. The `openai` kind adds `/chat/completions`, the `anthropic` kind adds `/v1/messages`.
    pub base: &'static str,
    /// Name of the environment variable with the key. Empty when the provider takes no key.
    pub key_env: &'static str,
    /// True when requests work without a key, as with a model server on this PC.
    pub key_optional: bool,
    /// One model name from the provider's documentation, as an example.
    pub model: &'static str,
    pub note: &'static str,
}

/// The part of an Alibaba address that stands for the workspace of the account.
pub const WORKSPACE: &str = "{WorkspaceId}";

pub const TABLE: &[Provider] = &[
    Provider { name: "claude-cli", label: "Claude subscription", kind: "claude-cli", base: "", key_env: "", key_optional: true, model: "sonnet",
               note: "the official claude command with its own login, no key" },
    Provider { name: "anthropic", label: "Anthropic API", kind: "anthropic", base: "https://api.anthropic.com", key_env: "ANTHROPIC_API_KEY", key_optional: false,
               model: "claude-opus-5-5", note: "Messages API with tool use, claude-haiku-5-5 is the smaller model" },
    Provider { name: "openai", label: "OpenAI", kind: "openai", base: "https://api.openai.com/v1", key_env: "OPENAI_API_KEY", key_optional: false,
               model: "gpt-6.2-astra", note: "chat completions" },
    Provider { name: "laertes", label: "A Laertes model family, in training", kind: "laertes", base: "", key_env: "", key_optional: true,
               model: "Telemach 1", note: "Nostos, Tiresias, Utis, Aiolos, Argos, Klea, Telemach or Proteus; shown in gold, cannot answer yet" },
    Provider { name: "perplexity", label: "Perplexity", kind: "openai", base: "https://api.perplexity.ai", key_env: "PERPLEXITY_API_KEY", key_optional: false,
               model: "sonar-pro", note: "search answers, OpenAI-compatible chat completions" },
    Provider { name: "cohere", label: "Cohere", kind: "openai", base: "https://api.cohere.ai/compatibility/v1", key_env: "COHERE_API_KEY", key_optional: false,
               model: "command-r-plus", note: "the OpenAI compatibility endpoint of Cohere" },
    Provider { name: "deepinfra", label: "DeepInfra", kind: "openai", base: "https://api.deepinfra.com/v1/openai", key_env: "DEEPINFRA_API_KEY", key_optional: false,
               model: "meta-llama/Llama-3.3-70B-Instruct", note: "open-weight models" },
    Provider { name: "nebius", label: "Nebius AI Studio", kind: "openai", base: "https://api.studio.nebius.ai/v1", key_env: "NEBIUS_API_KEY", key_optional: false,
               model: "meta-llama/Llama-3.3-70B-Instruct", note: "open-weight models" },
    Provider { name: "novita", label: "Novita AI", kind: "openai", base: "https://api.novita.ai/v3/openai", key_env: "NOVITA_API_KEY", key_optional: false,
               model: "meta-llama/llama-3.3-70b-instruct", note: "open-weight models" },
    Provider { name: "hyperbolic", label: "Hyperbolic", kind: "openai", base: "https://api.hyperbolic.xyz/v1", key_env: "HYPERBOLIC_API_KEY", key_optional: false,
               model: "meta-llama/Llama-3.3-70B-Instruct", note: "open-weight models" },
    Provider { name: "lambda", label: "Lambda Inference", kind: "openai", base: "https://api.lambdalabs.com/v1", key_env: "LAMBDA_API_KEY", key_optional: false,
               model: "llama3.3-70b-instruct-fp8", note: "Lambda's inference API" },
    Provider { name: "sambanova", label: "SambaNova", kind: "openai", base: "https://api.sambanova.ai/v1", key_env: "SAMBANOVA_API_KEY", key_optional: false,
               model: "Meta-Llama-3.3-70B-Instruct", note: "fast open-weight inference" },
    Provider { name: "nous", label: "Nous Research", kind: "openai", base: "https://inference-api.nousresearch.com/v1", key_env: "NOUS_API_KEY", key_optional: false,
               model: "Hermes-3-Llama-3.1-70B", note: "Hermes models, OpenAI-compatible" },
    Provider { name: "github-models", label: "GitHub Models", kind: "openai", base: "https://models.inference.ai.azure.com", key_env: "GITHUB_TOKEN", key_optional: false,
               model: "gpt-4o", note: "a GitHub token with the models scope; rate limited by plan" },
    Provider { name: "openrouter", label: "OpenRouter", kind: "openai", base: "https://openrouter.ai/api/v1", key_env: "OPENROUTER_API_KEY", key_optional: false,
               model: "openai/gpt-5.2", note: "one key for models of many providers" },
    Provider { name: "alibaba-intl", label: "Alibaba Cloud Model Studio, Singapore", kind: "openai",
               base: "https://{WorkspaceId}.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1", key_env: "DASHSCOPE_API_KEY", key_optional: false,
               model: "qwen4-max", note: "Qwen models, needs --workspace ID, the key belongs to this region" },
    Provider { name: "alibaba-cn", label: "Alibaba Cloud Model Studio, Beijing", kind: "openai",
               base: "https://{WorkspaceId}.cn-beijing.maas.aliyuncs.com/compatible-mode/v1", key_env: "DASHSCOPE_API_KEY", key_optional: false,
               model: "qwen4-max", note: "Qwen models in mainland China, needs --workspace ID, the key belongs to this region" },
    Provider { name: "deepseek", label: "DeepSeek", kind: "openai", base: "https://api.deepseek.com", key_env: "DEEPSEEK_API_KEY", key_optional: false,
               model: "deepseek-flash", note: "the address has no /v1 part" },
    Provider { name: "mistral", label: "Mistral", kind: "openai", base: "https://api.mistral.ai/v1", key_env: "MISTRAL_API_KEY", key_optional: false,
               model: "mistral-large-latest", note: "chat completions" },
    Provider { name: "groq", label: "Groq", kind: "openai", base: "https://api.groq.com/openai/v1", key_env: "GROQ_API_KEY", key_optional: false,
               model: "llama-3.3-70b-versatile", note: "rejects the fields logprobs, logit_bias and messages[].name" },
    Provider { name: "together", label: "Together AI", kind: "openai", base: "https://api.together.ai/v1", key_env: "TOGETHER_API_KEY", key_optional: false,
               model: "MiniMaxAI/MiniMax-M3.1-Flash", note: "open-weight models" },
    Provider { name: "fireworks", label: "Fireworks AI", kind: "openai", base: "https://api.fireworks.ai/inference/v1", key_env: "FIREWORKS_API_KEY", key_optional: false,
               model: "accounts/fireworks/models/glm-5p3-flash", note: "model names start with accounts/fireworks/models/" },
    Provider { name: "gemini", label: "Google Gemini", kind: "openai", base: "https://generativelanguage.googleapis.com/v1beta/openai", key_env: "GEMINI_API_KEY",
               key_optional: false, model: "gemini-4-hydrogen", note: "the OpenAI-compatible address of the Gemini API" },
    Provider { name: "xai", label: "xAI", kind: "openai", base: "https://api.x.ai/v1", key_env: "XAI_API_KEY", key_optional: false,
               model: "grok-4.7", note: "the documentation calls chat completions legacy" },
    Provider { name: "cerebras", label: "Cerebras", kind: "openai", base: "https://api.cerebras.ai/v1", key_env: "CEREBRAS_API_KEY", key_optional: false,
               model: "qwen-3.8-27b", note: "chat completions" },
    Provider { name: "moonshot", label: "Moonshot (Kimi), international", kind: "openai", base: "https://api.moonshot.ai/v1", key_env: "MOONSHOT_API_KEY", key_optional: false,
               model: "kimi-k3.1-pro", note: "Kimi models" },
    Provider { name: "moonshot-cn", label: "Moonshot (Kimi), mainland China", kind: "openai", base: "https://api.moonshot.cn/v1", key_env: "MOONSHOT_API_KEY", key_optional: false,
               model: "kimi-k3.1-pro", note: "Kimi models, the address for mainland China" },
    Provider { name: "zhipu", label: "Zhipu (GLM), international as Z.ai", kind: "openai", base: "https://api.z.ai/api/paas/v4", key_env: "ZAI_API_KEY", key_optional: false,
               model: "glm-5.3", note: "GLM models" },
    Provider { name: "zhipu-cn", label: "Zhipu (GLM), mainland China", kind: "openai", base: "https://open.bigmodel.cn/api/paas/v4", key_env: "ZAI_API_KEY", key_optional: false,
               model: "glm-5.3", note: "GLM models, the address for mainland China" },
    Provider { name: "huggingface", label: "Hugging Face inference router", kind: "openai", base: "https://router.huggingface.co/v1", key_env: "HF_TOKEN", key_optional: false,
               model: "deepseek-ai/DeepSeek-V4.1-Flash", note: "chat only, a suffix :fastest or :cheapest picks the provider" },
    Provider { name: "opencode", label: "OpenCode Go", kind: "openai", base: "https://opencode.ai/zen/go/v1", key_env: "OPENCODE_API_KEY",
               key_optional: false, model: "mimo-v2.6-flash", note: "one key for many models; Penelope sends the x-opencode-session header" },
    Provider { name: "ollama", label: "Ollama on this PC", kind: "openai", base: "http://localhost:11434/v1", key_env: "", key_optional: true,
               model: "qwen3:8b", note: "local, Ollama ignores the key" },
    Provider { name: "lmstudio", label: "LM Studio on this PC", kind: "openai", base: "http://localhost:1234/v1", key_env: "LM_API_TOKEN", key_optional: true,
               model: "ibm/granite-4-micro", note: "local, the token counts only when the server asks for one" },
    Provider { name: "llamacpp", label: "llama.cpp server on this PC", kind: "openai", base: "http://localhost:9931/v1", key_env: "LLAMA_API_KEY", key_optional: true,
               model: "gpt-3.5-turbo", note: "local, tool calls need the server flag --jinja, the key counts only with --api-key" },
    Provider { name: "vllm", label: "vLLM on this PC", kind: "openai", base: "http://localhost:8000/v1", key_env: "VLLM_API_KEY", key_optional: true,
               model: "Qwen/Qwen2.5-1.5B-Instruct", note: "local, tool calls need --enable-auto-tool-choice and --tool-call-parser" },
];

pub fn get(name: &str) -> Option<&'static Provider> {
    TABLE.iter().find(|p| p.name == name)
}

/// The table entry a model setting belongs to: same kind and same address. An Alibaba address matches with any workspace.
pub fn of(model: &Value) -> Option<&'static Provider> {
    let kind = model["kind"].as_str().unwrap_or("claude-cli");
    if kind == "claude-cli" {
        return get("claude-cli");
    }
    let base = model["base_url"].as_str().unwrap_or("").trim_end_matches('/');
    TABLE.iter().find(|p| p.kind == kind && match p.base.split_once(WORKSPACE) {
        Some((head, tail)) => base.starts_with(head) && base.ends_with(tail) && base.len() > head.len() + tail.len(),
        None => p.base == base,
    })
}

/// True if a text can be the name of an environment variable. A pasted key holds a dash or starts with lower case
/// letters in every key format the privacy filter knows, so it fails this check and never reaches `config.json`.
pub fn is_env_name(text: &str) -> bool {
    !text.is_empty() && text.len() <= 64 && text.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        && !text.starts_with(|c: char| c.is_ascii_digit())
}

/// A friendly label for a model whose name is a slug. The crew panel shows this instead of `claude-haiku-5-5`.
pub fn known_label(model: &str) -> Option<&'static str> {
    match model.trim().to_lowercase().as_str() {
        "haiku" => Some("Haiku"),
        "sonnet" => Some("Sonnet"),
        "opus" => Some("Opus"),
        "claude-haiku-5-5" => Some("Haiku 5.5"),
        "claude-sonnet-5-5" => Some("Sonnet 5.5"),
        "claude-opus-5-5" => Some("Opus 5.5"),
        "mimo-v2.6-flash" => Some("MiMo V2.6 Flash"),
        "deepseek-v4.1-flash" => Some("DeepSeek V4.1 Flash"),
        "deepseek-v4-pro" => Some("DeepSeek V4 Pro"),
        "glm-5.3-flash" => Some("GLM-5.3 Flash"),
        "glm-5.3" => Some("GLM-5.3"),
        "qwen3.8-flash" => Some("Qwen3.8 Flash"),
        "kimi-k3" => Some("Kimi K3"),
        _ => None,
    }
}

/// A list price in dollars per million tokens for a model family whose public list we know. Used only when
/// `provider use` writes a setting and the settings folder has no price for that name yet. A subscription price is
/// still a proposal, not a bill.
pub fn known_price(model: &str) -> Option<(f64, f64)> {
    let m = model.to_lowercase();
    let mut best: Option<(usize, f64, f64)> = None;
    for (name, i, o) in [("claude-haiku-5-5", 0.1, 0.5), ("mimo-v2.6", 0.14, 0.28), ("deepseek-v4.1", 0.15, 0.6),
                         ("glm-5.3", 0.2, 0.8), ("qwen3.8", 0.2, 0.6), ("kimi-k3", 0.6, 2.5), ("sonnet", 3.0, 15.0),
                         ("opus", 15.0, 75.0), ("haiku", 0.8, 4.0)] {
        if m.contains(name) && best.map(|(l, _, _)| name.len() > l).unwrap_or(true) {
            best = Some((name.len(), i, o));
        }
    }
    best.map(|(_, i, o)| (i, o))
}

/// The key of a model setting, read from the environment variable it names. Empty when none is named or set.
pub fn key(model: &Value) -> String {
    // The environment wins for one session; a stored login is the fallback.
    if let Some(v) = model["api_key_env"].as_str().filter(|n| is_env_name(n)).and_then(|n| std::env::var(n).ok()) {
        if !v.trim().is_empty() {
            return v.trim().to_string();
        }
    }
    of(model).map(|p| crate::auth::get(p.name)).unwrap_or_default()
}

/// The state of a provider for the table and `doctor`: the environment first, then a stored login.
pub fn state_of(p: &Provider) -> &'static str {
    let env = key_state(p.key_env, p.key_optional);
    if env == "key set" || env == "no key needed" || env == "no key, optional" {
        return env;
    }
    if crate::auth::has(p.name) { "stored" } else { env }
}

/// "key set" or "no key" for one variable name. The value itself is never returned.
pub fn key_state(key_env: &str, optional: bool) -> &'static str {
    if key_env.is_empty() {
        return "no key needed";
    }
    match std::env::var(key_env) {
        Ok(v) if !v.trim().is_empty() => "key set",
        _ if optional => "no key, optional",
        _ => "no key",
    }
}

/// The table as text: one row per provider with the state of its key.
pub fn list(ctx: &Ctx) -> String {
    let mut out = format!("{:<13} {:<10} {:<17} {:<19} {}\n", "name", "kind", "key", "variable", "address");
    for p in TABLE {
        out.push_str(&format!("{:<13} {:<10} {:<17} {:<19} {}\n", p.name, p.kind, state_of(p),
                              if p.key_env.is_empty() { "-" } else { p.key_env }, if p.base.is_empty() { "-" } else { p.base }));
        out.push_str(&format!("{:<13} example model {} · {}\n", "", p.model, p.note));
    }
    out.push_str("\nIn use here:\n");
    for role in ["main", "weak", "scout"] {
        let m = &ctx.cfg[role];
        if m["model"].is_string() {
            out.push_str(&format!("  {:<9} {} on {}\n", role, m["model"].as_str().unwrap_or("?"), of(m).map(|p| p.name).unwrap_or("an address outside the table")));
        }
    }
    for (role, what) in crate::route::ROLES {
        let m = &ctx.cfg["roles"][role];
        if m["model"].is_string() {
            out.push_str(&format!("  {:<9} {} on {} · {}\n", role, m["model"].as_str().unwrap_or("?"), of(m).map(|p| p.name).unwrap_or("an address outside the table"), what));
        } else if matches!(*role, "router" | "filter") {
            out.push_str(&format!("  {:<9} fixed rules · {}\n", role, what));
        }
    }
    out.push_str("\npenelope provider use NAME [--role main|weak|scout] [--model MODEL] [--workspace ID] [--base-url URL]\n\
A key is read from its environment variable when a request starts. No command prints or stores it.\n");
    out
}

/// The model setting for one provider. `workspace` fills the workspace part of an Alibaba address.
pub fn setting(p: &Provider, model: Option<&str>, workspace: Option<&str>, base_url: Option<&str>) -> Result<Value, String> {
    let model = model.filter(|m| !m.trim().is_empty()).unwrap_or(p.model).trim();
    if p.kind == "claude-cli" {
        let mut v = json!({"kind": "claude-cli", "model": model});
        if let Some(label) = known_label(model) {
            v["label"] = json!(label);
        }
        return Ok(v);
    }
    if p.kind == "laertes" {
        let family = crate::crew::family_of(&json!({"model": model})).unwrap_or("Telemach");
        return Ok(json!({"kind": "laertes", "family": family, "model": model, "label": format!("{} 1", family)}));
    }
    let base = match (base_url.map(|b| b.trim().trim_end_matches('/')), p.base.contains(WORKSPACE)) {
        (Some(b), _) if b.starts_with("http://") || b.starts_with("https://") => b.to_string(),
        (Some(_), _) => return Err("NOT set. An address starts with http:// or https://.".to_string()),
        (None, true) => match workspace.map(str::trim).filter(|w| !w.is_empty() && w.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')) {
            Some(w) => p.base.replace(WORKSPACE, w),
            None => return Err(format!("NOT set. The address of {} holds the workspace of your account: {}. Give it with --workspace ID. \
The console of Model Studio shows the ID in the column API Host.", p.name, p.base)),
        },
        (None, false) => p.base.to_string(),
    };
    let mut v = json!({"kind": p.kind, "base_url": base, "model": model, "api_key_env": p.key_env});
    if let Some(label) = known_label(model) {
        v["label"] = json!(label);
    }
    if base.contains("opencode.ai") {
        v["headers"] = json!({"x-opencode-session": "penelope"});
    }
    if p.key_env.is_empty() {
        v.as_object_mut().unwrap().remove("api_key_env");
    }
    Ok(v)
}

/// `penelope provider use NAME [--role R] [--model M] [--workspace ID] [--base-url URL]`: writes one model setting to
/// `config.json`. The file gets the name of the key variable and no key.
pub fn use_provider(ctx: &mut Ctx, args: &[String]) -> String {
    let flag = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).map(String::as_str);
    let Some(name) = args.first().filter(|a| !a.starts_with("--")) else {
        return "NOT set. Name a provider: penelope provider use NAME. penelope providers lists the names.".to_string();
    };
    let Some(p) = get(name) else {
        return format!("NOT set. No provider {} in the table. penelope providers lists the {} names.", name, TABLE.len());
    };
    let role = flag("--role").unwrap_or("main");
    let slot = match role {
        "main" | "lead" => "main".to_string(),
        "weak" | "condenser" => "weak".to_string(),
        "scout" => "scout".to_string(),
        r if crate::route::role_names().contains(&r) => format!("roles.{}", r),
        _ => return format!("NOT set. The role is main, weak, scout or one of: {}.", crate::route::role_names().join(", ")),
    };
    let value = match setting(p, flag("--model"), flag("--workspace"), flag("--base-url")) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if let Some(name) = slot.strip_prefix("roles.") {
        if !ctx.cfg["roles"].is_object() {
            ctx.cfg["roles"] = json!({});
        }
        ctx.cfg["roles"][name] = value.clone();
    } else {
        ctx.cfg[&slot] = value.clone();
    }
    // A known list price makes the euro report useful from the first call. An explicit price of the user wins.
    if let Some((input, output)) = known_price(value["model"].as_str().unwrap_or("")) {
        let key = value["model"].as_str().unwrap_or("");
        if ctx.cfg["prices"][key].is_null() {
            if !ctx.cfg["prices"].is_object() {
                ctx.cfg["prices"] = json!({});
            }
            ctx.cfg["prices"][key] = json!([input, output]);
        }
    }
    if let Err(e) = std::fs::write(ctx.root.join("config.json"), serde_json::to_string_pretty(&ctx.cfg).unwrap()) {
        return format!("NOT set. cannot write config.json: {}", e);
    }
    format!("set: {} model {} on {} ({}, {}). Key: {}{}.", role, value["model"].as_str().unwrap_or("?"), p.name, p.kind,
            value["base_url"].as_str().unwrap_or("the claude command"), state_of(p),
            if p.key_env.is_empty() { String::new() } else { format!(" in {}", p.key_env) })
}

/// One exact model name, its display name and the effort levels the provider accepts.
pub struct Model {
    pub id: &'static str,
    pub label: &'static str,
    pub efforts: &'static [&'static str],
}

const CLAUDE_MODELS: &[Model] = &[
    Model { id: "claude-opus-5-5", label: "Claude Opus 5.5", efforts: &["low", "medium", "high", "max"] },
    Model { id: "claude-sonnet-5-5", label: "Claude Sonnet 5.5", efforts: &["low", "medium", "high", "max"] },
    Model { id: "claude-haiku-5-5", label: "Claude Haiku 5.5", efforts: &["low", "medium", "high", "max"] },
];

const OPENCODE_MODELS: &[Model] = &[
    Model { id: "deepseek-v4.1-flash", label: "DeepSeek V4.1 Flash 1017", efforts: &["low", "high", "max"] },
    Model { id: "deepseek-v4-pro", label: "DeepSeek V4 Pro", efforts: &["high", "max"] },
    Model { id: "glm-5.3", label: "GLM-5.3", efforts: &["low", "high", "max"] },
    Model { id: "glm-5.3-flash", label: "GLM-5.3 Flash", efforts: &["low", "high", "max"] },
    Model { id: "grok-4.7", label: "Grok 4.7", efforts: &["low", "medium", "high", "xhigh"] },
    Model { id: "muse-spark-1.3-contributor", label: "Muse Spark 1.3", efforts: &["minimal", "low", "medium", "high", "xhigh"] },
    Model { id: "gpt-6-luna", label: "GPT-6 Luna", efforts: &["none", "low", "medium", "high", "xhigh", "max"] },
    Model { id: "gpt-5.6-luna", label: "GPT-5.6 Luna", efforts: &["none", "low", "medium", "high", "xhigh", "max"] },
    Model { id: "claude-haiku-5-5", label: "Claude Haiku 5.5", efforts: &["low", "medium", "high", "xhigh", "max"] },
    Model { id: "kimi-k3", label: "Kimi K3", efforts: &["max"] },
    Model { id: "kimi-k3.1-pro", label: "Kimi K3.1 Pro", efforts: &["low", "high", "max"] },
    Model { id: "kimi-k2.7-code", label: "Kimi K2.7 Code", efforts: &[] },
    Model { id: "minimax-m3.1-flash", label: "MiniMax M3.1 Flash", efforts: &["low", "medium", "high", "xhigh", "max"] },
    Model { id: "minimax-m3", label: "MiniMax M3", efforts: &[] },
    Model { id: "mimo-v2.6-flash", label: "MiMo V2.6 Flash", efforts: &[] },
    Model { id: "mimo-v2.6-pro", label: "MiMo V2.6 Pro", efforts: &[] },
    Model { id: "qwen4-max", label: "Qwen4 Max", efforts: &["low", "high", "xhigh"] },
    Model { id: "qwen3.8-max", label: "Qwen3.8 Max", efforts: &["low", "medium", "xhigh"] },
    Model { id: "qwen3.8-flash", label: "Qwen3.8 Flash", efforts: &["low", "medium", "xhigh"] },
    Model { id: "qwen3.7-plus", label: "Qwen3.7 Plus", efforts: &[] },
    Model { id: "gemini-4-argon", label: "Gemini 4 Argon", efforts: &["low", "medium", "high"] },
    Model { id: "gemini-decision-1", label: "Gemini Decision 1", efforts: &[] },
    Model { id: "gpt-6-luna-decision", label: "GPT-6 Luna Decision", efforts: &[] },
    Model { id: "step-5-preview", label: "Step 5 Preview", efforts: &["low", "medium", "high"] },
    Model { id: "hy4", label: "Hy4", efforts: &["none", "high"] },
    Model { id: "hy4-preview", label: "Hy4 Preview", efforts: &["none", "high"] },
    Model { id: "hy3", label: "Hy3", efforts: &["none", "low", "high"] },
    Model { id: "longcat-2.5", label: "LongCat 2.5", efforts: &[] },
    Model { id: "jev-1.14", label: "Jev 1.14", efforts: &[] },
];

/// The exact model list of a provider, when Penelope knows more than the one example in the table.
pub fn models(provider: &str) -> &'static [Model] {
    match provider {
        "claude-cli" => CLAUDE_MODELS,
        "opencode" => OPENCODE_MODELS,
        _ => &[],
    }
}

/// The efforts one model accepts. Empty means the provider has no effort control for it.
pub fn efforts(provider: &str, model: &str) -> &'static [&'static str] {
    models(provider).iter().find(|m| m.id == model).map(|m| m.efforts).unwrap_or(&[])
}

/// The first effort to use when a model is chosen. `high` is a deliberate default for coding and tool calls.
pub fn default_effort(provider: &str, model: &str) -> &'static str {
    let list = efforts(provider, model);
    if list.contains(&"high") { "high" } else { list.first().copied().unwrap_or("") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_is_well_formed_and_a_setting_names_the_variable_only() {
        assert_eq!(TABLE.len(), 35);
        for p in TABLE {
            assert!(["openai", "anthropic", "claude-cli", "laertes"].contains(&p.kind) && !p.model.is_empty() && !p.note.is_empty(), "{}", p.name);
            assert!(p.key_env.is_empty() || is_env_name(p.key_env), "{}", p.name);
            assert!(p.kind == "claude-cli" || p.kind == "laertes" || p.base.starts_with("http"), "{}", p.name);
            assert!(!p.base.ends_with('/') && TABLE.iter().filter(|q| q.name == p.name).count() == 1, "{}", p.name);
        }
        let groq = setting(get("groq").unwrap(), None, None, None).unwrap();
        assert_eq!(groq, json!({"kind": "openai", "base_url": "https://api.groq.com/openai/v1", "model": "llama-3.3-70b-versatile", "api_key_env": "GROQ_API_KEY"}));
        assert_eq!(of(&groq).unwrap().name, "groq");
        let ali = get("alibaba-intl").unwrap();
        assert!(setting(ali, None, None, None).unwrap_err().starts_with("NOT set"));
        let set = setting(ali, Some("qwen-plus"), Some("llm-abc123"), None).unwrap();
        assert_eq!(set["base_url"], "https://llm-abc123.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1");
        assert_eq!((of(&set).unwrap().name, set["model"].as_str()), ("alibaba-intl", Some("qwen-plus")));
        assert_eq!(setting(get("ollama").unwrap(), None, None, None).unwrap(), json!({"kind": "openai", "base_url": "http://localhost:11434/v1", "model": "qwen3:8b"}));
        assert!(is_env_name("GROQ_API_KEY") && !is_env_name("sk-abc123") && !is_env_name("gsk_abc") && !is_env_name(""));
        assert_eq!(known_label("mimo-v2.6-flash"), Some("MiMo V2.6 Flash"));
        assert_eq!(known_price("claude-haiku-5-5"), Some((0.1, 0.5)));
        assert_eq!(known_price("a-model-without-a-price"), None);
        let mut ctx = crate::config::load(&std::env::temp_dir().join(format!("penelope-price-{}", std::process::id())));
        ctx.cfg["prices"] = json!({});
        let said = use_provider(&mut ctx, &["opencode".to_string(), "--model".to_string(), "mimo-v2.6-flash".to_string()]);
        assert!(said.contains("set: main model mimo-v2.6-flash"));
        assert_eq!(ctx.cfg["prices"]["mimo-v2.6-flash"], json!([0.14, 0.28]));
        assert_eq!(ctx.cfg["main"]["label"], "MiMo V2.6 Flash");
    }
}
