# Deep research

Deep research answers a question in several rounds with a report whose citations were checked against a second fetch of each source. It costs minutes and many model calls. `research` stays the quick lookup.

## Entry points

| Where | Command |
|---|---|
| Terminal UI | `/research deep [--max-rounds N] [--budget EUR] QUESTION` |
| Command line | `penelope research --deep [--max-rounds N] [--budget EUR] QUESTION` |
| Model tool | `deep_research` with `question` and `max_rounds` |

Only the main role (`lead`) may call the tool. `route.rs` refuses it for scout, worker, judge, tool, condenser, filter and router with `NOT called. A scout has no tool deep_research`. Its result has origin level 0, like `research`.

The command line prints progress to stderr and the summary to stdout. The report is a file.

## The steps

1. Clarify. If the person can answer (the full screen interface or an editor with `ask_user` v2) and a model call finds the question ambiguous in terms, region, time range or depth, Penelope asks 1 to 4 questions. The answers become part of the question. Nobody can answer on the plain command line, so there the step is skipped and the unconfirmed reading goes into `assumptions.md`.
2. Plan. A planner splits the question into 3 to 6 sub-questions that exclude each other and together cover the question. Each has key questions and preferred source types. Fewer than 3 gets one retry, then three fixed angles (definitions, measured evidence, limits and criticism) fill up. More than 6 are cut.
3. Research. One researcher per sub-question, in threads, at most `research.max_parallel` at a time (default 3). A researcher writes 2 or 3 search queries, searches, reads up to `research.pages_per_sub` (default 4) full pages, at most 2 per host, and writes notes. A page longer than `research.page_chars` (default 12000) is stored as a Mitos attachment; the model reads its opening and the passages that share most words with the key questions, and the notes file names the attachment. Hits it did not open stay as snippets. Searches run one per 800 ms across all researchers, because the keyless engines refuse faster clients.
4. Gap loop. After each round a judge lists missing parts, contradictions and unsupported claims, marks each as material or minor, and plans the next round: a deeper read of a sub-question or a new sub-question. The loop stops when no material gap is left, at `max_rounds` (setting `research.max_rounds`, default 3, 1 to 8, override per call), when the budget is used, when the judge names no next step, when a later round reads no new page, or when the judge fails. Every round and its reason to continue or stop go to `log.md` and to the summary.
5. Report. A writer turns the notes into a report. Sections, in this order: Summary, Measured facts, Author claims, Unverified (U), Own inference, Open gaps. Citations are numbers in order of first appearance; a number the writer made up becomes `[?]`. The Open gaps section comes from the last judge pass, not from the writer. If the writer model fails, the notes are sorted into the sections without it.
6. Verify. The checker fetches each cited source again, picks the passages closest to the claims and asks whether they support each claim: yes, partial or no. A claim whose citations all fail is removed and listed under Verification. A claim with one failing citation stays with a mark `(check: [n] not supported: reason)`. A number in a claim that does not occur in the source downgrades a yes to partial. If the checker model fails, a number and word-overlap test decides. A source that does not load again is listed as not re-checked and its claims stay.
7. Assumptions. `assumptions.md` lists every (U) note, every own inference, every removed or marked claim, every source that was not re-checked, every open gap and every unconfirmed reading of the question, each with a status and a way to test it.

## Notes format

A researcher writes one claim per line:

```
- [M] Throughput was 4200 requests per second (source: https://example.org/study)
- [C] The authors say it scales to many cores (source: https://example.org/study)
- [M] A figure from an abstract (source: https://example.org/abstract) (U)
- [I] Own inference from the lines above
```

`[M]` is a measured fact, `[C]` a claim of the author, `[I]` the model's own inference. Penelope adds `(U)` itself when a line rests only on a snippet or abstract, has no source, or names an address that is not among the sources of the round. Such a line cannot reach the Measured facts section.

## Files

