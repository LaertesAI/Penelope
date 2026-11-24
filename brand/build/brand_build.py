"""Builds the Strix Forgery brand files from the four owl drawings. Usage: python3 brand/build/brand_build.py (from the harness folder)"""
import re
from pathlib import Path
from fontTools.ttLib import TTFont
from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.pens.transformPen import TransformPen

NAME = ("strix", "forgery")                  # the two words of the name; every file below follows this line
COMMAND = "strix"
TAGLINE = "A command line harness that remembers the way."
out, src = Path("brand"), Path("brand/concepts")
INK, NIGHT, AMBER, PAPER, TEAL, ASH = "#15131C", "#1E1B28", "#FFB43A", "#F6F3EC", "#2DD4BF", "#B5B1C2"
FONT = TTFont("/usr/share/fonts/TTF/JetBrainsMonoNerdFont-Bold.ttf")      # SIL Open Font License 1.1
GLYPHS, CMAP, UPM = FONT.getGlyphSet(), FONT.getBestCmap(), FONT["head"].unitsPerEm


def outline(text, size, x=0.0, y=0.0):
    """The text as one SVG path with its baseline at y. Returns (path data, width)."""
    scale, parts, pen_x = size / UPM, [], x
    for ch in text:
        name = CMAP[ord(ch)]
        pen = SVGPathPen(GLYPHS)
        GLYPHS[name].draw(TransformPen(pen, (scale, 0, 0, -scale, pen_x, y)))
        parts.append(pen.getCommands())
        pen_x += GLYPHS[name].width * scale
    return " ".join(parts), pen_x - x


def svg(viewbox, label, content):
    return f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="{viewbox}" role="img" aria-label="{label}">\n{content}\n</svg>\n'


def group(text, gid):
    """One group of a drawing by its id, with nested groups."""
    start = text.index(f'<g id="{gid}"')
    depth, pos = 0, start
    while True:
        m = re.compile(r"<g[ >]|</g>").search(text, pos)
        depth += 1 if m.group(0) != "</g>" else -1
        pos = m.end()
        if depth == 0:
            return text[start:pos]


def body(text):
    """A drawing without the svg element, the title and the dark ground."""
    inner = re.search(r"<svg[^>]*>(.*)</svg>", text, re.S).group(1)
    inner = re.sub(r"<title>.*?</title>", "", inner, flags=re.S)
    return re.sub(r'<rect[^>]*fill="#15131C"\s*/>', "", inner, count=1).strip()


raw = {n: (src / f"{n}.svg").read_text() for n in ("01-head", "02-anvil", "03-outline", "04-peeking")}
head = group(raw["01-head"], "owl")
peek_owl = group(raw["04-peeking"], "owl").replace(' clip-path="url(#tile-clip)"', "")
forge = group(raw["04-peeking"], "forge")

# App icon: the owl looks over the lower edge of an ink tile, above it an anvil with two sparks.
logo = f"""<defs><clipPath id="tile"><rect width="1024" height="1024" rx="232"/></clipPath></defs>
<g clip-path="url(#tile)">
<rect width="1024" height="1024" fill="{INK}"/>
{forge}
{peek_owl}
</g>"""
(out / "logo.svg").write_text(svg("0 0 1024 1024", "Strix Forgery: an owl looks over the edge below an anvil with sparks", logo))
HEAD_BOX = "112 135 800 800"
(out / "mark.svg").write_text(svg(HEAD_BOX, "Strix Forgery mark: owl head with amber eyes", head))
(out / "peek.svg").write_text(svg("62 490 890 534", "The owl looks over an edge", peek_owl))
(out / "mascot.svg").write_text(svg("130 110 800 800", "The owl sits on an anvil", body(raw["02-anvil"])))
outline_owl = body(raw["03-outline"])
(out / "mark-outline.svg").write_text(svg("100 130 824 790", "Strix Forgery mark, outline with a prompt sign and a cursor as eyes", outline_owl))
mono = outline_owl.replace(AMBER, "currentColor").replace(TEAL, "currentColor").replace(PAPER, "currentColor").replace(f'fill="{INK}" stroke', 'fill="none" stroke')
(out / "mark-mono.svg").write_text(svg("100 130 824 790", "Strix Forgery mark, one colour", mono))

