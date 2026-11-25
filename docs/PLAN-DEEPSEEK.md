# Plan for the coding agent: everything that is still missing in Penelope

Written on 2026-11-25 from a talk with the owner and from a read of the code at commit `10bac25`
(version 0.11.0, 229 tests, 53 files in `cli/src/`). The reader is a coding agent such as DeepSeek in OpenCode.
Bug numbers such as B33 point into `docs/FEATURES-BUGS.md`. Block numbers point into `docs/CLUSTER-IDEAS.md`.

## How to work

- One package below is one branch. Name it `feat/NAME`. Do not commit on `main`.
- Run `cargo test` in `cli/` before every commit. A commit with a failing test is not allowed.
- Do not push. The local `main` holds commits that are not public. The owner decides when a branch is pushed.
- Do not add a crate without asking. The crate has few dependencies on purpose.
- Every new command gets a line in the help text and one test that calls it.
- Write the state into `docs/DEVLOG.md` after each package: what runs, what was measured, what is open.
- Say plainly when something does not work. A report "done" names the command that proved it and its result.

## Fixed rules of the owner

1. No data of a user is used to train a model of Laertes. Runs, prompts, memory files and reports stay on the machine
   of the user. Training data comes from the owner's own runs with open models (Qwen, DeepSeek).
2. Every cluster reports back to the orchestrator. The orchestrator gives every instruction, so that two clusters do
   not undo the work of each other.
3. Penelope stays free. The paid plan is one key for several models. Other providers always stay usable.
4. The picture of the system is a machine of small parts in place of one large model. The orchestrator builds the
   clusters a project needs: for example research, dev and marketing at the same time.
5. Everything in the harness is modular and can be changed by a mod that Penelope writes itself.

## Words

| Word | Meaning |
|---|---|
| Cluster | a group of parts (agents) with one job, one memory file and one report line |
| Part | one model call with a role, a task and the tools of that role |
| Plugin | adds commands, skills and MCP servers |
| Skill | a playbook in `SKILL.md` that a model follows |
| Mod | changes the harness itself: theme, background picture, layout, key bindings |
| Factory | a cluster with a timer and one purpose, for example "check my mail every 3 hours" |
| App | a small program inside the harness with its own surface, installed from GitHub |
| Brief | one MCP tool call that carries a task and attachments: pictures, files, context files, links |

## Phase 0: before anything else

### 0.1 Merge the three modules of the branch `feat/cluster-building-blocks` — done
- State: `proof.rs`, `notes.rs` and `advise.rs` are on `main` with their 27 tests; the three `mod` lines are kept.
- Result: 165 tests, 0 failed. Evidence: commit `b24e518` and `cargo test`.

### 0.2 Find the test that failed once (B03) — done
- State: one old run showed 105 of 109; the failing test was never identified.
- Result: twenty complete `cargo test` runs passed, 20 of 20. B03 is downgraded to low; a nightly job is still open.
- Evidence: `docs/DEVLOG.md`, 2026-11-25.

## Phase 1: the cluster core

These packages decide whether the idea can be shown. Do them in this order.

### 1.1 One git worktree per cluster (B01, B33, block 2) — done
- Result: `b57dacf`. Worktree and branch per writing cluster, cwd injection, merge only after PASS, conflict reported, kill removes both. Live solo cluster wrote `result.txt` in its worktree and the live tree received it after PASS.
- State: a coding cluster writes into the live folder. Two clusters can overwrite each other.
- Build: `new_cluster()` creates a worktree and a branch for every cluster that may write. `kill()` removes both.
  `run_cluster()` merges into the folder of the user only after the validator said `PASS`. A merge conflict is
  reported to the orchestrator and never solved silently.
- Done when: a test starts two clusters that edit the same file, kills one, and the file holds only the other edit.

### 1.2 The evidence field `V:` (block 1) — done
- Result: `eedf32a`. `report()` uses `proof::render`; `validate()` fills V from real checks; `S:done` without V is refused. Live report: `S:done|P:2/2|T:16s|F:end|E:-|V:cat result.txt ... ok`.
- State: `report()` in `clusters.rs` writes `S:|P:|T:|F:|E:`. `proof.rs` can parse and check `V:`, nothing uses it.
- Build: `validate()` writes the command it ran and its result into `V:`. The orchestrator calls `proof::accept` and
  treats `S:done` without `V:` as a failure of the cluster.
