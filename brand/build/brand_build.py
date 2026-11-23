"""Builds the Kate brand files from the six concept drawings. Usage: python3 brand_build.py brand/concepts brand"""
import re
import sys
from pathlib import Path

src, out = Path(sys.argv[1]), Path(sys.argv[2])

INK, AMBER, PAPER, TEAL, ASH = "#15131C", "#FFB43A", "#F6F3EC", "#2DD4BF", "#B5B1C2"
MONO = "'JetBrains Mono', 'JetBrainsMono Nerd Font', ui-monospace, monospace"
NAMES = ["01-head", "02-sitting", "03-peeking", "04-outline", "05-asleep", "06-kitten"]
raw = [(src / f"{n}.svg").read_text() for n in NAMES]


def body(text):
    """The drawing without the svg element, the title and the dark tile."""
    inner = re.search(r"<svg[^>]*>(.*)</svg>", text, re.S).group(1)
    inner = re.sub(r"<title>.*?</title>", "", inner, flags=re.S)
    inner = re.sub(r'<rect[^>]*(?:width="1024"|width="1016")[^>]*fill="#15131C"\s*/>', "", inner, count=1)
    return inner.strip()


def group(text, gid):
    """One top-level group of a drawing by its id, with nested groups."""
    start = text.index(f'<g id="{gid}"')
    depth, pos = 0, start
    while True:
        m = re.compile(r"<g[ >]|</g>").search(text, pos)
        depth += 1 if m.group(0) != "</g>" else -1
        pos = m.end()
        if depth == 0:
            return text[start:pos]


def svg(viewbox, label, content, extra=""):
    return f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="{viewbox}" role="img" aria-label="{label}"{extra}>\n{content}\n</svg>\n'


head = group(raw[0], "cat")
sitting = body(raw[1])
peek_cat = group(raw[2], "cat")
peek_prompt = group(raw[2], "prompt")
outline = body(raw[3])
asleep = body(raw[4])

WORD = """<g fill="none" stroke="{c}" stroke-width="6.5" stroke-linecap="round" stroke-linejoin="round">
<path d="M4 3 V41 M4 31 L19 19 M10.5 26 L20 41"/>
<g transform="translate(31 0)"><circle cx="14" cy="29.5" r="11.5"/><path d="M25.5 18 V41"/></g>
<g transform="translate(68 0)"><path d="M8 7 V33 Q8 41 16 41 H19 M1 18 H18"/></g>
<g transform="translate(97 0)"><path d="M2.5 29.5 H25.5 A11.5 11.5 0 1 0 22.13 37.63"/></g>
</g>
<rect x="134" y="15" width="14" height="29.25" rx="3" fill="{k}"/>"""

# The app icon: the cat looks over the lower edge of a terminal, the prompt waits above it.
logo = f"""<defs><clipPath id="tile"><rect width="1024" height="1024" rx="232"/></clipPath></defs>
<g clip-path="url(#tile)">
<rect width="1024" height="1024" fill="{INK}"/>
{peek_prompt}
{peek_cat}
</g>"""
(out / "logo.svg").write_text(svg("0 0 1024 1024", "Kate: a cat looks over the edge of a terminal prompt", logo))

(out / "mark.svg").write_text(svg("82 120 850 850", "Kate mark: cat head with cursor eyes", head))
(out / "peek.svg").write_text(svg("96 500 826 484", "Kate looks over an edge", peek_cat))
(out / "mascot.svg").write_text(svg("120 90 780 850", "Kate sits, the tail ends in a prompt sign", sitting))
(out / "asleep.svg").write_text(svg("110 110 800 810", "Kate asleep", asleep))
(out / "mark-outline.svg").write_text(svg("115 127 790 790", "Kate mark, outline with prompt eyes", outline))
mono = outline.replace(AMBER, "currentColor").replace(TEAL, "currentColor")
(out / "mark-mono.svg").write_text(svg("115 127 790 790", "Kate mark, one colour", mono))

# Six moods: the head stays, the eyes change.
eyes_group = group(head, "eyes")
STROKE = f'fill="none" stroke="{INK}" stroke-width="34" stroke-linecap="round" stroke-linejoin="round"'
EYES = {
    "awake": eyes_group,
    "thinking": f'<path d="M340 548 H424" {STROKE}/><rect x="600" y="464" width="79" height="164" rx="35" fill="{INK}"/>',
    "working": f'<path d="M346 496 L420 546 L346 596 M676 496 L602 546 L676 596" {STROKE}/>',
    "asking": f'<g fill="{INK}"><circle cx="382" cy="546" r="66"/><circle cx="640" cy="546" r="66"/></g>'
              f'<g fill="{PAPER}"><circle cx="362" cy="522" r="19"/><circle cx="620" cy="522" r="19"/></g>',
    "learned": f'<path d="M338 584 Q382 486 426 584 M596 584 Q640 486 684 584" {STROKE}/>',
    "asleep": f'<path d="M340 556 H424 M598 556 H682" {STROKE}/>',
}
FACES = {"awake": "( o.o )", "thinking": "( -.o )", "working": "( >.< )", "asking": "( O.O )", "learned": "( ^.^ )", "asleep": "( -.- )"}


