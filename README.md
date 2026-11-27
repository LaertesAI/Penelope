<p align="center"><img src="brand/lockup.png" alt="Penelope, the harness of Laertes" width="70%"></p>

<p align="center">
  <a href="cli/Cargo.toml"><img src="https://img.shields.io/badge/version-0.11.0-C4623E?style=flat-square&labelColor=1C1A17" alt="version 0.11.0"></a>
  <a href="cli/Cargo.toml"><img src="https://img.shields.io/badge/Rust_edition-2021-C4623E?style=flat-square&labelColor=1C1A17" alt="Rust edition 2021"></a>
  <a href="#build"><img src="https://img.shields.io/badge/tests-349_passing-C4623E?style=flat-square&labelColor=1C1A17" alt="349 tests passing"></a>
  <a href="#status"><img src="https://img.shields.io/badge/platform-Linux-C4623E?style=flat-square&labelColor=1C1A17" alt="platform Linux"></a>
  <a href="#parts"><img src="https://img.shields.io/badge/MCP_tools-54-C4623E?style=flat-square&labelColor=1C1A17" alt="54 MCP tools"></a>
  <a href="docs/PROVIDERS.md"><img src="https://img.shields.io/badge/providers-35-C4623E?style=flat-square&labelColor=1C1A17" alt="35 providers"></a>
  <a href="#status"><img src="https://img.shields.io/badge/status-alpha-C4623E?style=flat-square&labelColor=1C1A17" alt="status alpha"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/licence-MIT-C4623E?style=flat-square&labelColor=1C1A17" alt="licence MIT"></a>
</p>

# Penelope

Penelope is a coding agent for the terminal, written in Rust. It sends searches and long tool output to a cheaper
model, keeps notes in a memory map, and asks before every change to a file.

<p align="center"><img src="docs/screens/chat.png" alt="penelope in kitty: transcript on the left, the painted picture and the session numbers on the right" width="100%"></p>

## Status

| | |
|---|---|
| Version | 0.11.0, set in [`cli/Cargo.toml`](cli/Cargo.toml) and [`core/Cargo.toml`](core/Cargo.toml) |
| Date | 2026-11-25 |
| Stage | alpha: one developer, one PC, no packaged release |
| Repository | https://github.com/LaertesAI/Penelope, branch `main` |
| Source | [`core/`](core) and [`cli/`](cli), one library and one binary |
| Binary | `target/release/penelope`, built from source |
| Build | `cargo build --release -p penelope-cli`, profile `release` with `opt-level = 2` and `strip = true` |
| Toolchain | rustc 1.99.1, edition 2021 |
| Tests | `cargo test --workspace`, no test needs a key. Live runs are listed in the devlog. |
| Real runs | 29 with the `claude` command up to 0.10.0, live OpenCode Go runs on 2026-11-24, and three live cluster runs with Claude + haiku on 2026-11-25. See [`DEVLOG.md`](docs/DEVLOG.md). |
| Platform | Linux. Background jobs read `/proc`. Windows and macOS are untried. |
| Settings folder | `.penelope/` in the project, else `~/.penelope/` |
| Licence | MIT, see [`LICENSE`](LICENSE). `cli/Cargo.toml` carries the same field. |

The badges above are static. Version and edition come from `cli/Cargo.toml` and `core/Cargo.toml`.
The tool count comes from `spec::TOOLS`, the provider count from `core/src/providers.rs`.

## What sets Penelope apart

- Cost mode (`route: "auto"`): a task starts on Haiku with effort low and moves to Sonnet with effort high when the
  tests fail twice, the watcher reports 3 strikes, you type `/escalate`, or the context passes 100,000 tokens. Measured
  on 28 tasks at 9.6 % of the cost; the hold-out is still running. See [`docs/ROUTING.md`](docs/ROUTING.md).
- Searches run on a cheaper model. In one request 12,218 tokens of searching stayed on Claude Haiku, the main model
  used 8,669.
- Long command output is shortened before the main model reads it. In one run 6,947 characters became 168.
- Notes live at waypoints of a tree. After a failed tool call the weaker model files up to 3 lessons, and every later
  turn reads them.
- A filter masks keys, passwords and tokens before any model reads them, in requests, files, tool output and answers.
- A change to a file shows its diff and waits for a yes. `undo` takes the last 50 changes back.
- The model is your choice in every role: the `claude` command with your own login, one of 35 providers, a local
  server, or a Laertes family that is still in training and is shown in gold.
- One contract layer checks every tool call: the role must hold the tool, then every argument must match the JSON schema.
- Agents are written with `$name(role=..., goal="...", max_sub_agents=3)`; the lead can also delegate with the tool
  `agent`. A store of memory is shared by every session, and `penelope brain` writes an Obsidian vault of it.
