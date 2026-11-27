//! Mods: a folder under `.penelope/mods/NAME/` with a manifest that changes the theme, the picture, the layout and
//! some keys of the interface. One mod is active at a time. The registry `.penelope/mods.json` keeps its name, so the
//! next start comes back with the same look. `/mod off` restores the theme, picture, layout and keys from before.
//!
//! A manifest is `.penelope-mod.json` or `mod.json`:
//! `{"name": "paper", "version": "1.0.0", "description": "...", "theme": {"ink": "#15131C"},
//!   "picture": "picture.png", "layout": "side|top|compact", "keys": {"sidebar": "ctrl+b"}, "startup": "..."}`
//! The interface reads the settings through `Mod`; the picture is loaded by `tui::picture` when the mod goes on.

use crate::config::Ctx;
use crate::theme::Rgb;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

pub const MANIFEST: &str = ".penelope-mod.json";
pub const ALT_MANIFEST: &str = "mod.json";
const MAX_MANIFEST: u64 = 512 * 1024;

/// The actions of the interface a mod may put on another key.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Action {
    Sidebar,
    Palette,
    Providers,
    NewSession,
    Sessions,
    Help,
    TabPrev,
    TabNext,
    CopyLast,
}

impl Action {
    pub const ALL: [Action; 9] = [Action::Sidebar, Action::Palette, Action::Providers, Action::NewSession,
                                  Action::Sessions, Action::Help, Action::TabPrev, Action::TabNext, Action::CopyLast];

    pub fn name(self) -> &'static str {
        match self {
            Action::Sidebar => "sidebar",
            Action::Palette => "palette",
            Action::Providers => "providers",
            Action::NewSession => "new_session",
            Action::Sessions => "sessions",
            Action::Help => "help",
            Action::TabPrev => "tab_prev",
            Action::TabNext => "tab_next",
            Action::CopyLast => "copy_last",
        }
    }

    pub fn parse(name: &str) -> Option<Action> {
        Action::ALL.into_iter().find(|a| a.name() == name.trim().to_lowercase())
    }

    pub fn defaults(self) -> &'static [&'static str] {
        match self {
            Action::Sidebar => &["ctrl+b"],
            Action::Palette => &["ctrl+p"],
            Action::Providers => &["ctrl+o"],
            Action::NewSession => &["ctrl+n"],
            Action::Sessions => &["f2"],
            Action::Help => &["f1"],
            Action::TabPrev => &["f3", "alt+left"],
            Action::TabNext => &["f4", "alt+right"],
            Action::CopyLast => &[],
        }
    }
}

/// The safe subset of a layout: the side panel on the right, no side panel, or a narrower side panel.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Layout {
    #[default]
    Side,
    Top,
    Compact,
}

impl Layout {
    pub fn name(self) -> &'static str {
        match self { Layout::Side => "side", Layout::Top => "top", Layout::Compact => "compact" }
    }

    pub fn parse(name: &str) -> Option<Layout> {
        match name.trim().to_ascii_lowercase().as_str() {
            "side" => Some(Layout::Side),
            "top" => Some(Layout::Top),
            "compact" => Some(Layout::Compact),
            _ => None,
        }
    }

    pub fn shows_sidebar(self) -> bool {
        self != Layout::Top
    }

    pub fn sidebar_width(self) -> u16 {
        match self { Layout::Side => 36, Layout::Compact => 28, Layout::Top => 0 }
    }
}

/// One manifest, ready to be applied. A missing picture is a warning, not a broken mod: the default picture stays.
#[derive(Clone, Debug)]
pub struct Mod {
    pub folder: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub theme: Vec<(String, Rgb)>,
    /// The resolved path, even when the file is missing.
    pub picture: Option<PathBuf>,
    pub picture_label: String,
    pub layout: Layout,
    pub keys: Vec<(Action, String)>,
    pub startup: String,
    pub warnings: Vec<String>,
}

