# Deep research

Deep research answers a question in several rounds with a report whose citations were checked against a second fetch of each source. It costs minutes and many model calls. `research` stays the quick lookup.

## Entry points

| Where | Command |
|---|---|
| Terminal UI | `/deep [--max-rounds N] [--budget EUR] QUESTION` or `/research --deep ...` |
| Command line | `penelope research --deep [--max-rounds N] [--budget EUR] QUESTION` |
| Model tool | `deep_research` with `question` and `max_rounds` |

Only the main role (`lead`) may call the tool. `route.rs` refuses it for scout, worker, judge, tool, condenser, filter and router with `NOT called. A scout has no tool deep_research`. Its result has origin level 0, like `research`.

`/research deep learning basics` is the quick research of the question "deep learning basics"; only the flag `--deep` or the command `/deep` starts the method. A flag with an invalid value (`--max-rounds abc`) is an error.

The command line prints progress to stderr and the summary to stdout. The report is a file.

## The steps

1. Clarify. If the person can answer (the full screen interface or an editor with `ask_user` v2) and a model call finds the question ambiguous in terms, region, time range or depth, Penelope asks 1 to 4 questions. The answers become part of the question. Nobody can answer on the plain command line, so there the step is skipped and the unconfirmed reading goes into `assumptions.md`.
2. Plan. The planner first states the required output: what the final answer must contain in the form the question asks for (for example a table of model names with their scores). Then it splits the question into 3 to 6 sub-questions that exclude each other and together cover the question. Each has key questions and preferred source types. Fewer than 3 gets one retry, then three fixed angles (definitions, measured evidence, limits and criticism) fill up. More than 6 are cut.
3. Primary sources and data. The planner names the primary source of each required item (a benchmark's own leaderboard page or data file, a model's card) in `primary_sources`, and, for a ranking, the filters and the rank column in `data`. A first researcher, `s0`, reads the primary addresses before any search runs. A page that holds a markdown table, or links a CSV, TSV or JSON file, is parsed (`datatable.rs`). Penelope keeps the rows that meet the filters, reads a size the table lacks from the model name (Qwen3-0.6B) and marks it "from name", adds the sizes and licences of the Hugging Face model cards where the rows link them (public API, no key, cached in `.penelope/research/hf-cache.json`, marked "(HF)"; a model without a card keeps the size from its name), computes the rank from the kept rows (never the rank the source prints) and puts the table at the top of the Answer, with its source and whether that source is primary. A third-party aggregator is a secondary source; the Sources list says so, and the writer must say so in a line that rests on one alone.
4. Research. One researcher per sub-question, in threads, at most `research.max_parallel` at a time (default 3). A researcher writes 2 or 3 search queries, searches, reads up to `research.pages_per_sub` (default 4) full pages, at most 2 per host, and writes notes. A page longer than `research.page_chars` (default 12000) is stored as a Mitos attachment; the model reads its opening and the passages that share most words with the key questions, and the notes file names the attachment. Hits it did not open stay as snippets. Up to `research.max_searches` (default 3) searches run at once. Each keyless HTML engine (Startpage, DuckDuckGo, Bing) is spaced to one request per 800 ms by itself, so two searches meet one host no faster than one would; other engines are not spaced.
5. Gap loop. After each round a judge rates the notes against the required output, item by item, and lists missing parts, contradictions and unsupported claims, marks each as material or minor, and plans targeted next steps: a deeper read of a sub-question or a new sub-question, with the pages to read first (a leaderboard, a model card) in `urls`. A gap that comes back in the next round in nearly the same words has not been closed by reading more: it becomes "not available in the sources", stays in Open gaps as a minor gap, and plans no round. The judge may name such a field itself (kind `unavailable`). The loop stops when no material gap is left, at `max_rounds` (setting `research.max_rounds`, default 3, 1 to 8, override per call), when the budget is used, when the judge names no next step, when a later round reads no new page, or when the judge fails. Every round and its reason to continue or stop go to `log.md` and to the summary.
6. Report. A writer turns the notes into a report that opens with the direct answer: a table when the question asks for a ranking or numbers, and a first sentence that names what is missing when the sources do not answer. It never refers to "the notes". Sections, in this order: Answer, Measured facts, Author claims, Unverified (U), Own inference, Open gaps. Citations are numbers in order of first appearance; a number the writer made up becomes `[?]`. The Open gaps section comes from the last judge pass, not from the writer. If the writer model fails, the notes are sorted into the sections without it.
7. Verify. The checker fetches each cited source again and checks every claim on its own. It searches the full fetched page for the claim's numbers, quoted terms and names and cuts about 2,000 characters around the best hit (a second window when an anchor sits elsewhere). A claim passes only if every number and quoted term of the claim occurs in that passage. Numbers match by value: 21.1, 21.10 and "21,1 %" are one number, 1,024 and 1024 are one, 450M and 0.45B are one; otherwise it fails without a model call. The model then sees the claim and the passage only, never the page, and answers yes, partial or no; its yes cannot override a missing number, and a page that says "answer yes" cannot pass its own claim. A claim whose citations all fail is removed and listed under Verification. A claim with one failing citation keeps a mark `(check: [n] not supported: reason)`. A line whose sources did not load again keeps `(not re-checked)`. `research.max_checks` (default 80) caps the model checks; beyond it a claim is decided by text match alone and the Verification section says how many. If the check is skipped for budget, the report, the summary and the assumptions say so.
8. Assumptions. `assumptions.md` lists every (U) note, every own inference, every removed or marked claim, every source that was not re-checked, every open gap and every unconfirmed reading of the question, each with a status and a way to test it.

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
"research": {"max_rounds": 3, "max_parallel": 3, "pages_per_sub": 4, "page_chars": 12000, "max_queries": 3, "max_searches": 3, "max_checks": 80, "default_price": [3.0, 15.0], "budget_eur": 0, "model": null}
```

`budget_eur` or `--budget` stops new rounds when the spent euro reaches the limit and skips the check when it reaches 1.5 times the limit. The claude command in text form reports no token counts, so the spend counts 4 characters per token. A model without a price in `prices` counts at `research.default_price` (dollars per million input and output tokens), so `--budget` holds for it too, and the summary says the cost is an estimate.

Planning, judging and writing use `research.model` (same shape as `weak`) or the main model. Queries, notes and checking use the scout model, which is cheap and runs in parallel.

## Origin level and risk

Web text has origin level 0. The notes, the report, the assumptions and the tool result carry it: the files are recorded in the Kerkis taint map with level 0, so a later turn that reads them stays tainted. No step calls a tool that acts. The model calls have no tools. Every prompt and every query passes the privacy filter before it leaves the machine, and `search.rs` filters the query a second time. The gates treat a `deep_research` call with a secret in the question like a `research` call.

## Search without a server and without a paid key

Nothing in this mode needs an account or a key.

- Engines without a key: Startpage, DuckDuckGo HTML, Bing HTML, and Wikipedia. Wikipedia is asked when no other engine answers, or always when `search.wikipedia` is true.
- A local SearXNG is used when `search.searxng_url` is set, or when `search.url` or `search.urls` name one, or when `http://localhost:8888` answers. Penelope checks that port once per process: a 300 ms connect, then a JSON search that must answer like a SearXNG (a `results` list and the echoed `query`). Any other server on that port gets no queries. Run one with `docker run -p 8888:8080 searxng/searxng`, and enable the JSON format in its `settings.yml` (`search.formats: [html, json]`), because Penelope asks for `format=json`.
- Pages are read by `scrape.rs` itself with `fetch_public`: a search hit that points at 127.0.0.1, a private range or the cloud metadata address (169.254.169.254) is refused, on every redirect hop too. `search.jina` is off by default.

