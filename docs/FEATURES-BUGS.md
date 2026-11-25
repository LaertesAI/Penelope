# Penelope: features, bugs and open goals

State: 2026-11-25, working tree after the 0.11.0 second pass and the first 0.12 work. Test suite: 112 tests.
This is the honest list behind `README.md` and `PLAN-0.12.md`. A feature is only listed as working when a test or a
live run on this PC backs it.

## Working features

| Area | Feature | Evidence |
|---|---|---|
| Normal use | TUI, plain chat, `ask`, sessions, `/quit` | TUI started in tmux 120×40; plain chat completed a turn; `ask` returned `ok` |
| Contracts | 42 tools, one schema table, role gate | `spec` and `route` tests, every name reaches a handler |
| Agents | `$name(...)`, plans, roles, budgets, crew panel | parser, router and live one-step tests |
| Tools | read/write/edit, undo, find/grep/tree, jobs, git, clipboard, HTTP, JSON | unit tests and real changes |
| Search | 10 sources, cache, ranking, health, reranker client, SearXNG pool | live 8 hits; `search health --probe` |
| Search ops | `search probe [--daily]`, `search instances add URL` | unit test, daily marker, config write |
| Read | HTML → Markdown, local PDF, reader API, warnings | live PDF (24 words), reader tests |
| Server | `/search`, `/read`, `/embed`, `/rerank`, `/redact`, `/whoami` | 6 tests, live curl and client search |
| ZIP | `unzip` with zip-slip/symlink/size checks, plugin ZIP install | 3 tests, live ZIP and live refusal |
| Plugins | add, install git/local/ZIP, import, list, allow, deny, update, remove | 5 tests, live update/remove |
| MCP | plugin servers + standalone add/remove/allow/list/probe/call | 1 test, live echo server |
| Login | Claude official login, API keys, `auth verify` | live Claude verify, fake `/models` test |
| Privacy | fixed rules, optional filter model, leak audit | 0 findings on 15 files |
| Cost | euro per role/model | live rows |
| Worktree | `work new/list/diff/apply/remove` | 1 test, live git worktree |
| Injection | text, zero-width/bidi, CSS-hidden, hidden attribute, fake system lines | 2 tests |

## Bugs and weak spots

| ID | Severity | What is wrong | Evidence | Workaround / next |
|---|---|---|---|---|
| B01 | high | A worker still changes the live tree. `work` creates a worktree and applies a patch, but no agent runs inside it and no judge gate exists yet. | `work.rs` has no model call | Run the worker manually with `cwd` in the worktree; next: judge + apply |
| B02 | high | Keyless search stops when Startpage/DuckDuckGo/Bing rate-limit. | DuckDuckGo returned “refuses automated searches” in probes | Add a SearXNG pool; `search instances add` |
| B03 | medium | The test suite failed once with 105/109 and then passed eight times. The failing test was never identified. | one run on 2026-11-25 | Nightly 20× job; isolate process/port/temp tests |
| B04 | medium | Windows/macOS code paths are not run on hardware. CI marks them experimental. | no runner here | GitHub Actions matrix, fix failures |
| B05 | medium | MCP starts one process per call; a stateful server loses state. | mcpclient design | Use its own tool or a long-lived client (0.13) |
| B06 | medium | MCP permissions are per plugin, not per tool. | `plugins.json` allow switch | Add `allowTools` (0.13) |
| B07 | medium | `plugin install` blocks the TUI while git runs. | DEVLOG limit | Background job + progress view |
| B08 | medium | No model endpoints on the server: `/chat/completions`, `/route`, `/check`, `/filter`, `/train`. | `SERVER-APIS.md` | 0.13 |
| B09 | medium | No OAuth device flow for API providers; keys are pasted. | `auth.rs` | Auth broker (0.13) |
| B10 | medium | Server has no TLS, multi-user auth or audit log; one bearer token only. | `server.rs` | Reverse proxy + 0.13 |
| B11 | medium | `max_parallel` is stored but steps run in order. | `agents.rs` | DAG scheduler (0.13) |
| B12 | medium | No session budget hard stop, no model failover, no context budget. | `config.rs` | 0.13 |
| B13 | low | `unzip` handles ZIP only, not `.tar.gz`, `.7z`, `.docx`. | `files.rs` | 1.0 |
| B14 | low | Embeddings are feature hashes and rerank is BM25, not semantic. | `server.rs` | Local embedding model (1.0) |
| B15 | low | Plugin `remove` leaves the folder; no `--delete`. | `plugins.rs` | 1.0 |
| B16 | low | `auth verify` covers main/weak/scout, not search/reader keys. | `auth.rs` | 0.11.1 |
| B17 | low | `search probe` needs a cron/systemd timer; nothing schedules it. | `search.rs` | Add unit file in deploy docs |
| B18 | low | A scanned PDF still needs OCR. | `scrape.rs` | reader API or OCR |
| B19 | low | `mcp add` has no `env`/`cwd`/remote transport. | `plugins.rs` | 0.13 |
| B20 | low | The website account/admin views are real for login only; downloads and full admin pages are still mock. | `Laertes/website` | Website 0.12 |

## Goals for 0.12

- [ ] Worker runs inside `work new` and a judge checks the diff; no edit reaches the live tree without a yes.
- [ ] `search probe --daily` runs from a systemd timer; 20 queries survive a blocked engine.
- [ ] Attack corpus: 20 ScreenCodec injections + 20 normal pages; recall 100 %, false positives < 10 %.
- [ ] 20 consecutive test runs are green; the one flake is found.
- [ ] Linux, Windows and macOS CI run the same 110 tests; macOS is signed or labelled unsigned.
- [ ] `pip-audit` and `cargo audit` run in CI; a known CVE fails the job.
- [ ] A `deploy/` unit and README for the daily probe.

## How to reproduce the baseline

```bash
cd cli
cargo test                       # 112 tests
cargo build --release
./target/release/penelope doctor
./target/release/penelope work list
./target/release/penelope search probe --daily
./target/release/penelope auth verify
```
