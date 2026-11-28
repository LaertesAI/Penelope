# Memory: speed, quality, availability

Measured on 2026-11-28 with `cli/tests/data/memory_bench.py` against synthetic stores. No model is called. The owner's real memory is never read.

## What N means

N notes are N lines in waypoint notes (5 per waypoint) and N entries in Kerkis `memory.log` files (100 per cluster). 50 of them are known facts with unique tokens; the rest is noise from 4,000 made-up words. At 50,000 the store has 10,000 waypoints, 500 clusters, 19.4 MB of data and 57.9 MB on disk.

Latencies are milliseconds, p50 / p95 over 14 repeats (CLI, one process per call) or 30 to 50 calls (Kerkis, one server process). "Warm" is a call after the first one in the same server. A CLI call is always a new process, so its first run differs from the rest only by the file cache. The page cache was not dropped (no root), so cold means a new process, not a cold disk.

## Run it

```
python3 cli/tests/data/memory_bench.py --bin target/release/penelope --dir ~/.penelope/tmp/memory-bench/run \
    --notes 100,1000,10000,50000 --sections speed,quality,avail,privacy --json result.json
```

`--dir` must lie under `~/.penelope/tmp/`. The script sets `HOME` to a folder inside `--dir` for every call, so a store without a `config.json` never falls back to the real shared memory. Run it with `systemd-run --user --pipe --wait -p MemoryMax=6G`. A full run takes 80 seconds.

## Speed

| call | N | before | after |
|---|---|---|---|
| `memory find` (CLI) | 100 | 7.6 / 8.3 | 5.2 / 5.7 |
| | 1,000 | 20 / 22 | 15 / 16 |
| | 10,000 | 87 / 102 | 116 / 124 |
| | 50,000 | 294 / 307 | 629 / 657 |
| `memory go`, new walk | 100 | 2.0 / 2.2 | 2.1 / 2.2 |
| | 1,000 | 4.7 / 5.4 | 3.3 / 4.0 |
| | 10,000 | 28 / 33 | 19 / 22 |
| | 50,000 | 123 / 133 | 89 / 104 |
| `memory go`, stored walk | 10,000 | 18 / 21 | 14 / 15 |
| | 50,000 | 76 / 80 | 58 / 63 |
| `memory note` | 100 | 2.1 / 2.7 | 2.6 / 2.7 |
| | 1,000 | 3.4 / 3.9 | 4.1 / 4.7 |
| | 10,000 | 18 / 20 | 19 / 22 |
| | 50,000 | 77 / 87 | 79 / 86 |
| Kerkis `memory_find`, warm | 1,000 | 6.4 / 7.3 | 0.1 / 0.1 |
| | 10,000 | 23 / 27 | 0.5 / 0.6 |
| | 50,000 | 23 / 28 | 2.9 / 4.1 |
| Kerkis `memory_find`, first call | 10,000 | 85 | 105 |
| | 50,000 | 309 | 594 |
| Kerkis `memory_search` | 1,000 | 1.2 / 1.8 | 1.1 / 1.6 |
| | 10,000 | 11 / 14 | 13 / 14 |
| | 50,000 | 58 / 68 | 63 / 69 |
| Kerkis `memory_get` | 50,000 | 0.1 / 0.2 | 0.0 / 0.1 |
| Kerkis `memory_add` | 50,000 | 0.1 / 0.2 | 0.1 / 0.3 |
| `memory_get`, one log of 20,000 entries | | 18 | 0.4 |

Peak memory (RSS, MB) of `memory find` and of the Kerkis server after a find, before / after: 100 notes 11 / 11, 1,000 notes 13 / 13, 10,000 notes 21 / 29, 50,000 notes 68 / 101. A `memory note` at 50,000 uses 49 MB.

The "before" `memory find` is faster at 10,000 and above because it was wrong: the graph kept 3,000 nodes, so it searched 3,000 of 60,000 nodes of a 50,000-note store (see Quality). The "after" time is the price of searching all of it.

### Where the time goes

At 50,000 notes one cold `memory find` takes 600 ms. A timing taken before the last two optimizations split 900 ms into 150 ms for reading the 10,000 note files, 75 ms for parsing the 500 logs, 240 ms for linking and 370 ms for the word lists; the changes cut the linking (mention edges are skipped) and the word lists (no regex, fewer copies). A Kerkis server pays this once and then answers in 3 ms. A write (a note, a log entry) invalidates the cache, so the next `memory_find` pays it again: 12 ms at 1,000 notes, 105 ms at 10,000, 590 ms at 50,000. Eight servers that add and find on one store of 10,000 notes finished 1,000 calls in 14.4 s (4.9 s before the fix, when each rebuild searched only 3,000 nodes).

`memory note` and `memory go` read and write `map.json` as a whole: 6.2 MB at 50,000 notes. That is the 80 ms. The cost grows with the number of waypoints, not with the length of a note. `memory_search` reads every log on every call (63 ms at 50,000, no index). Before the fix `memory go` also copied the signpost words of a whole subtree per level and rewrote `map.json` on every stored walk.

Not fixed: a per-source cache so that one added log line does not rebuild the word lists of all notes (the next step if a Kerkis memory ever reaches 10,000 entries a day), and an index for `memory_search`.

## Quality

50 questions about 50 known facts, three kinds. A: the words of the stored sentence ("what is the owner of X"). B: the attribute as a synonym ("tell me the responsible for X"). C: the question from the value ("which one has V as its snapshot"). Recall is the share of questions whose fact is among the top 1 or top 5 pointers. `memory_search` is shown with the raw question and with the question words removed.

