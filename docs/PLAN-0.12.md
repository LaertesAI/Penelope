# Penelope 0.12 — gap audit and plan

**Date:** 2026-11-25  
**Baseline:** working tree of 0.11.0, 109 tests, 42 tools, 45 Rust files, 15,571 lines  
**Method:** code and docs audit, `cargo test`, `penelope doctor`, live `auth verify`, live TUI/plain chat, live plugin
and MCP administration, live HTTP calls against `penelope serve`  
**Scope:** what is genuinely missing after the 0.11.0 second pass, in the order that reduces risk for a daily user

This file is the detailed plan. [`NEXT.md`](NEXT.md) stays the short status list; where the two disagree, this file
wins.

## 1. What is green today

These are not road-map rows. They were exercised on this PC.

| Area | Evidence | Confidence |
|---|---|---|
| Normal use | `penelope ask` → `ok`; `chat --plain` completed a turn; TUI started in tmux at 120×40 and quit with code 0 | high for Linux/claude-cli |
| Tool layer | 42 tools, one spec table, role and schema gate, `cargo test` 109 passed | high |
| Search | 8 live hits from Startpage + DuckDuckGo + Bing; cache, canonical URLs, health file, reranker client | medium: keyless engines rate-limit |
| Reading | static HTML to Markdown, local PDF through `pdftotext`, optional reader API | medium: no OCR, no JavaScript renderer of its own |
| Local server | `penelope serve`: `/search`, `/read`, `/embed`, `/rerank`, `/redact`, `/whoami`; token, 120 req/min | high for loopback, untested TLS/proxy |
| ZIP | `unzip` tool with zip-slip, symlink and size checks; plugin ZIP install | high |
| Plugins | add, install Git/local/ZIP, import, list, allow, deny, update, remove | high |
| MCP | plugin servers plus standalone add/remove/allow/list/probe/call; live echo server | medium: one process per call |
| Login | Claude official login and live verify; API key storage; `auth verify` for main/weak/scout | high for Claude, key presence for API providers |
| Leak audit | 0 findings on 15 files | high |

## 2. Gaps

Priority is risk to a daily user, not novelty. **P0** blocks another person using Penelope on real work. **P1** makes
the platform dependable. **P2** is polish.

| ID | Area | Gap | Risk | Priority |
|---|---|---|---|---|
| G01 | Safety | A worker changes the live tree. The worktree + judge gate is designed in `PLAN.md` but not built. | high: one bad agent edit is expensive | P0 |
| G02 | Search | Keyless engines still rate-limit. No daily probe, no `search instances add`, no bundled SearXNG. | high: search stops on a bad day | P0 |
| G03 | Injection | `scrape::scan` catches text lines, zero-width and bidi text. CSS-hidden blocks, fake system blocks and suspicious links are not covered. | high: a page can still steer a model | P0 |
| G04 | Tests | One full run failed once with 105/109 and then passed eight times. The test was not identified. | medium: a real failure can hide as a flake | P0 |
| G05 | Platforms | Windows and macOS code paths exist, but no CI or hardware run. No signed/notarised macOS build. | high for anyone not on Linux | P0 |
| G06 | Server models | No `/chat/completions`, `/route`, `/check`, `/filter` or `/train` for the eight families. | high: the server is not a model platform yet | P1 |
| G07 | Auth | API providers take a pasted key; no OAuth device flow, refresh or `/whoami` for them. `auth verify` checks roles, not search/reader keys. | medium: key handling is manual | P1 |
| G08 | MCP | One process per call; allow is per plugin, not per tool; no env/cwd in `mcp add`; no remote HTTP/SSE MCP. | medium: stateful servers are unusable | P1 |
| G09 | Agents | `max_parallel` is stored but steps run in order; no DAG, no shared scheduler. | medium: slow and expensive | P1 |
| G10 | Budget/context | No session budget hard stop, no model failover, no measured context budget. | medium: a long turn can overspend or overflow | P1 |
| G11 | Update | Client and manifest contract exist; no host, no signing key, no release job, no rollback test. | medium: no distribution path | P1 |
| G12 | Registry | No signed plugin index, no per-tool permissions, no revoked-version list. | medium: plugin trust is manual | P1 |
| G13 | Server ops | No TLS, no multi-user auth, no request audit, no metrics, no connection limit. | medium: only safe behind a local proxy | P1 |
| G14 | Plugin UX | `plugin install` blocks the TUI while git runs; no progress view; `remove` cannot delete the folder. | low: annoying, not unsafe | P2 |
| G15 | Files | ZIP only; no `.tar.gz`, `.7z`, `.docx`/`.odt`. | low: workaround exists | P2 |
| G16 | Memory | Hybrid memory search is lexical; embeddings are deterministic hashes, not semantic. | low: tree still works | P2 |
| G17 | Web login | No HTML login page for the server. `/whoami` verifies a bearer token; there is no user database, cookie session or OAuth callback. | low for API use, medium if a browser UI is wanted | P2 |
| G18 | Polish | `NEXT.md` still describes prompt-injection and search as open P0 items that are now partly built. | low: docs drift | P0 docs |
| G19 | Website account | The existing Django backend has a real `/sign-in/`; the prototype now uses it, but account/admin/downloads views are still mock. | medium: a user can sign in but sees demo data | P1 |
| G20 | Website tools | **Built in this pass:** the Django backend exposes `/account/tools/*` (session + CSRF) and `/api/v1/tools/*` (API key), forwarding only an allowlist to `penelope serve`. | low: tests and a live search passed | P1 done |
| G21 | Website security | No 2FA, no session list, no new-device mail; production TLS/mail/PostgreSQL are not deployed. | medium before a public start | P1 |
| G22 | Website ops | No deployment, domain, TLS proxy or real mail service. | medium | P1 |