/// The key bindings of the interface. Defaults come from `Action::defaults`; a mod may replace one action with one key.
#[derive(Clone, Debug, PartialEq)]
pub struct Keys {
    bindings: Vec<(Action, Vec<String>)>,
    custom: Vec<(Action, String)>,
}

impl Default for Keys {
    fn default() -> Keys {
        Keys {
            bindings: Action::ALL.into_iter().map(|a| (a, a.defaults().iter().map(|k| k.to_string()).collect())).collect(),
            custom: Vec::new(),
        }
    }
}

impl Keys {
    /// The default keys with the bindings of a mod laid over them. A collision is refused with both action names.
    pub fn with(&self, overrides: &[(Action, String)]) -> Result<Keys, String> {
        let mut bindings = self.bindings.clone();
        for (action, key) in overrides {
            if let Some(slot) = bindings.iter_mut().find(|(a, _)| a == action) {
                slot.1 = vec![key.clone()];
            }
        }
        let mut seen: Vec<(String, Action)> = Vec::new();
        for (action, keys) in &bindings {
            for key in keys {
                if let Some((_, other)) = seen.iter().find(|(k, _)| k == key) {
                    return Err(format!("the key {} is bound to {} and {}", key, other.name(), action.name()));
                }
                seen.push((key.clone(), *action));
            }
        }
        Ok(Keys { bindings, custom: overrides.to_vec() })
    }

    pub fn action_for(&self, key: &str) -> Option<Action> {
        self.bindings.iter().find(|(_, keys)| keys.iter().any(|k| k == key)).map(|(a, _)| *a)
    }

    /// A default key of an action the mod moved: it must do nothing instead of its old job.
    pub fn is_displaced_default(&self, key: &str) -> bool {
        self.custom.iter().any(|(action, _)| action.defaults().contains(&key) && self.action_for(key).is_none())
    }

    /// The first key of one action, for tests and the mod report.
    #[cfg(test)]
    pub fn binding(&self, action: Action) -> String {
        self.bindings.iter().find(|(a, _)| *a == action).and_then(|(_, keys)| keys.first().cloned()).unwrap_or_default()
    }
}

pub fn mods_dir(ctx: &Ctx) -> PathBuf {
    ctx.root.join("mods")
}

pub fn registry(ctx: &Ctx) -> PathBuf {
    ctx.root.join("mods.json")
}

fn read_registry(ctx: &Ctx) -> Value {
    fs::read_to_string(registry(ctx)).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_else(|| json!({"active": null}))
}

/// The folder name of the active mod.
pub fn active(ctx: &Ctx) -> Option<String> {
    read_registry(ctx)["active"].as_str().map(str::to_string).filter(|s| !s.is_empty())
}

/// Remembers the active mod, or forgets it with None.
pub fn set_active(ctx: &Ctx, name: Option<&str>) -> Result<(), String> {
    let mut value = read_registry(ctx);
    if !value.is_object() {
        value = json!({});
    }
    value["active"] = match name {
        Some(name) => json!(name),
        None => Value::Null,
    };
    if let Some(obj) = value.as_object_mut() {
        obj.entry("version").or_insert(json!(1));
    }
    fs::create_dir_all(ctx.root.as_path()).map_err(|e| format!("the mods registry cannot be written: {}", e))?;
    fs::write(registry(ctx), serde_json::to_string_pretty(&value).unwrap())
        .map_err(|e| format!("the mods registry cannot be written: {}", e))
}

