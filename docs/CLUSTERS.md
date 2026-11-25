# Penelope as a cluster framework

Written on 2026-11-25 from the user's description. Status: the first working layer is built in `cli/src/clusters.rs`
(profiles, parts, reports, one memory file, validator retry, dependencies, parallel execution and the cluster tools).
`PLAN.md` and `ROADMAP.md` still describe the older agent layer; the cluster layer is the target architecture for the
fixed role chain.

## What is built now

- A cluster profile is a JSON file under `.penelope/clusters/profiles/NAME.json`, or one of the built-ins `coding`,
  `backend`, `frontend`, `live-test`, `research` and `solo`.
- A profile is a list of parts. A part has `name`, `role`, `slot`, `task`, `model` and optional `mcp`. `model` may be
  `main`, `weak`, `scout`, `judge`, `worker`, a role name, or a full model setting (for example an OpenCode model).
- The orchestrator asks the router model for a plan of 1–4 clusters and falls back to one `coding` cluster. It can be
  forced with `--profile`.
- Clusters with no `after` start together in threads; a cluster waits for the ids named by `after`. A subcluster waits
  for its parent and its depth is checked against the parent's `max_depth`.
- Every part writes into the cluster's one `memory.md`. The validator reads that memory, runs the project's check where
  it can, and answers `PASS` or `FAIL`. A `FAIL` sends the result back to the workers for up to `retries` rounds.
- A profile with a worker part gets `worktree: true` by default: its cluster works in a branch `agent/<id>` under
  `.penelope/worktrees/`. The patch reaches the live tree only after the validator answered `PASS`; a conflict is
  reported and leaves the worktree for inspection. `kill` removes the worktree and its branch. `research` and
  `live-test` set `worktree: false` explicitly.
- The report file uses the compact line (`S:working|P:1/2|T:12s|F:backend|E:...|V:cargo test ok`). `V` is the check
  the validator ran and its result; `S:done` without a non-empty `V` is not accepted. `cluster_status` shows the tree,
  one cluster, its parts, report and memory.
- Tools: `cluster_start`, `cluster_status`, `cluster_kill`. Commands: `/cluster GOAL`, `/clusters`,
  `penelope cluster run/list/status/dump/profiles/kill`.
- A factory is a copy of a profile with a schedule (`every: "3h"` or `at: "09:30"`) and one purpose. The timer lives
  in the operating system (systemd user timer, launchd, Task Scheduler), so a factory runs while Penelope is closed.
  Commands: `/factory new/list/install/run/stop`, `penelope factory ...`; registry `.penelope/factories.json`.
- Live check: a `solo` cluster with the real Claude command created `hello.txt`, ran `cat`, and the haiku validator
  returned `PASS` with the `cat` evidence. The cluster closed `S:done|P:2/2|T:12s`.

## The idea in one paragraph

Penelope does not start one model that calls other models. Penelope is a cluster framework: an orchestrator model
gets the task, splits it, and creates clusters. A cluster is a group of model workers with one job (for example
"dev" or "search"), its own profile and its own budget. Clusters report to the orchestrator at a set interval.
The orchestrator sees a cluster that hangs, asks it for a compressed state, kills it, and respawns a new cluster
with a prompt that names where the old one got stuck. The models of Laertes are built for roles inside clusters:
Nostos calls tools, Tiresias routes, Argos judges, Aiolos compresses.

## Parts

| Part | What it does |
|---|---|
| Orchestrator | one model the user picks: the Laertes routing model, Claude Sonnet, or another. It holds the plan, creates and kills clusters, reads their reports |
| Cluster | a group of workers with one job, created from a cluster profile. It may create subclusters up to the depth limit |
| Cluster profile | the settings of one cluster, see below |
| Report | the short state a cluster sends to its parent at each interval |
| State dump | the compressed full state a cluster returns when its parent asks or kills it |
| Zombie watch | the check in the orchestrator that finds clusters which repeat themselves or make no progress |

## Cluster profile

| Field | Example | Meaning |
|---|---|---|
| `name` | `dev` | job of the cluster |
| `model` | `nostos-1-mini`, `claude-sonnet` | model of the workers |
| `workers` | 2 | number of parallel workers |
| `reasoning` | `off` | thinking on or off; a dev cluster that only applies a known change needs none |
| `output` | `format-only` | no comments and no free text, only the answer in the agreed format |
| `report_every` | `5m` | interval of the report to the parent, free to set per cluster (30 s for a short job, 5 min for a large one) |
| `max_depth` | 2 | how many levels of subclusters this cluster may create below itself |
| `budget` | `tokens: 200000, minutes: 30` | limit; a cluster over budget stops and returns its state dump |
| `tools` | `read, edit, run` | tools the workers may call; tool calls go through the routing model |
| `stuck_after` | `3 reports` | reports without progress before the orchestrator treats the cluster as stuck |
| `worktree` | `true` | own git worktree and branch; default true when the profile has a worker part |

