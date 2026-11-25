# Penelope as a cluster framework

Written on 2026-11-25 from the user's description. Status: plan, nothing of it is built. `PLAN.md` and
`ROADMAP.md` describe the harness as it stands (roles, agents, delegate); this file describes the target architecture
that replaces the fixed role chain.

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

A profile is a file under `.penelope/clusters/NAME.json`, so the user can keep and reuse profiles.

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

- Which models run inside clusters in the first version: Claude through the `claude` CLI, local Qwen models, or both.
- Whether a cluster's workers share one context or each get their own.
- How the orchestrator merges results of parallel dev clusters that touch the same files.
