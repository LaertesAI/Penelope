# Self-calibration per model

`penelope calibrate MODEL` runs a task set in several variants and stores the winning variant as the model's profile.
Penelope reads the profile before every turn on that model. On the four models measured here, the selection rule keeps
the default for every model. Two interventions after the review moved
pass counts. 24 rounds in place of 12 lifted DeepSeek from 42 to 47 of 50 tasks (b = 7, c = 2, p = 0.18) at 69 % more
euro. A tolerant reader for DeepSeek's own DSML template lifted the text tool format from 9 to 17 of 20 (b = 9, c = 1,
p = 0.021); on the 30 held-out tasks text passed 27 against the default's 26.

```bash
penelope calibrate glm-5.3 --provider opencode --tasks cli/tests/data/eval/hard_calib.jsonl --n 20
penelope calibrate glm-5.3 --provider opencode --tasks cli/tests/data/eval/hard_holdout.jsonl --validate
```

`--n` defaults to 20 for a calibration and to the whole file under `--validate`. A cut file prints
`using the first N of the M tasks`. `--variants A,B` runs a subset. `--no-sandbox` runs without bubblewrap.

## Variants

Each variant changes one factor of the default.

| Variant | Change | Applies to |
|---|---|---|
| `default` | native tool calls, full tool list, the model's default effort, the 11 rules of `backend::rules`, the task's `max_steps` as round cap | every model |
| `text` | tool calls as text blocks; the tool list moves into the system prompt, the API tool field stays empty, results return as one user message of `<tool_response>` blocks. The parser reads Hermes `<tool_call>` JSON, GLM's `<arg_key>` pairs and DeepSeek's DSML | HTTP models |
| `index` | the Mitos index of the tool list (name, first sentence, empty schema) plus `tool_describe`, which returns full schemas | HTTP models |
| `effort-low` | effort `low` | models whose default effort is `high` and that have `low` |
| `prompt-short` | a 6-line system prompt; line 2 is rule 2 of `backend::rules` for the `delegate` setting | every model |
| `rounds-24` | 24 tool-call rounds per turn in place of the task's `max_steps` (12 on the hard set) | HTTP models |

The claude command runs its own tool loop over MCP, so `claude-cli` models get only `effort-low` and `prompt-short`.
Their turns have no round cap.

`stop-green` (HTTP models, `--variants default,stop-green` only) changes one more factor. A calibration over the full
set leaves it out and no profile stores it, because the stop needs the task's own check. After every round in which a
file below the working folder changed (size or modification time, `.git` and caches left out), the runner runs the task
check in bubblewrap. When it passes, the turn ends with no further model call. The check count and the stop are
written to `stop-green.json` in the run's `.penelope/`. User tasks have no such check, so the variant measures a
ceiling, not a feature.

## Selection rule

A variant replaces the default in two cases:

