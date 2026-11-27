//! The painted picture of Penelope at the loom, shown as a real picture in the terminal.
//! A terminal cannot draw a picture from characters without large pixels, so the picture goes out through the
//! graphics protocol of kitty. Terminals that speak it: kitty, Ghostty, WezTerm and Konsole.
//! Every other terminal shows no picture, and the name stands alone.
//!
//! The view asks for the picture with `want(rect)` while it draws a frame. After the frame the loop calls
//! `Painter::sync`, which sends the picture once and then moves, shows or removes it.

use ratatui::layout::Rect;
use std::cell::{Cell, RefCell};
use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

/// The picture, 360 by 360 pixels with 48 colours, 78 kB. A mod may bring its own PNG; this one is the fallback.
static PNG: &[u8] = include_bytes!("../../art/penelope_360.png");
static ON: AtomicBool = AtomicBool::new(false);
/// `PENELOPE_PICTURE_DEBUG=1`: the terminal reports errors of picture commands (q=1) instead of staying silent (q=2).
static DEBUG: AtomicBool = AtomicBool::new(false);
static WANT: Mutex<Option<Rect>> = Mutex::new(None);
/// The number under which the terminal keeps the picture.
const ID: u32 = 7001;
/// A longer picture is refused before it reaches the terminal.
const MAX_BYTES: u64 = 16 * 1024 * 1024;

thread_local! {
    /// The picture of this interface thread. A mod changes it, and the painter resends it once.
    static IMAGE: RefCell<Option<Vec<u8>>> = RefCell::new(None);
    /// Counts every change of the picture, so the painter knows when to send it again.
    static GENERATION: Cell<u64> = Cell::new(0);
}

/// The bytes the terminal gets: the picture of the active mod, or the painted Penelope.
fn image() -> Vec<u8> {
    IMAGE.with(|i| i.borrow().clone()).unwrap_or_else(|| PNG.to_vec())
}

pub fn generation() -> u64 {
    GENERATION.with(|g| g.get())
}

/// True when a mod changed the picture.
#[cfg(test)]
pub fn custom() -> bool {
    IMAGE.with(|i| i.borrow().is_some())
}

/// The painted Penelope, after a mod goes off.
pub fn use_default() {
    let changed = IMAGE.with(|i| i.borrow_mut().take().is_some());
    if changed {
        GENERATION.with(|g| g.set(g.get() + 1));
    }
}

/// Takes a PNG for the terminal. The kitty protocol carries PNG, so a JPEG is converted first.
pub fn set_image(bytes: Vec<u8>) -> Result<(), String> {
    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err("the picture is not a PNG".to_string());
    }
    IMAGE.with(|i| *i.borrow_mut() = Some(bytes));
    GENERATION.with(|g| g.set(g.get() + 1));
    Ok(())
}

/// Loads a picture from a mod folder. A missing or unreadable file is an error, and the caller keeps the old picture.
pub fn set_path(path: &Path) -> Result<(), String> {
    let size = fs::metadata(path).map(|m| m.len()).map_err(|e| format!("{} cannot be read: {}", crate::files::show(path), e))?;
    if size > MAX_BYTES {
        return Err(format!("{} is larger than {} MB", crate::files::show(path), MAX_BYTES / (1024 * 1024)));
    }
    let bytes = fs::read(path).map_err(|e| format!("{} cannot be read: {}", crate::files::show(path), e))?;
    let png = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        bytes
    } else if bytes.starts_with(&[0xFF, 0xD8]) {
        jpeg_to_png(&bytes)?
    } else {
        return Err(format!("{} is not a PNG or JPEG", crate::files::show(path)));
    };
    set_image(png)
}

