<p align="center"><img src="brand/lockup.png" alt="Penelope, the harness of Laertes" width="70%"></p>

<p align="center">
  <a href="cli/Cargo.toml"><img src="https://img.shields.io/badge/version-0.10.0-C4623E?style=flat-square&labelColor=1C1A17" alt="version 0.10.0"></a>
  <a href="cli/Cargo.toml"><img src="https://img.shields.io/badge/Rust_edition-2021-C4623E?style=flat-square&labelColor=1C1A17" alt="Rust edition 2021"></a>
  <a href="#build"><img src="https://img.shields.io/badge/tests-39_passing-C4623E?style=flat-square&labelColor=1C1A17" alt="39 tests passing"></a>
  <a href="#status"><img src="https://img.shields.io/badge/platform-Linux-C4623E?style=flat-square&labelColor=1C1A17" alt="platform Linux"></a>
  <a href="#parts"><img src="https://img.shields.io/badge/MCP_tools-22-C4623E?style=flat-square&labelColor=1C1A17" alt="22 MCP tools"></a>
  <a href="docs/PROVIDERS.md"><img src="https://img.shields.io/badge/providers-23-C4623E?style=flat-square&labelColor=1C1A17" alt="23 providers"></a>
  <a href="#status"><img src="https://img.shields.io/badge/status-alpha-C4623E?style=flat-square&labelColor=1C1A17" alt="status alpha"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/licence-MIT-C4623E?style=flat-square&labelColor=1C1A17" alt="licence MIT"></a>
</p>

# Penelope

Penelope is a coding agent for the terminal, written in Rust. It sends searches and long tool output to a cheaper
model, keeps notes in a memory map, and asks before every change to a file.

<p align="center"><img src="docs/screens/chat.png" alt="penelope: transcript on the left, the woven mark and the session numbers on the right" width="100%"></p>

## Status

| | |
|---|---|
| Version | 0.10.0, set in [`cli/Cargo.toml`](cli/Cargo.toml) |
| Date | 2026-11-24 |
| Stage | alpha: one developer, one PC, no packaged release |
| Repository | https://github.com/LaertesAI/Penelope, branch `main` |
| Source | [`cli/`](cli), 22 Rust files, 6,580 lines |
| Binary | `cli/target/release/penelope`, 5.6 MB (5,628,752 bytes), built from source |
| Build | `cargo build --release`, profile `release` with `opt-level = 2` and `strip = true` |
| Toolchain | rustc 1.99.1, edition 2021 |
| Tests | 39 passed, 0 failed, no model call in any test |
| Real runs | 29 with the `claude` command: 25 up to version 0.9.0 and 4 for the tools new in 0.10.0, listed in [`docs/RUNS.md`](docs/RUNS.md). No run used a provider key. |
| Platform | Linux. Background jobs read `/proc`. Windows and macOS are untried. |
| Settings folder | `.penelope/` in the project, else `~/.penelope/` |
| Licence | MIT, see [`LICENSE`](LICENSE). `cli/Cargo.toml` carries the same field. |

The badges above are static. Version and edition come from `cli/Cargo.toml`, the test count from `cargo test`.
The tool count comes from `tool_specs` in `cli/src/mcp.rs`, the provider count from `cli/src/providers.rs`.

## What sets Penelope apart

- Searches run on a cheaper model. In one request 12,218 tokens of searching stayed on Claude Haiku, the main model
  used 8,669.
- Long command output is shortened before the main model reads it. In one run 6,947 characters became 168.
- Notes live at waypoints of a tree. After a failed tool call the weaker model files up to 3 lessons, and every later
  turn reads them.
- A filter masks keys, passwords and tokens before any model reads them, in requests, files, tool output and answers.
- A change to a file shows its diff and waits for a yes. `undo` takes the last 50 changes back.
- The model is your choice: the `claude` command with your own login, or one of 22 HTTP providers and local servers.

## Build

Needed: a Rust toolchain, Linux, and one model source (the `claude` command, an API key, or a local model server).

```bash
cd cli
cargo build --release
cargo test                          # 39 tests, no model call
./target/release/penelope doctor    # one line per program and setting Penelope depends on
```

## First steps

```bash
penelope setup --here    # makes .penelope/ in this folder and asks seven questions
penelope                 # opens the full screen interface
penelope ask "Run the tests and tell me what fails"
```