Everything is in `.penelope/research/SLUG/`. SLUG comes from the filtered question, so a secret in the question never becomes a path. Another question with the same slug gets `-2`.

| File | Content |
|---|---|
| `s1.md`, `s2.md`, ... | Notes of one sub-question, one section per round, with the sources read and their Mitos attachment |
| `report.md` | The report with numbered citations, sources, verification |
| `assumptions.md` | Table of assumptions and unsupported claims |
| `log.md` | One line per round with the reason to continue or stop |
| `question.txt` | The question as given |

## Settings

```json
"research": {"max_rounds": 3, "max_parallel": 3, "pages_per_sub": 4, "page_chars": 12000, "max_queries": 3, "budget_eur": 0, "model": null}
```

`budget_eur` or `--budget` stops new rounds when the spent euro reaches the limit and skips the check when it reaches 1.5 times the limit. The claude command in text form reports no token counts, so the spend counts 4 characters per token. A model without a price in `prices` counts as zero and the summary says so.

Planning, judging and writing use `research.model` (same shape as `weak`) or the main model. Queries, notes and checking use the scout model, which is cheap and runs in parallel.

## Origin level and risk

Web text has origin level 0. The notes, the report, the assumptions and the tool result carry it: the files are recorded in the Kerkis taint map with level 0, so a later turn that reads them stays tainted. No step calls a tool that acts. The model calls have no tools. Every prompt and every query passes the privacy filter before it leaves the machine, and `search.rs` filters the query a second time. The gates treat a `deep_research` call with a secret in the question like a `research` call.

## Search without a server and without a paid key

Nothing in this mode needs an account or a key.

- Engines without a key: Startpage, DuckDuckGo HTML, Bing HTML, and Wikipedia. Wikipedia is asked when no other engine answers, or always when `search.wikipedia` is true.
- A local SearXNG is used when `search.searxng_url` is set, or when `search.url` or `search.urls` name one, or when `http://localhost:8888` answers. Penelope checks that port once per process with a 300 ms connect. Run one with `docker run -p 8888:8080 searxng/searxng`, and enable the JSON format in its `settings.yml` (`search.formats: [html, json]`), because Penelope asks for `format=json`.
- Pages are read by `scrape.rs` itself. `search.jina` is off by default.

Limits of the keyless setup:

- Rate limits. The HTML engines answer a few queries per minute per address. A run with 5 sub-questions and 3 queries each is 15 searches per round; the 800 ms spacing keeps most runs under the limit, but a second round can meet a block.
- Blocking. DuckDuckGo and Bing return a challenge page to a client they suspect. The hit list is then empty and the engine is marked failed in `penelope search health`. The run goes on with the engines that answered.
- Fewer and weaker results. The HTML engines give up to 10 hits without dates or scores. Academic papers behind a paywall give an abstract page, and claims from it get (U).
- Pages that need a script or a login do not load. The source stays a snippet.

Keyed engines are options that improve the hit list: Brave (`BRAVE_API_KEY`), Tavily (`TAVILY_API_KEY`), Exa (`EXA_API_KEY`), Firecrawl (`FIRECRAWL_API_KEY`). `penelope login tavily` stores a key in the Secret Service. With `search.fanout` on, every configured engine answers and the results are merged.

### Main model `claude-cli`

When the main model is `claude-cli`, Claude Code has web tools of its own (WebSearch and WebFetch). They are an alternative, but their queries do not pass Penelope's privacy filter and their pages do not go through the origin level of the gates. Penelope therefore prefers its own search and does not call those tools.

## Not done

- No stop button: `esc` in the terminal UI does not end a running deep research; the threads finish their step.
- A (U) claim that the checker confirms on the full page keeps its (U).
- The checker compares a claim with the passage that shares most words with it. A claim built from two distant passages can be marked partial by mistake.
- Searches run one at a time, so a run with many sub-questions spends most of its time in the rate limit spacing.
