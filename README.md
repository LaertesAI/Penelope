# Kate

```
   /\_/\
  ( o.o )   Kate
   > ^ <
```

Kate is a small agent harness for the command line. The command is `katecli`.

Status: version 0.2.0, private. Three real runs so far.

## What it does

| Part | What it does | State |
|---|---|---|
| Routed tool output | The tool `run` executes a shell command. Output over 40 lines goes to a weaker model first and comes back as at most 12 lines. The raw output stays on disk under a run id. | works |
| Web pages | The tool `fetch` loads a page and lets the weaker model answer one question from it. The page text never enters the main model. | works, 1 page tried |
| Memory map | Notes live at waypoints of a tree under `.kate/memory/`. A lookup follows signposts from the start, one level at a time, and never searches all notes. Finished walks are stored, the same request jumps to its waypoint the next time. | works |
| Eyes | If `screencodec-mcp` is installed, Kate connects it: pages and pictures come as text, never as screenshots. | wired, not tested inside Kate |
| Main model | The official `claude` command with the user's own login, or any OpenAI-compatible endpoint (API key or local model). | claude tested, OpenAI-compatible written and untested |
| MCP server | `katecli mcp` offers `run`, `run_detail`, `fetch`, `memory_go`, `memory_note`, `memory_map`, `skill` to any MCP client. | works |
| Auto-learning | After each answer the weaker model reads the exchange and files lasting rules of the user at a waypoint. | works, 1 run |
| Plugins | `katecli plugin add DIR` registers a plugin folder of Claude Code: skills by name and description, MCP servers for the main model. | works, 1 plugin tried (9 skills, 7 servers) |
| Statistic | `katecli stats` counts how many characters of tool output stayed away from the main model. | works |

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
katecli plugin add ~/path/to/plugin            # a plugin folder of Claude Code
katecli stats
katecli mcp                                    # for other MCP clients
```

## Settings: `.kate/config.json`

```json
{
  "main": {"kind": "claude-cli", "model": "sonnet"},
  "weak": {"kind": "claude-cli", "model": "haiku"},
  "run_policy": "safe",
  "auto_learn": true,
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
- Direct call `katecli run cargo tree`: 6,947 characters of output, 168 characters reached the main model (2 percent).
- `fetch` on https://example.org/ returned the purpose of the page in 3 lines.
- Second run: the sentence "commit messages are English and carry no emojis" was filed at `git/commits` by the weaker model.

## Not built yet

Bots, scheduled tasks, the Hugging Face browser with COAA cassettes, a graphical window with the cat, approval
prompts for commands outside the safe list, hooks and agents of plugins. Tools of plugin MCP servers are visible to
the main model but not on the allow list yet.