- It passes at least `aa_band(n)` more tasks than the default. `aa_band` is the smallest gain that two runs of the same
  setting reach with at most 5 % chance, when 14 % of the tasks differ between them (Kimi's A/A rate, 4 of 28). That
  is 4 of 20 and 4 of 30.
- It passes no fewer tasks at 80 % of the default's tokens or less.

Of several, the most passes win, then the fewest tokens. The first rule took a gain of 1 task. Under it DeepSeek picked
`effort-low` on 18 against 17, a gain inside the noise of identical runs.

## Profile

The profile lives in `.penelope/models/<provider>__<model>.json`, for example `opencode__glm-5.3.json`:

```json
{"variant": "rounds-24", "tool_format": "native", "tool_list": "full", "effort": "high", "prompt": "current", "rounds": 24,
 "tasks": "cli/tests/data/eval/hard_calib.jsonl", "tasks_sha256": "...", "n": 20,
 "paired": {"b": 3, "c": 0, "aa_band": 4, "aa_discordance": 0.14}, "scores": [...]}
```

A calibration writes the profile only when the run held `default` and every variant that applies to the model, each
on every task. `--variants text,index` or a run with rows left out writes nothing.

`calibrate::apply` (core/src/calibrate.rs) runs at the start of `Chat::turn_once`. It copies `tool_format`,
`tool_list`, `prompt` and `rounds` (as `max_rounds`) into the main model setting of that turn. It sets `effort` only
while the main setting holds no effort or the provider's default. An effort from auto routing (`haiku low`), a work
mode or the user stays. `"calibration": "off"` in config.json switches the profile off.

All calibration runs use `delegate` `off`. A profile chosen there is not tested under the default `delegate` `auto`.

## Runner

The runner does not use `eval.rs`. Every task run gets its own folder under `--out`:

- `work/`: a copy of the setup repo, the model's working folder.
- `.penelope/`: the config (model under test as main and weak model, `edit_policy` and `run_policy` `all`, bwrap
  sandbox, no learning, no scout, the task's `max_steps` as `max_rounds`), the variant as profile file, `usage.json`,
  `turn.json` (the first 4,000 characters of the answer) and `history.json` (every message of the turn; empty for the
  claude command, which keeps its own history).

The turn runs in a child process, `penelope calibrate-one`, in its own process group. Over 600 s the runner kills
every process below the child, also those in a group of their own: the claude command and its MCP servers, and
background jobs. The check runs in bubblewrap with a read-only root, only `work/` writable, a private `/tmp` and no
network, because it runs code the model wrote. Without bubblewrap (macOS, Windows, Linux without `bwrap`) the command
refuses to start unless `--no-sandbox` is given. A check over its `check_timeout_seconds` loses its whole group.

At most 3 runs go in parallel. A lock on `--out` keeps a second calibrate process out. Each finished run is one line
in `results.jsonl`; a second call skips runs it finds there. A run that ends on the provider (status 429, 5xx, `no
answer from`) starts again from a fresh copy, up to 3 tries, and then writes no line, so the next call runs it again.
A usage window that names its reset ("resets in 34m", "resets in 45s", "resets at 03:08") is waited out, at most 5 hours.
Tokens come from the run's `usage.json`, euro from the prices in config.json.

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
each family form `hard_calib.jsonl` (20 tasks). Tasks 3 to 5 form `hard_holdout.jsonl` (30 tasks). Both sets share
the 10 families, so the hold-out is an in-distribution test, and the family clustering lowers the effective n below 30.

Every check holds a SHA-256 of the expected result, never the result itself, because the model sees the check command.
`python3 make_hard_tasks.py --verify DIR` runs every check on an untouched copy (50 of 50 fail) and on the reference
solution (50 of 50 pass).

Frozen before the first variant run, 2026-11-27: `cli/tests/data/eval/hard.sha256` lists all 84 files
(`sha256sum -c hard.sha256` in that folder). The file itself has SHA-256
`409b276ad9c329a4300e5fe302ddfd1ec9cf0cfda6ed50d8f5faeb2a3564257e`. `.gitattributes` marks the set `-text`, so Git
for Windows keeps LF; the test `the_frozen_hard_set_hashes_on_every_system` hashes all 84 files on every CI system.
Its first Windows run found `make_hard_tasks.py` converted to CRLF.

- `hard_calib.jsonl`: `f7a667e84741d671099af4adcee2774defe0fe1593c33b9a0138a7f2733506c7`
- `hard_holdout.jsonl`: `1f33bfdcac006bafd46a55af4e921374d980cb306415e00e1959dfc7d2dd6443`

## Experiment 1, 2026-11-27, 17:53 to 22:43

Models: OpenCode Go `deepseek-v4.1-flash`, `glm-5.3`, `kimi-k3`, `minimax-m3`, and `haiku` through the claude command.

1. Calibrate: `default`, `text`, `index`, `effort-low` and `prompt-short` on the 20 tasks of `hard_calib.jsonl`. The
   winner (first rule: one more pass) becomes the profile.
2. Validate: `default` and `calibrated` on the 30 tasks of `hard_holdout.jsonl`, paired per task. When the winner is
   the default, both arms run the same setting; their difference measures run-to-run noise (A/A).

b counts the tasks only the calibrated arm passed. c counts the tasks only the default passed. The test is the exact
two-sided McNemar test per model.

Hypotheses, written before the first variant run:

- H1: the calibrated profile solves at least 3 more of the 30 held-out tasks than the default (b minus c at least 3)
  for at least 2 of the 5 models.
- H2: no model gets worse by more than 1 task (c minus b at most 1 for every model).
- H3: at least one model's calibration picks a tool format other than native (the `text` variant wins).

Prices in USD per million tokens, input and output, at 0.92 EUR per USD: deepseek-v4.1 0.15 and 0.60, glm-5.3 0.20 and
0.80, kimi-k3 0.60 and 2.50, minimax-m3 0.30 and 1.20 (an assumption, no list price found), Claude Haiku 5.5 0.10 and
0.50. The EUR are computed list prices: OpenCode Go and the Claude subscription bill no tokens. The claude command
reports input tokens without cache reads; HTTP models report every input token. Token counts compare within one model.

594 recorded task runs on the hard sets over four models, plus 50 screening runs. A second check recounted all 595
rows before the cleanup: pass counts, b, c, p, tokens, EUR and wall time match. 594 rows match their folder's
`usage.json` token for token. All 540 rows with a run folder pass or fail the same with the check rerun in bubblewrap.

### Calibration, 20 tasks of `hard_calib.jsonl`

| Model | default | text | index | effort-low | prompt-short | First rule chose | Current rule chooses |
|---|---|---|---|---|---|---|---|
| deepseek-v4.1-flash | 17 | 0 | 12 | 18 | 17 | effort-low (+1) | default |
| glm-5.3 | 19 | 0 | 14 | 19 | 18 | default (effort-low at 80.5 % of the tokens) | default |
| kimi-k3 | 20 | 5 | 16 | no levels | 18 | default | default |
| haiku (claude-cli) | 20 | n/a | n/a | 18 | 20 | default | default |

GLM's index row ended once on the provider (`hard-rename-format-money`, 13 of 20 with that row as a failure). The
row was removed and the task ran again on 2026-11-28 with the current binary; it passed, so GLM index is 14 of 20.

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
| glm-5.3 | index | 988,587 | 40,119 | 0.2114 | 1056 | 15 |
| glm-5.3 | effort-low | 1,612,418 | 23,135 | 0.3137 | 618 | 8 |
| glm-5.3 | prompt-short | 2,031,693 | 48,344 | 0.4094 | 1229 | 14 |
| kimi-k3 | default | 2,789,381 | 117,401 | 1.8098 | 1890 | 15 |
| kimi-k3 | text | 633,217 | 27,043 | 0.4117 | 425 | 0 |
| kimi-k3 | index | 1,482,168 | 78,854 | 0.9995 | 1335 | 15 |
| kimi-k3 | prompt-short | 2,214,203 | 83,540 | 1.4144 | 1457 | 9 |
| haiku | default | 281,713 | 57,055 | 0.0522 | 522 | no cap |
| haiku | effort-low | 256,848 | 37,168 | 0.0407 | 390 | no cap |
| haiku | prompt-short | 275,355 | 59,264 | 0.0526 | 457 | no cap |

The GLM index row holds the refilled task. The first run's 1357 s included the provider failure.

### Validation, held-out tasks of `hard_holdout.jsonl`, paired

| Model | Calibrated arm | Pairs | Default passed | Calibrated passed | b | c | p | Tokens default | Tokens calibrated | EUR default | EUR calibrated | Wall default s | Wall calibrated s |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| deepseek-v4.1-flash | effort-low | 30 | 25 | 26 | 4 | 3 | 1.000 | 4,033,955 | 3,573,514 | 0.5995 | 0.5260 | 1605 | 1387 |
| glm-5.3 | effort-low (see below) | 25 | 23 | 22 | 2 | 3 | 1.000 | 2,764,605 | 2,213,957 | 0.5491 | 0.4264 | 1529 | 910 |
| kimi-k3 | default (A/A) | 28 | 26 | 24 | 1 | 3 | 0.625 | 3,850,633 | 3,812,395 | 2.4018 | 2.3463 | 2438 | 2177 |
| haiku | default (A/A) | 30 | 29 | 30 | 1 | 0 | 1.000 | 547,323 | 530,814 | 0.0912 | 0.0862 | 881 | 869 |
| pooled, all 4 | | 113 | | | 8 | 9 | 1.000 | | | | | | |
| pooled, DeepSeek and GLM | | 55 | | | 6 | 6 | 1.000 | | | | | | |

The pooled test belongs over the models whose arms differ. Kimi's and Haiku's arms ran the same setting, so their
5 discordant pairs are noise. DeepSeek and GLM give b = 6, c = 6 over 55 pairs, p = 1.000, pass difference
0 ± 12.3 points (95 %).

- H1 (at least 3 more of 30 for 2 models): rejected. The largest gain is DeepSeek with b minus c = 1. The test had no
  power: Haiku's default passed 29 of 30 and Kimi's 26 of 28, so only DeepSeek and GLM had room for +3. At DeepSeek's
  discordance (7 of 30) a true gain of 10 points has 9.8 % power at n = 30; 80 % needs 193 pairs.
- H2 (no model worse by more than 1): rejected on Kimi, c minus b = 2. Both Kimi arms ran the same default setting.
  Under A/A at 14 % discordance, c minus b of 2 or more has a chance of 23 % per model and of 65 % in at least 1 of 4
  models. The rejection measures noise.
- H3 (another tool format wins for one model): rejected. `text` passed 0, 0 and 5 of 20 against native 17, 19 and 20.
  The run tested the old parser: DeepSeek wrote DSML in 20 of 20 answers and the parser read none of them.

### Deviations

- OpenCode Go answered 429 on 44 GLM and Kimi runs. Those rows were deleted by hand and run again. The runner now
  writes no row for a provider failure. GLM's first selection (effort-low, at 79 % of the tokens) included 5
  rate-limited rows. After the rerun, effort-low sat at 80.5 % and the default won. The GLM hold-out row above is the
  effort-low arm from the first selection, so it tests a profile from contaminated calibration data.
- GLM's hold-out arms did not alternate: all 29 effort-low runs ran first, the 25 default runs came from a separate
  batch. GLM's wall-time difference is therefore confounded with time of day and provider load. DeepSeek and Kimi
  alternated the arms.
- `exp/opencode__glm-5.3/validate-hard_holdout/results.jsonl` repeats the 25 default rows of `holdout-effort-low`.
  Counts across files must take them once.
- One GLM calibration row (index, `hard-rename-format-money`) ended on `no answer from opencode.ai` and had no run
  folder. It counted as a failure (13 of 20). It is now removed and run again (note under the calibration table).
- The OpenCode Go 5-hour usage limit ran out at 21:25 and again at 22:40 (reset 03:08). 5 GLM default runs and 2 Kimi
  pairs are missing for that reason.
- minimax-m3 is dropped: none of its 140 runs fit into the usage window. Its default passed 10 of 10 easy tasks and hit
  the 12-round cap in 3 of them.
- The DeepSeek runs used the binary without the DSML parser. GLM, Kimi and Haiku never write DSML.
- `task.max_steps` was ignored; the cap was the built-in 12. Every hard task sets `max_steps` 12, so the runs are not
  affected. The runner now uses `max_steps`.

### What the runs show

1. Cap hits go with failures. Over the 389 unique HTTP runs outside `text`, 247 hit the 12-round cap and 52 of those
   failed; 195 cap hits passed. The 142 runs under the cap all passed. A cap hit is necessary for a failure here, not
   sufficient. Hard tasks can drive both. The comparison with Haiku (no cap, 20 of 20) mixes model and cap. Only the
   `rounds-24` run below tests the cap.
2. Models answer the text format in their own template. DeepSeek wrote DSML blocks (`<｜DSML｜ invoke name="run">`) in
   20 of 20 stored answers and no Hermes block. GLM wrote `<arg_key>`/`<arg_value>` in 17 of 20; the other 3 use
   `run command="..."` attributes or a broken pair. 7 of 20 GLM answers parse with the new parser. Kimi used the
   `<tool_call>` JSON block in 5 of 20 and passed those 5.
3. The Mitos index cuts tokens by 46 to 50 % and loses 4 to 5 of 20 tasks: DeepSeek 17 to 12, GLM 19 to 14, Kimi 20 to
   16. DeepSeek's index runs hit the cap in 20 of 20. That `tool_describe` calls cost the rounds is likely; experiment 1
   kept no transcript to count them.
4. `prompt-short` shows no difference that n = 20 can detect: 17, 18 and 18 against 17, 19 and 20, Kimi 2 fewer. The
   first short prompt also dropped rule 2 (where lookups go); it now carries the same line as the full rules.
5. Low effort is a cost setting. On the hold-out DeepSeek's EUR fell 12.3 % (bootstrap 95 % CI −20.5 % to −4.4 %,
   sign-flip p = 0.007), GLM's 22.3 % (CI −39.7 % to −4.9 %, p = 0.021, arms in separate batches). Wall time fell 13.6 %
   for DeepSeek (CI −27.0 % to −0.6 %, p = 0.060). Pass parity is a point estimate: DeepSeek's pass difference has a
   95 % CI of −4 to +6 tasks of 30, GLM's −6 to +3.6. Non-inferiority within 10 points needs about 144 pairs.
6. DeepSeek tried to reverse the SHA-256 in checks: 8 probe scripts (`hashprobe.py`, `probe.py`, `_probe_hash.py`,
   `hash_formats.py` and others). All 8 runs failed, and the probes cost rounds under the cap.

## Experiment 2, after the review, 2026-11-27 23:22 to 2026-11-28 03:50

Binary with the review fixes (`bin5`, then `bin7` with the tolerant readers). Arms of one model ran interleaved, task
by task, in one process. The OpenCode Go window was closed until 02:55; the runner waited it out by itself.

| Test | Model, set | Arms | Passed | b | c | p | Tokens, EUR, wall |
|---|---|---|---|---|---|---|---|
| Round cap | DeepSeek, all 50 | default / rounds-24 | 42 / 47 | 7 | 2 | 0.180 | EUR +68.6 % (CI +45.8 % to +91.7 %), tokens per pass +50.8 %, cap failures 8 / 3 |
| Round cap | DeepSeek, calib 20 | default / rounds-24 | 16 / 19 | 3 | 0 | 0.250 | EUR 0.3694 / 0.6065 |
| Round cap | DeepSeek, hold-out 30 | default / rounds-24 | 26 / 28 | 4 | 2 | 0.688 | EUR 0.5804 / 0.9950 |
| A/A | DeepSeek, all 50 | stored default / fresh default | 42 / 42 | 4 | 4 | 1.000 | discordance 8 of 50 (16 %) |
| Low effort | Haiku, hold-out 30 | default / effort-low | 30 / 29 | 0 | 1 | 1.000 | EUR −40.0 % (CI −46.9 % to −32.3 %), wall −44.8 % (CI −53.2 % to −34.3 %) |
| Text, strict DSML reader | DeepSeek, calib 20 | fresh default / text | 16 / 9 | | | | 8 of 11 failures ended on DSML the reader skipped |
| Text, tolerant DSML reader | DeepSeek, calib 20 | strict / tolerant | 9 / 17 | 9 | 1 | 0.021 | |
| Text, tolerant DSML reader | DeepSeek, calib 20 | fresh default / text | 16 / 17 | 2 | 1 | 1.000 | tokens per pass −14.5 % |
| Text, tolerant DSML reader | DeepSeek, hold-out 30 | fresh default / text | 26 / 27 | 3 | 2 | 1.000 | tokens per pass −19.9 %, EUR −10.5 % (CI −23.4 % to +8.5 %), cap hits 19 / 10 |
| Text, strict GLM reader | GLM, calib 20 | stored default / text | 19 / 0 | | | | GLM mixes its template with JSON |
| Text, loose GLM reader | GLM, calib 20 | stored default / text | 19 / 6 | | | | 39 stored first answers: 37 parse |
| Index, refilled row | GLM, calib 20 | default / index | 19 / 14 | 1 | 6 | 0.125 | |

Offline, on stored rows:

- Check in bubblewrap: the checks of all 540 rows with a run folder, rerun in the sandbox, give the stored result 540 times.
- Sequential calibration: a variant drops out once its deficit against the default reaches the A/A band of the tasks
  seen so far. In 20 random task orders it picks the same variant as the full run in 20 of 20 for all four models. It
  uses 66 %, 67 %, 61 % and 99 % of the runs (DeepSeek, GLM, Kimi, Haiku) and 87 %, 86 %, 77 % and 99 % of the tokens.
  It drops the cheap losers (`text` before the reader fix, `index`); the token budget of a 5-hour window barely moves.
- Selection rule on the stored calibration rows: default for all four models.

### Verdicts

- Round cap (F1 as an intervention): the prediction b ≥ 6 and c = 0 is rejected, c = 2. b = 7 against c = 2 has p =
  0.18 on 50 pairs. Cap failures fell from 8 to 3, at 69 % more euro. In both c tasks (`hard-cli-revenue`,
  `hard-rename-total-by-region`) the default passed at its 12-round cap and the 24-round arm ran to 24 and failed: more
  rounds can also undo a passing state. All 8 default failures were cap hits; 24 rounds turned 7 into passes, and 3
  runs failed at 24.
- Own templates (H3 improved): DeepSeek's text format with a reader for DSML comes within 2 tasks of native: 17 of 20
  against 17 (stored) and 16 (fresh) on the calibration set, 27 of 30 against 26 on the hold-out. Confirmed for
  DeepSeek. The tolerant reader was written after reading the round-2 answers on the calibration set; the hold-out run
  is the out-of-sample test. GLM is rejected: 6 of 20 against the bar of 7, and its answers stay malformed (unclosed
  blocks, `read":{...}`, `<|im_end|>`).
- Low effort as a cost setting: confirmed on euro for Haiku as for DeepSeek. Pass parity is not shown for any model:
  Haiku lost 1 of 30, DeepSeek's difference spans −4 to +6.
- A/A band: DeepSeek's fresh default against its stored default differs on 8 of 50 tasks (16 %), b = c = 4. The
  band's 14 % fits; at 16 % `aa_band(20)` stays 4.
- Sequential calibration: same pick in 20 of 20 orders, but 61 % to 67 % of the runs on HTTP models; the claim "at most
  60 % of the runs" is rejected.

## Final table

| Model | Profile under the current rule | Calibration default, best variant | Hold-out default, other arm | Measured lever |
|---|---|---|---|---|
| deepseek-v4.1-flash | default | 17, effort-low 18 | 25 and 26, effort-low 26, rounds-24 28, text 27 | rounds-24: +5 of 50 at +69 % EUR; text with DSML reader: equal passes, −20 % tokens per pass |
| glm-5.3 | default | 19, effort-low 19 | 23 of 25, effort-low 22 | effort-low: −22 % EUR (arms not interleaved) |
| kimi-k3 | default | 20, none above | 26 of 28 and 24 (A/A) | none; at the ceiling |
| haiku (claude-cli) | default | 20, prompt-short 20 | 29, 30 (A/A), effort-low 29 of 30 | effort-low: −40 % EUR, −45 % wall, 1 task lost |

Hypotheses of experiment 1: H1 rejected (no power), H2 rejected (noise of identical arms), H3 rejected as run and
confirmed for DeepSeek with a reader for its own template. Calibration as built does not raise the pass rate on these
four models. It finds cost settings (low effort) and parser gaps (DSML), and it measures each model's run-to-run noise.


## Next ideas, each with its test

- Rounds with a stop on progress: grant 4 more rounds while the last 4 rounds changed a file or moved the check
  result, up to 24. Test: against `default` on all 50 tasks for DeepSeek, interleaved. It holds when b ≥ 5, c ≤ 1 and
  tokens per pass stay within +20 % (`rounds-24` gave b = 7, c = 2 at +51 %).
- Text as DeepSeek's profile: `text` with the DSML reader on all 50 tasks, 3 interleaved repeats against `default`.
  It holds when the pass difference CI lies above −10 points and tokens per pass fall by at least 10 %.
- Non-inferiority of low effort: 144 interleaved hold-out pairs per model (DeepSeek, GLM, Haiku). It holds when the EUR
  CI upper bound is below −10 % and the pass-difference lower bound above −10 points.
- A gain claim needs headroom and n: a task set where the default passes 50 to 70 %, at least 193 interleaved pairs per
  model. Kimi (20 of 20) and Haiku (30 of 30) leave no room on the hard set.
- Index with a free describe: `tool_describe` calls cost no round. Test against `default` on `hard_calib` for DeepSeek
  and GLM; it holds at c − b ≤ 1 with at least 35 % fewer tokens. `history.json` now counts the describe calls.
- Hidden check text: show the check by name only and keep the SHA-256 out of the prompt. The 8 hash probes of
  experiment 1 all failed. Test: probe files and cap hits on the 5 `hard-cli` tasks, 6 runs per arm, DeepSeek.
- A profile with a confidence record: the profile stores b, c and the A/A band (done); `apply` could use a variant only
  when its lower bound beats the default. Test: two calibration runs per model pick the same profile for 4 of 4 models.
- GLM text: a reader for unclosed blocks and `name":{json}`, or a prompt that shows GLM's own `<arg_key>` template.
  Test: at least 12 of 20 on `hard_calib` against the default's 19.


## Experiment 3, 2026-11-28, after PR 70 (the DRY RUN fix)

Before PR 70, `run` returned "DRY RUN. E:TaintedSink" in 884 of 1,314 native calls and `verify_fix` "CHECK NOT RUN" in
134 of 135 (zero-cost report 28.11.). Predictions with numbers and abort rules: `predictions.md` on this branch,
committed before the first run. Installed binary `penelope`, main 22d1290. Run data: `~/.penelope/tmp/calib-28/`.

### Test 1. Sanity, Haiku (claude-cli), 3 tasks of `hard_calib`

| Check | Result |
|---|---|
| Passed | 3 of 3 (`hard-fix-clamp`, `hard-rename-load-rows`, `hard-fix-median`) |
| `run` calls with "DRY RUN" | 0 |
| `verify_fix` with "CHECK NOT RUN" | 0; `check/log.jsonl` of each run holds real results (`before_exit` 1, `after_exit` 0, status `passed`) |
| Calls per run | `run` 1 to 4, `verify_fix` 1 to 2 (from the claude transcripts of the 3 sessions, deduplicated) |

Prediction held (0 DRY RUN, at least 2 of 3 pass). The abort rule did not fire. The same count over the 60 runs of test 2
(124 `run` calls, 8 `verify_fix` calls): 0 DRY RUN, 0 CHECK NOT RUN.

### Test 2. F94 re-measure, Haiku, `default` against `effort-low`, 30 tasks of `hard_holdout`, interleaved

| Arm | Passed | Tokens in | Tokens out | EUR | Wall s (sum) | Tool calls per run |
|---|---|---|---|---|---|---|
| default | 30 | 287,866 | 87,431 | 0.5336 | 719 | 8.73 |
| effort-low | 30 | 222,607 | 44,030 | 0.3259 | 380 | 5.47 |

b = 0, c = 0, exact McNemar p = 1.000 over 30 pairs. Against default: input -22.7 %, output -49.6 %, EUR -38.9 %
(bootstrap 95 % CI -44.8 % to -32.5 %), wall -47.1 % (CI -57.1 % to -35.6 %), tool calls per run -37 %.

| Prediction | Result |
|---|---|
| Passes default 29 to 30, effort-low 28 to 30, b + c at most 3, p at least 0.25 | held: 30 and 30, b + c = 0 |
| Input -5 % to -15 % | missed: -22.7 % |
| Output -25 % to -40 % | missed: -49.6 % |
| EUR -25 % to -45 % | held: -38.9 % |
| Wall -30 % to -50 % | held: -47.1 % |

The earlier figures (EUR -40.0 %, wall -44.8 %, 30 against 29 passed) hold with a working `run`. The saving comes from
fewer rounds (tool calls -37 %) and half the output tokens. The hold-out is saturated: Haiku passed 30 of 30 in both
arms, so it shows no pass difference at all, in either direction. In the default arm 2.77 of 8.73 calls (31.7 %) come
after the last `edit` or `write`, in effort-low 1.67 of 5.47 (30.5 %). `verify_fix` ran 7 times in default and once in
effort-low; the model verifies with `run`.

### Tests 3 and 4. `stop-green` and the fixed default, DeepSeek v4.1 flash, 20 tasks of `hard_calib`

Not run. OpenCode Go answered status 429 on the first two runs (2026-11-28 15:04): "usage limit reached (weekly,
resets in 1d 9h)". The runner leaves no row on a 429, so nothing in `results.jsonl` is a result. The pair runs through
`~/.penelope/tmp/calib-28/t3.sh`: the binary built from this branch, `--variants default,stop-green --n 20 --jobs 2`,
repeated every 30 minutes until all 40 rows exist. The tests of the hook:
`stop_green_ends_the_turn_after_the_round_that_turns_the_check_green` (the read round runs no check, a wrong write runs
it and the turn goes on, the right write ends the turn with 3 of 4 scripted replies requested),
`stop_green_leaves_a_red_check_to_the_model`, `the_fingerprint_sees_files_and_ignores_git_and_caches`,
`stop_green_is_an_extra_variant_with_one_factor_changed`.