- Clusters are the larger architecture: an orchestrator builds groups from parts, starts them in parallel, orders
  them with `after`, and gives every cluster one memory file. A part can be a Laertes family, an OpenCode model or the
  Claude command. The validator can send a result back before the cluster closes.

## Build

Needed: a Rust toolchain, Linux, and one model source (the `claude` command, an API key, or a local model server).

```bash
cargo build --release -p penelope-cli
cargo test --workspace              # no model call
./target/release/penelope doctor    # one line per program and setting Penelope depends on
```

The workspace is described in [`docs/WORKSPACE.md`](docs/WORKSPACE.md): `core/` is `penelope-core`, `cli/` is the command.

## First steps

```bash
penelope setup --here    # makes .penelope/ in this folder and asks seven questions
penelope                 # opens the full screen interface
penelope ask "Run the tests and tell me what fails"
```

## Models and providers

A provider table with 35 entries names the address, the wire format and the key variable of each provider.
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

The two screens are separate. `/providers` connects a provider: arrows choose, `tab` switches main/weaker, `enter` signs in with the Claude command or opens the masked API-key window. `/model` chooses the model of the current provider: arrows choose, `tab` switches main/weaker, `r` opens the role models, `e` changes the effort, `enter` applies. The model names come from the provider's own list endpoint (`/models` for OpenAI-compatible addresses, `/v1/models` for Anthropic), are cached under `.penelope/models/`, and fall back to the built-in catalog when the endpoint has no key or no list. The Claude command has no list endpoint, so its three subscription aliases stay built in. `penelope models [PROVIDER]` prints the same list in the terminal.
- Claude: the official `claude auth login` page, then the terminal comes back.
- API providers: a second window takes the key and shows only its first four characters; `enter` stores it, `esc` cancels.

- `config.json` stores the name of the environment variable. A key is read at the start of a request and stored nowhere.
- The setup refuses a pasted key in place of a variable name.
- A Claude subscription works through the `claude` command only. Penelope has no login of its own.
- Tested with real models: the `claude` command and OpenCode Go. The two HTTP formats have tests against a local test server.
- Effort is sent only when the model has the field: `--effort` for the Claude command, `output_config.effort` and
  adaptive thinking for the Anthropic messages API, and `reasoning_effort` for OpenAI, OpenRouter, xAI, DeepSeek,
  Alibaba/Qwen, Zhipu/GLM, Moonshot/Kimi, Gemini and the OpenCode catalog. The levels are per model, not one global
  list. A model without the field shows `no effort` and no parameter is sent.
- [`docs/PROVIDERS.md`](docs/PROVIDERS.md) names the source of every address, read on 2026-11-24, and every field of a model setting.

## Parts

"Since" names the version that added the part. "Real use" counts runs with a real model.

