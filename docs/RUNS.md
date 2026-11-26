# Penelope: real runs

Every real run with its numbers, oldest first. The README names the headline numbers only.
The last section holds the 4 runs of the tools added in 0.10.0. The section after it holds the live runs of 0.11.0.

The runs of 2026-11-23 used the old command name `katecli`.

First run (2026-11-23), main model Claude Sonnet, weaker model Claude Haiku. Request: run `cargo tree`, count the
lines, name the TLS crate of ureq, store a note.

- `cargo tree` printed 120 lines. Haiku condensed them, Sonnet answered "rustls v0.23.46" and "120 lines".
- A piped command (`cargo tree | wc -l`) was refused by the safe policy.
- The note landed at the waypoint `rust/kate`.
- Tokens of the main model: 9,070 in, 699 out. A comparison run without condensing has not been made.
- Direct call `penelope run cargo tree`: 6,947 characters of output, 168 characters reached the main model (2 percent).
- `fetch` on https://example.org/ returned the purpose of the page in 3 lines.
- Second run: the sentence "commit messages are English and carry no emojis" was filed at `git/commits` by the weaker model.

Runs inside the full screen interface (2026-11-23, version 0.3.0):

- Request with a rule and a command: the interface showed `memory_note` and `run git log --oneline -3` as they ran,
  then the answer. 9,601 tokens in, 398 out. The weaker model filed the rule at `git/commits` a second time.
- Request with a piped command (`echo kate | tr a-z A-Z`): the question appeared, the answer `y` let it run, the model
  reported `KATE`.

Runs of version 0.4.0 (2026-11-24, command `strix`), in a folder with a file `demo.env` that holds a made-up token:

- Request "Run cat demo.env. Which port and which token does it set?": the model answered port 8080 and reported the
  token as masked. 8,050 tokens in, 266 out. The same request ran a second time with 8,506 in and 180 out.
- Request with a piped command (`echo strix | tr a-z A-Z`): the question appeared, `y` let it run, the model
  reported `STRIX`.

Runs of version 0.5.0 (2026-11-24, command `penelope`), same kind of folder:

- `penelope ask` with a made-up token inside the request: the filter masked it before the model read it. The model
  answered that it sees the text `[masked:secret]` only. 10,257 tokens in, 798 out. `penelope sessions` listed the
  conversation afterwards.
- Request in the interface "Read demo.env with cat": the model named `API_TOKEN` as masked and `NAME=demo`. The side
  panel counted 2 masked items.
- `penelope chat --resume 1` with a piped command (`echo penelope | tr a-z A-Z`): the question appeared, `y` let it
  run, the model reported `PENELOPE` and referred to the earlier turn.

Runs of version 0.6.0 (2026-11-24), main model Claude Sonnet, weaker model Claude Haiku, `delegate` on `auto`:

- Request with three steps: newest version of the crate ureq from the web, the file that prints a word, a comment on
  top of that file. The main model stored a plan, sent two scouts (14 s and 8 s, 4 steps, 12,218 tokens in and 1,151
  out on Haiku), read the file and asked to change it. After `y` the file started with `// newest ureq: 3.5.2`.
  The main model itself used 8,669 tokens in and 1,021 out. The version number was not checked by a person.
- A second session with one question, then `f3`: the first session came back with its whole transcript.
- `penelope scout` from the command line answered a web question and a file question in 15 s with 4 steps.
- A run before the fix sent the scout to the folder above the working directory. The rules now name the working directory.

Runs of version 0.7.0 (2026-11-24):

- `penelope ask` with a piped command: the command was refused outside the interface. After the turn the weaker
  model filed one lesson at `tools/project`. A later request quoted that lesson from its system prompt word for word.
- Request "ask me with ask_user which function to rename": the dialog showed `main` and `helper`, the key `2`
  answered, the model reported `helper`.
- `/scout web` changed the setting and stored it in `config.json`.

Runs of version 0.9.0 (2026-11-24), main model Claude Sonnet, weaker model Claude Haiku, `delegate` on `auto`.
The folder was new and held `words.txt` with 12 words, `demo.env` and `src/main.rs`. The main model ran 8 turns.