/// True for a folder name that stays inside `.penelope/mods`.
fn valid_name(name: &str) -> bool {
    let name = name.trim();
    !name.is_empty() && name.len() <= 64 && !name.starts_with('.') && name != "." && name != ".."
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// Opens one mod and checks all of it. Nothing is applied here: a broken manifest must leave the old look untouched.
pub fn open(ctx: &Ctx, folder: &str) -> Result<Mod, String> {
    let folder = folder.trim();
    if !valid_name(folder) {
        return Err(format!("\"{}\" is not a mod name (letters, digits, - _ and . only)", folder));
    }
    let dir = mods_dir(ctx).join(folder);
    if !dir.is_dir() {
        return Err(format!("there is no mod folder at {}", crate::files::show(&dir)));
    }
    let file = [MANIFEST, ALT_MANIFEST].iter().map(|name| dir.join(name)).find(|p| p.is_file())
        .ok_or_else(|| format!("no {} or {} in {}", MANIFEST, ALT_MANIFEST, crate::files::show(&dir)))?;
    let size = fs::metadata(&file).map(|m| m.len()).unwrap_or(0);
    if size > MAX_MANIFEST {
        return Err(format!("{} is larger than 512 KB", crate::files::show(&file)));
    }
    let text = fs::read_to_string(&file).map_err(|e| format!("{} cannot be read: {}", crate::files::show(&file), e))?;
    parse(folder, &dir, &text)
}

/// Reads a manifest. `dir` is the folder of the mod, for a relative picture path.
pub fn parse(folder: &str, dir: &Path, text: &str) -> Result<Mod, String> {
    let value: Value = serde_json::from_str(text).map_err(|e| format!("the manifest is not valid JSON: {}", e))?;
    let object = value.as_object().ok_or("the manifest must be a JSON object")?;
    let text_field = |key: &str| -> Result<Option<String>, String> {
        match object.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(s)) => Ok(Some(s.trim().to_string())),
            Some(_) => Err(format!("{} must be text", key)),
        }
    };
    let name = text_field("name")?.filter(|n| !n.is_empty()).ok_or("the manifest has no name")?;
    let version = text_field("version")?.unwrap_or_default();
    let description = text_field("description")?.unwrap_or_default();
    let startup = text_field("startup")?.unwrap_or_default();

    let mut theme = Vec::new();
    match object.get("theme") {
        None | Some(Value::Null) => {}
        Some(Value::Object(map)) => {
            for (key, value) in map {
                if !crate::theme::NAMES.contains(&key.as_str()) {
                    return Err(format!("theme colour \"{}\" is not one of {}", key, crate::theme::NAMES.join(", ")));
                }
                let raw = value.as_str().ok_or_else(|| format!("theme.{} must be a hex colour such as #E8825A", key))?;
                let rgb = parse_hex(raw).ok_or_else(|| format!("theme.{} = {} is not a hex colour such as #E8825A", key, raw))?;
                theme.push((key.clone(), rgb));
            }
        }
        Some(_) => return Err("theme must be an object of colour names".to_string()),
    }

    let layout = match text_field("layout")?.filter(|l| !l.is_empty()) {
        Some(layout) => Layout::parse(&layout).ok_or_else(|| format!("layout {} is not side, top or compact", layout))?,
        None => Layout::Side,
    };

    let mut keys = Vec::new();
    match object.get("keys") {
        None | Some(Value::Null) => {}
        Some(Value::Object(map)) => {
            for (action, value) in map {
                let action = Action::parse(action)
                    .ok_or_else(|| format!("key action \"{}\" is not one of {}", action, action_names()))?;
                let raw = value.as_str().ok_or_else(|| format!("keys.{} must be a key such as ctrl+b", action.name()))?;
                keys.push((action, normalize_key(raw)?));
            }
            Keys::default().with(&keys)?;
        }
        Some(_) => return Err("keys must be an object from action to key".to_string()),
    }

    let picture_label = text_field("picture")?.unwrap_or_default();
    let mut warnings = Vec::new();
    let picture = if picture_label.is_empty() {
        None
    } else {
        let extension = Path::new(&picture_label).extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        if !matches!(extension.as_str(), "png" | "jpg" | "jpeg") {
            return Err(format!("picture {} is not a .png, .jpg or .jpeg file", picture_label));
        }
        let path = if Path::new(&picture_label).is_absolute() { PathBuf::from(&picture_label) } else { dir.join(&picture_label) };
        if !path.is_file() {
            warnings.push(format!("the picture {} is missing; the default picture stays", picture_label));
        }
        Some(path)
    };

    Ok(Mod { folder: folder.to_string(), name, version, description, theme, picture, picture_label,
             layout, keys, startup, warnings })
}

