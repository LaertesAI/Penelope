# Executable checker for code tasks

Eurykleia checks the shape of a tool call. The checker runs code. In the lab's SWE-bench run, 20 of 28 patches failed and none of the 20 was flagged, because nothing executed the patch before the final answer.

## What it does

1. The model writes a reproduction test from the issue text as its own file.
2. It calls `verify_fix` with the path. The commands pass the same gate as the `run` tool (see Safety). Penelope adds a detached git worktree at HEAD in the temp folder, with git hooks switched off, and copies the test in.
3. The worktree gets the ignored build output of the checkout: `node_modules`, `.venv` and `venv` folders are linked, compiled extensions (`*.so`, `*.pyd`, `*.dylib`) are copied. Without them `import sklearn` or `require("x")` fails in the worktree before and after any fix.
4. The test runs on the original code. It must fail. A test that passes here does not reproduce the issue (`test_passes_before`).
5. The working-tree change (tracked diff plus untracked files, without the test) is applied to the worktree. The test runs again. It must pass (`still_fails` otherwise).
6. When the test fails with an import, syntax or collection error both before and after the change, the verdict is `env_error`: the worktree lacks something, and the test shows nothing about the issue. A pytest exit code from 2 to 5 (interrupted, internal error, usage error, no test collected) counts the same way.
7. The module tests run on the changed copy. They must pass. When they fail, they run once more on HEAD; a failure that was already there does not count (`breaks_module` only for a new failure).
8. The worktree is removed. The files of the user are never written. Git adds one entry under `.git/worktrees` for the check and prunes it afterwards.

Every verdict goes to `.penelope/check/log.jsonl`. The last check of a task goes to `.penelope/check/KEY/last.json`, where KEY hashes the repository root and the session. Two eval tasks in parallel, or two sessions on one machine, therefore never read each other's check. Outside a project with its own `.penelope` folder the record lies under `~/.penelope/check`.

### Safety

The checker executes code the model wrote: the test, the fix and every candidate. It is gated like the `run` tool.

- A `run` or `module_tests` command runs when it is on the safe list or the project allow list. Otherwise the person is asked once; a yes adds it to the allow list.
- The default interpreter run (`python3 TEST` and the others by file ending) needs one yes per session, because it runs a model-written file directly.
- `run_policy` `all` lets every command pass. A scout never runs a check outside the safe list.
- With `sandbox` set to `bwrap`, every command runs in bubblewrap: read-only root, the worktree and the shared cargo folder writable, private `/tmp`, no network.
- Without a person to ask and without `run_policy` `all`, the answer is `CHECK NOT RUN` with the reason. An eval run therefore needs `run_policy` `all` or a run command from the safe list, for example `pytest {test}`.
- Test output passes the privacy guard before it reaches the model, as `run` output does.

A candidate may change only the files that `files` lists (empty: the files the lead changed). It may never add or change `sitecustomize.py`, `usercustomize.py`, `conftest.py`, a `*.pth` file, `setup.cfg`, `pytest.ini`, `tox.ini` or `pyproject.toml` unless `files` lists it. A new `sitecustomize.py`, `usercustomize.py`, `conftest.py` or `*.pth` in the working tree is refused too. The reason: the worktree goes first on `PYTHONPATH`, so a one-line `sitecustomize.py` with `import os; os._exit(0)` turns every failing Python test into exit 0.

### Gate before the final answer

With `check.mode` set to `gate` (default), a code edit by the lead model arms the turn. A code edit is a `write` or `edit` on a file ending in `.py`, `.rs`, `.js`, `.ts`, `.go`, `.java`, `.c`, `.cpp` and 20 more endings. When the model then answers without a check in this turn, the API tool loop sends one reminder back (`check.nudges`, default 1). The reminder is skipped on the last round of the loop, so the answer the model just gave is never lost to the round limit. Subagents and cluster parts are not reminded. The judge role does not get `verify_fix`, because it may not write and its commands are gated.