- Done when: a cluster whose validator ran no check ends as failed, and the `solo` cluster ends with `V:cat hello.txt ok`.

### 1.3 Stop a running model call, hard budget (B12) — done
- Result: `8c8bb10`, PR #6. Cancel flag reaches the HTTP loop; Claude child killed as a tree; cluster and session euro budgets block without merge. Open: abandoned socket until provider timeout; Claude kill path not live-tested; session budget cluster-only.
- State: `kill()` sets a flag that is read between two parts. `budget_eur` is checked between two parts.
- Build: keep the child process or the HTTP request of every part and end it on kill. Check the budget while tokens
  arrive. Add a budget for the whole session.
- Done when: `cluster kill ID` ends a part within 2 seconds, and a cluster with a budget of 0.01 EUR stops in the call
  that crosses it.

### 1.4 Zombie watch, dump and respawn — done
- Result: `c641742`, PR #6. report_every/stuck_after, equal P/F or calls, dump, respawn (max 2), cluster_dump/respawn/profile tools. Open: heuristic can kill a long working part.
- State: missing. `report_every` and `stuck_after` are named in `CLUSTERS.md` and are not in the code. The tools
  `cluster_dump`, `cluster_respawn` and `cluster_profile` do not exist.
- Build: a profile gets `report_every` (seconds) and `stuck_after` (number of reports). The orchestrator marks a
  cluster as stuck when that many reports show the same `P:` and `F:`. It then asks for a dump, kills the cluster and
  starts it again. The new prompt names the cause from the dump. A respawn without a cause is not allowed. Limit: 2
  respawns per cluster.
- Done when: a test cluster that repeats one report three times is killed and started again, and its second prompt
  contains the text of the dump.

### 1.5 Parts of one cluster in parallel (B11) — done
- Result: `ae828ff`, PR #6. max_parallel 1–8 and needs dependencies, serialized memory/state, validator after the wave. Open: batch waves are not refilled; parallel parts see pre-wave memory.
- State: clusters run in threads, the parts of one cluster run in order. `max_parallel` is stored and unused.
- Build: parts without a dependency run at the same time up to `max_parallel`. Writes to `memory.md` stay serialized.
- Done when: a cluster with 3 coding parts and `max_parallel=3` takes less wall time than with `max_parallel=1` in a
  test with a fake model that sleeps 1 second.

### 1.6 Subclusters while running, with limits — done
- Result: `5a7fd53`, PR #8. `cluster_spawn` for worker/lead, root depth and tree-wide max_models, parent memory/report stream. Open: parent waits, child worktree from live project, duplicate names newest.
- State: depth is checked when a plan is created. A running part cannot start a subcluster.
- Build: a tool `cluster_spawn` for parts. It checks `max_depth` and `max_models` of the root profile. A subcluster of
  a subcluster is allowed up to the depth. Every subcluster reports to the orchestrator through its parent.
- Done when: a part at depth 3 of 3 gets a refusal with the reason, and a part at depth 1 of 3 gets a running subcluster.

### 1.7 A link between two clusters — done
- Result: `a6ff5aa`, PR #8. `cluster_ask`, question/answer in both memories, ASK lines in reports, no model call. Open: 6,000 char cap, running target may return only a state line.
- State: missing. Only `after` orders clusters.
- Build: a tool `cluster_ask(target, question)`. The orchestrator sees every question and every answer in the report
  stream. Example of the owner: the validator of the dev cluster rejects a design, the dev cluster asks the research
  cluster for design options.
- Done when: a test shows the question in the memory file of the target and the answer in the memory file of the asker.

### 1.8 A prompt cluster in between, and a slot for a human (block 5)
- Build: a profile field `before: "prompt"` sends the task through a named cluster that rewrites it. A part with
  `role: "human"` stops and waits for yes or no.
