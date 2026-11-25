# Eval and replay

`penelope eval` runs the same task file under three setups and puts the result in one table. The point is to replace
expected cluster advantages with measured ones.

```bash
penelope eval cli/tests/data/eval/tasks.jsonl --setup all
penelope eval tasks.jsonl --setup large --runs docs/RUNS.md
penelope replay <run-id> --model qwen3.8
```

`eval` appends one section to `docs/RUNS.md` (or the path after `--runs`). `replay` does not write there.

## Task file

The task file is JSONL: one JSON object per line, blank lines are allowed. A broken line stops the run; no task is
skipped silently.

```json
{
  "id": "write-answer",
  "prompt": "Create answer.txt in the project with the single word hello on its own line.",
  "setup": {"repo": "repo", "cwd": "", "copy": true},
  "check": "grep -qx hello answer.txt",
  "check_timeout_seconds": 60,
  "max_steps": 6
}
```

| Field | Required | Meaning |
|---|---|---|
| `id` | yes | 1 to 80 letters, digits, `-`, `_` or `.`. Unique in the file. |
| `prompt` | yes | The task text the model receives. |
| `setup` | yes | A folder path, or an object with `repo` (required), `cwd` (relative, optional) and `copy` (default `true`). A relative `repo` is resolved against the folder of the task file. |
| `check` | yes | One shell command. The task passes when it exits 0. |
| `check_timeout_seconds` | no | 1 to 3600, default 300. A timeout is a failed check. |
| `max_steps` | no | 1 to 30, default 12. The agent and cluster setups clamp their own limits to it. |

The example lives in [`cli/tests/data/eval/tasks.jsonl`](../cli/tests/data/eval/tasks.jsonl) with a one-file repo next
to it.

### The setup folder is copied

With `"copy": true` the runner copies the setup folder to `.penelope/eval/work/<run-id>/<task-id>/`, runs the model and
the check there, and removes the copy afterwards. The original task repo is not changed. The copy skips `.git`,
`target`, `node_modules`, `.venv`, `venv`, `__pycache__`, `.penelope`, `dist`, `.cache` and `.next`, and it does not
follow symbolic links. A task that needs git history or build output sets `"copy": false`; it then works in the setup
folder itself.

During a task the file and `run` tools use the task folder as their default `cwd`. A model that names no `cwd` still
reads and writes the right project. Absolute paths are not stopped by this default: the model can still name a path
outside. The eval is a measurement, not a sandbox; use a disposable machine account if a task must not reach outside.

## Setups

| Setup | What runs | Model |
|---|---|---|
| `large` | the normal chat path (`backend::Chat`) with the main model and its tools | `main` |
| `large-subagents` | `agents::run` with one worker and one judge | `main` for lead, worker and judge |
| `cluster` | `clusters::orchestrate` with the profile `eval-small` (one worker, one validator) | `weak` |

`--setup all` runs all three in that order. For each task the runner records:

- whether the check command exited 0;
- tokens in and out, as the delta of `.penelope/usage.json`;
- euro, as the delta of `cost.rs` (unpriced models count as 0.0000, never a guess);
- wall time, including the copy and the check.

An eval is unattended, so the run context sets `edit_policy` to `all`, `delegate` to `off`, `auto_learn` and
`auto_skills` to `false`. Those changes live in memory for the run; they are not written to `config.json`. The cluster
profile `eval-small` is written to `.penelope/clusters/profiles/eval-small.json`; a profile with that name is reserved
for the eval.

## Replay

```bash
penelope replay large-1764000000000 --model qwen3.8
```

Replay loads `.penelope/eval/runs/<id>.json` and sends every stored prompt to the named model. The provider, address
and key come from the current `main` setting; `--model` replaces only the model name. The main setting must be an HTTP
provider (`openai` or `anthropic`); the `claude` command cannot be held to the tool gate from here and is refused.

Replay is read-only and says so on the first line of its output:

- only `read`, `find`, `grep`, `tree`, `outline` and `json_query` are offered;
- `write`, `edit`, `run`, `job_start`, `screen`, `mcp_call`, `cluster_start` and every other name are refused by the
  gate in `eval.rs` before the handler;
- no task folder is entered, no check command is run, and no eval run is written.

The refusal appears in the output as `refused: write` (or the names of the tools the model tried). Replay records its
own tokens under the role `replay`, so `penelope cost` shows it.

## Tests without a provider

The tests in `cli/src/eval.rs` use `llm::fake::serve`, the local HTTP endpoint of the test suite. They prove:

- the task schema and the example fixture parse, and a line without a check is refused;
- `large` writes a file through the default task cwd and the check passes; the table and `RUNS.md` row are produced;
- `large-subagents` makes four calls with worker and judge system prompts on the same strong model;
- `cluster` calls `clusters::orchestrate` with `eval-small` and two weak-model calls;
- replay refuses a `write` call, leaves the target file absent, and sends the recorded prompt.

No test talks to a real provider. A row with fake numbers is only ever written to a temp path, never to the repository
`docs/RUNS.md`.

## What is not measured yet

No real eval run is in `docs/RUNS.md` at the time this module was added. The table format and the three setups are
tested, but a claim such as "the cluster costs less per passed check" still needs a run with real models and 20 tasks.
`docs/RUNS.md` is the place for that result, written by `penelope eval` itself.