| Part | What it does | Since | Real use |
|---|---|---|---|
| Routed tool output | `run` executes a shell command. Output over 40 lines goes to the weaker model and comes back as at most 12 lines. The raw output stays on disk under a run id. | 0.1 | yes |
| Web pages | `fetch` loads a page and the weaker model answers one question from it. The page text never enters the main model. | 0.1 | 1 page |
| Memory map | Notes live at waypoints under `.penelope/memory/`. A lookup follows signposts one level at a time and never searches all notes. A finished walk is stored, so the same request jumps to its waypoint. | 0.1 | 3 runs |
| MCP server | `penelope mcp` offers all 54 tools to any MCP client. | 0.1 | every turn |
| MCP management | `penelope mcp add/remove/allow/deny/list/call` manages standalone servers next to the plugin servers. | 0.11 | 1 test, live echo server |
| Local HTTP API | `penelope serve` answers `/search`, `/read`, `/embed`, `/rerank`, `/redact` and `/whoami` on loopback, with a bearer token for a remote bind and 120 requests per minute. | 0.11 | 6 tests, live curl |
| Worktrees | `penelope work new/list/diff/apply/remove` keeps a task on its own branch; the live tree changes only on `apply`. | 0.12 | 1 test, live git worktree |
| Checkpoints and rewind | A checkpoint at every turn; `/rewind N` restores the transcript and the file changes of the last N turns. | 0.12 | 2 tests |
| Providers and models | `/providers` connects a provider; `/model` chooses the model of the current provider. Two separate screens, no mixed list. A pasted API key goes into the masked key field, never the prompt. | 0.12 | 3 tests, live screens and paste |
| Role models | `/model` → `r`: tool calls, processing, research/web search, coding, judge, router and filter, each with its own provider. | 0.12 | 1 test |
| Exact models and effort | `/model` asks the provider for its model list (cached under `.penelope/models/`), falls back to the built-in names, and shows the provider's own effort column. `e` cycles it; a model without the field shows `no effort` and gets no parameter. | 0.12 | 3 tests, live OpenCode list |
| Monthly budget | `/budget 20`, consumption and the rest of the month in the side panel and `/cost`. | 0.12 | 1 test |
| Sandbox and allowlist | `/sandbox on` runs shell commands through bubblewrap without network; a confirmed command is kept hashed for this project. | 0.12 | 2 tests, live bwrap |
| CI event stream | `penelope ask --json` writes newline-delimited events for CI and editors. | 0.12 | live Claude event stream |
| Search ops | `search probe [--daily]`, `search instances add URL`, health file and a systemd timer. `fanout` is on by default: every configured engine is asked in parallel, then the hits are deduplicated, scored and merged. `/search fanout on|off` switches it. | 0.12 | 1 test, live 3 engines |
| Eyes | With `screencodec-mcp` installed, pages and pictures arrive as text. | 0.1 | wired, no run in the harness |
| Auto-learning | After each answer the weaker model files lasting rules of the user at a waypoint. | 0.2 | 2 runs |
| Plugins | `penelope plugin add DIR` registers a plugin folder of Claude Code with its commands, skills and MCP servers. `/plugins` is a filterable screen; a command runs as `/plugin:command ARGS`. | 0.2 / 0.12 | 3 tests, live command turn |
| ZIP and server | `unzip` unpacks a ZIP inside the workspace after one question; `penelope serve` runs the local search, read, embed, rerank and redact API. | 0.11 | 3 tests, live ZIP, live HTTP API |
| Statistic | `penelope stats` counts the characters kept away from the main model and the masked items. | 0.2 | yes |
| Full screen interface | Transcript, input box, side panel with the painted picture, command list on `/`, palette on `ctrl+p`. | 0.3 | 12 turns |
| Approval | A command outside the safe list asks: run once, allow all in this session, or refuse. | 0.3 | 3 commands |
| Privacy filter | Known key formats, values behind names such as `password` or `token`, and private key blocks become `[masked:...]`. The level `pii` adds e-mail addresses and IBANs. | 0.4 | 5 turns |
| Text piece by piece | The answer appears while the model writes. | 0.4 | 15 words in 7 pieces |
| Sessions | Every conversation is stored with its transcript. The top row shows sessions as tabs. | 0.5 | 9 sessions |
| Unravel | `penelope memory unravel [N]` takes back the last N notes. | 0.5 | test only |
| Scout | A search task runs on the weaker model with 10 read-only tools. The main model gets a report of at most 15 lines. | 0.6 | 4 tasks |
| Web search | `web_search` ranks a server API, Startpage, DuckDuckGo, Bing, SearXNG, Brave, Tavily, Exa, Firecrawl and Wikipedia, with a cache and a score. | 0.6 / 0.11 | 9 tests, live 8 results |
| File tools | `tree`, `outline`, `read`, `find`, `grep`, `write`, `edit`. | 0.6 | 3 changes |
| Plan | The main model keeps the steps of a task. The side panel shows them. | 0.6 | 2 runs |
| Setup | `penelope setup` asks seven questions with numbered answers. | 0.6 | test only |
| Tool contract layer | One table in `spec.rs`; `route::dispatch` checks the role and every argument before a handler runs. A forged tool joins the same gate. | 0.11 | 4 tests, 3 live tool calls |
| Eight roles | lead, scout, worker, judge, tool, condenser, filter, router. `penelope provider use NAME --role ROLE` plugs a model into any of them. | 0.11 | `penelope route`, 1 live agent |
| Privacy model | `roles.filter` puts a model between the others. Strict JSON, fallback to the fixed rules, failures counted. | 0.11 | test against a stand-in server |
| Router model | `roles.router` chooses the plan. A broken plan falls back to the fixed plan; the limits of the user always win. | 0.11 | test against a stand-in server |
| Agents with `$` | Name, role, thinking, goal, limits, budget. The goal is not stored. The crew panel shows the names and models. | 0.11 | 1 live one-step agent, parser tests |
| Clusters | The orchestrator starts 1–4 clusters from parts. A profile names workers, an optional validator, slots (Telemach, Argos, Nostos), model or role per part, max parts and depth. Clusters can run in parallel and wait on each other with `after`; every part writes one `memory.md`; the validator can send the result back. `/cluster GOAL`, `/clusters`, `penelope cluster`. | 0.12 | 5 tests, live solo cluster with worker + validator |
| Crew panel | Lead, subagents by their names, plugged model and state. A Laertes family is gold and says "in training". | 0.11 | screen picture, 1 live run |
| Shared memory | `scope global` and a shared map for every project; consolidation keeps rules and complaints without a model call. | 0.11 | 2 tests |
| Brain and Obsidian | `/brain` draws the map; `penelope brain --write` makes a vault of Markdown files with `[[links]]`. | 0.11 | 1 test, screen picture |
| Cost in euro | `.penelope/usage.json` per role and model, prices in `config.json`, `penelope cost` and `/cost`. | 0.11 | 1 test, live rows |
| ScreenCodec and cursor | `screen` with the real tools and ids, `desktop`, `cursor` through hyprctl, xdotool, ydotool, the uinput helper, PowerShell or macOS Quartz. | 0.11 | 122 lines read, live desktop click 0→1→2, native pointer moved |
| Plugin MCP tools | `mcp_servers` and `mcp_call` behind the plugin allow gate, for HTTP models too. | 0.11 | 1 test |
| Plugin install and import | `/plugins install LINK` clones, unpacks or registers a plugin; the first start takes over the plugins of other harnesses. A ZIP is unpacked into the plugin home first. | 0.11 | 5 tests, live ZIP |
| Plugin administration | `plugin update NAME` clones the new commit of a git plugin into a quarantine folder, checks it there and switches the registry only after a clean check; a local/ZIP plugin is re-read. The registry pins every git plugin to its checked commit. `plugin remove NAME` takes it out of the registry and leaves the folder. | 0.11 | 3 tests, live update/remove |
| More tools | `session_search`, `http_request`, `json_query`, `git_diff`, `clipboard`, `agent`. | 0.11 | 1 test |
| Logo and keys | The woven mark above the wordmark, a short weaving animation on the home screen, F5 for a new session, F6 twice for delete, Delete in the session list. | 0.11 / 0.12 | TUI tests, tmux capture |
| Web search, ranked | 10 sources, cache, canonical URLs, diversity, scores, rate-limit fallback. A server search API and a reranker follow the same JSON contract. | 0.11 | 9 tests, live 8 results, live health rows |
| Scraper | HTML to Markdown: article, metadata, JSON-LD, lists, tables, code, boilerplate removed. | 0.11 | 6 tests |
| Research | several queries, the pages behind the hits, a cited report on the weak model. | 0.11 | 1 test, live cited report |
| Page warnings | The scraper flags instruction lines, zero-width/bidi text, CSS-hidden blocks, `hidden`/`aria-hidden` and fake system lines instead of obeying them. | 0.11 | 2 tests |
| PDF and reader API | `fetch` reads a PDF with the local `pdftotext`; a scan or a JavaScript page can fall back to one configured reader endpoint. A private target is refused before the request. | 0.11 | live PDF, 2 tests, live `/read` |
| Login and verification | `penelope login` signs in to Claude or stores an API key in the Secret Service (file fallback, mode 0600). `penelope auth` shows the state; `penelope auth verify` asks each provider for its model list. | 0.11 | live Claude verify, 1 fake-server test, 1 keyring test |
| Modes | `auto`, `manual`, `safe`, `review`, `privacy`, `power` as presets of delegate, run, edit, privacy and verify. | 0.11 | 1 test, sidebar row |
| Tool role and `computer` | A smaller model with ScreenCodec, cursor and MCP tools takes a whole computer task; its steps stay out of the lead's context. | 0.11 | 1 test, 1 live run |
| Skills Penelope writes | A successful turn with 5 or more tool calls writes a playbook under `.penelope/skills`; `skill_new` writes one on purpose. | 0.11 | 1 test, 1 live skill |
| Leak audit | `penelope audit` scans the settings folder and the endpoints; keys and plain http are reported, never printed. | 0.11 | 1 test, 0 findings live |
| Native pointer | `penelope input install` builds a `/dev/uinput` helper for Wayland; ScreenCodec uses it when ydotool is absent. | 0.11 | live desktop click 1→2 |
| Updater | `penelope update --check` and `update` read a signed manifest; no server yet. | 0.11 | 2 tests, blueprint |
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

