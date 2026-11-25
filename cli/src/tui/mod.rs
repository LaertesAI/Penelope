//! The full screen interface of penelope: a transcript, an input box, a side panel with the loom and its mood.
//!
//! One thread draws and reads keys. A turn of the main model runs in a second thread and reports through a channel.

pub(crate) mod art;
pub(crate) mod picture;
mod text;
mod view;

use crate::backend::{Chat, Event as TurnEvent};
use crate::config::Ctx;
use crate::{mods, plugins, tools};
use art::Mood;
use ratatui::crossterm::event::{self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, Event, KeyCode,
                                KeyEvent, KeyEventKind, KeyModifiers, MouseEventKind};
use ratatui::crossterm::execute;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq)]
pub enum ToolState {
    Running,
    Done,
    Failed,
}

pub enum Item {
    User(String),
    Reply(String),
    Tool { id: String, name: String, detail: String, state: ToolState, summary: String },
    /// A status line of the harness: the way through the memory map, a stored note.
    Note { icon: &'static str, label: &'static str, text: String, colour: art::Rgb },
    Error(String),
}

/// The role models the extended tab offers. Web search and research share the scout role.
pub const ROLE_MODELS: &[(&str, &str)] = &[
    ("tool", "Tool calls"),
    ("condenser", "Processing"),
    ("scout", "Research + web search"),
    ("worker", "Coding"),
    ("judge", "Judge"),
    ("router", "Router"),
    ("filter", "Privacy filter"),
];

pub enum Overlay {
    None,
    Palette { query: String, sel: usize },
    /// Exact model names of one target: main, weak, scout or a role such as tool, condenser or worker.
    Models { sel: usize, target: String },
    /// Providers and their login state. Enter signs in or opens the key dialog.
    Providers { sel: usize, target: String, scroll: usize },
    /// The list of role models: tool calls, processing, research, coding, judge, router, filter.
    Roles { sel: usize },
    /// The masked API key dialog. The key is never shown in full, not even in the transcript.
    ApiKey { name: String, label: String, input: Vec<char>, cursor: usize, error: String, target: String },
    Pager { title: String, lines: Vec<String>, scroll: usize },
    /// A command outside the safe list or a change to a file. `diff` holds the changed lines of the file.
    Approve { id: String, command: String, cwd: String, file: bool, diff: Vec<String> },
    /// The list of stored sessions. Enter opens one.
    Sessions { sel: usize },
    /// A question of the main model with fixed answers.
    Question { id: String, question: String, options: Vec<String>, sel: usize },
    /// The memory map as a graph, with the vault command.
    Brain { scroll: usize },
    /// Registered plugins with commands, skills and MCP servers.
    Plugins { sel: usize, scroll: usize, filter: String, mode: u8 },
    /// The commands of one plugin.
    PluginCommands { plugin: String, sel: usize, scroll: usize },
}

pub struct Cmd {
    pub name: &'static str,
    pub help: &'static str,
    pub key: &'static str,
    /// True if the command needs text after its name.
    pub takes_text: bool,
}

pub const COMMANDS: &[Cmd] = &[
    Cmd { name: "/new", help: "start a new session", key: "ctrl+n", takes_text: false },
    Cmd { name: "/model", help: "choose the model of the main or the weaker setting", key: "ctrl+o", takes_text: false },
    Cmd { name: "/mode", help: "auto, manual, safe, review, privacy or power", key: "", takes_text: false },
    Cmd { name: "/sessions", help: "list the stored sessions and open one", key: "f2", takes_text: false },
    Cmd { name: "/setup", help: "answer the setup questions again", key: "", takes_text: false },
    Cmd { name: "/login", help: "sign in to Claude, or store the API key of a provider", key: "", takes_text: true },
    Cmd { name: "/logout", help: "remove the stored key of a provider", key: "", takes_text: true },
    Cmd { name: "/auth", help: "the login status of Claude and the providers", key: "", takes_text: false },
    Cmd { name: "/update", help: "check for a newer Penelope", key: "", takes_text: false },
    Cmd { name: "/audit", help: "look for key and plain-http leaks in .penelope", key: "", takes_text: false },
    Cmd { name: "/input", help: "native pointer helper: status, install, enable", key: "", takes_text: false },
    Cmd { name: "/providers", help: "connect a provider: login or API key", key: "", takes_text: false },
    Cmd { name: "/undo", help: "take back the last change to a file", key: "", takes_text: false },
    Cmd { name: "/jobs", help: "list the background jobs", key: "", takes_text: false },
    Cmd { name: "/scout", help: "who searches: auto, web, offer or off (no word: the next one)", key: "", takes_text: false },
    Cmd { name: "/crew", help: "the model families and what does each job today", key: "", takes_text: false },
    Cmd { name: "/map", help: "show the memory map", key: "", takes_text: false },
    Cmd { name: "/go", help: "follow the memory map to a topic", key: "", takes_text: true },
    Cmd { name: "/note", help: "store a rule or a fact in the memory map", key: "", takes_text: true },
    Cmd { name: "/unravel", help: "take back the last note of the memory map", key: "", takes_text: false },
    Cmd { name: "/stats", help: "how much tool output stayed away from the main model", key: "", takes_text: false },
    Cmd { name: "/btw", help: "keep a quick note without interrupting the turn", key: "", takes_text: true },
    Cmd { name: "/rules", help: "show the shared rules, or add one", key: "", takes_text: true },
    Cmd { name: "/orchestrate", help: "run an agent on a goal", key: "", takes_text: true },
    Cmd { name: "/agents", help: "the agents written with $name(...)", key: "", takes_text: false },
    Cmd { name: "/cluster", help: "orchestrate clusters: /cluster GOAL, profiles, status ID, kill ID", key: "", takes_text: true },
    Cmd { name: "/clusters", help: "the cluster tree with reports and memory", key: "", takes_text: false },
    Cmd { name: "/factory", help: "factories: new NAME --profile P --every 3h|--at HH:MM, list, install, run, stop", key: "", takes_text: true },
    Cmd { name: "/brain", help: "the memory map as a graph; /brain write makes an Obsidian vault", key: "", takes_text: false },
    Cmd { name: "/cost", help: "tokens and euro per role and model", key: "", takes_text: false },
    Cmd { name: "/context", help: "show what @ and the memory map would send", key: "", takes_text: true },
    Cmd { name: "/rewind", help: "restore files and transcript N turns back: /rewind 3", key: "", takes_text: true },
    Cmd { name: "/search", help: "search several engines and show the ranked results; /search fanout on|off", key: "", takes_text: true },
    Cmd { name: "/research", help: "search, read the best pages and show a cited report", key: "", takes_text: true },
    Cmd { name: "/route", help: "the roles, their tools and their models", key: "", takes_text: false },
    Cmd { name: "/tools", help: "every tool, its arguments and its roles", key: "", takes_text: false },
    Cmd { name: "/consolidate", help: "keep the lasting rules of this session for every project", key: "", takes_text: false },
    Cmd { name: "/forge", help: "write a tool from the notes at a memory waypoint", key: "", takes_text: true },
    Cmd { name: "/plugins", help: "the filterable plugin screen; install, import, allow, deny, update, remove", key: "", takes_text: false },
    Cmd { name: "/mcp", help: "MCP servers: list, probe, add, remove, allow, deny, call", key: "", takes_text: false },
    Cmd { name: "/screen", help: "ScreenCodec: tools, open URL, see, click ID, desktop", key: "", takes_text: false },
    Cmd { name: "/cursor", help: "move Penelope's cursor: /cursor X Y [click]", key: "", takes_text: false },
    Cmd { name: "/export", help: "write this session as Markdown under .penelope/exports", key: "", takes_text: false },
    Cmd { name: "/forget", help: "take back one note by its text", key: "", takes_text: true },
    Cmd { name: "/price", help: "set a price: /price MODEL INPUT OUTPUT in dollars per million", key: "", takes_text: true },
    Cmd { name: "/budget", help: "monthly euro budget: /budget 20, or /budget to show it", key: "", takes_text: true },
    Cmd { name: "/sandbox", help: "run shell commands in bubblewrap without network: /sandbox on|off", key: "", takes_text: true },
    Cmd { name: "/auto", help: "switch auto-learning on or off", key: "", takes_text: false },
    Cmd { name: "/learn", help: "the lessons and the feedback of this folder", key: "", takes_text: false },
    Cmd { name: "/skills", help: "the skills Penelope wrote for this project", key: "", takes_text: false },
    Cmd { name: "/mod", help: "mods: list, new TEXT, on NAME, off, remove NAME", key: "", takes_text: true },
    Cmd { name: "/doctor", help: "check the programs and models Penelope needs", key: "", takes_text: false },
    Cmd { name: "/copy", help: "copy the last answer to the clipboard", key: "", takes_text: false },
    Cmd { name: "/shot", help: "save the screen as a picture under .penelope/screens", key: "f12", takes_text: false },
    Cmd { name: "/sidebar", help: "show or hide the side panel", key: "ctrl+b", takes_text: false },
    Cmd { name: "/help", help: "keys and commands", key: "f1", takes_text: false },
    Cmd { name: "/quit", help: "leave Penelope", key: "ctrl+c", takes_text: false },
];

enum Msg {
    Trail(Vec<String>, bool),
    Turn(TurnEvent),
    /// Waypoints of the lessons the weaker model drew from the tool calls of a turn.
    Lessons(Vec<String>),
    Done(Box<Chat>, Result<String, String>),
    Learned(Vec<String>),
    /// The context references of a prompt, expanded before the turn.
    Context(Vec<crate::context::Ref>),
    AgentDone(Result<String, String>),
    /// A skill Penelope wrote from a long successful turn.
    Skill(String),
    /// The model list a provider answered for the model screen.
    Models { provider: String, result: Result<Vec<crate::providers::ModelInfo>, String> },
}

/// Numbers of the side panel that come from files under `.penelope`.
#[derive(Default)]
pub struct Side {
    pub waypoints: usize,
    pub walks: usize,
    /// Calls and the percentage of characters that stayed away from the main model, for `run` and `fetch`.
    pub routed: [(u64, u64); 2],
    pub screencodec: bool,
    pub plugins: usize,
    /// Clusters that are queued, working, validating or being killed.
    pub clusters: usize,
    /// Secrets the privacy filter masked in tool output.
    pub masked: u64,
    /// Tasks the scout ran on the weaker model, and the tokens it spent there.
    pub scout: (u64, u64),
    /// The plan of the running task: text and state of each step.
    pub plan: Vec<(String, String)>,
    /// The euro spent so far in this folder.
    pub cost: String,
    /// True when Penelope can move her own cursor on this PC.
    pub cursor: bool,
    /// The last Claude login status, one process call per refresh, not per frame.
    pub claude: String,
}

pub struct App {
    pub ctx: Ctx,
    pub items: Vec<Item>,
    pub input: Vec<char>,
    pub cursor: usize,
    history: Vec<String>,
    hist_at: Option<usize>,
    pub overlay: Overlay,
    pub slash_sel: usize,
    pub busy: Option<Instant>,
    pub learning: bool,
    /// True while the last transcript item is a reply that still grows.
    streaming: bool,
    /// Lines between the lower edge of the view and the end of the transcript.
    pub scroll: usize,
    pub page: usize,
    pub sidebar: bool,
    pub tick: u64,
    /// The colours of the interface. A mod lays its theme over the default palette.
    pub palette: art::Palette,
    /// The safe subset of a mod's layout: side, top (no side panel) or compact (a narrow side panel).
    pub layout: mods::Layout,
    /// The keys of the main actions. A mod may move one, never esc, ctrl+c, f5 or f6.
    pub keys: mods::Keys,
    /// The folder name of the active mod.
    pub mod_name: Option<String>,
    /// What to restore on `/mod off`: palette, layout, keys and the side panel as they were before the mod.
    mod_restore: Option<(art::Palette, mods::Layout, mods::Keys, bool)>,
    mod_startup: Option<String>,
    last_key: Instant,
    flash: Option<(Mood, Instant)>,
    /// A short message under the input box and the time it appeared.
    pub toast: Option<(String, Instant)>,
    chat: Option<Chat>,
    cancel: Arc<AtomicBool>,
    pid: Arc<AtomicU32>,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    pub tokens: (u64, u64),
    pub turns: u64,
    pub tool_calls: u64,
    pub trail: Vec<String>,
    pub learned: Vec<String>,
    pub side: Side,
    approve_dir: PathBuf,
    allow_all: bool,
    seen: HashSet<String>,
    shot: bool,
    quit: bool,
    /// The claude session this transcript belongs to. None before the first answer.
    pub session_id: Option<String>,
    /// When the interface opened, for the short startup weave.
    pub intro: Instant,
    /// The references of the last prompt that were expanded from @path.
    pub context_refs: Vec<String>,
    /// Stored sessions as tabs: id and title, oldest first.
    pub tabs: Vec<(String, String)>,
    setup: bool,
    /// A provider whose login runs on the plain terminal before the interface comes back.
    login: Option<String>,
    /// First F6 arms the delete; a second F6 within 4 seconds removes the session.
    delete_armed: Option<Instant>,
    /// Rotates the one-line tips that appear after every few turns.
    tip: usize,
    /// The plugins as rows for the plugins screen, refreshed with the side panel.
    pub plugin_rows: Vec<plugins::Entry>,
    /// First x arms the removal of a plugin; a second x removes it.
    plugin_armed: Option<String>,
    /// Model lists from the providers, by provider name. The static catalog is the fallback.
    pub model_cache: HashMap<String, Vec<crate::providers::ModelInfo>>,
    /// Where the list in the model screen came from: the provider, the cache, or the built-in catalog.
    pub model_source: String,
    /// Providers whose model list is being fetched right now.
    models_fetching: HashSet<String>,
}

/// Labels and icons of status lines, so that a stored transcript finds its fixed texts again.
const NOTE_KINDS: &[(&str, &str)] = &[("memory", "◆"), ("learned", "✦"), ("utis", "◆"), ("resumed", "↺"), ("unravel", "↺"), ("stopped", "■"), ("lesson", "✦"), ("scout", "◆"), ("note", "◆"), ("cluster", "◆")];

fn item_json(item: &Item) -> serde_json::Value {
    use serde_json::json;
    match item {
        Item::User(t) => json!({"k": "user", "t": t}),
        Item::Reply(t) => json!({"k": "reply", "t": t}),
        Item::Tool { name, detail, state, summary, .. } => json!({"k": "tool", "name": name, "detail": detail, "ok": *state != ToolState::Failed, "summary": summary}),
        Item::Note { label, text, colour, .. } => json!({"k": "note", "label": label, "t": text, "c": [colour.0, colour.1, colour.2]}),
        Item::Error(t) => json!({"k": "error", "t": t}),
    }
}

fn item_from(v: &serde_json::Value) -> Option<Item> {
    let t = || v["t"].as_str().unwrap_or("").to_string();
    Some(match v["k"].as_str()? {
        "user" => Item::User(t()),
        "reply" => Item::Reply(t()),
        "tool" => Item::Tool { id: String::new(), name: v["name"].as_str().unwrap_or("").to_string(), detail: v["detail"].as_str().unwrap_or("").to_string(),
                               state: if v["ok"].as_bool().unwrap_or(true) { ToolState::Done } else { ToolState::Failed }, summary: v["summary"].as_str().unwrap_or("").to_string() },
        "note" => {
            let (label, icon) = NOTE_KINDS.iter().find(|(l, _)| Some(*l) == v["label"].as_str()).copied().unwrap_or(("note", "◆"));
            let c = |i: usize| v["c"][i].as_u64().unwrap_or(180) as u8;
            Item::Note { icon, label, text: t(), colour: (c(0), c(1), c(2)) }
        }
        "error" => Item::Error(t()),
        _ => return None,
    })
}

fn save_config(ctx: &Ctx) {
    let _ = fs::write(ctx.root.join("config.json"), serde_json::to_string_pretty(&ctx.cfg).unwrap());
}

impl App {
    pub fn new(ctx: Ctx) -> App {
        let (tx, rx) = channel();
        let approve_dir = ctx.root.join("approvals");
        let _ = fs::remove_dir_all(&approve_dir);
        let _ = fs::create_dir_all(&approve_dir);
        let history = fs::read_to_string(ctx.root.join("history.jsonl")).unwrap_or_default().lines()
            .filter_map(|l| serde_json::from_str::<String>(l).ok()).collect::<Vec<_>>();
        let history = history[history.len().saturating_sub(200)..].to_vec();
        let mut app = App {
            ctx, items: Vec::new(), input: Vec::new(), cursor: 0, history, hist_at: None, overlay: Overlay::None, slash_sel: 0,
            busy: None, learning: false, streaming: false, scroll: 0, page: 10, sidebar: true, tick: 0, last_key: Instant::now(), flash: None, toast: None,
            palette: art::Palette::DEFAULT, layout: mods::Layout::Side, keys: mods::Keys::default(), mod_name: None, mod_restore: None, mod_startup: None,
            chat: Some(Chat::new()), cancel: Arc::new(AtomicBool::new(false)), pid: Arc::new(AtomicU32::new(0)), tx, rx,
            tokens: (0, 0), turns: 0, tool_calls: 0, trail: Vec::new(), learned: Vec::new(), side: Side::default(),
            approve_dir, allow_all: false, seen: HashSet::new(), shot: false, quit: false, session_id: None, tabs: Vec::new(), setup: false,
            intro: Instant::now(), context_refs: Vec::new(), login: None, delete_armed: None, tip: 0,
            plugin_rows: Vec::new(), plugin_armed: None, model_cache: HashMap::new(), model_source: String::new(), models_fetching: HashSet::new(),
        };
        app.refresh_tabs();
        app.refresh_side();
        app.load_saved_mod();
        app
    }

