# Penelope roadmap

Every part of the plan with its state on 2026-11-24, version 0.8.0. Earlier names: Kate until 0.3.0, Strix Forgery in 0.4.0.

## Built

| Part | Where | Evidence |
|---|---|---|
| Command `penelope` in Rust | `cli/` | 26 tests |
| Full screen terminal interface | `cli/src/tui/` | 7 tests, 5 real turns |
| Text that appears piece by piece | `cli/src/backend.rs`, `cli/src/tui/mod.rs` | 1 test, 15 words arrived in 7 pieces |
| Privacy filter for the request and for tool output, level `pii` for e-mail addresses and IBANs | `cli/src/privacy.rs`, `cli/src/tools.rs` | 4 tests, a made-up token stayed hidden in 4 real turns |
| Sessions with transcripts, tabs in the top row, list on `f2` | `cli/src/sessions.rs`, `cli/src/tui/` | 1 test, 2 real sessions, tab change shown in `docs/screens/tabs.png` |
| Scout: searches run on the weaker model (`delegate`) | `cli/src/scout.rs`, `cli/src/mcp.rs` | 3 real tasks, 12,218 tokens stayed on Haiku in one request |
| Web search with four sources | `cli/src/search.rs` | 2 tests, claude and DuckDuckGo tried |
| File tools read, write, edit, find, grep | `cli/src/files.rs` | 3 tests, 1 real change after a yes |
| Plan of the running task in the side panel | `cli/src/tools.rs`, `cli/src/tui/view.rs` | 1 real run |
| Setup with seven questions | `cli/src/setup.rs` | 1 test |
| Lessons from tool calls, filed under `tools` and given to every later turn | `cli/src/tools.rs`, `cli/src/backend.rs` | 2 tests, 1 real lesson |
| Scout settings: `delegate` with four values, `/scout`, own model | `cli/src/mcp.rs`, `cli/src/scout.rs` | 1 real switch |
| Row of search sources with handover | `cli/src/search.rs` | claude and DuckDuckGo tried |
| Tools `tree`, `outline`, `ask_user` | `cli/src/files.rs`, `cli/src/tools.rs` | 1 test, 1 real question |
| Unravel: take back the last notes | `cli/src/memory.rs` | 1 test |
| Crew in the side panel (lead, aiolos, utis) and `penelope crew` | `cli/src/tui/view.rs`, `cli/src/tools.rs` | shown in every screen picture |
| Woven mark with six moods in the interface | `cli/src/tui/art.rs` | built from `brand/term-weave.svg`, the loose thread takes the colour of the mood |
| Routed tool output through a weaker model | `cli/src/tools.rs`, `cli/src/weak.rs` | 6,947 characters became 168 in one run |
| Approval question for unsafe commands | `cli/src/tools.rs`, `cli/src/tui/mod.rs` | 2 real commands answered with yes |
| Memory map with waypoints under `.penelope/memory/` | `cli/src/memory.rs` | notes filed and found in 3 runs |
| Auto-learning after each answer | `cli/src/tools.rs` | 2 runs filed a rule at `git/commits` |
| Plugin import from Claude Code folders | `cli/src/plugins.rs` | 1 plugin: 9 skills, 7 servers |
| MCP server with the tools of the harness | `cli/src/mcp.rs` | used by the main model in every turn |
| ScreenCodec as the only picture source | `cli/src/backend.rs` | wired, no run inside the harness yet |
| Claude subscription through the official `claude` command | `cli/src/backend.rs` | all real runs |
| OpenAI-compatible endpoints, API keys, local models | `cli/src/backend.rs`, `cli/src/weak.rs` | written, no run yet |
| Brand: hand-drawn weave, lockup, small marks | `brand/` | see `brand/BRAND.md` |

## Next

| Part | First step |
|---|---|
| Tools of plugin MCP servers on the allow list | one setting per plugin in `plugins.json` |
| MCP client for the OpenAI-compatible backend | start the ScreenCodec server from the harness and pass its tools on |
| Privacy filter for the answers of the models | the same `mask` call on every text block before it is drawn |
| Sessions for the OpenAI-compatible backend | store the message history per session on disk |
| Scout with a local model | one run of `scout` against an OpenAI-compatible endpoint, the code path exists and is untested |
| Several sessions running at once | one process per tab, today one turn runs at a time |
| A tool that starts and watches training runs | first on Qwen2.5-0.5B on this PC, with a yes before every run on rented hardware |
| Lessons from the scout's own tool calls | the scout reports its failed calls, today only the calls of the main model are read |
| A check that a lesson helps | count the tool calls of the same request before and after the lesson |
| Diff in the question "Change this file?" | show the changed lines, today the dialog names file, line and line counts |
| Utis as a model next to the rules | the first trained Utis 1 Mini behind the same `guard_count` call |
| Test of ScreenCodec inside the harness | one request that opens a local page |
| Comparison run without condensing | same request with `run_policy` unchanged and the weaker model switched off |

## Later

| Part | Open question |
|---|---|
| The eight model families (Nostos, Tiresias, Utis, Aiolos, Argos, Klea, Telemach, Proteus) | none is trained, each has a brief under `../models/NAME/research/` |
| Scheduled tasks with a list in the interface | timer inside the harness or systemd user timers |
| Bots (chat services) | which service first |
| Hugging Face model browser and download | where models are stored, which runtime loads them |
| COAA cassettes trained from the harness | the COAA prototype needs a stable command first |
| Graphical window | which toolkit |
| Hooks and agents of plugins | which hook events the harness has |
| Two install options (ScreenCodec MCP server only, or the whole harness) | package format per platform |

## Limits known today

- Text appears piece by piece with the claude command only. The OpenAI-compatible backend still sends whole blocks.
- The privacy filter works with fixed rules. A secret without a known prefix and without a key name passes.
- The filter reads the request and tool output. The answers of the models pass unread.
- The level `pii` knows e-mail addresses and IBANs only. Names, addresses and phone numbers pass.
- Sessions are stored for the claude command only. Notes written before 0.5.0 are not in the journal of unravel.
- A session stored before 0.6.0 has no transcript. Its tab opens with one line that names it.
- DuckDuckGo's HTML page refused automated searches after 2 queries. The search tool of the claude command took 15 s per query.
- `grep` finds plain text. It has no regular expressions.
- A lesson is written by the weaker model and is not checked. A wrong lesson stays until `penelope memory unravel` takes it back.
- Whether lessons shorten later turns is not measured.
- A scout cannot run commands outside the safe list and cannot change files.
- A question about an unsafe command exists in the full screen interface only. Every other caller gets a refusal.
- The interface was run in tmux with 124 columns and 40 rows. Other terminals are untested.
- The mood asleep starts after 3 minutes without a key. No test waits that long.
