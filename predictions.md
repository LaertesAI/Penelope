# Predictions and freeze: cluster loop

Started 2026-11-26 on `feat/cluster-loop` from Penelope `main` `4389032`. This file is written before every change.
The loop follows `intern/DeepSeek-Auftrag Cluster-Loop.md`. All numbers are measurements from the G2 runs or are
derived with python3.

## Hold-out freeze

The 50-task hold-out is `cli/tests/data/eval/holdout50.jsonl` with the repo `cli/tests/data/eval/holdout_repo/`.
It holds 50 tasks of other kinds than the 50 development tasks: merge, replace, extract, reverse, dedupe, number,
nested JSON, CSV, averages, word counts, folders, moves, splits, compare, file counts and headers. All 50 checks
pass against deterministic reference solutions (local verifier, no model). The loop does not run it. It runs once
at the end, large and cluster.

- tasks sha256: `ce9c73a5f2eb05b21f55864d267d26534b6bdb278b8586415de138a98f4569d2`
- repo sha256: `c3224aba7eeb471f6512c30d072bf547a11bde1ed5c4f9c42f184df566b1c830`

The repo hash is SHA-256 over sorted relative path, NUL, file bytes, NUL for every file. The test
`the_holdout_is_frozen` recomputes both hashes.

## Step 0: baseline G2

- large 35/50, 1.2187 EUR, 446.8 s. cluster 39/50, 1.0265 EUR, 905.1 s.
- Paired large against cluster: b = 4 (cluster only: script-1, script-2, script-4, script-5), c = 0,
  psi = 0.08, exact McNemar two-sided p = 0.125.
- Failure causes: 10 of 11 are one cause. The shipped `run_policy: safe` refuses `mv` and `rm`; the MCP
  `rename_file` is not granted. 1 of 11 is a model fallback error: script-3 gave up when `run` refused, while
  script-1, 2, 4 and 5 wrote the computed output.
- Noise: 10 tasks, 3 repeats each, 0 flips. The same 5 pass and 5 fail in all three runs. The smallest real
  improvement is 2 tasks (the brief's floor; twice the mean flip count is 0).
- Time: 50 clusters, 903 s in the report streams. Worker 534 s (59.1 %), validator 369 s (40.9 %),
  orchestration 0 s. Mean 18.06 s per cluster.
- Cost: 800,191 in and 118,906 out, 1.0265 EUR, 100 part calls. The validator writes 47.1 % of the output
  characters and takes 40.9 % of the time; estimated share 41 % of EUR.

## Round 1

Hypothesis, written before the change: the dominant cause is the shipped `run_policy: safe`, which refuses the
commands that rename and delete. An eval is unattended and runs in a scratch copy. Setting `run_policy: all` and
`sandbox: bwrap` for every eval setup lets both setups act, keeps the rest of the machine read-only and cuts no
network. Expected: cluster +10 tasks (the five rename and five delete tasks), large +10 to +15, EUR and time within
10 %. The change is one block in `eval_context`, not a rule about a task.

Keep rule: hits rise by at least the noise threshold, EUR and time rise by at most 10 %, or hits stay equal and
EUR or time fall by at least 15 %.

## Round 2

Hypothesis, written before the change: the validator takes 40.9 % of the cluster time and about 41 % of its euro.
A profile with one worker and no retry cannot use a validator to correct anything: a FAIL is final. Skipping that
validator and letting the worker's own check be the evidence keeps the hits and cuts EUR and time by about 40 %.
Expected: cluster 50/50, about 0.50 EUR, about 535 s. The change is one profile flag (`single_model`) and one branch
in `run_cluster_inner`; the eval profile sets the flag, the built-in profiles keep the validator.

Keep rule: hits stay equal and EUR or time falls by at least 15 %, or hits rise by at least the noise threshold.

## Round 1 result

large 50/50, 0.9844 EUR, 307.8 s. cluster 50/50, 0.8451 EUR, 818.9 s. Paired b = 0, c = 0, p = 1.
Kept: hits +11 against G2 (39 to 50), EUR -17.7 %, time -9.5 %. The target 45/50 and EUR at most large are met;
the time 818.9 s is above 581 s, so round 2 must cut the time.

## Round 2 result

cluster 50/50, 0.5119 EUR, 372.4 s. large stays 50/50, 0.9844 EUR, 307.8 s.
Kept: hits equal, EUR -39.4 %, time -54.5 %. The target is met: 50 at least 45, EUR 0.5119 at most 0.9844,
time 372.4 s at most 1.3 times large (400.1 s) and below the brief's 581 s. The loop stops after 2 rounds on success.

## Hold-out

The hold-out hashes are unchanged. It runs once after the loop stopped, large and cluster, with the same binary.
The result is added here after the run.

Hold-out result: large 50/50, 1.1096 EUR, 345.0 s. cluster 50/50, 0.5408 EUR, 548.9 s.
b = 0, c = 0, exact McNemar p = 1.0 on the hold-out and on all 100 pairs. The cluster is 51.3 % cheaper and
1.59 times slower. A hit lead is not proven. Both 50-task sets are saturated for these two setups.

## Next set: SWE-bench Lite, 50 tasks

Selection, before any run: all 300 test rows of `princeton-nlp/SWE-bench_Lite`, sorted by SHA-256 of `instance_id`,
first 50. The hash of the selected list is `ef59b9f230078a2efd5c554c576b4ead3ed27a55272afb1c31f721bba67dbf77`.
Repos: django 15, sympy 14, matplotlib 4, sphinx 4, pylint 3, scikit-learn 3, pytest 2, xarray 2, seaborn 2,
requests 1.

Check: the model sees the issue text and the repo at `base_commit`, not the test patch. The check applies the
instance's `test_patch` and runs every `FAIL_TO_PASS` and every `PASS_TO_PASS` test in a prebuilt environment. A task
is resolved only when all of them pass. An environment that cannot start is `infra`, not a model fail, and is
reported separately.

Prediction, before any run:

- large: 12 of 50 (24 %).
- cluster: 4 of 50 (8 %).
- Paired: large wins 8, cluster wins 0, exact McNemar p = 0.0078.
- EUR: large about 12.5, cluster about 6.0. The repo contexts and the many steps make a task 5 to 10 times as
  expensive as a file task. The estimate is rough and needs an owner's budget approval.
- Time: large about 6,000 s, cluster about 7,500 s.
- 2 to 4 tasks fail on the environment, not the model.
- The cluster will be cheaper, not better. The gap of 8 points is the result the set is built to show.

Risks: the harness is not a full SWE-bench agent. The resolved rate can be lower for both. If the environment fails
on more than 10 tasks, stop and fix the harness before scoring.
