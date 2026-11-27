//! The question dialog: the data of `ask_user` v2, the file protocol to the full screen interface, the state machine
//! of the dialog and the text the model reads back.
//!
//! The model sends 1 to 6 questions. Each has a short header, 2 to 6 options with an optional description, a flag for
//! several answers and a flag that lets the person type a free answer. The old form (`question` plus `options` as
//! plain texts) stays valid and is handled in `tools::ask_user`.
//!
//! The interface names a folder in PENELOPE_APPROVE_DIR. A question set is the file `ID.req` with `"kind":"questions"`,
//! the answer is the file `ID.ans`. The dialog itself (`Dialog`) holds no drawing code, so its keys are tested without
//! a terminal and the same state machine can run in another front end.

use crate::config::Ctx;
use ratatui::crossterm::event::KeyCode;
use serde_json::{json, Value};
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// Seconds the dialog waits for the person when `ask.timeout_secs` is not set.
pub const DEFAULT_TIMEOUT: u64 = 900;
pub const MAX_QUESTIONS: usize = 6;
const MAX_OPTIONS: usize = 6;

#[derive(Clone, Debug, PartialEq)]
pub struct Opt {
    pub label: String,
    pub description: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Question {
    pub question: String,
    pub header: String,
    pub options: Vec<Opt>,
    pub multi_select: bool,
    pub allow_text: bool,
}

/// The answer to one question: chosen labels, a typed text, or closed.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Answer {
    pub labels: Vec<String>,
    pub text: Option<String>,
    pub closed: bool,
}