fn action_names() -> String {
    Action::ALL.iter().map(|a| a.name()).collect::<Vec<_>>().join(", ")
}

/// `#RRGGBB` or `RRGGBB` to a colour.
pub fn parse_hex(raw: &str) -> Option<Rgb> {
    let text = raw.trim().trim_start_matches('#');
    if text.len() != 6 || !text.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&text[i..i + 2], 16).ok();
    Some((byte(0)?, byte(2)?, byte(4)?))
}

/// A key a mod may bind: ctrl+ or alt+ with a letter or digit, or F1 to F12. The fixed keys stay fixed.
pub fn normalize_key(raw: &str) -> Result<String, String> {
    let lower = raw.trim().to_ascii_lowercase().replace(' ', "");
    if lower.is_empty() {
        return Err("a key cannot be empty".to_string());
    }
    if matches!(lower.as_str(), "esc" | "ctrl+c" | "f5" | "f6") || lower.ends_with("+f5") || lower.ends_with("+f6") {
        return Err(format!("{} cannot be changed by a mod", lower));
    }
    let parts: Vec<&str> = lower.split('+').collect();
    let (base, mods) = parts.split_last().ok_or("a key cannot be empty")?;
    let mut ctrl = false;
    let mut alt = false;
    for modifier in mods {
        match *modifier {
            "ctrl" if !ctrl => ctrl = true,
            "alt" if !alt => alt = true,
            "ctrl" | "alt" => return Err(format!("{} names a modifier twice", lower)),
            other => return Err(format!("{} uses {}, but only ctrl+ and alt+ are allowed", lower, other)),
        }
    }
    let function = base.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()).filter(|n| (1..=12).contains(n)).is_some();
    let plain = base.len() == 1 && base.chars().all(|c| c.is_ascii_alphanumeric());
    if !function && !plain {
        return Err(format!("{} is not a letter, a digit or f1 to f12", base));
    }
    if mods.is_empty() && !function {
        return Err(format!("{} is a plain key and would be typed into the input; add ctrl+ or alt+", base));
    }
    let mut out = String::new();
    if ctrl { out.push_str("ctrl+"); }
    if alt { out.push_str("alt+"); }
    out.push_str(base);
    Ok(out)
}

/// One row of `/mod list`.
#[derive(Clone, Debug)]
pub struct Listing {
    pub folder: String,
    pub module: Option<Mod>,
    pub error: Option<String>,
}

/// Every folder under `.penelope/mods`, with the manifest read or the reason it was refused.
pub fn list(ctx: &Ctx) -> Vec<Listing> {
    let dir = mods_dir(ctx);
    let mut out = Vec::new();
    let Ok(read) = fs::read_dir(&dir) else { return out };
    let mut entries: Vec<_> = read.flatten().filter(|e| e.path().is_dir()).collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let folder = entry.file_name().to_string_lossy().to_string();
        match open(ctx, &folder) {
            Ok(module) => out.push(Listing { folder, module: Some(module), error: None }),
            Err(error) => out.push(Listing { folder, module: None, error: Some(error) }),
        }
    }
    out
}

