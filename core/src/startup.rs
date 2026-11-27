//! The startup animation of the command line.
//!
//! Five frames weave the loose thread into the mark, then the name and the two model settings settle. The animation is
//! only drawn on a terminal that can take it: `--no-animation`, `PENELOPE_NO_ANIM=1` or a pipe skips it, so a script, a
//! test and a screen reader never wait on a frame. `frames` is pure, so the pictures can be tested without a terminal.

use std::io::{IsTerminal, Write};

const FRAMES: usize = 5;

/// The frames of the animation, each a block of text. The last frame is the one a piped terminal prints.
pub fn frames(sub: &str) -> Vec<String> {
    let mut out = Vec::new();
    for i in 0..FRAMES {
        let thread = ["", "~", "~~", "~~~", "~~~~"][i];
        let quiet = match i {
            0 => "      ",
            1 => "  ·   ",
            2 => " · ·  ",
            3 => "· · · ",
            _ => "",
        };
        out.push(format!("   +=+=+=+\n   +=+=+=+{}    Penelope\n   +=+=+=+     {}{}\n", thread, quiet, sub));
    }
    out
}

pub fn enabled() -> bool {
    std::io::stdout().is_terminal() && std::env::var("PENELOPE_NO_ANIM").is_err() && std::env::var("TERM").map(|t| t != "dumb").unwrap_or(true)
}

/// Draws the frames on a terminal, else prints the last one. `enabled` can force the short form (a pipe, `--no-animation`).
pub fn play(sub: &str, enabled: bool) {
    let frames = frames(sub);
    if !enabled || !self::enabled() {
        println!("{}", frames.last().unwrap());
        return;
    }
    let mut out = std::io::stdout();
    for (i, frame) in frames.iter().enumerate() {
        let _ = write!(out, "\x1b[2J\x1b[H{}", frame);
        let _ = out.flush();
        if i + 1 < frames.len() {
            std::thread::sleep(std::time::Duration::from_millis(55));
        }
    }
    let _ = writeln!(out);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_frames_weave_the_thread_and_end_with_the_name() {
        let f = frames("main model: sonnet");
        assert_eq!(f.len(), 5);
        assert!(!f[0].contains('~') && f[3].contains("~~~") && f[4].contains("~~~~"), "{:?}", f);
        assert!(f.iter().all(|x| x.contains("Penelope") && x.contains("main model: sonnet")));
        assert_eq!(f[4].lines().count(), 3, "{:?}", f[4]);
    }
}