## Models and providers

A provider table with 23 entries names the address, the wire format and the key variable of each provider.
`penelope providers` prints the table and says per entry whether its key is set.

| Kind | Entries |
|---|---|
| `claude-cli` | the official `claude` command with its own login, no key |
| `anthropic` | the Messages API of Anthropic |
| `openai` (chat completions) | OpenAI, OpenRouter, Alibaba Model Studio (2 regions), DeepSeek, Mistral, Groq, Together AI, Fireworks AI, Google Gemini, xAI, Cerebras, Moonshot (2 regions), Zhipu (2 regions), Hugging Face router |
| `openai`, on this PC | Ollama, LM Studio, llama.cpp server, vLLM |

```bash
penelope provider use groq                          # main model, example model of the table
penelope provider use ollama --role weak --model qwen3:8b
penelope provider use alibaba-intl --workspace ID   # the address holds the workspace of the account
penelope provider use openai --base-url http://localhost:9000/v1 --model NAME
```

- `config.json` stores the name of the environment variable. A key is read at the start of a request and stored nowhere.
- The setup refuses a pasted key in place of a variable name.
- A Claude subscription works through the `claude` command only. Penelope has no login of its own.
- Tested with real models: the `claude` command. The two HTTP formats have 4 tests against a local test server.
- No request has reached a real provider, because no key was set on the test PC.
- [`docs/PROVIDERS.md`](docs/PROVIDERS.md) names the source of every address, read on 2026-11-24, and every field of a model setting.

## Parts

"Since" names the version that added the part. "Real use" counts runs with a real model.

| Part | What it does | Since | Real use |
|---|---|---|---|
| Routed tool output | `run` executes a shell command. Output over 40 lines goes to the weaker model and comes back as at most 12 lines. The raw output stays on disk under a run id. | 0.1 | yes |
| Web pages | `fetch` loads a page and the weaker model answers one question from it. The page text never enters the main model. | 0.1 | 1 page |
| Memory map | Notes live at waypoints under `.penelope/memory/`. A lookup follows signposts one level at a time and never searches all notes. A finished walk is stored, so the same request jumps to its waypoint. | 0.1 | 3 runs |
| MCP server | `penelope mcp` offers all 22 tools to any MCP client. | 0.1 | every turn |
| Eyes | With `screencodec-mcp` installed, pages and pictures arrive as text. | 0.1 | wired, no run in the harness |
| Auto-learning | After each answer the weaker model files lasting rules of the user at a waypoint. | 0.2 | 2 runs |
| Plugins | `penelope plugin add DIR` registers a plugin folder of Claude Code with its skills and MCP servers. | 0.2 | 1 plugin, 9 skills, 7 servers |
| Statistic | `penelope stats` counts the characters kept away from the main model and the masked items. | 0.2 | yes |
| Full screen interface | Transcript, input box, side panel with the painted picture, command list on `/`, palette on `ctrl+p`. | 0.3 | 12 turns |
| Approval | A command outside the safe list asks: run once, allow all in this session, or refuse. | 0.3 | 3 commands |
| Privacy filter | Known key formats, values behind names such as `password` or `token`, and private key blocks become `[masked:...]`. The level `pii` adds e-mail addresses and IBANs. | 0.4 | 5 turns |
| Text piece by piece | The answer appears while the model writes. | 0.4 | 15 words in 7 pieces |
| Sessions | Every conversation is stored with its transcript. The top row shows sessions as tabs. | 0.5 | 9 sessions |
| Unravel | `penelope memory unravel [N]` takes back the last N notes. | 0.5 | test only |
| Scout | A search task runs on the weaker model with 10 read-only tools. The main model gets a report of at most 15 lines. | 0.6 | 4 tasks |
| Web search | `web_search` tries the search tool of the `claude` command, SearXNG, Brave and DuckDuckGo in this order. | 0.6 | claude and DuckDuckGo |
| File tools | `tree`, `outline`, `read`, `find`, `grep`, `write`, `edit`. | 0.6 | 3 changes |
| Plan | The main model keeps the steps of a task. The side panel shows them. | 0.6 | 2 runs |
| Setup | `penelope setup` asks seven questions with numbered answers. | 0.6 | test only |
| Lessons from tool calls | After a failed or refused call the weaker model files up to 3 lessons under the waypoint `tools`. | 0.7 | 4 lessons |
| Question to the user | `ask_user` opens a dialog with 2 to 6 answers. | 0.7 | 2 questions |
| Diff before a change | The question "Change this file?" shows the changed lines, at most 12 per side. Since 0.10 it leaves out equal lines between two changed places. | 0.9 | 3 changes |
| Filter for answers | Every finished text block of a model passes the privacy filter before screen and transcript. | 0.9 | 1 turn |
| Allow and deny for plugins | `penelope plugin allow NAME` puts the tools of a plugin's MCP servers on the allow list. A new plugin starts denied. | 0.9 | 1 turn |
| Undo | `undo` restores the file as it was before the last `write` or `edit`. The last 50 changes are kept. `penelope undo` and `/undo` do the same. | 0.10 | 1 turn |
| Edit with several places | One `edit` call replaces several pieces of one file. All happen or none, with one question and one diff. | 0.10 | 1 turn with 3 places |
| Grep with patterns | `grep` takes a regular expression with `regex: true`. Plain text stays the default. | 0.10 | 1 turn, 3 matching lines |
| Background jobs | `job_start`, `job_read` and `job_stop` run servers, watchers and long builds next to the conversation. At most 8 run at once. A command outside the safe list asks first. | 0.10 | 1 turn with 3 tool calls |
| Provider table | 23 entries, `penelope providers` and `penelope provider use`. | 0.10 | test only |
| Messages API | The kind `anthropic` talks to the Messages API with tool use. | 0.10 | test only |
| Sessions for HTTP endpoints | The messages of a conversation lie in `.penelope/sessions/ID.history.json`, so `penelope chat --resume N` goes on with it. | 0.10 | 1 resumed session against a stand-in script, no real provider |

