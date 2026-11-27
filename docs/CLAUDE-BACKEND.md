# The claude backend

Penelope drives the official `claude` executable for models of kind `claude-cli`. It never opens, copies or refreshes the credential files of that command. The child logs in by itself.

## Transports

Config `claude.transport`:

- `sdk-stream` (default): one `claude` process per Penelope session. Penelope writes one JSON line per user message to its stdin and reads `system`, `assistant`, `user`, `stream_event` and `result` lines from its stdout. These are the flags and message shapes the Claude Agent SDKs use (`--input-format stream-json --output-format stream-json --verbose`, message `{"type":"user","message":{"role":"user","content":...}}`), checked against the sources of the Python SDK `claude-agent-sdk` 0.2.189. Penelope is a native Rust client of that protocol, no sidecar.
- `per-turn`: one `claude -p` process per turn, as before. Use it when the stream transport misbehaves. `PENELOPE_CLAUDE_TRANSPORT=per-turn` sets it for one run.

A new process starts when the flags change (model, effort, allowed tools, MCP servers, rules) or when the old one ended or crashed. The new process resumes the stored session with `--resume`. Stopping a turn kills the process, and the next turn starts a new one the same way.

## Which usage pool

Both transports start `claude` in print mode, the way the Agent SDKs do. According to the rule the owner cited (since 15 June 2026, `claude -p` and the Agent SDK are the allowed way for third-party harnesses on a Claude subscription, and they draw from the separate Agent SDK credit), a Penelope turn draws from the person's Agent SDK credit, not from the interactive Claude Code limit. Penelope does not read the pool from the account and does not verify this. The usage page of the account shows where a turn was counted.

## API keys

`ANTHROPIC_API_KEY` and `ANTHROPIC_AUTH_TOKEN` are removed from the environment of every `claude` child, so a turn never bills an API key by accident. Config `claude.api_key_mode: true` keeps them.

## What the child does not load

The child starts with `--setting-sources project,local` and `--disable-slash-commands`: the user settings of Claude Code (hooks of other tools that run at the start and the end of every turn, enabled plugins) and its skills and commands stay out. Penelope offers its own plugins and skills. Config `claude.user_settings: true` loads everything again.
The child also gets `CLAUDE_CODE_CLASSIFIER_SUMMARY=0` unless the person set it. Without it the command asks a model for a summary after every turn and holds the result back for about 2 seconds. Penelope does not show that summary.

## Tools, gates and filters

Penelope's own tools run through its MCP server (`penelope mcp`), started by `claude` with the process. The gates, the tool scan, the vet and the privacy filter run in that server and in the chat, as before. In the stream transport the server stays up between turns. Before each tool call it reads the live turn file (`.penelope/gates/live-*.json`, mode 0600): a new `seq` starts a new turn there (counters, lifted rules, the person's text for the anchor rule, the session taint). The person's text never goes on a command line.
The memory notes, lessons and matching skills of a turn go into the user message, marked as context from Penelope. The rules and the skill names stay in the system prompt, which does not change between turns.

## MCP servers per turn

Only servers that have an allowed tool are started. A plugin server with `allow: false` is not started. The ScreenCodec server starts when the request mentions the screen, a browser, a page, a picture or a click, and stays on for the rest of the session once a turn used it. Config `screencodec.always: true` starts it every time.

## Timing

`PENELOPE_TIMING=1` writes one line per phase to stderr. `PENELOPE_TIMING=/path/file` appends to a file, also from the TUI and from the MCP server. The hidden command `penelope bench-turns N "text"` runs N turns in one chat.
