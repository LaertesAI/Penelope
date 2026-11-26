# Building blocks for the cluster layer

Written on 2026-11-25 from a talk with the user. Status: a list of ideas. Three of them are being built as
modules without wiring on the branch `feat/cluster-building-blocks` (blocks 1, 7 and 8). The other blocks need
`cli/src/clusters.rs` and are not started.

Two fixed rules from the user:

- No data of a user is used to train a model of Laertes. Runs, prompts, memory files and reports stay on the
  machine of the user. Training data comes from the user's own runs with open models (Qwen, DeepSeek).
- Every cluster reports back to the orchestrator. The orchestrator gives every instruction, so that two clusters
  do not undo the work of each other.

Each block names what it does, why, where it would hook into the code, and the check that shows it works.

## Blocks for the product

### 1. Evidence field in a report

- What: a report with `S:done` must carry a field `V:` with the command and its result, for example
  `V:cargo test 41/41`. The orchestrator does not accept `done` without it.
- Why: an agent that says "done" without proof is the most common failure of agent systems.
- Hook: `report()` writes the line, `validate()` fills the field from the check it ran.
- Check: a report `S:done|P:2/2|T:12s` without `V:` is rejected, the same line with `V:cat hello.txt ok` passes.

### 2. One git worktree per cluster

- What: a cluster that changes files works in its own git worktree. `cluster_kill` removes the worktree. The
  orchestrator merges a worktree only after the validator answered `PASS`.
- Why: two dev clusters that change the same file overwrite each other today. This is an open point of the plan.
- Hook: `new_cluster()` creates the worktree, `kill()` removes it, `run_cluster()` merges after `validate()`.
- Check: two clusters edit the same file, one is killed, the file in the main folder holds only the other edit.

### 3. Test tasks inside a cluster profile

- What: a profile carries a list `tests` of tasks with an expected check. `penelope cluster test NAME` runs them and
  prints a hit rate.
- Why: a cluster becomes testable like code. A changed profile shows its effect as a number.
- Hook: `parse_profile()` reads `tests`, a new command next to `penelope cluster run`.
- Check: the profile `solo` with 2 test tasks prints `2/2`.

### 4. Plan and cost before the start

- What: `penelope cluster run --dry` prints the clusters, the parts, the model of each part and an estimate of
  tokens and euro. Nothing starts until the user confirms.
- Why: a machine of many models can cost much. The user sees the bill before it runs.
- Hook: `orchestrate()` after `parse_plan()`, prices from `cost.rs`.
- Check: a dry run prints a table and makes no model call for the work.

### 5. A slot for a human

- What: a part with `role: "human"` stops the cluster and waits for a yes or no, for example before a push.
- Why: permission points become part of the program and not of a prompt.
- Hook: `run_workers()` treats the role as a prompt in the TUI or on the command line.
- Check: a cluster with a human part before `git push` does not push after a no.

## Blocks for the assumption that small models are enough

### 6. Two small models, the large one only on disagreement

- What: two small models answer the same step. If the answers agree, the step is done. If not, the large model
  decides.
- Why: the share of steps that never reach the large model is the number that tests the assumption.
- Hook: a part setting `quorum: 2` in the profile, counted in the cluster state.
- Check: the state of a finished cluster holds `agreed` and `escalated` counts.

### 7. Notes with links and a status

- What: the memory of a cluster is a set of notes in Markdown with `[[links]]`. An agent loads only the notes at
  most 2 links away from its task. Each note carries a source, a date and a status: `confirmed` by the validator,
  `open`, or `stale`.
- Why: a small model needs a small context. A stale note must not be read as a fact.
- Hook: `memory_line()` and the prompt builders `worker_system()` and `validator_system()`.
- Check: from 10 linked notes a start note returns exactly the notes within 2 links, without the stale ones.

### 8. A score per part and a suggestion for the profile

- What: after each cluster run Penelope stores, per part and task type, how often the validator accepted or
  rejected its result. From that it prints a suggestion, for example "coding agent 3 was rejected 4 of 5 times,
  change its model". The file stays in `.penelope/` of the project.
- Why: the same kind of task gets cheaper with each run, without any training. Run 1 against run 3 is the number.
- Hook: `validate()` writes the outcome, a new command `penelope cluster advise NAME`.
- Check: 4 rejections of 5 for one part produce a suggestion that names this part, 1 of 5 produces none.

## Newer ideas, not rated yet

### 9. Shadow cluster

- What: while the large model does a task, a cluster of small models does the same task silently in a worktree.
  Penelope compares both results and tells the user which kind of task the small cluster already solves.
- Why: the user gets the evidence for the assumption on the own project, and no data leaves the machine.
- Cost: the task is paid twice while the shadow runs, so it needs a switch and a budget.

### 10. Routing by the score table

- What: the orchestrator reads the scores of block 8 and gives a task type to the part with the best score for it.
- Why: the choice of a model per job is learned from the validator and not set by hand.

### 11. Two variants of one cluster, one winner

- What: the orchestrator starts the same cluster twice with different models or prompts, each in a worktree. The
  validator picks the result that passes, the other cluster is killed.
- Why: a cheap second try in parallel costs less time than a retry after a failure.

### 12. Change a running cluster

- What: `cluster_profile` changes a running cluster: add a coding agent, change the model of a part. The memory
  file carries the state, so the cluster does not start again.
- Why: a cluster that is too small or too weak is fixed without losing its work.

## The measurement that is still missing

20 fixed tasks in one repository, run in three setups: one large model, one large model with subagents, a cluster
of small models. The numbers to publish are tokens, time and passed checks. Without this run every advantage of a
cluster is an expectation.