Every turn that changed code ends with one line under the answer: `Checker: verified.`, `Checker: NOT verified (still_fails).` or `Checker: not verified. No reproduction test ran ...`. The line also works for the claude command, which runs its own tool loop: Penelope reads the record of the task and the tool trace of the turn. The MCP server of the claude command gets the session in `PENELOPE_CHECK_SESSION`, so both sides use the same record. An unverified code turn is counted in the log as `turn:unverified`.

`report` keeps the line and drops the reminder. `off` drops both and removes rule 12 from the system prompt.

### Best-of-N

`verify_fix` with `best_of: N`, or `/check best TEST N -- ISSUE`, builds N candidates (default 3, at most 8):

- The weaker model gets the issue, the test and the files at HEAD, and answers with SEARCH/REPLACE blocks. A file that fits the budget of 40,000 characters goes whole. A longer one goes as the lead's changed hunks with 80 lines around each, and the prompt names the ranges shown.
- Each candidate gets a different hint (smallest change, fix where the value is produced, edge case, read the test first), because two runs of one model mostly return the same patch.
- `check.models` names the models in turn, for example `["weak", {"kind": "openai", "model": "deepseek-v4.1", ...}]`. Mixed models are the path to more coverage.
- The working tree joins as one more candidate when it has a change.

The working tree is tried first. When it passes, it wins, and the model candidates still run for the report while the time budget lasts. When it fails, the model candidates run in diff-size order, smallest first, and the first that passes wins. The test on HEAD runs once for all. A file with CRLF line endings keeps them, so diff sizes compare fairly. The winning diff goes to `winner.diff` in the record folder of the task; Penelope does not apply it.

### Config

```json
"check": {"mode": "gate", "module_tests": "", "run": "", "candidates": 3, "models": ["weak"], "timeout": 300, "budget": 1200, "nudges": 1}
```

- `module_tests`: command for the existing tests; empty uses `test_command`.
- `run`: command for the reproduction test, `{test}` is its path. Empty uses `python3` (`python` on Windows), `node`, `sh` or `ruby` by the file ending. A Rust or Go test needs `run`, for example `cargo test repro_issue_12`.
- `timeout`: seconds per command; the process tree is killed after it.
- `budget`: seconds for one whole check or best-of call. A command that would start after it is skipped, and the check says so. Without it, 1 + 2 x 9 + 1 runs at 300 s could block one tool call for about 100 minutes.

Every command in the worktree runs with the worktree, and its `src` folder when there is one, first on `PYTHONPATH`. Rust tests share one target folder under `~/.cache/penelope/check-target` unless `CARGO_TARGET_DIR` is set.

### Commands

- `/check` and `penelope check`: settings, the last check of this session, the log counts, the counts of `env_error`, `test_passes_before` and `still_fails` per repository, and the number of turns where a failed check was followed by a passed one (repairs).
- `/check TEST [--run CMD] [--module CMD] [--cwd DIR]`: check the working tree.
- `/check best TEST [N] [--files a,b] -- ISSUE`: best-of-N.
- `/check gate|report|off`: set the mode in `config.json`.

## Research

| Work | Setting | Numbers |
|---|---|---|
| Agentless (Xia et al. 2024, arXiv 2407.01489) | SWE-bench Lite, 40 patches and 40 generated reproduction tests per issue | 213 of 300 issues (71 %) got a test that reproduced the issue on the original code. With the gold patch, only 94 of those 213 (44 %) reported the issue as resolved. Both rates come from one test selected out of 40 per issue. Selection: majority vote 77 (25.7 %), plus regression tests 81 (27.0 %), plus reproduction tests 96 (32.0 %). |
| CodeMonkeys (Ehrlich et al. 2025, arXiv 2501.14723) | SWE-bench Verified, 10 edits per issue, each with a test script that must flag the unedited code | Coverage 69.8 %, random pick 45.8 %, selection 57.4 %. Selection keeps 82 % of coverage and closes 48 % of the room between random and coverage. |
| OpenHands critic (All Hands blog, April 2025) | SWE-bench Verified, Claude 3.7 Sonnet, trained Qwen 2.5 Coder 32B critic | 60.6 % with one rollout, 66.4 % with best of 5. |
| Large Language Monkeys (Brown et al. 2024, arXiv 2407.21787) | SWE-bench Lite, DeepSeek-Coder-V2 | 15.9 % with 1 sample, 56 % with 250, 16.7 points per tenfold samples (7.98 points from N = 1 to N = 3). Without a verifier, majority vote and reward models stop gaining beyond several hundred samples. |