A profile is a file under `.penelope/clusters/NAME.json`, so the user can keep and reuse profiles.

### Factories

A factory is a profile with a schedule and one purpose. `/factory new NAME --profile P --every 3h` (or
`--at 09:30`) copies P into `.penelope/clusters/profiles/NAME.json`, adds `every`/`at` and `purpose`, and writes an
entry into `.penelope/factories.json`. `/factory install NAME --yes` renders and enables the operating-system timer:
a systemd user service and timer on Linux, a launchd plist on macOS, a Task Scheduler task on Windows. Every timer runs
`penelope cluster run --profile NAME GOAL`, the normal cluster command without the interface. `--dry` prints the units
and writes nothing; `/factory stop NAME` disables the timer and removes its unit files but keeps the profile. The run
count and the last result are in the registry; the cluster state stays under `.penelope/clusters/`.

## Lifecycle

1. The orchestrator gets the task and writes a plan: which clusters, which profiles, which order.
2. It creates the clusters (`cluster_create PROFILE TASK`). A cluster may create subclusters while its depth
   allows it.
3. Every cluster sends a report at its interval: `working`, `done`, `blocked`, with the step it is on and a progress
   counter.
4. The zombie watch compares the reports. A cluster is a zombie when its progress counter stays the same for
   `stuck_after` reports, when it repeats the same tool call with the same arguments, or when it runs over budget.
5. For a zombie the orchestrator asks for the state dump (`cluster_dump ID`), reads it, and kills the cluster
   (`cluster_kill ID`). Killing a cluster kills its subclusters.
6. The orchestrator respawns with a new prompt: the old task plus the cause from the dump ("hung at the TLS
   handshake in src/net/handshake.rs:88, the proxy strips SNI; check that first"). It never respawns with the old
   prompt unchanged.
7. `done` reports flow up to the orchestrator, which merges the results and closes the plan.

## Report and state dump format

Clusters write for machines. No greeting, no full sentences, no paragraphs. One line, fields in a fixed order,
`KEY:value` pairs joined by `|`, values without spaces:

```
S:stuck|P:4/9|T:312s|F:src/net/handshake.rs:88|E:TLSHandshakeTimeoutAfter3Retries|TRIED:Timeout10s+PinRustls0.23|SUSPECT:ProxyStripsSNI
```

| Key | Content |
|---|---|
| `S` | state: working, done, blocked, stuck |
| `P` | progress: steps done / steps planned |
| `T` | time on the current step |
| `F` | file and line, or the resource it works on |
| `E` | the error or the open question |
| `TRIED` | what it tried, joined with `+` |
| `SUSPECT` | its guess of the cause |
| `OUT` | result or diff summary when done |
| `V` | the check the validator ran and its result; required when `S` is `done` |

Measured on 2026-11-25 with the Qwen2.5 tokenizer, tokens before and after removing spaces, tabs, commas and line
breaks:

| Text | Tokens | Without | Saved |
|---|---|---|---|
| one status line, 191 characters | 60 | 55 | 8 % |
| `memory.rs`, Rust, 21 KB | 5,575 | 4,685 | 16.0 % |
| `train.py`, Python, 7.5 KB | 2,178 | 1,754 | 19.5 % |
| error report, Markdown with code, 11 KB | 3,615 | 3,094 | 14.4 % |
| `PLAN.md`, prose, 13 KB | 3,361 | 3,277 | 2.5 % |

Code and diffs lose their indentation, so a state dump with code saves 15 to 20 percent. In prose the space
belongs to the word token (" word"), so removing it saves almost nothing; there the saving comes from leaving out
words. The format therefore fixes the fields, forbids prose and strips whitespace from code parts. The Aiolos
family (compression) is the place to train a denser code later.

## Tools of the orchestrator

| Tool | Arguments |
|---|---|
| `cluster_create` | profile, task, parent (optional) |
| `cluster_status` | id (empty: all clusters as a tree) |
| `cluster_dump` | id |
| `cluster_kill` | id, reason |
| `cluster_respawn` | id, extra prompt |
| `cluster_profile` | name, fields (create or change a profile) |

The routing model Nostos learns these six tools as part of the Penelope tool set, including the decision to kill:
a report with `S:stuck` or an unchanged `P` over `stuck_after` reports leads to `cluster_dump` and then
`cluster_kill`, not to waiting.

## Open points

- A cluster cannot yet create a new subcluster while it runs; the orchestrator writes subclusters into the plan.
- Workers inside one cluster run in order. Parallel workers need a merge rule for overlapping files.
- Cost is recorded per cluster and role, but there is no hard per-cluster stop while a model call is running.
- The zombie watch (`stuck_after`, repeated tool calls) is stored in the profile but not enforced yet.
- The 8 Laertes families are parts with `slot` labels; a family in training blocks its cluster instead of being
  silently replaced. A real plan endpoint is still missing.