    pub fn refresh_tabs(&mut self) {
        self.tabs = crate::sessions::all(&self.ctx).iter().map(|e| (e["id"].as_str().unwrap_or("").to_string(), e["title"].as_str().unwrap_or("").to_string())).collect();
    }

    /// Writes the transcript of the running session to disk. Before the first answer there is no session to write to.
    fn save_session(&self) {
        if let Some(id) = &self.session_id {
            let items: Vec<serde_json::Value> = self.items.iter().map(item_json).collect();
            crate::sessions::save_transcript(&self.ctx, id, &items);
        }
    }

    /// Opens a stored session: its transcript comes back and the next request goes on in the same conversation.
    pub fn open_session(&mut self, entry: &serde_json::Value) {
        if self.busy.is_some() {
            self.toast("a turn is running. Stop it with esc before you change the session".to_string());
            return;
        }
        let Some(id) = entry["id"].as_str() else { return };
        self.save_session();
        let chat = Chat::resume(&self.ctx, entry);
        self.tokens = (chat.input_tokens, chat.output_tokens);
        self.turns = chat.turns;
        self.chat = Some(chat);
        self.items = crate::sessions::load_transcript(&self.ctx, id).iter().filter_map(item_from).collect();
        self.tool_calls = self.items.iter().filter(|i| matches!(i, Item::Tool { .. })).count() as u64;
        if self.items.is_empty() {
            self.note("↺", "resumed", entry["title"].as_str().unwrap_or("").to_string(), art::SEA);
        }
        self.session_id = Some(id.to_string());
        self.trail.clear();
        self.learned.clear();
        self.scroll = 0;
        let _ = fs::remove_file(self.ctx.root.join("plan.json"));
        self.refresh_side();
    }

    /// The tab left or right of the open one. A new session without an answer sits right of all tabs.
    fn step_tab(&mut self, by: i64) {
        if self.tabs.is_empty() {
            return;
        }
        let now = self.session_id.as_ref().and_then(|id| self.tabs.iter().position(|(t, _)| t == id)).map(|i| i as i64).unwrap_or(self.tabs.len() as i64);
        let next = (now + by).clamp(0, self.tabs.len() as i64 - 1) as usize;
        self.open_tab(next);
    }

    fn open_tab(&mut self, index: usize) {
        let Some((id, _)) = self.tabs.get(index).cloned() else { return };
        if self.session_id.as_deref() == Some(id.as_str()) {
            return;
        }
        if let Some(entry) = crate::sessions::all(&self.ctx).into_iter().find(|e| e["id"].as_str() == Some(id.as_str())) {
            self.open_session(&entry);
        }
    }

    pub fn refresh_side(&mut self) {
        self.side.waypoints = crate::brain::counts(&self.ctx).0;
        self.side.walks = fs::read_to_string(self.ctx.root.join("memory/paths.json")).ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|v| v.as_object().map(|o| o.len())).unwrap_or(0);
        let stats: serde_json::Value = fs::read_to_string(self.ctx.root.join("stats.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        for (i, tool) in ["run", "fetch"].iter().enumerate() {
            let (raw, sent) = (stats[tool]["raw_chars"].as_u64().unwrap_or(0), stats[tool]["sent_chars"].as_u64().unwrap_or(0));
            self.side.routed[i] = (stats[tool]["calls"].as_u64().unwrap_or(0), if raw > 0 { 100 - (sent * 100 / raw).min(100) } else { 0 });
        }
        self.side.masked = stats["privacy"]["masked"].as_u64().unwrap_or(0);
        self.side.scout = (stats["scout"]["tasks"].as_u64().unwrap_or(0), stats["scout"]["tokens_in"].as_u64().unwrap_or(0) + stats["scout"]["tokens_out"].as_u64().unwrap_or(0));
        let plan: serde_json::Value = fs::read_to_string(self.ctx.root.join("plan.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        self.side.plan = plan.as_array().map(|a| a.iter().map(|i| (i["text"].as_str().unwrap_or("").to_string(), i["state"].as_str().unwrap_or("").to_string())).collect()).unwrap_or_default();
        self.side.screencodec = self.ctx.cfg["screencodec"]["command"].as_str().map(|c| !c.is_empty()).unwrap_or(false)
            || crate::config::which("screencodec-mcp").is_some();
        self.plugin_rows = plugins::entries(&self.ctx);
        self.side.plugins = self.plugin_rows.len();
        self.side.clusters = crate::clusters::working(&self.ctx);
        self.side.cost = crate::cost::budget_summary(&self.ctx);
        self.side.cursor = crate::cursor::backend(&self.ctx).1;
        self.side.claude = crate::auth::claude_status();
    }

    /// Reads the mod that was active when Penelope last closed and brings it back. A manifest that broke in the
    /// meantime is refused once with a line; the next start opens with the default look.
    fn load_saved_mod(&mut self) {
        let Some(name) = mods::active(&self.ctx) else { return };
        match self.apply_mod(&name) {
            Ok((_, warnings)) => {
                if let Some(text) = self.mod_startup.take() {
                    self.note("◆", "mod", text, art::SEA);
                }
                if let Some(warning) = warnings.into_iter().next() {
                    self.toast(warning);
                }
            }
            Err(error) => {
                let _ = mods::set_active(&self.ctx, None);
                self.toast(format!("the saved mod {} was refused: {}", name, error));
            }
        }
    }

    /// Turns one mod on. Everything is checked before the first change, so a broken manifest leaves the old look
    /// intact. Only one mod is active: another one first restores the look from before the running mod.
    pub fn apply_mod(&mut self, name: &str) -> Result<(String, Vec<String>), String> {
        let module = mods::open(&self.ctx, name)?;
        let palette = art::Palette::DEFAULT.with(&module.theme)?;
        let keys = mods::Keys::default().with(&module.keys)?;
        mods::set_active(&self.ctx, Some(&module.folder))?;
        if self.mod_name.is_some() {
            self.restore_previous();
        }
        let previous = (self.palette, self.layout, self.keys.clone(), self.sidebar);
        self.palette = palette;
        self.layout = module.layout;
        self.keys = keys;
        self.sidebar = module.layout.shows_sidebar();
        self.mod_restore = Some(previous);
        self.mod_name = Some(module.folder.clone());
        self.mod_startup = Some(module.startup.clone()).filter(|s| !s.is_empty());

        let mut warnings = module.warnings.clone();
        match &module.picture {
            Some(path) if path.is_file() => {
                if let Err(error) = picture::set_path(path) {
                    warnings.push(error);
                    picture::use_default();
                }
            }
            Some(_) => picture::use_default(),
            None => picture::use_default(),
        }
        if module.picture.is_some() && !picture::on() {
            warnings.push("this terminal cannot show pictures; the default picture is not drawn".to_string());
        }
        let message = format!("mod {} on · layout {} · {} colour{} · {} key{}", module.folder, module.layout.name(),
                              module.theme.len(), if module.theme.len() == 1 { "" } else { "s" },
                              module.keys.len(), if module.keys.len() == 1 { "" } else { "s" });
        Ok((message, warnings))
    }

    /// `/mod off`: the palette, picture, layout, keys and side panel from before the mod come back.
    pub fn mod_off(&mut self) -> String {
        if self.mod_name.is_none() && mods::active(&self.ctx).is_none() {
            return "no mod is active".to_string();
        }
        let name = self.mod_name.clone().or_else(|| mods::active(&self.ctx)).unwrap_or_default();
        self.restore_previous();
        let _ = mods::set_active(&self.ctx, None);
        format!("mod {} off · the previous theme, picture, layout and keys are back", name)
    }

    fn restore_previous(&mut self) {
        let (palette, layout, keys, sidebar) = self.mod_restore.take()
            .unwrap_or((art::Palette::DEFAULT, mods::Layout::Side, mods::Keys::default(), true));
        self.palette = palette;
        self.layout = layout;
        self.keys = keys;
        self.sidebar = sidebar;
        picture::use_default();
        self.mod_name = None;
        self.mod_startup = None;
    }

    pub fn mood(&self) -> Mood {
        if matches!(self.overlay, Overlay::Approve { .. } | Overlay::Question { .. }) {
            return Mood::Asking;
        }
        if self.busy.is_some() {
            let tool = self.items.iter().any(|i| matches!(i, Item::Tool { state: ToolState::Running, .. }));
            return if tool { Mood::Working } else { Mood::Thinking };
        }
        if let Some((m, t)) = self.flash {
            if t.elapsed() < Duration::from_secs(4) {
                return m;
            }
        }
        if self.last_key.elapsed() > Duration::from_secs(180) {
            Mood::Asleep
        } else {
            Mood::Awake
        }
    }

    pub fn input_text(&self) -> String {
        self.input.iter().collect()
    }

    /// The commands that match the input while it holds a slash word and nothing else.
    pub fn slash_matches(&self) -> Vec<&'static Cmd> {
        let text = self.input_text();
        if !text.starts_with('/') || text.contains(char::is_whitespace) {
            return Vec::new();
        }
        COMMANDS.iter().filter(|c| c.name.starts_with(text.as_str())).collect()
    }

    pub fn palette_matches(query: &str) -> Vec<&'static Cmd> {
        let q = query.to_lowercase();
        COMMANDS.iter().filter(|c| q.is_empty() || c.name.contains(&q) || c.help.contains(&q)).collect()
    }

    fn provider_name(&self, target: &str) -> String {
        let m = self.model_value(target);
        crate::providers::of(m).map(|p| p.name.to_string()).unwrap_or_else(|| m["kind"].as_str().unwrap_or("claude-cli").to_string())
    }

    /// The model names the picker offers: the provider's own list first, then the static catalog, then the current name.
    pub fn model_choices(&self, target: &str) -> Vec<String> {
        let provider = self.provider_name(target);
        let current = self.model_value(target)["model"].as_str().unwrap_or("").to_string();
        let mut out = if let Some(list) = self.model_cache.get(&provider) {
            list.iter().map(|m| m.id.clone()).collect::<Vec<_>>()
        } else {
            crate::providers::models(&provider).iter().map(|m| m.id.to_string()).collect::<Vec<_>>()
        };
        if !current.is_empty() && !out.iter().any(|m| *m == current) {
            out.insert(0, current);
        }
        out
    }

    /// The effort levels of one model. The provider's answer wins over the static rules.
    pub fn model_efforts(&self, target: &str, model: &str) -> Vec<String> {
        let provider = self.provider_name(target);
        if let Some(found) = self.model_cache.get(&provider).and_then(|list| list.iter().find(|m| m.id == model)) {
            return found.efforts.clone();
        }
        crate::providers::efforts(&provider, model).iter().map(|e| e.to_string()).collect()
    }

    /// Shows the cached list at once and asks the provider in the background for a fresh one.
    fn load_models(&mut self, target: &str) {
        let provider = self.provider_name(target);
        if matches!(provider.as_str(), "claude-cli" | "laertes") {
            self.model_source = if provider == "claude-cli" { "built-in Claude aliases".to_string() } else { "the family is in training".to_string() };
            return;
        }
        let cached = crate::providers::cached_models(&self.ctx, &provider);
        if !cached.is_empty() {
            self.model_source = format!("{} cached models · {} · refreshing", cached.len(), provider);
            self.model_cache.insert(provider.clone(), cached);
        } else {
            self.model_source = format!("asking {} for its models", provider);
        }
        if !self.models_fetching.insert(provider.clone()) { return; }
        let model = self.model_value(target).clone();
        let (ctx, tx) = (self.ctx.clone(), self.tx.clone());
        std::thread::spawn(move || {
            let result = crate::providers::fetch_models(&ctx, &provider, &model);
            let _ = tx.send(Msg::Models { provider, result });
        });
    }

    fn model_value(&self, target: &str) -> &serde_json::Value {
        match target {
            "main" | "weak" | "scout" => &self.ctx.cfg[target],
            _ => &self.ctx.cfg["roles"][target],
        }
    }

    fn set_model(&mut self, target: &str, model: &str) {
        let efforts = self.model_efforts(target, model);
        let effort = if efforts.iter().any(|e| e == "high") { "high".to_string() } else { efforts.first().cloned().unwrap_or_default() };
        let value = if matches!(target, "main" | "weak" | "scout") { &mut self.ctx.cfg[target] } else { &mut self.ctx.cfg["roles"][target] };
        value["model"] = serde_json::json!(model);
        if effort.is_empty() {
            if let Some(obj) = value.as_object_mut() { obj.remove("effort"); }
        } else {
            value["effort"] = serde_json::json!(effort);
        }
        save_config(&self.ctx);
        self.refresh_side();
        self.toast(format!("{} model set to {}{}", target, model, if effort.is_empty() { String::new() } else { format!(" · effort {}", effort) }));
    }

    fn cycle_effort(&mut self, target: &str) {
        let (_sel, model) = match &self.overlay {
            Overlay::Models { sel, .. } => (*sel, self.model_choices(target).get(*sel).cloned()),
            _ => return,
        };
        let Some(model) = model else { return };
        let list = self.model_efforts(target, &model);
        if list.is_empty() {
            self.toast(format!("{} has no effort field; nothing is sent.", model));
            return;
        }
        let value = if matches!(target, "main" | "weak" | "scout") { &mut self.ctx.cfg[target] } else { &mut self.ctx.cfg["roles"][target] };
        let current = value["effort"].as_str().unwrap_or("");
        let at = list.iter().position(|e| e == current).unwrap_or(0);
        let next = list[(at + 1) % list.len()].clone();
        value["effort"] = serde_json::json!(next);
        save_config(&self.ctx);
        self.toast(format!("{} · effort {}", model, next));
    }

    /// The indices of the plugins that match the filter. A command name matches too, so typing `fix` finds the plugin
    /// that owns it.
    pub fn plugin_matches(&self, filter: &str, mode: u8) -> Vec<usize> {
        let f = filter.to_lowercase();
        let mut out: Vec<usize> = self.plugin_rows.iter().enumerate().filter(|(_, e)| {
            let mode_ok = match mode { 1 => !e.commands.is_empty(), 2 => !e.servers.is_empty(), _ => true };
            mode_ok && (f.is_empty() || e.name.to_lowercase().contains(&f) || e.commands.iter().any(|c| c.name.to_lowercase().contains(&f)))
        }).map(|(i, _)| i).collect();
        // Plugins with commands first, then MCP servers, then the rest; alphabetical inside each group.
        out.sort_by_key(|i| {
            let e = &self.plugin_rows[*i];
            (e.commands.is_empty(), e.servers.is_empty(), e.name.to_lowercase())
        });
        out
    }

    fn show_tip(&mut self) {
        const TIPS: &[&str] = &[
            "F5 starts a new session; F6 twice deletes the current one.",
            "/model chooses the model; r sets one model per role; e changes the effort.",
            "/providers connects a provider: Claude login or a masked API key.",
            "/model → tab switches the main and the weaker model.",
            "/rules session TEXT keeps a rule in this project; /rules global TEXT keeps it for every project.",
            "Ctrl+O opens the provider list from anywhere; Esc closes a dialog.",
        ];
        let tip = TIPS[self.tip % TIPS.len()];
        self.tip += 1;
        self.note("◆", "tip", tip.to_string(), art::SEA);
    }

    /// F6: the first press asks again, the second removes the current session.
    fn arm_delete_session(&mut self) {
        if self.session_id.is_none() {
            self.toast("this session has no stored transcript yet".to_string());
            return;
        }
        match self.delete_armed {
            Some(at) if at.elapsed() < Duration::from_secs(4) => {
                let id = self.session_id.clone().unwrap_or_default();
                if crate::sessions::delete(&self.ctx, &id) {
                    self.items.clear();
                    self.chat = Some(Chat::new());
                    self.session_id = None;
                    self.tokens = (0, 0);
                    self.turns = 0;
                    self.tool_calls = 0;
                    self.refresh_tabs();
                    self.refresh_side();
                    self.intro = Instant::now();
                    self.delete_armed = None;
                    self.toast("session deleted".to_string());
                } else {
                    self.toast("the session was already gone".to_string());
                }
            }
            _ => {
                self.delete_armed = Some(Instant::now());
                self.toast("press F6 again within 4 seconds to delete this session".to_string());
            }
        }
    }

