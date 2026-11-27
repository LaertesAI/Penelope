//! Worktrees for a worker: one branch per task, a diff to inspect, and an explicit apply.
//!
//! This is the safe half of the worker plan: the live tree stays untouched until the person applies the diff. A
//! model-driven worker and a judge gate can build on these commands without changing the safety rule.

use crate::config::Ctx;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git").arg("-C").arg(dir).args(args).output()
        .map_err(|e| format!("git could not start: {}", e))?;
    if !out.status.success() {
        return Err(format!("git {} said: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim().chars().take(300).collect::<String>()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

pub fn project(cwd: &str) -> Result<PathBuf, String> {
    let start = if cwd.trim().is_empty() { std::env::current_dir().map_err(|e| e.to_string())? } else { PathBuf::from(cwd) };
    let out = git(&start, &["rev-parse", "--show-toplevel"])?;
    let root = PathBuf::from(out.trim());
    if !root.is_dir() { return Err(format!("{} is no git worktree", root.display())); }
    Ok(root)
}

fn safe_name(name: &str) -> String {
    name.trim().chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).collect()
}

fn worktrees(project: &Path) -> PathBuf {
    project.join(".penelope").join("worktrees")
}

fn path_of(project: &Path, name: &str) -> Result<PathBuf, String> {
    let name = safe_name(name);
    if name.is_empty() { return Err("give a worktree a name".to_string()); }
    let path = worktrees(project).join(&name);
    if !path.is_dir() { return Err(format!("no worktree named {}. penelope work list shows them.", name)); }
    Ok(path)
}

/// Makes a worktree and its branch under the project. The caller keeps the branch name for the merge and the cleanup.
pub fn create(project: &Path, name: &str) -> Result<(PathBuf, String), String> {
    let name = safe_name(name);
    if name.is_empty() { return Err("give a worktree a name".to_string()); }
    let path = worktrees(project).join(&name);
    if path.exists() { return Err(format!("{} already exists", path.display())); }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("cannot make {}: {}", parent.display(), e))?;
    }
    let branch = format!("agent/{}", name);
    git(project, &["worktree", "add", "-b", &branch, &path.to_string_lossy()])?;
    Ok((path, branch))
}

pub fn new(ctx: &Ctx, cwd: &str, name: &str) -> String {
    let _ = ctx;
    let project = match project(cwd) { Ok(p) => p, Err(e) => return format!("NOT created. {}", e) };
    match create(&project, name) {
        Ok((path, branch)) => format!("worktree {} on branch {} at {}", safe_name(name), branch, path.display()),
        Err(e) => format!("NOT created. {}", e),
    }
}

pub fn list(ctx: &Ctx, cwd: &str) -> String {
    let _ = ctx;
    let project = match project(cwd) { Ok(p) => p, Err(e) => return e };
    let out = match git(&project, &["worktree", "list", "--porcelain"]) { Ok(o) => o, Err(e) => return e };
    let base = worktrees(&project);
    let mut rows = Vec::new();
    for block in out.split("\n\n") {
        let path = block.lines().find_map(|l| l.strip_prefix("worktree ")).map(PathBuf::from);
        let branch = block.lines().find_map(|l| l.strip_prefix("branch ")).map(|b| b.trim_start_matches("refs/heads/").to_string());
        if let Some(path) = path {
            if path.starts_with(&base) {
                let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                let dirty = git(&path, &["status", "--porcelain"]).map(|s| s.lines().count()).unwrap_or(0);
                rows.push(format!("{:<16} {:<24} {:>4} changed  {}", name, branch.unwrap_or_default(), dirty, path.display()));
            }
        }
    }
    if rows.is_empty() { "no worktree. penelope work new NAME creates one.".to_string() }
    else { format!("{:<16} {:<24} {:>4}          {}\n{}\n", "name", "branch", "files", "path", rows.join("\n")) }
}

fn patch(path: &Path) -> Result<Vec<u8>, String> {
    // Stage everything in the worktree so new and deleted files appear in one patch. The live tree is not touched.
    git(path, &["add", "-A"])?;
    let out = Command::new("git").arg("-C").arg(path).args(["diff", "--cached", "--binary", "HEAD"]).output()
        .map_err(|e| format!("git could not start: {}", e))?;
    if !out.status.success() { return Err(format!("git diff said: {}", String::from_utf8_lossy(&out.stderr).trim())); }
    Ok(out.stdout)
}

pub fn diff(ctx: &Ctx, cwd: &str, name: &str, stat: bool) -> String {
    let _ = ctx;
    let project = match project(cwd) { Ok(p) => p, Err(e) => return e };
    let path = match path_of(&project, name) { Ok(p) => p, Err(e) => return e };
    if stat {
        return git(&project, &["diff", "--stat", "HEAD", "--", &path.to_string_lossy()]).unwrap_or_else(|_| {
            patch(&path).map(|p| format!("{} bytes of diff", p.len())).unwrap_or_else(|e| e)
        });
    }
    match patch_of(&project, name) {
        Ok(bytes) if bytes.is_empty() => "the worktree has no change".to_string(),
        Ok(bytes) => String::from_utf8_lossy(&bytes).to_string(),
        Err(e) => e,
    }
}