# Six moods: the head stays, the eyes change.
eyes, pupils = group(head, "eyes"), group(head, "pupils")
STROKE = f'fill="none" stroke="{INK}" stroke-width="34" stroke-linecap="round" stroke-linejoin="round"'
MOODS = {
    "awake": eyes + pupils,
    "thinking": eyes + f'<path d="M244 553 A104 104 0 0 1 452 553 Z" fill="{ASH}"/><path d="M244 553 H452" {STROKE}/>'
                       f'<path d="M297 553 A51 51 0 0 0 399 553 Z" fill="{INK}"/><circle cx="700" cy="524" r="51" fill="{INK}"/>',
    "working": eyes + f'<path d="M318 503 L390 553 L318 603 M706 503 L634 553 L706 603" {STROKE}/>',
    "asking": eyes + f'<g fill="{INK}"><circle cx="348" cy="553" r="72"/><circle cx="676" cy="553" r="72"/></g>'
                     f'<g fill="{PAPER}"><circle cx="324" cy="527" r="20"/><circle cx="652" cy="527" r="20"/></g>',
    "learned": eyes + f'<path d="M290 580 Q348 494 406 580 M618 580 Q676 494 734 580" {STROKE}/>',
    "asleep": f'<path d="M262 548 Q348 610 434 548 M590 548 Q676 610 762 548" {STROKE}/>',
}
FACES = {"awake": "{ o,o }", "thinking": "{ -,o }", "working": "{ >,< }", "asking": "{ O,O }", "learned": "{ ^,^ }", "asleep": "{ -,- }"}


def mood_head(mood):
    return head.replace(eyes, "").replace(pupils, f'<g id="eyes-{mood}">{MOODS[mood]}</g>')


cells = []
for i, mood in enumerate(MOODS):
    x = 40 + i * 232
    label, _ = outline(mood, 20, 0, 0)
    face, fw = outline(FACES[mood], 17, 0, 0)
    _, lw = outline(mood, 20, 0, 0)
    cells.append(f'<g transform="translate({x} 24) scale(.2425)"><g transform="translate(-112 -135)">{mood_head(mood)}</g></g>'
                 f'<path transform="translate({x + 97 - lw / 2:.1f} 250)" d="{label}" fill="{PAPER}"/>'
                 f'<path transform="translate({x + 97 - fw / 2:.1f} 280)" d="{face}" fill="{AMBER}"/>')
    (out / f"mood-{mood}.svg").write_text(svg(HEAD_BOX, f"The owl, {mood}", mood_head(mood)))
(out / "moods.svg").write_text(svg("0 0 1460 310", "Six moods of the owl", f'<rect width="1460" height="310" rx="24" fill="{INK}"/>\n' + "\n".join(cells)))


def word(size, x, y, main, accent, cursor, second=True):
    """The name in two colours with a cursor block at the end. Returns (svg, width)."""
    a, wa = outline(NAME[0], size, x, y)
    parts, w = [f'<path d="{a}" fill="{main}"/>'], wa
    if second:
        b, wb = outline(NAME[1], size, x + wa + size * 0.3, y)
        parts.append(f'<path d="{b}" fill="{accent}"/>')
        w += size * 0.3 + wb
    parts.append(f'<rect x="{x + w + size * 0.12:.1f}" y="{y - size * 0.74:.1f}" width="{size * 0.42:.1f}" height="{size * 0.92:.1f}" fill="{cursor}"/>')
    return "".join(parts), w + size * 0.54