    /// Writes one provider into main, weak, scout or a role and saves it.
    fn select_provider(&mut self, name: &str, target: &str) {
        let Some(p) = crate::providers::get(name) else { return; };
        if p.kind == "laertes" { return; }
        let (model, effort) = if p.kind == "claude-cli" {
            ("claude-sonnet-5-5", crate::providers::default_effort("claude-cli", "claude-sonnet-5-5"))
        } else {
            (p.model, crate::providers::default_effort(p.name, p.model))
        };
        let mut value = serde_json::json!({"kind": p.kind, "model": model, "base_url": p.base, "api_key_env": p.key_env});
        if !effort.is_empty() { value["effort"] = serde_json::json!(effort); }
        if p.kind == "claude-cli" { value["base_url"] = serde_json::json!(""); value["api_key_env"] = serde_json::json!(""); }
        if matches!(target, "main" | "weak" | "scout") {
            self.ctx.cfg[target] = value;
        } else {
            if !self.ctx.cfg["roles"].is_object() { self.ctx.cfg["roles"] = serde_json::json!({}); }
            self.ctx.cfg["roles"][target] = value;
        }
        save_config(&self.ctx);
        self.refresh_side();
    }

    fn note(&mut self, icon: &'static str, label: &'static str, text: String, colour: art::Rgb) {
        self.items.push(Item::Note { icon, label, text, colour });
        self.scroll = 0;
    }

    fn toast(&mut self, text: String) {
        self.toast = Some((text, Instant::now()));
    }

    fn pager(&mut self, title: &str, body: String) {
        self.overlay = Overlay::Pager { title: title.to_string(), lines: body.lines().map(String::from).collect(), scroll: 0 };
    }

    fn set_input(&mut self, text: &str) {
        self.input = text.chars().collect();
        self.cursor = self.input.len();
        self.slash_sel = 0;
    }

    fn insert(&mut self, text: &str) {
        for c in text.chars().filter(|c| *c != '\r') {
            self.input.insert(self.cursor, c);
            self.cursor += 1;
        }
        self.slash_sel = 0;
    }

    /// A paste goes into the field that is open. An API key must never land in the prompt by accident.
    fn paste(&mut self, text: &str) {
        let clean: String = text.replace("\r\n", "\n").replace('\r', "\n");
        let mut prompt = false;
        match &mut self.overlay {
            Overlay::ApiKey { input, cursor, error, .. } => {
                for c in clean.chars().filter(|c| !c.is_whitespace() && !c.is_control()) {
                    input.insert(*cursor, c);
                    *cursor += 1;
                }
                error.clear();
            }
            Overlay::Palette { query, sel } => {
                query.push_str(&clean.replace(['\n', '\r'], " "));
                *sel = 0;
            }
            Overlay::Plugins { filter, sel, scroll, .. } => {
                filter.push_str(&clean.replace(['\n', '\r'], " "));
                *sel = 0;
                *scroll = 0;
            }
            Overlay::None => prompt = true,
            _ => {}
        }
        if prompt {
            self.insert(&clean);
        }
    }

    fn save_checkpoint(&self) {
        let dir = self.ctx.root.join("checkpoints");
        let _ = fs::create_dir_all(&dir);
        let items: Vec<serde_json::Value> = self.items.iter().map(item_json).collect();
        let _ = fs::write(dir.join(format!("{}.json", self.turns)),
                          serde_json::json!({"turns": self.turns, "tokens": self.tokens, "session": self.session_id, "items": items}).to_string());
    }

    fn rewind(&mut self, turns: usize) -> String {
        let target = self.turns.saturating_sub(turns as u64);
        let path = self.ctx.root.join("checkpoints").join(format!("{}.json", target));
        let Ok(text) = fs::read_to_string(&path) else {
            return format!("no checkpoint for turn {}. F5 starts a new session; checkpoints are kept from then on.", target);
        };
        let v: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
        let files = crate::files::rewind_to(&self.ctx, target as u64);
        self.items = v["items"].as_array().map(|a| a.iter().filter_map(item_from).collect()).unwrap_or_default();
        self.turns = v["turns"].as_u64().unwrap_or(target as u64) as u64;
        self.tokens = (v["tokens"][0].as_u64().unwrap_or(0), v["tokens"][1].as_u64().unwrap_or(0));
        self.session_id = v["session"].as_str().map(str::to_string);
        self.chat = Some(Chat::new());
        self.refresh_tabs();
        self.refresh_side();
        format!("rewound to turn {} · {} file change(s) restored", target, files)
    }

