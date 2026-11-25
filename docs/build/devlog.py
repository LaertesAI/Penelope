#!/usr/bin/env python3
"""Build the Penelope devlog PDF from docs/DEVLOG.md.

    python3 docs/build/devlog.py

Needs Chromium. The Markdown subset here is the one the devlog uses: headings, paragraphs, bold, inline code, links,
images, lists, tables and fenced code. No external Python package.
"""

import html
import re
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
DOCS = ROOT / "docs"
SOURCE = DOCS / "DEVLOG.md"
PDF = DOCS / "Penelope 0.11.0 Devlog.pdf"
FONT = "ui-monospace, 'JetBrains Mono', 'DejaVu Sans Mono', monospace"

CSS = """
@page { size: A4; margin: 18mm 17mm 18mm 17mm; }
* { box-sizing: border-box; }
body { font: 9.4pt/1.45 'DejaVu Sans', 'Helvetica', sans-serif; color: #1C1A17; margin: 0; }
h1 { font: 400 26pt/1.05 'DejaVu Serif', Georgia, serif; letter-spacing: .02em; margin: 0 0 3mm; color: #1C1A17; }
h2 { font: 600 14pt/1.2 'DejaVu Serif', Georgia, serif; color: #C4623E; margin: 7mm 0 2.5mm; padding-top: 2mm;
     border-top: 1.5pt solid #1C1A17; break-after: avoid; }
h3 { font: 600 10.5pt/1.25 'DejaVu Sans', sans-serif; color: #1C1A17; margin: 5mm 0 1.5mm; break-after: avoid; }
p { margin: 0 0 2.2mm; }
strong { font-weight: 650; }
em { font-style: italic; }
code { font: 8.6pt 'DejaVu Sans Mono', monospace; background: #F1EDE6; padding: .3mm 1mm; border-radius: 1mm; }
pre { background: #1C1A17; color: #F6F3EC; padding: 2.5mm 3mm; border-radius: 1.5mm; font: 8.3pt/1.4 'DejaVu Sans Mono', monospace;
      white-space: pre-wrap; word-break: break-word; break-inside: avoid; margin: 2mm 0 3mm; }
pre code { background: none; color: inherit; padding: 0; }
table { border-collapse: collapse; width: 100%; margin: 1.5mm 0 4mm; font-size: 8.4pt; break-inside: avoid; }
th { text-align: left; font-weight: 650; border-bottom: 1pt solid #1C1A17; padding: 1.2mm 2mm 1.2mm 0; vertical-align: bottom; }
td { border-bottom: .35pt solid #CFC9BD; padding: 1.2mm 2mm 1.2mm 0; vertical-align: top; }
td:first-child { font-weight: 600; }
ul, ol { margin: 0 0 2.5mm; padding-left: 5mm; }
li { margin-bottom: .9mm; }
a { color: #C4623E; text-decoration: none; }
figure { margin: 3mm 0 4mm; break-inside: avoid; }
figure img { width: 100%; border: .4pt solid #CFC9BD; border-radius: 1.5mm; }
figure figcaption { font-size: 8pt; color: #6F6A62; margin-top: 1mm; }
.lead { color: #6F6A62; font-size: 10pt; margin-bottom: 5mm; }
.meta { border-left: 2.5pt solid #C4623E; padding: 1mm 0 1mm 3mm; margin: 0 0 6mm; color: #4A453E; }
.pagebreak { break-before: page; }
"""


def inline(text):
    """Bold, inline code and links, with HTML escaped first."""
    out = html.escape(text, quote=False)
    out = re.sub(r"`([^`]+)`", lambda m: f"<code>{m.group(1)}</code>", out)
    out = re.sub(r"\*\*([^*]+)\*\*", r"<strong>\1</strong>", out)
    out = re.sub(r"(?<!\*)\*([^*]+)\*(?!\*)", r"<em>\1</em>", out)
    out = re.sub(r"\[([^\]]+)\]\(([^)]+)\)", r'<a href="\2">\1</a>', out)
    return out


