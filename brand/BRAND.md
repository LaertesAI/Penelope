# Penelope brand

The mark of the harness is a woven square with one loose thread, painted in cream on terracotta `#C4623E`. Penelope wove
the shroud for Laertes by day and undid part of the work at night. The loose thread stands for that: the memory map can
take notes back.

<img src="lockup.png" alt="Penelope lockup" width="60%">

This folder holds only what the repository needs: the lockup for the README and the four SVG files the terminal
pictures are built from. The full set of marks is kept outside this repository.

| File | Use |
|---|---|
| `lockup.png` | weave picture, name and the word Harness, used at the top of the README |
| `term-weave.svg`, `term-thread.svg` | the weave without its loose thread, and the thread alone as a mask |
| `term-mark.svg`, `term-word.svg` | the mark and the name for the start screen |

## In the terminal

`sh brand/build/term_art.sh` renders four small pictures into `cli/art/`. The interface draws them with half block
characters, two pixels per character cell.

| Picture | Size in pixels | Where |
|---|---|---|
| `weave_22x22.rgb` | 22 by 22 | side panel |
| `thread_22x22.gray` | 22 by 22 | mask of the loose thread, coloured by the mood |
| `mark_24x24.rgb` | 24 by 24 | start screen |
| `word_80x16.rgb` | 80 by 16 | the name on the start screen |

Colours of the interface: ink `#15131C`, night `#1E1B28`, raised `#2B2739`, paper `#F6F3EC`, clay `#E8825A`,
sea `#5FB3E4`, ash `#B5B1C2`, dim `#7C778C`, red `#FF6B6B`. Clay is the terracotta of the mark, made lighter for dark grounds.

| Mood | Thread colour | Face as text |
|---|---|---|
| awake | clay | `[#|#]` |
| thinking, working | sea | `[#~#]`, `[#>#]` |
| asking | red | `[#?#]` |
| learned | paper | `[#+#]` |
| asleep | dim | `[#.#]` |
