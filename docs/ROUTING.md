# Cost mode with automatic escalation

Config `route` has two values. `fixed` is the default: the main role runs on the `main` model, as before. `auto` starts
every task on the weak model with effort low and moves it to the main model with effort high when a trigger fires.

## Turn it on

- In the interface: `/route auto`. `/route fixed` turns it off. `/route` shows the mode, the current model, the effort
  and the number of escalations in this session.
- In `.penelope/config.json`: `"route": "auto"`. `"route_context_limit": 100000` sets the context trigger.
- The weak model is the `weak` setting (default `claude-cli` `haiku`), the main model is the `main` setting (default
  `sonnet`). Both must use the same kind, provider and base URL. With different hosts the mode stays inactive and
  `/route` says so, so an escalation never sends the weak history to another host.
- The test trigger needs `test_command` in the config. Without it Penelope does not run tests, so no test result exists.

The footer shows `auto · haiku low`, and after an escalation `auto · sonnet high (escalated: tests failed 2x)`.

## What a task is

A task is one chat: a new session, `/new` or one eval task. A task starts on the weak model. After an escalation it
stays on the main model until the next task starts. `/escalate` before the first turn applies to the coming task and is
counted once. Two chats on one folder keep separate state.

## Triggers

Each is counted per task.

1. The test command fails twice in a row. The first failure raises the weak effort from low to medium. A pass between
   two failures resets the count.
2. Eurykleia reports `E_STRIKES`: 3 failed calls of one tool in one turn.
3. You type `/escalate`, or a message that is only "nochmal", "retry" or "try again".
4. The context of the last turn is over `route_context_limit` tokens (default 100,000, the point where the Haiku 5.5
   price rises).

A change applies from the next turn. If a trigger fires inside a weak turn (tests or strikes), Penelope runs one
follow-up turn on the main model in the same call, so a one-shot run such as an eval task also escalates. A weak turn
is not cut short: it ends on its own, or on the third failed test run (`test_attempts`).

## What is recorded

`.penelope/usage.json` holds `route.tasks`, `route.escalated`, `route.reasons` (a count per reason) and `route.log`
(the last 500 tasks with `escalated` and `reason`). `penelope eval` prints `escalation rate: N of M tasks` per setup
when tasks ran in `auto`.

## Measured basis

Measured on 27.11.2026 on 28 hard SWE-bench tasks with the same harness:

| setup | solved | cost (computed) | wall time |
|---|---|---|---|
| Sonnet 5.5 | 8 of 28 | 1.5065 EUR | 2,125 s |
| Haiku 5.5, effort low | the same 8 of 28 | 0.1446 EUR (9.6 %) | 1,619 s |

- Effort low cut output tokens by 58.6 % without losing a hit.
- Prices from the official Anthropic page, per million tokens up to 100k prompt tokens: Haiku 5.5 0.10 / 0.50 USD,
  Sonnet 5.5 2.00 / 10.00 USD. The EUR figures are computed from these prices, not billed.
- Claude Code with Haiku low solved 9 of 28. No Penelope-specific gain over another harness is shown. The saving comes
  from the model and the effort level.

Caveats:

- The 28 tasks were also used for tuning. The result may not hold on other tasks.
- The 20-task hold-out for Haiku is still running.
- Both models solved the same 8 tasks. That says nothing about how often Haiku fails where Sonnet would succeed.

## Not measured yet

- The hold-out result.
- The escalation rate of `auto` on real tasks, and the cost and hit rate of the full mode against `fixed`.
- A real multi-model cluster (this mode switches one lead between two models; it does not run a cluster).
- The context trigger: the 100,000 limit comes from the price table, no run has hit it.

## Limits

- Test results and strikes reach the chat as small signed files in Penelope's state folder (`PENELOPE_STATE_DIR`, default
  `~/.penelope/state`), one folder per chat, outside the workspace. The chat takes each file by
  renaming it. A file with a wrong signature is ignored, and `write` and `edit` refuse that folder. A command run
  through `run` is not blocked from writing there; the signature key is the only guard against that.
- The context size of a turn is its fullest single step: input, cache creation and cache read tokens. For an API turn
  it is the largest input of one request.
