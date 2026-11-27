# Questions

Penelope asks before it guesses. The tool `ask_user` puts up to 6 questions in one dialog. Before Penelope creates a cluster or a scheduled task, it checks a spec for missing fields and asks for them. A workflow will use the same check.

## The tool ask_user

Send a `questions` list. Each question has:

| Field | Meaning |
| --- | --- |
| `question` | The text. Required. |
| `header` | A short label for the tab, up to 16 characters. Default `Q1`, `Q2`. |
| `options` | 2 to 6 answers. Each is a text or an object with `label` and an optional `description`. |
| `multi_select` | `true` lets the person choose several options. Default `false`. |
| `allow_text` | `true` lets the person type a free answer. Default `true`. |

```json
{"questions": [
  {"question": "Which database?", "header": "Database",
   "options": [{"label": "sqlite", "description": "one file"}, "postgres"]},
  {"question": "Which checks?", "header": "Checks", "multi_select": true,
   "options": ["lint", "tests", "audit"], "allow_text": false}
]}
```

The old form still works: `question` plus `options` as plain texts, 2 to 6 of them. It returns `the user chose: ANSWER`.

A faulty question set is refused with the rule that failed, for example `question 1 needs 2 to 6 options`, and nothing is shown.

### What the model gets back

One line per question: the header, the question and the answer.

```
the user answered 2 questions:
1. Database: Which database? -> postgres
2. Checks: Which checks? -> lint, audit + typed: and a security scan
```

- Chosen labels are joined with commas.
- A typed answer follows `typed:`. In a single-choice question it replaces the choice.
- A question the person did not answer reads `closed`.
- If the person closes the whole dialog, the model is told to go on with the most careful choice and to say which one it took.
- If nobody answers in time, the text names the seconds: `the user did not answer within 900 seconds`.

## The dialog in the full screen interface

One question is open at a time. The top row is a tab strip of the headers with a tick on answered ones and a last tab, `send`, for the summary page. The top right shows the time left.

| Key | Action |
| --- | --- |
| Up, Down | Move through the options and the text row. |
| 1 to 6 | Choose that option. In a single-choice question this also moves on. |
| Space | In a multi-select question, toggle the option under the cursor. |
| Enter | Choose the option under the cursor and move on. In a multi-select question it moves on once one option is chosen. On the text row it starts typing. |
| Any letter | Starts the free answer when `allow_text` is on. |
| Digit on the text row | Starts the free answer with that digit. Move down to the text row first (or press Enter or Space there), then digits type instead of choosing option 1 to 6. |
| PgUp, PgDn | Scroll a long question text. The options never scroll away. |
| Tab, Right | Next question. |
| Shift+Tab, Left | Previous question. |
| Enter on the summary | Sends all answers. A question without an answer sends the person back to it. |
| Esc | Closes the whole dialog. Every answer counts as closed. |

The answers stay in the transcript as one `answered` line per question.

### Timeout

`ask.timeout_secs` in `.penelope/config.json` sets how long Penelope waits. The default is 900, the lowest value is 10. The dialog shows the time left and turns red under 60 seconds. When the time ends, the dialog closes and the model gets the timeout text.

## ACP

`penelope acp` sends each question to the editor as a `session/request_permission` request. The options are the answers, and the last option is `Other (answer in the chat)`, `Skip` or `Done`. A multi-select question repeats the request until the person picks `Done`, at most 12 rounds. An unknown or repeated option id is ignored.

Every option of a model question has the kind `reject_once` and the title starts with `Question from the model:`, so an editor never offers it as Allow. Only the create confirmation of Penelope itself has an `allow_once` yes. When Penelope gives up waiting, it stops asking the editor and drops late answers. A free typed answer is not possible there, so the person writes it in the chat.

If the editor does not know the method, Penelope posts the questions as a chat message, tells the model to end its turn, and the answers arrive as the next prompt.

## Clarify before creating

`cli/src/clarify.rs` holds the field list and the rules. Penelope Desktop can call the same functions with its own dialog.

A spec has these fields. A field is asked only when it is missing or the request names two different values for it.

| Field | Header | Cluster | Scheduled task | Workflow |
| --- | --- | --- | --- | --- |
| goal | Goal | yes | yes | yes |
| done criterion | Done when | yes | yes | yes |
| abort rule | Abort rule | yes | yes | yes |
| budget in tokens or euro | Budget | yes | yes | yes |
| maximum iterations or runs | Max rounds | yes | yes | yes |
| schedule | Schedule | no | yes | no |
| rights: read-only, write files, network, install packages | Rights | yes | yes | yes |
| models or roles | Models | yes | yes | yes |
| where results go | Results | yes | yes | yes |
| person approves each run | Approval | no | yes | yes |

### How a field counts as filled

Penelope reads the request before it asks. It takes values from:

1. Values the caller passes: tool arguments such as `done`, `budget` or `rights` of `cluster_start`, and command flags such as `--every` and `--profile`.
2. Lines that name a field, for example `Budget: 4 EUR`.
3. Phrases: `done when ...`, `stop if ...`, `5 EUR`, `200k tokens`, `3 rounds`, `every 3h`, `at 08:00`, `daily`, `read-only`, `may write`, `with network`, `using the solo profile`, `results go to ...`, `ask me before each run`.

A complete request asks no field question. It still gets the final yes.

### Rounds and the final yes

Missing fields are asked in rounds of at most 6 questions, so a sparse request needs two rounds. Then Penelope shows the whole spec once, rights and budget first and every field cut at 200 characters on its own, and asks `Create this cluster?`. The cursor opens on `No, do not create`; `Yes, create it` is the second option. Anything but yes creates nothing. A closed dialog or a timeout creates nothing.

### Where the spec goes

- Cluster: the orchestrator receives the goal followed by the agreed limits, and each new top-level cluster keeps `clusters/ID/spec.json`.
- Scheduled task: the factory profile carries the spec under `spec`. A euro budget also sets `budget_eur` of the profile.
- Workflow: `clarify::clarify_workflow(ctx, request, preset)` returns the spec. The coming `/workflow` stores `spec.to_value()` in the workflow it creates and starts nothing when it returns an error.

### Where Penelope asks

- `/cluster GOAL` in the full screen interface.
- The tool `cluster_start`, when the model creates a cluster.
- `/factory new NAME --profile P ...` in the full screen interface, unless `--dry` is set.

Penelope asks nobody when nobody can answer: in a timer, in a script, in an eval run, in a scout, inside a cluster worker and in a subagent. The tool `ask_user` with `questions` is refused there with the same rule. The request then runs as before, without a spec.

## Not enforced yet

- A budget in tokens is stored in the spec and in the goal text. The cluster runner only counts euro through the profile.
- A factory whose spec says `approve each run: yes` is refused at `factory install --yes`, because a timer starts runs alone. `/factory list` shows the stored spec. The manual `factory run` is your yes.
- Rights, abort rule, max rounds and results are not enforced by `clusters.rs` yet (planned as C2b). They reach the workers as text in the goal. The cluster runner does not read them as settings.
