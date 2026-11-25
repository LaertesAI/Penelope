# Penelope roadmap

Every part of the plan with its state on 2026-11-24, version 0.11.0. Earlier names: Kate until 0.3.0, Strix Forgery in 0.4.0.
The repository is public at https://github.com/LaertesAI/Penelope. Licence: MIT, see [`LICENSE`](../LICENSE).

## Built

| Part | Where | Evidence |
|---|---|---|
| Command `penelope` in Rust | `cli/` | 112 tests |
| Full screen terminal interface | `cli/src/tui/` | 8 tests, 9 real turns |
| Text that appears piece by piece | `cli/src/backend.rs`, `cli/src/tui/mod.rs` | 1 test, 15 words arrived in 7 pieces |
| Privacy filter for the request and for tool output, level `pii` for e-mail addresses and IBANs | `cli/src/privacy.rs`, `cli/src/tools.rs` | 4 tests, a made-up token stayed hidden in 4 real turns |
| Privacy filter for the answers of the models, setting `privacy_answers` | `cli/src/backend.rs`, `cli/src/tools.rs` | 1 test, 1 real turn: a made-up token arrived as `[masked:secret]` on the screen and in the transcript |
| Diff in the question "Change this file?", 12 lines per side | `cli/src/files.rs`, `cli/src/tui/view.rs` | 2 tests, 1 real change, shown in `docs/screens/approve-file.png` |
| Tools of plugin MCP servers on the allow list, `penelope plugin allow` and `deny` | `cli/src/plugins.rs`, `cli/src/backend.rs` | 1 test, 1 real turn: 1 allowed call worked, 1 denied call was refused |
| Lesson check with one pair of runs | README, runs of version 0.9.0 | 2 tool calls became 1, tokens out went from 656 to 401, no run returned the answer |
| Log of every lesson step | `cli/src/tools.rs`, `.penelope/lessons.log` | 3 real lines |
| Sessions with transcripts, tabs in the top row, list on `f2` | `cli/src/sessions.rs`, `cli/src/tui/` | 1 test, 6 real sessions, tab change shown in `docs/screens/tabs.png` |
| Scout: searches run on the weaker model (`delegate`) | `cli/src/scout.rs`, `cli/src/mcp.rs` | 4 real tasks, 12,218 tokens stayed on Haiku in one request |
| Web search with ten sources, cache, canonical ranking and fallback | `cli/src/search.rs` | 9 tests, live 8 results |
| Server search API client, several SearXNG instances, engine health and an optional reranker | `cli/src/search.rs` | 3 tests, client and `penelope serve` live |
| Scraper: HTML to Markdown with article, metadata, tables, code and instruction warnings | `cli/src/scrape.rs` | 6 tests |
| Local server: search, read, embed, rerank, redact, token and limit | `cli/src/server.rs` | 6 tests, live HTTP calls |
| `unzip` and plugin ZIP install with zip-slip and size checks | `cli/src/files.rs`, `cli/src/plugins.rs` | 3 tests, live ZIP |
| Plugin update/remove, standalone MCP servers, `auth verify` | `cli/src/plugins.rs`, `cli/src/mcpclient.rs`, `cli/src/auth.rs` | 3 tests, live update/remove/echo/Claude |
| PDF through local `pdftotext` and a guarded reader API client | `cli/src/scrape.rs` | live PDF, 2 tests, no server |
| Research: queries, pages, cited report on the weaker model | `cli/src/research.rs` | 1 test, live cited report |
| File tools read, write, edit, find, grep | `cli/src/files.rs` | 8 tests, 3 real changes after a yes |
| Provider table with 23 entries, `penelope providers`, `penelope provider use`, the table in the setup | `cli/src/providers.rs`, `cli/src/setup.rs`, [`PROVIDERS.md`](PROVIDERS.md) | 2 tests, every address read in the provider's documentation on 2026-11-24, no real provider call |
| Backend kind `anthropic`: the Messages API with a tool loop, for main model, weaker model and scout | `cli/src/llm.rs`, `cli/src/backend.rs`, `cli/src/weak.rs`, `cli/src/scout.rs` | 2 tests against a stand-in server, no real call |
| Sessions for the kinds `openai` and `anthropic`: message history on disk, `--resume` | `cli/src/sessions.rs`, `cli/src/backend.rs` | 2 tests, 1 resumed session against a stand-in script |
| Key handling: variable name in `config.json`, key read per request and removed from every answer text | `cli/src/providers.rs`, `cli/src/llm.rs` | 1 test with an endpoint that repeats the key 3 times |
| Tool `undo`, `penelope undo`, `/undo`: copy of the old file under `.penelope/undo/N/` | `cli/src/files.rs` | 1 test, 1 real turn |
| Several replacements in one `edit` call with one question and one diff | `cli/src/files.rs`, `cli/src/mcp.rs` | 1 test, 1 real turn with 3 places |
| `grep` with regular expressions behind `regex: true`, crate `regex` | `cli/src/files.rs` | 1 test, 1 real turn with 3 matching lines |
| Background jobs `job_start`, `job_read`, `job_stop`, `penelope jobs` | `cli/src/jobs.rs` | 1 test, 1 real turn with 3 tool calls |
| Diff that leaves out equal lines between two changed places | `cli/src/files.rs` | 2 tests, 1 real dialog with 3 places |
| Plan of the running task in the side panel | `cli/src/tools.rs`, `cli/src/tui/view.rs` | 2 real runs |
| Setup with seven questions | `cli/src/setup.rs` | 1 test |
| Lessons from tool calls, filed under `tools` and given to every later turn | `cli/src/tools.rs`, `cli/src/backend.rs` | 2 tests, 4 real lessons |
| Scout settings: `delegate` with four values, `/scout`, own model | `cli/src/mcp.rs`, `cli/src/scout.rs` | 1 real switch |
| Row of search sources with handover | `cli/src/search.rs` | claude and DuckDuckGo tried |
| Tools `tree`, `outline`, `ask_user` | `cli/src/files.rs`, `cli/src/tools.rs` | 1 test, 2 real questions |
| Unravel: take back the last notes | `cli/src/memory.rs` | 1 test |
| Crew in the side panel (lead, aiolos, utis) and `penelope crew` | `cli/src/tui/view.rs`, `cli/src/tools.rs` | shown in every screen picture |
| Woven mark with six moods in the interface | `cli/src/tui/art.rs` | built from `brand/term-weave.svg`, the loose thread takes the colour of the mood |
| Routed tool output through a weaker model | `cli/src/tools.rs`, `cli/src/weak.rs` | 6,947 characters became 168 in one run |
| Approval question for unsafe commands | `cli/src/tools.rs`, `cli/src/tui/mod.rs` | 3 real commands answered with yes |
| Memory map with waypoints under `.penelope/memory/` | `cli/src/memory.rs` | notes filed and found in 3 runs |
| Auto-learning after each answer | `cli/src/tools.rs` | 2 runs filed a rule at `git/commits` |
| Plugin import from Claude Code folders | `cli/src/plugins.rs` | 1 plugin: 9 skills, 7 servers, 2 demo plugins with 1 server each |
| MCP server with the tools of the harness | `cli/src/mcp.rs` | used by the main model in every turn |
| ScreenCodec as the only picture source | `cli/src/backend.rs` | wired, no run inside the harness yet |
| Claude subscription through the official `claude` command | `cli/src/backend.rs` | all real runs |
| Backend kind `openai`: chat completions with a tool loop, API keys, local models | `cli/src/llm.rs` | 2 tests against a stand-in server, no real call |
| Login for Claude and API providers, Secret Service, file fallback 0600 | `cli/src/auth.rs` | 1 test, live dummy key store/remove |
| 35 providers, 33 HTTP endpoints | `cli/src/providers.rs` | table test, live OpenCode |
| Six modes, safe refuses changes | `cli/src/mode.rs`, `cli/src/files.rs` | 2 tests |
| Tool role and `computer` | `cli/src/route.rs`, `cli/src/tools.rs`, `cli/src/agents.rs` | 1 test, 1 live run |
| Skills Penelope writes (`skill_new`, from a 5-call turn) | `cli/src/skills.rs` | 1 test, 1 live skill |
| Leak audit and redirect-free HTTP | `cli/src/audit.rs`, `cli/src/net.rs` | 2 tests, live audit 0 findings |
| Visible Wayland cursor and uinput click helper | `cli/src/platform.rs`, `cli/src/input.rs`, `cli/tools/uinput-click.c` | 1 test, live counter 0→1→2 |
| Updater client and blueprint | `cli/src/update.rs`, `docs/AUTOUPDATE.md` | 1 test, no server |
| Windows branches (cmd, taskkill, clipboard, pointer, PATHEXT) | `cli/src/platform.rs`, `cli/src/jobs.rs` | source only, untested |
| Brand: hand-drawn weave, lockup, small marks | `brand/` | see `brand/BRAND.md` |
| One contract table for 42 tools, schema check and role gate | `cli/src/spec.rs`, `cli/src/route.rs` | 4 tests, every name reaches a handler, 3 live calls |
| Tools Penelope writes for a project | `cli/src/forge.rs` | 2 tests, no shell break-out |
| Eight roles, a model per role, `provider use --role` | `cli/src/route.rs`, `cli/src/providers.rs` | 1 test, 1 live agent step |
| Privacy model between the models with a strict JSON contract and a fallback | `cli/src/privacy.rs` | 1 test |
| Router model for the plan, with fallback to the fixed plan | `cli/src/agents.rs` | 1 test |
| Agents written with `$name(...)`, limits, budget, cancel | `cli/src/agents.rs` | 3 tests, 1 live one-step run |
| Crew panel with subagent names, model and state; Laertes families in gold | `cli/src/crew.rs`, `cli/src/tui/view.rs` | screen picture, 1 live run |
| Shared memory, consolidation, complaint feedback, used-path count | `cli/src/memory.rs`, `cli/src/tools.rs` | 2 tests |
| Brain graph and Obsidian vault | `cli/src/brain.rs` | 1 test, screen picture |
| Cost in euro per role and model | `cli/src/cost.rs` | 1 test, live rows |
| Real ScreenCodec actions, desktop capture, own cursor | `cli/src/screen.rs`, `cli/src/cursor.rs` | 2 tests, 122 lines read, live click 0→1, pointer moved |
| Plugin MCP tools for HTTP models | `cli/src/mcpclient.rs` | 1 test |
| Plugin install from git and take-over on first start | `cli/src/plugins.rs` | 2 tests |
| Tools `session_search`, `http_request`, `json_query`, `git_diff`, `clipboard`, `agent` | `cli/src/tools.rs` | 1 test |
| Startup animation | `cli/src/startup.rs` | 1 test |