| N | tool | before @1 / @5 | after @1 / @5 |
|---|---|---|---|
| 100 | `memory_find` | 0.68 / 0.68 | 1.00 / 1.00 |
| 1,000 | `memory_find` | 0.68 / 0.68 | 1.00 / 1.00 |
| 10,000 | `memory_find` | 0.36 / 0.36 | 1.00 / 1.00 |
| 50,000 | `memory_find` | 0.08 / 0.08 | 1.00 / 1.00 |
| any | `memory_search`, raw question | 0.00 / 0.00 | 1.00 / 1.00 |
| any | `memory_search`, keywords only | 0.34 / 0.34 | 1.00 / 1.00 |

What was wrong: (1) the graph kept 3,000 nodes, so at 10,000 notes 64 percent of the facts were missing and at 50,000 92 percent. (2) A question word such as "which" or "has" counted in the denominator of the score, so a question with one matching word fell under the threshold of 0.2 (kind C failed at every N). (3) `memory_search` needs every word, so no paraphrase matched.

The 1.00 is an upper bound. The facts carry unique tokens, so every paraphrase still shares one rare token with its fact. The search is lexical: a question that shares no word with the note finds nothing at any N.

## Availability

| case | before | after |
|---|---|---|
| 1,000 sequential calls, 10,000 notes | 1000 answered, p50 11.4 then 11.7 ms, exit 0 | 1000 answered, p50 11.9 then 12.0 ms, exit 0 |
| 8 parallel servers, 1,000 calls, 400 adds | all answered, 400 unique ids, 400 in the log | same |
| 2 and 8 writers of `memory note` to one waypoint (50 and 200 notes) | 1 and 7 to 10 notes lost | 0 lost |
| `map.json` cut in half, then `memory note` | 112 waypoints, 3 left; the note files stay as orphans | 114 waypoints; old file kept as `map.json.corrupt-TIME`, map rebuilt from the notes |
| note file with non-UTF-8 bytes or garbage, then `memory note` into it | the file is replaced by 1 header and 1 line | readable lines kept, original bytes kept as `NAME.md.bad-TIME` |
| note file with a half-written last line | the half line stays | the half line stays a line of its own |
| read-only folder, `memory note` | prints "noted at", exit 0, nothing written | "NOT noted. cannot lock ...: Permission denied", exit 1 |
| read-only cluster folder, `memory_add` | waits 10 s, then "stayed locked" | "NOT added. cannot lock ...: Permission denied" at once |
| missing folder | creates it, answers "unknown" | same (an empty memory is no error) |
| 126 KB note by CLI, 10 MB line in a note file, 1 MB value by MCP | all work: 8 ms, `find` 48 ms, value cut to 1,000 characters | same |
| log line with bad bytes, bad bytes on stdin | the line is read, the server answers a parse error and goes on | same |
| `memory_add` into a log with 20,000 entries | 1.2 ms | 1.0 ms |

The loss from concurrent writers varies by run: with 8 writers 9, 7 and 10 of 200 notes in three runs. The race window is short, so a slower disk or more writers lose more. The two cases that destroyed data in every run were the cut `map.json` and the note file with bad bytes. Both are fixed.

## Privacy

Level 3 (person) nodes appear only with `--allow-person` (Kerkis) or `--include-person` (export). The bench checks this at every N:

| N | `memory_find` without the flag, level 3 pointers in 20 queries | export without the flag, level 3 nodes | `memory_add` with level 3 and no flag |
|---|---|---|---|
| 100 to 50,000 | 0 | 0 | refused |

With the flag the same queries return level 3 pointers (20 of 20) and the export holds them (25 at 100 notes, 3,000 at 50,000). `memory find` on the command line is the person's own tool and shows level 3 notes without a flag; that did not change.

One change in the export: without `--include-person` the notes are no longer read at all. Before, the 3,000-node cap was filled with notes that the export then dropped, so a default export of a 50,000-entry memory held 0 nodes. It now holds the newest 3,000 entries.

## What was fixed

1. Writes in `memory.rs` take the lock `memory.lock`, read `map.json` and `paths.json` again if another process changed them, and write through a temporary file and a rename (`fslock.rs`). Eight writers lose no note.
2. A damaged or missing `map.json` is kept as `map.json.corrupt-TIME` and rebuilt from the note files. A map that cannot be read (permission) refuses the write instead of starting an empty map.
3. A note file is read with replacement characters for bad bytes. Before a rewrite the original bytes are copied to `NAME.md.bad-TIME`.
4. `Memory::try_note` returns the error; `memory note` prints `NOT noted. REASON` and exits 1. `Memory::note` prints the error on stderr.
5. The lock fails in about 10 ms in a read-only folder (5 refused tries) instead of waiting 10 seconds. `memory_add` uses the same lock.
6. `memory_get` reads up to its line instead of parsing the log; `memory_add` counts lines in 64 KB blocks.
7. `memory_find` searches all waypoints and all log lines through word lists, ignores question words and unknown words in the score, and skips mention edges.
8. `memory_search` falls back to the entries with the rarest query words when no entry holds every word. The answer says `mode: ranked`.
9. `memory go` scores a subtree in one walk without copies, and a stored walk writes `paths.json` only. `paths.json` keeps at most 800 walks.
10. A default export does not read level 3 notes.

## Assumptions

- Timings come from one machine (16 threads, warm file cache) with the release profile. Compare before and after, not the absolute numbers.
- No fsync before the rename. A killed process cannot leave half a file; a power cut can still lose the last write.
- The lock is a file. A process killed while it holds the lock leaves it for 30 seconds.