The 54 tools: `run`, `run_detail`, `job_start`, `job_read`, `job_stop`,
`fetch`, `web_search`, `research`, `read`, `write`,
`edit`, `unzip`, `pr_create`, `undo`, `find`,
`grep`, `tree`, `outline`, `lsp`, `ask_user`,
`scout`, `scouts`, `plan`, `memory_go`, `memory_note`,
`memory_forget`, `memory_map`, `skill_new`, `skill`, `git_commit`,
`check`, `tool_new`, `tool_list`, `mcp_servers`, `mcp_call`,
`session_search`, `http_request`, `json_query`, `git_diff`, `clipboard`,
`agent`, `cluster_start`, `cluster_spawn`, `cluster_ask`, `cluster_status`,
`cluster_kill`, `cluster_dump`, `cluster_respawn`, `cluster_profile`, `computer`,
`screen`, `cursor`, `brief_check`, `brief_put`.

## The full screen interface

<p align="center"><img src="docs/screens/home.png" alt="Start screen of penelope in kitty with the painted picture of Penelope at the loom and the input box" width="80%"></p>

The painted picture is a real picture, sent through the graphics protocol of kitty. It shows in kitty, Ghostty, WezTerm and Konsole.
Other terminals and tmux show the name alone. `PENELOPE_PICTURE=off` switches it off, `PENELOPE_PICTURE=on` forces it.
The screens in this file are pictures of kitty on a virtual display. The chat screen shows the stored transcript of the run of 2026-11-24, opened as a session.

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

