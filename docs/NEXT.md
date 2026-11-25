# What comes next

A plan, not a promise. Each row has a reason, a first step and a way to see whether it worked. "Now" is built in the
working tree of 0.11.0; the rest is ordered by risk to the user, not by how interesting it is.

> The detailed gap audit and the 0.11.1–1.0 sequence, with acceptance tests and effort, are in
> [`PLAN-0.12.md`](PLAN-0.12.md). This file stays the short status list.

## Now (the second 0.11.0 pass)

| Part | State | Evidence |
|---|---|---|
| Contract layer for 42 tools | built | 2 tests, every name reaches a handler |
| Login for Claude and 35 providers | built | live dummy key in the Secret Service |
| Six modes incl. a refusing `safe` | built | 1 test, live `/mode safe` |
| Tool role and `computer` | built | live page, counter 0 |
| Shared memory, Obsidian brain | built | 1 test, screen picture |
| Cost in euro | built | live rows |
| Search: 10 sources, cache, rank, scraper, research | built | 9 unit tests, live 8 results, live cited report |
| Server search API client, multi-SearXNG, engine health, optional reranker | built | 3 tests, strict shapes, fallback on a broken answer |
| PDF through `pdftotext` and a guarded reader API client | built | live PDF, 2 tests |
| Local server: search, read, embed, rerank, redact | built | 6 tests, live HTTP calls, [`SERVER.md`](SERVER.md) |
| Model endpoints of the eight families | not built | contracts in [`SERVER-APIS.md`](SERVER-APIS.md) |
| Plugins from GitHub, Claude and ZIP | built | 178 skills from one GitHub repo, 4 installed Claude plugins, live ZIP |
| Plugin lifecycle: update and remove | built | 1 test, live update/remove |
| Standalone MCP servers: add, remove, allow, call | built | 1 test, live echo server |
| Login verification | built | live Claude verify, fake `/models` test |
| `unzip` tool with zip-slip, symlink and size limits | built | 2 tests, live ZIP, one question |
| Self-written skills | built | live 5-call turn wrote a playbook |
| Native Wayland cursor and uinput click | built | live counter 0 → 1 → 2 |
| Worktree lifecycle: new, list, diff, apply, remove | built | 1 test, live git worktree |
| Daily search probe and SearXNG instance list | built | 1 test, live probe marker |
| CSS/fake-system injection warnings | built | 2 tests, live hidden HTML |
| CI matrix, `cargo audit` and `pip-audit` | built | workflow files; Windows/macOS experimental |
| Leak audit and retry | built | 112 tests, live 0 findings |
| Updater client and blueprint | built | 1 test, no server |

## P0 — before another person uses it daily

### 1. Worker in a worktree, with a judge gate

A worker that changes files in a copy of the repository, the judge reads the diff and the tests, the person sees one
diff and says yes or no. This removes the largest remaining risk: an agent writing in the live tree.

- First step: `git worktree add .penelope/worktrees/ID -b agent/ID`, run the worker with `cwd` in the worktree, copy
  the diff back only after the judge and the person agree.
- Acceptance: 10 tasks from the Telemach set; no task touches the live tree before the yes; a failed judge leaves the
  worktree for inspection; `penelope worktrees` lists and removes them.
- Measure: solved tasks, lead tokens, wall time, wrong "done" claims caught.

### 2. Prompt-injection hardening (partly built)

`scrape::scan` now catches instruction lines, zero-width and bidi text, and `fetch` and `research` carry the warning.
CSS-hidden blocks, fake `system:`/`assistant:` blocks and suspicious citation links are still missing.

- Next step: extend the same scanner with CSS visibility, hidden attributes, tiny/off-screen text and a host check for
  links; keep the result a warning, never a silent drop.
- Acceptance: 0 of the known attacks reach the lead without a warning; a false-positive set of 20 normal pages stays
  below 10 %.
- Measure: attacks caught, false positives, added milliseconds.

### 3. Search resilience (partly built)

Startpage is the working keyless source, `search health` records every engine, `search.urls` accepts a SearXNG pool and
`search.api_url` can point at `penelope serve`. A daily probe and a one-line `search instances add URL` are missing.

- Next step: a `search probe --daily` job and a documented Docker SearXNG, so the fallback order is tested every day.
- Acceptance: 20 queries in a row still return results from at least one engine; `penelope doctor` says which source is
  alive and when it was last checked.
- Measure: success rate over 100 queries, latency, cache hit rate.

### 4. Windows and macOS builds that are actually run