/// JPEG support without a new crate: the terminal only takes PNG, so a local converter does the step. A machine without
/// ffmpeg or ImageMagick keeps the default picture and gets a clear line instead.
fn jpeg_to_png(bytes: &[u8]) -> Result<Vec<u8>, String> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let base = std::env::temp_dir().join(format!("penelope-picture-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    let (jpg, png) = (base.with_extension("jpg"), base.with_extension("png"));
    fs::write(&jpg, bytes).map_err(|e| format!("the JPEG could not be written for conversion: {}", e))?;
    let mut tried = false;
    for program in ["ffmpeg", "magick"] {
        if crate::config::which(program).is_none() {
            continue;
        }
        tried = true;
        let ok = if program == "ffmpeg" {
            std::process::Command::new(program).args(["-y", "-loglevel", "error", "-i"]).arg(&jpg).arg(&png)
                .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status()
        } else {
            std::process::Command::new(program).arg(&jpg).arg(&png)
                .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status()
        };
        if ok.map(|s| s.success()).unwrap_or(false) {
            if let Ok(out) = fs::read(&png) {
                if out.starts_with(b"\x89PNG\r\n\x1a\n") {
                    let _ = fs::remove_file(&jpg);
                    let _ = fs::remove_file(&png);
                    return Ok(out);
                }
            }
        }
    }
    let _ = fs::remove_file(&jpg);
    let _ = fs::remove_file(&png);
    Err(if tried {
        "the JPEG could not be converted to PNG; the default picture stays".to_string()
    } else {
        "a JPEG needs ffmpeg or ImageMagick to become a PNG; the default picture stays".to_string()
    })
}

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
    env("KITTY_WINDOW_ID").is_some() || env("KITTY_PID").is_some() || term.contains("kitty") || term.contains("ghostty")
        || program == "ghostty" || program == "kitty" || env("GHOSTTY_RESOURCES_DIR").is_some() || program == "WezTerm" || env("KONSOLE_VERSION").is_some()
}

/// Reads the environment once at the start of the interface.
pub fn detect() {
    ON.store(supported(|key| std::env::var(key).ok()), Ordering::Relaxed);
    DEBUG.store(std::env::var("PENELOPE_PICTURE_DEBUG").map(|v| v == "1").unwrap_or(false), Ordering::Relaxed);
}

/// Sets the switch by hand, for tests and for the pictures of the screen in the docs.
#[cfg(test)]
pub fn force(on: bool) {
    ON.store(on, Ordering::Relaxed);
}

/// The quiet level of picture commands: 2 hides every answer, 1 lets the terminal report errors.
fn q() -> u8 {
    if DEBUG.load(Ordering::Relaxed) { 1 } else { 2 }
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
    format!("data:image/png;base64,{}", base64(&image()))
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
    generation: u64,
    size: Option<(u16, u16)>,
}

impl Painter {
    /// Call after every drawn frame. Writes nothing when the place did not change.
    pub fn sync(&mut self, out: &mut impl Write) -> io::Result<()> {
        let generation = generation();
        if generation != self.generation {
            // a mod brought another picture: the terminal forgets the old one, the new one goes out below
            if self.at.is_some() {
                write!(out, "\x1b_Ga=d,d=i,i={},q={}\x1b\\", ID, q())?;
            }
            self.sent = false;
            self.at = None;
            self.generation = generation;
        }
        let want = WANT.lock().unwrap().take();
        if !on() || want == self.at {
            return Ok(());
        }
        if self.at.is_some() {
            write!(out, "\x1b_Ga=d,d=i,i={},q={}\x1b\\", ID, q())?;
        }
        if let Some(rect) = want {
            if !self.sent {
                // the picture travels once, in pieces of 4,096 characters, and stays in the terminal under its number
                let data = base64(&image());
                let pieces: Vec<&str> = data.as_bytes().chunks(4096).map(|c| std::str::from_utf8(c).unwrap()).collect();
                for (i, piece) in pieces.iter().enumerate() {
                    let more = if i + 1 < pieces.len() { 1 } else { 0 };
                    if i == 0 {
                        write!(out, "\x1b_Ga=t,f=100,t=d,i={},q={},m={};{}\x1b\\", ID, q(), more, piece)?;
                    } else {
                        write!(out, "\x1b_Gm={};{}\x1b\\", more, piece)?;
                    }
                }
                self.sent = true;
            }
            // save the cursor, go to the corner, show the picture over `width` columns under the text layer, come back
            write!(out, "\x1b7\x1b[{};{}H\x1b_Ga=p,i={},p=1,c={},z=-1,C=1,q={}\x1b\\\x1b8", rect.y + 1, rect.x + 1, ID, rect.width, q())?;
        }
        self.at = want;
        out.flush()
    }

