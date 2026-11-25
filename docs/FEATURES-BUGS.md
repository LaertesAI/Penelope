# Penelope: features, bugs and open goals

State: 2026-11-25, working tree after the 0.11.0 second pass and the first 0.12 work. Test suite: 240 tests (227 unit and 2 eval command tests).
This is the honest list behind `README.md` and `PLAN-0.12.md`. A feature is only listed as working when a test or a
live run on this PC backs it.

## Working features

| Area | Feature | Evidence |
|---|---|---|
| Normal use | TUI, plain chat, `ask`, sessions, `/quit` | TUI started in tmux 120×40; plain chat completed a turn; `ask` returned `ok` |
| Contracts | 46 tools, one schema table, role gate | `spec` and `route` tests, every name reaches a handler |
| Agents | `$name(...)`, plans, roles, budgets, crew panel | parser, router and live one-step tests |
| Clusters | orchestrator plan, profiles from parts, parallel clusters, `after`/parent order, one memory file, validator retry, reports, kill | 5 tests, live solo cluster with Claude + haiku validator |
| Cluster base modules | `proof` (evidence `V:`), `notes` (linked notes and status), `advise` (part scores and suggestions) | 27 tests, not wired yet |
| Tools | read/write/edit, undo, find/grep/tree, jobs, git, clipboard, HTTP, JSON | unit tests and real changes |
| Search | 10 sources, parallel fanout, cache, dedupe/rank, health, reranker client, SearXNG pool | live 3 engines; `search health --probe` |
| Search ops | `search probe [--daily]`, `search instances add URL`, `search fanout on|off` | 2 tests, live 3 engines, daily marker |
| Read | HTML → Markdown, local PDF, reader API, warnings | live PDF (24 words), reader tests |
| Server | `/search`, `/read`, `/embed`, `/rerank`, `/redact`, `/whoami` | 6 tests, live curl and client search |
| ZIP | `unzip` with zip-slip/symlink/size checks, plugin ZIP install | 3 tests, live ZIP and live refusal |
| Plugins | add, install git/local/ZIP, import, list, allow, deny, update, remove, commands, filterable screen | 7 tests, live command turn |
| MCP | plugin servers + standalone add/remove/allow/list/probe/call | 1 test, live echo server |
| Login | Claude official login, API keys, `auth verify` | live Claude verify, fake `/models` test |
| Privacy | fixed rules, optional filter model, leak audit | 0 findings on 15 files |
| Cost | euro per role/model | live rows |
| Worktree | `work new/list/diff/apply/remove` | 1 test, live git worktree |
| Injection | text, zero-width/bidi, CSS-hidden, hidden attribute, fake system lines | 2 tests |
| Interface | crisp text start screen, weaving animation on a new session, F5 new, F6 delete | tmux captures, TUI tests |
| Models | provider list, masked API-key window, Claude login, role models, provider model list with cache and fallback, per-model effort | 5 tests, live OpenCode list |
| Budget | monthly spend and remaining in the side panel and `/cost` | 1 cost test |
| Checkpoints | checkpoint per turn, `/rewind N` restores transcript and files | 2 tests |
| Sandbox | `bwrap` with read-only root, one writable folder and no network | 1 test, live `echo` |
| Allowlist | confirmed commands hashed per project, `penelope allow` | 1 test |
| CI stream | `penelope ask --json` events | live Claude JSON stream |
| PR | `pr_create` with a question, never pushes | spec/handler, no live PR |

## Bugs and weak spots

| ID | Severity | What is wrong | Evidence | Workaround / next |
|---|---|---|---|---|
| B01 | high | A worker still changes the live tree. `work` creates a worktree and applies a patch, but no agent runs inside it and no judge gate exists yet. | `work.rs` has no model call | Run the worker manually with `cwd` in the worktree; next: judge + apply |
| B02 | high | Keyless search stops when Startpage/DuckDuckGo/Bing rate-limit. | DuckDuckGo returned “refuses automated searches” in probes | Add a SearXNG pool; `search instances add` |
| B03 | low | The test suite failed once with 105/109 and then passed. The failing test was never identified; 20 complete runs in a row now pass. | one old run; 20/20 green on 2026-11-25 | Nightly 20× job; isolate process/port/temp tests |
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
| B21 | medium | No test loop after an edit and no stop after three failed attempts. | no `test_command` runner | Add `test_command` and a bounded repair loop |
| B22 | medium | No LSP tool; rust-analyzer is installed but not wired. | `which rust-analyzer` | `lsp` tool with diagnostics and types |
| B23 | medium | No tree-sitter repo index; grep and tree are the only overview. | no tree-sitter dependency | Index files and symbols at start |
| B24 | medium | The eval runner and read-only replay exist; no real 20-task run with three models has been made yet. | `eval.rs`, `docs/EVAL.md` | First real table in `docs/RUNS.md` |
| B25 | medium | No ACP server for Zed/Neovim. | no ACP module | JSON-RPC endpoint over stdio |
| B26 | medium | The Laertes provider is a placeholder without a plan key or a model list. | `providers.rs` | Add the plan endpoint and models |
| B27 | low | `/rewind` restores the transcript and files but starts a fresh chat history; the next HTTP turn does not see the rewound messages. | `rewind` in TUI | Rebuild the model history from the checkpoint |
| B28 | low | Research and web search share the scout role; they cannot have separate models yet. | `ROLE_MODELS` | Split the research tool onto its own role |
| B29 | low | The model list comes from the provider, but the effort column is still inferred by model family when the endpoint does not report it. A custom model name can show `no effort` even when its API accepts the field. | `providers::supports_effort`, `fetch_models` | Add an explicit per-setting override in 0.13 |
| B31 | low | Plugin commands expand `$ARGUMENTS`, `$@` and `$1` to `$9`; Claude command features such as `!` bash execution and `@file` in the body are not interpreted. | `plugins::expand_args` | Add the safe subset after a permission check |
| B32 | low | Search "clustering" is dedupe, scoring and domain diversity, not semantic grouping; the agent steps share one `previous` text, not a message bus. | `search::rank`, `agents::run_with_cancel` | Embedding clusters and a real message log (0.13) |
| B33 | medium | The first cluster layer runs workers inside one cluster in order, keeps coding clusters in the live tree, cannot spawn a subcluster while running, and does not enforce the zombie watch yet. | `clusters.rs`, CLUSTERS.md open points | Parallel workers, worktree isolation, dynamic subclusters, zombie watch |
| B30 | low | `pr_create` needs `gh` and an already pushed branch; it never pushes. | `tools.rs` | Add a push step with its own question |

## Goals for 0.12

- [ ] Worker runs inside `work new` and a judge checks the diff; no edit reaches the live tree without a yes.
- [ ] `search probe --daily` runs from a systemd timer; 20 queries survive a blocked engine.
- [ ] Attack corpus: 20 ScreenCodec injections + 20 normal pages; recall 100 %, false positives < 10 %.
- [ ] 20 consecutive test runs are green; the one flake is found.
- [ ] Linux, Windows and macOS CI run the same 189 tests; macOS is signed or labelled unsigned.
- [ ] `pip-audit` and `cargo audit` run in CI; a known CVE fails the job.
- [ ] A `deploy/` unit and README for the daily probe.
- [ ] Test loop, LSP, tree-sitter index, eval/replay and an ACP server.
- [ ] A real Laertes plan provider with its model list.

## How to reproduce the baseline

```bash
cd cli
cargo test                       # 240 tests
cargo build --release
./target/release/penelope doctor
./target/release/penelope work list
./target/release/penelope search probe --daily
./target/release/penelope auth verify
```
