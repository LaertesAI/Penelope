//! Colours and moods of the terminal interface, and the screen as an SVG picture.
//! The painted picture of Penelope lives in `picture.rs`.

use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier};
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, Ordering};

pub type Rgb = (u8, u8, u8);
pub const INK: Rgb = (0x15, 0x13, 0x1C);
pub const NIGHT: Rgb = (0x1E, 0x1B, 0x28);
pub const RAISED: Rgb = (0x2B, 0x27, 0x39);
pub const PAPER: Rgb = (0xF6, 0xF3, 0xEC);
pub const CLAY: Rgb = (0xE8, 0x82, 0x5A);
pub const SEA: Rgb = (0x5F, 0xB3, 0xE4);
pub const ASH: Rgb = (0xB5, 0xB1, 0xC2);
pub const DIM: Rgb = (0x7C, 0x77, 0x8C);
pub const RED: Rgb = (0xFF, 0x6B, 0x6B);
/// A model of a Laertes family. None of the eight can answer yet, so this colour always means "in training".
pub const GOLD: Rgb = (0xF2, 0xC9, 0x6B);

/// The ten colours a mod may change. The names are the keys of the theme object in a mod manifest.
pub const NAMES: [&str; 10] = ["ink", "night", "raised", "paper", "clay", "sea", "ash", "dim", "red", "gold"];

/// The palette of the interface. The constants above are its default; a mod may replace single colours.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Palette {
    pub ink: Rgb,
    pub night: Rgb,
    pub raised: Rgb,
    pub paper: Rgb,
    pub clay: Rgb,
    pub sea: Rgb,
    pub ash: Rgb,
    pub dim: Rgb,
    pub red: Rgb,
    pub gold: Rgb,
}

impl Palette {
    pub const DEFAULT: Palette = Palette { ink: INK, night: NIGHT, raised: RAISED, paper: PAPER, clay: CLAY,
                                          sea: SEA, ash: ASH, dim: DIM, red: RED, gold: GOLD };

    /// Writes one named colour. False means the name is not part of the palette.
    pub fn set(&mut self, name: &str, rgb: Rgb) -> bool {
        match name {
            "ink" => self.ink = rgb, "night" => self.night = rgb, "raised" => self.raised = rgb,
            "paper" => self.paper = rgb, "clay" => self.clay = rgb, "sea" => self.sea = rgb,
            "ash" => self.ash = rgb, "dim" => self.dim = rgb, "red" => self.red = rgb,
            "gold" => self.gold = rgb, _ => return false,
        }
        true
    }

    /// The default palette with the colours of a mod laid over it. An unknown name is an error.
    pub fn with(&self, overrides: &[(String, Rgb)]) -> Result<Palette, String> {
        let mut out = *self;
        for (name, rgb) in overrides {
            if !out.set(name, *rgb) {
                return Err(format!("{} is not a colour of the palette ({})", name, NAMES.join(", ")));
            }
        }
        Ok(out)
    }
}

thread_local! {
    /// The palette of the frame being drawn. A mod changes it per draw, so two tests on two threads never share it.
    static ACTIVE: Cell<Palette> = Cell::new(Palette::DEFAULT);
}

pub fn palette() -> Palette {
    ACTIVE.with(|p| p.get())
}

/// The palette the next `col` calls use. `view::draw` sets it from the app before it paints a frame.
pub fn set_palette(p: Palette) {
    ACTIVE.with(|a| a.set(p));
}

/// A default colour that a mod replaced is drawn in its new colour. Other colours stay as they are.
fn themed(c: Rgb) -> Rgb {
    let p = palette();
    if c == INK { p.ink } else if c == NIGHT { p.night } else if c == RAISED { p.raised }
    else if c == PAPER { p.paper } else if c == CLAY { p.clay } else if c == SEA { p.sea }
    else if c == ASH { p.ash } else if c == DIM { p.dim } else if c == RED { p.red }
    else if c == GOLD { p.gold } else { c }
}

static TRUECOLOR: AtomicBool = AtomicBool::new(true);