The lesson check used this request in `penelope ask`, in separate sessions:
"How many distinct words does words.txt hold? It has one word per line. Use a shell command to count them."

| Run | Lessons in the prompt | Tool calls | Refused calls | Tokens in | Tokens out | Count returned |
|---|---|---|---|---|---|---|
| 1 | 0 | 2 | 2 | 4,812 | 656 | no |
| 2 | 2 | 1 | 1 | 7,528 | 401 | no |

- Run 1 tried `sort -u words.txt | wc -l` and an `awk` line, and the safe policy refused both.
- After run 1 the weaker model filed 2 lessons at `tools/run`. One of them reads "After one denial, report the block
  instead of trying a sibling command."
- Run 2 tried the same `sort` pipe once and then stopped with a report of the refusal.
- Limit: one pair of runs. The lesson saved 1 tool call and 255 output tokens in that pair.
- Limit: no run returned the count of 7 distinct words. The lesson named no command that the safe list accepts.
- Limit: two earlier first runs of the same request used 7,658 and 4,536 tokens in. The 7,528 tokens of run 2 lie
  inside that spread, so the pair shows no effect on input tokens.
- Those two earlier first runs filed no lesson after 2 refused calls each. The lesson log did not exist yet, so the
  answers of the weaker model are lost.
- After run 2 the weaker model wrote the path `tools/Bash`, and the filter for paths dropped the line. Version 0.9.0
  writes such a path in lower case before the check.

Four turns in the full screen interface, one session each:

- Request "Three steps, keep a plan: find the file that sets the port, read it, then change the port to 9090 and add a
  comment line above it with the date of today." The main model stored a plan with 3 steps and sent 1 scout (4 steps,
  13,547 tokens in and 1,052 out on Haiku, 9 s). It read 2 files and asked to change `src/main.rs`. The dialog showed
  line 5 as removed and the new lines 5 and 6 as added. After `y` the file held `let port = 9090;` below a comment
  with the date. The turn took 7 tool calls, 6,792 tokens in and 1,273 out.
- Request "Ask me with ask_user which fruit I want, apple or pear. Then write one example line for a .env file that
  sets API_TOKEN to a made-up value of 24 letters and digits." The dialog showed `apple` and `pear`, the key `2`
  answered. The answer reached the screen and the stored transcript as `API_TOKEN=[masked:secret]`. The count of
  masked items in the side panel went from 4 to 5. The turn took 5,005 tokens in and 157 out.
- Two demo plugins, `atlas` allowed and `ledger` denied, each with 1 MCP server and 1 tool `lookup`. The request named
  both tools. The call `atlas-notes lookup` returned "atlas holds 42 notes". The `claude` command refused the call
  `ledger-notes lookup` with "Claude requested permissions to use mcp__ledger-notes__lookup". The turn took 2 tool
  calls, 10,765 tokens in and 172 out.
- Request with a piped command (`echo penelope | tr a-z A-Z`): the question appeared, `y` let it run, the model
  reported `PENELOPE`. The turn took 7,177 tokens in and 82 out.

Runs of version 0.10.0 (2026-11-24), main model Claude Sonnet through the `claude` command, 4 turns. The folder was new
and held `src/config.txt` with 7 lines, `src/lib.rs` with 6 lines and `log.txt` with 2 lines. `delegate` was `off` and
`auto_learn` was `false`, so each turn started 1 `claude` process.

| Turn | Where | Tool calls | Tokens in | Tokens out | Result |
|---|---|---|---|---|---|
| `grep` with `regex: true` | `penelope ask` | 1 | 10,988 | 166 | 3 matching lines, the functions `alpha`, `beta` and `gamma` |
| `job_start`, `job_read`, `job_stop` | `penelope ask` | 3 | 4,832 | 244 | job 1 ran `tail -f log.txt`, 2 lines read, "job 1 stopped after 2 s" |
| `edit` with 3 replacements | interface | 2 | 5,094 | 328 | 1 question with 1 diff of 6 lines, after `y` the lines 1, 3 and 6 were changed |
| `undo` | interface, same session | 1 | 556 | 249 | `src/config.txt` had its 7 old lines again |

- The pattern was `^(pub(\(crate\))? )?fn \w+`. The file also held `let fn_like = 3;` and a comment with `fn`, and
  neither line matched.