## 2b. The existing website backend

The audit found the real server the earlier work referred to: `Laertes/website/backend`, Django 5.2, 372 tests, clean
working tree. It already has:

- `/sign-in/`, `/register/`, `/confirm/<token>/`, `/reset/` and `/reset/<token>/`;
- Argon2 passwords, 12-character minimum, session rotation on login, staff sessions capped at 12 hours;
- per-email and per-IP rate limits on login, register and reset; generic answers against account enumeration;
- CSRF on every HTML form, CSP, `X-Frame-Options: DENY`, `nosniff`, `Referrer-Policy: same-origin`;
- hashed one-time tokens, hashed API keys with a prefix and a revocation list, a usage API with its own rate limit;
- role gates for customer/employee/admin, an audit row per changed field, and a last-admin guard.

What was missing and is now added in that repository:

- a JSON auth layer under `/api/v1/auth/` for the static prototype, with CSRF, CORS for `SITE_ORIGIN`, rate limits and
  the same forms as the HTML pages;
- production security switches: `SECURE_SSL_REDIRECT`, HSTS, `SECURE_PROXY_SSL_HEADER`, `X_FRAME_OPTIONS` and
  `Permissions-Policy`;
- the prototype sign-in and register dialogs now call the backend when opened with `?auth-api=/api/v1/auth`; the demo
  stays the default without that parameter;
- a tool gateway: `/account/tools/{search,read,embed,rerank,redact}` for a signed-in session (CSRF) and
  `/api/v1/tools/*` for an `lae_` API key, forwarding only the allowlist to the loopback `penelope serve` with
  `PENELOPE_TOOLS_TOKEN`. Live: a signed-in customer searched through Django → Penelope → Startpage and got 2 hits.
  Tests: 376 pass, 4 new ones cover forwarding, auth, unknown tools, body limits and a 502 without a token leak.

Still open in that repository: account/admin/download views are mock, no 2FA, no session list, no real
mail/TLS/PostgreSQL deployment. Those are G19, G21 and G22 below.

## 3. The plan

### Milestone 0.11.1 — make the baseline honest (target: 2–3 days)