/// The text behind `/mod list`.
pub fn report(ctx: &Ctx) -> String {
    let mut out = format!("Mods under {}\n\n", crate::files::show(&mods_dir(ctx)));
    let active = active(ctx);
    let list = list(ctx);
    if list.is_empty() {
        out.push_str("No mod yet. /mod new \"what it should change\" lets the model write one.\n");
    }
    for entry in &list {
        let is_active = active.as_deref() == Some(entry.folder.as_str());
        match &entry.module {
            Some(m) => {
                out.push_str(&format!("{} {} · {} · {}{}\n", if is_active { "●" } else { "○" }, entry.folder, m.name,
                                      if m.version.is_empty() { "no version".to_string() } else { format!("version {}", m.version) },
                                      if is_active { " · active" } else { "" }));
                if !m.description.is_empty() {
                    out.push_str(&format!("    {}\n", m.description));
                }
                out.push_str(&format!("    {} · {} colour{} · picture {} · {} key{}\n", m.layout.name(), m.theme.len(),
                                      if m.theme.len() == 1 { "" } else { "s" },
                                      if m.picture_label.is_empty() { "default".to_string() } else { m.picture_label.clone() },
                                      m.keys.len(), if m.keys.len() == 1 { "" } else { "s" }));
                for warning in &m.warnings {
                    out.push_str(&format!("    ! {}\n", warning));
                }
            }
            None => out.push_str(&format!("✖ {} · refused: {}\n", entry.folder, entry.error.as_deref().unwrap_or("unknown error"))),
        }
    }
    out.push_str("\n/mod on NAME applies one mod; only one is active at a time.\n/mod off restores the theme, picture, layout and keys from before.\n");
    out
}

/// Deletes the folder of a mod. The command turns an active mod off first.
pub fn remove(ctx: &Ctx, folder: &str) -> Result<String, String> {
    let folder = folder.trim();
    if !valid_name(folder) {
        return Err(format!("\"{}\" is not a mod name", folder));
    }
    let dir = mods_dir(ctx).join(folder);
    if !dir.is_dir() {
        return Err(format!("there is no mod named {}", folder));
    }
    fs::remove_dir_all(&dir).map_err(|e| format!("{} cannot be removed: {}", crate::files::show(&dir), e))?;
    if active(ctx).as_deref() == Some(folder) {
        set_active(ctx, None)?;
    }
    Ok(format!("mod {} removed; {} is gone", folder, crate::files::show(&dir)))
}

