//! A leak audit of the settings folder.
//!
//! It reads every text file under `.penelope` (except the auth file, which exists to hold a key), runs the same fixed
//! privacy rules over it and reports a file that still holds something that looks like a key. It also reads the model
//! settings: a key pasted into `config.json`, or an `http://` endpoint that is not on this PC. It never prints a value,
//! only a path and a count. It writes nothing.

use crate::config::Ctx;
use std::fs;
use std::path::Path;

fn walk(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let path = e.path();
        if path.is_dir() {
            walk(&path, out);
        } else {
            out.push(path);
        }
    }
}

fn local(base: &str) -> bool {
    ["http://localhost", "http://127.0.0.1", "http://[::1]"].iter().any(|h| base.starts_with(h)) || !base.starts_with("http://")
}

pub fn run(ctx: &Ctx) -> String {
    let mut out = String::new();
    let mut findings = 0usize;

    // 1. The settings: a key value in the file, or an unencrypted remote endpoint.
    let mut config = ctx.cfg.clone();
    if config["auth"].is_object() { config.as_object_mut().unwrap().remove("auth"); }
    for field in ["api_key", "key", "token", "secret"] {
        if config[field].is_string() {
            out.push_str(&format!("WARN config.json holds a value in \"{}\". A key belongs in the login store, not in the settings.\n", field));
            findings += 1;
        }
    }
    let mut endpoints = vec![("main", config["main"].clone()), ("weak", config["weak"].clone()), ("scout", config["scout"].clone())];
    if let Some(roles) = config["roles"].as_object() {
        for (name, model) in roles {
            endpoints.push((name.as_str(), model.clone()));
        }
    }
    for (role, model) in endpoints {
        if let Some(base) = model["base_url"].as_str() {
            if !local(base) {
                out.push_str(&format!("WARN role {} talks to {} over plain http. The key and the text can be read on the way.\n", role, base));
                findings += 1;
            }
        }
        if model["api_key"].is_string() {
            out.push_str(&format!("WARN role {} holds an api_key value in config.json.\n", role));
            findings += 1;
        }
    }

    // 2. Every text file in the settings folder.
    let mut files = Vec::new();
    walk(&ctx.root, &mut files);
    let mut scanned = 0usize;
    for path in files {
        if path.file_name().map(|n| n == "auth.json").unwrap_or(false) {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(meta) = fs::metadata(&path) {
                    let mode = meta.permissions().mode() & 0o777;
                    out.push_str(&format!("ok   auth file {} has mode {:o}{}\n", crate::files::show(&path), mode,
                                          if mode & 0o077 != 0 { " (WARN: other users can read it)" } else { "" }));
                    if mode & 0o077 != 0 { findings += 1; }
                }
            }
            continue;
        }
        if path.file_name().map(|n| n == "config.json").unwrap_or(false) {
            // config.json holds variable names and is checked field by field above; masking "OPENCODE_API_KEY"
            // would be a false alarm.
            continue;
        }
        let Ok(meta) = fs::metadata(&path) else { continue };
        if meta.len() > 2_000_000 || path.extension().and_then(|e| e.to_str()).map(|e| matches!(e, "log" | "png" | "jpg" | "svg" | "pdf")).unwrap_or(false) {
            continue;
        }
        let Ok(bytes) = fs::read(&path) else { continue };
        if bytes.contains(&0) { continue; }
        let text = String::from_utf8_lossy(&bytes);
        let (_, hits) = crate::privacy::mask(&text);
        scanned += 1;
        if hits > 0 {
            out.push_str(&format!("WARN {} holds {} item(s) that look like a key or a secret. The privacy filter would mask them; the file itself is not masked.\n", crate::files::show(&path), hits));
            findings += 1;
        }
    }
    out.insert_str(0, &format!("Leak audit of {}\n\n", crate::files::show(&ctx.root)));
    out.push_str(&format!("\n{} text files scanned, {} findings.\n", scanned, findings));
    out.push_str("Every HTTP client uses redirects=0, so a key header cannot follow a Location to another host. The model calls have\n\
a timeout; the ScreenCodec and MCP clients start one process per call and stop it. Penelope listens on no port itself:\n\
the MCP server speaks stdin/stdout, background jobs write files, and a local model server is reached as a client.\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn a_key_in_a_file_and_a_plain_http_endpoint_are_found() {
        let dir = std::env::temp_dir().join(format!("penelope-audit-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let mut ctx = crate::config::load(&dir.join(".penelope"));
        fs::create_dir_all(ctx.root.join("sessions")).unwrap();
        fs::write(ctx.root.join("sessions/leak.txt"), "token sk-abcdefghijklmnopqrstuvwx\n").unwrap();
        ctx.cfg["main"] = serde_json::json!({"kind": "openai", "base_url": "http://api.example.org/v1", "model": "x", "api_key_env": "X_KEY"});
        ctx.cfg["api_key"] = serde_json::json!("pasted");
        let text = run(&ctx);
        // The report names the file by its full path, with the separator of the system.
        assert!(text.contains(&format!("sessions{}leak.txt holds 1 item", std::path::MAIN_SEPARATOR)), "{}", text);
        assert!(text.contains("plain http") && text.contains("holds a value in \"api_key\""), "{}", text);
        assert!(text.contains("findings") && !text.contains("sk-abcdef"), "{}", text);
        let _ = fs::remove_dir_all(&dir);
    }
}