def convert(md):
    lines = md.splitlines()
    out = []
    i = 0
    in_list = None
    paragraph = []

    def flush():
        nonlocal paragraph
        if paragraph:
            out.append("<p>" + inline(" ".join(paragraph)) + "</p>")
            paragraph = []

    def close_list():
        nonlocal in_list
        if in_list:
            out.append(f"</{in_list}>")
            in_list = None

    while i < len(lines):
        line = lines[i]
        stripped = line.strip()
        if stripped.startswith("```"):
            flush(); close_list()
            lang = stripped[3:].strip()
            i += 1
            code = []
            while i < len(lines) and not lines[i].strip().startswith("```"):
                code.append(lines[i])
                i += 1
            out.append(f'<pre><code class="language-{html.escape(lang)}">{html.escape(chr(10).join(code))}</code></pre>')
            i += 1
            continue
        if not stripped:
            flush(); close_list()
            i += 1
            continue
        m = re.match(r"^(#{1,3})\s+(.*)$", stripped)
        if m:
            flush(); close_list()
            level = len(m.group(1))
            out.append(f"<h{level}>{inline(m.group(2))}</h{level}>")
            i += 1
            continue
        m = re.match(r"^!\[([^\]]*)\]\(([^)]+)\)$", stripped)
        if m:
            flush(); close_list()
            src = (DOCS / m.group(2)).resolve().as_uri() if not m.group(2).startswith("http") else m.group(2)
            out.append(f'<figure><img src="{src}" alt="{html.escape(m.group(1))}"><figcaption>{inline(m.group(1))}</figcaption></figure>')
            i += 1
            continue
        if stripped.startswith("|") and i + 1 < len(lines) and re.match(r"^\|[\s:|-]+\|$", lines[i + 1].strip()):
            flush(); close_list()
            header = [c.strip() for c in stripped.strip("|").split("|")]
            rows = []
            i += 2
            while i < len(lines) and lines[i].strip().startswith("|"):
                rows.append([c.strip() for c in lines[i].strip().strip("|").split("|")])
                i += 1
            out.append("<table><thead><tr>" + "".join(f"<th>{inline(c)}</th>" for c in header) + "</tr></thead><tbody>")
            for row in rows:
                out.append("<tr>" + "".join(f"<td>{inline(c)}</td>" for c in row) + "</tr>")
            out.append("</tbody></table>")
            continue
        m = re.match(r"^[-*]\s+(.*)$", stripped)
        if m:
            flush()
            if in_list != "ul":
                close_list(); out.append("<ul>"); in_list = "ul"
            out.append(f"<li>{inline(m.group(1))}</li>")
            i += 1
            continue
        m = re.match(r"^\d+\.\s+(.*)$", stripped)
        if m:
            flush()
            if in_list != "ol":
                close_list(); out.append("<ol>"); in_list = "ol"
            out.append(f"<li>{inline(m.group(1))}</li>")
            i += 1
            continue
        if stripped in ("---", "***"):
            flush(); close_list()
            out.append("<hr>")
            i += 1
            continue
        paragraph.append(stripped)
        i += 1
    flush(); close_list()
    return "\n".join(out)


def main():
    md = SOURCE.read_text(encoding="utf-8")
    body = convert(md)
    # The first heading and the block under it become the cover block.
    body = body.replace("<h1>", '<h1 class="title">', 1)
    page = f"""<!doctype html><html><head><meta charset="utf-8"><title>Penelope 0.11.0 Devlog</title>
<style>{CSS}</style></head><body>
<div class="meta">Penelope · the harness of Laertes · 2026-11-24</div>
{body}
</body></html>"""
    tmp = Path(tempfile.mkdtemp(prefix="penelope-devlog-")) / "devlog.html"
    tmp.write_text(page, encoding="utf-8")
    cmd = ["chromium", "--headless", "--disable-gpu", "--no-sandbox", "--no-pdf-header-footer",
           f"--print-to-pdf={PDF}", tmp.as_uri()]
    result = subprocess.run(cmd, capture_output=True, text=True)
    if result.returncode != 0 or not PDF.exists():
        print(result.stderr[-2000:], file=sys.stderr)
        sys.exit(1)
    print(f"{PDF} · {PDF.stat().st_size} bytes · {len(md.splitlines())} lines of Markdown")


if __name__ == "__main__":
    main()