- After `job_stop` the command `penelope jobs` printed "1 jobs, 0 running". No `tail` process was left.
- The dialog showed the lines 1, 3 and 6 as removed and as added. It left out the 3 equal lines between them.
- The command `/undo` after the `undo` turn answered "nothing to undo".
- Limit: each of the 4 tools has 1 real turn. The answer text of `undo` was shortened after its turn.
- Limit: no run used a provider key, because no key was set on the test PC. The kinds `openai` and `anthropic` have
  no real call.

One run without a model (2026-11-24): a stand-in script for chat completions listened on port 18765 of the test PC.
It answered `penelope ask` and then `penelope chat --resume 1 --plain` from a second process. The second request
carried 4 messages, the first request and its answer among them.

## Live runs of version 0.11.0 (2026-11-24)

Provider: OpenCode Go, `https://opencode.ai/zen/go/v1`, model `mimo-v2.6-flash`, wire format `openai`. The key lived
only in `OPENCODE_API_KEY`; `config.json` stored the variable name. The settings folder was `/tmp/penelope-live`.

| Run | Tool calls | Tokens in | Tokens out | Result |
|---|---|---|---|---|
| `ask "Answer with one word: pong"` | 0 | 3,565 | 5 | `pong` |
| `ask "Use the tool run to execute echo penelope-live"` | 1 | 7,032 | 61 | exact output `penelope-live`, exit 0 |
| `orchestrate '$reader(role=judge,goal="Read the README and say in one sentence what Penelope is")'` | 1 | 9,008 | 105 | `reader.check` read the README and answered in one sentence |
| `ask` with ScreenCodec: open a local page, snapshot, click `button1`, snapshot | 3 | 24,814 | 270 | counter 0 → 1, 34 s |

ScreenCodec 0.8.1 ran from a temporary venv (`/tmp/screencodec-venv`):

- `screencodec-mcp doctor` passed Chromium, Tesseract, OpenCV, a test page, the click check, a real window capture
  (837×668, 1.2 s) and a monitor capture (3440×1440, 8.9 s).
- `penelope screen demo http://127.0.0.1:8765/` read 122 lines of hardened SDL text in one process; open and snapshot
  in one process is what keeps the page.
- `screen desktop` with `screencodec.desktop = "on"` returned the active window as 44 text elements in 1.2 s. No
  screenshot was stored. The setting went back to `off` and the captured text was deleted.
- `penelope cursor 100 100` moved the X11 pointer to `X=100 Y=100`, checked with `xdotool getmouselocation`.

Limits of these runs: one provider, one small model, one PC. The desktop capture showed private content and is not
part of the repository. `desktop_click` was not tested, because this session has no ydotool with a running ydotoold.

## Live runs of the second 0.11.0 pass (2026-11-24)

| Run | Result |
|---|---|
| `penelope login groq` with a dummy key, then `penelope auth`, then `penelope logout groq` | key stored in the Secret Service, status said `groq stored`, logout removed it, lookup empty. The test file had mode 600 and held no key. |
| `penelope plugin import` in a fresh folder | 4 installed Claude plugins and 20+ skills from `~/.claude/skills` and `~/.agents/skills` were registered, each with its MCP tools denied. |
| `ask "Use the tool computer ... report the counter"` | lead made 1 tool call; the tool role opened the page and reported 0; 10,929 tokens in, 117 out, 25 s. |
| `ask` with five explicit `run` calls | 5 tool calls, `done`, auto-skill `call-tool-run-exactly` written with clean Markdown; 33,523 tokens in, 182 out, 18 s. |
| `ask "From now on, always run the test suite ..."` | weak model filed `rust/tests`, consolidation added `rules/rust-notes`; 11,928 tokens in, 908 out. |
| `screen desktop-click "Increase the counter"` with the uinput helper | counter 0 → 1, then 1 → 2; the native cursor moved; `desktop` was set back to `ask`. |
| `penelope audit` on the live folder | 20 text files, 0 findings; redirects off, no listener. |
| `penelope mode` and `penelope update --check` | six modes listed; updater said no server is set and named `docs/AUTOUPDATE.md`. |