    fn start_turn(&mut self, text: String) {
        self.save_checkpoint();
        let mut chat = self.chat.take().unwrap_or_else(Chat::new);
        self.cancel = chat.cancel.clone();
        self.pid = chat.pid.clone();
        self.busy = Some(Instant::now());
        self.items.push(Item::User(text.clone()));
        self.scroll = 0;
        let mut ctx = self.ctx.clone();
        ctx.cfg["_turn"] = serde_json::json!(self.turns + 1);
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let expanded = crate::context::expand(&ctx, &text);
            if !expanded.refs.is_empty() {
                let _ = tx.send(Msg::Context(expanded.refs));
            }
            let (notes, trail, cached) = tools::recall_all(&ctx, &expanded.text, 1600);
            if !trail.is_empty() {
                let _ = tx.send(Msg::Trail(trail, cached));
            }
            let result = chat.turn_with(&ctx, &expanded.text, &notes, &mut |e| {
                let _ = tx.send(Msg::Turn(e));
            });
            let answer = result.clone().ok();
            let ok = result.is_ok();
            let trace = chat.trace.clone();
            let _ = tx.send(Msg::Done(Box::new(chat), result));
            if let Some(a) = answer {
                let ids = if ctx.cfg["auto_learn"].as_bool().unwrap_or(true) { tools::learn(&ctx, &text, &a) } else { Vec::new() };
                let _ = tx.send(Msg::Learned(ids));
            }
            let lessons = tools::learn_tools(&ctx, &text, &trace);          // also after a failed turn: failures teach the most
            if !lessons.is_empty() {
                let _ = tx.send(Msg::Lessons(lessons));
            }
            if ok {
                if let Some(name) = crate::skills::from_trace(&ctx, &text, &trace) {
                    let _ = tx.send(Msg::Skill(name));
                }
            }
        });
    }

    /// Runs a stored agent plan in the background. The interface stays live; the result arrives as one block.
    fn start_agent(&mut self, spec: crate::agents::Spec) {
        if self.busy.is_some() {
            self.toast("a turn is already running. esc stops it before an agent starts.".to_string());
            return;
        }
        self.items.push(Item::User(format!("${}(goal=\"{}\", max_sub_agents={})", spec.name, spec.goal, spec.max_sub_agents)));
        self.busy = Some(Instant::now());
        self.scroll = 0;
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = cancel.clone();
        let (ctx, tx) = (self.ctx.clone(), self.tx.clone());
        std::thread::spawn(move || {
            let out = crate::agents::run_with_cancel(&ctx, &spec, Some(&cancel));
            let _ = tx.send(Msg::AgentDone(out));
        });
    }

    /// Runs one slow tool (ScreenCodec, a plugin MCP server) off the drawing thread. The result arrives as a reply.
    fn start_bg(&mut self, work: impl FnOnce(Ctx) -> Result<String, String> + Send + 'static) {
        if self.busy.is_some() {
            self.toast("a turn is running. esc stops it first.".to_string());
            return;
        }
        self.busy = Some(Instant::now());
        let (ctx, tx) = (self.ctx.clone(), self.tx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(Msg::AgentDone(work(ctx)));
        });
    }

    fn stop_turn(&mut self) {
        self.cancel.store(true, Ordering::SeqCst);
        let pid = self.pid.load(Ordering::SeqCst);
        if pid != 0 {
            let _ = crate::platform::kill_tree(pid as u64);
        }
    }

    fn drain(&mut self) {
        if self.tick % 25 == 0 { self.side.clusters = crate::clusters::working(&self.ctx); }
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Trail(trail, cached) => {
                    let way = format!("start / {}{}", trail.last().map(|t| t.replace('/', " / ")).unwrap_or_default(), if cached { "  (stored walk)" } else { "" });
                    self.trail = trail;
                    self.note("◆", "memory", way, art::SEA);
                }
                Msg::Turn(TurnEvent::Delta(t)) => {
                    match self.items.last_mut() {
                        Some(Item::Reply(text)) if self.streaming => text.push_str(&t),
                        _ => self.items.push(Item::Reply(t.trim_start().to_string())),
                    }
                    self.streaming = true;
                    self.scroll = 0;
                }
                Msg::Turn(TurnEvent::Text(t)) => {
                    match self.items.last_mut() {
                        Some(Item::Reply(text)) if self.streaming => *text = t,        // the finished block replaces its pieces
                        _ => self.items.push(Item::Reply(t)),
                    }
                    self.streaming = false;
                    self.scroll = 0;
                }
                Msg::Turn(TurnEvent::Masked(n)) => {
                    self.note("◆", "utis", format!("{} masked in the request before the model read it", if n == 1 { "1 item".to_string() } else { format!("{} items", n) }), art::SEA);
                    self.refresh_side();
                }
                Msg::Turn(TurnEvent::MaskedAnswer(n)) => {
                    self.note("◆", "utis", format!("{} masked in the answer before it was drawn and stored", if n == 1 { "1 item".to_string() } else { format!("{} items", n) }), art::SEA);
                    self.refresh_side();
                }
                Msg::Turn(TurnEvent::ToolStart { id, name, detail }) => {
                    self.tool_calls += 1;
                    self.streaming = false;
                    self.items.push(Item::Tool { id, name, detail, state: ToolState::Running, summary: String::new() });
                    self.scroll = 0;
                }
                Msg::Turn(TurnEvent::ToolEnd { id, ok, summary }) => {
                    let mut stored = false;
                    for item in self.items.iter_mut().rev() {
                        if let Item::Tool { id: tid, state, summary: s, name, .. } = item {
                            if *tid == id {
                                *state = if ok { ToolState::Done } else { ToolState::Failed };
                                *s = summary;
                                stored = ok && name == "memory_note";
                                break;
                            }
                        }
                    }
                    if stored {
                        self.flash = Some((Mood::Learned, Instant::now()));
                    }
                    self.refresh_side();
                }
                Msg::Context(refs) => {
                    self.context_refs = refs.iter().map(|r| format!("{} ({} bytes)", r.token, r.bytes)).collect();
                    self.note("◆", "context", format!("{} attached · {}", refs.len(), refs.iter().map(|r| format!("{} {}", r.kind, r.token)).collect::<Vec<_>>().join(", ")), art::SEA);
                }
                Msg::Skill(name) => {
                    self.note("✦", "skill", format!("{} (written from the tool calls of this turn)", name), art::CLAY);
                    self.flash = Some((Mood::Learned, Instant::now()));
                    self.save_session();
                    self.refresh_side();
                }
                Msg::Models { provider, result } => {
                    self.models_fetching.remove(&provider);
                    match result {
                        Ok(list) => {
                            let count = list.len();
                            self.model_cache.insert(provider.clone(), list);
                            self.model_source = format!("{} models from {}", count, provider);
                            if let Overlay::Models { sel: _, target } = &self.overlay {
                                let last = self.model_choices(target).len().saturating_sub(1);
                                if let Overlay::Models { sel, .. } = &mut self.overlay { *sel = (*sel).min(last); }
                            }
                        }
                        Err(e) => {
                            let cached = self.model_cache.get(&provider).map(|l| l.len()).unwrap_or(0);
                            self.model_source = if cached > 0 { format!("{} cached models · provider unreachable: {}", cached, e) } else { format!("built-in names · {}", e) };
                            self.toast(format!("the {} model list is unavailable: {}", provider, e));
                        }
                    }
                }
                Msg::AgentDone(result) => {
                    self.busy = None;
                    match result {
                        Ok(text) => {
                            for line in text.lines() {
                                self.items.push(Item::Reply(line.to_string()));
                            }
                        }
                        Err(e) if e == "stopped" => self.note("■", "stopped", "the agent run was stopped".to_string(), art::ASH),
                        Err(e) => self.items.push(Item::Error(e)),
                    }
                    self.refresh_side();
                    self.scroll = 0;
                    self.refresh_side();
                }
                Msg::Done(chat, result) => {
                    if result.is_ok() && !self.trail.is_empty() {
                        crate::memory::Memory::open(&self.ctx.root).mark_used(&self.trail);
                    }
                    self.tokens = (chat.input_tokens, chat.output_tokens);
                    self.turns = chat.turns;
                    self.session_id = chat.session.clone().or(self.session_id.take());
                    self.chat = Some(*chat);
                    self.busy = None;
                    self.streaming = false;
                    if self.turns > 0 && self.turns % 4 == 0 { self.show_tip(); }
                    let failed = result.is_err();
                    for item in self.items.iter_mut() {
                        if let Item::Tool { state, .. } = item {
                            if *state == ToolState::Running {
                                *state = if failed { ToolState::Failed } else { ToolState::Done };
                            }
                        }
                    }
                    match result {
                        Ok(answer) => {
                            // The answer repeats the last text block of the turn. A note of utis may stand behind that block.
                            let shown = self.items.iter().rev().take_while(|i| !matches!(i, Item::User(_))).any(|i| matches!(i, Item::Reply(t) if t.trim() == answer.trim()));
                            if !shown && !answer.is_empty() {
                                self.items.push(Item::Reply(answer));
                            }
                            self.learning = self.ctx.cfg["auto_learn"].as_bool().unwrap_or(true);
                        }
                        Err(e) if e == "stopped" => self.note("■", "stopped", "the turn was stopped".to_string(), art::ASH),
                        Err(e) => self.items.push(Item::Error(e)),
                    }
                    self.scroll = 0;
                    // The lasting rules of this session go to the shared memory at its end, without a model call.
                    let users: Vec<String> = self.items.iter().filter_map(|i| if let Item::User(t) = i { Some(t.clone()) } else { None }).collect();
                    for id in tools::consolidate(&self.ctx, &users) {
                        self.note("✦", "learned", format!("{} (shared by every project)", id), art::CLAY);
                    }
                    self.save_session();
                    self.refresh_tabs();
                    self.refresh_side();
                }
                Msg::Lessons(ids) => {
                    for id in ids {
                        self.note("✦", "lesson", format!("{} (from the tool calls of this turn)", id), art::CLAY);
                        self.flash = Some((Mood::Learned, Instant::now()));
                    }
                    self.save_session();
                    self.refresh_side();
                }
                Msg::Learned(ids) => {
                    self.learning = false;
                    for id in ids {
                        self.note("✦", "learned", id.clone(), art::CLAY);
                        self.learned.push(id);
                        self.flash = Some((Mood::Learned, Instant::now()));
                    }
                    self.save_session();
                    self.refresh_side();
                }
            }
        }
    }

    /// Looks for a question of the run tool: a command outside the safe list that waits for a yes.
    fn poll_approvals(&mut self) {
        if self.tick % 3 != 0 {
            return;
        }
        if let Overlay::Approve { id, .. } | Overlay::Question { id, .. } = &self.overlay {       // the asker gave up: the question is gone
            if id != "x" && !self.approve_dir.join(format!("{}.req", id)).exists() {
                self.overlay = Overlay::None;
                self.toast("the question ran out of time and counted as no".to_string());
            }
        }
        if !matches!(self.overlay, Overlay::None) {
            return;
        }
        let Ok(dir) = fs::read_dir(&self.approve_dir) else { return };
        for entry in dir.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            let Some(id) = name.strip_suffix(".req") else { continue };
            if self.seen.contains(id) {
                continue;
            }
            self.seen.insert(id.to_string());
            let v: serde_json::Value = fs::read_to_string(entry.path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
            let command = v["command"].as_str().unwrap_or("").to_string();
            if v["kind"].as_str() == Some("question") {
                let options = v["options"].as_array().map(|a| a.iter().filter_map(|o| o.as_str().map(String::from)).collect()).unwrap_or_default();
                self.overlay = Overlay::Question { id: id.to_string(), question: command, options, sel: 0 };
                return;
            }
            if self.allow_all {
                self.answer(id, true);
                continue;
            }
            self.overlay = Overlay::Approve { id: id.to_string(), command, cwd: v["cwd"].as_str().unwrap_or("").to_string(), file: v["kind"].as_str() == Some("file"),
                                              diff: v["diff"].as_str().unwrap_or("").lines().map(String::from).collect() };
            return;
        }
    }

    fn answer(&mut self, id: &str, yes: bool) {
        self.answer_text(id, if yes { "yes" } else { "no" });
    }

    fn answer_text(&mut self, id: &str, text: &str) {
        let tmp = self.approve_dir.join(format!("{}.tmp", id));
        let _ = fs::write(&tmp, text);
        let _ = fs::rename(&tmp, self.approve_dir.join(format!("{}.ans", id)));
    }

    fn copy_last(&mut self) {
        let Some(text) = self.items.iter().rev().find_map(|i| if let Item::Reply(t) = i { Some(t.clone()) } else { None }) else {
            self.toast("no answer to copy yet".to_string());
            return;
        };
        for (program, args) in [("wl-copy", vec![]), ("xclip", vec!["-selection", "clipboard"]), ("pbcopy", vec![])] {
            if let Ok(mut child) = Command::new(program).args(&args).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
                let _ = child.stdin.take().unwrap().write_all(text.as_bytes());
                let _ = child.wait();
                self.toast(format!("{} characters copied with {}", text.chars().count(), program));
                return;
            }
        }
        self.toast("no clipboard program found (wl-copy, xclip or pbcopy)".to_string());
    }

    fn command(&mut self, line: &str) {
        let (name, rest) = line.split_once(char::is_whitespace).map(|(a, b)| (a, b.trim())).unwrap_or((line, ""));
        match name {
            "/quit" | "/q" | "/exit" => self.quit = true,
            "/new" => {
                if self.busy.is_some() {
                    self.stop_turn();
                }
                let users: Vec<String> = self.items.iter().filter_map(|i| if let Item::User(t) = i { Some(t.clone()) } else { None }).collect();
                let _ = tools::consolidate(&self.ctx, &users);
                self.save_session();
                self.session_id = None;
                self.items.clear();
                let _ = fs::remove_file(self.ctx.root.join("plan.json"));
                self.refresh_side();
                self.chat = Some(Chat::new());
                self.tokens = (0, 0);
                self.turns = 0;
                self.tool_calls = 0;
                self.trail.clear();
                self.learned.clear();
                self.scroll = 0;
                self.intro = Instant::now();
                self.delete_armed = None;
            }
            "/models" => self.toast("/model chooses a model; /providers connects a provider".to_string()),
            "/model" => {
                let target = "main".to_string();
                self.load_models(&target);
                let current = self.model_value(&target)["model"].as_str().unwrap_or("").to_string();
                let at = self.model_choices(&target).iter().position(|m| *m == current).unwrap_or(0);
                self.overlay = Overlay::Models { sel: at, target };
            }
            "/mode" if !rest.is_empty() => {
                match crate::mode::apply(&mut self.ctx, rest) {
                    Ok(text) => {
                        self.note("◆", "mode", text, art::SEA);
                        self.refresh_side();
                    }
                    Err(e) => self.items.push(Item::Error(e)),
                }
            }
            "/mode" => self.pager("Modes", crate::mode::report(&self.ctx)),
            "/map" => self.pager("Memory map", tools::memory_map(&self.ctx)),
            "/go" if !rest.is_empty() => self.pager("Memory map: the way", tools::memory_go(&self.ctx, rest)),
            "/note" if !rest.is_empty() => {
                let report = tools::memory_note(&self.ctx, "", rest);
                self.note("✦", "learned", report.trim_start_matches("noted at ").to_string(), art::CLAY);
                self.flash = Some((Mood::Learned, Instant::now()));
                self.refresh_side();
            }
            "/go" | "/note" => self.toast(format!("{} TEXT", name)),
            "/sessions" => {
                self.refresh_tabs();
                let at = self.session_id.as_ref().and_then(|id| self.tabs.iter().rev().position(|(t, _)| t == id)).unwrap_or(0);
                self.overlay = Overlay::Sessions { sel: at };
            }
            "/setup" => self.setup = true,
            "/auth" if rest == "verify" => self.start_bg(|ctx| Ok(crate::auth::verify(&ctx, None))),
            "/auth" => self.pager("Login and keys", crate::auth::status(&self.ctx)),
            "/update" => self.start_bg(|ctx| crate::update::check(&ctx)),
            "/audit" => self.pager("Leak audit", crate::audit::run(&self.ctx)),
            "/input" => match rest {
                "install" => self.start_bg(|_| crate::input::install()),
                "enable" => match crate::input::enable(&mut self.ctx) {
                    Ok(text) => self.toast(text),
                    Err(e) => self.items.push(Item::Error(e)),
                },
                _ => self.pager("Desktop pointer", crate::input::status(&self.ctx)),
            },
            "/login" => self.login = Some(rest.to_string()),
            "/logout" if !rest.is_empty() => {
                if rest == "claude" || rest == "claude-cli" {
                    self.toast("Claude logout runs in a terminal: penelope logout claude".to_string());
                } else {
                    self.toast(crate::auth::remove(rest));
                }
                self.refresh_side();
            }
            "/logout" => self.toast("/logout NAME removes a stored key".to_string()),
            "/scout" => {
                let modes = ["auto", "web", "offer", "off"];
                let now = self.ctx.cfg["delegate"].as_str().unwrap_or("auto").to_string();
                let next = if modes.contains(&rest) { rest } else { modes[(modes.iter().position(|m| *m == now).unwrap_or(3) + 1) % 4] };
                self.ctx.cfg["delegate"] = serde_json::json!(next);
                save_config(&self.ctx);
                self.note("◆", "scout", match next {
                    "auto" => "auto: every search runs on the weaker model, from the next request on",
                    "web" => "web: web research runs on the weaker model, the main model searches files itself",
                    "offer" => "offer: the main model decides whether it searches itself or sends a scout",
                    _ => "off: the main model does every search itself",
                }.to_string(), art::SEA);
            }
            "/providers" if rest == "table" => self.pager("Providers", crate::providers::list(&self.ctx)),
            "/providers" => self.overlay = Overlay::Providers { sel: 0, target: "main".into(), scroll: 0 },
            "/undo" => {
                let report = crate::files::undo(&self.ctx);
                self.note("↺", "undo", report, art::SEA);
            }
            "/jobs" => self.pager("Background jobs", crate::jobs::list(&self.ctx)),
            "/crew" => self.pager("Crew", tools::crew(&self.ctx)),
            "/unravel" => {
                let report = tools::unravel(&self.ctx, 1);
                self.note("↺", "unravel", report, art::SEA);
                self.refresh_side();
            }
            "/stats" => self.pager("Routed tool output", tools::stats(&self.ctx)),
            "/plugins" => {
                let (verb, arg) = rest.split_once(char::is_whitespace).map(|(a, b)| (a, b.trim())).unwrap_or((rest, ""));
                match verb {
                    "" => {
                        self.refresh_side();
                        self.plugin_armed = None;
                        self.overlay = Overlay::Plugins { sel: 0, scroll: 0, filter: String::new(), mode: 0 };
                    }
                    "install" if !arg.is_empty() => {
                        let report = plugins::install(&self.ctx, arg);
                        self.refresh_side();
                        self.pager("Plugins", report);
                    }
                    "import" => {
                        let lines = plugins::import_known(&self.ctx);
                        let report = if lines.is_empty() { "no new plugin found in the folders of other harnesses".to_string() } else { lines.join("\n") };
                        self.refresh_side();
                        self.pager("Plugins", report);
                    }
                    "allow" if !arg.is_empty() => {
                        let report = plugins::set_allow(&self.ctx, arg, true);
                        self.refresh_side();
                        self.pager("Plugins", report);
                    }
                    "deny" if !arg.is_empty() => {
                        let report = plugins::set_allow(&self.ctx, arg, false);
                        self.refresh_side();
                        self.pager("Plugins", report);
                    }
                    "update" if !arg.is_empty() => {
                        let report = plugins::update(&self.ctx, arg);
                        self.refresh_side();
                        self.pager("Plugins", report);
                    }
                    "remove" if !arg.is_empty() => {
                        let report = plugins::remove(&self.ctx, arg);
                        self.refresh_side();
                        self.pager("Plugins", report);
                    }
                    other => self.toast(format!("/plugins {} is none of install LINK, import, allow NAME, deny NAME, update NAME, remove NAME; /plugins opens the screen", other)),
                }
            }
            "/btw" if !rest.is_empty() => {
                let report = tools::memory_note_scope(&self.ctx, "", rest, "project");
                self.note("✦", "learned", report, art::CLAY);
                self.flash = Some((Mood::Learned, Instant::now()));
                self.refresh_side();
            }
            "/btw" => self.toast("/btw TEXT keeps a quick note".to_string()),
            "/rules" if !rest.is_empty() => {
                let (scope, text) = if let Some(rest) = rest.strip_prefix("session ") {
                    ("project", rest.trim())
                } else if let Some(rest) = rest.strip_prefix("global ") {
                    ("global", rest.trim())
                } else {
                    ("global", rest)
                };
                let report = tools::memory_note_scope(&self.ctx, "rules", text, scope);
                self.note("✦", "learned", report, art::CLAY);
                self.flash = Some((Mood::Learned, Instant::now()));
                self.refresh_side();
            }
            "/rules" => self.pager("Shared rules", tools::memory_go(&self.ctx, "rules")),
            "/agents" => self.pager("Agents", crate::agents::list(&self.ctx)),
            "/clusters" => {
                self.refresh_side();
                self.pager("Clusters", crate::clusters::status_text(&self.ctx, None));
            }
            "/cluster" if rest.is_empty() => {
                self.refresh_side();
                self.pager("Clusters", crate::clusters::status_text(&self.ctx, None));
            }
            "/cluster" if rest.starts_with("profiles") => self.pager("Cluster profiles", crate::clusters::profiles_text(&self.ctx)),
            "/cluster" if rest.starts_with("status ") => {
                let id = rest.trim_start_matches("status ").trim().to_string();
                self.pager("Cluster", crate::clusters::status_text(&self.ctx, Some(&id)));
            }
            "/cluster" if rest.starts_with("dump ") => {
                let id = rest.trim_start_matches("dump ").trim().to_string();
                self.pager("Cluster", crate::clusters::dump(&self.ctx, &id));
            }
            "/cluster" if rest.starts_with("kill ") => {
                let id = rest.trim_start_matches("kill ").trim().to_string();
                let report = crate::clusters::kill(&self.ctx, &id);
                self.refresh_side();
                self.note("■", "cluster", report, art::ASH);
            }
            "/cluster" if !rest.is_empty() => {
                let mut words = Vec::new();
                let mut profile = None;
                let mut parallel = true;
                let mut parts = rest.split_whitespace().peekable();
                if parts.peek().map(|p| *p == "run").unwrap_or(false) { parts.next(); }
                while let Some(word) = parts.next() {
                    match word {
                        "--profile" => { profile = parts.next().map(str::to_string); }
                        "--sequential" => { parallel = false; }
                        _ => words.push(word.to_string()),
                    }
                }
                let goal = words.join(" ");
                if goal.is_empty() {
                    self.toast("/cluster GOAL [--profile NAME] [--sequential], or /cluster profiles".to_string());
                } else {
                    let profile = profile.clone();
                    self.start_bg(move |ctx| crate::clusters::orchestrate(&ctx, &goal, profile.as_deref(), parallel));
                }
            }
            "/factory" if rest.is_empty() => {
                self.refresh_side();
                self.pager("Factories", crate::factories::list_text(&self.ctx));
            }
            "/factory" if rest.starts_with("run ") => {
                let name = rest.trim_start_matches("run ").trim().to_string();
                if name.is_empty() {
                    self.toast("/factory run NAME".to_string());
                } else {
                    self.start_bg(move |ctx| crate::factories::run(&ctx, &name));
                }
            }
            "/factory" => {
                let report = crate::factories::slash(&self.ctx, rest);
                self.refresh_side();
                match report {
                    Ok(text) => self.pager("Factories", text),
                    Err(e) => self.items.push(Item::Error(e)),
                }
            }

            "/orchestrate" if !rest.is_empty() => {
                let spec = crate::agents::parse(rest).unwrap_or_else(|_| crate::agents::Spec {
                    name: "orchestrator".into(), role: "orchestrator".into(), thinking: "high".into(), goal: rest.to_string(),
                    max_sub_agents: 3, max_parallel: 2, max_steps: 12, budget_eur: 0.0, model: String::new(),
                    verify: self.ctx.cfg["agent_verify"].as_bool().unwrap_or(true), language: "en".into(), mcp: Vec::new() });
                let _ = crate::agents::save(&self.ctx, &spec);
                self.start_agent(spec);
            }
            "/orchestrate" => self.toast("/orchestrate GOAL or $name(goal=\"...\")".to_string()),
            "/brain" if rest == "write" => {
                match crate::brain::write_vault(&self.ctx) {
                    Ok((path, files)) => {
                        self.note("◆", "memory", format!("{} files in {} · open it as a vault in Obsidian", files, crate::brain::show(&path)), art::SEA);
                        self.refresh_side();
                    }
                    Err(e) => self.items.push(Item::Error(e)),
                }
            }
            "/brain" => self.overlay = Overlay::Brain { scroll: 0 },
            "/cost" => self.pager("Cost in euro", crate::cost::report(&self.ctx)),
            "/route" => self.pager("Roles and models", crate::route::report(&self.ctx)),
            "/tools" => self.pager("Tool contracts", crate::route::table(&self.ctx)),
            "/consolidate" => {
                let users: Vec<String> = self.items.iter().filter_map(|i| if let Item::User(t) = i { Some(t.clone()) } else { None }).collect();
                let ids = tools::consolidate(&self.ctx, &users);
                if ids.is_empty() { self.toast("no lasting rule found in this session".to_string()); }
                for id in ids { self.note("✦", "learned", format!("{} (shared by every project)", id), art::CLAY); }
                self.refresh_side();
            }
            "/context" if !rest.is_empty() => {
                let expanded = crate::context::expand(&self.ctx, rest);
                let (notes, trail, _) = tools::recall_all(&self.ctx, &expanded.text, 1600);
                let path = trail.last().cloned().unwrap_or_else(|| "start".into());
                let body = format!("{} bytes after @ expansion · memory waypoint {}\n\n--- what the model would read after the request ---\n{}\n\n--- notes on the way ---\n{}",
                                   expanded.text.len(), path, expanded.text, if notes.is_empty() { "(none)".to_string() } else { notes });
                self.pager("Context preview", body);
            }
            "/context" => self.toast("/context TEXT shows what @ and the memory map would send".to_string()),
            "/search" if rest == "health" => self.pager("Search engine health", crate::search::health(&self.ctx, false)),
            "/search" if rest == "fanout" => self.pager("Search fanout", crate::search::health(&self.ctx, false)),
            "/search" if rest == "fanout on" || rest == "fanout off" => {
                let report = crate::search::set_fanout(&self.ctx, rest.ends_with("on"));
                self.ctx.cfg["search"]["fanout"] = serde_json::json!(rest.ends_with("on"));
                self.note("◆", "search", report, art::SEA);
            }
            "/search" if !rest.is_empty() => {
                let q = rest.to_string();
                self.start_bg(move |ctx| {
                    let opts = crate::search::Options { limit: 8, ..Default::default() };
                    crate::search::search_hits(&ctx, &q, &opts).map(|(hits, engines)| crate::tools::guard(&ctx, &crate::search::format_hits(&hits, &q, &engines)))
                });
            }
            "/search" => self.toast("/search WORDS".to_string()),
            "/research" if !rest.is_empty() => {
                let q = rest.to_string();
                self.start_bg(move |ctx| Ok(crate::research::run(&ctx, &q, "standard", 4, "")));
            }
            "/research" => self.toast("/research QUESTION".to_string()),
            "/mcp" => {
                if rest == "probe" {
                    self.start_bg(|ctx| Ok(crate::mcpclient::list(&ctx, true)));
                } else if rest.starts_with("call ") {
                    let mut parts = rest[5..].splitn(3, char::is_whitespace);
                    let server = parts.next().unwrap_or("").to_string();
                    let tool = parts.next().unwrap_or("").to_string();
                    let args: serde_json::Value = parts.next().and_then(|j| serde_json::from_str(j).ok()).unwrap_or_else(|| serde_json::json!({}));
                    self.start_bg(move |ctx| crate::mcpclient::call(&ctx, &server, &tool, &args));
                } else if rest.starts_with("add ") {
                    let parts: Vec<&str> = rest[4..].split_whitespace().collect();
                    if parts.len() < 2 {
                        self.toast("/mcp add NAME COMMAND [ARGS...] [--allow]".to_string());
                    } else {
                        let allow = parts.iter().any(|p| *p == "--allow");
                        let extra: Vec<String> = parts[2..].iter().filter(|p| **p != "--allow").map(|p| p.to_string()).collect();
                        let report = crate::plugins::mcp_add(&self.ctx, parts[0], parts[1], &extra, allow);
                        self.pager("MCP servers", report);
                    }
                } else if rest.starts_with("remove ") {
                    self.pager("MCP servers", crate::plugins::mcp_remove(&self.ctx, rest[7..].trim()));
                } else if rest.starts_with("allow ") {
                    self.pager("MCP servers", crate::plugins::mcp_set_allow(&self.ctx, rest[6..].trim(), true));
                } else if rest.starts_with("deny ") {
                    self.pager("MCP servers", crate::plugins::mcp_set_allow(&self.ctx, rest[5..].trim(), false));
                } else {
                    self.pager("MCP servers", crate::mcpclient::list(&self.ctx, false));
                }
            }
            "/screen" => {
                let (verb, arg) = rest.split_once(char::is_whitespace).map(|(a, b)| (a, b.trim())).unwrap_or((rest, ""));
                match verb {
                    "" | "tools" => self.start_bg(|ctx| crate::screen::tools(&ctx).map(|list| list.iter().map(|(n, d)| format!("{}: {}", n, d.chars().take(120).collect::<String>())).collect::<Vec<_>>().join("\n"))),
                    "open" if !arg.is_empty() => {
                        let url = arg.to_string();
                        self.start_bg(move |ctx| crate::screen::tool(&ctx, &serde_json::json!({"action": "open", "url": url, "task": "read this page", "level": "lean"})));
                    }
                    "see" | "snapshot" => self.start_bg(|ctx| crate::screen::tool(&ctx, &serde_json::json!({"action": "snapshot", "level": "lean"}))),
                    "click" if !arg.is_empty() => {
                        let id = arg.to_string();
                        self.start_bg(move |ctx| crate::screen::tool(&ctx, &serde_json::json!({"action": "click", "id": id})));
                    }
                    "type" if !arg.is_empty() => {
                        let (id, text) = arg.split_once(char::is_whitespace).unwrap_or((arg, ""));
                        let (id, text) = (id.to_string(), text.to_string());
                        self.start_bg(move |ctx| crate::screen::tool(&ctx, &serde_json::json!({"action": "type", "id": id, "text": text})));
                    }
                    "desktop" => self.start_bg(|ctx| crate::screen::tool(&ctx, &serde_json::json!({"action": "desktop", "level": "lean"}))),
                    "desktop-click" if !arg.is_empty() => {
                        let find = arg.to_string();
                        self.start_bg(move |ctx| {
                            let before = crate::screen::tool(&ctx, &serde_json::json!({"action": "desktop", "find": find, "level": "lean"}))?;
                            let mut id = None;
                            for line in before.lines() {
                                if !line.to_lowercase().contains(&find.to_lowercase()) { continue; }
                                if let Some(word) = line.split_whitespace().find(|w| {
                                    let w = w.trim_matches(|c: char| !c.is_ascii_alphanumeric());
                                    w.len() > 3 && w.chars().any(|c| c.is_ascii_digit()) && w.chars().next().map(|c| c.is_ascii_alphabetic()).unwrap_or(false)
                                }) {
                                    id = Some(word.trim_matches(|c: char| !c.is_ascii_alphanumeric()).to_string());
                                    break;
                                }
                            }
                            let Some(id) = id else { return Err(format!("no element with the text \"{}\"", find)); };
                            let click = crate::screen::tool(&ctx, &serde_json::json!({"action": "desktop_click", "id": id}))?;
                            let after = crate::screen::tool(&ctx, &serde_json::json!({"action": "desktop", "level": "lean"}))?;
                            Ok(format!("element {} · {}\n{}\n--- after ---\n{}", id, find, click, after))
                        });
                    }
                    "call" if !arg.is_empty() => {
                        let mut parts = arg.splitn(2, char::is_whitespace);
                        let tool = parts.next().unwrap_or("").to_string();
                        let args: serde_json::Value = parts.next().and_then(|j| serde_json::from_str(j).ok()).unwrap_or_else(|| serde_json::json!({}));
                        self.start_bg(move |ctx| crate::screen::tool(&ctx, &serde_json::json!({"action": "call", "tool": tool, "arguments": args})));
                    }
                    other => self.toast(format!("/screen {} is none of tools, open URL, see, click ID, type ID TEXT, desktop, desktop-click TEXT, call TOOL JSON", other)),
                }
            }
            "/cursor" => {
                let mut parts = rest.split_whitespace();
                let x: i64 = parts.next().and_then(|n| n.parse().ok()).unwrap_or(0);
                let y: i64 = parts.next().and_then(|n| n.parse().ok()).unwrap_or(0);
                let click = parts.any(|p| p == "click");
                match crate::cursor::tool(&self.ctx, &serde_json::json!({"x": x, "y": y, "click": click})) {
                    Ok(text) => self.toast(text),
                    Err(e) => self.items.push(Item::Error(e)),
                }
            }
            "/export" => {
                let id = self.session_id.clone().unwrap_or_else(|| "new".to_string());
                let mut body = format!("# Penelope session {}\n\nExported {}.\n\n", id, crate::sessions::stamp(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)));
                for item in &self.items {
                    match item {
                        Item::User(t) => body.push_str(&format!("## User\n\n{}\n\n", t)),
                        Item::Reply(t) => body.push_str(&format!("{}\n\n", t)),
                        Item::Tool { name, detail, state, summary, .. } => body.push_str(&format!("- tool `{}` {} — {} · {}\n", name, detail, if *state == ToolState::Failed { "failed" } else { "ok" }, summary)),
                        Item::Note { label, text, .. } => body.push_str(&format!("- {}: {}\n", label, text)),
                        Item::Error(t) => body.push_str(&format!("- error: {}\n", t)),
                    }
                }
                let dir = self.ctx.root.join("exports");
                let _ = fs::create_dir_all(&dir);
                let path = dir.join(format!("{}.md", id));
                match fs::write(&path, body) {
                    Ok(_) => self.toast(format!("session written to {}", crate::files::show(&path))),
                    Err(e) => self.items.push(Item::Error(format!("the export failed: {}", e))),
                }
            }
            "/skills" => {
                let list = crate::skills::list(&self.ctx);
                let body = if list.is_empty() {
                    "no skill written here yet. A turn with 5 or more successful tool calls writes one; the model can write one with skill_new.".to_string()
                } else {
                    list.iter().map(|(n, d)| format!("{}: {}", n, d)).collect::<Vec<_>>().join("\n")
                };
                self.pager("Skills of this project", body);
            }
            "/mod" => {
                let (verb, arg) = rest.split_once(char::is_whitespace).map(|(a, b)| (a, b.trim())).unwrap_or((rest, ""));
                match verb {
                    "" | "list" => self.pager("Mods", mods::report(&self.ctx)),
                    "new" if !arg.is_empty() => {
                        let (name, prompt) = mods::new_request(&self.ctx, arg);
                        self.note("◆", "mod", format!("the model writes the mod under .penelope/mods/{}. /mod on {} applies it when it is done.", name, name), art::SEA);
                        self.start_turn(prompt);
                    }
                    "new" => self.toast("/mod new \"what the mod should change\"".to_string()),
                    "on" if !arg.is_empty() => match self.apply_mod(arg) {
                        Ok((message, warnings)) => {
                            self.toast(message);
                            for warning in warnings {
                                self.note("◆", "mod", warning, art::ASH);
                            }
                            if let Some(text) = self.mod_startup.clone() {
                                self.note("◆", "mod", text, art::SEA);
                            }
                        }
                        Err(error) => self.items.push(Item::Error(format!("mod {} was not turned on: {}", arg, error))),
                    },
                    "on" => self.toast("/mod on NAME; /mod list shows the names".to_string()),
                    "off" => {
                        let report = self.mod_off();
                        self.note("◆", "mod", report, art::SEA);
                    }
                    "remove" if !arg.is_empty() => {
                        if self.mod_name.as_deref() == Some(arg) {
                            let report = self.mod_off();
                            self.note("◆", "mod", report, art::SEA);
                        }
                        match mods::remove(&self.ctx, arg) {
                            Ok(report) => self.toast(report),
                            Err(error) => self.items.push(Item::Error(error)),
                        }
                    }
                    "remove" => self.toast("/mod remove NAME".to_string()),
                    other => self.toast(format!("/mod {} is none of list, new TEXT, on NAME, off, remove NAME", other)),
                }
            }
            "/forget" if !rest.is_empty() => {
                let report = tools::memory_forget(&self.ctx, rest, "auto");
                self.note("↺", "unravel", report, art::SEA);
                self.refresh_side();
            }
            "/forget" => self.toast("/forget TEXT takes back one note".to_string()),
            "/sandbox" => match rest.trim() {
                "on" => {
                    self.ctx.cfg["sandbox"] = serde_json::json!("bwrap");
                    save_config(&self.ctx);
                    self.toast("sandbox on: run uses bubblewrap, read-only root, no network".to_string());
                }
                "off" => {
                    self.ctx.cfg["sandbox"] = serde_json::json!("off");
                    save_config(&self.ctx);
                    self.toast("sandbox off: run uses the normal shell".to_string());
                }
                _ => self.toast("/sandbox on or /sandbox off".to_string()),
            },
            "/rewind" if !rest.is_empty() => match rest.trim().parse::<usize>() {
                Ok(n) if n > 0 => {
                    let report = self.rewind(n);
                    self.note("↺", "rewind", report, art::SEA);
                }
                _ => self.toast("/rewind N, for example /rewind 3".to_string()),
            },
            "/rewind" => self.toast("/rewind N restores files and transcript N turns back".to_string()),
            "/budget" if !rest.is_empty() => match rest.trim().parse::<f64>() {
                Ok(value) if value >= 0.0 => {
                    self.ctx.cfg["monthly_budget_eur"] = serde_json::json!(value);
                    save_config(&self.ctx);
                    self.refresh_side();
                    self.toast(format!("monthly budget is {:.2} EUR", value));
                }
                _ => self.toast("/budget EUR, for example /budget 20".to_string()),
            },
            "/budget" => self.pager("Monthly budget", crate::cost::report(&self.ctx)),
            "/price" if !rest.is_empty() => {
                let parts: Vec<&str> = rest.split_whitespace().collect();
                if parts.len() != 3 || parts[1].parse::<f64>().is_err() || parts[2].parse::<f64>().is_err() {
                    self.toast("/price MODEL INPUT OUTPUT in dollars per million tokens".to_string());
                } else {
                    let (model, input, output) = (parts[0], parts[1].parse::<f64>().unwrap(), parts[2].parse::<f64>().unwrap());
                    self.ctx.cfg["prices"][model] = serde_json::json!([input, output]);
                    save_config(&self.ctx);
                    self.toast(format!("price for {}: {} / {} dollars per million tokens", model, input, output));
                    self.refresh_side();
                }
            }
            "/auto" => {
                let now = self.ctx.cfg["auto_learn"].as_bool().unwrap_or(true);
                self.ctx.cfg["auto_learn"] = serde_json::json!(!now);
                save_config(&self.ctx);
                self.toast(format!("auto-learning is now {}", if now { "off" } else { "on" }));
            }
            "/learn" => {
                let lessons = tools::memory_go(&self.ctx, "tools");
                let feedback = tools::memory_go(&self.ctx, "rules feedback");
                self.pager("What Penelope learned", format!("Lessons from tool calls\n\n{}\n\nComplaints and rules\n\n{}", lessons, feedback));
            }
            "/forge" if !rest.is_empty() => {
                let prompt = format!("Read the memory waypoint \"{}\". If its notes describe a command that repeats, write one tool with tool_new: a lower case name, a short description, a JSON schema and a command with {{argument}} placeholders. If the notes do not describe such a command, say so and write nothing.", rest);
                self.start_turn(prompt);
            }
            "/forge" => self.toast("/forge TOPIC writes a tool from the notes at that waypoint".to_string()),
            "/doctor" => self.pager("Doctor", crate::doctor_text(&self.ctx)),
            "/copy" => self.copy_last(),
            "/shot" => self.shot = true,
            "/sidebar" => self.sidebar = !self.sidebar,
            "/help" => {
                let mut text = help_text();
                let plugin_commands = plugins::command_index(&self.ctx, None);
                if !plugin_commands.is_empty() {
                    text.push_str("\nPlugin commands\n");
                    text.push_str(&plugin_commands);
                } else {
                    text.push_str("\nNo plugin command yet. /plugins opens the plugin screen.\n");
                }
                self.pager("Keys and commands", text);
            }
            other => {
                let command = other.trim_start_matches('/');
                if let Some(text) = plugins::expand_command(&self.ctx, command, rest) {
                    if self.busy.is_some() {
                        self.toast("Penelope still works on the last request. esc stops it.".to_string());
                    } else {
                        self.start_turn(text);
                    }
                } else {
                    self.toast(format!("no command {} (ctrl+p lists the commands; /plugins shows plugin commands)", other));
                }
            }
        }
    }

    fn run_cmd(&mut self, cmd: &Cmd) {
        if cmd.takes_text {
            self.set_input(&format!("{} ", cmd.name));
        } else {
            self.set_input("");
            self.command(cmd.name);
        }
    }

    /// One action a mod may put on another key. The fixed keys esc, ctrl+c, f5 and f6 never reach here.
    fn run_action(&mut self, action: mods::Action) {
        match action {
            mods::Action::Sidebar => self.sidebar = !self.sidebar,
            mods::Action::Palette => self.overlay = Overlay::Palette { query: String::new(), sel: 0 },
            mods::Action::Providers => self.overlay = Overlay::Providers { sel: 0, target: "main".into(), scroll: 0 },
            mods::Action::NewSession => self.command("/new"),
            mods::Action::Sessions => self.command("/sessions"),
            mods::Action::Help => self.command("/help"),
            mods::Action::TabPrev => self.step_tab(-1),
            mods::Action::TabNext => self.step_tab(1),
            mods::Action::CopyLast => self.copy_last(),
        }
    }

    fn submit(&mut self) {
        let text = self.input_text().trim().to_string();
        if text.is_empty() {
            return;
        }
        if !text.starts_with('/') && self.busy.is_some() {
            self.toast("Penelope still works on the last request. esc stops it.".to_string());
            return;
        }
        self.set_input("");
        self.hist_at = None;
        if self.history.last() != Some(&text) {
            self.history.push(text.clone());
            if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(self.ctx.root.join("history.jsonl")) {
                let _ = writeln!(f, "{}", serde_json::to_string(&text).unwrap());
            }
        }
        let actions = crate::context::parse(&text);
        let many = actions.len() > 1 || matches!(actions.first(), Some(crate::context::Action::Command(_) | crate::context::Action::Agent(_)));
        if many {
            for action in actions {
                match action {
                    crate::context::Action::Command(command) => self.command(&format!("/{}", command)),
                    crate::context::Action::Agent(call) => match crate::agents::parse(&call) {
                        Ok(spec) => {
                            let _ = crate::agents::save(&self.ctx, &spec);
                            self.start_agent(spec);
                        }
                        Err(e) => self.items.push(Item::Error(e)),
                    },
                    crate::context::Action::Turn(turn) => self.start_turn(turn),
                }
            }
        } else {
            self.start_turn(text);
        }
    }

    fn history_step(&mut self, back: bool) {
        if self.history.is_empty() {
            return;
        }
        let next = match (self.hist_at, back) {
            (None, true) => Some(self.history.len() - 1),
            (Some(i), true) => Some(i.saturating_sub(1)),
            (Some(i), false) if i + 1 < self.history.len() => Some(i + 1),
            (_, false) => None,
        };
        self.hist_at = next;
        let text = next.map(|i| self.history[i].clone()).unwrap_or_default();
        self.set_input(&text);
    }

    fn overlay_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let mut close = false;
        let mut run: Option<&'static Cmd> = None;
        let mut reply: Option<(String, bool, bool)> = None;
        let mut provider: Option<(usize, String)> = None;
        let mut models_open: Option<String> = None;
        let mut roles_open = false;
        let mut providers_open: Option<String> = None;
        let mut key_save: Option<(String, String, String, String)> = None;
        let mut pick: Option<(usize, String)> = None;
        let mut effort: Option<String> = None;
        let mut open: Option<usize> = None;
        let mut said: Option<(String, String)> = None;
        let mut plugin_open: Option<String> = None;
        let mut plugin_back: Option<String> = None;
        let mut plugin_toggle: Option<(String, bool)> = None;
        let mut plugin_update: Option<String> = None;
        let mut plugin_remove: Option<String> = None;
        let mut plugin_use: Option<(String, String)> = None;
        let mut delete_session: Option<String> = None;
        if !matches!(key.code, KeyCode::Char('X')) { self.plugin_armed = None; }
        let plugin_filtered = if let Overlay::Plugins { filter, mode, .. } = &self.overlay { self.plugin_matches(filter, *mode) } else { Vec::new() };
        match &mut self.overlay {
            Overlay::None => {}
            Overlay::Palette { query, sel } => match key.code {
                KeyCode::Esc => close = true,
                KeyCode::Up => *sel = sel.saturating_sub(1),
                KeyCode::Down => *sel = (*sel + 1).min(App::palette_matches(query).len().saturating_sub(1)),
                KeyCode::Enter => {
                    run = App::palette_matches(query).get(*sel).copied();
                    close = true;
                }
                KeyCode::Backspace => {
                    query.pop();
                    *sel = 0;
                }
                KeyCode::Char('p') if ctrl => close = true,
                KeyCode::Char(c) if !ctrl => {
                    query.push(c);
                    *sel = 0;
                }
                _ => {}
            },
            Overlay::Providers { sel, target, scroll } => match key.code {
                KeyCode::Esc => close = true,
                KeyCode::Up => *sel = sel.saturating_sub(1),
                KeyCode::Down => *sel += 1,
                KeyCode::PageUp => *sel = sel.saturating_sub(8),
                KeyCode::PageDown => *sel += 8,
                KeyCode::Tab | KeyCode::Left | KeyCode::Right => {
                    *target = if target == "main" { "weak".to_string() } else if target == "weak" { "main".to_string() } else { target.clone() };
                    *sel = 0;
                    *scroll = 0;
                }
                KeyCode::Enter => provider = Some((*sel, target.clone())),
                _ => {}
            },
            Overlay::Roles { sel } => match key.code {
                KeyCode::Esc => close = true,
                KeyCode::Up => *sel = sel.saturating_sub(1),
                KeyCode::Down => *sel = (*sel + 1).min(ROLE_MODELS.len().saturating_sub(1)),
                KeyCode::Enter => providers_open = Some(ROLE_MODELS[(*sel).min(ROLE_MODELS.len().saturating_sub(1))].0.to_string()),
                _ => {}
            },
            Overlay::ApiKey { name, label, input, cursor, error, target } => match key.code {
                KeyCode::Esc => close = true,
                KeyCode::Enter => key_save = Some((name.clone(), label.clone(), input.iter().collect::<String>(), target.clone())),
                KeyCode::Backspace => {
                    if *cursor > 0 { *cursor -= 1; input.remove(*cursor); }
                    error.clear();
                }
                KeyCode::Delete => {
                    if *cursor < input.len() { input.remove(*cursor); }
                    error.clear();
                }
                KeyCode::Left => *cursor = cursor.saturating_sub(1),
                KeyCode::Right => *cursor = (*cursor + 1).min(input.len()),
                KeyCode::Home => *cursor = 0,
                KeyCode::End => *cursor = input.len(),
                KeyCode::Char('u') if ctrl => { input.clear(); *cursor = 0; error.clear(); }
                KeyCode::Char(c) if !ctrl => { input.insert(*cursor, c); *cursor += 1; error.clear(); }
                _ => {}
            },
            Overlay::Models { sel, target } => match key.code {
                KeyCode::Esc => close = true,
                KeyCode::Up => *sel = sel.saturating_sub(1),
                KeyCode::Down => *sel += 1,
                KeyCode::PageUp => *sel = sel.saturating_sub(10),
                KeyCode::PageDown => *sel += 10,
                KeyCode::Tab => models_open = Some(if target == "main" { "weak".to_string() } else { "main".to_string() }),
                KeyCode::Enter => pick = Some((*sel, target.clone())),
                KeyCode::Char('e') => effort = Some(target.clone()),
                KeyCode::Char('r') => roles_open = true,
                _ => {}
            },
            Overlay::Pager { scroll, lines, .. } => match key.code {
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => close = true,
                KeyCode::Up => *scroll = scroll.saturating_sub(1),
                KeyCode::Down => *scroll = (*scroll + 1).min(lines.len().saturating_sub(1)),
                KeyCode::PageUp => *scroll = scroll.saturating_sub(10),
                KeyCode::PageDown => *scroll = (*scroll + 10).min(lines.len().saturating_sub(1)),
                _ => {}
            },
            Overlay::Question { id, options, sel, .. } => match key.code {
                KeyCode::Up => *sel = sel.saturating_sub(1),
                KeyCode::Down => *sel = (*sel + 1).min(options.len().saturating_sub(1)),
                KeyCode::Enter => said = Some((id.clone(), options.get(*sel).cloned().unwrap_or_default())),
                KeyCode::Char(c) if c.is_ascii_digit() && (c as usize - '0' as usize) >= 1 && (c as usize - '0' as usize) <= options.len() =>
                    said = Some((id.clone(), options[c as usize - '1' as usize].clone())),
                KeyCode::Esc => said = Some((id.clone(), String::new())),
                _ => {}
            },
            Overlay::Sessions { sel } => match key.code {
                KeyCode::Esc | KeyCode::Char('q') => close = true,
                KeyCode::Up => *sel = sel.saturating_sub(1),
                KeyCode::Down => *sel = (*sel + 1).min(self.tabs.len().saturating_sub(1)),
                KeyCode::Delete => {
                    if let Some((id, _)) = self.tabs.get(self.tabs.len().saturating_sub(1 + *sel)) { delete_session = Some(id.clone()); }
                }
                KeyCode::Enter => {
                    open = Some(self.tabs.len().saturating_sub(1 + *sel));        // the list shows the newest first
                    close = true;
                }
                _ => {}
            },
            Overlay::Brain { scroll } => match key.code {
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => close = true,
                KeyCode::Up => *scroll = scroll.saturating_sub(1),
                KeyCode::Down => *scroll = (*scroll + 1).min(crate::brain::ascii(&self.ctx).lines().count().saturating_sub(1)),
                KeyCode::PageUp => *scroll = scroll.saturating_sub(10),
                KeyCode::PageDown => *scroll += 10,
                KeyCode::Char('w') => {
                    let report = match crate::brain::write_vault(&self.ctx) {
                        Ok((path, files)) => format!("{} files in {} · open it as a vault in Obsidian", files, crate::brain::show(&path)),
                        Err(e) => e,
                    };
                    self.overlay = Overlay::None;
                    self.toast(report);
                }
                _ => {}
            },
            Overlay::Plugins { sel, scroll, filter, mode } => {
                let matches = &plugin_filtered;
                match key.code {
                    KeyCode::Tab => { *mode = (*mode + 1) % 3; *sel = 0; *scroll = 0; }
                    KeyCode::Esc if !filter.is_empty() => { filter.clear(); *sel = 0; *scroll = 0; }
                    KeyCode::Esc if filter.is_empty() => close = true,
                    KeyCode::Up => *sel = sel.saturating_sub(1),
                    KeyCode::Down => *sel = (*sel + 1).min(matches.len().saturating_sub(1)),
                    KeyCode::PageUp => *sel = sel.saturating_sub(8),
                    KeyCode::PageDown => *sel = (*sel + 8).min(matches.len().saturating_sub(1)),
                    KeyCode::Backspace => { filter.pop(); *sel = 0; *scroll = 0; }
                    KeyCode::Enter => {
                        if let Some(entry) = matches.get(*sel).and_then(|i| self.plugin_rows.get(*i)) { plugin_open = Some(entry.name.clone()); }
                    }
                    KeyCode::Char('A') if filter.is_empty() => {
                        if let Some(entry) = matches.get(*sel).and_then(|i| self.plugin_rows.get(*i)) { plugin_toggle = Some((entry.name.clone(), !entry.allow)); }
                    }
                    KeyCode::Char('U') if filter.is_empty() => {
                        if let Some(entry) = matches.get(*sel).and_then(|i| self.plugin_rows.get(*i)) { plugin_update = Some(entry.name.clone()); }
                    }
                    KeyCode::Char('X') if filter.is_empty() => {
                        if let Some(entry) = matches.get(*sel).and_then(|i| self.plugin_rows.get(*i)) { plugin_remove = Some(entry.name.clone()); }
                    }
                    KeyCode::Char(c) if !ctrl && (c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | ':' | ' ')) => {
                        filter.push(c); *sel = 0; *scroll = 0;
                    }
                    _ => {}
                }
                let rows = 9usize;
                if *sel < *scroll { *scroll = *sel; }
                if *sel >= *scroll + rows { *scroll = *sel + 1 - rows; }
            }
            Overlay::PluginCommands { plugin, sel, scroll } => {
                let list = self.plugin_rows.iter().find(|e| e.name == *plugin).map(|e| e.commands.clone()).unwrap_or_default();
                match key.code {
                    KeyCode::Esc | KeyCode::Char('q') => plugin_back = Some(plugin.clone()),
                    KeyCode::Up => *sel = sel.saturating_sub(1),
                    KeyCode::Down => *sel = (*sel + 1).min(list.len().saturating_sub(1)),
                    KeyCode::PageUp => *sel = sel.saturating_sub(8),
                    KeyCode::PageDown => *sel = (*sel + 8).min(list.len().saturating_sub(1)),
                    KeyCode::Enter => {
                        if let Some(command) = list.get(*sel) { plugin_use = Some((plugin.clone(), command.name.clone())); }
                    }
                    _ => {}
                }
                let rows = 14usize;
                if *sel < *scroll { *scroll = *sel; }
                if *sel >= *scroll + rows { *scroll = *sel + 1 - rows; }
            }
            Overlay::Approve { id, .. } => match key.code {
                KeyCode::Char('y') | KeyCode::Enter => reply = Some((id.clone(), true, false)),
                KeyCode::Char('a') => reply = Some((id.clone(), true, true)),
                KeyCode::Char('n') | KeyCode::Esc => reply = Some((id.clone(), false, false)),
                _ => {}
            },
        }
        if let Overlay::Models { sel: _, target } = &self.overlay {
            let last = self.model_choices(target).len().saturating_sub(1);
            if let Overlay::Models { sel, .. } = &mut self.overlay {
                *sel = (*sel).min(last);
            }
        }
        if let Overlay::Providers { sel, scroll, .. } = &mut self.overlay {
            let last = crate::providers::TABLE.len().saturating_sub(1);
            *sel = (*sel).min(last);
            let rows = 20usize;
            if *sel < *scroll { *scroll = *sel; }
            if *sel >= *scroll + rows { *scroll = *sel + 1 - rows; }
        }
        if let Overlay::Roles { sel } = &mut self.overlay {
            *sel = (*sel).min(ROLE_MODELS.len().saturating_sub(1));
        }
        if let Some(target) = models_open {
            self.load_models(&target);
            let current = self.model_value(&target)["model"].as_str().unwrap_or("").to_string();
            let at = self.model_choices(&target).iter().position(|m| *m == current).unwrap_or(0);
            self.overlay = Overlay::Models { sel: at, target };
            close = false;
        }
        if roles_open {
            self.overlay = Overlay::Roles { sel: 0 };
            close = false;
        }
        if let Some(target) = providers_open {
            self.overlay = Overlay::Providers { sel: 0, target, scroll: 0 };
            close = false;
        }
        if let Some((sel, target)) = provider {
            let p = &crate::providers::TABLE[sel.min(crate::providers::TABLE.len().saturating_sub(1))];
            if p.kind == "laertes" {
                self.toast(format!("{} is a Laertes family in training; it cannot log in yet.", p.label));
                close = true;
            } else if p.name == "claude-cli" {
                self.select_provider(p.name, &target);
                self.overlay = Overlay::None;
                self.login = Some("claude".to_string());
                close = true;
            } else {
                self.overlay = Overlay::ApiKey { name: p.name.to_string(), label: p.label.to_string(), input: Vec::new(), cursor: 0, error: String::new(), target };
                close = false;
            }
        }
        if let Some((name, label, key, target)) = key_save {
            match crate::auth::store(&name, &key) {
                Ok(store) => {
                    self.select_provider(&name, &target);
                    self.overlay = Overlay::None;
                    self.toast(format!("{} key stored in {}. {} model set.", label, store, target));
                    close = false;
                }
                Err(e) => {
                    if let Overlay::ApiKey { error, .. } = &mut self.overlay { *error = e; }
                    close = false;
                }
            }
        }
        if let Some(name) = plugin_open {
            self.overlay = Overlay::PluginCommands { plugin: name, sel: 0, scroll: 0 };
            close = false;
        }
        if let Some(name) = plugin_back {
            let at = self.plugin_rows.iter().position(|e| e.name == name).unwrap_or(0);
            self.overlay = Overlay::Plugins { sel: at, scroll: 0, filter: String::new(), mode: 0 };
            close = false;
        }
        if let Some((name, allow)) = plugin_toggle {
            let report = plugins::set_allow(&self.ctx, &name, allow);
            self.refresh_side();
            self.toast(report);
            close = false;
        }
        if let Some(name) = plugin_update {
            self.overlay = Overlay::None;
            self.start_bg(move |ctx| Ok(plugins::update(&ctx, &name)));
            close = false;
        }
        if let Some(name) = plugin_remove {
            if self.plugin_armed.as_deref() == Some(name.as_str()) {
                let report = plugins::remove(&self.ctx, &name);
                self.refresh_side();
                self.toast(report);
                self.plugin_armed = None;
            } else {
                self.plugin_armed = Some(name.clone());
                self.toast(format!("press x again to remove {}; the folder stays", name));
            }
            close = false;
        }
        if let Some((plugin, command)) = plugin_use {
            self.set_input(&format!("/{}:{} ", plugin, command));
            close = true;
        }
        if let Some(target) = effort {
            self.cycle_effort(&target);
            close = false;
        }
        if let Some((sel, target)) = pick {
            if let Some(model) = self.model_choices(&target).get(sel).cloned() {
                self.set_model(&target, &model);
            }
            close = true;
        }
        if let Some(id) = delete_session {
            if crate::sessions::delete(&self.ctx, &id) {
                if self.session_id.as_deref() == Some(id.as_str()) {
                    self.items.clear();
                    self.chat = Some(Chat::new());
                    self.session_id = None;
                    self.tokens = (0, 0);
                    self.turns = 0;
                    self.tool_calls = 0;
                    self.trail.clear();
                    self.learned.clear();
                    self.scroll = 0;
                }
                self.refresh_tabs();
                if self.tabs.is_empty() {
                    close = true;
                } else if let Overlay::Sessions { sel } = &mut self.overlay {
                    *sel = (*sel).min(self.tabs.len().saturating_sub(1));
                }
                self.refresh_side();
                self.toast(format!("session {} deleted", id));
            } else {
                self.toast("the session was already gone".to_string());
            }
        }
        if let Some((id, text)) = said {
            self.answer_text(&id, &text);
            close = true;
        }
        if let Some(index) = open {
            self.overlay = Overlay::None;
            self.open_tab(index);
        }
        if let Some((id, yes, always)) = reply {
            self.allow_all |= always;
            self.answer(&id, yes);
            close = true;
        }
        if close {
            self.overlay = Overlay::None;
            self.plugin_armed = None;
        }
        if let Some(cmd) = run {
            self.run_cmd(cmd);
        }
    }

    fn key(&mut self, key: KeyEvent) {
        self.last_key = Instant::now();
        if key.code == KeyCode::F(12) {
            self.shot = true;
            return;
        }
        if !matches!(self.overlay, Overlay::None) {
            if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) && !matches!(self.overlay, Overlay::Approve { .. } | Overlay::Question { .. }) {
                self.overlay = Overlay::None;
                return;
            }
            self.overlay_key(key);
            return;
        }
        // The keys of the main actions come from the active mod. A key the mod moved away from its default does
        // nothing instead of its old job; esc, ctrl+c, f5 and f6 are not part of this table.
        if let Some(name) = mods::key_name(&key) {
            if let Some(action) = self.keys.action_for(&name) {
                self.run_action(action);
                return;
            }
            if self.keys.is_displaced_default(&name) {
                return;
            }
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let slash = self.slash_matches();
        match key.code {
            KeyCode::Char('c') if ctrl => {
                if self.input.is_empty() {
                    self.quit = true;
                } else {
                    self.set_input("");
                }
            }
            KeyCode::Char('d') if ctrl && self.input.is_empty() => self.quit = true,
            KeyCode::F(5) => self.command("/new"),
            KeyCode::F(6) => self.arm_delete_session(),
            KeyCode::Char(c) if alt && c.is_ascii_digit() && c != '0' => self.open_tab(c as usize - '1' as usize),
            KeyCode::Esc => {
                if self.busy.is_some() {
                    self.stop_turn();
                } else {
                    self.set_input("");
                }
            }
            KeyCode::Up if !slash.is_empty() => self.slash_sel = self.slash_sel.saturating_sub(1),
            KeyCode::Down if !slash.is_empty() => self.slash_sel = (self.slash_sel + 1).min(slash.len() - 1),
            KeyCode::Tab if !slash.is_empty() => {
                let cmd = slash[self.slash_sel.min(slash.len() - 1)];
                self.set_input(&format!("{}{}", cmd.name, if cmd.takes_text { " " } else { "" }));
            }
            KeyCode::Enter if !slash.is_empty() && !alt => {
                let cmd = slash[self.slash_sel.min(slash.len() - 1)];
                self.run_cmd(cmd);
            }
            KeyCode::Enter if alt => self.insert("\n"),
            KeyCode::Char('j') if ctrl => self.insert("\n"),
            KeyCode::Enter => self.submit(),
            KeyCode::Up if !self.input.contains(&'\n') => self.history_step(true),
            KeyCode::Down if !self.input.contains(&'\n') => self.history_step(false),
            KeyCode::PageUp => self.scroll += self.page,
            KeyCode::PageDown => self.scroll = self.scroll.saturating_sub(self.page),
            KeyCode::End if ctrl => self.scroll = 0,
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => self.cursor = (self.cursor + 1).min(self.input.len()),
            KeyCode::Home => self.cursor = self.input[..self.cursor].iter().rposition(|c| *c == '\n').map(|i| i + 1).unwrap_or(0),
            KeyCode::End => self.cursor = self.input[self.cursor..].iter().position(|c| *c == '\n').map(|i| self.cursor + i).unwrap_or(self.input.len()),
            KeyCode::Char('a') if ctrl => self.cursor = 0,
            KeyCode::Char('e') if ctrl => self.cursor = self.input.len(),
            KeyCode::Char('u') if ctrl => {
                self.input.drain(..self.cursor);
                self.cursor = 0;
            }
            KeyCode::Char('w') if ctrl => {
                let mut start = self.cursor;
                while start > 0 && self.input[start - 1].is_whitespace() {
                    start -= 1;
                }
                while start > 0 && !self.input[start - 1].is_whitespace() {
                    start -= 1;
                }
                self.input.drain(start..self.cursor);
                self.cursor = start;
            }
            KeyCode::Backspace if self.cursor > 0 => {
                self.cursor -= 1;
                self.input.remove(self.cursor);
                self.slash_sel = 0;
            }
            KeyCode::Delete if self.cursor < self.input.len() => {
                self.input.remove(self.cursor);
            }
            KeyCode::Char(c) if !ctrl && !alt => self.insert(&c.to_string()),
            _ => {}
        }
    }

    fn save_shot(&mut self, svg: String) {
        let dir = self.ctx.root.join("screens");
        let _ = fs::create_dir_all(&dir);
        let n = fs::read_dir(&dir).map(|d| d.count()).unwrap_or(0) + 1;
        let path = dir.join(format!("screen-{}.svg", n));
        match fs::write(&path, svg) {
            Ok(_) => {
                let (full, home) = (path.display().to_string(), crate::platform::home_str());
                let short = if !home.is_empty() && full.starts_with(&home) { format!("~{}", &full[home.len()..]) } else { full };
                self.toast(format!("screen saved as {}", short));
            }
            Err(e) => self.items.push(Item::Error(format!("the screen could not be saved: {}", e))),
        }
    }
}