What the checker takes from each: the fail-before rule and the regression filter from Agentless and CodeMonkeys, test-based selection from CodeMonkeys, smallest diff as a tie rule in place of Agentless's majority vote (3 candidates are too few for a vote), and the coverage model from Large Language Monkeys.

## The hit model

### One test per task (what the code does)

The checker writes one reproduction test per task and runs all N candidates against it. Test quality is therefore drawn once per task, not once per candidate. The model, with the Agentless rates as the test quality:

- With chance 0.29 there is no test that fails on HEAD. Nothing is selected; the first candidate stays: h = p.
- With chance 0.71 x 0.44 = 0.31 the test is good: it passes every correct candidate (t = 1) and a wrong one with chance f. With q = p + (1 - p) f, h = p (1 - (1 - q)^N) / q.
- With chance 0.71 x 0.56 = 0.40 the test is bad: it passes no correct candidate (t = 0) and a wrong one with chance f. h = p (p + (1 - p)(1 - f))^(N - 1), which is below p for every f above 0.

The bad-test term is why more candidates can lose: each extra wrong candidate is one more chance for a bad test to pick it. On the mixture below (8 tasks at p = 0.97, 1 at 0.5, 3 at 0.1, 16 at 0; 8.56 hits per run):

| Test quality | f | N = 1 | N = 3 | N = 5 |
|---|---|---|---|---|
| Agentless rates | 0.2 | 8.56 | 8.71 | 8.69 |
| Agentless rates | 0.1 | 8.56 | 8.81 | 8.87 |
| Agentless rates | 0.04 | 8.56 | 8.87 | 8.99 |
| Agentless rates | 0 | 8.56 | 8.91 | 9.07 |
| good test in 80 % of reproducing tasks | 0.2 | 8.56 | 9.00 | 9.10 |
| good test in 80 % of reproducing tasks | 0.04 | 8.56 | 9.16 | 9.40 |
| 95 % reproduce, 80 % of those good | 0.04 | 8.56 | 9.36 | 9.69 |

With f = 0.2 the gain is +0.15 tasks of 28 at N = 3 and +0.13 at N = 5. With f = 0 it is +0.35 and +0.51. The false-pass rate f decides the gain more than N does. The unit test `the_shared_test_formula_matches_a_simulation_and_the_doc_table` checks the formula against 100,000 simulated rounds for three settings and checks the four Agentless rows of this table.

These numbers are upper bounds for same-model best-of-N. The real pool is N single-shot SEARCH/REPLACE answers that see only the files the lead edited, plus the working tree. Single-shot p is probably lower than the agent's p, and the candidates share the lead's localization.

Working tree first: a simulation of 200,000 rounds per point (this PR, not checked by the reviewers) gives no change when all candidates share one p (0.544 either way at p = 0.5, N = 3 weak candidates, f = 0.2). When the weak candidates have half the lead's p, trying the lead first gives 0.484 against 0.469 for random order at p = 0.5. Both are below 0.5: with a shared test and f = 0.2, weaker candidates can cost hits.

### One test per candidate (the earlier model)

The first version of this document drew t and f per candidate. With q = p t + (1 - p) f:

    h(p, t, f, N) = p t (1 - (1 - q)^N) / q  +  p (1 - t) (1 - q)^(N - 1)

It gives 8.86 at N = 3 and 9.01 at N = 5 for t = 0.44, f = 0.2, against 8.71 and 8.69 with one shared test. It overstates the gain because independent tests per candidate do not exist in the code. The formula is checked against 100,000 simulated rounds in `the_hit_formula_matches_a_simulation` (tolerance 0.01). A separate Python simulation of 200,000 rounds gave 0.69407 against 0.69375 for p = 0.5, t = 0.7, f = 0.2, N = 3. The standard error there is 0.0010, so the right tolerance is 3 standard errors (0.003), not the 0.0005 stated earlier.

Two properties hold in both models:

- With N = 1, h = p for every t and f. Selection needs more than one candidate. At N = 1 the checker changes hits only through repair inside the turn: a failed check, a new fix, a passed check. That effect is not in the model and is measured directly (arm B against A below).
- Coverage c(N) = sum of 1 - (1 - p)^N is the ceiling.

## What the lab's two Haiku runs say

The two runs solved 8 and 9 of 28 tasks, union 9. For independent runs, expected union minus expected single score is the sum of p (1 - p). Here that sum is 9 - 8.5 = 0.5. The number of tasks solved in exactly one run is D = 1, and E[D] = 2 times the sum of p (1 - p).

For every task, 1 - (1 - p)^N - p is at most (N - 1) p (1 - p). So the coverage gain of N iid runs of one agent over one run is at most (N - 1) E[D] / 2. This is the pass@k idea of Chen et al. 2021 plus concavity; it is not new.

- Point estimate at N = 3: 1.0 task of 28 (3.6 points).
- With D = 1 observed, the one-sided 95 % Poisson upper bound for E[D] is 4.74, so the cap at N = 3 is 4.74 tasks (16.9 points).
- The Large Language Monkeys slope predicts 7.98 points at N = 3, which needs E[D] of at least 2.24. P(D at most 1 given E[D] = 2.24) = 0.34. Two runs do not refute the slope.
- The bound covers iid draws of one agent. The code draws hinted single-shot candidates, so p changes per candidate and the two-run D does not transfer to them.

The mixture fits poorly in one respect: it gives a sum of p (1 - p) of 0.753 and a union of 9.31, against 0.5 and 9 observed. Its exact N = 3 coverage gain (1.13 tasks) is above the 1.0-task point estimate, and below its own bound of 1.5.

## Hypotheses, written before measuring

### H1: the checker notices failures (main claim)

At N = 1 with `gate`, on the 28 tasks, unnoticed failures fall from 20 of 20. A failure counts as noticed only when a check reproduced the issue on HEAD (failed there, no `env_error`) and did not pass afterwards. A check that ended `test_passes_before` does not count: that test never reproduced the issue.

- Prediction: 4 to 8 unnoticed of 20, which means f = 0.2 to 0.4 only if every failed task gets a check.
- What 28 tasks can show: 8 of 20 has a Wilson 95 % interval of 0.22 to 0.61. That rules out 20 of 20. It cannot confirm f at most 0.4.
- Reported next to it: t, the share of gold-resolved tasks whose check passed. A checker that rejects everything has 0 unnoticed failures and t = 0, so H1 holds only together with t.
- At least 60 % of tasks get a reproducing test. Agentless's 71 % comes from one test selected out of 40, so it is an upper reference, not a forecast.

### H2: same-model best-of-3 adds little

Haiku best-of-3 with the checker, one shared test per task: +0.15 tasks of 28 at f = 0.2, +0.35 at f = 0. On 100 tasks that is under 2 points.

- At 100 paired tasks with 6 % base discordance, power is 0.02 for +1 point and 0.09 for +3 points (exact McNemar). A null result at 100 tasks is the prediction.
- Refutation: a gain above 16.9 points, the 95 % bound from the two runs. A smaller gain says nothing at n = 100. The clean test is a same-model best-of-3 on 200 or more tasks against (N - 1) D / 2 measured on the same tasks.

### H3: mixed models are the way to a measurable gain

Coverage is the ceiling, and two Haiku runs share it almost fully. With t = 0.9 and f = 0.1 per candidate, the checker turns 80 % of a coverage gain into hits ((9.462 - 8.56) / (9.688 - 8.56) = 0.800). For +7.8 points of hits the pool needs about +10 points of coverage over one Haiku run. With one shared test the conversion is lower, so +10 points of coverage is a floor.

