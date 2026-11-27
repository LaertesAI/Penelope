# The claude backend

Penelope drives the official `claude` executable for models of kind `claude-cli`. It never opens, copies or refreshes the credential files of that command. The child logs in by itself.

## Transports

Config `claude.transport`:

- `sdk-stream` (default): one `claude` process per Penelope session. Penelope writes one JSON line per user message to its stdin and reads `system`, `assistant`, `user`, `stream_event` and `result` lines from its stdout. These are the flags and message shapes the Claude Agent SDKs use (`--input-format stream-json --output-format stream-json --verbose`, message `{"type":"user","message":{"role":"user","content":...}}`), checked against the sources of the Python SDK `claude-agent-sdk` 0.2.189. Penelope is a native Rust client of that protocol, no sidecar.
- `per-turn`: one `claude -p` process per turn, as before. Use it when the stream transport misbehaves. `PENELOPE_CLAUDE_TRANSPORT=per-turn` sets it for one run.

Lines that arrive late (a summary, a result of a background task) are dropped before the next message, and a `result` before the first output of a turn is ignored. If the process sends nothing for `claude.idle_timeout_secs` (default 600), the turn ends with an error and the process is stopped.

A new process starts when the flags change (model, effort, allowed tools, MCP servers, rules) or when the old one ended or crashed. The new process resumes the stored session with `--resume`. Stopping a turn kills the process, and the next turn starts a new one the same way.

## Which usage pool

Both transports start `claude` in print mode, the way the Agent SDKs do. According to the rule the owner cited (since 15 June 2026, `claude -p` and the Agent SDK are the allowed way for third-party harnesses on a Claude subscription, and they draw from the separate Agent SDK credit), a Penelope turn draws from the person's Agent SDK credit, not from the interactive Claude Code limit. Penelope does not read the pool from the account and does not verify this. The usage page of the account shows where a turn was counted.

## API keys

`ANTHROPIC_API_KEY` and `ANTHROPIC_AUTH_TOKEN` are removed from the environment of every `claude` child, so a turn never bills an API key by accident. Config `claude.api_key_mode: true` keeps them.

## What the child does not load

Every `claude` that Penelope starts (turns, scouts, agent steps, the weaker model) goes through one helper, `claudecli::print_command`. It removes the key variables and adds `--setting-sources project,local --disable-slash-commands`. The settings come from `claude.command`, `claude.user_settings` and `claude.api_key_mode`. The cluster parts start `claude` in `clusters.rs`, which this change does not touch.

`--setting-sources project,local` drops the user settings file of Claude Code (`~/.claude/settings.json`). That means:

- user hooks (SessionStart, Stop, PostToolUse of other tools) do not run;
- permission rules from the user file do not apply, among them deny and ask rules for MCP tools. The allow list of Penelope (`--allowedTools`) and the gates of Penelope's MCP server decide instead;
- `apiKeyHelper` and the `env` block of the user file are not read (the child still logs in by itself);
- enabled plugins, their skills and commands, and user-level MCP servers are not loaded (`--strict-mcp-config` already limits MCP servers to Penelope's list).

Project and local settings (`.claude/settings.json`, `.claude/settings.local.json` of the folder) still load. Config `claude.user_settings: true` removes both flags and loads everything again.
The child also gets `CLAUDE_CODE_CLASSIFIER_SUMMARY=0` unless the person set it. Without it the command asks a model for a summary after every turn and holds the result back for about 2 seconds. Penelope does not show that summary.

## Tools, gates and filters

Penelope's own tools run through its MCP server (`penelope mcp`), started by `claude` with the process. `claude` runs in its own process group. Ending a session closes its input, waits about 2 seconds and kills the group, so the MCP servers and the ScreenCodec browser leave with it. The gates, the tool scan, the vet and the privacy filter run in that server and in the chat, as before. In the stream transport the server stays up between turns. Before each tool call it reads the live turn file (`.penelope/gates/live-*.json`, mode 0600): a new `seq` starts a new turn there (counters, lifted rules, the person's text for the anchor rule, the session taint). The person's text never goes on a command line.
Each write of that file carries a signature over a random nonce, `seq`, the text and the taint. The nonce is made per process and handed to the MCP server through its environment, which removes it at the first call. A file without the right signature, or with a `seq` that does not grow, is ignored; the chat ignores a forged taint as well. `.penelope` is a control folder: `write` and `edit` ask first, and a shell redirect into it is not a safe command. The limit: the nonce lives in the environment of the `claude` process, which a command run by the same user could read from `/proc`. Where `run_policy` lets any command run, that command could do anything the user can. The nonce stops the file tools and a plain redirect.
If the file cannot be written, the turn is refused. A changed setting in `config.json` (gates, policies, privacy) starts a new process, because the server reads the config once.
The memory notes, lessons and matching skills of a turn go into the user message between two markers, with the origin named (stored notes, not the person). A marker inside a note is replaced, so a note cannot close the block or fake the person's request. The rules and the skill names stay in the system prompt, which does not change between turns.

## MCP servers per turn

Only servers that have an allowed tool are started. A plugin server with `allow: false` is not started. The ScreenCodec server starts when the text the person typed (not attached files) mentions the screen, a site, a url, a link, a page, a browser, a picture, a click or similar words in English or German, and stays on. If the model calls a screen tool that was not started, the process restarts once with the server and the turn goes on for the rest of the session once a turn used it. Config `screencodec.always: true` starts it every time.

## Timing

`PENELOPE_TIMING=1` writes one line per phase to stderr. `PENELOPE_TIMING=/path/file` appends to a file, also from the TUI and from the MCP server. The hidden command `penelope bench-turns N "text"` runs N turns in one chat.

## Terminal

The weaving animation draws box and block characters. With `tui.ascii: true`, `PENELOPE_ASCII=1` or an East Asian locale it draws plain ASCII. `tui.reduced_motion: true` or `PENELOPE_REDUCED_MOTION=1` stops all motion.
