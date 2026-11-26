# Penelope plan: remaining tools and the orchestration of subagents

Written on 2026-11-24 for version 0.9.0, updated for 0.11.0. `ROADMAP.md` lists what is built and what is open. This
file says how the open parts fit together, in which order they come, and how each one is measured. Version 0.11.0 built
the contract layer, the eight roles, agents, the shared memory, the brain, the cost view, ScreenCodec and the cursor.

> The current gap audit and the 0.11.1–1.0 sequence are in [`PLAN-0.12.md`](PLAN-0.12.md). This file keeps the
> architecture and the older 0.13–0.15 outline.

## Where the harness stands

| Part | State in 0.11.0 |
|---|---|
| Tools | 54 built-in tools and tools a project writes itself (`tool_new`). `spec.rs` is the only table; `route::dispatch` checks role and schema |
| Subagents | 8 roles: lead, scout, worker, judge, tool, condenser, filter, router. An agent written with `$name(...)` runs a plan of role calls; the lead can delegate with the tool `agent` |
| Who decides | the role gate, the fixed `delegate`, `run_policy` and `edit_policy`; a router model may choose the plan, a privacy model may redact between models |
| Learning | `learn`, `learn_tools`, consolidation of rules and complaints into the shared memory, and `mark_used` for paths that worked |
| Cost view | characters kept away, scout tokens, and euro per role and model from `.penelope/usage.json` |
| Local server | `penelope serve`: search, read, embed, rerank and redact, with a token and a rate limit. `/whoami` verifies a client token. See [`SERVER.md`](SERVER.md) |
| Plugin and MCP management | plugin add/install/update/remove/allow/deny; standalone MCP servers with add/remove/allow/list/call |
| Login verification | `penelope auth verify [main|weak|scout]` asks the provider for its model list; Claude uses its auth status |

Version 0.9.0 had three gaps. A change could not be taken back, and a long command blocked the turn. The tool `undo`
and the 3 job tools of 0.10.0 close these 2 gaps. The third gap is open: only one helper runs at a time, and it can only read.

## Part 1: the remaining tools (0.11.0 built most of this)

Each tool is small enough for one version. The column "asks" names the question the person sees.

| Tool | What it does | Who may call it | Asks | Version |
|---|---|---|---|---|
| `undo` | Restores the file as it was before the last `write` or `edit`. Every change first copies the old file to `.penelope/undo/N/`. | main model, command `penelope undo` | no, it only goes back | 0.10, built |
| `unzip` | Unpacks a ZIP below the working folder: zip-slip, symlinks, 10,000 entries, 50 MB per file and 200 MB total are refused. The same extractor installs a plugin ZIP. | main model, worker | "Unpack FILE into DIR?" with the file list | 0.11, built |
| `edit` with several places | One call carries a list of replacements for one file. One question shows all of them as one diff. | main model, worker | "Change this file?" once | 0.10, built |
| `grep` with patterns | Regular expressions through the crate `regex`, switched on by a field `regex: true`. Plain text stays the default. | main model, scout, worker | no | 0.10, built |
| `job_start`, `job_read`, `job_stop` | Starts a command in the background, reads its new output, stops it. For servers, watchers and long builds. Output passes the privacy filter. | main model, worker | like `run`: outside the safe list it asks | 0.10, built |
| `memory_forget` | Takes back one note by its text. The command `memory unravel` exists, the tool does not. | main model | "Forget this note?" | 0.11, built |
| `git_commit` | Stages named files and commits with a message. Never pushes. Identity comes from the repository, no extra trailer. | main model | "Commit these N files?" with the file list | 0.11, built |
| `scouts` | Several lookups at once, see part 2. | main model | no | 0.11, built |
| `check` | Hands a claim and its evidence to the judge, see part 2. | main model | no | 0.11, built |
| `work` | Hands a change to a worker in its own worktree, see part 2. | main model | "Apply this change?" with the diff | 0.13 |
| bridge tools | A bridge to a private research prototype: hand it a document, ask it a question. | main model | before every step that needs minutes on the GPU | 0.15 |
| `train_plan`, `train_start`, `train_watch` | Proteus, first step: writes a training configuration, starts a run on this PC, reads the loss and the test score. | main model | before every start, with model, data, steps and the time estimate | 0.15 |

