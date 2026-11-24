#!/bin/sh
# Renders the small pictures of the terminal interface as raw RGB bytes. Usage: sh brand/build/term_art.sh (from the harness folder)
set -e
out=cli/art
tmp=$(mktemp -d)
# The head without pupils on the Night ground. The view box puts the pupils on columns 5 to 7 and 14 to 16, rows 9 to 12.
python3 - brand/mark.svg "$tmp/head.svg" <<'PY'
import re, sys
text = open(sys.argv[1]).read()
text = re.sub(r'<g id="pupils".*?</g>', "", text, flags=re.S)
open(sys.argv[2], "w").write(text.replace('viewBox="112 135 800 800"', 'viewBox="112 153 800 800"'))
PY
rsvg-convert -w 22 -h 22 -b "#1E1B28" "$tmp/head.svg" -o "$tmp/head.png"
rsvg-convert -w 40 -h 24 -b "#15131C" brand/peek.svg -o "$tmp/peek.png"
rsvg-convert -w 60 -h 20 -b "#15131C" brand/wordmark-short.svg -o "$tmp/word.png"
magick "$tmp/head.png" -depth 8 "rgb:$out/head_22x22.rgb"
magick "$tmp/peek.png" -depth 8 "rgb:$out/peek_40x24.rgb"
magick "$tmp/word.png" -depth 8 "rgb:$out/word_60x20.rgb"
rm -r "$tmp"
