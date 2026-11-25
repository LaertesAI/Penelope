# Design in Penelope — specification draft

Status: draft for the owner's acceptance. No code is built from this file yet. The name is **Design**; "Weaver" is a
second idea and "Da Vinci" is not used. This is not built on Claude Design.

## What a design is

A design is one deliverable that a cluster produces from a short brief and a set of source files. Penelope supports
three kinds, smallest first:

| Kind | Input | Output | First use |
|---|---|---|---|
| `poster` | title, subtitle, optional picture, palette | one PNG and one SVG at a chosen size | a launch card, a cover, a social image |
| `page` | copy, sections, optional picture and tokens | one self-contained HTML file, no external network | a landing page or a one-page brief |
| `motion` | a page or poster, a duration, a beat list | one MP4 or GIF plus the HTML source | a short loop or a title card |

A design is not a live app. It does not need a server, a database or a framework. It must open from disk and render
the same way on a machine without Penelope.

## Files

```
.penelope/designs/NAME/
  design.json        kind, size, title, palette, fonts, source files, checks
  design.html        page or the source of a poster/motion
  tokens.json        colours, spacing, type scale, radii, shadows
  assets/            local pictures and fonts only
  out/               final.png, final.svg, final.mp4 or final.gif
  report.txt         the cluster's report line and the validator's V:
```

`design.json` is the contract. A cluster writes it, the validator checks it, and the launcher reads it without a
model call. A file that escapes the design folder is refused.

## The cluster

A Design profile uses three parts:

- **composer** (`worker`): writes `design.json`, `design.html` and the assets.
- **renderer** (`worker` or `tool`): runs the local renderer and writes `out/`.
- **validator** (`judge`, Argos slot): opens `out/`, checks the brief, the text, the palette and the output size, and
  writes `V:` with the command and result.

The renderer is local. HTML to PNG/MP4 uses the browser or ffmpeg only when they are installed; without them the
cluster returns the HTML and says which program is missing. No external image service is called.

## Checks

A design passes only when all of these hold:

1. `design.json` parses and names a known kind and a size.
2. Every text in the output appears in the brief or in an allowed rewrite list.
3. `out/` holds a file with the expected type and a non-zero size.
4. The output opens from disk with no network request.
5. The palette and fonts in `tokens.json` are the only ones used in the output.
6. The validator's `V:` names the render command and its result.

A poster with a missing picture, a page with an external font, or a motion file that does not loop is a failed
cluster, not a warning.

## Commands

```text
/design new "one sentence brief"
/design list
/design open NAME
/design render NAME
/design remove NAME
```

The first command starts a Design cluster. `open` shows the file in the browser or the terminal picture layer. The
TUI may show a small preview. No design is uploaded anywhere by Penelope.

## What is deliberately not in the first version

- No cloud rendering, no image-generation service and no stock-photo API.
- No collaborative editing and no accounts.
- No design system editor; `tokens.json` is edited as a file.
- No live website; a page is one file that a person can host later.
- No automatic brand extraction from a website in the first version.

## Acceptance

The owner accepts this specification, or names the changes, before a package is written. The first implementation
package is a `poster` with a local palette and one local picture, because it is the smallest end-to-end proof.
