# Self-calibration per model

`penelope calibrate MODEL` runs a task set in several variants and stores the variant with the most passed checks as
the model's profile. Penelope reads the profile before every turn on that model.

```bash
penelope calibrate glm-5.3 --provider opencode --tasks cli/tests/data/eval/hard_calib.jsonl --n 20
penelope calibrate glm-5.3 --provider opencode --tasks cli/tests/data/eval/hard_holdout.jsonl --n 30 --validate
```

## Variants

Each variant changes one factor of the default.

| Variant | Change | Applies to |
|---|---|---|
| `default` | native tool calls, full tool list, the model's default effort, the 11 rules of `backend::rules` | every model |
| `text` | tool calls as `<tool_call>` text blocks; the tool list moves into the system prompt, the API tool field stays empty, results return as one user message of `<tool_response>` blocks | HTTP models |
| `index` | the Mitos index of the tool list (name, first sentence, empty schema) plus `tool_describe`, which returns full schemas | HTTP models |
| `effort-low` | effort `low` | models whose default effort is `high` and that have `low` |
| `prompt-short` | a 6-line system prompt | every model |
| `rounds-24` | 24 tool-call rounds per turn in place of 12; added after the experiment below, not part of it | HTTP models |

The claude command runs its own tool loop over MCP, so `claude-cli` models get only `effort-low` and `prompt-short`.

The winner: a variant replaces the default with more passed checks, or with equal passes at 80 % of the default's
tokens or less. Every other result keeps the default.

## Profile

The profile lives in `.penelope/models/<provider>__<model>.json`, for example `opencode__glm-5.3.json`:

```json
{"variant": "index", "tool_format": "native", "tool_list": "index", "effort": "high", "prompt": "current",
 "tasks": "cli/tests/data/eval/hard_calib.jsonl", "tasks_sha256": "...", "n": 20, "scores": [...]}
```

`calibrate::apply` (core/src/calibrate.rs) runs at the start of `Chat::turn_once`. It copies `tool_format`, `tool_list`, `prompt` and `effort`
into the main model setting of that turn. `"calibration": "off"` in config.json switches the profile off.

## Runner

The runner does not use `eval.rs`. Every task run gets its own folder under `--out`:

- `work/`: a copy of the setup repo, the model's working folder.
- `.penelope/`: the config (model under test as main and weak model, `edit_policy` and `run_policy` `all`, bwrap
  sandbox, no learning, no scout), the variant as profile file, and `usage.json` of that run.

The turn runs in a child process, `penelope calibrate-one`, in its own process group. A turn over 600 s or a check over
its `check_timeout_seconds` loses the whole group. At most 3 runs go in parallel. Each finished run is one line in
`results.jsonl`; a second call skips runs it finds there. A run that ends on status 429 starts again from a fresh
copy, up to 3 tries; a usage window that names its reset ("resets in 34m") is waited out. Tokens come from the run's `usage.json`, euro from the prices
in config.json.

## Task sets

The easy sets `tasks50.jsonl` and `holdout50.jsonl` are saturated. The default on the first 10 tasks of `tasks50.jsonl`:

| Model | Passed | Tokens in | Tokens out | Wall |
|---|---|---|---|---|
| deepseek-v4.1-flash | 10/10 | 371,821 | 5,181 | 94 s |
| glm-5.3 | 9/10 | 347,830 | 3,396 | 309 s |
| kimi-k3 | 10/10 | 438,999 | 14,344 | 384 s |
| minimax-m3 | 10/10 | 942,154 | 24,800 | 563 s |
| haiku (claude-cli) | 10/10 | 138,086 | 5,326 | 90 s |

49 of 50 passed. The set cannot show a difference, so `make_hard_tasks.py` writes 50 harder tasks on `hard_repo/`.
Ten families of five tasks: bug fixes against a test, renames across 4 to 6 files, three JSON edits at once, CSV
sums, a search over 40 JSON files, counts over 4 log files, a new CLI command, a new module with its own test and a
registry entry, pipelines with one bug in each of two files, and Markdown edits outside code blocks. Tasks 1 and 2 of
each family form `hard_calib.jsonl` (20 tasks). Tasks 3 to 5 form `hard_holdout.jsonl` (30 tasks).

Every check holds a SHA-256 of the expected result, never the result itself, because the model sees the check command.
`python3 make_hard_tasks.py --verify DIR` runs every check on an untouched copy (50 of 50 fail) and on the reference
solution (50 of 50 pass).

Frozen before the first variant run, 2026-11-27: `cli/tests/data/eval/hard.sha256` lists all 84 files
(`sha256sum -c hard.sha256` in that folder). The file itself has SHA-256
`409b276ad9c329a4300e5fe302ddfd1ec9cf0cfda6ed50d8f5faeb2a3564257e`.

