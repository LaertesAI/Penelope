//! The painted picture of Penelope at the loom, shown as a real picture in the terminal.
//! A terminal cannot draw a picture from characters without large pixels, so the picture goes out through the
//! graphics protocol of kitty. Terminals that speak it: kitty, Ghostty, WezTerm and Konsole.
//! Every other terminal shows no picture, and the name stands alone.
//!
//! The view asks for the picture with `want(rect)` while it draws a frame. After the frame the loop calls
//! `Painter::sync`, which sends the picture once and then moves, shows or removes it.

use ratatui::layout::Rect;
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

/// The picture, 360 by 360 pixels with 48 colours, 78 kB.
static PNG: &[u8] = include_bytes!("../../art/penelope_360.png");
static ON: AtomicBool = AtomicBool::new(false);
static WANT: Mutex<Option<Rect>> = Mutex::new(None);
/// The number under which the terminal keeps the picture.
const ID: u32 = 7001;

/// Whether the terminal named by these environment variables shows pictures.
/// `PENELOPE_PICTURE=off` switches the picture off, `PENELOPE_PICTURE=on` forces it.
pub fn supported(env: impl Fn(&str) -> Option<String>) -> bool {
    match env("PENELOPE_PICTURE").as_deref() {
        Some("off") | Some("0") => return false,
        Some("on") | Some("kitty") | Some("1") => return true,
        _ => {}
    }
    if env("TMUX").is_some() {
        return false; // tmux passes graphics commands on only with a special setting
    }
    let term = env("TERM").unwrap_or_default();
    let program = env("TERM_PROGRAM").unwrap_or_default();
    env("KITTY_WINDOW_ID").is_some() || term.contains("kitty") || term.contains("ghostty")
        || program == "ghostty" || program == "WezTerm" || env("KONSOLE_VERSION").is_some()
}

/// Reads the environment once at the start of the interface.
pub fn detect() {
    ON.store(supported(|key| std::env::var(key).ok()), Ordering::Relaxed);
}

/// Sets the switch by hand, for tests and for the pictures of the screen in the docs.
#[cfg(test)]
pub fn force(on: bool) {
    ON.store(on, Ordering::Relaxed);
}

pub fn on() -> bool {
    ON.load(Ordering::Relaxed)
}

/// The view wants the picture at this place in the frame it is drawing. The picture is square:
/// it fills the width of `rect` and takes its height from that width.
pub fn want(rect: Rect) {
    *WANT.lock().unwrap() = Some(rect);
}

/// The frame shows no picture, for example because a dialog lies over its place.
pub fn cancel() {
    *WANT.lock().unwrap() = None;
}

/// The place the last frame asked for, without taking it. The SVG picture of the screen reads this.
pub fn wanted() -> Option<Rect> {
    *WANT.lock().unwrap()
}

/// The picture as text for an SVG file.
pub fn data_uri() -> String {
    format!("data:image/png;base64,{}", base64(PNG))
}

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (chunk[0] as u32) << 16 | (*chunk.get(1).unwrap_or(&0) as u32) << 8 | *chunk.get(2).unwrap_or(&0) as u32;
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { TABLE[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { TABLE[n as usize & 63] as char } else { '=' });
    }
    out
}

/// Keeps what the terminal shows in step with what the view wants.
#[derive(Default)]
pub struct Painter {
    sent: bool,
    at: Option<Rect>,
}