    /// The terminal was cleared or resized: the placement is gone, and the terminal may have dropped the data too.
    /// The next frame sends the picture again and shows it.
    pub fn reset(&mut self, out: &mut impl Write) {
        if on() && self.at.is_some() {
            let _ = write!(out, "\x1b_Ga=d,d=i,i={},q={}\x1b\\", ID, q());
            let _ = out.flush();
        }
        self.at = None;
        self.sent = false;
    }

    /// Call before every frame with the terminal size. ratatui clears the screen when the size changed, which removes
    /// the placement without any event reaching the painter, so a same-sized wish would otherwise stay unplaced.
    pub fn watch(&mut self, size: (u16, u16), out: &mut impl Write) {
        if self.size.is_some() && self.size != Some(size) {
            self.reset(out);
        }
        self.size = Some(size);
    }

    /// The interface ends: the terminal forgets the picture.
    pub fn end(&mut self, out: &mut impl Write) {
        if on() && self.sent {
            let _ = write!(out, "\x1b_Ga=d,d=I,i={},q={}\x1b\\", ID, q());
            let _ = out.flush();
        }
        self.sent = false;
        self.at = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static LOCK: Mutex<()> = Mutex::new(());

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
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
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

    #[test]
    fn a_mod_picture_replaces_the_default_and_the_default_comes_back() {
        use_default();
        let before = generation();
        assert!(!custom());
        set_image(b"\x89PNG\r\n\x1a\nfake".to_vec()).unwrap();
        assert!(custom() && generation() == before + 1);
        assert!(data_uri().ends_with(&base64(b"\x89PNG\r\n\x1a\nfake")));
        assert!(set_image(b"not a png".to_vec()).is_err());
        use_default();
        assert!(!custom() && generation() == before + 2);
    }

    fn sync_with(painter: &mut Painter, rect: Option<Rect>) -> String {
        let mut out = Vec::new();
        match rect { Some(r) => want(r), None => cancel() }
        painter.sync(&mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    /// Reproduces the lost picture after a login or setup: the terminal gets a fresh alternate screen without the
    /// picture, the wish stays the same, and the old painter sent nothing because it believed the picture was there.
    #[test]
    fn a_new_screen_gets_the_data_and_the_placement_again() {
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        force(true);
        let (mut painter, rect) = (Painter::default(), Rect::new(4, 2, 22, 11));
        assert!(sync_with(&mut painter, Some(rect)).contains("a=t,f=100"));
        let mut sink = Vec::new();
        painter.end(&mut sink);
        let again = sync_with(&mut painter, Some(rect));
        assert!(again.contains("a=t,f=100") && again.contains("a=p,i=7001"), "data and placement go out again");
        force(false);
    }

    /// ratatui clears the screen on a size change before it draws, so the placement is gone while the wish is equal.
    #[test]
    fn a_size_change_places_the_picture_again_even_for_the_same_rect() {
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        force(true);
        let (mut painter, rect) = (Painter::default(), Rect::new(4, 2, 22, 11));
        let mut sink = Vec::new();
        painter.watch((120, 40), &mut sink);
        sync_with(&mut painter, Some(rect));
        painter.watch((120, 40), &mut sink);
        assert_eq!(sync_with(&mut painter, Some(rect)), "", "the same size writes nothing");
        painter.watch((120, 44), &mut sink);
        let after = sync_with(&mut painter, Some(rect));
        assert!(after.contains("a=t,f=100") && after.contains("a=p,i=7001"), "{}", after.len());
        force(false);
    }

    /// A dialog takes the wish away for some frames; when it closes the picture comes back at the same place.
    #[test]
    fn the_picture_returns_after_a_dialog_at_the_same_place() {
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        force(true);
        let (mut painter, rect) = (Painter::default(), Rect::new(4, 2, 22, 11));
        sync_with(&mut painter, Some(rect));
        assert!(sync_with(&mut painter, None).starts_with("\x1b_Ga=d"));
        assert!(sync_with(&mut painter, Some(rect)).contains("a=p,i=7001"));
        force(false);
    }

    #[test]
    fn more_terminals_are_found_by_their_variables() {
        assert!(supported(env(&[("KITTY_PID", "812")])));
        assert!(supported(env(&[("TERM_PROGRAM", "kitty")])));
        assert!(supported(env(&[("GHOSTTY_RESOURCES_DIR", "/usr/share/ghostty")])));
    }
}