| Step | Work | Files | Acceptance |
|---|---|---|---|
| 1 | Find the transient test failure: run the suite 20×, then `--test-threads=1`, isolate temp folders and any real process/port. | `cli/src/**/tests` | 20 consecutive runs are 109/109, or the failing test is found and fixed |
| 2 | Docs truth pass: mark prompt-injection and search as partly built in `NEXT.md`, link this plan, refresh counts and PDF. | `docs/*.md`, README | no row claims a state the code does not have |
| 3 | `auth verify` for search and reader endpoints: one small request, no query text in logs. | `auth.rs`, `server.rs` | `auth verify search` and `auth verify reader` report ok/FAIL with latency |
| 4 | `mcp add` gains `--env KEY=VALUE` and `--cwd DIR`; the registry stores them. | `plugins.rs`, `mcp.rs` | a server that needs one env var starts through `mcp add` |

**Why first:** the next milestone should not build on a flaky suite or stale claims.

### Milestone 0.12.0 — safety and daily reliability (target: 3–4 weeks)

#### G01 — Worker in a worktree with a judge gate

This is the largest remaining safety hole.

- First step: `penelope work start ID`, `git worktree add .penelope/worktrees/ID -b agent/ID`, run the worker with
  `cwd` in the worktree, collect its diff.
- Judge step: the judge reads the diff and the test output; the person sees one diff and says yes/no.
- Yes: apply the diff to the live tree, remove the worktree, keep the transcript. No: keep the worktree for
  `penelope work show ID`.
- Files: new `work.rs`, `agents.rs`, `files.rs`, `tui/mod.rs`, `spec.rs` if a `work` tool is exposed.
- Acceptance: 10 real tasks from the Telemach set; no task touches the live tree before the yes; a failed judge leaves
  the worktree; `penelope worktrees` lists and removes them.
- Measure: solved tasks, lead tokens, wall time, wrong “done” claims caught.
- Effort: L (5–8 days).

#### G02 — Search that survives a rate limit

The client now has `search health`, `search.urls`, Startpage and an optional API. That is not yet a daily guarantee.

- First step: `penelope search probe --daily` writes one health row per engine once a day; `penelope search instances
  add URL` appends a SearXNG base to `search.urls`.
- Add a small `docs/SEARXNG.md` with a Docker command and the JSON setting.
- Fallback order stays: server API → SearXNG pool → Startpage → DuckDuckGo → Bing → Wikipedia.
- Acceptance: 20 queries in a row return at least one result; `doctor` says which source is alive and when it was last
  checked; a dead instance is skipped in under 2 s.
- Measure: success rate over 100 queries, p95 latency, cache hit rate.
- Effort: M (3–5 days).

#### G03 — Prompt-injection defence in the reader

The scanner exists but is textual. A page can hide an instruction in CSS, a fake system block or a link that looks
like a citation.

- First step: extend `scrape::scan` with CSS `display:none`/`visibility:hidden`, `aria-hidden`, white-on-white,
  off-screen and tiny-font detection; fake `system:`/`assistant:` blocks; a link-allowlist warning for a URL whose
  host differs from the page.
- Keep the rule: warning, never silent drop; the page is still evidence.
- Test corpus: the 20 ScreenCodec attacks plus 20 normal pages; target recall 100 %, false positives under 10 %.
- Files: `scrape.rs`, `research.rs`, `tools.rs`.
- Measure: attacks caught, false positives, added milliseconds.
- Effort: M (3–5 days).

#### G04 — Test-suite stability

- Run the suite 20×, then serial; isolate any test that uses a real program, port, clock or shared temp name.
- If a test needs the network, mark it ignored by default and run it in a nightly job.
- Acceptance: 20 consecutive green runs; no test depends on another test's order.
- Effort: S (0.5–1 day).

#### G05 — Windows and macOS that are actually run

- GitHub Actions matrix: `ubuntu-24.04`, `windows-2025`, `macos-15`.
- Jobs: `cargo test`, `--help`, `doctor`, one fake-provider `ask`, `penelope input status`.
- macOS: sign with a Developer ID when one exists; otherwise state “unsigned, Gatekeeper blocks” in the release notes.
- Acceptance: 109 tests green on all three; one live local-model ask on each; input status chooses the right backend.
- Effort: M (3–5 days), plus certificate time.