for tone, main, accent, cursor in (("dark", PAPER, AMBER, TEAL), ("light", INK, "#C97700", "#0E9F8E")):
    w_svg, w = word(100, 0, 80, main, accent, cursor)
    (out / f"wordmark-{tone}.svg").write_text(svg(f"-4 -4 {w + 8:.0f} 112", " ".join(NAME), w_svg))
    w_svg, w = word(92, 200, 120, main, accent, cursor)
    tile = f'<rect width="176" height="176" rx="40" fill="{INK}"/>' if tone == "light" else ""          # paper head needs a dark ground
    lock = f'{tile}<g transform="translate(12 12) scale(.19)"><g transform="translate(-112 -135)">{head}</g></g>\n{w_svg}'
    (out / f"lockup-{tone}.svg").write_text(svg(f"0 0 {200 + w + 8:.0f} 176", "Strix Forgery logo with name", lock))
w_svg, w = word(100, 0, 80, PAPER, AMBER, AMBER, second=False)            # first word alone, for the start screen of the terminal
(out / "wordmark-short.svg").write_text(svg(f"-6 -6 {w + 12:.0f} 112", NAME[0], w_svg))

# README banner 1280 x 320 and repository preview 1280 x 640.
w_svg, w = word(88, 72, 128, PAPER, AMBER, TEAL)
t, _ = outline(TAGLINE, 27, 72, 200)
c1, c1w = outline("$", 19, 72, 258)
c2, _ = outline(COMMAND, 19, 72 + c1w + 12, 258)
by, _ = outline("a VeeAI harness", 19, 72 + c1w + 12 + 150, 258)
banner = f"""<defs><clipPath id="frame"><rect width="1280" height="320" rx="28"/></clipPath></defs>
<g clip-path="url(#frame)">
<rect width="1280" height="320" fill="{INK}"/>
{w_svg}
<path d="{t}" fill="{PAPER}"/>
<path d="{c1}" fill="{AMBER}"/><path d="{c2}" fill="{PAPER}"/><path d="{by}" fill="#7C778C"/>
<g transform="translate(905 113) scale(.388)"><g transform="translate(-62 -490)">{peek_owl}</g></g>
</g>"""
(out / "banner.svg").write_text(svg("0 0 1280 320", "Strix Forgery, " + TAGLINE, banner))

w_svg, w = word(84, 80, 170, PAPER, AMBER, TEAL)
l1, _ = outline("A command line harness", 34, 80, 262)
l2, _ = outline("that remembers the way.", 34, 80, 306)
lines = []
for i, text in enumerate(("Routes tool output through a smaller model.", "Keeps a map of what it learned.", "Sees the screen through ScreenCodec.")):
    p, _ = outline(text, 21, 80, 374 + i * 32)
    lines.append(f'<path d="{p}" fill="#D9D5E3"/>')
by, _ = outline("a VeeAI harness", 21, 80, 560)
social = f"""<rect width="1280" height="640" fill="{INK}"/>
{w_svg}
<path d="{l1}" fill="{PAPER}"/><path d="{l2}" fill="{PAPER}"/>
{''.join(lines)}
<path d="M88 506 H620" fill="none" stroke="{TEAL}" stroke-width="2" stroke-dasharray="2 10" stroke-linecap="round"/>
<circle cx="88" cy="506" r="7" fill="{TEAL}"/><circle cx="265" cy="506" r="7" fill="{TEAL}"/><circle cx="442" cy="506" r="7" fill="{TEAL}"/><circle cx="620" cy="506" r="8" fill="{AMBER}"/>
<path d="{by}" fill="#7C778C"/>
<g transform="translate(840 250) scale(.38)">{forge}
{peek_owl}</g>"""
(out / "social-preview.svg").write_text(svg("0 0 1280 640", "Strix Forgery, " + TAGLINE, social))
print("written:", ", ".join(sorted(p.name for p in out.iterdir() if p.is_file())))