fn suggest_name(request: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for c in request.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            out.push(c);
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
        if out.len() >= 32 {
            break;
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() { "custom".to_string() } else { out }
}

fn available_name(ctx: &Ctx, base: &str) -> String {
    let dir = mods_dir(ctx);
    if !dir.join(base).exists() {
        return base.to_string();
    }
    for n in 2..100 {
        let name = format!("{}-{}", base, n);
        if !dir.join(&name).exists() {
            return name;
        }
    }
    format!("{}-{}", base, std::process::id())
}

/// The prompt for `/mod new`: the model writes the manifest and the picture with its normal tools, no special crate.
/// Returns the folder name and the text of the turn.
pub fn new_request(ctx: &Ctx, request: &str) -> (String, String) {
    let request = request.trim();
    let name = available_name(ctx, &suggest_name(request));
    let dir = mods_dir(ctx).join(&name);
    let _ = fs::create_dir_all(&dir);
    let colors = crate::theme::NAMES.join(", ");
    let actions = action_names();
    let prompt = format!(
        "Write a Penelope mod for this request: \"{request}\"\n\n\
A mod is one folder with a manifest. Create this folder: {dir}\n\
Write the manifest as {manifest} (or {alt}) with the normal write tool. Use run only to copy a picture.\n\n\
Fields of the manifest:\n\
- name: \"{name}\" (required, text)\n\
- version: \"1.0.0\"\n\
- description: one short line\n\
- theme: optional object; the only keys are {colors}; each value is #RRGGBB\n\
- picture: optional path inside the mod folder to a .png, .jpg or .jpeg; copy the picture the person named into the folder first\n\
- layout: \"side\", \"top\" or \"compact\" (side keeps the right panel, top hides it, compact narrows it)\n\
- keys: optional object from action to key; the actions are {actions}; keys look like ctrl+b, alt+1 or f7. esc, ctrl+c, f5 and f6 cannot be changed\n\
- startup: optional line Penelope shows when the mod goes on\n\n\
Rules: do not edit config.json; keep the manifest valid JSON; every colour and action name must be from those lists; no key may be bound twice. If the person named a picture, copy that file into {dir} and name it in picture. After writing, say which files you wrote. /mod on {name} applies the mod.",
        request = request, dir = dir.display(), manifest = MANIFEST, alt = ALT_MANIFEST, name = name,
        colors = colors, actions = actions);
    (name, prompt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static N: AtomicUsize = AtomicUsize::new(0);

    fn ctx() -> Ctx {
        let root = std::env::temp_dir().join(format!("penelope-mods-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        crate::config::load(&root)
    }

    fn write_mod(ctx: &Ctx, folder: &str, manifest: Value) {
        let dir = mods_dir(ctx).join(folder);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(MANIFEST), serde_json::to_string_pretty(&manifest).unwrap()).unwrap();
    }

    #[test]
    fn a_manifest_carries_theme_picture_layout_keys_and_startup() {
        let ctx = ctx();
        let dir = mods_dir(&ctx).join("warm");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("picture.png"), b"\x89PNG\r\n\x1a\nfake").unwrap();
        let manifest = json!({
            "name": "Warm Paper", "version": "1.2.0", "description": "a warm light look",
            "theme": {"ink": "#010203", "clay": "FF0000"},
            "picture": "picture.png", "layout": "compact",
            "keys": {"sidebar": "ctrl+y", "copy_last": "alt+1"}, "startup": "welcome"
        });
        write_mod(&ctx, "warm", manifest);
        let m = open(&ctx, "warm").unwrap();
        assert_eq!((m.folder.as_str(), m.name.as_str(), m.version.as_str()), ("warm", "Warm Paper", "1.2.0"));
        assert_eq!(m.layout, Layout::Compact);
        assert_eq!(m.theme, vec![("clay".to_string(), (255, 0, 0)), ("ink".to_string(), (1, 2, 3))]);
        assert_eq!(m.keys, vec![(Action::CopyLast, "alt+1".to_string()), (Action::Sidebar, "ctrl+y".to_string())]);
        assert_eq!(m.startup, "welcome");
        assert!(m.picture.as_ref().unwrap().is_file());
        let palette = crate::theme::Palette::DEFAULT.with(&m.theme).unwrap();
        assert_eq!((palette.ink, palette.clay), ((1, 2, 3), (255, 0, 0)));
    }

    #[test]
    fn a_broken_manifest_is_refused_with_the_line() {
        let ctx = ctx();
        for (folder, text, wanted) in [
            ("json", "{nope".to_string(), "not valid JSON"),
            ("name", json!({"version": "1"}).to_string(), "no name"),
            ("colour", json!({"name": "x", "theme": {"pink": "#FF00FF"}}).to_string(), "not one of"),
            ("hex", json!({"name": "x", "theme": {"ink": "#12"}}).to_string(), "not a hex colour"),
            ("layout", json!({"name": "x", "layout": "wide"}).to_string(), "not side, top or compact"),
            ("action", json!({"name": "x", "keys": {"launch": "ctrl+l"}}).to_string(), "key action"),
            ("picture", json!({"name": "x", "picture": "a.gif"}).to_string(), "not a .png"),
        ] {
            let dir = mods_dir(&ctx).join(folder);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join(MANIFEST), text).unwrap();
            let error = open(&ctx, folder).unwrap_err();
            assert!(error.contains(wanted), "{}: {}", folder, error);
        }
    }

    #[test]
    fn a_key_collision_and_the_fixed_keys_are_refused() {
        let ctx = ctx();
        let collision = json!({"name": "x", "keys": {"sidebar": "f1"}});
        write_mod(&ctx, "collision", collision);
        let error = open(&ctx, "collision").unwrap_err();
        assert!(error.contains("f1") && error.contains("help") && error.contains("sidebar"), "{}", error);

        let duplicate = json!({"name": "x", "keys": {"sidebar": "ctrl+x", "palette": "ctrl+x"}});
        write_mod(&ctx, "duplicate", duplicate);
        let error = open(&ctx, "duplicate").unwrap_err();
        assert!(error.contains("ctrl+x") && error.contains("sidebar") && error.contains("palette"), "{}", error);

        for fixed in ["esc", "ctrl+c", "f5", "f6"] {
            let manifest = json!({"name": "x", "keys": {"sidebar": fixed}});
            write_mod(&ctx, fixed.replace('+', "-").as_str(), manifest);
            let error = open(&ctx, &fixed.replace('+', "-")).unwrap_err();
            assert!(error.contains("cannot be changed"), "{}: {}", fixed, error);
        }
    }

    #[test]
    fn a_missing_picture_is_a_warning_and_the_mod_still_loads() {
        let ctx = ctx();
        let manifest = json!({"name": "x", "picture": "missing.png", "theme": {"sea": "#00FF00"}});
        write_mod(&ctx, "nopicture", manifest);
        let m = open(&ctx, "nopicture").unwrap();
        assert_eq!(m.theme, vec![("sea".to_string(), (0, 255, 0))]);
        assert!(m.picture.as_ref().unwrap().ends_with("missing.png"));
        assert!(m.warnings.iter().any(|w| w.contains("missing") && w.contains("default picture")), "{:?}", m.warnings);
    }

    #[test]
    fn a_model_written_folder_has_a_prompt_and_can_be_removed() {
        let ctx = ctx();
        let (name, prompt) = new_request(&ctx, "use this picture as background");
        assert!(name.starts_with("use-this-picture"), "{}", name);
        assert!(prompt.contains(".penelope-mod.json") && prompt.contains(&name) && prompt.contains("#RRGGBB"), "{}", prompt);
        let dir = mods_dir(&ctx).join(&name);
        fs::write(dir.join("picture.png"), b"\x89PNG\r\n\x1a\nfake").unwrap();
        fs::write(dir.join(MANIFEST), json!({"name": name, "picture": "picture.png", "layout": "top"}).to_string()).unwrap();
        let m = open(&ctx, &name).unwrap();
        assert_eq!(m.layout, Layout::Top);
        assert!(remove(&ctx, &name).unwrap().contains("removed"));
        assert!(!dir.exists() && open(&ctx, &name).is_err());
    }

    #[test]
    fn keys_move_defaults_and_refuse_another_actions_key() {
        let keys = Keys::default();
        assert_eq!(keys.action_for("ctrl+b"), Some(Action::Sidebar));
        assert_eq!(keys.action_for("alt+left"), Some(Action::TabPrev));
        let moved = keys.with(&[(Action::Sidebar, "ctrl+y".to_string())]).unwrap();
        assert_eq!(moved.action_for("ctrl+y"), Some(Action::Sidebar));
        assert_eq!(moved.action_for("ctrl+b"), None);
        assert!(moved.is_displaced_default("ctrl+b") && !moved.is_displaced_default("ctrl+p"));
        assert_eq!(moved.binding(Action::Sidebar), "ctrl+y");
        assert!(keys.with(&[(Action::Sidebar, "f1".to_string())]).is_err());
        assert!(normalize_key("ctrl+shift+b").is_err());
        assert!(normalize_key("a").is_err());
        assert_eq!(normalize_key("CTRL+B").unwrap(), "ctrl+b");
        assert_eq!(normalize_key("#") , Err("# is not a letter, a digit or f1 to f12".to_string()));
    }

    #[test]
    fn the_registry_remembers_one_name() {
        let ctx = ctx();
        assert_eq!(active(&ctx), None);
        set_active(&ctx, Some("warm")).unwrap();
        assert_eq!(active(&ctx), Some("warm".to_string()));
        assert!(registry(&ctx).is_file());
        set_active(&ctx, None).unwrap();
        assert_eq!(active(&ctx), None);
    }
}
