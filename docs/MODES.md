# Work modes

Penelope has four work modes: `chat`, `work`, `agents` and `code`. A mode sets the tools, a part of the system prompt, the default model, the effort and the cost route of one chat. The modes can give tasks to each other.

A work mode is not the policy preset of `/mode auto|manual|safe|review|privacy|power`. The preset sets the gates for commands and file changes. The work mode sets what the model can reach. Both apply, and a work mode can only narrow the tools, never widen what the policy allows. `/mode` takes both kinds of name.

## The four modes

| Mode | For | Tools | Model and effort by default |
|---|---|---|---|
| `chat` | a plain conversation | `memory_find`, `ask_user` | the weaker model (`weak`), effort `low`, route `fixed` |
| `work` | documents, files, web search, deep research, attachments | file tools (`read`, `write`, `edit`, `unzip`, `undo`, `find`, `grep`, `tree`, `outline`, `json_query`), `web_search`, `fetch`, `research`, `deep_research`, `http_request`, `attachment_read`, `scout`, `scouts`, `run`, `run_detail`, `plan`, the memory tools, `skill`, `clipboard`, MCP tools, `hand_off` | the main model |
| `agents` | clusters, factories and workflows | the `cluster_*` tools, `agent`, `scout`, `scouts`, read-only file tools, `attachment_read`, the memory tools, `plan`, `ask_user`, `hand_off` | the main model |
| `code` | a repository | every tool, including `edit`, `git_commit`, `pr_create`, `check`, `verify_fix` and `hand_off` | the main model, and the cost mode of `route` |

`work` runs commands through the same gates as before: a command outside the safe list asks, and outside text from the web gates risky calls (see `KERKIS-GATES.md`). `agents` asks for every open limit and a final yes before it creates a cluster (see `QUESTIONS.md` and `CLUSTERS.md`). `chat` has no tool that changes anything.

The tool list also follows the setting `delegate`: with `auto` the main model hands searches to the scout, so `web_search` and `fetch` stay hidden from it even in `work`.

## Switching

| Where | How |
|---|---|
| Interface | `/mode chat\|work\|agents\|code`, or Shift+Tab to go to the next mode in the order chat, work, agents, code |
| Footer | `code · sonnet · high`: mode, model, effort |
| Side panel | the row `mode` shows the work mode and the policy preset |
| Command line | `penelope ask --mode code "TEXT"`, `penelope chat --mode work`, also with `--plain` |
| Several commands | `/mode code;;/check` runs both in order |

A switch keeps the session. The transcript gets a line `mode work · sonnet · high`, and a stored session opens in the mode of its last line. A new interface starts in `modes.default` (`code`), or in the mode of `--mode`. `penelope ask` without `--mode` runs without a work mode, as before.

Alt+E and `/effort` change the effort of the mode you are in. A mode that sets its own effort (`chat` does) keeps the level in `modes.NAME.effort`. Ctrl+E stays end of line.

## Settings

```json
"modes": {
  "default": "code",
  "chat":   {"model": "weak", "effort": "low", "route": "fixed"},
  "work":   {"model": "", "effort": "", "route": ""},
  "agents": {"model": "", "effort": "", "route": ""},
  "code":   {"model": "", "effort": "", "route": ""}
}
```

| Key | Values | Effect |
|---|---|---|
| `modes.default` | `chat`, `work`, `agents`, `code` | the mode a new interface starts in |
| `modes.NAME.model` | `""` or `main` keeps the main model; `weak` takes the weaker model; any other word is a model name on the provider of `main`; an object is a full model setting (`kind`, `base_url`, `model`, `api_key_env`, `effort`) | the model of the mode |
| `modes.NAME.effort` | `""` keeps the effort of the model; a level such as `low`, `medium`, `high`, `max` | the effort of the mode |
| `modes.NAME.route` | `""` keeps `route`; `fixed` or `auto` | the cost mode of the mode (see `ROUTING.md`) |
| `modes.NAME.tools` | a list of tool names, `*` for every tool, `-name` to take one away | replaces the default tool list. `["*", "-computer", "-screen"]` is code without the desktop tools. Tools Penelope wrote for the project are offered only with `*`. |

A settings screen for these keys is planned. Until then edit `config.json`.

## Hand-offs between modes

The tool `hand_off(to_mode, task, context?)` gives a task to another mode.

1. Penelope opens a sub-session in the target mode on its own model, effort and tools.
2. The sub-session gets the task and the short `context` the caller wrote (at most 1,500 characters). It does not get the history of the caller.
3. For `agents` the task goes through the cluster runner. It asks the person for every open limit and for the yes, as `/cluster` does.
4. The full result is stored as a Mitos attachment (see `MITOS.md`). The caller receives one summary line and the attachment card, and reads ranges with `attachment_read`.

All modes share the memory graph, the notes and the attachment store of the folder.

Links are kept in `.penelope/handoffs.json`. One entry holds the mode and the session id on both sides, the depth and the task. A link is found from either session. `/mode` lists the hand-offs of the open session.

Guards:

- A hand-off is a risky action of the class `delegate` when the turn holds outside text (level 0, such as a web page). The person must say yes. The sub-session starts with the same outside text, so its own calls are gated, and its result is stored at level 0.
- The depth limit is 2. A sub-session at depth 2 gets the refusal `NOT handed off. Hand-offs are 2 deep already`. A mode cannot hand off to itself.
- The transcript shows each hand-off as a tool line with the target mode and the task. Esc stops the turn and the sub-session with it.
- `chat` has no `hand_off` tool by default, because its tool set is two tools. Add `"hand_off"` to `modes.chat.tools` to allow it.

## The person hands off

`/send MODE TASK` hands a task to another mode from the interface:

```
/send code "fix the failing test in parser.rs"
```

Penelope builds the context from the last two exchanges of the transcript, runs the sub-session in the background and shows the result and the card. The interface then switches to the target mode. `/back` returns to the mode before `/send`. The next request there carries the result and the card, and the transcript keeps only your words.

## Lines with several commands

Commands are split at `;;` or a new line, and run in order. A command that starts background work (`/cluster GOAL`) or opens a dialog (a clarify question, `ask_user`, an approval) holds back the rest of the line until the work ends and no dialog is open. `/cluster GOAL;;/graph` opens the graph after the cluster command is done. Esc drops the rest of the line.

## Not done yet

- The settings screen for `modes`.
- The `/workflow` command (the `agents` mode will use it when it exists).
- A hand-off from the MCP server of a `claude` turn cannot show a live view of the sub-session, only the result.