The 22 tools: `run`, `run_detail`, `job_start`, `job_read`, `job_stop`, `fetch`, `web_search`, `read`, `write`, `edit`,
`undo`, `find`, `grep`, `tree`, `outline`, `ask_user`, `scout`, `plan`, `memory_go`, `memory_note`, `memory_map`, `skill`.

## The full screen interface

<p align="center"><img src="docs/screens/home.png" alt="Start screen of penelope in kitty with the name, the painted picture of Penelope at the loom and the input box" width="80%"></p>

The painted picture is a real picture, sent through the graphics protocol of kitty. It shows in kitty, Ghostty, WezTerm and Konsole.
Other terminals and tmux show the name alone. `PENELOPE_PICTURE=off` switches it off, `PENELOPE_PICTURE=on` forces it.
The start screen above is a picture of kitty on a virtual display. The other three screens are from version 0.9.0 and show the earlier woven mark.

`penelope` without arguments opens it. `penelope chat --plain` keeps the line by line chat. A pipe on standard input
or output selects the plain chat by itself.

| Key | Effect |
|---|---|
| `enter` | send the request |
| `alt+enter`, `ctrl+j` | new line in the request |
| `up`, `down` | earlier and later requests, stored in `.penelope/history.jsonl` |
| `page up`, `page down`, mouse wheel | scroll the transcript |
| `esc` | stop the running turn, or empty the input |
| `/` | list of commands, `tab` completes, `enter` runs |
| `ctrl+p` | command palette with search |
| `ctrl+o` | choose the main and the weaker model |
| `ctrl+n` | new session |
| `f2` | list of the stored sessions, `enter` opens one |
| `f3`, `f4` or `alt+left`, `alt+right` | the tab to the left or right |
| `alt+1` to `alt+9` | the tab with that number |
| `ctrl+b` | show or hide the side panel |
| `f1` | keys and commands |
| `f12` | save the screen as an SVG picture under `.penelope/screens/` |
| `ctrl+c` | empty the input, leave when it is empty |

Commands: `/new`, `/models`, `/sessions`, `/setup`, `/providers`, `/undo`, `/jobs`, `/scout [auto|web|offer|off]`,
`/crew`, `/map`, `/go TEXT`, `/note TEXT`, `/unravel`, `/stats`, `/plugins`, `/doctor`, `/copy`, `/shot`, `/sidebar`,
`/help`, `/quit`.

The side panel shows the woven square. Its loose thread at the lower right takes the colour of the mood:

| Mood | Face as text | Thread | When |
|---|---|---|---|
| awake | `[#|#]` | clay | ready for a request |
| thinking | `[#~#]` | blue | the main model works on its answer |
| working | `[#>#]` | blue | a tool runs |
| asking | `[#?#]` | red | a command or a file change waits for an answer |
| learned | `[#+#]` | white | a rule was stored in the memory map, for 4 seconds |
| asleep | `[#.#]` | grey | no key for 3 minutes |

Below the mark: tokens and turns of the session, the crew, the way through the memory map, the share of tool output
kept away from the main model, ScreenCodec, plugins and the run policy. A terminal under 100 columns hides the panel
and shows the face as text in the input box. A terminal without 24 bit colour gets the nearest of 216 colours.

<p align="center"><img src="docs/screens/approve-file.png" alt="Question before a change to a file: one removed line in red, two added lines in clay" width="80%"></p>

## Commands

```bash
penelope                          # full screen interface
penelope chat --plain             # line by line chat
penelope chat --resume 1          # go on with the newest session
penelope ask "TEXT"               # one question, one answer
penelope init                     # create .penelope/ in this folder
penelope setup [--here]           # seven questions: models, who searches, privacy, policies, web search
penelope providers                # the provider table and the state of each key
penelope provider use NAME [--role main|weak|scout] [--model MODEL] [--workspace ID] [--base-url URL]
penelope sessions                 # stored conversations, newest first
penelope crew                     # the eight model families and their stand-ins
penelope memory map               # the whole memory map
penelope memory go "how do we name api endpoints in java"
penelope memory note --path java/microservices/api "REST endpoints use plural nouns"
penelope memory unravel 2         # take back the last two notes
penelope search "ratatui terminal library"    # up to 8 results
penelope scout "Where does this project parse its settings?"
penelope run --ask "which test fails?" cargo test
penelope undo                     # take back the last change of write or edit
penelope jobs                     # background jobs of this folder
penelope jobs read N              # new output of job N
penelope jobs stop N              # end job N and every process it started
penelope plugin add ~/path/to/plugin
penelope plugin list
penelope plugin allow NAME
penelope plugin deny NAME
penelope stats
penelope mcp [--root DIR]         # serve the 22 tools to another MCP client
penelope doctor
```

## Settings

`.penelope/config.json` with its defaults:

```json
{
  "main": {"kind": "claude-cli", "model": "sonnet"},
  "weak": {"kind": "claude-cli", "model": "haiku"},
  "run_policy": "safe",
  "edit_policy": "ask",
  "delegate": "auto",
  "search": {"kind": "auto"},
  "privacy": "rules",
  "privacy_answers": true,
  "auto_learn": true,
  "screencodec": {"command": "", "hosts": "", "images": ""}
}
```

| Setting | Values | Effect |
|---|---|---|
| `main`, `weak`, `scout` | `kind` is `claude-cli`, `openai` or `anthropic` | The HTTP kinds take `base_url`, `model` and `api_key_env`. `anthropic` also takes `max_tokens` (default 16000) and `api_version`. Without `scout` the scout uses `weak`. |
| `run_policy` | `safe`, `all` | `safe` runs read-only and build commands without shell operators. Every other command asks in the interface and is refused outside it. `all` runs every command. |
| `edit_policy` | `ask`, `all` | `ask` asks before every change to a file. `all` changes files without a question. |
| `delegate` | `auto`, `web`, `offer`, `off` | `auto` sends every search to the scout, the main model has no search tools. `web` sends web research only. `offer` lets the main model choose. `off` has no scout. |
| `search.kind` | `auto`, `claude`, `duckduckgo`, `searxng`, `brave`, `off` | `auto` takes the first source that answers. `searxng` takes `url`, `brave` takes `api_key_env`. |
| `privacy` | `rules`, `pii`, `off` | `rules` masks secrets. `pii` also masks e-mail addresses and IBANs. `off` passes everything unchanged. |
| `privacy_answers` | `true`, `false` | `true` sends every finished text block of a model through the filter. |
| `auto_learn` | `true`, `false` | `true` lets the weaker model file rules and lessons after a turn. |
| `screencodec` | `command`, `hosts`, `images` | Path of `screencodec-mcp`, allowed hosts, picture folders. An empty `command` looks on the `PATH`. |

The safe list holds 25 programs such as `ls`, `cat`, `grep`, `git` and `cargo`. `git`, `cargo`, `npm`, `node`,
`python3` and `rustc` pass with read-only or build subcommands only. A command with `|`, `;`, `&&`, `>` or `sudo`
always asks.