## Next

[`PLAN.md`](PLAN.md) orders the open parts into versions 0.11 to 0.15. It holds 7 further rows of tools, 7 roles for
subagents and 4 ways they work together. Version 0.10 of that plan is built.

| Part | First step |
|---|---|
| A long-lived MCP client for stateful plugin servers | one process per server with a request id and a timeout, as in `screen.rs` |
| Real desktop click on Wayland | ydotool with a running ydotoold, then `desktop_click` on a test window |
| A progress view for `plugin install` | a background job with the git output in the transcript |
| Allow rule for a single tool of a plugin | a list of tool names next to `"allow"` in `plugins.json` |
| One real call per HTTP kind | set one key, run `penelope ask` once on the cheapest model of that provider, write down tokens and answer |
| Text piece by piece for the kinds `openai` and `anthropic` | read the event stream of both formats, today whole blocks arrive |
| MCP client for the kinds `openai` and `anthropic` | start the ScreenCodec server from the harness and pass its tools on |
| Privacy filter for the pieces of a growing answer | filter the text of the open block on every piece, today the finished block is filtered |
| Scout with a local model | one run of `scout` against a local model server, the code path has 1 test against a stand-in server |
| Several sessions running at once | one process per tab, today one turn runs at a time |
| A tool that starts and watches training runs | first on Qwen2.5-0.5B on this PC, with a yes before every run on rented hardware |
| Lessons from the scout's own tool calls | the scout reports its failed calls, today only the calls of the main model are read |
| More pairs of runs for the lesson check | 5 pairs with a request that a safe command can answer, the first pair had no such command |
| Lessons that name the command that works | tell the weaker model the safe list, the 2 lessons of the first pair only said stop |
| Utis as a model next to the rules | the first trained Utis 1 Mini behind the same `guard_count` call |
| Test of ScreenCodec inside the harness | one request that opens a local page |
| Comparison run without condensing | same request with `run_policy` unchanged and the weaker model switched off |

