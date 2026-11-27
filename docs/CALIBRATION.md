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

The claude command runs its own tool loop over MCP, so `claude-cli` models get only `effort-low` and `prompt-short`.

The winner: a variant replaces the default with more passed checks, or with equal passes at 80 % of the default's
tokens or less. Every other result keeps the default.

## Profile

The profile lives in `.penelope/models/<provider>__<model>.json`, for example `opencode__glm-5.3.json`:

```json
{"variant": "index", "tool_format": "native", "tool_list": "index", "effort": "high", "prompt": "current",
 "tasks": "cli/tests/data/eval/hard_calib.jsonl", "tasks_sha256": "...", "n": 20, "scores": [...]}
```

`calibrate::apply` runs at the start of `Chat::turn_once`. It copies `tool_format`, `tool_list`, `prompt` and `effort`
into the main model setting of that turn. `"calibration": "off"` in config.json switches the profile off.

## Runner

The runner does not use `eval.rs`. Every task run gets its own folder under `--out`:

- `work/`: a copy of the setup repo, the model's working folder.
- `.penelope/`: the config (model under test as main and weak model, `edit_policy` and `run_policy` `all`, bwrap
  sandbox, no learning, no scout), the variant as profile file, and `usage.json` of that run.

The turn runs in a child process, `penelope calibrate-one`, in its own process group. A turn over 600 s or a check over
its `check_timeout_seconds` loses the whole group. At most 3 runs go in parallel. Each finished run is one line in
`results.jsonl`; a second call skips runs it finds there. Tokens come from the run's `usage.json`, euro from the prices
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

Pending.