Not planned as tools: a browser of its own (ScreenCodec covers pages and pictures), a tool that pushes to a remote,
a tool that installs system packages.

### Rules every new tool follows

1. The output a model reads passes `guard_count`.
2. A tool that changes something outside `.penelope` asks first, unless a policy setting says otherwise.
3. The first line of a result says what happened in under 160 characters. The interface shows that line.
4. A refusal starts with `NOT` and names the reason and the way out.
5. Every tool has one unit test without a model call and one real run that is written into the README with its numbers.

## Part 2: orchestration of subagents

### Roles

A role is a process with its own model, its own tool list and its own budget. The names follow the model families,
because each role is the place where that family will work later.

| Role | Family | Job | Tools | Model today | May change files |
|---|---|---|---|---|---|
| lead | Telemach | talks to the person, plans, decides | all tools of the main model | setting `main` | yes, after a yes |
| scout | Nostos | finds things: web, files, commands that read | the 10 read-only tools | setting `scout`, else `weak` | no |
| worker | Telemach | makes one change in a copy of the project | read tools, `write`, `edit`, `run`, `job_*` inside its worktree | setting `worker`, else `main` | only in its worktree |
| judge | Argos | says whether a claim holds, with one sentence of reason | `read`, `grep`, `run` from the safe list | setting `judge`, else `weak` | no |
| condenser | Aiolos | shortens long output | none | setting `weak` | no |
| filter | Utis | masks secrets and personal data | none | fixed rules | no |
| router | Tiresias | picks the role and the model for a step | none | fixed rules | no |

### How the roles work together

Four patterns, each built on the one before it.

| Pattern | Flow | Version | Measured by |
|---|---|---|---|
| A. One scout | lead sends 1 task, waits, reads 15 lines | built in 0.6 | tokens on the weaker model against tokens of the lead |
| B. Scouts side by side | lead sends 2 to 4 tasks in one call, the harness runs them at once and returns the reports in order | 0.11 | wall time against the same tasks in a row |
| C. Check after work | after a change the harness runs the project's test command, the judge reads claim and output and answers pass or fail | 0.12 | share of wrong "done" claims the judge catches on a set of 30 tasks |
| D. Worker in a worktree | lead writes a brief, a worker changes files in `.penelope/worktrees/ID`, the judge checks, the person sees one diff and says yes or no | 0.13 | solved tasks and lead tokens against the lead doing the change itself |

Pattern C and D need a set of tasks with tests. That set is experiment P1 of the Telemach brief: 30 small tasks in
two of the own projects. Without it the patterns can be built and cannot be judged.

### Mechanics

- Process: every subagent is one child process, started like the scout today. With the `claude` command it gets its
  own MCP configuration and allow list. With an endpoint in the format of OpenAI it runs the tool loop inside the harness.
- Environment: `PENELOPE_ROLE=scout|worker|judge` replaces `PENELOPE_SCOUT`. The MCP server offers the tool list of that role.
- Depth: a subagent starts no subagent. The check sits in the MCP server and in the tool loop.
- Budget per task: a limit for steps, for tokens and for seconds, set in `config.json` under `roles.NAME`. Defaults:
  scout 12 steps and 120 s, judge 6 steps and 60 s, worker 40 steps and 600 s. A task that hits a limit returns what it
  has and says which limit it hit.
- At once: at most 4 children, set by `roles.parallel`. The `claude` command and the endpoint may have their own limits.
- Stop: `esc` stops the lead and every child of the turn. Each child's process id is stored in `.penelope/agents/ID.json`.
- Report: fixed shape for every role. Line 1 is the answer. Then facts with file paths and line numbers or addresses.
  Then what was not found or not done. At most 15 lines for a scout, 8 for a judge, 20 for a worker.
- Questions: a child cannot ask the person, except the worker for a command outside the safe list. The dialog names the role.
- Cost: `stats.json` gets one entry per role with tasks, steps, tokens in and out, seconds.
- Records: every task is written to `.penelope/agents/ID.json` with role, brief, report, steps, tokens, time and the
  tool calls. The lessons step reads these files too, so failed calls of a scout become lessons.

