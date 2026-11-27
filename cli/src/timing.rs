//! Phase timing for one run. `PENELOPE_TIMING=1` writes one line per phase to stderr; `PENELOPE_TIMING=/path/file`
//! appends the lines to that file, which also works inside the TUI and inside the MCP child. Unset, every call returns
//! after one cached environment check.

use std::io::Write;
use std::sync::OnceLock;
use std::time::Instant;

static START: OnceLock<Instant> = OnceLock::new();
static SINK: OnceLock<Option<String>> = OnceLock::new();

fn sink() -> &'static Option<String> {
    SINK.get_or_init(|| std::env::var("PENELOPE_TIMING").ok().filter(|v| !v.is_empty() && v != "0"))
}

pub fn on() -> bool {
    sink().is_some()
}

/// Starts the clock of this process. The first call wins.
pub fn start() {
    let _ = START.get_or_init(Instant::now);
}

fn since_start() -> u128 {
    START.get_or_init(Instant::now).elapsed().as_millis()
}

/// One line: milliseconds since process start, the phase and its note.
pub fn mark(phase: &str, note: &str) {
    let Some(target) = sink() else { return };
    let who = std::env::var("PENELOPE_ROLE").ok().filter(|r| !r.is_empty()).unwrap_or_else(|| "main".to_string());
    let line = format!("[timing {:>6} ms pid {} {}] {}{}{}\n", since_start(), std::process::id(), who, phase, if note.is_empty() { "" } else { ": " }, note);
    if target == "1" {
        let _ = std::io::stderr().write_all(line.as_bytes());
    } else if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(target) {
        let _ = f.write_all(line.as_bytes());
    }
}

/// Logs the duration of a phase when dropped.
pub struct Span {
    phase: &'static str,
    note: String,
    at: Instant,
}

pub fn span(phase: &'static str) -> Span {
    Span { phase, note: String::new(), at: Instant::now() }
}

impl Span {
    pub fn note(&mut self, text: impl Into<String>) {
        self.note = text.into();
    }
}

impl Drop for Span {
    fn drop(&mut self) {
        if on() {
            mark(self.phase, format!("took {} ms {}", self.at.elapsed().as_millis(), self.note).trim_end());
        }
    }
}

/// Tokens as the prompt counter in this crate estimates them: four characters each.
pub fn tokens(text: &str) -> usize {
    text.chars().count() / 4
}