fn help_text() -> String {
    let mut out = String::from("Keys\n\
        enter            send the request\n\
        alt+enter        new line in the request (also ctrl+j)\n\
        up, down         earlier and later requests\n\
        page up, down    scroll the transcript (also the mouse wheel)\n\
        esc              stop the running turn, or empty the input\n\
        ctrl+c           empty the input, leave Penelope when it is empty\n\
        shift+drag       mark text with the mouse\n\nCommands\n");
    for c in COMMANDS {
        out.push_str(&format!("{:<10} {:<8} {}\n", c.name, c.key, c.help));
    }
    out.push_str("\nA command outside the safe list waits for an answer: y runs it once, a allows every command in this session, n refuses.\n");
    out.push_str("A mod may move the keys of the main actions (never esc, ctrl+c, f5 or f6): /mod list shows the mods, /mod on NAME applies one.\n");
    out
}

/// Runs the interface until the person leaves it.
pub fn run(ctx: Ctx, resume: Option<serde_json::Value>) -> Result<(), String> {
    art::detect_colour();
    picture::detect();
    let mut painter = picture::Painter::default();
    let imported = plugins::import_known(&ctx);
    let mut app = App::new(ctx);
    for line in imported {
        app.note("◆", "plugins", line, art::SEA);
    }
    if let Some(entry) = resume {
        app.open_session(&entry);
    }
    std::env::set_var("PENELOPE_APPROVE_DIR", &app.approve_dir);
    let mut terminal = ratatui::init();
    let _ = execute!(std::io::stdout(), EnableMouseCapture, EnableBracketedPaste);
    let result = (|| -> Result<(), String> {
        loop {
            app.drain();
            app.poll_approvals();
            let frame = terminal.draw(|f| view::draw(f, &mut app)).map_err(|e| e.to_string())?;
            if app.shot {
                app.shot = false;
                let svg = art::buffer_svg(frame.buffer);
                app.save_shot(svg);
            }
            let _ = painter.sync(&mut std::io::stdout());
            if app.quit {
                let users: Vec<String> = app.items.iter().filter_map(|i| if let Item::User(t) = i { Some(t.clone()) } else { None }).collect();
                let _ = tools::consolidate(&app.ctx, &users);
                app.save_session();
                return Ok(());
            }
            if let Some(provider) = app.login.take() {   // Claude login or an API key needs the plain terminal
                let _ = execute!(std::io::stdout(), DisableMouseCapture, DisableBracketedPaste);
                ratatui::restore();
                match crate::auth::login(&provider) {
                    Ok(text) => println!("{}", text),
                    Err(e) => eprintln!("{}", e),
                }
                println!("press enter to return to the interface");
                let _ = std::io::stdin().read_line(&mut String::new());
                terminal = ratatui::init();
                let _ = execute!(std::io::stdout(), EnableMouseCapture, EnableBracketedPaste);
                app.toast("login done".to_string());
                app.refresh_side();
                continue;
            }
            if app.setup {                       // the setup asks on the plain terminal, then the interface comes back
                app.setup = false;
                painter.end(&mut std::io::stdout());
                let _ = execute!(std::io::stdout(), DisableMouseCapture, DisableBracketedPaste);
                ratatui::restore();
                app.ctx.cfg = crate::setup::wizard(&mut std::io::stdin().lock(), &mut std::io::stdout(), app.ctx.cfg.clone());
                save_config(&app.ctx);
                terminal = ratatui::init();
                let _ = execute!(std::io::stdout(), EnableMouseCapture, EnableBracketedPaste);
                app.toast("settings saved".to_string());
                app.refresh_side();
                continue;
            }
            if event::poll(Duration::from_millis(80)).map_err(|e| e.to_string())? {
                loop {
                    match event::read().map_err(|e| e.to_string())? {
                        Event::Key(k) if k.kind != KeyEventKind::Release => app.key(k),
                        Event::Paste(s) => app.paste(&s),
                        Event::Resize(..) => painter.reset(&mut std::io::stdout()),
                        Event::Mouse(m) => match m.kind {
                            MouseEventKind::ScrollUp => app.scroll += 3,
                            MouseEventKind::ScrollDown => app.scroll = app.scroll.saturating_sub(3),
                            _ => {}
                        },
                        _ => {}
                    }
                    if !event::poll(Duration::from_millis(0)).map_err(|e| e.to_string())? {
                        break;
                    }
                }
            }
            app.tick += 1;
        }
    })();
    if app.busy.is_some() {
        app.stop_turn();
    }
    painter.end(&mut std::io::stdout());
    let _ = execute!(std::io::stdout(), DisableMouseCapture, DisableBracketedPaste);
    ratatui::restore();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn app() -> App {
        let root = std::env::temp_dir().join(format!("penelope-test-{}-{:?}", std::process::id(), std::thread::current().id()));
        let _ = fs::remove_dir_all(&root);
        App::new(crate::config::load(&root))
    }

    fn screen(app: &mut App, w: u16, h: u16) -> String {
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        let frame = t.draw(|f| view::draw(f, app)).unwrap();
        let buf = frame.buffer;
        (0..h).map(|y| (0..w).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>()).collect::<Vec<_>>().join("\n")
    }

    fn press(app: &mut App, code: KeyCode) {
        app.key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    fn type_text(app: &mut App, text: &str) {
        for c in text.chars() {
            press(app, KeyCode::Char(c));
        }
    }

    #[test]
    fn every_size_draws() {
        let mut a = app();
        for (w, h) in [(20, 5), (40, 12), (80, 24), (100, 30), (120, 40), (200, 60)] {
            screen(&mut a, w, h);
        }
        a.items.push(Item::User("how do commits work in this repo?".into()));
        a.items.push(Item::Tool { id: "1".into(), name: "run".into(), detail: "git log --oneline".into(), state: ToolState::Running, summary: String::new() });
        a.items.push(Item::Reply("# Commits\n\n- English\n- no emojis\n\n```\ngit commit -m \"text\"\n```".into()));
        a.items.push(Item::Error("the claude command ended without a result".into()));
        a.busy = Some(Instant::now());
        for overlay in 0..5 {
            a.overlay = match overlay {
                0 => Overlay::None,
                1 => Overlay::Palette { query: "m".into(), sel: 1 },
                2 => Overlay::Providers { sel: 0, target: "weak".into(), scroll: 0 },
                3 => Overlay::Pager { title: "Help".into(), lines: help_text().lines().map(String::from).collect(), scroll: 2 },
                _ => Overlay::Approve { id: "x".into(), command: "rm -rf target && cargo build".into(), cwd: "/tmp".into(), file: overlay % 2 == 0,
                                        diff: crate::files::diff_text("a\nb\n", "a\nc\nd\n").lines().map(String::from).collect() },
            };
            for (w, h) in [(20, 5), (40, 12), (80, 24), (100, 30), (120, 40), (200, 60)] {
                screen(&mut a, w, h);
            }
        }
    }

    #[test]
    fn text_grows_piece_by_piece_and_the_finished_block_replaces_it() {
        let mut a = app();
        for piece in [" Two", " files", ": a"] {
            a.tx.send(Msg::Turn(TurnEvent::Delta(piece.into()))).unwrap();
        }
        a.drain();
        assert!(matches!(a.items.as_slice(), [Item::Reply(t)] if t == "Two files: a"));
        a.tx.send(Msg::Turn(TurnEvent::Text("Two files: a and b.".into()))).unwrap();
        a.tx.send(Msg::Turn(TurnEvent::Delta("Next".into()))).unwrap();
        a.drain();
        assert!(matches!(a.items.as_slice(), [Item::Reply(t), Item::Reply(n)] if t == "Two files: a and b." && n == "Next"));
    }

    #[test]
    fn home_shows_the_wordmark() {
        let mut a = app();
        let text = screen(&mut a, 120, 40);
        assert!(text.contains("P E N E L O P E"), "{}", text);
    }

    #[test]
    fn home_shows_the_input_and_chat_shows_the_transcript() {
        let mut a = app();
        let home = screen(&mut a, 120, 40);
        assert!(home.contains("Ask Penelope anything"), "{}", home);
        a.items.push(Item::User("list the files".into()));
        a.items.push(Item::Reply("Three files: **a**, `b` and c.".into()));
        let chat = screen(&mut a, 120, 40);
        assert!(chat.contains("list the files") && chat.contains("Three files: a, b and c."), "{}", chat);
        assert!(chat.contains("MEMORY") && chat.contains("awake") && chat.contains("CREW") && chat.contains("utis"), "{}", chat);
        let narrow = screen(&mut a, 70, 24);
        assert!(!narrow.contains("MEMORY") && narrow.contains("[#|#]"), "{}", narrow);
    }

    #[test]
    fn slash_popup_completes_and_runs() {
        let mut a = app();
        type_text(&mut a, "/ma");
        assert_eq!(a.slash_matches().iter().map(|c| c.name).collect::<Vec<_>>(), vec!["/map"]);
        assert!(screen(&mut a, 100, 30).contains("show the memory map"));
        press(&mut a, KeyCode::Enter);
        assert!(matches!(a.overlay, Overlay::Pager { .. }));
        press(&mut a, KeyCode::Esc);
        type_text(&mut a, "/no");
        press(&mut a, KeyCode::Tab);
        assert_eq!(a.input_text(), "/note ");
        type_text(&mut a, "tests run with cargo test");
        press(&mut a, KeyCode::Enter);
        assert!(matches!(a.items.last(), Some(Item::Note { label: "learned", .. })));
        assert_eq!(a.mood(), Mood::Learned);
        assert!(a.side.waypoints >= 1);
    }

    #[test]
    fn editing_keys_work() {
        let mut a = app();
        type_text(&mut a, "hello world");
        a.key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL));
        assert_eq!(a.input_text(), "hello ");
        a.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT));
        type_text(&mut a, "x");
        assert_eq!(a.input_text(), "hello \nx");
        press(&mut a, KeyCode::Home);
        press(&mut a, KeyCode::Delete);
        assert_eq!(a.input_text(), "hello \n");
        a.key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(a.input.is_empty() && !a.quit);
        a.key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(a.quit);
    }

    #[test]
    fn approval_is_answered_through_files() {
        let mut a = app();
        fs::write(a.approve_dir.join("7.req"), "{\"command\":\"rm x\",\"cwd\":\"\"}").unwrap();
        a.poll_approvals();
        assert!(matches!(&a.overlay, Overlay::Approve { command, .. } if command == "rm x"));
        assert_eq!(a.mood(), Mood::Asking);
        press(&mut a, KeyCode::Char('n'));
        assert_eq!(fs::read_to_string(a.approve_dir.join("7.ans")).unwrap(), "no");
        fs::write(a.approve_dir.join("8.req"), "{\"command\":\"rm y\",\"cwd\":\"\"}").unwrap();
        a.poll_approvals();
        press(&mut a, KeyCode::Char('a'));
        fs::write(a.approve_dir.join("9.req"), "{\"command\":\"rm z\",\"cwd\":\"\"}").unwrap();
        a.poll_approvals();
        assert_eq!(fs::read_to_string(a.approve_dir.join("9.ans")).unwrap(), "yes");
        assert!(matches!(a.overlay, Overlay::None));
    }

    #[test]
    fn the_file_dialog_shows_the_diff_in_two_colours_and_clips_it() {
        let mut a = app();
        let old: String = (1..=20).map(|i| format!("old {}\n", i)).collect();
        let req = serde_json::json!({"kind": "file", "command": "edit ~/a.txt at line 1", "cwd": "", "diff": crate::files::diff_text(&old, "new 1\nnew 2\n")});
        fs::write(a.approve_dir.join("5.req"), req.to_string()).unwrap();
        a.poll_approvals();
        assert!(matches!(&a.overlay, Overlay::Approve { file: true, diff, .. } if diff.len() == 15));
        let mut t = Terminal::new(TestBackend::new(120, 40)).unwrap();
        let frame = t.draw(|f| view::draw(f, &mut a)).unwrap();
        let rows: Vec<String> = (0..40).map(|y| (0..120).map(|x| frame.buffer[(x, y)].symbol().to_string()).collect()).collect();
        let colour_of = |text: &str| {
            let (y, row) = rows.iter().enumerate().find(|(_, r)| r.contains(text)).unwrap_or_else(|| panic!("no row with {}", text));
            frame.buffer[(row.chars().take(row.find(text).unwrap()).count() as u16, y as u16)].fg
        };
        assert_eq!(colour_of("-    1  old 1"), art::col(art::RED));
        assert_eq!(colour_of("+    2  new 2"), art::col(art::CLAY));
        assert!(rows.iter().any(|r| r.contains("8 more removed lines")), "{}", rows.join("\n"));
        let small = screen(&mut a, 80, 20);
        assert!(small.contains("-    1  old 1") && small.contains("more changed lines") && !small.contains("+    2  new 2"), "{}", small);
    }

    #[test]
    fn several_commands_in_one_prompt_run_in_order() {
        let mut a = app();
        type_text(&mut a, "/note tests run with cargo test;;/cost");
        press(&mut a, KeyCode::Enter);
        assert!(a.items.iter().any(|i| matches!(i, Item::Note { label: "learned", .. })), "the note was not stored");
        assert!(matches!(&a.overlay, Overlay::Pager { title, .. } if title == "Cost in euro"), "the cost page did not open");
        assert!(a.side.waypoints >= 1);
    }

    #[test]
    fn the_brain_opens_as_a_graph_and_the_context_preview_shows_the_block() {
        let mut a = app();
        a.command("/note rust errors use anyhow");
        a.command("/brain");
        let text = screen(&mut a, 110, 36);
        assert!(text.contains("memory map as a graph") && text.contains("waypoints") && text.contains("● notes"), "{}", text);
        let file = std::env::temp_dir().join(format!("penelope-tui-context-{}.txt", std::process::id()));
        fs::write(&file, "one\ntwo\n").unwrap();
        a.overlay = Overlay::None;
        a.command(&format!("/context @{}", file.display()));
        assert!(matches!(&a.overlay, Overlay::Pager { title, .. } if title == "Context preview"));
        let _ = fs::remove_file(file);
    }

    #[test]
    fn model_and_providers_are_two_separate_screens() {
        let mut a = app();
        a.command("/model");
        assert!(matches!(a.overlay, Overlay::Models { .. }));
        a.command("/providers");
        assert!(matches!(a.overlay, Overlay::Providers { .. }));
        a.command("/models");
        assert!(matches!(a.overlay, Overlay::Providers { .. }));        // the old command only points at the two new ones
        let text = screen(&mut a, 120, 40);
        assert!(text.contains("Providers · connections"), "{}", text);
    }

    #[test]
    fn the_provider_list_opens_a_masked_key_window_and_a_claude_login() {
        let mut a = app();
        a.command("/providers");
        assert!(matches!(a.overlay, Overlay::Providers { .. }));
        let at = crate::providers::TABLE.iter().position(|p| p.name == "anthropic").unwrap();
        a.overlay = Overlay::Providers { sel: at, target: "main".into(), scroll: 0 };
        press(&mut a, KeyCode::Enter);
        match &a.overlay {
            Overlay::ApiKey { name, input, .. } => {
                assert_eq!((name.as_str(), input.len()), ("anthropic", 0));
            }
            _ => panic!("the API provider must open the key window"),
        }
        a.paste("sk-abcdefghijklmnop");
        assert!(a.input_text().is_empty());                  // the paste must not land in the prompt
        let shown = screen(&mut a, 120, 40);
        assert!(shown.contains("sk-a") && !shown.contains("sk-abcdefghijklmnop"), "{}", shown);
        press(&mut a, KeyCode::Esc);
        a.overlay = Overlay::Providers { sel: 0, target: "main".into(), scroll: 0 };
        press(&mut a, KeyCode::Enter);
        assert_eq!(a.login.as_deref(), Some("claude"));
        assert!(matches!(a.overlay, Overlay::None));
    }

    #[test]
    fn delete_removes_the_selected_session_from_the_list() {
        let mut a = app();
        fs::write(a.ctx.root.join("sessions.json"), serde_json::json!([
            {"id": "one", "title": "first", "started": 1, "last": 2, "turns": 1},
            {"id": "two", "title": "second", "started": 3, "last": 4, "turns": 2}
        ]).to_string()).unwrap();
        a.refresh_tabs();
        a.overlay = Overlay::Sessions { sel: 0 };
        press(&mut a, KeyCode::Delete);
        assert_eq!(a.tabs.len(), 1);
        assert!(a.tabs.iter().all(|(id, _)| id == "one"));
        assert!(crate::sessions::all(&a.ctx).iter().all(|s| s["id"] == "one"));
    }

    #[test]
    fn a_turn_checkpoint_rewinds_the_transcript_and_turn_counter() {
        let mut a = app();
        a.turns = 0;
        a.items.push(Item::User("first".into()));
        a.save_checkpoint();
        a.turns = 2;
        a.items.push(Item::Reply("second".into()));
        a.items.push(Item::User("third".into()));
        let said = a.rewind(2);
        assert!(said.contains("turn 0"), "{}", said);
        assert!(a.turns == 0 && a.items.len() == 1 && matches!(&a.items[0], Item::User(t) if t == "first"));
    }

    #[test]
    fn the_cluster_commands_show_the_tree_and_the_profiles() {
        let mut a = app();
        a.command("/clusters");
        match &a.overlay {
            Overlay::Pager { lines, .. } => assert!(lines.join("\n").contains("no cluster yet")),
            _ => panic!("clusters must open the pager"),
        }
        a.command("/cluster profiles");
        match &a.overlay {
            Overlay::Pager { lines, .. } => {
                let text = lines.join("\n");
                assert!(text.contains("coding") && text.contains("backend (worker)") && text.contains("live-test"), "{}", text);
            }
            _ => panic!("profiles must open the pager"),
        }
    }

    #[test]
    fn the_plugins_screen_lists_a_plugin_and_inserts_its_command() {
        let mut a = app();
        let dir = a.ctx.root.join("plug");
        fs::create_dir_all(dir.join("commands")).unwrap();
        fs::write(dir.join("commands/fix.md"), "---\nname: fix\ndescription: Fix one file\nargument-hint: FILE\n---\nFix $1.\n").unwrap();
        assert!(plugins::add(&a.ctx, &dir.to_string_lossy()).contains("1 commands"));
        a.refresh_side();
        a.command("/plugins");
        assert!(matches!(a.overlay, Overlay::Plugins { .. }));
        let text = screen(&mut a, 120, 40);
        assert!(text.contains("Plugins") && text.contains("plug") && text.contains("1c"), "{}", text);
        assert_eq!((a.plugin_matches("", 0).len(), a.plugin_matches("", 1).len(), a.plugin_matches("fix", 0).len()), (1, 1, 1));
        press(&mut a, KeyCode::Char('f'));
        assert!(matches!(&a.overlay, Overlay::Plugins { filter, .. } if filter == "f"));
        press(&mut a, KeyCode::Tab);
        assert!(matches!(&a.overlay, Overlay::Plugins { mode, .. } if *mode == 1));
        press(&mut a, KeyCode::Enter);
        assert!(matches!(a.overlay, Overlay::PluginCommands { .. }));
        press(&mut a, KeyCode::Enter);
        assert_eq!(a.input_text(), "/plug:fix ");
        assert!(matches!(a.overlay, Overlay::None));
        a.command("/help");
        match &a.overlay {
            Overlay::Pager { lines, .. } => {
                let text = lines.join("\n");
                assert!(text.contains("Plugin commands") && text.contains("/plug:fix"), "{}", text);
            }
            _ => panic!("help must open the pager"),
        }
    }

    #[test]
    fn the_picker_uses_the_provider_list_and_its_effort_column() {
        let mut a = app();
        a.ctx.cfg["main"] = serde_json::json!({"kind": "openai", "base_url": "https://opencode.ai/zen/go/v1",
                                               "model": "deepseek-v4.1-flash", "effort": "low"});
        a.model_cache.insert("opencode".to_string(), vec![
            crate::providers::ModelInfo { id: "from-provider".into(), label: "From Provider".into(), efforts: vec!["low".into(), "high".into()] },
        ]);
        assert_eq!(a.model_choices("main"), vec!["deepseek-v4.1-flash".to_string(), "from-provider".to_string()]);
        assert_eq!(a.model_efforts("main", "from-provider"), vec!["low".to_string(), "high".to_string()]);
        a.overlay = Overlay::Models { sel: 0, target: "main".into() };
        press(&mut a, KeyCode::Char('e'));
        assert_eq!(a.ctx.cfg["main"]["effort"], serde_json::json!("high"));
    }

    #[test]
    fn a_provider_choice_sets_effort_and_the_effort_key_cycles_it() {
        let mut a = app();
        a.select_provider("opencode", "main");
        assert!(a.ctx.cfg["main"]["effort"].is_null());          // mimo-v2.6-flash has no effort field
        a.ctx.cfg["main"] = serde_json::json!({"kind": "openai", "base_url": "https://opencode.ai/zen/go/v1",
                                               "model": "deepseek-v4.1-flash", "effort": "low"});
        a.overlay = Overlay::Models { sel: 0, target: "main".into() };
        press(&mut a, KeyCode::Char('e'));
        assert_eq!(a.ctx.cfg["main"]["effort"], serde_json::json!("high"));      // low -> high for deepseek-v4.1-flash
        press(&mut a, KeyCode::Char('e'));
        assert_eq!(a.ctx.cfg["main"]["effort"], serde_json::json!("max"));
        press(&mut a, KeyCode::Char('e'));
        assert_eq!(a.ctx.cfg["main"]["effort"], serde_json::json!("low"));       // wraps to the first level
        a.select_provider("claude-cli", "main");
        assert_eq!((a.ctx.cfg["main"]["model"].as_str(), a.ctx.cfg["main"]["effort"].as_str()), (Some("claude-sonnet-5-5"), Some("high")));
    }

    #[test]
    fn roles_have_models_and_opencode_and_claude_names_are_exact() {
        let mut a = app();
        a.command("/model");
        press(&mut a, KeyCode::Char('r'));
        assert!(matches!(a.overlay, Overlay::Roles { .. }));
        press(&mut a, KeyCode::Enter);
        match &a.overlay {
            Overlay::Providers { target, .. } => assert_eq!(target, "tool"),
            _ => panic!("the role must open the provider list"),
        }
        a.ctx.cfg["main"] = serde_json::json!({"kind": "openai", "base_url": "https://opencode.ai/zen/go/v1", "model": "mimo-v2.6-flash", "api_key_env": ""});
        let opencode = a.model_choices("main");
        assert!(opencode.contains(&"deepseek-v4.1-flash".to_string()) && opencode.len() > 10, "{:?}", opencode);
        a.ctx.cfg["main"] = serde_json::json!({"kind": "claude-cli", "model": "sonnet"});
        let claude = a.model_choices("main");
        assert!(claude.contains(&"claude-sonnet-5-5".to_string()) && claude.contains(&"claude-opus-5-5".to_string()), "{:?}", claude);
    }

    #[test]
    fn export_price_forget_and_learn_work() {
        let mut a = app();
        a.command("/note rust errors use anyhow in the binary");
        a.command("/forget use anyhow in the binary");
        assert!(a.items.iter().any(|i| matches!(i, Item::Note { label: "unravel", .. })));
        a.command("/price test-model 1.5 4.5");
        assert_eq!(a.ctx.cfg["prices"]["test-model"][1], 4.5);
        a.command("/export");
        let exported = a.ctx.root.join("exports/new.md");
        assert!(exported.is_file() && fs::read_to_string(&exported).unwrap().contains("Penelope session"), "{}", exported.display());
        a.command("/learn");
        assert!(matches!(a.overlay, Overlay::Pager { .. }));
    }

    #[test]
    fn the_screen_exports_as_svg() {
        let mut a = app();
        let mut t = Terminal::new(TestBackend::new(100, 30)).unwrap();
        let frame = t.draw(|f| view::draw(f, &mut a)).unwrap();
        let svg = art::buffer_svg(frame.buffer);
        assert!(svg.starts_with("<svg") && svg.contains(">A</text>") && svg.ends_with("</svg>\n"));
    }

    fn put_mod(a: &App, folder: &str, manifest: serde_json::Value) {
        let dir = mods::mods_dir(&a.ctx).join(folder);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(mods::MANIFEST), manifest.to_string()).unwrap();
    }

    #[test]
    fn a_mod_changes_theme_layout_and_keys_and_off_restores_them() {
        let mut a = app();
        a.items.push(Item::User("hello".into()));
        a.items.push(Item::Reply("a reply".into()));
        assert!(screen(&mut a, 120, 40).contains("MEMORY"));
        put_mod(&a, "evening", serde_json::json!({
            "name": "Evening", "theme": {"ink": "#010203", "clay": "#AABBCC"},
            "layout": "top", "keys": {"sidebar": "ctrl+y"}
        }));
        a.command("/mod on evening");
        assert_eq!(a.palette.ink, (1, 2, 3));
        assert_eq!(a.palette.clay, (0xAA, 0xBB, 0xCC));
        assert_eq!(a.layout, mods::Layout::Top);
        assert!(!a.sidebar && !a.layout.shows_sidebar());
        assert_eq!(a.keys.action_for("ctrl+y"), Some(mods::Action::Sidebar));
        assert_eq!(a.mod_name.as_deref(), Some("evening"));
        let themed = screen(&mut a, 120, 40);
        assert!(!themed.contains("MEMORY") && themed.contains("a reply"), "{}", themed);

        // the palette reaches the buffer: the ground is drawn in the ink of the mod
        let mut t = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let frame = t.draw(|f| view::draw(f, &mut a)).unwrap();
        assert_eq!(frame.buffer[(0, 0)].bg, art::col((1, 2, 3)));

        // the moved key works and the old default is silent
        a.key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::CONTROL));
        assert!(a.sidebar);
        a.key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
        assert!(a.sidebar, "ctrl+b was moved away and must not toggle the panel");

        a.command("/mod off");
        assert_eq!(a.palette, art::Palette::DEFAULT);
        assert_eq!(a.layout, mods::Layout::Side);
        assert!(a.sidebar);
        assert_eq!(a.keys.action_for("ctrl+b"), Some(mods::Action::Sidebar));
        assert_eq!(a.keys.action_for("ctrl+y"), None);
        assert!(a.mod_name.is_none());
        assert!(screen(&mut a, 120, 40).contains("MEMORY"));
    }

    #[test]
    fn a_broken_manifest_is_refused_and_leaves_the_running_mod_alone() {
        let mut a = app();
        put_mod(&a, "good", serde_json::json!({"name": "Good", "theme": {"sea": "#123456"}, "layout": "compact"}));
        a.command("/mod on good");
        assert_eq!(a.palette.sea, (0x12, 0x34, 0x56));
        put_mod(&a, "broken", serde_json::json!({"name": "Broken", "layout": "wide"}));
        a.command("/mod on broken");
        let error = a.items.iter().rev().find_map(|i| if let Item::Error(t) = i { Some(t.clone()) } else { None }).unwrap_or_default();
        assert!(error.contains("broken") && error.contains("not side, top or compact"), "{}", error);
        assert_eq!(a.mod_name.as_deref(), Some("good"));
        assert_eq!(a.palette.sea, (0x12, 0x34, 0x56));
        assert_eq!(a.layout, mods::Layout::Compact);
        // an unreadable manifest is a line, not a crash
        let dir = mods::mods_dir(&a.ctx).join("garbage");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(mods::MANIFEST), "{nope").unwrap();
        a.command("/mod on garbage");
        let error = a.items.iter().rev().find_map(|i| if let Item::Error(t) = i { Some(t.clone()) } else { None }).unwrap_or_default();
        assert!(error.contains("not valid JSON"), "{}", error);
        assert_eq!(a.mod_name.as_deref(), Some("good"));
    }

    #[test]
    fn a_mod_written_by_the_model_changes_the_picture_and_off_brings_the_default_back() {
        let mut a = app();
        let (name, prompt) = mods::new_request(&a.ctx, "use this picture as background");
        assert!(prompt.contains(".penelope-mod.json") && prompt.contains("picture"), "{}", prompt);
        let dir = mods::mods_dir(&a.ctx).join(&name);
        fs::write(dir.join("picture.png"), b"\x89PNG\r\n\x1a\nfake model picture").unwrap();
        fs::write(dir.join(mods::MANIFEST), serde_json::json!({
            "name": name, "picture": "picture.png", "layout": "side"
        }).to_string()).unwrap();
        picture::use_default();
        a.command(&format!("/mod on {}", name));
        assert_eq!(a.mod_name.as_deref(), Some(name.as_str()));
        assert!(picture::custom());
        assert_eq!(mods::active(&a.ctx).as_deref(), Some(name.as_str()));
        a.command("/mod off");
        assert!(!picture::custom());
        assert!(a.mod_name.is_none());
        assert_eq!(mods::active(&a.ctx), None);
    }

    #[test]
    fn a_missing_picture_falls_back_to_the_default_with_a_line() {
        let mut a = app();
        put_mod(&a, "lost", serde_json::json!({"name": "Lost", "picture": "not-here.png"}));
        picture::use_default();
        a.command("/mod on lost");
        assert_eq!(a.mod_name.as_deref(), Some("lost"));
        assert!(!picture::custom());
        let line = a.items.iter().find_map(|i| if let Item::Note { text, .. } = i { Some(text.clone()) } else { None }).unwrap_or_default();
        assert!(line.contains("missing") && line.contains("default picture"), "{}", line);
    }

    #[test]
    fn a_saved_mod_comes_back_at_the_next_start() {
        let mut a = app();
        put_mod(&a, "saved", serde_json::json!({"name": "Saved", "theme": {"gold": "#010101"}, "layout": "top"}));
        a.command("/mod on saved");
        let root = a.ctx.root.clone();
        drop(a);
        let mut b = App::new(crate::config::load(&root));
        assert_eq!(b.mod_name.as_deref(), Some("saved"));
        assert_eq!(b.palette.gold, (1, 1, 1));
        assert_eq!(b.layout, mods::Layout::Top);
        b.command("/mod off");
        assert_eq!(b.palette, art::Palette::DEFAULT);
    }

    #[test]
    fn mod_list_and_help_show_the_mod_command() {
        let mut a = app();
        put_mod(&a, "listed", serde_json::json!({"name": "Listed", "description": "a test mod", "layout": "compact"}));
        a.command("/mod list");
        match &a.overlay {
            Overlay::Pager { title, lines, .. } => {
                assert_eq!(title, "Mods");
                let text = lines.join("\n");
                assert!(text.contains("listed") && text.contains("a test mod") && text.contains("compact"), "{}", text);
            }
            _ => panic!("mods must open the pager"),
        }
        a.command("/help");
        match &a.overlay {
            Overlay::Pager { lines, .. } => {
                let text = lines.join("\n");
                assert!(text.contains("/mod") && text.contains("mods: list"), "{}", text);
            }
            _ => panic!("help must open the pager"),
        }
        assert!(COMMANDS.iter().any(|c| c.name == "/mod"));
    }

    #[test]
    fn factory_list_and_help_show_the_factory_command() {
        let mut a = app();
        a.command("/factory list");
        match &a.overlay {
            Overlay::Pager { title, lines, .. } => {
                assert_eq!(title, "Factories");
                let text = lines.join("\n");
                assert!(text.contains("no factory yet") && text.contains("/factory new"), "{}", text);
            }
            _ => panic!("factories must open the pager"),
        }
        a.command("/help");
        match &a.overlay {
            Overlay::Pager { lines, .. } => {
                let text = lines.join("\n");
                assert!(text.contains("/factory") && text.contains("factories: new"), "{}", text);
            }
            _ => panic!("help must open the pager"),
        }
        assert!(COMMANDS.iter().any(|c| c.name == "/factory"));
    }
}
