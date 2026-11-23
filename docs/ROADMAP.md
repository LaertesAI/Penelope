# Kate roadmap

Every part of the plan for Kate with its state on 2026-11-23, version 0.3.0.

## Built

| Part | Where | Evidence |
|---|---|---|
| Command `katecli` in Rust | `cli/` | release binary of 3.4 MB |
| Full screen terminal interface | `cli/src/tui/` | 12 tests, 2 real turns |
| Cat mascot with six moods in the interface | `cli/src/tui/art.rs` | drawn from `brand/mark.svg`, eyes set per mood |
| Routed tool output through a weaker model | `cli/src/tools.rs`, `cli/src/weak.rs` | 6,947 characters became 168 in one run |
| Approval question for unsafe commands | `cli/src/tools.rs`, `cli/src/tui/mod.rs` | 1 real command answered with yes |
| Memory map with waypoints under `.kate/memory/` | `cli/src/memory.rs` | notes filed and found in 3 runs |
| Auto-learning after each answer | `cli/src/tools.rs` | 2 runs filed a rule at `git/commits` |
| Plugin import from Claude Code folders | `cli/src/plugins.rs` | 1 plugin: 9 skills, 7 servers |
| MCP server with Kate's tools | `cli/src/mcp.rs` | used by the main model in every turn |
| ScreenCodec as the only picture source | `cli/src/backend.rs` | wired, no run inside Kate yet |
| Claude subscription through the official `claude` command | `cli/src/backend.rs` | all real runs |
| OpenAI-compatible endpoints, API keys, local models | `cli/src/backend.rs`, `cli/src/weak.rs` | written, no run yet |
| Brand: logo, marks, mascot, banner | `brand/` | see `brand/BRAND.md` |

## Next

| Part | First step |
|---|---|
| Text that appears word by word | read `stream_event` lines of the claude command with `--include-partial-messages` |
| List of earlier sessions | store the session id and the first request per session under `.kate/sessions/` |
| Tools of plugin MCP servers on the allow list | one setting per plugin in `plugins.json` |
| MCP client for the OpenAI-compatible backend | start the ScreenCodec server from Kate and pass its tools on |
| Test of ScreenCodec inside Kate | one request that opens a local page |
| Comparison run without condensing | same request with `run_policy` unchanged and the weaker model switched off |

## Later

| Part | Open question |
|---|---|
| Scheduled tasks with a list in the interface | timer inside Kate or systemd user timers |
| Bots (chat services) | which service first |
| Hugging Face model browser and download | where models are stored, which runtime loads them |
| COAA cassettes trained from Kate | the COAA prototype needs a stable command first |
| Graphical window with the cat | which toolkit |
| Hooks and agents of plugins | which hook events Kate has |
| Two install options (ScreenCodec MCP server only, or the whole harness) | package format per platform |

## Limits known today

- The answer of the main model appears in whole blocks, one per message of the claude command.
- A question about an unsafe command exists in the full screen interface only. Every other caller gets a refusal.
- The interface was run in tmux with 124 columns and 40 rows. Other terminals are untested.
- The mood asleep starts after 3 minutes without a key. No test waits that long.