Commands: `/new` (or F5), `/model` (`r` role models, `e` effort), `/providers` (connect a provider), `/sessions`, `/setup`, `/undo`, `/jobs`,
`/scout [auto|web|offer|off]`, `/crew`, `/map`, `/go TEXT`, `/note TEXT`, `/btw TEXT`, `/rules [session|global] TEXT`, `/forge TOPIC`,
`/unravel`, `/forget TEXT`, `/stats`, `/cost`, `/context TEXT`, `/route [auto|fixed]`, `/escalate`, `/tools`, `/agents`, `/orchestrate GOAL`,
`/cluster GOAL [--profile NAME]`, `/clusters`.
`/brain [write]`, `/mcp [probe|add NAME COMMAND|remove NAME|allow NAME|deny NAME|call SERVER TOOL JSON]`,
`/screen tools|open URL|see|click ID|desktop`, `/cursor X Y [click]`, `/export`, `/price MODEL IN OUT`, `/auto`,
`/learn`, `/skills`, `/consolidate`, `/mode [NAME]`, `/login [NAME]`, `/logout NAME`, `/auth [verify]`, `/rewind N`,
`/budget [EUR]`, `/sandbox on|off`,
`/input [install|enable]`, `/audit`, `/update`,
`/plugins` (screen: filter, tab all/commands/MCP, A allow, U update, X twice removes), `/plugin:command ARGS`,
`/search fanout on|off`, `/doctor`, `/copy`, `/shot`, `/sidebar`,
`/help`, `/quit`.

One prompt may hold several commands, separated by a newline or `;;`:

```text
/scout web;;/note tests run with cargo test
/btw the API host is staging.internal;;/cost
```

`@path` in a request attaches a compact context block: a short file whole, a long file as outline and first lines, a
folder as a one-level tree. `@memory:topic` attaches the waypoint. `/context TEXT` shows the block before a model sees
it. A `$name(...)` call runs an agent; the same form in `/orchestrate` or on the command line does too.

