# Executable checker for code tasks

Eurykleia checks the shape of a tool call. The checker runs code. In the lab's SWE-bench run, 20 of 28 patches failed and none of the 20 was flagged, because nothing executed the patch before the final answer.

## What it does

1. The model writes a reproduction test from the issue text as its own file.
2. It calls `verify_fix` with the path. Penelope adds a detached git worktree at HEAD in the temp folder and copies the test in.
3. The test runs on the original code. It must fail. A test that passes here does not reproduce the issue (`test_passes_before`).
4. The working-tree change (tracked diff plus untracked files, without the test) is applied to the worktree. The test runs again. It must pass (`still_fails` otherwise).
5. The module tests run on the changed copy. They must pass. When they fail, they run once more on HEAD; a failure that was already there does not count (`breaks_module` only for a new failure).
6. The worktree is removed. The files of the user are never written.

Every verdict goes to `.penelope/check/log.jsonl`, the last check to `.penelope/check/last.json`.

### Gate before the final answer

With `check.mode` set to `gate` (default), a code edit by the lead model arms the turn. A code edit is a `write` or `edit` on a file ending in `.py`, `.rs`, `.js`, `.ts`, `.go`, `.java`, `.c`, `.cpp` and 20 more endings. When the model then answers without a check in this turn, the API tool loop sends one reminder back (`check.nudges`, default 1). Subagents and cluster parts are not reminded.

Every turn that changed code ends with one line under the answer: `Checker: verified.`, `Checker: NOT verified (still_fails).` or `Checker: not verified. No reproduction test ran ...`. The line also works for the claude command, which runs its own tool loop: Penelope reads `last.json` and the tool trace of the turn. An unverified code turn is counted in the log as `turn:unverified`, so the share of unnoticed failures can be read back with `/check`.

`report` keeps the line and drops the reminder. `off` drops both and removes rule 12 from the system prompt.

### Best-of-N

`verify_fix` with `best_of: N`, or `/check best TEST N -- ISSUE`, builds N candidates (default 3, at most 8):

- The weaker model gets the issue, the test and the changed files at HEAD, and answers with SEARCH/REPLACE blocks.
- Each candidate gets a different hint (smallest change, fix where the value is produced, edge case, read the test first), because two runs of one model mostly return the same patch (see the hypothesis below).
- `check.models` names the models in turn, for example `["weak", {"kind": "openai", "model": "deepseek-v4.1", ...}]`. Mixed models are the path to more coverage.
- The working tree joins as one more candidate when it has a change.

All candidates are applied first, which is cheap. They are sorted by diff size, smallest first, ties by order. The tests run in that order and the first candidate that passes wins. The test on HEAD runs once for all. The winning diff goes to `.penelope/check/winner.diff`; Penelope does not apply it to the working tree.

### Config

```json
"check": {"mode": "gate", "module_tests": "", "run": "", "candidates": 3, "models": ["weak"], "timeout": 300, "nudges": 1}
```

- `module_tests`: command for the existing tests; empty uses `test_command`.
- `run`: command for the reproduction test, `{test}` is its path. Empty uses `python3` (`python` on Windows), `node`, `sh` or `ruby` by the file ending. A Rust or Go test needs `run`, for example `cargo test repro_issue_12`.
- `timeout`: seconds per command; the process tree is killed after it.

Every command in the worktree runs with the worktree first on `PYTHONPATH`. SWE-bench environments install the package with `pip install -e .`, which points at the original checkout. Without this, the "original code" run would import the fixed files of the user and every test would pass before the fix. A Rust test in the worktree builds a fresh `target` folder; set `run` with `CARGO_TARGET_DIR` to share one.

### Commands

- `/check` and `penelope check`: settings, last check and the log counts.
- `/check TEST [--run CMD] [--module CMD] [--cwd DIR]`: check the working tree.
- `/check best TEST [N] [--files a,b] -- ISSUE`: best-of-N.
- `/check gate|report|off`: set the mode in `config.json`.

## Research