The predictions for both tests stand in `predictions.md`. Test 4 compares the default arm of this run with the 84
stored pre-fix default rows of `~/.penelope/tmp/deepseek/stopgreen-r2*` (76 passed, 9.87 rounds, 126,038 input tokens per run).

### New hypotheses (test 5)

1. Non-inferiority of low effort on a set with headroom. The hold-out cannot show a loss (30 of 30 twice, 29 of 30
   once). Claim: on the 28 SWE-bench tasks where Haiku with effort-low solved 9, `default` solves at most 2 tasks more
   (c - b at most 2) and effort-low saves at least 30 % EUR. Abort after 14 pairs at c - b of 3 or more. Cheapest
   test: 28 interleaved pairs, claude subscription, about 2 h wall.
2. A stop hint through the tool result for the claude command. Rejected first version: put the hint into the
   `verify_fix` result. Haiku calls `verify_fix` in 6 of 30 default runs and 1 of 30 effort-low runs, so the hint
   reaches 20 % and 3 % of the runs and saves at most 0.1 call per run. Improved version: the MCP `run` and `edit`
   results append "The task check passes. Write the final answer now." when the check of the task passes (check
   command in the MCP server's environment, calibration only). Claim: calls after the last change fall from 2.77 to at
   most 1.2 per run (default), tool calls per run -15 % or more, c - b at most 1. Abort when Haiku keeps calling tools
   after the hint in more than 50 % of the green runs. Cheapest test: 30 hold-out pairs, Haiku default with and
   without the hint.
3. The verification tail is a constant share of the rounds. Haiku default 31.7 % and effort-low 30.5 % of the calls
   come after the last change (zero-cost count on the 60 runs). Claim: on DeepSeek with a working `run`, `stop-green`
   saves 25 % to 35 % of the rounds per passing run and no more than 39 % (the replay ceiling from before PR 70).
   It is rejected when the saving is below 15 % or above 45 %. Cheapest test: tests 3 and 4 as predicted, no new run.