/// Terminals that do not announce 24 bit colour get the nearest of the 216 cube colours.
pub fn detect_colour() {
    let c = std::env::var("COLORTERM").unwrap_or_default();
    TRUECOLOR.store(c.contains("truecolor") || c.contains("24bit"), Ordering::Relaxed);
}

pub fn col(c: Rgb) -> Color {
    let c = themed(c);
    if TRUECOLOR.load(Ordering::Relaxed) {
        Color::Rgb(c.0, c.1, c.2)
    } else {
        let q = |v: u8| ((v as u16 * 5 + 127) / 255) as u8;
        Color::Indexed(16 + 36 * q(c.0) + 6 * q(c.1) + q(c.2))
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Mood {
    Awake,
    Thinking,
    Working,
    Asking,
    Learned,
    Asleep,
}

impl Mood {
    pub fn name(self) -> &'static str {
        match self {
            Mood::Awake => "awake",
            Mood::Thinking => "thinking",
            Mood::Working => "working",
            Mood::Asking => "asking",
            Mood::Learned => "learned",
            Mood::Asleep => "asleep",
        }
    }

    /// The face as text, for narrow terminals and the plain command line.
    pub fn face(self) -> &'static str {
        match self {
            Mood::Awake => "[#|#]",
            Mood::Thinking => "[#~#]",
            Mood::Working => "[#>#]",
            Mood::Asking => "[#?#]",
            Mood::Learned => "[#+#]",
            Mood::Asleep => "[#.#]",
        }
    }
}

fn hex(c: Color, fallback: Rgb) -> String {
    let (r, g, b) = match c {
        Color::Rgb(r, g, b) => (r, g, b),
        _ => fallback,
    };
    format!("#{:02X}{:02X}{:02X}", r, g, b)
}

/// The screen as an SVG picture, one rectangle and one glyph per character cell.
pub fn buffer_svg(buf: &Buffer) -> String {
    let (cw, ch) = (9.0f32, 20.0f32);
    let area = buf.area;
    let p = palette();
    let mut out = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {} {}\" font-family=\"'JetBrains Mono', 'JetBrainsMono Nerd Font', ui-monospace, monospace\" font-size=\"15\">\n<rect width=\"100%\" height=\"100%\" fill=\"{}\"/>\n",
        area.width as f32 * cw, area.height as f32 * ch, hex(Color::Reset, p.ink));
    let mut glyphs = String::new();
    for y in 0..area.height {
        for x in 0..area.width {
            let cell = &buf[(area.x + x, area.y + y)];
            let (px, py) = (x as f32 * cw, y as f32 * ch);
            let (fg, bg) = (hex(cell.fg, p.paper), hex(cell.bg, p.ink));
            if bg != hex(Color::Reset, p.ink) {
                out.push_str(&format!("<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"{}\"/>\n", px, py, cw + 0.5, ch + 0.5, bg));
            }
            match cell.symbol() {
                " " | "" => {}
                "▀" => out.push_str(&format!("<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"{}\"/>\n", px, py, cw + 0.5, ch / 2.0, fg)),
                "┃" => out.push_str(&format!("<rect x=\"{}\" y=\"{}\" width=\"2\" height=\"{}\" fill=\"{}\"/>\n", px + cw / 2.0 - 1.0, py, ch + 0.5, fg)),
                s => {
                    let s = s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
                    let bold = if cell.modifier.contains(Modifier::BOLD) { " font-weight=\"700\"" } else { "" };
                    glyphs.push_str(&format!("<text x=\"{}\" y=\"{}\" fill=\"{}\"{}>{}</text>\n", px, py + 15.0, fg, bold, s));
                }
            }
        }
    }
    if let Some(r) = super::picture::wanted() {
        // the painted picture is square and as wide as its place
        let side = r.width as f32 * cw;
        out.push_str(&format!("<image x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" href=\"{}\"/>\n", r.x as f32 * cw, r.y as f32 * ch, side, side, super::picture::data_uri()));
    }
    out.push_str(&glyphs);
    out.push_str("</svg>\n");
    out
}
