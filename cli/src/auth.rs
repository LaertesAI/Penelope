//! Login for Claude and the API providers.
//!
//! Claude uses the official `claude auth login`; Penelope reads the status and never keeps the OAuth token itself.
//! Every other provider takes an API key. The key goes to the Secret Service (`secret-tool`) when this desktop has one,
//! else to `~/.penelope/auth.json` with mode 0600. The settings file only ever holds the name of the environment
//! variable, never a key. A key is printed nowhere: status says `stored`, not what is stored.
//!
//! If `OPENAI_API_KEY` (or the variable of the provider) is set, that value wins over the stored one, so a shell can
//! override a login for one session.

use crate::config::Ctx;
use serde_json::{json, Value};
use std::fs;
use std::io::{BufRead, IsTerminal, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn home() -> PathBuf {
    crate::platform::home()
}

/// The file that holds the fallback keys and the index of keyring entries.
pub fn auth_file() -> PathBuf {
    match std::env::var("PENELOPE_AUTH_FILE").ok().filter(|p| !p.trim().is_empty()) {
        Some(p) => PathBuf::from(p),
        None => home().join(".penelope").join("auth.json"),
    }
}

fn read_file(path: &std::path::Path) -> Value {
    fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_else(|| json!({"keys": {}, "providers": {}}))
}

fn write_file(path: &std::path::Path, value: &Value) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {}", dir.display(), e))?;
    }
    fs::write(path, serde_json::to_string_pretty(value).unwrap()).map_err(|e| format!("cannot write {}: {}", path.display(), e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

fn secret(args: &[&str], stdin: Option<&str>) -> Option<String> {
    if crate::config::which("secret-tool").is_none() {
        return None;
    }
    let mut child = Command::new("secret-tool").args(args).stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    if let Some(text) = stdin {
        use std::io::Write;
        child.stdin.take()?.write_all(text.as_bytes()).ok()?;
    }
    let out = child.wait_with_output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn keyring_get(provider: &str) -> Option<String> {
    secret(&["lookup", "service", "penelope", "provider", provider], None).filter(|s| !s.is_empty())
}

fn keyring_store(provider: &str, key: &str) -> bool {
    secret(&["store", "--label", &format!("Penelope {}", provider), "service", "penelope", "provider", provider], Some(key)).is_some()
}

fn keyring_remove(provider: &str) -> bool {
    secret(&["clear", "service", "penelope", "provider", provider], None).is_some()
}

/// The stored key of a provider, from the keyring if its index says so, else from the file. Empty when none is stored.
pub fn get(provider: &str) -> String {
    let path = auth_file();
    let v = read_file(&path);
    match v["providers"][provider]["store"].as_str() {
        Some("keyring") => keyring_get(provider).unwrap_or_default(),
        Some("file") => v["keys"][provider].as_str().unwrap_or("").to_string(),
        _ => String::new(),
    }
}

/// True when the index says a key is stored for this provider. It does not ask the keyring, so a status page stays
/// fast; `get` does the real lookup when a request needs the key.
pub fn has(provider: &str) -> bool {
    let v = read_file(&auth_file());
    matches!(v["providers"][provider]["store"].as_str(), Some("keyring") | Some("file"))
}

/// Stores one key. The keyring is tried first; a desktop without one gets the file with mode 0600.
pub fn store(provider: &str, key: &str) -> Result<&'static str, String> {
    if key.trim().len() < 8 {
        return Err("NOT stored. The key is shorter than 8 characters.".to_string());
    }
    let path = auth_file();
    let mut v = read_file(&path);
    if keyring_store(provider, key.trim()) {
        if let Some(keys) = v["keys"].as_object_mut() {
            keys.remove(provider);
        }
        v["providers"][provider] = json!({"store": "keyring", "at": crate::sessions::stamp(now())});
        write_file(&path, &v)?;
        return Ok("the Secret Service");
    }
    v["keys"][provider] = json!(key.trim());
    v["providers"][provider] = json!({"store": "file", "at": crate::sessions::stamp(now())});
    write_file(&path, &v)?;
    Ok("the file ~/.penelope/auth.json (mode 0600)")
}

/// Removes one stored key from both stores.
pub fn remove(provider: &str) -> String {
    let path = auth_file();
    let mut v = read_file(&path);
    let had = has(provider);
    if let Some(keys) = v["keys"].as_object_mut() {
        keys.remove(provider);
    }
    if let Some(providers) = v["providers"].as_object_mut() {
        providers.remove(provider);
    }
    let _ = write_file(&path, &v);
    let _ = keyring_remove(provider);
    if had { format!("{}: the stored key is removed", provider) } else { format!("{}: no stored key", provider) }
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// The status of the Claude command as one line without the e-mail address.
pub fn claude_status() -> String {
    let out = match Command::new("claude").args(["auth", "status"]).output() {
        Ok(o) => o,
        Err(_) => return "claude: the claude command is not on the PATH".to_string(),
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let v: Value = serde_json::from_str(text.trim()).unwrap_or(Value::Null);
    if v["loggedIn"].as_bool().unwrap_or(false) {
        format!("claude: signed in via {}{}", v["authMethod"].as_str().unwrap_or("account"),
                v["subscriptionType"].as_str().map(|s| format!(" ({})", s)).unwrap_or_default())
    } else if out.status.success() {
        "claude: not signed in. Run: penelope login claude".to_string()
    } else {
        "claude: the status could not be read".to_string()
    }
}

fn read_secret(prompt: &str) -> Result<String, String> {
    print!("{}", prompt);
    let _ = std::io::stdout().flush();
    let stdin = std::io::stdin();
    let hidden = stdin.is_terminal() && cfg!(unix);
    if hidden {
        let _ = Command::new("stty").arg("-echo").status();
    }
    let mut line = String::new();
    let read = stdin.lock().read_line(&mut line);
    if hidden {
        let _ = Command::new("stty").arg("echo").status();
        println!();
    }
    read.map_err(|e| e.to_string())?;
    let key = line.trim().to_string();
    if key.is_empty() {
        return Err("NOT stored. No key was typed.".to_string());
    }
    Ok(key)
}

/// `penelope login [provider]` and `/login`. `claude` runs the official login; every other name takes an API key.
pub fn login(provider: &str) -> Result<String, String> {
    let name = if provider.trim().is_empty() {
        // The main model decides which login is meant.
        String::new()
    } else {
        provider.trim().to_lowercase()
    };
    let name = if name.is_empty() {
        let ctx = crate::config::load(&crate::config::find_root(None));
        crate::providers::of(&ctx.cfg["main"]).map(|p| p.name).unwrap_or("claude-cli").to_string()
    } else {
        name
    };
    if name == "claude" || name == "claude-cli" {
        let status = Command::new("claude").args(["auth", "login"]).status().map_err(|e| format!("cannot start the claude command: {}", e))?;
        if !status.success() {
            return Err(format!("the claude login ended with {}", status));
        }
        return Ok(claude_status());
    }
    if let Some((_, env)) = crate::search::SEARCH_KEYS.iter().find(|(n, _)| *n == name) {
        let key = read_secret(&format!("{}: paste the API key (it is not shown): ", name))?;
        let store = store(&name, &key)?;
        return Ok(format!("{}: search key stored in {}. A search reads it when {} is not set.", name, store, env));
    }
    let Some(p) = crate::providers::get(&name) else {
        return Err(format!("NOT logged in. No provider {} in the table and no search key. penelope providers lists the names.", name));
    };
    if p.key_env.is_empty() || p.kind == "laertes" {
        return Err(format!("{} takes no API key.", p.name));
    }
    let key = read_secret(&format!("{}: paste the API key (it is not shown): ", p.name))?;
    let store = store(p.name, &key)?;
    Ok(format!("{}: key stored in {}. A request reads it from there when {} is not set.", p.name, store, p.key_env))
}

/// One live check of a model setting. It never prints the key; it says what the provider answered.
fn verify_one(ctx: &Ctx, role: &str) -> String {
    let m = &ctx.cfg[role];
    let model = m["model"].as_str().unwrap_or("?");
    let kind = m["kind"].as_str().unwrap_or("claude-cli");
    let provider = crate::providers::of(m);
    let name = provider.map(|p| p.name).unwrap_or("an address outside the table");
    let started = std::time::Instant::now();
    if kind == "claude-cli" {
        let status = claude_status();
        let ok = status.contains("signed in");
        return format!("{}: {} — {} · {}", role, if ok { "ok" } else { "FAIL" }, status, elapsed(started));
    }
    if kind == "laertes" {
        return format!("{}: FAIL — {} / {} · a Laertes family is in training and cannot answer yet", role, name, model);
    }
    let env = m["api_key_env"].as_str().filter(|s| !s.is_empty()).or_else(|| provider.map(|p| p.key_env)).unwrap_or("");
    let key = std::env::var(env).ok().filter(|v| !v.trim().is_empty()).unwrap_or_else(|| get(name));
    if key.is_empty() && !(provider.map(|p| p.key_optional).unwrap_or(false) || model.starts_with("local")) {
        return format!("{}: FAIL — {} / {} · no key in {}", role, name, model, if env.is_empty() { "the environment" } else { env });
    }
    let base = m["base_url"].as_str().filter(|b| !b.is_empty()).or_else(|| provider.map(|p| p.base)).unwrap_or("");
    if base.is_empty() {
        return format!("{}: FAIL — {} / {} · no address", role, name, model);
    }
    let base = base.trim_end_matches('/');
    let result = if kind == "anthropic" { verify_anthropic(base, model, &key) } else { verify_openai(base, model, &key) };
    match result {
        Ok(detail) => format!("{}: ok — {} / {} · {} · {}", role, name, model, detail, elapsed(started)),
        Err(e) => format!("{}: FAIL — {} / {} · {} · {}", role, name, model, e, elapsed(started)),
    }
}

fn elapsed(start: std::time::Instant) -> String {
    format!("{} ms", start.elapsed().as_millis())
}

fn verify_openai(base: &str, _model: &str, key: &str) -> Result<String, String> {
    let mut request = crate::net::agent(15).get(&format!("{}/models", base));
    if !key.is_empty() {
        request = request.set("Authorization", &format!("Bearer {}", key));
    }
    match request.call() {
        Ok(response) if response.status() == 200 => {
            let value: Value = response.into_json().unwrap_or(Value::Null);
            let count = value["data"].as_array().map(|a| a.len()).unwrap_or(0);
            Ok(if count > 0 { format!("{} models listed", count) } else { "key accepted".to_string() })
        }
        Ok(response) => Err(format!("the provider answered HTTP {}", response.status())),
        Err(ureq::Error::Status(404 | 405, _)) => verify_openai_chat(base, _model, key),
        Err(ureq::Error::Status(code, _)) => Err(format!("the provider answered HTTP {}", code)),
        Err(e) => Err(format!("no answer: {}", e)),
    }
}

fn verify_openai_chat(base: &str, model: &str, key: &str) -> Result<String, String> {
    let mut request = crate::net::agent(20).post(&format!("{}/chat/completions", base));
    if !key.is_empty() {
        request = request.set("Authorization", &format!("Bearer {}", key));
    }
    match request.send_json(serde_json::json!({"model": model, "messages": [{"role": "user", "content": "ping"}], "max_tokens": 1})) {
        Ok(_) => Ok("the model answered a one-token ping".to_string()),
        Err(ureq::Error::Status(code, _)) => Err(format!("the model endpoint answered HTTP {}", code)),
        Err(e) => Err(format!("no answer: {}", e)),
    }
}

fn verify_anthropic(base: &str, _model: &str, key: &str) -> Result<String, String> {
    let response = crate::net::agent(15).get(&format!("{}/v1/models", base))
        .set("x-api-key", key).set("anthropic-version", "2023-06-01").call();
    match response {
        Ok(response) if response.status() == 200 => {
            let value: Value = response.into_json().unwrap_or(Value::Null);
            let count = value["data"].as_array().map(|a| a.len()).unwrap_or(0);
            Ok(if count > 0 { format!("{} models listed", count) } else { "key accepted".to_string() })
        }
        Ok(response) => Err(format!("the provider answered HTTP {}", response.status())),
        Err(ureq::Error::Status(404 | 405, _)) => verify_anthropic_chat(base, _model, key),
        Err(ureq::Error::Status(code, _)) => Err(format!("the provider answered HTTP {}", code)),
        Err(e) => Err(format!("no answer: {}", e)),
    }
}

fn verify_anthropic_chat(base: &str, model: &str, key: &str) -> Result<String, String> {
    let body = serde_json::json!({"model": model, "max_tokens": 1, "messages": [{"role": "user", "content": "ping"}]});
    match crate::net::agent(20).post(&format!("{}/v1/messages", base))
        .set("x-api-key", key).set("anthropic-version", "2023-06-01").send_json(body) {
        Ok(_) => Ok("the model answered a one-token ping".to_string()),
        Err(ureq::Error::Status(code, _)) => Err(format!("the model endpoint answered HTTP {}", code)),
        Err(e) => Err(format!("no answer: {}", e)),
    }
}

/// `penelope auth verify [main|weak|scout]`: one real request per model setting. It verifies the login, not the bill.
pub fn verify(ctx: &Ctx, only: Option<&str>) -> String {
    let roles: Vec<&str> = match only.map(str::trim).filter(|s| !s.is_empty()) {
        Some(role @ ("main" | "weak" | "scout")) => vec![role],
        Some(other) => return format!("verify takes main, weak or scout, not {}", other),
        None => vec!["main", "weak", "scout"],
    };
    let mut out = format!("{} \n", claude_status());
    for role in roles {
        out.push_str(&verify_one(ctx, role));
        out.push('\n');
    }
    out.push_str("\nA verify call uses the Claude auth status, or asks an HTTP provider for its model list (a one-token ping when there is no list). It sends no prompt of yours.\n");
    out
}

/// `penelope auth [status]` and `/login` without a provider: Claude, the stored keys and the keys of the environment.
pub fn status(ctx: &Ctx) -> String {
    let mut out = format!("{} \n", claude_status());
    let path = auth_file();
    out.push_str(&format!("auth file: {}\n\n", crate::files::show(&path)));
    out.push_str(&format!("{:<14} {:<16} {:<22} {}\n", "provider", "key", "variable", "address"));
    for p in crate::providers::TABLE.iter().filter(|p| !p.key_env.is_empty()) {
        let env = crate::providers::key_state(p.key_env, p.key_optional);
        let stored = if has(p.name) { "stored" } else { "no login" };
        let state = if env == "key set" { "env set" } else if stored == "stored" { "stored" } else { "no key" };
        let mark = if p.name == crate::providers::of(&ctx.cfg["main"]).map(|x| x.name).unwrap_or("") { "*" } else { " " };
        out.push_str(&format!("{}{:<13} {:<16} {:<22} {}\n", mark, p.name, state,
                              if p.key_env.is_empty() { "-" } else { p.key_env }, p.base));
    }
    out.push_str("\nsearch and reader keys\n");
    for (name, env) in crate::search::SEARCH_KEYS {
        let state = if std::env::var(env).ok().filter(|v| !v.trim().is_empty()).is_some() { "env set" }
                    else if has(name) { "stored" } else { "no key" };
        out.push_str(&format!(" {:<12} {:<10} {}\n", name, state, env));
    }
    out.push_str("\nlogin: penelope login NAME · remove: penelope logout NAME\n");
    out
}

/// The fallback file functions, tested without touching the home of the person.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_is_stored_and_read_back_from_the_file() {
        let path = std::env::temp_dir().join(format!("penelope-auth-{}.json", std::process::id()));
        let _ = fs::remove_file(&path);
        let mut v = read_file(&path);
        v["keys"]["groq"] = json!("test-key-0123456789");
        v["providers"]["groq"] = json!({"store": "file"});
        write_file(&path, &v).unwrap();
        assert!(path.is_file());
        let back = read_file(&path);
        assert_eq!(back["keys"]["groq"], "test-key-0123456789");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("test-key"));
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn verify_names_the_provider_answer_and_never_prints_the_key() {
        let (address, _seen) = crate::llm::fake::serve(vec![(200, serde_json::json!({"data": [{"id": "a"}, {"id": "b"}]}).to_string())]);
        let dir = std::env::temp_dir().join(format!("penelope-auth-verify-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        std::env::set_var("PENELOPE_VERIFY_TEST_KEY", "secret-verify-key");
        let mut ctx = crate::config::load(&dir.join(".penelope"));
        ctx.cfg["main"] = serde_json::json!({"kind": "openai", "model": "test-model", "base_url": address, "api_key_env": "PENELOPE_VERIFY_TEST_KEY"});
        let said = verify_one(&ctx, "main");
        assert!(said.starts_with("main: ok"), "{}", said);
        assert!(said.contains("2 models") && !said.contains("secret-verify-key"), "{}", said);
        std::env::remove_var("PENELOPE_VERIFY_TEST_KEY");
        let _ = fs::remove_dir_all(&dir);
    }
}