`.penelope/plugins.json` holds one entry per plugin with `"allow": true` or `false`. A folder `.strix` or `.kate` of
a version before 0.5.0 is renamed to `.penelope` on the first start.

## How a turn reaches the screen

- With the `claude` command the main model runs as `claude -p --output-format stream-json --include-partial-messages`.
  Penelope reads one JSON line per event and draws it when it arrives.
- The tools live in a second process, the MCP server `penelope mcp`.
- With an HTTP endpoint the tool loop runs inside Penelope: send the history, run the tool calls, send the results.
- A question about a command or a file travels as a file `ID.req` in `.penelope/approvals/`, the answer as `ID.ans`.
  No answer within 120 seconds counts as no.
- A finished block of text passes the privacy filter in `cli/src/backend.rs` before the interface receives it.

## Measured

Main model Claude Sonnet, weaker model Claude Haiku. [`docs/RUNS.md`](docs/RUNS.md) holds every run with its numbers.

| What | Number | Runs |
|---|---|---|
| Output of `cargo tree` that reached the main model | 168 of 6,947 characters | 1 |
| Tokens of searching that stayed on Haiku in one request | 12,218 on Haiku, 8,669 on Sonnet | 1 |
| Tool calls with 2 lessons in the prompt | 1, down from 2 | 1 pair |
| Output tokens with 2 lessons in the prompt | 401, down from 656 | 1 pair |
| Made-up token that stayed hidden from the model | 5 of 5 turns | 5 |

No comparison run without condensing exists. No run of the lesson pair returned the right count.

## The crew

Laertes plans one small model family per job. `penelope crew` prints this table with the settings of the folder.
None of the eight families is trained yet.

| Family | Job | Today in Penelope |
|---|---|---|
| Nostos | tool calling | the weaker model runs lookups as scout |
| Tiresias | router and decisions | fixed rules: safe list for commands, walk through the memory map |
| Utis | privacy and PII detection | fixed rules in `cli/src/privacy.rs` |
| Aiolos | compression and summaries | the weaker model condenses long tool output |
| Argos | judge and scoring | not in use |
| Klea | data filter | not in use |
| Telemach | coding and agents | the main model |
| Proteus | ML engineering | not in use |

## Limits

- No request has reached a real provider through the kinds `openai` and `anthropic`. Both ran against a stand-in server only.
- Text appears piece by piece with the `claude` command only. An HTTP endpoint sends whole blocks.
- An HTTP endpoint gets Penelope's own tools only. ScreenCodec and the MCP servers of plugins need the `claude` command.
- The privacy filter works with fixed rules. A secret without a known prefix and without a key name passes.
- The level `pii` knows e-mail addresses and IBANs only. Names, addresses and phone numbers pass.
- A lesson is written by the weaker model and is not checked. `penelope memory unravel` takes a wrong one back.
- A scout cannot run commands outside the safe list and cannot change files.
- The interface was run in tmux with 124 columns and 40 rows. Other terminals are untested.
- `"allow"` covers every tool of every MCP server of one plugin. A rule for a single tool is missing.

[`docs/ROADMAP.md`](docs/ROADMAP.md) lists every limit and every planned part.
[`docs/PLAN.md`](docs/PLAN.md) orders the open parts into versions.

## Not built yet

Bots, scheduled tasks, a Hugging Face model browser, a graphical window, hooks and agents of plugins, subagents that
change files, a tool that starts training runs, the eight model families.

## Folders

| Folder | Content |
|---|---|
| [`cli/`](cli) | the Rust crate with the command `penelope` |
| [`cli/src/tui/`](cli/src/tui) | the full screen interface |
| [`brand/`](brand) | mark, lockup and the four pictures of the interface, see [`BRAND.md`](brand/BRAND.md) |
| [`docs/`](docs) | [`ROADMAP.md`](docs/ROADMAP.md), [`PLAN.md`](docs/PLAN.md), [`PROVIDERS.md`](docs/PROVIDERS.md), [`RUNS.md`](docs/RUNS.md), screen pictures |

## Name

In the Odyssey Penelope weaves a shroud for Laertes and undoes part of it every night. `penelope memory unravel` takes
notes back the same way. Versions before 0.5.0 were called Kate (`katecli`) and Strix Forgery (`strix`).