- Pre-test: one Haiku run and one DeepSeek V4.1 run (OpenCode Go) on the same tasks. The expected coverage added by one DeepSeek candidate is the sum of p_D (1 - p_H), which is the count of tasks DeepSeek solves and Haiku does not.
- On 100 tasks with a bar of 10, the pre-test is noisy: P(count at least 10 given a true 7) = 0.17, P(count below 10 given a true 12) = 0.24. On 200 tasks with a bar of 20: 0.08 and 0.18. The pre-test runs on 200 tasks.
- Main test: D against A on 200 pairs. At 16 % discordance, 80 % power for +8 points needs about 210 pairs; at n = 100 power is 0.44. The example 12 gained, 4 lost has exact two-sided p = 0.077, not significant. Alternative: a one-sided test registered before the run (12 against 4 gives one-sided p = 0.038).
- The pass rule is the significance test, not a point bar. A bar of +8 points equals the expected gain if H3 is exactly true, so the run would miss it about half the time.

### Q: checker quality from the gold labels

t = passed among gold-resolved tasks, f = passed among unresolved tasks. The gains above need t at least 0.6 and f at most 0.3. At 100 tasks: about 65 failures give a 95 % half-width of 11.9 points at 40 % (Wilson 0.29 to 0.52 for 26 of 65); about 35 resolved tasks give plus or minus 16 points on t. t at least 0.6 is barely separable from Agentless's 0.44 at n = 100.

### Sample sizes

| Comparison | n | Discordant share | Result |
|---|---|---|---|
| MDE, alpha 0.05, 80 % power | 100 | 4 / 8 / 10 / 16 / 20 % | 4.0 / 7.8 / 8.8 / 11.1 / 12.4 points |
| H2, C against A | 100 | 6 % | power 0.02 for +1 point, 0.09 for +3 |
| H3, D against A, 12 gained and 4 lost | 100 | 16 % | p = 0.077 two-sided, power 0.44 for +8 |
| H3, 80 % power for +8 points | about 210 | 16 % | 154 pairs at 10 gained and 2 lost |
| H1, 8 of 20 unnoticed | 28 | | Wilson 0.22 to 0.61 |
| B, D = 1 | 28 | | Poisson bound 4.74 one-sided, 5.57 two-sided |

### What a run must show

Paired arms on the same tasks, McNemar test:

- A: Haiku, N = 1, checker off. B: Haiku, N = 1, `gate`. C: Haiku, best-of-3. D: mixed models, best-of-3.
- B against A, 100 tasks: unnoticed failures at most 40 % of failures in B, against about 100 % in A, with t reported. Also the number of turns where a failed check was followed by a passed one (`/check` prints it as repairs) and the gold-resolved share among them. Success for repair: B gains at least 5 tasks that pass only after a repair.
- C against A: under 2 points, inside noise at 100 tasks (H2).
- D against A, 200 pairs: significant by McNemar at 0.05, or one-sided as registered (H3).

## Assumptions

1. "20 of 28" means: 28 tasks, 8 solved, 20 failed, none of the 20 flagged by any check.
2. The two Haiku runs are independent draws of the same per-task chances. Prompt, seed and caching effects are ignored.
3. Test quality does not depend on p. A hard task probably also gets a weaker test, which would lower t on exactly the tasks that matter.
4. Diff size is independent of correctness. If small diffs are more often right, smallest-first ordering does better than the table.
5. The Agentless rates (71 % reproduce, 44 % of those pass the gold patch) transfer to Haiku and DeepSeek. Both come from a test selected out of 40, so a single model-written test is probably worse.
6. f = 0.2 is assumed, not measured.
7. The module tests that the model or the config names are the relevant ones. The checker does not select them as Agentless does.
8. SWE-bench repositories are git checkouts with a HEAD commit, so the worktree approach applies.

## Findings after review

A finding counts as a discovery only when the math check and the code review both support it as new. None of the four does. They stay as properties and hypotheses.