impl Painter {
    /// Call after every drawn frame. Writes nothing when the place did not change.
    pub fn sync(&mut self, out: &mut impl Write) -> io::Result<()> {
        let want = WANT.lock().unwrap().take();
        if !on() || want == self.at {
            return Ok(());
        }
        if self.at.is_some() {
            write!(out, "\x1b_Ga=d,d=i,i={},q=2\x1b\\", ID)?;
        }
        if let Some(rect) = want {
            if !self.sent {
                // the picture travels once, in pieces of 4,096 characters, and stays in the terminal under its number
                let data = base64(PNG);
                let pieces: Vec<&str> = data.as_bytes().chunks(4096).map(|c| std::str::from_utf8(c).unwrap()).collect();
                for (i, piece) in pieces.iter().enumerate() {
                    let more = if i + 1 < pieces.len() { 1 } else { 0 };
                    if i == 0 {
                        write!(out, "\x1b_Ga=t,f=100,t=d,i={},q=2,m={};{}\x1b\\", ID, more, piece)?;
                    } else {
                        write!(out, "\x1b_Gm={};{}\x1b\\", more, piece)?;
                    }
                }
                self.sent = true;
            }
            // save the cursor, go to the corner, show the picture over `width` columns under the text layer, come back
            write!(out, "\x1b7\x1b[{};{}H\x1b_Ga=p,i={},p=1,c={},z=-1,C=1,q=2\x1b\\\x1b8", rect.y + 1, rect.x + 1, ID, rect.width)?;
        }
        self.at = want;
        out.flush()
    }

    /// The terminal was cleared or resized: the next frame shows the picture again.
    pub fn reset(&mut self, out: &mut impl Write) {
        if on() && self.at.is_some() {
            let _ = write!(out, "\x1b_Ga=d,d=i,i={},q=2\x1b\\", ID);
            let _ = out.flush();
        }
        self.at = None;
    }

    /// The interface ends: the terminal forgets the picture.
    pub fn end(&mut self, out: &mut impl Write) {
        if on() && self.sent {
            let _ = write!(out, "\x1b_Ga=d,d=I,i={},q=2\x1b\\", ID);
            let _ = out.flush();
        }
        self.sent = false;
        self.at = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |key| pairs.iter().find(|(k, _)| *k == key).map(|(_, v)| v.to_string())
    }

    #[test]
    fn the_terminal_decides_whether_the_picture_shows() {
        assert!(supported(env(&[("TERM", "xterm-kitty")])));
        assert!(supported(env(&[("KITTY_WINDOW_ID", "3")])));
        assert!(supported(env(&[("TERM_PROGRAM", "WezTerm")])));
        assert!(supported(env(&[("KONSOLE_VERSION", "240800")])));
        assert!(!supported(env(&[("TERM", "alacritty")])));
        assert!(!supported(env(&[("TERM", "xterm-kitty"), ("TMUX", "/tmp/tmux-1000/default,1,0")])));
        assert!(!supported(env(&[("TERM", "xterm-kitty"), ("PENELOPE_PICTURE", "off")])));
        assert!(supported(env(&[("TERM", "alacritty"), ("PENELOPE_PICTURE", "on")])));
    }

    #[test]
    fn base64_matches_the_known_cases() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert!(PNG.starts_with(b"\x89PNG"));
    }

    #[test]
    fn the_picture_is_sent_once_and_moved_after_that() {
        force(true);
        let mut painter = Painter::default();
        let mut out = Vec::new();
        want(Rect::new(4, 2, 22, 11));
        painter.sync(&mut out).unwrap();
        let first = String::from_utf8(out.clone()).unwrap();
        assert!(first.contains("a=t,f=100") && first.contains("\x1b[3;5H") && first.contains("a=p,i=7001,p=1,c=22"));
        out.clear();
        want(Rect::new(4, 2, 22, 11));
        painter.sync(&mut out).unwrap();
        assert!(out.is_empty(), "the same place writes nothing");
        want(Rect::new(8, 2, 22, 11));
        painter.sync(&mut out).unwrap();
        let moved = String::from_utf8(out.clone()).unwrap();
        assert!(moved.starts_with("\x1b_Ga=d,d=i,i=7001") && moved.contains("\x1b[3;9H") && !moved.contains("a=t,"));
        out.clear();
        painter.sync(&mut out).unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), "\x1b_Ga=d,d=i,i=7001,q=2\x1b\\", "no wish removes the picture");
        force(false);
    }
}