| Work | Setting | Numbers |
|---|---|---|
| Agentless (Xia et al. 2024, arXiv 2407.01489) | SWE-bench Lite, 40 patches and 40 generated reproduction tests per issue | 213 of 300 issues (71 %) got a test that reproduced the issue on the original code. With the gold patch, only 94 of those 213 (44 %) reported the issue as resolved. Selection: majority vote 77 (25.7 %), plus regression tests 81 (27.0 %), plus reproduction tests 96 (32.0 %). |
| CodeMonkeys (Ehrlich et al. 2025, arXiv 2501.14723) | SWE-bench Verified, 10 edits per issue, each with a test script that must flag the unedited code | Coverage 69.8 %, random pick 45.8 %, selection 57.4 %. Selection keeps 82 % of coverage and closes 48 % of the room between random and coverage. |
| OpenHands critic (All Hands blog, April 2025) | SWE-bench Verified, Claude 3.7 Sonnet, trained Qwen 2.5 Coder 32B critic | 60.6 % with one rollout, 66.4 % with best of 5. |
| Large Language Monkeys (Brown et al. 2024, arXiv 2407.21787) | SWE-bench Lite, DeepSeek-Coder-V2 | 15.9 % with 1 sample, 56 % with 250, about 16.7 points per tenfold samples. Without a verifier, majority vote and reward models stop gaining beyond several hundred samples. |

What the checker takes from each: the fail-before rule and the regression filter from Agentless and CodeMonkeys, test-based selection from CodeMonkeys, smallest diff as a tie rule in place of Agentless's majority vote (3 candidates are too few for a vote), and the coverage model from Large Language Monkeys.

## Hypotheses, written before measuring

### The hit model

A task has a chance p that one candidate is correct. The checker passes a correct candidate with chance t (true positive rate) and a wrong one with chance f (false positive rate). The first passing candidate wins; when none passes, the first candidate stays. With q = p t + (1 - p) f, the chance that the task ends solved is

    h(p, t, f, N) = p t (1 - (1 - q)^N) / q  +  p (1 - t) (1 - q)^(N - 1)

and expected hits are the sum of h over the tasks. The formula is checked against a simulation of 100,000 rounds for three settings in the unit test `the_hit_formula_matches_a_simulation` (difference below 0.01), and again by a separate Python simulation of 200,000 rounds (difference below 0.0005).

Two properties follow:

- With N = 1, h = p for every t and f. A checker on one candidate never changes the expected hit rate. At N = 1 its value is noticing failures and in-turn repair.
- Coverage c(N) = sum of 1 - (1 - p)^N is the ceiling: with t = 1 and f = 0, h reaches it.

### What the lab's two Haiku runs say

The two runs solved 8 and 9 of 28 tasks, union 9. For independent runs, expected union minus expected single score is the sum of p (1 - p). Here that sum is 9 - 8.5 = 0.5. The number of tasks solved in exactly one run is D = 1, and E[D] = 2 times the sum of p (1 - p).

For every task, 1 - (1 - p)^N - p = p ((1 - p) + ... + (1 - p)^(N - 1)), which is at most (N - 1) p (1 - p). So the coverage gain of N candidates of the same model over one is at most (N - 1) D / 2 in expectation:

- N = 3: at most 1.0 task of 28 (3.6 points), point estimate.
- With D = 1 observed, the 95 % Poisson upper bound for E[D] is 4.74, so the gain at N = 3 is at most 4.7 tasks (17 points) at that bound.

A mixture that fits both runs: 8 tasks at p = 0.97, 1 at 0.5, 3 at 0.1, 16 at 0. It gives 8.56 per run and a union of 9.31. Its coverage is 9.69 at N = 3 and 10.2 at N = 5. Expected hits:

| t | f | N = 1 | N = 3 | N = 5 |
|---|---|---|---|---|
| 0.44 (Agentless test quality) | 0.2 | 8.56 | 8.86 | 9.01 |
| 0.7 | 0.2 | 8.56 | 9.14 | 9.35 |
| 0.9 | 0.1 | 8.56 | 9.46 | 9.81 |
| 0.7 | 0.4 | 8.56 | 8.89 | 8.96 |
| 1.0 | 0 | 8.56 | 9.69 | 10.20 |

The Large Language Monkeys slope would predict 8 points from N = 1 to N = 3. The lab's runs bound it at 3.6 points. Same-model best-of-3 on this set gains 0.3 to 0.9 tasks of 28 (1 to 3 points).

### H1: the checker notices failures (main claim)

At N = 1 with `gate`, unnoticed failures on the 28 tasks fall from 20 of 20 to at most 8 (f at most 0.4), predicted 4 to 8. At least 60 % of tasks get a reproduction test that fails on the original code (Agentless: 71 %). Measure: `turn:unverified` plus checks that passed on tasks the benchmark grades as failed, over all failed tasks. A 28-task run can show this: 20 failures with 8 or fewer unnoticed is far outside 20 of 20.

### H2: same-model best-of-3 adds little

Haiku best-of-3 with the checker: +0.3 to +0.9 tasks of 28. On 100 tasks: +1 to +3 points. The minimum detectable difference at 100 paired tasks is 7.8 points at 80 % power (Paper 68). A 100-task run will not separate H2 from zero, and a non-result there is the prediction, not a failure.