- `hard_calib.jsonl`: `f7a667e84741d671099af4adcee2774defe0fe1593c33b9a0138a7f2733506c7`
- `hard_holdout.jsonl`: `1f33bfdcac006bafd46a55af4e921374d980cb306415e00e1959dfc7d2dd6443`

## Experiment

Models: OpenCode Go `deepseek-v4.1-flash`, `glm-5.3`, `kimi-k3`, `minimax-m3`, and `haiku` through the claude command.

1. Calibrate: all variants of the model on the 20 tasks of `hard_calib.jsonl`. The winner becomes the profile.
2. Validate: `default` and `calibrated` on the 30 tasks of `hard_holdout.jsonl`, paired per task. When the winner is
   the default, both arms run the same setting; their difference measures run-to-run noise.

b counts the tasks only the calibrated arm passed. c counts the tasks only the default passed. The test is the exact
two-sided McNemar test per model and over all 150 pooled pairs.

Hypotheses, written before the first variant run:

- H1: the calibrated profile solves at least 3 more of the 30 held-out tasks than the default (b minus c at least 3)
  for at least 2 of the 5 models.
- H2: no model gets worse by more than 1 task (c minus b at most 1 for every model).
- H3: at least one model's calibration picks a tool format other than native (the `text` variant wins).

Prices in USD per million tokens, input and output, at 0.92 EUR per USD: deepseek-v4.1 0.15 and 0.60, glm-5.3 0.20 and
0.80, kimi-k3 0.60 and 2.50, minimax-m3 0.30 and 1.20 (an assumption, no list price found), Claude Haiku 5.5 0.10 and
0.50. The EUR are computed list prices: OpenCode Go and the Claude subscription bill no tokens. The claude command
reports input tokens without cache reads; HTTP models report every input token. Token counts compare within one model.

## Results

Runs: 2026-11-27, 17:53 to 22:43. 570 recorded task runs on the hard sets over the four models below, plus 50 screening runs.

The answer: calibration did not solve more held-out tasks. H1, H2 and H3 are rejected. Low effort kept the pass rate on
DeepSeek and GLM within 1 task at 12 % to 22 % fewer euro and 14 % to 40 % less wall time.

### Calibration, 20 tasks of `hard_calib.jsonl`

| Model | default | text | index | effort-low | prompt-short | Chosen |
|---|---|---|---|---|---|---|
| deepseek-v4.1-flash | 17 | 0 | 12 | 18 | 17 | effort-low (more passes) |
| glm-5.3 | 19 | 0 | 13 | 19 | 18 | default (effort-low at 80.5 % of the tokens, the limit is 80 %) |
| kimi-k3 | 20 | 5 | 16 | no levels | 18 | default |
| haiku (claude-cli) | 20 | n/a | n/a | 18 | 20 | default |

Tokens in and out, EUR and wall time of the calibration runs:

| Model | Variant | Tokens in | Tokens out | EUR | Wall s | Runs at the 12-round cap |
|---|---|---|---|---|---|---|
| deepseek-v4.1-flash | default | 2,590,786 | 80,247 | 0.4018 | 1169 | 13 |
| deepseek-v4.1-flash | text | 166,814 | 2,150 | 0.0242 | 45 | 0 |
| deepseek-v4.1-flash | index | 1,278,451 | 52,775 | 0.2056 | 972 | 20 |
| deepseek-v4.1-flash | effort-low | 2,326,240 | 55,669 | 0.3518 | 911 | 9 |
| deepseek-v4.1-flash | prompt-short | 2,443,396 | 66,890 | 0.3741 | 982 | 13 |
| glm-5.3 | default | 1,986,722 | 45,767 | 0.3992 | 1337 | 12 |
| glm-5.3 | text | 144,293 | 1,034 | 0.0273 | 129 | 0 |
| glm-5.3 | index | 937,241 | 39,217 | 0.2013 | 1357 | 14 |
| glm-5.3 | effort-low | 1,612,418 | 23,135 | 0.3137 | 618 | 8 |
| glm-5.3 | prompt-short | 2,031,693 | 48,344 | 0.4094 | 1229 | 14 |
| kimi-k3 | default | 2,789,381 | 117,401 | 1.8098 | 1890 | 15 |
| kimi-k3 | text | 633,217 | 27,043 | 0.4117 | 425 | 0 |
| kimi-k3 | index | 1,482,168 | 78,854 | 0.9995 | 1335 | 15 |
| kimi-k3 | prompt-short | 2,214,203 | 83,540 | 1.4144 | 1457 | 9 |
| haiku | default | 281,713 | 57,055 | 0.0522 | 522 | no cap |
| haiku | effort-low | 256,848 | 37,168 | 0.0407 | 390 | no cap |
| haiku | prompt-short | 275,355 | 59,264 | 0.0526 | 457 | no cap |