## Later

| Part | Open question |
|---|---|
| The eight model families (Nostos, Tiresias, Utis, Aiolos, Argos, Klea, Telemach, Proteus) | none is trained, each has a brief outside this repository |
| Scheduled tasks with a list in the interface | timer inside the harness or systemd user timers |
| Bots (chat services) | which service first |
| Hugging Face model browser and download | where models are stored, which runtime loads them |
| A bridge to a private research prototype | the prototype needs a stable command first |
| Graphical window | which toolkit |
| Hooks and agents of plugins | which hook events the harness has |
| Two install options (ScreenCodec MCP server only, or the whole harness) | package format per platform |

## Limits known today

- No request has reached a real provider through the kinds `openai` and `anthropic`. No key was set on the test PC.
- Tool calls are documented for 10 of the 22 HTTP entries of the provider table. For the other 12 no page on tool calls was read.
- Text appears piece by piece with the claude command only. The kinds `openai` and `anthropic` send whole blocks.
- The history file of an HTTP session holds the blocks of the model without the privacy filter. The provider needs them back unchanged.
- A refusal of the Messages API ends the turn. The field `fallbacks` asks for a second model, is off by default and is untested.
- The privacy filter works with fixed rules. A secret without a known prefix and without a key name passes.
- The filter for answers reads finished blocks of text. The pieces of a block are drawn unfiltered while the model writes.
- The `claude` command keeps its own record of every session under `~/.claude`. Penelope does not filter that record.
- An answer that quotes a code line such as `token = "value"` shows `[masked:secret]` in place of the value. `"privacy_answers": false` switches that off.
- The filter for answers has 1 test and 1 real turn with 1 masked item.
- The diff compares whole lines and leaves out equal lines. Above 1,000,000 pairs of lines the lines between the first and the last change count as changed.
- The dialog shows 12 lines per side of the diff. A dialog that is too short ends with a line that counts the rest.
- `"allow"` covers every tool of every MCP server of one plugin. Tools of a denied plugin stay visible to the main model, and the `claude` command refuses the call.
- The allow rule `mcp__SERVER__*` ran in 1 real turn with 2 servers named `atlas-notes` and `ledger-notes`. Server names with dots or spaces have 1 test and no real turn.
- The kinds `openai` and `anthropic` connect no MCP server of a plugin, so `"allow"` has no effect there.
- The level `pii` knows e-mail addresses and IBANs only. Names, addresses and phone numbers pass.
- A session keeps the wire format it started in: `claude-cli`, `openai` or `anthropic`. Notes written before 0.5.0 are not in the journal of unravel.
- A session stored before 0.6.0 has no transcript. Its tab opens with one line that names it.
- DuckDuckGo's HTML page refused automated searches after 2 queries. The search tool of the claude command took 15 s per query.
- `grep` with `regex: true` reads one line at a time, so a pattern cannot span 2 lines.
- `undo` keeps the last 50 changes of `write` and `edit`. A change that `run` or a job made cannot be undone.
- A background job has no limit for time or for the size of its output file. At most 8 jobs run at once.
- Background jobs read `/proc` and send signals with the `kill` command, so they work on Linux only.
- Each of the 4 tools of 0.10.0 has 1 real turn. The answer text of `undo` was shortened after its turn.
- A lesson is written by the weaker model and is not checked. A wrong lesson stays until `penelope memory unravel` takes it back.
- One pair of runs measured lessons: 2 tool calls became 1 and 656 output tokens became 401. No run of the pair returned the answer.
- Input tokens of the same request were 7,658, 4,536 and 4,812 in 3 runs without a lesson. The 7,528 tokens of the run with lessons lie inside that spread.
- 3 of 5 lesson steps of version 0.9.0 filed nothing. Two of them ran before `.penelope/lessons.log` existed, so their cause is unknown.
- A scout cannot run commands outside the safe list and cannot change files.
- A question about an unsafe command exists in the full screen interface only. Every other caller gets a refusal.
- The interface was run in tmux with 124 columns and 40 rows. Other terminals are untested.
- The mood asleep starts after 3 minutes without a key. No test waits that long.