1. Two runs bound same-model best-of-N at (N - 1) D / 2. Algebra correct, not new (pass@k plus concavity), and overstated: the 95 % cap is 16.9 points, which does not exclude the 8-point Monkeys slope (p = 0.34). Hypothesis. Refuted if a same-model best-of-3 on 200 or more tasks gains more than (N - 1) D / 2 measured on the same tasks.
2. A checker on one candidate cannot raise the expected hit rate. Correct and trivial: with one candidate there is nothing to select. The lever at N = 1 is repair inside the turn, measured as arm B against A.
3. Better tests beat more candidates. Arithmetic confirmed (0.30, 0.45 and 0.90 tasks in the per-candidate model). Under the real code, with one shared test, the lever is the false-pass rate f: f = 0.2 gives +0.15 at N = 3 and +0.13 at N = 5, f = 0 gives +0.35 and +0.51. Hypothesis. Refuted if gold-label t stays at 0.44 and best-of-5 still beats best-of-3 significantly.
4. A worktree check of a Python project with an editable install tests the wrong code unless the worktree comes first on `PYTHONPATH`. Mechanism correct and not new (a known worktree and tox pitfall): `PYTHONPATH` precedes `.pth` entries and PEP 660 finders. The test `the_worktree_beats_an_editable_install_on_the_import_path` now runs it both ways with a `.pth` file that points at the fixed checkout: without the path order the copy of the original code passes (wrong code imported), with it the original code fails as it must. The fix had two side effects, both closed in this PR: a candidate `sitecustomize.py` could make every Python test pass, and in-place C extensions were missing from the worktree.

## Next hypotheses

Each comes with the number that would show it works.

- Two independent tests: a candidate must pass the lead's test and one the weak model writes from the issue alone, and both must fail on HEAD. f becomes roughly f squared (0.2 to 0.04: +0.31 tasks at N = 3 in the table instead of +0.15). Test on 100 SWE-bench Verified tasks with gold labels: f falls to at most half of the one-test value, and t drops by at most 10 points.
- Gold-patch calibration before any best-of run: Haiku writes a reproduction test per task, the checker runs it against the gold patch and an empty patch. This measures t and the `env_error` share for one test-writing call per task. On 100 tasks t has a 95 % interval of about plus or minus 10 points. Continue to best-of only when t is at least 0.6 and `env_error` is at most 5 %.
- Mutation check: revert the winning candidate hunk by hunk. The test must fail again when the hunk at the issue site is reverted. Target: at least 18 of 20 hand-made wrong-but-passing candidates rejected, t on gold patches down by at most 5 points.
- DeepSeek V4.1 through OpenCode Go as the default second candidate source, next to Haiku, each with the full file and the lead's localization. Pre-test on 200 tasks: DeepSeek solves at least 20 tasks Haiku does not. Then D against A on 200 pairs reaches McNemar p below 0.05.
- `env_error` visibility: `/check` prints the counts per repository. After an eval run, `env_error` is below 5 % in every repository with at least 5 tasks; above that, the sandbox needs more dependency linking for that repository.

## Measured offline

`cli/src/checker.rs` has 23 tests. 14 build a git fixture repository, 9 are unit tests without one. Two of the fixture tests use Python and return early when it is missing.

- a correct fix passes, the working tree is unchanged after the check, and no worktree is left behind;
- a test that passes on the original code is rejected;
- a wrong fix fails, and a fix that breaks a module test fails;
- an untracked new file counts as part of the change;
- no git, a missing test and a path outside the project are named;
- best-of-N with 6 fixed candidates (one too big, one wrong, one that does not apply, a one-line `sitecustomize.py`, one outside the allowed files, one small and right) picks the small right one, refuses the `sitecustomize.py` and the outside change with their reasons, and writes only the winner's diff;
- a new `sitecustomize.py` in the working tree is refused;
- ignored `*.so` files and `node_modules` come into the worktree, are no change in the diff, and the user's post-checkout hook does not run;
- the time budget ends a check with an error, and a hanging test is killed in under 5 s at a 1 s limit;
- the editable-install order, run both ways (Python);
- a test that cannot import gives `env_error` (Python);
- a command off the safe list does not run without a yes, a declined one says so, the default interpreter run asks, and `run_policy` `all` passes;
- a pass recorded for one task does not silence the reminder of another;
- the reminder comes once after a code edit, never after a Markdown edit, never for a worker, and not after a check in the same turn;
- the decision rule, `env_error` detection, block parsing (a second block keeps the path, CRLF stays, an empty SEARCH never appends), the cut of a long file, the candidate order, word splitting, the log counts per repository and the repair count, and both hit formulas against simulations.

The fixture tests use `git grep` as the test command, which exists on every CI runner. No model call runs in the tests.
