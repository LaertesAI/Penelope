# Strix Forgery brand files

Strix is the genus of the wood owls. The owl of the logo looks over the lower edge of a dark tile. Above it stands an
anvil with two sparks. The eyes are amber, the colour of the cursor and of every accent in the terminal interface.

## Files

| file | use |
|---|---|
| `logo.svg`, `logo.png` (512 px) | app icon: ink tile, the owl looks over the lower edge, anvil and sparks above |
| `favicon-32.png` | 32 px icon |
| `mark.svg` | owl head with amber eyes, no tile, for dark grounds |
| `mark-outline.svg` | outline head, one eye is a prompt sign, the other a cursor block, for dark grounds |
| `mark-mono.svg` | the outline head in one colour, takes `currentColor` |
| `peek.svg` | the owl alone, to set on an edge |
| `mascot.svg` | the owl sits on an anvil (drawn for dark grounds) |
| `moods.svg`, `mood-*.svg` | six faces: awake, thinking, working, asking, learned, asleep |
| `wordmark-dark.svg`, `wordmark-light.svg` | the name in two colours with a cursor block |
| `wordmark-short.svg` | the first word alone, source of the name picture in the terminal |
| `lockup-dark.svg`, `lockup-light.svg` | head plus name, the light one sets the head on an ink tile |
| `banner.svg`, `banner.png` | README banner, 1280 x 320 |
| `social-preview.svg`, `social-preview.png` | repository preview, 1280 x 640 |
| `concepts/` | the four source drawings |
| `build/brand_build.py` | builds every file above from `concepts/` |
| `build/term_art.sh` | renders the three pictures of the terminal interface into `cli/art/` |

Rebuild from the harness folder:

```bash
python3 brand/build/brand_build.py
rsvg-convert -w 512 -h 512 brand/logo.svg -o brand/logo.png
rsvg-convert -w 32 -h 32 brand/logo.svg -o brand/favicon-32.png
rsvg-convert -w 2560 brand/banner.svg -o brand/banner.png
rsvg-convert -w 1280 -h 640 brand/social-preview.svg -o brand/social-preview.png
sh brand/build/term_art.sh
```

The name stands in one line of `brand_build.py` (`NAME = ("strix", "forgery")`). A change there renames every
wordmark, lockup, banner and preview on the next build.

## Colours

| name | hex | use |
|---|---|---|
| Ink | `#15131C` | ground, pupils, text on paper |
| Night | `#1E1B28` | panels and cards on ink |
| Paper | `#F6F3EC` | text on ink, light ground, the head of the owl |
| Amber | `#FFB43A` | eyes, sparks, second word of the name, prompt sign |
| Teal | `#2DD4BF` | cursor block of the name, memory path and waypoints, shared with ScreenCodec and VeeAI |
| Ash | `#B5B1C2` | facial disc, anvil, second line of text on ink |

Amber on paper has a contrast of 1.6 to 1. On paper the second word uses `#C97700` and the cursor `#0E9F8E`.

## Type

- All text in the brand files is JetBrains Mono Bold, converted to outlines by `brand_build.py` (fontTools). The SVG
  files need no installed font. JetBrains Mono carries the SIL Open Font License 1.1.
- The build reads `/usr/share/fonts/TTF/JetBrainsMonoNerdFont-Bold.ttf`.

## Moods

| mood | face | when |
|---|---|---|
| awake | `{ o,o }` | ready for a request |
| thinking | `{ -,o }` | the main model works on its answer |
| working | `{ >,< }` | a tool runs |
| asking | `{ O,O }` | a command outside the safe list waits for an answer |
| learned | `{ ^,^ }` | a rule was stored in the memory map |
| asleep | `{ -,- }` | no key for 3 minutes |

All six are wired into the full screen interface (`cli/src/tui/art.rs`). The head there is `mark.svg` without pupils
at 22 x 22 pixels, two pixels per character cell. The pupils are set per mood as a 3 x 4 pixel pattern on the amber
eyes (columns 5 to 7 and 14 to 16, rows 9 to 12).

## Where the drawings come from

The project was called Kate until 2026-11-23 and carried a cat (commits 31bb467 and 19b7a81). The owl replaced it on
2026-11-24. The four drawings in `concepts/` were generated as SVG with GPT Image 2.6 through Higgsfield, one request
each, quality high, 1.25 credits each. Every prompt named the same colours, flat vector shapes, no text and no
gradients. No prompt held the name of the project.

| file | subject of the prompt | used for |
|---|---|---|
| `01-head.svg` | owl head from the front, ear tufts, facial disc, round amber eyes, brow line in a V | `mark.svg`, lockups, moods, the head in the terminal |
| `02-anvil.svg` | small owl sits on an anvil, three sparks at the right | `mascot.svg` |
| `03-outline.svg` | outline owl face on a tile, left eye a prompt sign, right eye a cursor block | `mark-outline.svg`, `mark-mono.svg` |
| `04-peeking.svg` | owl looks over the lower edge of a tile, anvil with two sparks above | `logo.svg`, `peek.svg`, banner, preview, start screen of the terminal |

The eyes of the moods other than awake, the lockups, the banner and the preview were composed by hand from these
drawings.

## Brief

Subject: an owl for a command line harness, with a forge. Style: flat vector, solid shapes. Palette: `#15131C`,
`#F6F3EC`, `#FFB43A`, `#2DD4BF`, `#B5B1C2`. Background: ink tile or transparent. Output: SVG, 1024 x 1024.
Variants: 4, one subject each.
