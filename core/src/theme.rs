//! The colours of the interface as data.
//!
//! A mod names a colour of this palette in its manifest. The palette lives in the core so the manifest can be read
//! and checked without a terminal; each frontend maps the names to its own drawing.

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
pub const GOLD: Rgb = (0xF2, 0xC9, 0x6B);

/// The ten colours a mod may change. The names are the keys of the theme object in a mod manifest.
pub const NAMES: [&str; 10] = [
    "ink", "night", "raised", "paper", "clay", "sea", "ash", "dim", "red", "gold",
];

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
    pub const DEFAULT: Palette = Palette {
        ink: INK,
        night: NIGHT,
        raised: RAISED,
        paper: PAPER,
        clay: CLAY,
        sea: SEA,
        ash: ASH,
        dim: DIM,
        red: RED,
        gold: GOLD,
    };

    /// Writes one named colour. False means the name is not part of the palette.
    pub fn set(&mut self, name: &str, rgb: Rgb) -> bool {
        match name {
            "ink" => self.ink = rgb,
            "night" => self.night = rgb,
            "raised" => self.raised = rgb,
            "paper" => self.paper = rgb,
            "clay" => self.clay = rgb,
            "sea" => self.sea = rgb,
            "ash" => self.ash = rgb,
            "dim" => self.dim = rgb,
            "red" => self.red = rgb,
            "gold" => self.gold = rgb,
            _ => return false,
        }
        true
    }

    /// The default palette with the colours of a mod laid over it. An unknown name is an error.
    pub fn with(&self, overrides: &[(String, Rgb)]) -> Result<Palette, String> {
        let mut out = *self;
        for (name, rgb) in overrides {
            if !out.set(name, *rgb) {
                return Err(format!(
                    "{} is not a colour of the palette ({})",
                    name,
                    NAMES.join(", ")
                ));
            }
        }
        Ok(out)
    }
}