/// The patch of one worktree against HEAD, without touching the live tree.
pub fn patch_of(project: &Path, name: &str) -> Result<Vec<u8>, String> {
    let path = path_of(project, name)?;
    patch(&path)
}

/// Applies a patch to the live tree. A conflict is returned as an error and is never solved here.
pub fn apply_bytes(project: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut child = Command::new("git").arg("-C").arg(project).args(["apply", "--binary"]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()
        .map_err(|e| format!("git could not start: {}", e))?;
    use std::io::Write;
    if let Some(mut stdin) = child.stdin.take() {
        if stdin.write_all(bytes).is_err() { return Err("the diff could not be written to git apply".to_string()); }
    }
    match child.wait_with_output() {
        Ok(o) if o.status.success() => Ok(()),
        Ok(o) => Err(format!("git apply said: {}", String::from_utf8_lossy(&o.stderr).trim().chars().take(300).collect::<String>())),
        Err(e) => Err(e.to_string()),
    }
}

pub fn apply(ctx: &Ctx, cwd: &str, name: &str) -> String {
    let _ = ctx;
    let project = match project(cwd) { Ok(p) => p, Err(e) => return e };
    let bytes = match patch_of(&project, name) { Ok(b) => b, Err(e) => return e };
    if bytes.is_empty() { return "nothing to apply: the worktree has no change".to_string(); }
    match apply_bytes(&project, &bytes) {
        Ok(()) => format!("applied {} bytes from {} to {}. The worktree stays for inspection.", bytes.len(), name, project.display()),
        Err(e) => format!("NOT applied. {}", e),
    }
}

pub fn remove(ctx: &Ctx, cwd: &str, name: &str, force: bool) -> String {
    let _ = ctx;
    let project = match project(cwd) { Ok(p) => p, Err(e) => return e };
    let path = match path_of(&project, name) { Ok(p) => p, Err(e) => return e };
    let mut args = vec!["worktree", "remove"];
    if force { args.push("--force"); }
    let path_text = path.to_string_lossy().to_string();
    args.push(&path_text);
    match git(&project, &args) {
        Ok(_) => format!("worktree {} removed", name),
        Err(e) => format!("NOT removed. {}", e),
    }
}

/// Removes the worktree and the branch that belongs to it. A dirty worktree is discarded: this is an explicit kill.
pub fn remove_with_branch(project: &Path, name: &str) -> Result<String, String> {
    let name = safe_name(name);
    if name.is_empty() { return Err("give a worktree a name".to_string()); }
    let path = worktrees(project).join(&name);
    if path.exists() {
        git(project, &["worktree", "remove", "--force", &path.to_string_lossy()])?;
    } else {
        // A worktree removed outside git leaves an entry behind that blocks the branch deletion.
        let _ = git(project, &["worktree", "prune"]);
    }
    let branch = format!("agent/{}", name);
    match git(project, &["branch", "-D", &branch]) {
        Ok(_) => {}
        Err(e) if e.to_lowercase().contains("not found") => {}
        Err(e) => return Err(e),
    }
    Ok(format!("worktree {} and branch {} removed", name, branch))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> (std::path::PathBuf, crate::config::Ctx) {
        let dir = std::env::temp_dir().join(format!("penelope-work-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let run = |args: &[&str]| {
            let out = Command::new("git").current_dir(&dir).args(args).output().unwrap();
            assert!(out.status.success(), "git {:?}: {}", args, String::from_utf8_lossy(&out.stderr));
        };
        run(&["init", "-q"]);
        // The Windows runners set core.autocrlf=true globally; the test compares bytes, so this repository keeps \n.
        run(&["config", "core.autocrlf", "false"]);
        run(&["config", "user.email", "t@example.test"]);
        run(&["config", "user.name", "T"]);
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "first"]);
        let ctx = crate::config::load(&dir.join(".penelope"));
        (dir, ctx)
    }

    #[test]
    fn a_worktree_is_made_diffed_applied_and_removed() {
        let (dir, ctx) = repo();
        let cwd = dir.to_string_lossy().to_string();
        let said = new(&ctx, &cwd, "task");
        assert!(said.contains("worktree task"), "{}", said);
        let tree = dir.join(".penelope/worktrees/task");
        std::fs::write(tree.join("a.txt"), "one\ntwo\n").unwrap();
        assert!(list(&ctx, &cwd).contains("task") && list(&ctx, &cwd).contains("1 changed"), "{}", list(&ctx, &cwd));
        let patch = diff(&ctx, &cwd, "task", false);
        assert!(patch.contains("+two"), "{}", patch);
        assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "one\n");
        assert!(apply(&ctx, &cwd, "task").starts_with("applied"));
        assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "one\ntwo\n");
        assert!(remove(&ctx, &cwd, "task", true).contains("removed"));
        assert!(!tree.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
