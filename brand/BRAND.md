# Kate brand files

Kate is a small amber cat that looks over the edge of a terminal. Above her waits a prompt sign and a cursor block.
The name ends in the same cursor block.

## Files

| file | use |
|---|---|
| `logo.svg`, `logo.png` (512 px) | app icon: ink tile, the cat looks over the lower edge, prompt above |
| `favicon-32.png` | 32 px icon |
| `mark.svg` | cat head with cursor eyes, no tile, for dark and light grounds |
| `mark-outline.svg` | outline head, one eye is a prompt sign, the other a cursor block, for dark grounds |
| `mark-mono.svg` | the outline head in one colour, takes `currentColor` |
| `peek.svg` | the cat alone, to set on an edge |
| `mascot.svg` | the sitting cat, the tail ends in a prompt sign (drawn for dark grounds) |
| `asleep.svg` | the cat curled up |
| `moods.svg`, `mood-*.svg` | six faces: awake, thinking, working, asking, learned, asleep |
| `wordmark-dark.svg`, `wordmark-light.svg` | the drawn name for dark and light grounds |
| `lockup-dark.svg`, `lockup-light.svg` | head plus name |
| `banner.svg`, `banner.png` | README banner, 1280 x 320 |
| `social-preview.svg`, `social-preview.png` | repository preview, 1280 x 640 |
| `concepts/` | the six source drawings |
| `build/brand_build.py` | builds every file above from `concepts/` |
| `build/term_art.sh` | renders the three pictures of the terminal interface into `cli/art/` |

Rebuild from the Kate folder:

```bash
python3 brand/build/brand_build.py brand/concepts brand
rsvg-convert -w 512 -h 512 brand/logo.svg -o brand/logo.png
rsvg-convert -w 32 -h 32 brand/logo.svg -o brand/favicon-32.png
rsvg-convert -w 2560 brand/banner.svg -o brand/banner.png
rsvg-convert -w 1280 -h 640 brand/social-preview.svg -o brand/social-preview.png
sh brand/build/term_art.sh
```

## Colours

| name | hex | use |
|---|---|---|
| Ink | `#15131C` | ground, eyes, text on paper |
| Night | `#1E1B28` | panels and cards on ink |
| Paper | `#F6F3EC` | text on ink, light ground, muzzle and inner ears |
| Amber | `#FFB43A` | the cat, the cursor of the name, the prompt sign |
| Teal | `#2DD4BF` | cursor block of the logo, memory path and waypoints, shared with ScreenCodec |
| Ash | `#B5B1C2` | second line of text on ink |

Amber on paper has a contrast of 1.6 to 1. Use it there for shapes only. The cursor block on paper uses `#E08A00`.

## Type

- Headings and running text: Space Grotesk 400, 500, 700.
- Commands, paths and terminal output: JetBrains Mono 400, 700.
- The name is drawn from strokes (6.5 on a height of 44, round ends). It is no font.
- `banner.png` and `social-preview.png` were rendered with JetBrains Mono for all text, because Space Grotesk was
  not installed on the build machine.

## Moods

| mood | face | when |
|---|---|---|
| awake | `( o.o )` | ready for a request |
| thinking | `( -.o )` | the main model works on its answer |
| working | `( >.< )` | a tool runs |
| asking | `( O.O )` | a command outside the safe list waits for an answer |
| learned | `( ^.^ )` | a rule was stored in the memory map |
| asleep | `( -.- )` | no key for 3 minutes |

All six are wired into the full screen interface (`cli/src/tui/art.rs`). The head there is `mark.svg` at 22 x 22
pixels, two pixels per character cell. The eyes are set per mood as a 3 x 4 pixel pattern.

## Where the drawings come from

The first brand (commit 31bb467) was drawn by hand as SVG. It was replaced on 2026-11-23. The six drawings in
`concepts/` were generated as SVG with GPT Image 2.6 through Higgsfield, one request each, quality high, 1.25 credits
each. Every prompt named the same four colours, a rounded ink tile, flat vector shapes, no text and no gradients.

| file | subject of the prompt | used for |
|---|---|---|
| `01-head.svg` | cat head from the front, eyes as upright cursor blocks, whiskers | `mark.svg`, lockups, moods, the head in the terminal |
| `02-sitting.svg` | sitting cat, tail tip forms a prompt sign next to a cursor | `mascot.svg` |
| `03-peeking.svg` | cat looks over the lower edge, prompt sign and cursor block above | `logo.svg`, `peek.svg`, banner, preview, start screen of the terminal |
| `04-outline.svg` | one line cat face, left eye a prompt sign, right eye a cursor block | `mark-outline.svg`, `mark-mono.svg` |
| `05-asleep.svg` | cat curled up asleep, tail ends in a cursor block | `asleep.svg` |
| `06-kitten.svg` | kitten head, tilted, prompt sign as forehead marking | not used |

The eyes of the moods other than awake, the lockups, the banner and the preview were composed by hand from these
drawings. The name is still the hand drawn one.

## Brief

```
Subject:     small cat for a command line harness, with a prompt sign or a cursor block
Style:       flat vector, round shapes, mascot
Palette:     #15131C, #FFB43A, #F6F3EC, #2DD4BF, stripes #E08A00
Background:  ink tile for the app icon, transparent for marks
Output:      SVG, PNG exports with rsvg-convert
Size:        1024 grid for drawings, 1280 x 320 banner, 1280 x 640 preview
Variants:    6 subjects, 1 drawing each
```
