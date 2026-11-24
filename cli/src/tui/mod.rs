//! The full screen interface of penelope: a transcript, an input box, a side panel with the loom and its mood.
//!
//! One thread draws and reads keys. A turn of the main model runs in a second thread and reports through a channel.

mod art;
mod text;
mod view;

use crate::backend::{Chat, Event as TurnEvent};
use crate::config::Ctx;
use crate::memory::Memory;
use crate::{plugins, tools};
use art::Mood;
use ratatui::crossterm::event::{self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, Event, KeyCode,
                                KeyEvent, KeyEventKind, KeyModifiers, MouseEventKind};
use ratatui::crossterm::execute;
use std::collections::HashSet;
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

pub enum Overlay {
    None,
    Palette { query: String, sel: usize },
    Models { sel: usize, weak: bool },
    Pager { title: String, lines: Vec<String>, scroll: usize },
    Approve { id: String, command: String, cwd: String },
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
    Cmd { name: "/models", help: "choose the main and the weaker model", key: "ctrl+o", takes_text: false },
    Cmd { name: "/sessions", help: "list the stored sessions", key: "", takes_text: false },
    Cmd { name: "/crew", help: "the model families and what does each job today", key: "", takes_text: false },
    Cmd { name: "/map", help: "show the memory map", key: "", takes_text: false },
    Cmd { name: "/go", help: "follow the memory map to a topic", key: "", takes_text: true },
    Cmd { name: "/note", help: "store a rule or a fact in the memory map", key: "", takes_text: true },
    Cmd { name: "/unravel", help: "take back the last note of the memory map", key: "", takes_text: false },
    Cmd { name: "/stats", help: "how much tool output stayed away from the main model", key: "", takes_text: false },
    Cmd { name: "/plugins", help: "list the registered plugins", key: "", takes_text: false },
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
    Done(Box<Chat>, Result<String, String>),
    Learned(Vec<String>),
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
    /// Secrets the privacy filter masked in tool output.
    pub masked: u64,
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
            chat: Some(Chat::new()), cancel: Arc::new(AtomicBool::new(false)), pid: Arc::new(AtomicU32::new(0)), tx, rx,
            tokens: (0, 0), turns: 0, tool_calls: 0, trail: Vec::new(), learned: Vec::new(), side: Side::default(),
            approve_dir, allow_all: false, seen: HashSet::new(), shot: false, quit: false,
        };
        app.refresh_side();
        app
    }

    pub fn refresh_side(&mut self) {
        let map = tools::memory_map(&self.ctx);
        let lines: Vec<&str> = map.lines().collect();
        self.side.waypoints = lines.len().saturating_sub(2);
        self.side.walks = lines.last().and_then(|l| l.split_whitespace().next()).and_then(|n| n.parse().ok()).unwrap_or(0);
        let stats: serde_json::Value = fs::read_to_string(self.ctx.root.join("stats.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        for (i, tool) in ["run", "fetch"].iter().enumerate() {
            let (raw, sent) = (stats[tool]["raw_chars"].as_u64().unwrap_or(0), stats[tool]["sent_chars"].as_u64().unwrap_or(0));
            self.side.routed[i] = (stats[tool]["calls"].as_u64().unwrap_or(0), if raw > 0 { 100 - (sent * 100 / raw).min(100) } else { 0 });
        }
        self.side.masked = stats["privacy"]["masked"].as_u64().unwrap_or(0);
        self.side.screencodec = self.ctx.cfg["screencodec"]["command"].as_str().map(|c| !c.is_empty()).unwrap_or(false)
            || crate::config::which("screencodec-mcp").is_some();
        self.side.plugins = plugins::load(&self.ctx)["plugins"].as_array().map(|a| a.len()).unwrap_or(0);
    }

    pub fn mood(&self) -> Mood {
        if matches!(self.overlay, Overlay::Approve { .. }) {
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

    /// The model names the picker offers for the main or the weaker model.
    pub fn model_choices(&self, weak: bool) -> Vec<String> {
        let m = &self.ctx.cfg[if weak { "weak" } else { "main" }];
        let current = m["model"].as_str().unwrap_or("").to_string();
        let mut out: Vec<String> = if m["kind"].as_str().unwrap_or("claude-cli") == "openai" { Vec::new() } else {
            ["sonnet", "opus", "haiku"].iter().map(|s| s.to_string()).collect()
        };
        if !current.is_empty() && !out.contains(&current) {
            out.insert(0, current);
        }
        out
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

    fn start_turn(&mut self, text: String) {
        let mut chat = self.chat.take().unwrap_or_else(Chat::new);
        self.cancel = chat.cancel.clone();
        self.pid = chat.pid.clone();
        self.busy = Some(Instant::now());
        self.items.push(Item::User(text.clone()));
        self.scroll = 0;
        let (ctx, tx) = (self.ctx.clone(), self.tx.clone());
        std::thread::spawn(move || {
            let (notes, trail, cached) = Memory::open(&ctx.root).recall(&text, 1600);
            if !trail.is_empty() {
                let _ = tx.send(Msg::Trail(trail, cached));
            }
            let result = chat.turn_with(&ctx, &text, &notes, &mut |e| {
                let _ = tx.send(Msg::Turn(e));
            });
            let answer = result.clone().ok();
            let _ = tx.send(Msg::Done(Box::new(chat), result));
            if let Some(a) = answer {
                let ids = if ctx.cfg["auto_learn"].as_bool().unwrap_or(true) { tools::learn(&ctx, &text, &a) } else { Vec::new() };
                let _ = tx.send(Msg::Learned(ids));
            }
        });
    }

    fn stop_turn(&mut self) {
        self.cancel.store(true, Ordering::SeqCst);
        let pid = self.pid.load(Ordering::SeqCst);
        if pid != 0 {
            let _ = Command::new("kill").arg(pid.to_string()).stdout(Stdio::null()).stderr(Stdio::null()).status();
        }
    }

    fn drain(&mut self) {
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
                    self.note("◆", "outis", format!("{} masked in the request before the model read it", if n == 1 { "1 item".to_string() } else { format!("{} items", n) }), art::SEA);
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
                Msg::Done(chat, result) => {
                    self.tokens = (chat.input_tokens, chat.output_tokens);
                    self.turns = chat.turns;
                    self.chat = Some(*chat);
                    self.busy = None;
                    self.streaming = false;
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
                            let shown = matches!(self.items.last(), Some(Item::Reply(t)) if t.trim() == answer.trim());
                            if !shown && !answer.is_empty() {
                                self.items.push(Item::Reply(answer));
                            }
                            self.learning = self.ctx.cfg["auto_learn"].as_bool().unwrap_or(true);
                        }
                        Err(e) if e == "stopped" => self.note("■", "stopped", "the turn was stopped".to_string(), art::ASH),
                        Err(e) => self.items.push(Item::Error(e)),
                    }
                    self.scroll = 0;
                    self.refresh_side();
                }
                Msg::Learned(ids) => {
                    self.learning = false;
                    for id in ids {
                        self.note("✦", "learned", id.clone(), art::CLAY);
                        self.learned.push(id);
                        self.flash = Some((Mood::Learned, Instant::now()));
                    }
                    self.refresh_side();
                }
            }
        }
    }

    /// Looks for a question of the run tool: a command outside the safe list that waits for a yes.
    fn poll_approvals(&mut self) {
        if !matches!(self.overlay, Overlay::None) || self.tick % 3 != 0 {
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
            if self.allow_all {
                self.answer(id, true);
                continue;
            }
            self.overlay = Overlay::Approve { id: id.to_string(), command, cwd: v["cwd"].as_str().unwrap_or("").to_string() };
            return;
        }
    }

    fn answer(&mut self, id: &str, yes: bool) {
        let tmp = self.approve_dir.join(format!("{}.tmp", id));
        let _ = fs::write(&tmp, if yes { "yes" } else { "no" });
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
                self.items.clear();
                self.chat = Some(Chat::new());
                self.tokens = (0, 0);
                self.turns = 0;
                self.tool_calls = 0;
                self.trail.clear();
                self.learned.clear();
                self.scroll = 0;
            }
            "/models" => self.overlay = Overlay::Models { sel: 0, weak: false },
            "/map" => self.pager("Memory map", tools::memory_map(&self.ctx)),
            "/go" if !rest.is_empty() => self.pager("Memory map: the way", tools::memory_go(&self.ctx, rest)),
            "/note" if !rest.is_empty() => {
                let report = tools::memory_note(&self.ctx, "", rest);
                self.note("✦", "learned", report.trim_start_matches("noted at ").to_string(), art::CLAY);
                self.flash = Some((Mood::Learned, Instant::now()));
                self.refresh_side();
            }
            "/go" | "/note" => self.toast(format!("{} TEXT", name)),
            "/sessions" => self.pager("Sessions", crate::sessions::list(&self.ctx)),
            "/crew" => self.pager("Crew", tools::crew(&self.ctx)),
            "/unravel" => {
                let report = tools::unravel(&self.ctx, 1);
                self.note("↺", "unravel", report, art::SEA);
                self.refresh_side();
            }
            "/stats" => self.pager("Routed tool output", tools::stats(&self.ctx)),
            "/plugins" => self.pager("Plugins", plugins::list(&self.ctx)),
            "/doctor" => self.pager("Doctor", crate::doctor_text(&self.ctx)),
            "/copy" => self.copy_last(),
            "/shot" => self.shot = true,
            "/sidebar" => self.sidebar = !self.sidebar,
            "/help" => self.pager("Keys and commands", help_text()),
            other => self.toast(format!("no command {} (ctrl+p lists the commands)", other)),
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
        if text.starts_with('/') {
            self.command(&text);
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
        let mut pick: Option<(usize, bool)> = None;
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
            Overlay::Models { sel, weak } => match key.code {
                KeyCode::Esc => close = true,
                KeyCode::Up => *sel = sel.saturating_sub(1),
                KeyCode::Down => *sel += 1,
                KeyCode::Tab | KeyCode::Left | KeyCode::Right => {
                    *weak = !*weak;
                    *sel = 0;
                }
                KeyCode::Enter => pick = Some((*sel, *weak)),
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
            Overlay::Approve { id, .. } => match key.code {
                KeyCode::Char('y') | KeyCode::Enter => reply = Some((id.clone(), true, false)),
                KeyCode::Char('a') => reply = Some((id.clone(), true, true)),
                KeyCode::Char('n') | KeyCode::Esc => reply = Some((id.clone(), false, false)),
                _ => {}
            },
        }
        if let Overlay::Models { weak, .. } = &self.overlay {
            let last = self.model_choices(*weak).len().saturating_sub(1);
            if let Overlay::Models { sel, .. } = &mut self.overlay {
                *sel = (*sel).min(last);
            }
        }
        if let Some((sel, weak)) = pick {
            if let Some(model) = self.model_choices(weak).get(sel).cloned() {
                self.ctx.cfg[if weak { "weak" } else { "main" }]["model"] = serde_json::json!(model);
                save_config(&self.ctx);
                self.toast(format!("{} model is now {}", if weak { "weaker" } else { "main" }, model));
            }
            close = true;
        }
        if let Some((id, yes, always)) = reply {
            self.allow_all |= always;
            self.answer(&id, yes);
            close = true;
        }
        if close {
            self.overlay = Overlay::None;
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
            if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) && !matches!(self.overlay, Overlay::Approve { .. }) {
                self.overlay = Overlay::None;
                return;
            }
            self.overlay_key(key);
            return;
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
            KeyCode::Char('p') if ctrl => self.overlay = Overlay::Palette { query: String::new(), sel: 0 },
            KeyCode::Char('o') if ctrl => self.overlay = Overlay::Models { sel: 0, weak: false },
            KeyCode::Char('n') if ctrl => self.command("/new"),
            KeyCode::Char('b') if ctrl => self.sidebar = !self.sidebar,
            KeyCode::F(1) => self.command("/help"),
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
                let (full, home) = (path.display().to_string(), std::env::var("HOME").unwrap_or_default());
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
    out
}