The source has `cmd /C`, `taskkill`, `clip`, PowerShell pointer code, `PATHEXT` and a Quartz/cliclick path. None was run
on the named OS.

- First step: a GitHub Actions matrix (`ubuntu-24.04`, `windows-2025`, `macos-15`) that runs `cargo test`, the
  contract test and a `--help` smoke test; sign the macOS binary with a Developer ID when one exists.
- Acceptance: 112 tests green on all three; one live `ask` with a local model on each; `penelope input status` chooses the
  right backend.
- Measure: build time, test failures, install size.

## P1 — the next month

### 5. Parallel subagents with one scheduler

`max_parallel` is stored but steps run in order. Add a scheduler that runs independent read-only steps at once and
keeps dependent steps in order. A worker and a scout may overlap; a worker and its judge may not.

- First step: a small DAG in the agent plan (`depends_on` per step), a fixed worker pool, one shared budget.
- Acceptance: two scouts finish in about the slower one; the judge never starts before its worker; cancellation stops
  the pool in under 2 s.
- Measure: wall time against sequential, token cost, cancellation latency.

### 6. Model failover and a real context budget

A provider can return 429/5xx or run out of context. A fallback model and a measured budget per turn make the harness
predictable.

- First step: `models.main.fallbacks: [...]`, a pre-flight token estimate, a compact step when the history crosses 80 %
  of the window.
- Acceptance: a 429 on the first model is answered by the second without losing the turn; a 200k-token session stays
  under the window; `cost` shows the fallback.
- Measure: turns rescued, extra tokens, latency.

### 7. A per-session and per-agent budget in euro

The agent has `budget_eur`, the turn does not. Add `session_budget_eur` and a warning at 80 %. A hard stop must be
clear: a message, not a silent failure.

- First step: one counter in `usage.json`, checked before each model call.
- Acceptance: a live session stops at the limit; the next turn needs an explicit `--over-budget`; `cost` shows the
  break down.
- Measure: budget overshoot, false stops.

### 8. MCP registry with one-tool permissions

`mcp_call` starts one process per call and the allow switch is per plugin, not per tool. A long-lived client and a
per-tool allow list make a browser or database server usable.

- First step: the `screen.rs` bridge generalized to any MCP server (one child, request ids, timeout, restart), a
  `plugins.json` entry `"allowTools": ["read", "search"]`.
- Acceptance: a stateful server keeps its state across two calls; a denied tool is refused by name; a crashed server
  restarts once.
- Measure: state held, restarts, leak of processes.

### 9. The update server

`docs/AUTOUPDATE.md` is ready. The missing part is a static host, a signed stable/beta manifest and a CDN.

- First step: a GitHub release job that builds all platforms, computes SHA-256, signs the manifest with minisign and
  uploads it to `updates.laertes.ai`.
- Acceptance: a 0.11.0 install updates to the next build on Linux and Windows; a wrong checksum installs nothing; the
  old binary is kept.
- Measure: update success, rollback success, bytes downloaded.

## P2 — later, if the daily work asks for it

| Part | Why | First step |
|---|---|---|
| Hybrid memory search | the tree is fast but a word can be in an unexpected branch | local embeddings over waypoints, top-5 by cosine plus the tree walk |
| Obsidian-like interactive graph | a picture is not a browser | clickable nodes in the TUI, filters by topic, open a note in the pager |
| Per-tool plugin permission | one bad tool should not open a whole server | the MCP registry above |
| Scraper for Office files | many sources are `.docx` or `.odt` | unzip `document.xml`, keep the text; PDF is already local |
| A semantic local reranker | BM25 exists, but misses synonyms and paraphrase | a real local embedding model behind `/embed` and `/rerank`, top-8 by cosine |
| Long-term task queue | a task can outlive a session | `.penelope/tasks/` with state, owner, budget, next step |
| Conversation handoff | a second session can continue | a signed `handoff.json` with memory path, plan, decisions |
| Local voice | hands-free on the PC | optional TTS/STT commands, no new model |

## Not planned

- A graphical window of its own. The TUI and ScreenCodec cover the screen.
- Training the eight families inside this repository. The briefs and experiments live under `Laertes/models`.
- A public plugin registry with automatic trust. Install and import stay explicit.
- Reading a page with a cloud scraper by default. Jina, Firecrawl and Exa are opt-in.

## How progress is judged

Every row above needs three numbers before it is called done: a functional acceptance test, a cost measure, and one
real run in `docs/RUNS.md`. A feature that cannot name those three stays in this file.
