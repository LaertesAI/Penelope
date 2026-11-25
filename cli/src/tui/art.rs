//! Colours and pictures of the terminal interface. A picture is a grid of pixels. Two pixels share one character
//! cell: the upper half block carries the upper pixel as text colour and the lower pixel as background colour.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier};
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

static TRUECOLOR: AtomicBool = AtomicBool::new(true);

/// Terminals that do not announce 24 bit colour get the nearest of the 216 cube colours.
pub fn detect_colour() {
    let c = std::env::var("COLORTERM").unwrap_or_default();
    TRUECOLOR.store(c.contains("truecolor") || c.contains("24bit"), Ordering::Relaxed);
}

pub fn col(c: Rgb) -> Color {
    if TRUECOLOR.load(Ordering::Relaxed) {
        Color::Rgb(c.0, c.1, c.2)
    } else {
        let q = |v: u8| ((v as u16 * 5 + 127) / 255) as u8;
        Color::Indexed(16 + 36 * q(c.0) + 6 * q(c.1) + q(c.2))
    }
}

#[derive(Clone)]
pub struct Bitmap {
    pub w: usize,
    pub h: usize,
    px: Vec<Rgb>,
}

impl Bitmap {
    fn from_rgb(w: usize, h: usize, bytes: &[u8]) -> Bitmap {
        Bitmap { w, h, px: bytes.chunks(3).take(w * h).map(|c| (c[0], c[1], c[2])).collect() }
    }

    fn set(&mut self, x: usize, y: usize, c: Rgb) {
        if x < self.w && y < self.h {
            self.px[y * self.w + x] = c;
        }
    }

    /// Rows of character cells the picture needs.
    pub fn rows(&self) -> u16 {
        self.h.div_ceil(2) as u16
    }

    /// Draws the picture with its upper left corner at (x, y). Cells outside `clip` are left alone.
    pub fn draw(&self, buf: &mut Buffer, x: u16, y: u16, clip: Rect) {
        for row in 0..self.h.div_ceil(2) {
            for cx in 0..self.w {
                let (px, py) = (x + cx as u16, y + row as u16);
                if px < clip.x || px >= clip.right() || py < clip.y || py >= clip.bottom() {
                    continue;
                }
                let top = self.px[row * 2 * self.w + cx];
                let bottom = if row * 2 + 1 < self.h { self.px[(row * 2 + 1) * self.w + cx] } else { top };
                if let Some(cell) = buf.cell_mut((px, py)) {
                    cell.set_symbol(if top == bottom { " " } else { "▀" }).set_fg(col(top)).set_bg(col(bottom));
                }
            }
        }
    }
}

/// The woven mark for the start screen: 24 columns, 12 rows.
pub fn peek() -> Bitmap {
    Bitmap::from_rgb(24, 24, include_bytes!("../../art/mark_24x24.rgb"))
}

/// The name in tall letters: 80 columns, 8 rows.
pub fn wordmark() -> Bitmap {
    Bitmap::from_rgb(80, 16, include_bytes!("../../art/word_80x16.rgb"))
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

/// The woven square, 22 columns and 11 rows. The loose thread at the lower right takes the colour of the mood.
/// `blink` dims the thread for one frame while Penelope is awake.
pub fn head(mood: Mood, blink: bool) -> Bitmap {
    let mut bm = Bitmap::from_rgb(22, 22, include_bytes!("../../art/weave_22x22.rgb"));
    let mask = include_bytes!("../../art/thread_22x22.gray");
    let thread = match mood {
        Mood::Awake if blink => DIM,
        Mood::Awake => CLAY,
        Mood::Thinking | Mood::Working => SEA,
        Mood::Asking => RED,
        Mood::Learned => PAPER,
        Mood::Asleep => DIM,
    };
    for (i, a) in mask.iter().take(22 * 22).enumerate() {
        if *a > 0 {
            let (a, base) = (*a as u32, bm.px[i]);
            let mix = |t: u8, b: u8| ((t as u32 * a + b as u32 * (255 - a)) / 255) as u8;
            bm.set(i % 22, i / 22, (mix(thread.0, base.0), mix(thread.1, base.1), mix(thread.2, base.2)));
        }
    }
    bm
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
    let mut out = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {} {}\" font-family=\"'JetBrains Mono', 'JetBrainsMono Nerd Font', ui-monospace, monospace\" font-size=\"15\">\n<rect width=\"100%\" height=\"100%\" fill=\"{}\"/>\n",
        area.width as f32 * cw, area.height as f32 * ch, hex(Color::Reset, INK));
    let mut glyphs = String::new();
    for y in 0..area.height {
        for x in 0..area.width {
            let cell = &buf[(area.x + x, area.y + y)];
            let (px, py) = (x as f32 * cw, y as f32 * ch);
            let (fg, bg) = (hex(cell.fg, PAPER), hex(cell.bg, INK));
            if bg != hex(Color::Reset, INK) {
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
    out.push_str(&glyphs);
    out.push_str("</svg>\n");
    out
}
