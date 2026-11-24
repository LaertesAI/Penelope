#!/bin/sh
# Renders the small pictures of the terminal interface as raw bytes. Usage: sh brand/build/term_art.sh (from the harness folder)
# The SVG files come from the brand build of the Laertes folder (brand/build/brand_build.py there).
set -e
out=cli/art
tmp=$(mktemp -d)
rsvg-convert -w 22 -h 22 -b "#1E1B28" brand/term-weave.svg -o "$tmp/weave.png"      # side panel, on the Night ground
rsvg-convert -w 22 -h 22 -b "#000000" brand/term-thread.svg -o "$tmp/thread.png"    # the loose thread as a mask
rsvg-convert -w 24 -h 24 -b "#15131C" brand/term-mark.svg -o "$tmp/mark.png"        # start screen
rsvg-convert -w 80 -h 16 -b "#15131C" brand/term-word.svg -o "$tmp/word.png"
magick "$tmp/weave.png" -alpha off -depth 8 "rgb:$out/weave_22x22.rgb"
magick "$tmp/thread.png" -alpha off -colorspace Gray -depth 8 "gray:$out/thread_22x22.gray"
magick "$tmp/mark.png" -alpha off -depth 8 "rgb:$out/mark_24x24.rgb"
magick "$tmp/word.png" -alpha off -depth 8 "rgb:$out/word_80x16.rgb"
rm -r "$tmp"