Limits of the keyless setup:

- Rate limits. The HTML engines answer a few queries per minute per address. A run with 5 sub-questions and 3 queries each is 15 searches per round; the 800 ms spacing keeps most runs under the limit, but a second round can meet a block.
- Blocking. DuckDuckGo and Bing return a challenge page to a client they suspect. The hit list is then empty and the engine is marked failed in `penelope search health`. The run goes on with the engines that answered.
- Fewer and weaker results. The HTML engines give up to 10 hits without dates or scores. Academic papers behind a paywall give an abstract page, and claims from it get (U).
- Pages that need a script or a login do not load. The source stays a snippet.

Keyed engines are options that improve the hit list: Brave (`BRAVE_API_KEY`), Tavily (`TAVILY_API_KEY`), Exa (`EXA_API_KEY`), Firecrawl (`FIRECRAWL_API_KEY`). `penelope login tavily` stores a key in the Secret Service. With `search.fanout` on, every configured engine answers and the results are merged.

### Main model `claude-cli`

When the main model is `claude-cli`, Claude Code has web tools of its own (WebSearch and WebFetch). They are an alternative, but their queries do not pass Penelope's privacy filter and their pages do not go through the origin level of the gates. Penelope therefore prefers its own search and does not call those tools.

## Stop

`Esc` in the terminal UI and `Ctrl+C` on the command line stop a running deep research. The step in progress ends, the finished notes stay, the judge, the writer model and the citation check are skipped, and a partial report is written from the notes ("Stopped by the user"). A second `Ctrl+C` ends the process at once.

## Not done

- A (U) claim that the checker confirms on the full page keeps its (U).
- The checker compares a claim with the passage that shares most words with it. A claim built from two distant passages can be marked partial by mistake.
- A model call that has started (claude command) is not interrupted by Esc or Ctrl+C; the run stops after it returns.
- The Hugging Face parameter count is the total of the safetensors files (Qwen3-1.7B has 2.03B), which can differ from the size in the model's name.