- Done when: the task text a worker receives differs from the text the orchestrator wrote, and a cluster with a human
  part before `git push` does not push after a no.

### 1.9 A cluster as a file, and the cost before the start (block 4)
- State: a profile is JSON. Requests already know `$name(role=..., goal="...")` and `;;`.
- Build: a cluster file in the same language as requests, for example
  `$dev(parts=[researcher, coder*3, validator, tool], max_models=8, max_depth=3, budget_eur=2)`. It is parsed into the
  existing `Profile`. `penelope cluster run --dry` prints clusters, parts, the model of each part and an estimate of
  tokens and euro, and starts nothing.
- Done when: a cluster file and its JSON profile produce the same `Profile` in a test, and a dry run makes no model call.

### 1.10 The measurement (B24) — done
- Result: `0bdaf59`, PR #4. `penelope eval` for large / large-subagents / cluster with scratch copy, check, tokens, euro, wall time and `docs/RUNS.md`; `penelope replay` is read-only. 187 unit + 2 integration tests. No real 20-task run yet.
- State: no eval runner exists. Every advantage of a cluster is an expectation.
- Build: `penelope eval tasks.jsonl --setup NAME`. A task has a prompt, a repository state and a check command. Three
  setups: one large model, one large model with subagents, a cluster of small models. Output per setup: passed checks,
  tokens in and out, euro, wall time. Add `penelope replay ID` for one stored run.
- Done when: 20 tasks run in all three setups and the table is written to `docs/RUNS.md`.

## Phase 2: memory and learning without training

### 2.1 Notes with links and a status (block 7)
- State: `brain.rs` writes an Obsidian vault with parent and child links. `notes.rs` (unwired) reads a status.
- Build: a cluster note carries source, date and status `confirmed`, `open` or `stale`. The validator sets
  `confirmed`. A part loads only the notes at most 2 links away from its task and never a stale one.
- Done when: the prompt of a part in a test holds 3 of 10 notes and no stale note.

### 2.2 Score per part and advice for the profile (block 8)
- Build: `validate()` records accept or reject per part and task type with `advise::record` in
  `.penelope/clusters/scores.json`. `penelope cluster advise NAME` prints the suggestions.
- Done when: 4 rejections of 5 for one part print one suggestion that names the part.

### 2.3 Later, each as its own package
- Two small models answer one step, the large model decides only on disagreement (block 6).
- Shadow cluster: a small cluster does the same task beside the large model, Penelope shows the match rate (block 9).
- Routing by the score table (block 10), two variants with one winner (block 11), change of a running cluster (block 12).

## Phase 3: MCP

### 3.1 A client that lives between two calls (B05, B06, B19) — done
- Result: `d475172`, `a4fddc7`, PR #2. One process per server/session, env/cwd, per-tool rules, media files, timeouts. 9 mcpclient tests. Open: native claude still gets `mcp__SERVER__*` for a per-tool deny; the pool is per Penelope session.
- State: `mcpclient.rs` starts one process per call, stdio only, text results only, no env and no cwd.
- Build: keep one process per server and session. Add env, cwd, allow or deny per tool, and picture and resource
  content in results.
- Done when: two calls to one server reach the same process, shown by a counter tool in a test.

### 3.2 The brief
- State: a design, a prototype in Python and a paper exist in the research repository:
  `prototypes/11_mcp_auftrag` and `reports/Paper 7 Auftrag statt Aufruf.md`. Measured there for 3 pages: 3 calls and
  645 tokens with a brief, 18 calls and 1,225 tokens with handle tools, 15 calls and 13,383 tokens with classic tools.
- The format: a tool has an argument `brief`. The model writes the short form
  `{"task": "...", "attach": ["link:URL", "file:NAME", "image:NAME", "context:NAME"]}`. The client replaces each name by
  an item `{"id", "kind", "name", "mime", "bytes", "sha256"}` with the content in exactly one of `text`, `data`
  (base64, up to 256 KB), `upload` (sent before in chunks through the tool `brief_put`) or `uri`. A server announces
  the extension as `io.github.laertesai/brief`.
