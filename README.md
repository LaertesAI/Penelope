# Kate

```
   /\_/\
  ( o.o )   Kate
   > ^ <
```

Kate is a small agent harness for the command line. The command is `katecli`.

Status: first working version 0.1.0, private. One real run so far.

## What it does

| Part | What it does | State |
|---|---|---|
| Routed tool output | The tool `run` executes a shell command. Output over 40 lines goes to a weaker model first and comes back as at most 12 lines. The raw output stays on disk under a run id. | works |
| Web pages | The tool `fetch` loads a page and lets the weaker model answer one question from it. The page text never enters the main model. | written, not tested against a live page |
| Memory map | Notes live at waypoints of a tree under `.kate/memory/`. A lookup follows signposts from the start, one level at a time, and never searches all notes. Finished walks are stored, the same request jumps to its waypoint the next time. | works |
| Eyes | If `screencodec-mcp` is installed, Kate connects it: pages and pictures come as text, never as screenshots. | wired, not tested inside Kate |
| Main model | The official `claude` command with the user's own login, or any OpenAI-compatible endpoint (API key or local model). | claude tested, OpenAI-compatible written and untested |
| MCP server | `katecli mcp` offers `run`, `run_detail`, `fetch`, `memory_go`, `memory_note`, `memory_map` to any MCP client. | works |

## Build

```bash
cargo build --release
./target/release/katecli doctor
```

## Use

```bash
katecli init                                   # .kate folder in this project
katecli                                        # chat
katecli ask "Run the tests and tell me what fails"
katecli memory note --path java/microservices/api "REST endpoints use plural nouns"
katecli memory go "how do we name api endpoints in java"
katecli memory map
katecli run --ask "which test fails?" cargo test
katecli mcp                                    # for other MCP clients
```

## Settings: `.kate/config.json`

```json
{
  "main": {"kind": "claude-cli", "model": "sonnet"},
  "weak": {"kind": "claude-cli", "model": "haiku"},
  "run_policy": "safe",
  "screencodec": {"command": "", "hosts": "", "images": ""}
}
```

- `kind: "openai"` takes `base_url`, `model` and `api_key_env` and works with OpenAI, OpenRouter, Ollama or llama.cpp.
- `run_policy: "safe"` runs read-only and build commands without shell operators. `"all"` runs every command.
- A Claude subscription is used through the official `claude` command only. Kate has no login of its own.

## First real run (2026-11-23)

Request: run `cargo tree`, count the lines, name the TLS crate of ureq, store a note. Main model Claude Sonnet,
weaker model Claude Haiku.

- `cargo tree` printed 120 lines. Haiku condensed them, Sonnet answered "rustls v0.23.46" and "120 lines".
- A piped command (`cargo tree | wc -l`) was refused by the safe policy.
- The note landed at the waypoint `rust/kate`.
- Tokens of the main model: 9,070 in, 699 out. A comparison run without condensing has not been made.

## Not built yet

Plugins from Claude Code, bots, scheduled tasks, auto-learning after each turn, the Hugging Face browser with COAA
cassettes, a graphical window with the cat, approval prompts for commands outside the safe list.