### Milestone 0.13.0 — the platform (target: 4–6 weeks after 0.12)

#### G20 — Penelope tools behind the website login (built)

- `/account/tools/{search,read,embed,rerank,redact}` for a signed-in session with CSRF; `/api/v1/tools/*` for an
  `lae_` API key. Fixed upstream URL, 256 KB body, 30 s timeout, 60 calls/minute per user, no prompt or page text in
  the log, and `PENELOPE_TOOLS_TOKEN` never leaves the server.
- Acceptance met: a signed-in customer searched through Django → `penelope serve` → Startpage; anonymous and bad-key
  requests are 401; an unknown tool is 404; a non-JSON body is 415; an upstream 500 becomes 502 with no token in the
  answer. Tests: 376 pass in the website backend.

#### G19 — Real account views in the prototype

- `GET /api/v1/account` returns the signed-in profile, plan, usage and keys (without key material).
- The prototype account view reads it instead of the demo data; admin view reads the console counts for admins only.
- Acceptance: a real customer sees their own address and plan; a demo URL still shows the demo; no account data in the
  static HTML.
- Effort: M (3–5 days).

#### G21 — 2FA, sessions and mail

- TOTP for console users; a session table with device, last use and revoke; a mail on a new device.
- Real SMTP in production; confirmation and reset mails leave the terminal.
- Acceptance: an admin without TOTP cannot enter the console; a session can be revoked; a mail arrives in a test inbox.
- Effort: L (6–10 days).

#### G06 — Model endpoints for the eight families

Start with the two that unblock the most work, then the rest:

1. **Nostos** — `/chat/completions` with strict tool schemas and one call per request.
2. **Telemach** — the coding tool loop, streaming and stop reasons.
3. **Tiresias** — `/route` returning a plan with roles and limits; invalid JSON falls back.
4. **Argos** — `/check` returning `PASS/FAIL` and one reason.
5. **Aiolos** — `/chat/completions` with max output tokens and no tools.
6. **Utis** — `/redact` with a model behind the fixed-rule floor; failure falls back and is counted.
7. **Klea** — `/filter` keep/drop plus reason, no generation.
8. **Proteus** — `/train` job id, status, loss, checkpoint.

Every endpoint needs: model name, context size and price in `/models`; a health endpoint; request ids in errors; a
timeout; token counts that `cost` can read. The existing deterministic `/embed`, `/rerank` and `/redact` stay as
fallbacks.

- Acceptance: each family passes a contract test against a stand-in; a broken response never enters the loop; `cost`
  records the role.
- Effort: L (10–15 days for the first four, then one family per week).

#### G07 — Auth broker and real verification

- OAuth device flow for the providers that support it; token refresh with scope and expiry in the OS keyring.
- `GET /whoami` stays the server check; add `penelope auth verify search|reader`.
- A browser login page is **not** part of this step; the server is an API.
- Acceptance: a token refreshes without a prompt; an expired token is reported by `auth verify`; a key never appears
  in a log or a response.
- Effort: M (5–8 days).

#### G08 — MCP: state and per-tool permissions

- Generalize the `screen.rs` bridge to a long-lived MCP child with request ids, timeout and one restart.
- `plugins.json` and `mcp.json` gain `"allowTools": ["read", "search"]`; a call to another tool is refused by name.
- `mcp add` gains `env`, `cwd` and a transport field (`stdio` now; HTTP/SSE later).
- Acceptance: a stateful server keeps state across two calls; a denied tool is refused; a crashed server restarts once.
- Effort: L (5–8 days).

#### G09 — Parallel subagents

- Add `depends_on` to agent steps, a small DAG and a fixed worker pool.
- A worker and a scout may overlap; a worker and its judge may not.
- Acceptance: two scouts finish in about the slower one; judge never starts before its worker; cancellation stops the
  pool in under 2 s.
- Effort: M (5–7 days).

#### G10 — Budget, failover and context