- Build in the client: when a tool schema has a property `brief` and the server announces the extension, pack the
  short form before the call. Build in the server `mcp.rs`: accept a brief, check size, hash, read folders and allowed
  hosts, and return steps, findings and a line `next:` with the next call as JSON.
- Rule of the owner: only the MCP servers of Laertes get this. Foreign servers keep working without it.
- Done when: the 22 checks of the prototype pass against the Rust server, and Penelope sends a brief with one picture
  to the ScreenCodec server.

## Phase 4: the platform

### 4.1 A check before every install (G12) — done
- Result: `a9848be`, PR #5. `plugins::check(dir)` scans executable files before add/install ZIP/git/marketplace; patterns include `curl|sh`, `wget|bash`, `base64|sh`, eval curl, `rm -rf /`, reverse shells and dangerous package.json hooks. Report names file:line. 9 new tests. Open: heuristic, no linters, no interactive yes-after-report, line-based detection, skipped large/binary files.
- State: a plugin from git is cloned and registered without any check. It starts as denied.
- Build: before a plugin, mod or app is installed, Penelope runs a local check and shows the result: a scan for
  known malware patterns (network calls in install scripts, obfuscated code, writes outside the own folder), the
  linter of each language found, and a summary by the weak model. The install needs a yes after the report.
- Done when: a test plugin with `curl ... | sh` in a script is refused with the line that caused it.

### 4.2 Mods — done
- Result: `a09c6e2`, PR #7. `/mod list|new|on|off|remove`, palette, picture, layout side/compact/top, validated keys, restore on off. 14 tests. Open: JPEG needs ffmpeg/ImageMagick; layout affects chat sidebar only; subset of keys; /mod new simulated.
- State: missing. The start picture is fixed.
- Build: a mod is a folder with a manifest and settings for theme, background picture, layout and key bindings.
  `/mod new "use this picture as background"` lets the model write the mod. `/mod list`, `/mod on`, `/mod off`.
- Done when: a mod written by the model changes the background picture, and `/mod off` brings the old one back.

### 4.3 Factories (B17)
- State: missing. `jobs.rs` runs background shell jobs without a timer.
- Build: a factory is a cluster profile with `every: "3h"` or a time of day, and one purpose. It runs through the
  scheduler of the operating system (systemd timer, launchd, Task Scheduler), so it works while Penelope is closed.
  `/factory new`, `/factory list`, `/factory stop`.
- Done when: a factory that writes the time into a file every minute has written 3 lines after 3 minutes.

### 4.4 Apps and a launcher
- State: missing.
- Build later: `/app install GITHUB-LINK` like a plugin, with the check of 4.1 and a list of rights per app (files,
  network, shell). First app: a view of all running clusters and their parts. No server of Laertes is needed, apps
  live on GitHub.
- Done when: the cluster view app installs from a GitHub link and shows two running clusters.

### 4.5 Design
- State: missing. Only the TUI art exists.
- Name: the owner chose "Design" as the name of the part. "Weaver" is a second idea. "Da Vinci" is not used.
- Rule: not built on Claude Design.
- Build later: first a written spec of what a design is in Penelope (a page, a picture, an animation) and which file
  it produces. No code before the spec is accepted by the owner.

## Phase 5: selling

### 5.1 The provider Laertes (B26, B08)
- State: `providers.rs` holds a placeholder that returns "no model endpoint yet".
- Build: a plan key, a model list from the endpoint, and the provider as the default of a fresh install. The built-in
  cluster profiles name the slots of Laertes: Nostos for tool calls, Argos as validator, Aiolos for shortening. A
  missing model falls back to the role model of the user.
- Done when: `penelope models --provider laertes` lists the models of the plan with a test key against a fake endpoint.

## What is still open and needs the owner

- The exact syntax of a cluster file (1.9).
- How many models one cluster can hold before its reports cost more than its work.
- Whether the brief keeps its own fields for files or takes the file objects of the official MCP draft
  `io.modelcontextprotocol/files` once that draft has a reference implementation.
- The spec of Design (4.5) and the first three apps (4.4).