### The worker and its worktree

1. The lead calls `work` with a brief: the goal, the files or the area, and what done looks like.
2. In a git repository the harness creates `git worktree add .penelope/worktrees/ID` from the current commit. Without
   git it copies the project folder, leaving out the folders that `find` skips.
3. The worker runs with `edit_policy` set to `all` for that folder only. Paths outside it are refused.
4. The worker ends with a report and the harness computes the diff against the start.
5. The judge reads the brief, the diff and the output of the test command. Fail sends the report back to the worker once.
6. The person sees the dialog "Apply this change?" with the diff, the judge's sentence and the test result.
7. Yes applies the diff to the project and removes the worktree. No keeps it for `penelope work show ID`.

### The interface

- Side panel: a section AGENTS with one row per running child: role, first words of the task, steps, seconds.
- `/agents` opens a list of the last 50 tasks. Enter shows brief, report and tool calls of one task.
- The transcript shows one line per subagent task, as today for the scout, with the cost in the second line.
- The command `penelope agents` prints the same list outside the interface.

### Who decides what runs where

The setting `delegate` stays the switch for searches. Two more settings follow the same shape.

| Setting | Values | Default | Meaning |
|---|---|---|---|
| `delegate` | `auto`, `web`, `offer`, `off` | `auto` | who searches (built) |
| `verify` | `auto`, `offer`, `off` | `offer` | whether the judge checks after a change |
| `work` | `offer`, `off` | `off` | whether the lead may hand a change to a worker |

`work` starts as `off`, because a worker spends the tokens of a second strong model. The setup gets one question per setting.

## Order and done criteria

| Version | Content | Done when |
|---|---|---|
| 0.10, built | `undo`, several places in `edit`, patterns in `grep`, background jobs | 4 unit tests pass and each tool has 1 real run in [`RUNS.md`](RUNS.md). Version 0.10.0 also added the provider table and the kind `anthropic`, see [`PROVIDERS.md`](PROVIDERS.md). |
| 0.11 | roles in the settings, `PENELOPE_ROLE`, budgets, records in `.penelope/agents/`, scouts side by side, section AGENTS, `memory_forget`, `git_commit` | 3 scouts with 3 tasks finish in less wall time than the same 3 in a row, both times written down |
| 0.12 | judge, tool `check`, setting `verify`, lessons from the records of subagents | the 30 tasks of P1 exist and the judge's verdicts on them are compared with the tests |
| 0.13 | worker in a worktree, dialog "Apply this change?", command `penelope work` | the same 30 tasks solved once by the lead and once through workers, with solved count and lead tokens for both |
| 0.14 | all roles through an HTTP endpoint, MCP client for that path, one scout on a local model (sessions for that path came in 0.10.0) | 1 real scout task on a local model with its time and its answer checked by hand |
| 0.15 | `train_*` for Qwen2.5-0.5B on this PC, bridge tools for a private research prototype | 1 training run started, watched and scored from the harness, with a yes before the start |

Later, without a version: scheduled tasks, bots, the model browser, several sessions that run at once.

## What can go wrong

| Risk | What the plan does about it |
|---|---|
| Subagents cost more than they save | every pattern has a number to beat. A pattern that loses stays switched off by default |
| A worker changes the wrong files | it works in a worktree and the person approves one diff |
| A scout reports something false | reports name paths and lines. Pattern C lets the judge check a claim that matters |
| Children run forever | budgets for steps, tokens and seconds, and one key that stops them all |
| A lesson from a subagent is wrong | lessons stay in the journal, `penelope memory unravel` takes one back |
| The `claude` command changes its flags | every flag in use is listed in `backend.rs` and `scout.rs`. `penelope doctor` gets a check that calls it once |

## Open decisions for the owner

1. Which model works as worker: the same as the lead, or a cheaper one with the judge behind it?
2. May a worker run commands outside the safe list inside its worktree without a question?
3. Should the judge run after every change (`verify: auto`) once the 30 tasks show its hit rate?
4. Which two projects supply the 30 tasks of P1?
5. Should `train_start` be able to reach a rented GPU server, or stay on this PC until a budget is set?