`/search WORDS` runs the ranked search, `/research QUESTION` runs the cited research report, and `penelope research
QUESTION` does the same on the command line. The design, engine table and limits are in
[`docs/SEARCH.md`](docs/SEARCH.md). `/research deep QUESTION` and `penelope research --deep QUESTION` run the multi-round
method with a plan, parallel researchers, a gap loop, checked citations and an assumptions list; see
[`docs/DEEP-RESEARCH.md`](docs/DEEP-RESEARCH.md).

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
penelope search "ratatui terminal library"    # ranked results
penelope search --engine bing --fresh "ratatui"
penelope search health --probe                # one small query per engine, then the counters
penelope search probe --daily                 # one probe a day, for a timer
penelope search instances add http://127.0.0.1:8080
penelope work new task-1                      # a git worktree on branch agent/task-1
penelope work diff task-1                     # the patch, live tree untouched
penelope work apply task-1                    # one explicit yes
penelope allow                                # confirmed commands for this project, masked
penelope ask --json "run the tests"           # event stream for CI
penelope cluster run "build the thing"         # the orchestrator plans clusters and runs them
penelope cluster profiles                      # the parts of every profile
penelope cluster list                          # the cluster tree
penelope models                                # the model list the provider reports
penelope plugin commands                      # slash commands provided by plugins
penelope plugin command notes:summarize a.md  # print the expanded prompt
penelope research "How does tokio select work?" --depth standard --pages 4
penelope scout "Where does this project parse its settings?"
penelope run --ask "which test fails?" cargo test
penelope eval tasks.jsonl --setup all   # large, large with subagents, cluster; writes docs/RUNS.md
penelope replay <run-id> --model MODEL  # re-run stored eval prompts read-only
penelope undo                     # take back the last change of write or edit
penelope jobs                     # background jobs of this folder
penelope jobs read N              # new output of job N
penelope jobs stop N              # end job N and every process it started
penelope plugin add ~/path/to/plugin
penelope plugin update NAME   # check a new git commit in quarantine, or re-read a local or ZIP plugin
penelope plugin remove NAME   # take it out of the registry; the folder stays
penelope plugin install LINK   # clone a git plugin, or unpack a .zip, and register it
penelope plugin import         # take over the plugins of other harnesses once
penelope plugin list
penelope plugin allow NAME
penelope plugin deny NAME
penelope tools                 # every tool, its arguments and the roles that may call it
penelope tool call NAME JSON   # one call through the same gate a model uses
penelope route                 # roles, tools and models
penelope agents                # the agents written with $name(...)
penelope orchestrate '$name(goal="...")'   # quote the $ in a shell
penelope brain [--write]       # the graph, or an Obsidian vault
penelope cost                  # tokens and euro per role and model
penelope price MODEL IN OUT    # dollars per million tokens, for the euro report
penelope consolidate           # keep the lasting rules of the last session
penelope screen demo URL       # real ScreenCodec: open and read in one process
penelope screen tools          # the tools ScreenCodec really offers
penelope cursor X Y [--click]  # Penelope's own pointer
penelope stats
penelope mcp [--root DIR]         # serve the 54 tools to another MCP client
penelope mcp add NAME COMMAND [ARGS...] [--allow]
penelope mcp remove NAME
penelope mcp allow|deny NAME
penelope mcp call SERVER TOOL JSON
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
  "route": "fixed",
  "route_context_limit": 100000,
  "test_command": "",
  "test_attempts": 3,
  "delegate": "auto",
  "search": {"kind": "auto", "fanout": true, "cache": true, "cache_ttl": 3600, "limit": 8, "language": "en", "safe": true,
             "jina": false, "wikipedia": false, "api_url": "", "urls": [],
             "rerank": {"enabled": false, "url": "", "api_key_env": "RERANK_API_KEY"}},
  "reader": {"enabled": true, "url": "", "api_key_env": "PENELOPE_READER_API_KEY"},
  "privacy": "rules",
  "privacy_answers": true,
  "auto_learn": true,
  "roles": {},
  "mode": "auto",
  "agent_verify": true,
  "auto_skills": true,
  "update": {"url": "", "channel": "stable", "public_key": ""},
  "screencodec": {"command": "", "hosts": "", "images": "", "args": "", "desktop": "off", "desktop_input": "off", "input_cmd": ""},
  "cursor": {"command": "", "enabled": true},
  "eur_per_usd": 0.92,
  "prices": {"opus": [15.0, 75.0], "sonnet": [3.0, 15.0], "haiku": [0.8, 4.0]},
  "global_memory": null,
  "plugin_dirs": null,
  "plugin_home": null,
  "clipboard": {"read": "", "write": ""}
}
```

| Setting | Values | Effect |
|---|---|---|
| `main`, `weak`, `scout` | `kind` is `claude-cli`, `openai` or `anthropic` | The HTTP kinds take `base_url`, `model`, `api_key_env` and optionally `effort` (`low`, `medium`, `high`, `max`, or an OpenCode level). `anthropic` also takes `max_tokens` (default 16000) and `api_version`. Without `scout` the scout uses `weak`. Search stays client-side by default; the local server is in [`docs/SERVER.md`](docs/SERVER.md) and the remaining APIs are in [`docs/SERVER-APIS.md`](docs/SERVER-APIS.md). |
| `run_policy` | `safe`, `all` | `safe` runs read-only and build commands without shell operators. Every other command asks in the interface and is refused outside it. `all` runs every command. |
| `edit_policy` | `ask`, `all` | `ask` asks before every change to a file. `all` changes files without a question. |
| `test_command` | shell command | Empty means the test loop is off. After a successful `write` or `edit`, Penelope runs the command in the project or the call's `cwd` and adds its last 20 lines to the tool result. It is a normal shell command, not another tool call. |
| `test_attempts` | number, default 3 | Failed test runs in one turn before the loop stops. The next tool result says `TEST LOOP STOPPED`; no further tool runs in that turn and the main model answers with the failure. A new user turn starts the count again. A cluster part uses its profile's `test_command` only, so the loop stays off there by default. |
| `delegate` | `auto`, `web`, `offer`, `off` | `auto` sends every search to the scout, the main model has no search tools. `web` sends web research only. `offer` lets the main model choose. `off` has no scout. |
| `search` | object | `kind` is `auto`, `startpage`, `duckduckgo`, `bing`, `searxng`, `brave`, `tavily`, `exa`, `firecrawl`, `wikipedia`, `api` or `off`. `fanout` asks all configured engines and merges; `cache` and `cache_ttl` keep a query; `limit`, `language`, `safe`; `jina` sends a JavaScript page to `r.jina.ai`; `wikipedia` adds it to the normal fanout. `api_url` is a server search API (`POST` with `{query, language, country, safe, recency, limit, page}`; `server_api_key_env` names its bearer key); `urls` lists several SearXNG instances; `rerank` is `{enabled, url, api_key_env}` and reorders the top hits with `{query, documents, top_k}`. Key variables: `api_key_env` (Brave), `tavily_api_key_env`, `exa_api_key_env`, `firecrawl_api_key_env`, `PENELOPE_SEARCH_API_KEY`, `RERANK_API_KEY`; the reader row names `PENELOPE_READER_API_KEY`. `penelope login tavily|exa|firecrawl|brave|search|rerank|reader` stores a service key. `penelope search health [--probe]` shows or measures each engine. |
| `reader` | `enabled`, `url`, `api_key_env` | The P1 reader endpoint. Empty `url` means no server reader; static HTML and local PDF still work. `POST {url, max_chars, formats:["markdown"], wait_ms}` must answer `{title, author, published, markdown, links, final_url, content_type}`. A private, loopback or unresolved target is refused before the request. |
| `privacy` | `rules`, `pii`, `off` | `rules` masks secrets. `pii` also masks e-mail addresses and IBANs. `off` passes everything unchanged. |
| `privacy_answers` | `true`, `false` | `true` sends every finished text block of a model through the filter. |
| `auto_learn` | `true`, `false` | `true` lets the weaker model file rules and lessons after a turn. |
| `screencodec` | `command`, `hosts`, `images`, `args`, `desktop`, `desktop_input`, `input_cmd` | Path of `screencodec-mcp`, allowed hosts, picture folders, extra arguments. `desktop` is `off`, `ask` or `on`; `desktop_input` allows clicks; `input_cmd` names the pointer injector. `penelope input install` and `input enable` fill the last two. An empty `command` looks on the `PATH`. |
| `roles` | object | One model setting per role: `router`, `filter`, `judge`, `worker`, `condenser`. A role without a model runs on fixed rules. `penelope provider use NAME --role ROLE` writes one. |
| `cursor` | `command`, `enabled` | `command` takes `{x}`, `{y}` and `{label}`; empty tries xdotool or ydotool. `enabled` false turns the pointer off. |
| `prices` | object | Dollars per million tokens: `{"model-name": [input, output]}`. The longest name that occurs in the model wins. A model without a price is marked `no price`. |
| `monthly_budget_eur` | number | A monthly euro limit, default 0 (no limit). The side panel and `/cost` show spent and remaining. Set it with `/budget 20`. |
| `sandbox` | `off`, `bwrap` | `bwrap` runs every `run` through bubblewrap: read-only root, the working folder writable, private `/tmp`, no network. |
| `eur_per_usd` | number | The rate for the cost report, default 0.92. |
| `global_memory` | path | The shared memory. Empty means `~/.penelope/global-memory`. |
| `plugin_dirs` | list | Folders that `plugin import` scans. Empty means the folders of Claude, Codex, Cursor, OpenCode and `~/.penelope/plugins`. |
| `plugin_home` | path | Where `plugin install` clones a plugin, default `~/.penelope/plugins`. |
| `clipboard` | `read`, `write` | Shell commands for the clipboard when no wl-clipboard, xclip or pbcopy is installed. Text goes in through stdin. |
| `mode` | `auto`, `manual`, `safe`, `review`, `privacy`, `power` | The preset shown in the side panel. `penelope mode NAME` writes the fields. |
| `agent_verify` | true, false | Adds the judge step to an agent unless the `$` call says otherwise. |
| `auto_skills` | true, false | A successful turn with 5 or more tool calls writes a playbook skill. |
| `update` | `url`, `channel`, `public_key` | The updater. No server yet; see [`docs/AUTOUPDATE.md`](docs/AUTOUPDATE.md). |

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
- A finished block of text passes the privacy filter in `core/src/backend.rs` before the interface receives it.

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

## Agents with `$`

```text
$builder(role=orchestrator,thinking=max,goal="Add a --version flag and a test",max_sub_agents=3,verify=true)
```

Settings: `role` (orchestrator, lead, worker, judge), `thinking` (max, high, medium, low), `goal`, `max_sub_agents`
(1 to 8), `max_parallel` (1 to 4), `max_steps` (2 to 30), `budget_eur`, `model`, `verify`, `language`. The name and the
settings are stored in `.penelope/agents.json`; the goal is not. The plan is inspect, work, check, report; a router
model may choose another plan inside the same limits. Each step runs on the model of its role with the tools of that
role. The side panel shows the subagents by the names of the run:

```text
CREW
lead          mimo-v2.6-flash · openai · ready
builder.plan  lead · sonnet · done
builder.work  worker · Haiku 5.5 Mid · working
builder.check judge · Argos 1 · in training
gold · Laertes model, in training
```

A Laertes family in a role is shown in gold and says `in training`; it is refused before a request is sent. Plug one
in for the display with `penelope provider use laertes --role judge --model "Argos 1"`.

<p align="center"><img src="docs/screens/crew-0.11.0.png" alt="The crew panel: the lead and a named subagent, its model in gold because Argos 1 is in training" width="80%"></p>

## The crew

Laertes plans one small model family per job. `penelope crew` prints this table with the settings of the folder.
None of the eight families is trained yet.

| Family | Job | Today in Penelope |
|---|---|---|
| Nostos | tool calling | the weaker model runs lookups as scout |
| Tiresias | router and decisions | fixed rules: safe list for commands, walk through the memory map |
| Utis | privacy and PII detection | fixed rules in `core/src/privacy.rs` |
| Aiolos | compression and summaries | the weaker model condenses long tool output |
| Argos | judge and scoring | not in use |
| Klea | data filter | not in use |
| Telemach | coding and agents | the main model |
| Proteus | ML engineering | not in use |

## Limits

- Live runs on 2026-11-24 reached OpenCode Go through the `openai` kind: one `run` tool call, one one-step agent and
  one ScreenCodec click sequence. The key lived only in the environment; `config.json` holds the variable name.
- The `anthropic` kind and every other provider were tested against a stand-in server only.
- Text appears piece by piece with the `claude` command only. An HTTP endpoint sends whole blocks.
- An HTTP endpoint gets Penelope's own tools. Plugin MCP tools are reached with `mcp_servers` and `mcp_call`, one
  process per call; a stateful server (a browser, a database session) still needs the `claude` command or its own tool.
- The privacy filter works with fixed rules. A secret without a known prefix and without a key name passes.
- The level `pii` knows e-mail addresses and IBANs only. Names, addresses and phone numbers pass.
- A lesson is written by the weaker model and is not checked. `penelope memory unravel` takes a wrong one back.
- A scout cannot run commands outside the safe list and cannot change files.
- The interface was run in tmux with 120 and 124 columns and 40 rows. Other terminals are untested.
- The Windows branches (`cmd /C`, `taskkill`, `clip`, PowerShell pointer) compile only as source; no Windows machine was
  available to run them.
- `penelope input enable` writes a helper that injects real clicks through `/dev/uinput`; use it only on a desktop whose
  user has an ACL on that device. The setting is off until you run it.
- `"allow"` covers every tool of every MCP server of one plugin. A rule for a single tool is missing.
- DuckDuckGo and Bing refuse automated clients after a few queries from one address. Wikipedia is the last resort; a
  SearXNG instance or a keyed search API is the reliable path. No Google, Yandex or Mojeek.
- The search ranker is lexical, not semantic. A page that needs JavaScript needs ScreenCodec or `search.jina`.
- `max_parallel` of an agent is stored and shown, but the steps run in order: a worker may change a file that the judge
  then reads.
- A real desktop click worked on this Wayland PC through the `/dev/uinput` helper (counter 0 → 1 → 2). Without the
  helper, Wayland needs ydotool with a running ydotoold; X11 uses xdotool; macOS uses cliclick or Quartz; Windows uses
  PowerShell. Windows and macOS were not run on hardware.
- A price is a proposal in `config.json`. A model without a price is counted and marked `no price`, never guessed.

[`docs/ROADMAP.md`](docs/ROADMAP.md) lists every limit and every planned part.
[`docs/PLAN.md`](docs/PLAN.md) orders the open parts into versions.

## Not built yet

Bots, scheduled tasks, a Hugging Face model browser, a graphical window, hooks and agents of plugins, subagents in an
isolated worktree, a long-lived MCP client for stateful servers, a tool that starts training runs, the eight model
families.

## Folders

| Folder | Content |
|---|---|
| [`core/`](core) | `penelope-core`, the shared library: settings, models, tools, clusters, memory, cost |
| [`cli/`](cli) | the Rust crate with the command `penelope` |
| [`cli/src/tui/`](cli/src/tui) | the full screen interface |
| [`brand/`](brand) | mark, lockup and the four pictures of the interface, see [`BRAND.md`](brand/BRAND.md) |
| [`docs/`](docs) | [`ROADMAP.md`](docs/ROADMAP.md), [`PLAN.md`](docs/PLAN.md), [`PROVIDERS.md`](docs/PROVIDERS.md), [`RUNS.md`](docs/RUNS.md), [`DEVLOG.md`](docs/DEVLOG.md), [`AUTOUPDATE.md`](docs/AUTOUPDATE.md), [`SEARCH.md`](docs/SEARCH.md), [`SERVER.md`](docs/SERVER.md), [`SERVER-APIS.md`](docs/SERVER-APIS.md), [`NEXT.md`](docs/NEXT.md), [`PLAN-0.12.md`](docs/PLAN-0.12.md), [`FEATURES-BUGS.md`](docs/FEATURES-BUGS.md), [`Penelope 0.11.0 Devlog.pdf`](docs/Penelope%200.11.0%20Devlog.pdf), screen pictures |

## Name

In the Odyssey Penelope weaves a shroud for Laertes and undoes part of it every night. `penelope memory unravel` takes
notes back the same way. Versions before 0.5.0 were called Kate (`katecli`) and Strix Forgery (`strix`).