/// Runs the interface until the person leaves it.
pub fn run(ctx: Ctx, resume: Option<(Chat, String)>) -> Result<(), String> {
    art::detect_colour();
    let mut app = App::new(ctx);
    if let Some((chat, title)) = resume {
        app.tokens = (chat.input_tokens, chat.output_tokens);
        app.turns = chat.turns;
        app.chat = Some(chat);
        app.note("↺", "resumed", title, art::SEA);
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
            if app.quit {
                return Ok(());
            }
            if event::poll(Duration::from_millis(80)).map_err(|e| e.to_string())? {
                loop {
                    match event::read().map_err(|e| e.to_string())? {
                        Event::Key(k) if k.kind != KeyEventKind::Release => app.key(k),
                        Event::Paste(s) => app.insert(&s.replace("\r\n", "\n").replace('\r', "\n")),
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
                2 => Overlay::Models { sel: 0, weak: true },
                3 => Overlay::Pager { title: "Help".into(), lines: help_text().lines().map(String::from).collect(), scroll: 2 },
                _ => Overlay::Approve { id: "x".into(), command: "rm -rf target && cargo build".into(), cwd: "/tmp".into() },
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
    fn home_shows_the_input_and_chat_shows_the_transcript() {
        let mut a = app();
        let home = screen(&mut a, 120, 40);
        assert!(home.contains("Ask Penelope anything"), "{}", home);
        a.items.push(Item::User("list the files".into()));
        a.items.push(Item::Reply("Three files: **a**, `b` and c.".into()));
        let chat = screen(&mut a, 120, 40);
        assert!(chat.contains("list the files") && chat.contains("Three files: a, b and c."), "{}", chat);
        assert!(chat.contains("MEMORY") && chat.contains("awake") && chat.contains("CREW") && chat.contains("outis"), "{}", chat);
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
    fn the_screen_exports_as_svg() {
        let mut a = app();
        let mut t = Terminal::new(TestBackend::new(100, 30)).unwrap();
        let frame = t.draw(|f| view::draw(f, &mut a)).unwrap();
        let svg = art::buffer_svg(frame.buffer);
        assert!(svg.starts_with("<svg") && svg.contains(">A</text>") && svg.ends_with("</svg>\n"));
    }
}