- `session_budget_eur` with an 80 % warning and a hard stop; `models.main.fallbacks` on 429/5xx.
- Pre-flight token estimate and a compact step at 80 % of the window.
- Acceptance: a 429 is answered by the second model without losing the turn; a 200k-token session stays under the
  window; the cost report names the fallback.
- Effort: M (5–8 days).

#### G11 — Update server and release pipeline

- GitHub release job: build all platforms, SHA-256, minisign manifest, upload artifact first, manifest second.
- `updates.laertes.ai` or a static bucket; `Cache-Control: max-age=60` on manifests.
- Acceptance: 0.11.0 updates to the next build on Linux and Windows; a wrong checksum installs nothing; `penelope.old`
  rolls back.
- Effort: M (3–5 days), plus key handling.

#### G12 — Plugin registry

- `GET /index.json` with name, description, source, version, signature and per-tool permissions.
- Signed archives, SHA-256 and a revoked-version list; Claude marketplace files stay installable.
- Acceptance: an unsigned or revoked plugin is refused by policy; a signed one installs; a permission is enforced per
  tool.
- Effort: L (5–8 days).

#### G13 — Server operations

- TLS termination is a reverse proxy, documented; add a request log without prompt text, a metrics endpoint and a
  connection limit.
- `serve --allow-remote` stays refused without a token.
- Acceptance: a proxy terminates TLS; `/metrics` shows requests, errors and latency; a log holds no query or page.
- Effort: M (3–5 days).

### Milestone 1.0 — polish (after 0.13)

| Gap | First step | Acceptance | Effort |
|---|---|---|---|
| G14 plugin UX | Install in a background job with a progress view; `plugin remove --delete` only for `plugin_home`. | TUI stays responsive; delete asks once and touches only the plugin home | S |
| G15 archives/Office | `.tar.gz` through `tar`, `.docx`/`.odt` through `document.xml`; keep ZIP as the safe default | text from all three formats; no path escape | M |
| G16 semantic memory | a local embedding server behind `/embed`, top-8 cosine plus tree walk | synonym query finds the note; no page leaves the PC | M |
| G17 web login | only if a browser UI is wanted: OAuth callback, session cookie, CSRF and logout | a browser can sign in and see a read-only status page; API tokens still work | L |
| G18 docs/PDF | regenerate README, DEVLOG, PLAN, NEXT and PDF after each milestone | counts match `cargo test`, commands match `--help` | S |

## 4. Order and gates

| Gate | Contents | Exit test |
|---|---|---|
| 0.11.1 | flake, docs truth, search/reader verify, `mcp add --env` | 20× green suite, `auth verify search` and `reader` live |
| 0.12.0 | worktree worker, search resilience, injection hardening, CI matrix | 10 worktree tasks, 20 search queries, attack corpus, 109+ tests on 3 OS |
| 0.13.0 | model endpoints, auth broker, MCP state, parallel, budget, update server, registry | one live task per family; stateful MCP; a 429 failover; a signed update |
| 1.0 | plugin UX, archives, semantic memory, optional web login | a second person can install, update and use it without a terminal debug session |

A milestone is not done until it has:

1. a functional acceptance test;
2. one live run recorded in `docs/RUNS.md`;
3. a measured cost or latency number;
4. a line in `DEVLOG.md` and, when the interface changes, a screenshot.

## 5. What is deliberately not planned

- Training the eight families inside this repository.
- A public plugin registry with automatic trust.
- A cloud scraper by default.
- Telemetry that sends prompts.
- Replacing the TUI with a graphical window.
- A web UI before the API and the auth model are stable.

## 6. Open questions

1. Should a worker run commands outside the safe list inside its worktree without a question, or should every command
   still ask?
2. Is the first model endpoint Nostos or Telemach? Nostos unblocks tools; Telemach unblocks coding.
3. Does the update server live under `laertes.ai` or a neutral static host?
4. Is a browser login page actually wanted, or is the terminal plus `/whoami` enough?
5. Which provider gets OAuth first after Claude?

The default answers in this plan: worker asks outside the safe list; Nostos first, Telemach second; neutral static
host; no web UI before 1.0; OpenAI-compatible provider first.
