# Strix Forgery roadmap

Every part of the plan with its state on 2026-11-24, version 0.4.0. The project was called Kate until version 0.3.0.

## Built

| Part | Where | Evidence |
|---|---|---|
| Command `strix` in Rust | `cli/` | 16 tests |
| Full screen terminal interface | `cli/src/tui/` | 7 tests, 5 real turns |
| Text that appears piece by piece | `cli/src/backend.rs`, `cli/src/tui/mod.rs` | 1 test, 15 words arrived in 7 pieces |
| Privacy filter for tool output | `cli/src/privacy.rs`, `cli/src/tools.rs` | 3 tests, a made-up token stayed hidden in 2 real turns |
| Formation in the side panel: lead, tools, privacy | `cli/src/tui/view.rs` | shown in every screen picture |
| Owl with six moods in the interface | `cli/src/tui/art.rs` | drawn from `brand/mark.svg`, pupils set per mood |
| Routed tool output through a weaker model | `cli/src/tools.rs`, `cli/src/weak.rs` | 6,947 characters became 168 in one run |
| Approval question for unsafe commands | `cli/src/tools.rs`, `cli/src/tui/mod.rs` | 2 real commands answered with yes |
| Memory map with waypoints under `.strix/memory/` | `cli/src/memory.rs` | notes filed and found in 3 runs |
| Auto-learning after each answer | `cli/src/tools.rs` | 2 runs filed a rule at `git/commits` |
| Plugin import from Claude Code folders | `cli/src/plugins.rs` | 1 plugin: 9 skills, 7 servers |
| MCP server with the tools of the harness | `cli/src/mcp.rs` | used by the main model in every turn |
| ScreenCodec as the only picture source | `cli/src/backend.rs` | wired, no run inside the harness yet |
| Claude subscription through the official `claude` command | `cli/src/backend.rs` | all real runs |
| OpenAI-compatible endpoints, API keys, local models | `cli/src/backend.rs`, `cli/src/weak.rs` | written, no run yet |
| Brand: owl logo, marks, mascot, banner | `brand/` | see `brand/BRAND.md` |

## Next

| Part | First step |
|---|---|
| List of earlier sessions | store the session id and the first request per session under `.strix/sessions/` |
| Tools of plugin MCP servers on the allow list | one setting per plugin in `plugins.json` |
| MCP client for the OpenAI-compatible backend | start the ScreenCodec server from the harness and pass its tools on |
| Privacy filter for the request text and for file reads | the same `mask` call before the main model starts |
| Test of ScreenCodec inside the harness | one request that opens a local page |
| Comparison run without condensing | same request with `run_policy` unchanged and the weaker model switched off |

## Later

| Part | Open question |
|---|---|
| Wren (tool calling), Strix (privacy), Corvus (coding) | none of the three models exists, training data and base model are open |
| Scheduled tasks with a list in the interface | timer inside the harness or systemd user timers |
| Bots (chat services) | which service first |
| Hugging Face model browser and download | where models are stored, which runtime loads them |
| COAA cassettes trained from the harness | the COAA prototype needs a stable command first |
| Graphical window with the owl | which toolkit |
| Hooks and agents of plugins | which hook events the harness has |
| Two install options (ScreenCodec MCP server only, or the whole harness) | package format per platform |

## Limits known today

- Text appears piece by piece with the claude command only. The OpenAI-compatible backend still sends whole blocks.
- The privacy filter works with fixed rules. A secret without a known prefix and without a key name passes.
- The filter reads tool output only. The request of the user and the answers of the models pass unread.
- A question about an unsafe command exists in the full screen interface only. Every other caller gets a refusal.
- The interface was run in tmux with 124 columns and 40 rows. Other terminals are untested.
- The mood asleep starts after 3 minutes without a key. No test waits that long.