Limits: the desktop click used a `/dev/uinput` helper because this Wayland session has no ydotool; the helper is
installed by `penelope input install` and only enabled by a person. The Windows branches were not run.

## Web search of the second pass (2026-11-24)

| Run | Result |
|---|---|
| `penelope search "rust tokio select!"` | 8 results from DuckDuckGo in 0.7 s, ranked, with engine and score |
| `penelope search "Rust 2026 release notes" --fresh` after DuckDuckGo refused | Bing answered; the `bing.com/ck/a` tracking links were decoded to Wikipedia and rust-lang pages |
| `penelope research "What is the tokio select macro and when should I use it?" --depth standard --pages 3` | 2 queries, 3 pages, DuckDuckGo, 10.3 s; cited answer with 10 sources on `mimo-v2.6-flash` |
| `penelope research` on an old folder with `search.kind` only | the deep config merge added the new search fields without touching the old value |

The research answer cited the docs.rs page, a tutorial and a blog, named the missing alternatives (`join!`,
`FuturesUnordered`) and listed the pages that were only snippets. The search cache lives under
`.penelope/search/cache/`; `penelope audit` found no key in it.

## Eval runner (2026-11-25)

`penelope eval TASKS.jsonl --setup large|large-subagents|cluster|all` writes its measured table here. The schema, the
three setups and the replay guarantee are in [`EVAL.md`](EVAL.md).

**No real eval run is in this file yet.** The runner, the table and `penelope replay ID --model MODEL` were tested
against the local fake endpoint of the test suite only. A fake-provider table is never written to this repository
file. The first real row will name the task file, the models, the passed checks, the tokens, the euro and the wall
time of each setup; it comes from `penelope eval`, not from a hand-written estimate.

What is already fixed before a real run:

- A task file holds `id`, `prompt`, `setup.repo` (with optional `cwd` and `copy`) and `check`.
- `large` is the main model through `backend::Chat`; `large-subagents` is `agents::run` with one worker and one judge;
  `cluster` is `clusters::orchestrate` with the weak model in the `eval-small` profile.
- Passed means the task's check command exited 0 in the task folder.
- Tokens and euro are the delta of `.penelope/usage.json` and `cost.rs` during the task. An unpriced model shows 0.0000
  and is not guessed.
- Replay offers only read-only tools and refuses `write`, `edit`, `run`, jobs, screen, MCP and clusters by name.

## Eval run 2026-11-26 17:39 · large, large-subagents, cluster

`penelope eval cli/tests/data/eval/tasks50.jsonl --setup all`

Tasks: `cli/tests/data/eval/tasks50.jsonl`. Models: large = claude-sonnet-5-5, large-subagents = claude-sonnet-5-5, cluster = haiku.

| setup | tasks | passed | tokens in | tokens out | euro | wall | run id |
|---|---|---|---|---|---|---|---|
| large | 50 | 35 | 286429 | 31026 | 1.2187 | 446.8s | `large-1795711750811` |
| large-subagents | 50 | 39 | 1599244 | 131051 | 6.2224 | 1637.6s | `large-subagents-1795712197658` |
| cluster | 50 | 39 | 800191 | 118906 | 1.0265 | 905.1s | `cluster-1795713835239` |

Passed means the task's own check command exited 0 in the task folder. Tokens and euro are the delta of `.penelope/usage.json`
during the task; euro uses the prices in `config.json` and marks an unpriced model as 0.0000 rather than guessing.

Setup details:

- Penelope commit `4389032` (main after PR 31), release build, eval root `/tmp/opencode/evalroot`.
- The eval context sets `edit_policy: all` and `auto_learn: false`; `run_policy` stays at the shipped default `safe`.
- `large`: `backend::Chat`, model `claude-sonnet-5-5`, no profile, one model with the Penelope tools.
- `large-subagents`: `agents::run`, model `claude-sonnet-5-5` for lead, worker and judge, no cluster profile.
- `cluster`: `clusters::orchestrate`, profile `eval-small`, worker slot `small` and validator slot `small-check`, both `haiku`.
  The profile has one worker, one validator, `max_steps: 8`, `retries: 0`, `worktree: false`.
- All 50 tasks ran once per setup. The sum of the per-task wall times is the wall column.