### H3: mixed models are the way to a measurable gain

Coverage is the ceiling, and two Haiku runs share it almost fully. With t = 0.9 and f = 0.1 the checker turns 80 % of a coverage gain into hits (from the table: (9.46 - 8.56) / (9.69 - 8.56)). For +7.8 points of hits on 100 tasks, the candidate pool needs about +10 points of coverage over one Haiku run. A cheap pre-test decides it before any best-of run: one Haiku run and one DeepSeek V4.1 run (OpenCode Go) on the same 100 tasks. If DeepSeek solves at least 10 tasks that Haiku does not, H3 is worth a best-of run with `check.models` set to both. If it solves fewer than 5, H3 fails on this model pair and the next pair is tried.

### What a 100-task run must show

Paired arms on the same 100 tasks, McNemar test:

- A: Haiku, N = 1, checker off. B: Haiku, N = 1, `gate`. C: Haiku, best-of-3. D: mixed models, best-of-3.
- B against A: unnoticed failures at most 40 % of failures in B, against about 100 % in A. With about 65 failures, the 95 % interval is about plus or minus 12 points, so this is decisive.
- D against A: at least +8 points of hits, for example 12 tasks gained and 4 lost.
- C against A: +1 to +3 points; a larger gain would refute the two-run bound.
- Checker quality from the gold labels: t = passed among resolved, f = passed among unresolved. The gains above need t at least 0.6 and f at most 0.3.

## Assumptions

1. "20 of 28" means: 28 tasks, 8 solved, 20 failed, none of the 20 flagged by any check.
2. The two Haiku runs are independent draws of the same per-task chances. Prompt, seed and caching effects are ignored.
3. t and f do not depend on p. A hard task probably also gets a weaker test, which would lower t on exactly the tasks that matter.
4. Tie breaking by smallest diff is independent of correctness. If small diffs are more often right, the table is a lower bound.
5. The Agentless test quality (44 % accept the gold patch, given that the test reproduces the issue) transfers to Haiku and DeepSeek.
6. The module tests that the model or the config names are the relevant ones. The checker does not select them as Agentless does.
7. Paper 68's minimum detectable difference of 7.8 points at 100 pairs applies to this task set.
8. SWE-bench repositories are git checkouts with a HEAD commit, so the worktree approach applies.

## Discoveries

1. The lab's two runs bound same-model best-of-N: the sum of p (1 - p) is 0.5 on 28 tasks, and the coverage gain of N runs is at most (N - 1) D / 2 with D the tasks solved in exactly one of two runs. For Haiku at N = 3 that is 1 task (3.6 points), against 8 points from the Large Language Monkeys slope. Two runs are therefore a cheap test of whether best-of-N can pay before running it. Checked twice: the bound algebraically, and on the fitted mixture (exact gain 1.13 tasks against a bound of 1.5).
2. A checker on one candidate cannot raise the expected hit rate (h = p at N = 1 for all t and f). Selection needs N above 1; at N = 1 the checker's value is noticing failures and repair.
3. Agentless's own numbers give its self-written tests a true positive rate near 0.44 as a filter. At that t (f = 0.2), Haiku best-of-3 gains 0.3 tasks of 28 and best-of-5 gains 0.45. Better tests raise the gain more than more candidates: t = 0.9 and f = 0.1 at N = 3 gain 0.9 tasks, three times as much.

4. A worktree check of a Python project with an editable install tests the wrong code unless the worktree comes first on `PYTHONPATH`: the import resolves to the original checkout, which holds the fix, so the reproduction test passes on "the original code" and the check rejects a correct fix as `test_passes_before`. The checker sets the path; the unit test with a test in a subfolder covers the import order.

## Measured offline

The unit tests in `cli/src/checker.rs` build small git fixture repositories:

- a correct fix passes, the working tree is unchanged after the check, and no worktree is left behind;
- a test that passes on the original code is rejected;
- a wrong fix fails, and a fix that breaks a module test fails;
- an untracked new file counts as part of the change;
- no git, a missing test and a path outside the project are named;
- best-of-N with 4 fixed candidates (one too big, one wrong, one that does not apply, one small and right) picks the small right one and writes only its diff;
- a test that hangs is killed after its time limit;
- a real Python fixture runs when Python is installed, with the test in a subfolder that imports from the worktree root;
- the reminder comes once after a code edit, never after a Markdown edit, never for a worker, and not after a check in the same turn.

The fixture tests use `git grep` as the test command, which exists on every CI runner. No model call runs in the tests.
