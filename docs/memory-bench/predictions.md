# Predictions before the first measurement

Written 2026-11-28 from reading `memory.rs`, `graph.rs`, `mcpmem.rs`. No number below was measured yet.

Store shape: N notes means N waypoint note lines (5 per waypoint, so N/5 waypoints) and N Kerkis log entries (100 per cluster). 50 of the notes are known facts, the rest is noise.

## Speed (CLI = one process per call, p50)

| call | 100 | 1,000 | 10,000 | 50,000 |
|---|---|---|---|---|
| memory find | 25 ms | 70 ms | 400 ms | 1.8 s |
| memory go (new walk) | 12 ms | 20 ms | 120 ms | 400 ms |
| memory note | 12 ms | 15 ms | 60 ms | 250 ms |
| Kerkis memory_search (warm) | 8 ms | 20 ms | 150 ms | 800 ms |
| Kerkis memory_get (warm) | 1 ms | 1 ms | 1 ms | 2 ms |
| Kerkis memory_find (first call / warm) | 25 / 1 ms | 70 / 1 ms | 400 / 2 ms | 1.8 s / 5 ms |

Peak RSS of `memory find`: 40 MB, 60 MB, 250 MB, 900 MB. Peak RSS of one memory_search process at 50,000: 350 MB.

Where the time goes:
1. `memory find` and the first memory_find build the whole graph: every note file and every log line is read and parsed per process. The 30 s cache cannot help a CLI call.
2. `memory go` scores a child by collecting the signposts of its whole subtree into a Vec of Strings, per level. Cost grows with the map, not with the path.
3. `memory note` and a new `go` walk rewrite `map.json` and `paths.json` as pretty JSON. Cost grows with the map.
4. memory_search re-reads and lowercases every log on every call. There is no index.

Cold start (first call after generating the store) is 1.3 to 2 times the warm p50 at 10,000 and above, because the files come from disk.

## Retrieval quality (50 paraphrased queries)

| tool | recall@1 | recall@5 |
|---|---|---|
| memory find at 100 to 10,000 | 0.55 | 0.80 |
| memory find at 50,000 | 0.10 | 0.15 (the graph keeps 3,000 nodes, the facts are cut) |
| memory_search | 0.30 | 0.30 (every query word must appear, a paraphrase misses) |

## Availability

| case | prediction |
|---|---|
| 1,000 sequential calls to one server | 1000 of 1000 answered, no growth in latency |
| 8 parallel servers on one store | all answered, all memory_add ids unique (a lock exists) |
| corrupt `map.json`, then `memory note` | all waypoints leave the map, the note files stay as orphans. Data loss |
| non-UTF-8 note file, then `memory note` to it | the file is replaced by a new header and one line. Data loss |
| two processes writing notes at once | 30 to 60 percent of 200 notes lost (read, modify, write of `map.json` and the note file without a lock) |
| missing folder | no error, an empty answer, the folder is created |
| 10 MB note | accepted, `find` 1 s or more |
| read-only folder, `memory note` | prints "noted at ..." and writes nothing. Silent loss |
| torn last line of a `memory.log` | tolerated (the code says so) |

## Privacy

Without `--allow-person` memory_find returns no level 3 pointer at every N. The bench changes nothing there. `memory find` on the CLI shows level 3 notes without a flag (it is the person's own tool), I expect this and will report it.
