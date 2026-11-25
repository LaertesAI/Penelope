//! Skills that Penelope writes for herself.
//!
//! A learned skill is a Markdown file under `.penelope/skills/NAME/SKILL.md` with the same front matter a Claude Code
//! skill uses, so it can be copied into another harness and read there. `from_trace` turns a long, successful turn into
//! a short playbook of the tool calls that worked; `skill_new` lets a model write one on purpose.
//!
//! The playbook holds tool names and arguments, never tool output, so a secret from a command cannot land in a skill.

use crate::config::Ctx;
use std::fs;
use std::path::PathBuf;

pub fn dir(ctx: &Ctx) -> PathBuf {
    ctx.root.join("skills")
}

pub fn valid_name(name: &str) -> bool {
    (2..=48).contains(&name.len()) && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
        && name.chars().next().map(|c| c.is_ascii_lowercase()).unwrap_or(false)
}

fn file(ctx: &Ctx, name: &str) -> PathBuf {
    dir(ctx).join(name).join("SKILL.md")
}

fn front(text: &str, key: &str) -> Option<String> {
    let mut inside = false;
    for line in text.lines() {
        if line.trim() == "---" {
            if inside { break; }
            inside = true;
            continue;
        }
        if inside {
            if let Some(v) = line.strip_prefix(&format!("{}:", key)) {
                return Some(v.trim().trim_matches('"').to_string());
            }
        }
    }
    None
}

/// The skills of this project, as `(name, description)`.
pub fn list(ctx: &Ctx) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(dir(ctx)) else { return out };
    for e in entries.flatten() {
        let path = e.path().join("SKILL.md");
        let Ok(text) = fs::read_to_string(&path) else { continue };
        let name = front(&text, "name").unwrap_or_else(|| e.file_name().to_string_lossy().to_string());
        out.push((name, front(&text, "description").unwrap_or_default()));
    }
    out.sort();
    out
}

pub fn body(ctx: &Ctx, name: &str) -> Option<String> {
    let path = file(ctx, name);
    fs::read_to_string(path).ok()
}

/// Writes one skill. The name is lower case; the description is one line; the body is the instructions.
pub fn write(ctx: &Ctx, name: &str, description: &str, body: &str) -> Result<String, String> {
    let name = name.trim().to_lowercase();
    if !valid_name(&name) {
        return Err("NOT written. The skill name is 2 to 48 lower case letters, digits, - or _.".to_string());
    }
    let description = description.split_whitespace().collect::<Vec<_>>().join(" ");
    if description.len() < 10 || description.len() > 240 {
        return Err("NOT written. The description is one line of 10 to 240 characters.".to_string());
    }
    if body.trim().len() < 20 || body.len() > 20_000 {
        return Err("NOT written. The body is 20 to 20,000 characters.".to_string());
    }
    let description = crate::tools::guard(ctx, &description);
    let body = crate::tools::guard(ctx, body);
    let path = file(ctx, &name);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("NOT written. {}", e))?;
    }
    let text = format!("---\nname: {}\ndescription: {}\nsource: penelope\n---\n\n{}", name, description, body.trim());
    fs::write(&path, text).map_err(|e| format!("NOT written. {}", e))?;
    Ok(format!("skill {} written: {}", name, crate::files::show(&path)))
}

fn slug(text: &str) -> String {
    const STOP: &[&str] = &["the", "and", "for", "with", "that", "this", "from", "into", "please", "report", "make", "eine", "einen", "und", "mit"];
    let mut out = String::new();
    for word in text.to_lowercase().split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| w.len() > 2 && !STOP.contains(w)).take(4) {
        if !out.is_empty() {
            out.push('-');
        }
        out.push_str(word);
    }
    out.chars().take(40).collect::<String>().trim_matches('-').to_string()
}

/// After a successful turn with at least 5 tool calls: one playbook skill with the calls that worked. No model, no
/// output, nothing private. A skill of the same name is left alone and the turn adds nothing.
pub fn from_trace(ctx: &Ctx, request: &str, trace: &[crate::tools::Call]) -> Option<String> {
    let request = crate::tools::guard(ctx, request);
    if !ctx.cfg["auto_skills"].as_bool().unwrap_or(true) || trace.len() < 5 {
        return None;
    }
    if trace.iter().any(crate::tools::in_trouble) {
        return None;
    }
    let name = slug(&request);
    if name.len() < 3 || file(ctx, &name).exists() {
        return None;
    }
    let description: String = format!("Learned playbook: {}", request.split_whitespace().collect::<Vec<_>>().join(" ")).chars().take(200).collect();
    let mut body = format!("# {}\n\nThis playbook was written after a turn that succeeded with {} tool calls. Use it when a similar\nrequest comes. The arguments are the ones that worked; check them against the project before you reuse them.\n\n## Steps\n\n",
                            request.split_whitespace().collect::<Vec<_>>().join(" "), trace.len());
    for (i, (tool, detail, _, _)) in trace.iter().enumerate() {
        body.push_str(&format!("{}. `{}` — {}\n", i + 1, tool, detail.chars().take(200).collect::<String>()));
    }
    body.push_str("\n## After the steps\n\nHand the claim and the evidence to `check`. If a step failed, do not copy it; write a new lesson instead.\n");
    write(ctx, &name, &description, &body).ok().map(|_| name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_successful_turn_becomes_a_skill_without_output() {
        let dir = std::env::temp_dir().join(format!("penelope-skills-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let ctx = crate::config::load(&dir.join(".penelope"));
        let trace: Vec<crate::tools::Call> = (1..=6).map(|i| ("run".to_string(), format!("cargo test --test {}", i), true, "secret output".to_string())).collect();
        let name = from_trace(&ctx, "Run the six test files and report failures", &trace).unwrap();
        assert_eq!(name, "run-six-test-files");
        let text = body(&ctx, &name).unwrap();
        assert!(text.contains("description: Learned playbook") && text.contains("cargo test --test 6") && !text.contains("secret output"), "{}", text);
        assert!(!text.contains("\\n") && text.lines().next() == Some("---"), "the Markdown holds literal backslash-n: {}", text);
        assert_eq!(from_trace(&ctx, "Run the six test files and report failures", &trace), None);
        assert_eq!(list(&ctx).len(), 1);
        assert!(write(&ctx, "Bad Name", "A description that is long enough", "A body that is long enough for a skill.").unwrap_err().contains("lower case"));
        assert!(body(&ctx, "missing").is_none());
        let _ = fs::remove_dir_all(&dir);
    }
}
