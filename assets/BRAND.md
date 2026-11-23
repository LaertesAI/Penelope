# Kate brand files

Kate is a small amber cat. Her eyes are two text cursors, the name ends in a cursor block.

## Files

| file | use |
|---|---|
| `logo.svg`, `logo.png` (512 px) | app icon: ink tile with the amber cat head |
| `favicon-32.png` | 32 px icon |
| `mark.svg` | cat head without tile, for dark grounds |
| `mark-mono.svg` | one colour, takes `currentColor`, eyes are cut out |
| `wordmark-dark.svg`, `wordmark-light.svg` | the drawn name for dark and light grounds |
| `lockup-dark.svg`, `lockup-light.svg` | mark plus name |
| `mascot.svg` | the sitting cat |
| `moods.svg` | six faces: awake, thinking, working, asking, learned, asleep |
| `banner.svg`, `banner.png` | README banner, 1280 x 320 |
| `social-preview.svg`, `social-preview.png` | repository preview, 1280 x 640 |

## Colours

| name | hex | use |
|---|---|---|
| Ink | `#15131C` | ground, eyes, text on paper |
| Night | `#1E1B28` | panels and cards on ink |
| Paper | `#F6F3EC` | text on ink, light ground |
| Amber | `#FFB43A` | the cat, the cursor, the prompt sign |
| Teal | `#2DD4BF` | memory path and waypoints, shared with ScreenCodec |
| Ash | `#B5B1C2` | second line of text on ink |

Amber on paper has a contrast of 1.6 to 1. Use it there for shapes only. The cursor block on paper uses `#E08A00`.

## Type

- Headings and running text: Space Grotesk 400, 500, 700.
- Commands, paths and terminal output: JetBrains Mono 400, 700.
- The name is drawn from strokes (6.5 on a height of 44, round ends). It is no font.
- `banner.png` and `social-preview.png` were rendered with JetBrains Mono for all text, because Space Grotesk was
  not installed on the build machine.

## Moods in the terminal

| mood | face | when |
|---|---|---|
| awake | `( o.o )` | ready for a request |
| thinking | `( -.o )` | the main model writes its answer |
| working | `( >.< )` | a tool runs and the smaller model reads its output |
| asking | `( O.O )` | a step needs a yes from the person first |
| learned | `( ^.^ )` | a rule or a walk was stored in the memory map |
| asleep | `( -.- )` | nothing to do |

`src/cat.rs` prints the awake face today. The other five faces are drawn and not wired in yet.

## Geometry

All marks share one head on a 256 grid: two ear triangles `56,122 64,54 112,96` (mirrored at x = 128) and a body
rectangle `52,92` with size `152 x 112` and corner radius 52, all with a 12 unit round outline. Eyes are
`20 x 40` blocks with radius 8 at x = 88 and x = 148.

## Brief

```
Subject:     small cat head, eyes are text cursors, for a command line harness
Style:       flat vector, two colours, round joins
Palette:     #15131C, #FFB43A, #F6F3EC, #2DD4BF
Background:  transparent for marks, ink tile for the app icon
Output:      SVG, PNG exports with rsvg-convert
Size:        256 grid for marks, 1280 x 320 banner, 1280 x 640 preview
Variants:    1 direction, drawn by hand as SVG, no image generator
```