### Validation, held-out tasks of `hard_holdout.jsonl`, paired

| Model | Calibrated arm | Pairs | Default passed | Calibrated passed | b | c | p | Tokens default | Tokens calibrated | EUR default | EUR calibrated | Wall default s | Wall calibrated s |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| deepseek-v4.1-flash | effort-low | 30 | 25 | 26 | 4 | 3 | 1.000 | 4,033,955 | 3,573,514 | 0.5995 | 0.5260 | 1605 | 1387 |
| glm-5.3 | effort-low (see below) | 25 | 23 | 22 | 2 | 3 | 1.000 | 2,764,605 | 2,213,957 | 0.5491 | 0.4264 | 1529 | 910 |
| kimi-k3 | default (same setting twice) | 28 | 26 | 24 | 1 | 3 | 0.625 | 3,850,633 | 3,812,395 | 2.4018 | 2.3463 | 2438 | 2177 |
| haiku | default (same setting twice) | 30 | 29 | 30 | 1 | 0 | 1.000 | 547,323 | 530,814 | 0.0912 | 0.0862 | 881 | 869 |
| pooled | | 113 | | | 8 | 9 | 1.000 | | | | | | |

- H1 (at least 3 more of 30 for 2 models): rejected. The largest gain is DeepSeek with b minus c = 1.
- H2 (no model worse by more than 1): rejected on Kimi, c minus b = 2. Both Kimi arms ran the same default setting, so
  run-to-run noise alone spans 2 tasks of 28. Haiku's identical arms differ by 1.
- H3 (another tool format wins for one model): rejected. `text` passed 0, 0 and 5 of 20.

### Deviations

- OpenCode Go answered 429 on 44 GLM and Kimi runs. Those rows were dropped and run again; the runner now does this by
  itself. GLM's first selection (effort-low, at 79 % of the tokens) included 5 rate-limited rows. After the rerun,
  effort-low sat at 80.5 % and the default won. The GLM row above is the effort-low hold-out arm from the first
  selection. GLM's default-against-default arm did not run.
- The OpenCode Go 5-hour usage limit ran out at 21:25 and again at 22:40 (reset 03:08). 5 GLM default runs and 2 Kimi
  pairs are missing for that reason.
- minimax-m3 is dropped: none of its 140 runs fit into the usage window. Its default passed 10 of 10 easy tasks and hit
  the 12-round cap in 3 of them.
- The DeepSeek runs used the binary without the DSML parser. The later runs had it; GLM, Kimi and Haiku never write DSML.

### What the runs show

1. The 12-round cap decides most failures. Of 361 HTTP runs outside `text`, 235 hit the cap; 49 of them failed. Of the
   126 runs under the cap, 1 failed. Haiku through the claude command has no cap and passed 20 of 20.
2. Models answer the text format in their own template. DeepSeek wrote DSML blocks
   (`<｜DSML｜ invoke name="run">`) in 20 of 20 first answers. GLM wrote its `<arg_key>`/`<arg_value>` template in 20
   of 20, 7 of them well formed and 13 mixed with JSON. Neither used the requested `<tool_call>` JSON block, which
   matches the Qwen observation in the lab. Kimi used the JSON block in some runs and passed 5 of 20.
3. The Mitos index halves the tokens and loses 4 to 7 of 20 tasks: every `tool_describe` call costs a round under the cap.
4. A short prompt changes nothing measurable: 17, 18 and 18 against 17, 19 and 20.
5. Low effort is a cost setting: equal passes or 1 more on DeepSeek and GLM, 18 of 20 on Haiku against 20.
6. DeepSeek tried to reverse the SHA-256 in a check: it wrote `hashprobe.py` in `hard-cli-regions`. The probe costs rounds.

### Next ideas, each with its test

- `rounds-24`: the cap as a sixth factor (`"max_rounds"` in the profile, read in `turn_api`). Unit test
  `a_saved_profile_loads_and_changes_the_main_setting`. Run after the reset:
  `penelope calibrate deepseek-v4.1-flash --provider opencode --tasks cli/tests/data/eval/hard_calib.jsonl --n 20 --variants rounds-24`,
  paired against the default rows above. Prediction: at least 2 of the 4 default failures of DeepSeek (3) and GLM (1) pass.
- Native text templates: `llm::text_tool_calls` now reads DSML and GLM's `<arg_key>` blocks. Tests
  `deepseek_dsml_blocks_become_calls` and `glm_arg_key_blocks_become_calls` use answers from these runs. On the
  stored first answers, calls parse in 20 of 20 DeepSeek answers (before: 0) and 7 of 20 GLM answers (before: 0). The
  run of the `text` variant with these parsers is open.
- Selection rule: the 80 % token limit flipped GLM on 0.5 points. A rule on euro per passed task, with a margin of one
  task, is the next candidate; it needs a second calibration run per model to measure its noise.