impl Answer {
    pub fn closed() -> Answer {
        Answer { labels: Vec::new(), text: None, closed: true }
    }
    #[cfg(test)]
    pub fn chosen(label: &str) -> Answer {
        Answer { labels: vec![label.to_string()], text: None, closed: false }
    }
    #[cfg(test)]
    pub fn typed(text: &str) -> Answer {
        Answer { labels: Vec::new(), text: Some(text.to_string()), closed: false }
    }
    /// The answer as one line: labels joined, then the typed text.
    pub fn line(&self) -> String {
        if self.closed {
            return "closed".to_string();
        }
        let mut parts: Vec<String> = Vec::new();
        if !self.labels.is_empty() {
            parts.push(self.labels.join(", "));
        }
        if let Some(t) = self.text.as_ref().filter(|t| !t.trim().is_empty()) {
            parts.push(format!("typed: {}", t.trim()));
        }
        if parts.is_empty() { "closed".to_string() } else { parts.join(" + ") }
    }
    fn to_value(&self) -> Value {
        json!({"labels": self.labels, "text": self.text, "closed": self.closed})
    }
    fn from_value(v: &Value) -> Answer {
        Answer {
            labels: v["labels"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default(),
            text: v["text"].as_str().map(String::from),
            closed: v["closed"].as_bool().unwrap_or(false),
        }
    }
}

pub fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Seconds the person has: `ask.timeout_secs` from the configuration, 10 to 86400, default 900.
pub fn timeout_secs(ctx: &Ctx) -> u64 {
    ctx.cfg["ask"]["timeout_secs"].as_u64().unwrap_or(DEFAULT_TIMEOUT).clamp(10, 86_400)
}

fn short(text: &str, n: usize) -> String {
    text.trim().chars().take(n).collect()
}

fn parse_option(v: &Value) -> Option<Opt> {
    match v {
        Value::String(s) => Some(Opt { label: short(s, 70), description: String::new() }),
        Value::Object(_) => Some(Opt { label: short(v["label"].as_str().unwrap_or(""), 70), description: short(v["description"].as_str().unwrap_or(""), 160) }),
        _ => None,
    }
}

/// Reads the `questions` list of an `ask_user` call (or of a request file). Every fault names the question and the rule.
pub fn parse_questions(v: &Value) -> Result<Vec<Question>, String> {
    let list = v.as_array().ok_or("questions must be a list of 1 to 6 questions")?;
    if list.is_empty() || list.len() > MAX_QUESTIONS {
        return Err(format!("give 1 to {} questions, got {}", MAX_QUESTIONS, list.len()));
    }
    let mut out = Vec::new();
    for (i, q) in list.iter().enumerate() {
        let n = i + 1;
        let question = short(q["question"].as_str().unwrap_or(""), 4000);
        if question.is_empty() {
            return Err(format!("question {} has no text", n));
        }
        let options: Vec<Opt> = q["options"].as_array().map(|a| a.iter().filter_map(parse_option).collect()).unwrap_or_default();
        if options.len() < 2 || options.len() > MAX_OPTIONS || options.iter().any(|o| o.label.is_empty()) {
            return Err(format!("question {} needs 2 to {} options, each with a label", n, MAX_OPTIONS));
        }
        let mut labels: Vec<&String> = options.iter().map(|o| &o.label).collect();
        labels.sort();
        labels.dedup();
        if labels.len() != options.len() {
            return Err(format!("question {} repeats an option label", n));
        }
        let header = short(q["header"].as_str().unwrap_or(""), 16);
        out.push(Question {
            question,
            header: if header.is_empty() { format!("Q{}", n) } else { header },
            options,
            multi_select: q["multi_select"].as_bool().unwrap_or(false),
            allow_text: q["allow_text"].as_bool().unwrap_or(true),
        });
    }
    Ok(out)
}

pub fn questions_to_value(qs: &[Question]) -> Value {
    Value::Array(qs.iter().map(|q| json!({
        "question": q.question, "header": q.header, "multi_select": q.multi_select, "allow_text": q.allow_text,
        "options": q.options.iter().map(|o| json!({"label": o.label, "description": o.description})).collect::<Vec<_>>()
    })).collect())
}

/// How a question set ended.
#[derive(Debug, PartialEq)]
pub enum Reply {
    Answers(Vec<Answer>),
    Closed,
    TimedOut(u64),
    /// No interface can show the questions.
    Nobody,
    /// The front end could not show a dialog and gave the model a text to act on.
    Text(String),
}

/// The folder of the question files. The ACP server names its own in PENELOPE_ASK_DIR; the full screen interface uses
/// the approval folder.
pub fn dir() -> Option<std::path::PathBuf> {
    ["PENELOPE_ASK_DIR", "PENELOPE_APPROVE_DIR"].iter().find_map(|k| std::env::var(k).ok().filter(|d| !d.trim().is_empty())).map(Into::into)
}

/// True when someone can answer.
pub fn available() -> bool {
    dir().is_some()
}

/// Puts the questions of the model to the person and waits at most `timeout` seconds.
pub fn ask_questions(qs: &[Question], timeout: u64) -> Reply {
    match dir() {
        Some(dir) => ask_in(&dir, qs, timeout, false),
        None => Reply::Nobody,
    }
}

/// The final yes of Penelope itself ("Create this cluster?"). Only this call may look like an approval in an editor.
pub fn ask_confirm(qs: &[Question], timeout: u64) -> Reply {
    match dir() {
        Some(dir) => ask_in(&dir, qs, timeout, true),
        None => Reply::Nobody,
    }
}

thread_local! {
    static SESSION: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

/// Runs a block as the turn of one ACP session, so a question it asks reaches the editor in that session.
pub fn with_session<T>(id: &str, f: impl FnOnce() -> T) -> T {
    let before = SESSION.with(|s| s.replace(id.to_string()));
    let out = f();
    SESSION.with(|s| *s.borrow_mut() = before);
    out
}

/// The ACP session of this turn: set on this thread, or inherited by an MCP child as PENELOPE_ACP_SESSION.
pub fn current_session() -> String {
    let own = SESSION.with(|s| s.borrow().clone());
    if own.is_empty() { std::env::var("PENELOPE_ACP_SESSION").unwrap_or_default() } else { own }
}

/// True for the interactive main turn. A scout, a part of a cluster and a subagent inherit the question folder but
/// must not put a dialog in front of the person.
pub fn is_main_turn(scout: bool, cluster: Option<&str>, role: &str) -> bool {
    !scout && cluster.map(|c| c.trim().is_empty()).unwrap_or(true) && role.trim().is_empty()
}

/// `is_main_turn` for this process.
pub fn main_turn_here() -> bool {
    is_main_turn(crate::scout::inside(), crate::clusters::current_cluster().as_deref(), &std::env::var("PENELOPE_ROLE").unwrap_or_default())
}

/// The file protocol against one folder. The loop checks every 200 ms for the answer file.
pub fn ask_in(dir: &Path, qs: &[Question], timeout: u64, confirm: bool) -> Reply {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let id = format!("{}-{}", std::process::id(), nanos);
    let (req, ans, tmp) = (dir.join(format!("{}.req", id)), dir.join(format!("{}.ans", id)), dir.join(format!("{}.tmp", id)));
    let body = json!({"kind": "questions", "command": qs.first().map(|q| q.question.clone()).unwrap_or_default(), "cwd": "",
                      "questions": questions_to_value(qs), "deadline": now_secs() + timeout, "timeout": timeout,
                      "confirm": confirm, "session": current_session()});
    if fs::write(&tmp, body.to_string()).is_err() || fs::rename(&tmp, &req).is_err() {
        return Reply::Nobody;
    }
    let started = std::time::Instant::now();
    loop {
        if let Ok(answer) = fs::read_to_string(&ans) {
            let _ = fs::remove_file(&ans);
            let _ = fs::remove_file(&req);
            return parse_reply(&answer, qs.len());
        }
        if started.elapsed().as_secs() >= timeout {
            let _ = fs::remove_file(&req);
            let _ = fs::remove_file(&ans);          // an answer that came in the same moment is late
            return Reply::TimedOut(timeout);
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}

/// True if a process with this id runs. Windows cannot tell cheaply, so it counts as alive and the deadline decides.
pub fn pid_alive(pid: u32) -> bool {
    if cfg!(target_os = "linux") {
        return Path::new("/proc").join(pid.to_string()).exists();
    }
    if cfg!(unix) {
        return std::process::Command::new("kill").args(["-0", &pid.to_string()]).stderr(std::process::Stdio::null()).status().map(|s| s.success()).unwrap_or(true);
    }
    true
}

/// Removes the files of requests nobody waits for: the asking process is gone, or the deadline of a question set passed.
pub fn drop_stale(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let id = name.rsplit_once('.').map(|(a, _)| a).unwrap_or(&name);
        let pid: Option<u32> = id.split_once('-').and_then(|(p, _)| p.parse().ok());        // ids are PID-NANOS
        let mut stale = pid.map(|p| !pid_alive(p)).unwrap_or(false);
        if name.ends_with(".req") && !stale {
            let v: Value = fs::read_to_string(e.path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
            stale = v["deadline"].as_u64().map(|d| d > 0 && d < now_secs()).unwrap_or(false);
        }
        if stale {
            let _ = fs::remove_file(e.path());
        }
    }
}

/// The answer file: JSON with `answers`, or anything else (an empty file) for a closed dialog.
pub fn parse_reply(text: &str, n: usize) -> Reply {
    let Ok(v) = serde_json::from_str::<Value>(text.trim()) else { return Reply::Closed };
    if let Some(t) = v["text_fallback"].as_str() {
        return Reply::Text(t.to_string());
    }
    let Some(list) = v["answers"].as_array() else { return Reply::Closed };
    let mut answers: Vec<Answer> = list.iter().map(Answer::from_value).collect();
    answers.resize(n, Answer::closed());
    if answers.iter().all(|a| a.closed) { Reply::Closed } else { Reply::Answers(answers) }
}

pub fn answers_file(answers: &[Answer]) -> String {
    json!({"answers": answers.iter().map(Answer::to_value).collect::<Vec<_>>()}).to_string()
}

/// One line per question for the transcript and for the model.
pub fn answer_lines(qs: &[Question], answers: &[Answer]) -> Vec<String> {
    qs.iter().zip(answers).enumerate().map(|(i, (q, a))| format!("{}. {}: {} -> {}", i + 1, q.header, q.question, a.line())).collect()
}

/// What the model reads after the dialog ended.
pub fn reply_text(qs: &[Question], reply: &Reply) -> String {
    let careful = "Go on with the most careful choice and say which one you took.";
    match reply {
        Reply::Answers(a) => format!("the user answered {} question{}:\n{}", qs.len(), if qs.len() == 1 { "" } else { "s" }, answer_lines(qs, a).join("\n")),
        Reply::Closed => format!("the user closed the dialog without an answer. {}", careful),
        Reply::TimedOut(s) => format!("the user did not answer within {} seconds. {}", s, careful),
        Reply::Nobody => format!("nobody can answer outside the full screen interface. {}", careful),
        Reply::Text(t) => t.clone(),
    }
}

/// What a person sees as the state of the dialog.
#[derive(Debug, PartialEq)]
pub enum Outcome {
    Continue,
    Send(Vec<Answer>),
    Close,
}

#[derive(Clone, Debug)]
pub struct Dialog {
    pub qs: Vec<Question>,
    /// The open question. `qs.len()` is the summary page.
    pub tab: usize,
    pub cursor: Vec<usize>,
    pub picked: Vec<Vec<bool>>,
    pub text: Vec<String>,
    pub typing: bool,
    /// Unix seconds at which the asker gives up.
    pub deadline: u64,
    pub warn: String,
    /// First visible row of a long question text.
    pub scroll: usize,
}

impl Dialog {
    pub fn new(qs: Vec<Question>, deadline: u64) -> Dialog {
        let n = qs.len();
        let picked = qs.iter().map(|q| vec![false; q.options.len()]).collect();
        Dialog { qs, tab: 0, cursor: vec![0; n], picked, text: vec![String::new(); n], typing: false, deadline, warn: String::new(), scroll: 0 }
    }

    /// Builds the dialog from a request file written by `ask_in`.
    pub fn from_request(v: &Value) -> Option<Dialog> {
        let qs = parse_questions(&v["questions"]).ok()?;
        Some(Dialog::new(qs, v["deadline"].as_u64().unwrap_or(0)))
    }

    pub fn remaining(&self, now: u64) -> u64 {
        self.deadline.saturating_sub(now)
    }

    pub fn on_summary(&self) -> bool {
        self.tab >= self.qs.len()
    }

    /// The options plus, when allowed, the row for a typed answer.
    pub fn rows(&self, i: usize) -> usize {
        self.qs[i].options.len() + usize::from(self.qs[i].allow_text)
    }

    pub fn answered(&self, i: usize) -> bool {
        self.picked[i].iter().any(|p| *p) || !self.text[i].trim().is_empty()
    }

    pub fn answers(&self) -> Vec<Answer> {
        self.qs.iter().enumerate().map(|(i, q)| {
            let labels = q.options.iter().zip(&self.picked[i]).filter(|(_, p)| **p).map(|(o, _)| o.label.clone()).collect();
            let t = self.text[i].trim();
            Answer { labels, text: if t.is_empty() { None } else { Some(t.to_string()) }, closed: false }
        }).collect()
    }

    fn pick(&mut self, i: usize, k: usize) {
        if self.qs[i].multi_select {
            self.picked[i][k] = !self.picked[i][k];
        } else {
            self.picked[i].iter_mut().for_each(|p| *p = false);
            self.picked[i][k] = true;
            self.text[i].clear();
        }
        self.cursor[i] = k;
    }

    fn go(&mut self, tab: usize) {
        self.tab = tab.min(self.qs.len());
        self.scroll = 0;
        self.typing = false;
        self.warn.clear();
    }

    /// One key. Esc always closes the whole dialog.
    pub fn key(&mut self, code: KeyCode) -> Outcome {
        if code == KeyCode::Esc {
            return Outcome::Close;
        }
        match code {
            KeyCode::PageDown => { self.scroll += 3; return Outcome::Continue; }
            KeyCode::PageUp => { self.scroll = self.scroll.saturating_sub(3); return Outcome::Continue; }
            _ => {}
        }
        if self.on_summary() {
            match code {
                KeyCode::Enter => match (0..self.qs.len()).find(|i| !self.answered(*i)) {
                    None => return Outcome::Send(self.answers()),
                    Some(i) => {
                        self.go(i);
                        self.warn = format!("question {} has no answer yet", i + 1);
                    }
                },
                KeyCode::Left | KeyCode::BackTab | KeyCode::Up => self.go(self.tab.saturating_sub(1)),
                _ => {}
            }
            return Outcome::Continue;
        }
        let i = self.tab;
        let n_opts = self.qs[i].options.len();
        let text_row = self.qs[i].allow_text.then_some(n_opts);
        match code {
            KeyCode::Tab => { self.go(self.tab + 1); return Outcome::Continue; }
            KeyCode::BackTab => { self.go(self.tab.saturating_sub(1)); return Outcome::Continue; }
            _ => {}
        }
        if self.typing {
            match code {
                KeyCode::Char(c) => { self.text[i].push(c); if !self.qs[i].multi_select { self.picked[i].iter_mut().for_each(|p| *p = false); } }
                KeyCode::Backspace => { self.text[i].pop(); }
                KeyCode::Enter => {
                    self.typing = false;
                    if self.text[i].trim().is_empty() {
                        self.warn = "type an answer or choose one of the options".to_string();
                    } else {
                        self.go(self.tab + 1);
                    }
                }
                KeyCode::Up => { self.typing = false; self.cursor[i] = n_opts.saturating_sub(1); }
                KeyCode::Down => self.typing = false,
                _ => {}
            }
            return Outcome::Continue;
        }
        self.warn.clear();
        match code {
            KeyCode::Up => self.cursor[i] = self.cursor[i].saturating_sub(1),
            KeyCode::Down => self.cursor[i] = (self.cursor[i] + 1).min(self.rows(i) - 1),
            KeyCode::Right => self.go(self.tab + 1),
            KeyCode::Left => self.go(self.tab.saturating_sub(1)),
            KeyCode::Char(c) if c.is_ascii_digit() && c != '0' && (c as usize - '0' as usize) <= n_opts && Some(self.cursor[i]) != text_row => {
                let k = c as usize - '1' as usize;
                self.pick(i, k);
                if !self.qs[i].multi_select { self.go(self.tab + 1); }
            }
            KeyCode::Char(' ') => {
                if Some(self.cursor[i]) == text_row { self.typing = true; } else { let k = self.cursor[i]; self.pick(i, k); }
            }
            KeyCode::Enter => {
                if Some(self.cursor[i]) == text_row {
                    self.typing = true;
                } else if self.qs[i].multi_select {
                    if self.answered(i) {
                        self.go(self.tab + 1);
                    } else {
                        self.warn = "choose at least one with space".to_string();
                    }
                } else {
                    let k = self.cursor[i];
                    self.pick(i, k);
                    self.go(self.tab + 1);
                }
            }
            KeyCode::Char(c) if text_row.is_some() && !c.is_control() => {
                // A letter starts the free answer.
                self.cursor[i] = n_opts;
                self.typing = true;
                self.text[i].push(c);
                if !self.qs[i].multi_select { self.picked[i].iter_mut().for_each(|p| *p = false); }
            }
            _ => {}
        }
        Outcome::Continue
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(n: usize, multi: bool, text: bool) -> Value {
        json!({"question": format!("Which {}?", n), "header": format!("H{}", n), "multi_select": multi, "allow_text": text,
               "options": [{"label": "alpha", "description": "first"}, "beta", {"label": "gamma"}]})
    }

    fn dialog(multi: bool) -> Dialog {
        Dialog::new(parse_questions(&json!([q(1, multi, true), q(2, false, false)])).unwrap(), now_secs() + 100)
    }

    #[test]
    fn the_schema_takes_one_to_six_questions_with_two_to_six_options() {
        assert_eq!(parse_questions(&json!([q(1, false, true)])).unwrap().len(), 1);
        assert!(parse_questions(&json!([])).unwrap_err().contains("1 to 6"));
        assert!(parse_questions(&json!((0..7).map(|i| q(i, false, true)).collect::<Vec<_>>())).unwrap_err().contains("1 to 6"));
        assert!(parse_questions(&json!([{"question": "x", "options": ["only"]}])).unwrap_err().contains("2 to 6 options"));
        assert!(parse_questions(&json!([{"question": "x", "options": ["a", "b", "c", "d", "e", "f", "g"]}])).unwrap_err().contains("2 to 6 options"));
        assert!(parse_questions(&json!([{"question": "", "options": ["a", "b"]}])).unwrap_err().contains("no text"));
        assert!(parse_questions(&json!([{"question": "x", "options": ["a", "a"]}])).unwrap_err().contains("repeats"));
        assert!(parse_questions(&json!({"question": "x"})).is_err());
    }

    #[test]
    fn defaults_fill_the_header_and_allow_text_and_options_may_be_texts_or_objects() {
        let qs = parse_questions(&json!([{"question": "Pick", "options": ["a", {"label": "b", "description": "why"}]}])).unwrap();
        assert_eq!(qs[0].header, "Q1");
        assert!(qs[0].allow_text && !qs[0].multi_select);
        assert_eq!(qs[0].options[1], Opt { label: "b".into(), description: "why".into() });
        let long = parse_questions(&json!([{"question": "x", "header": "a very long header indeed", "options": ["a", "b"]}])).unwrap();
        assert_eq!(long[0].header.chars().count(), 16);
    }

    #[test]
    fn a_single_choice_advances_and_the_summary_sends() {
        let mut d = dialog(false);
        assert_eq!(d.key(KeyCode::Char('2')), Outcome::Continue);
        assert_eq!(d.tab, 1);
        assert_eq!(d.key(KeyCode::Down), Outcome::Continue);
        assert_eq!(d.key(KeyCode::Enter), Outcome::Continue);
        assert!(d.on_summary());
        match d.key(KeyCode::Enter) {
            Outcome::Send(a) => assert_eq!(a, vec![Answer::chosen("beta"), Answer::chosen("beta")]),
            other => panic!("{:?}", other),
        }
    }

    #[test]
    fn multi_select_toggles_with_space_and_needs_one_pick() {
        let mut d = dialog(true);
        d.key(KeyCode::Enter);
        assert_eq!(d.tab, 0, "nothing picked yet");
        assert!(d.warn.contains("at least one"));
        d.key(KeyCode::Char(' '));
        d.key(KeyCode::Down);
        d.key(KeyCode::Down);
        d.key(KeyCode::Char(' '));
        d.key(KeyCode::Char('1'));
        assert_eq!(d.answers()[0].labels, vec!["gamma"]);
        d.key(KeyCode::Char('1'));
        d.key(KeyCode::Enter);
        assert_eq!(d.tab, 1);
        assert_eq!(d.answers()[0].labels, vec!["alpha", "gamma"]);
    }

    #[test]
    fn a_free_text_answer_replaces_the_choice_and_enter_confirms_it() {
        let mut d = dialog(false);
        d.key(KeyCode::Char('1'));
        d.key(KeyCode::Left);
        assert!(d.answers()[0].labels == vec!["alpha"]);
        d.key(KeyCode::Down);
        d.key(KeyCode::Down);
        d.key(KeyCode::Down);
        d.key(KeyCode::Enter);
        assert!(d.typing);
        for c in "two weeks".chars() { d.key(KeyCode::Char(c)); }
        d.key(KeyCode::Backspace);
        d.key(KeyCode::Char('s'));
        assert_eq!(d.key(KeyCode::Enter), Outcome::Continue);
        assert_eq!(d.tab, 1);
        let a = d.answers();
        assert_eq!(a[0], Answer::typed("two weeks"));
    }

    #[test]
    fn a_letter_starts_the_free_answer_and_a_question_without_text_row_ignores_it() {
        let mut d = dialog(false);
        d.key(KeyCode::Char('x'));
        assert!(d.typing && d.text[0] == "x");
        d.key(KeyCode::Enter);
        d.go(1);
        d.key(KeyCode::Char('x'));
        assert!(!d.typing && d.text[1].is_empty());
    }

    #[test]
    fn the_summary_refuses_an_unanswered_question_and_jumps_to_it() {
        let mut d = dialog(false);
        d.key(KeyCode::Char('1'));
        d.key(KeyCode::Tab);
        assert!(d.on_summary());
        assert_eq!(d.key(KeyCode::Enter), Outcome::Continue);
        assert_eq!(d.tab, 1);
        assert!(d.warn.contains("question 2"));
    }

    #[test]
    fn escape_closes_the_whole_dialog_anywhere() {
        let mut d = dialog(false);
        assert_eq!(d.key(KeyCode::Esc), Outcome::Close);
        d.key(KeyCode::Char('x'));
        assert_eq!(d.key(KeyCode::Esc), Outcome::Close);
        d.go(2);
        assert_eq!(d.key(KeyCode::Esc), Outcome::Close);
    }

    #[test]
    fn digits_type_once_the_text_row_is_active_and_still_choose_on_an_option_row() {
        let mut d = Dialog::new(parse_questions(&json!([q(1, false, true)])).unwrap(), now_secs() + 100);
        for _ in 0..3 { d.key(KeyCode::Down); }
        assert_eq!(d.cursor[0], 3);
        d.key(KeyCode::Char('2'));
        assert!(d.typing && d.text[0] == "2", "a digit on the text row starts the free answer");
        for c in "0 days".chars() { d.key(KeyCode::Char(c)); }
        assert_eq!(d.text[0], "20 days");
        d.key(KeyCode::Enter);
        assert_eq!(d.answers()[0], Answer::typed("20 days"));
        let mut e = Dialog::new(parse_questions(&json!([q(1, false, true)])).unwrap(), now_secs() + 100);
        e.key(KeyCode::Char('2'));
        assert_eq!(e.answers()[0], Answer::chosen("beta"), "on an option row a digit still chooses");
    }

    #[test]
    fn stale_request_files_are_dropped() {
        let dir = std::env::temp_dir().join(format!("penelope-stale-{}-{:?}", std::process::id(), std::thread::current().id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let me = std::process::id();
        let live = format!("{{\"deadline\": {}}}", now_secs() + 600);
        let past = format!("{{\"deadline\": {}}}", now_secs() - 5);
        fs::write(dir.join(format!("{}-1.req", me)), live).unwrap();
        fs::write(dir.join(format!("{}-2.req", me)), past).unwrap();
        fs::write(dir.join("x.req"), "{}").unwrap();
        #[cfg(target_os = "linux")]
        fs::write(dir.join("4294967294-1.req"), "{}").unwrap();
        drop_stale(&dir);
        assert!(dir.join(format!("{}-1.req", me)).exists(), "a live asker with time left stays");
        assert!(!dir.join(format!("{}-2.req", me)).exists(), "a deadline in the past goes");
        assert!(dir.join("x.req").exists(), "a request without a process id is left alone");
        #[cfg(target_os = "linux")]
        assert!(!dir.join("4294967294-1.req").exists(), "a dead process goes");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_long_question_text_scrolls_and_the_page_keys_never_answer() {
        let mut d = dialog(false);
        d.key(KeyCode::PageDown);
        assert_eq!(d.scroll, 3);
        d.key(KeyCode::PageUp);
        d.key(KeyCode::PageUp);
        assert_eq!(d.scroll, 0);
        assert!(!d.answered(0));
    }

    #[test]
    fn the_timeout_comes_from_the_configuration_and_defaults_to_900_seconds() {
        let mut ctx = Ctx { root: std::env::temp_dir(), cfg: json!({}) };
        assert_eq!(timeout_secs(&ctx), 900);
        ctx.cfg = json!({"ask": {"timeout_secs": 120}});
        assert_eq!(timeout_secs(&ctx), 120);
        ctx.cfg = json!({"ask": {"timeout_secs": 1}});
        assert_eq!(timeout_secs(&ctx), 10);
        let d = Dialog::new(parse_questions(&json!([q(1, false, true)])).unwrap(), 1000);
        assert_eq!(d.remaining(940), 60);
        assert_eq!(d.remaining(2000), 0);
    }

    #[test]
    fn the_file_protocol_times_out_and_returns_answers() {
        let dir = std::env::temp_dir().join(format!("penelope-ask-{}-{:?}", std::process::id(), std::thread::current().id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let qs = parse_questions(&json!([q(1, false, true)])).unwrap();
        assert_eq!(ask_in(&dir, &qs, 1, false), Reply::TimedOut(1));
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 0, "the request file is removed");
        let d2 = dir.clone();
        let helper = std::thread::spawn(move || {
            for _ in 0..50 {
                std::thread::sleep(std::time::Duration::from_millis(100));
                for e in fs::read_dir(&d2).unwrap().flatten() {
                    let name = e.file_name().to_string_lossy().to_string();
                    if let Some(id) = name.strip_suffix(".req") {
                        let body: Value = serde_json::from_str(&fs::read_to_string(e.path()).unwrap()).unwrap();
                        assert_eq!(body["kind"], "questions");
                        assert!(body["deadline"].as_u64().unwrap() > now_secs());
                        fs::write(d2.join(format!("{}.ans", id)), answers_file(&[Answer::chosen("beta")])).unwrap();
                        return;
                    }
                }
            }
        });
        assert_eq!(ask_in(&dir, &qs, 30, false), Reply::Answers(vec![Answer::chosen("beta")]));
        helper.join().unwrap();
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn replies_read_back_as_text_with_closed_and_typed_answers() {
        let qs = parse_questions(&json!([q(1, true, true), q(2, false, true)])).unwrap();
        let a = vec![Answer { labels: vec!["alpha".into(), "beta".into()], text: Some("and more".into()), closed: false }, Answer::closed()];
        let text = reply_text(&qs, &Reply::Answers(a));
        assert!(text.contains("1. H1: Which 1? -> alpha, beta + typed: and more"), "{}", text);
        assert!(text.contains("2. H2: Which 2? -> closed"), "{}", text);
        assert!(reply_text(&qs, &Reply::Closed).contains("closed the dialog"));
        assert!(reply_text(&qs, &Reply::TimedOut(900)).contains("within 900 seconds"));
        assert_eq!(parse_reply("", 2), Reply::Closed);
        assert_eq!(parse_reply("{\"answers\":[{\"labels\":[\"a\"],\"closed\":false}]}", 2), Reply::Answers(vec![Answer::chosen("a"), Answer::closed()]));
    }
}