def mood_head(mood):
    return head.replace(eyes_group, f'<g id="eyes-{mood}">{EYES[mood]}</g>')


cells = []
for i, mood in enumerate(EYES):
    x = 40 + i * 232
    cells.append(f'<g transform="translate({x} 24) scale(.19)">{mood_head(mood)}</g>'
                 f'<text x="{x + 97}" y="250" text-anchor="middle" font-family="{MONO}" font-size="20" font-weight="700" fill="{PAPER}">{mood}</text>'
                 f'<text x="{x + 97}" y="280" text-anchor="middle" font-family="{MONO}" font-size="17" fill="{AMBER}" xml:space="preserve">{FACES[mood].replace(">", "&gt;").replace("<", "&lt;")}</text>')
    (out / f"mood-{mood}.svg").write_text(svg("82 120 850 850", f"Kate, {mood}", mood_head(mood)))
(out / "moods.svg").write_text(svg("0 0 1460 310", "Six moods of Kate", f'<rect width="1460" height="310" rx="24" fill="{INK}"/>\n' + "\n".join(cells)))

# Name alone and name with the head.
for tone, c, k in (("dark", PAPER, AMBER), ("light", INK, "#E08A00")):
    (out / f"wordmark-{tone}.svg").write_text(svg("0 -2 150 48", "kate", WORD.format(c=c, k=k)))
    lock = (f'<g transform="translate(0 0) scale(.2165)"><g transform="translate(-82 -120)">{head}</g></g>\n'
            f'<g transform="translate(222 38) scale(2.3)">{WORD.format(c=c, k=k)}</g>')
    (out / f"lockup-{tone}.svg").write_text(svg("0 0 572 184", "Kate logo with name", lock))

# README banner, 1280 x 320: the cat looks over the lower edge.
banner = f"""<defs><clipPath id="frame"><rect width="1280" height="320" rx="28"/></clipPath></defs>
<g clip-path="url(#frame)">
<rect width="1280" height="320" fill="{INK}"/>
<g transform="translate(72 58) scale(2.4)">{WORD.format(c=PAPER, k=AMBER)}</g>
<text x="72" y="222" font-family="{MONO}" font-size="27" font-weight="500" fill="{PAPER}">A command line harness that remembers the way.</text>
<text x="72" y="268" font-family="{MONO}" font-size="18" fill="{ASH}"><tspan fill="{AMBER}">$</tspan> katecli</text>
<g transform="translate(846 79) scale(.5)"><g transform="translate(-96 -500)">{peek_cat}</g></g>
</g>"""
(out / "banner.svg").write_text(svg("0 0 1280 320", "Kate, a command line harness that remembers the way", banner))

# Repository preview, 1280 x 640.
social = f"""<rect width="1280" height="640" fill="{INK}"/>
<g transform="translate(80 92) scale(2.6)">{WORD.format(c=PAPER, k=AMBER)}</g>
<text x="80" y="290" font-family="{MONO}" font-size="34" font-weight="500" fill="{PAPER}">A command line harness</text>
<text x="80" y="334" font-family="{MONO}" font-size="34" font-weight="500" fill="{PAPER}">that remembers the way.</text>
<g font-family="{MONO}" font-size="21" fill="#D9D5E3"><text x="80" y="392">Routes tool output through a smaller model.</text><text x="80" y="424">Keeps a map of what it learned.</text><text x="80" y="456">Sees the screen through ScreenCodec.</text></g>
<path d="M88 560 H620" fill="none" stroke="{TEAL}" stroke-width="2" stroke-dasharray="2 10" stroke-linecap="round"/>
<circle cx="88" cy="560" r="7" fill="{TEAL}"/><circle cx="265" cy="560" r="7" fill="{TEAL}"/><circle cx="442" cy="560" r="7" fill="{TEAL}"/><circle cx="620" cy="560" r="8" fill="{AMBER}"/>
<g transform="translate(690 91) scale(.56)">{peek_prompt}
{peek_cat}</g>"""
(out / "social-preview.svg").write_text(svg("0 0 1280 640", "Kate, a command line harness that remembers the way", social))
print("written:", ", ".join(sorted(p.name for p in out.iterdir())))
