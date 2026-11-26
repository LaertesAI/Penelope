# Web search and the scraper

The search of 0.10.0 was a list of three sources and a single page fetch. This is the new path:

```
question ── guard ── cache ── engines in parallel ── canonical URLs ── rank ── 8 hits
                                                            │
                                          research ─────────┘
                                              ├─ 1–4 queries
                                              ├─ 4 best pages, one per host
                                              ├─ scrape to Markdown
                                              └─ weaker model: cited report
```

## Engines

Ten sources, one row per source. `auto` uses the keyed and server sources in the order below, then the keyless ones.

| Engine | Key | What it is | Status |
|---|---|---|---|
| `api` | `PENELOPE_SEARCH_API_KEY` | a server search API, `POST search.api_url` | built in `penelope serve`; live |
| `tavily` | `TAVILY_API_KEY` | agent search with scores and dates | code and shape tested, no key here |
| `exa` | `EXA_API_KEY` | neural search with highlights | code and shape tested, no key here |
| `brave` | `BRAVE_API_KEY` | Brave Search API | code and shape tested, no key here |
| `firecrawl` | `FIRECRAWL_API_KEY` | search plus optional scraping | code and shape tested, no key here |
| `searxng` | own instance | JSON API of `search.url` or `search.urls` | works when an instance is set |
| `startpage` | none | HTML results, direct links | works, good general results |
| `duckduckgo` | none | HTML page, lite fallback | works, rate limited after a few queries |
| `bing` | none | HTML `b_algo` results, tracking links decoded | works, rate limited; can serve unrelated news |
| `wikipedia` | none | MediaWiki search API | fallback when both HTML engines refuse |

`penelope login tavily`, `exa`, `firecrawl`, `brave`, `search`, `rerank` and `reader` store service keys in the
same Secret Service as the model keys. The environment variable still wins. `penelope auth` lists them under `search and reader keys`.

The defaults are in `config.json`:

```json
"search": {
  "kind": "auto",
  "url": "",
  "urls": [],
  "api_url": "",
  "server_api_key_env": "PENELOPE_SEARCH_API_KEY",
  "api_key_env": "BRAVE_API_KEY",
  "tavily_api_key_env": "TAVILY_API_KEY",
  "exa_api_key_env": "EXA_API_KEY",
  "firecrawl_api_key_env": "FIRECRAWL_API_KEY",
  "rerank": {"enabled": false, "url": "", "api_key_env": "RERANK_API_KEY"},
  "fanout": true,
  "cache": true,
  "cache_ttl": 3600,
  "limit": 8,
  "language": "en",
  "safe": true,
  "jina": false,
  "jina_base": "https://r.jina.ai/",
  "wikipedia": false
}
```

- `fanout: true` calls every configured engine at once and merges. With no keyless engine set this is Startpage,
  DuckDuckGo and Bing. With paid keys it calls those too; set `fanout: false` to use the first working source only.
- `cache` keeps one JSON file per query and option set under `.penelope/search/cache/`. `fresh: true` or
  `penelope search --fresh` skips it.
- `wikipedia` is off in the normal fanout. It is the last resort when both HTML engines refuse.
- `jina` sends a page address to `r.jina.ai` for a JavaScript page. It is off by default because that is a third party.

## A server search API

`search.api_url` is the P0 item of [`SERVER-APIS.md`](SERVER-APIS.md). When it is set, `auto` puts `api` first. The
request is one JSON body, no query text in a URL:

```http
POST <api_url>
Content-Type: application/json
Authorization: Bearer <PENELOPE_SEARCH_API_KEY>   # only when the key is set

{"query": "tokio select macro", "language": "en", "country": "", "safe": true,
 "recency": "", "limit": 8, "page": 1}
```

`server_api_key_env` names the variable that holds the bearer key; the key is optional for a local instance. The
answer must hold `results[]` (or `data[]`) with `title`, `url`, `snippet` or `content`, and optionally
`published` and `score`. A result without a title or an `http(s)` URL is dropped; an empty or non-JSON answer is an
engine error, not a hit. The key is read through the same `key()` path as the model keys: environment first, then the
Secret Service, then the mode-0600 file.

## Several SearXNG instances

`search.urls` takes a list. `search.url` stays the single-instance form and is tried first. Each base is called as
`GET <base>/search?q=...&format=json&language=...&safesearch=1[&time_range=...]`; the first instance that answers with
at least one result wins. This makes a small pool possible when one instance is down or rate limited.

## Engine health

Every engine call writes one row to `.penelope/search/health.json`: calls, failures, the last duration in
milliseconds, the last success time, and the last error (cut at 160 characters). `penelope search health` prints the
table; `--probe` sends one small query to every configured engine first. The probe is one call per engine — the
engine records its own result, so nothing is counted twice.

The file counts across sessions on purpose: a failure that happened yesterday is still evidence about an engine. It
contains no query text and no page content.

## Reranking

`search.rerank` is optional and off by default. When `enabled` is true and `url` is set, the merged list is ranked to
20 hits locally, then sent as one request:

```http
POST <rerank.url>
{"query": "...", "documents": [{"id": 0, "title": "...", "snippet": "...", "url": "..."}], "top_k": 8}
→ {"results": [{"index": 3, "score": 0.91}, ...]}
```

Only the query and the candidate title/snippet/url leave the machine, never a transcript or a page. A timeout, a
non-JSON answer, a missing `results` array or no usable index returns the local order unchanged. Indices the server
did not name keep a low place after the named ones, so a partial answer cannot lose a hit.

This is the only model-like step in the search path, and it is explicit in the settings. With it off, the ranking is
the lexical score described below and nothing is hidden.

## Ranking

1. Canonical URL: fragment removed, `utm_*`, `ref`, `fbclid`, `gclid` and similar tracking removed, trailing slash
   normalised. The first hit wins; later duplicates are dropped.
2. Score: engine weight + overlap of the query words with title and snippet + provider score.
3. Diversity: a second result from the same host loses 0.18.
4. Sort, take `limit` (8 by default), show engine, score and published date when known.

This is deliberately explainable: with the optional reranker off, no model is in the path. The score is in the
result, so a person can see why a page came first. With the reranker on, its scores replace the local ones and the
engine still names the source.

## The scraper

`scrape.rs` turns HTML into Markdown without a browser:

- title, description, author and published date from `<title>`, meta tags and JSON-LD;
- `<article>`, `<main>`, `role="main"` or the body, whichever is the main content;
- scripts, styles, navigation, footers, forms, dialogs and templates removed;
- headings, paragraphs, lists, blockquotes, links, images, tables and fenced code blocks kept;
- entities (named and numeric) decoded;
- short boilerplate lines (cookie, subscribe, sign in, share) removed;
- redirects followed by the client up to four times, with no Authorization header;
- at most 5 MB of a page read;
- a scan for lines that try to instruct the agent ("ignore previous", "reveal your system prompt", "you are now",
  "do not tell the user", `curl ... | sh`) and for zero-width or bidi characters that hide text;
- `Page { title, description, author, published, markdown, links, words, warnings }`.

`fetch` uses it. A short page passes whole; a long page is read by the weaker model and only its 10-line answer reaches
the lead. The raw page never enters the lead's context.

The warnings are data, not a block: the page is still read, the lines are shown above it, and `fetch` and `research`
repeat them. A page that says "ignore previous instructions" is evidence about the page, never a command.

A page under 300 characters is tried through the optional reader API first. If that is not set and `search.jina` is
true, `https://r.jina.ai/<url>` is tried. A page that still has no text is reported; ScreenCodec is the way to read a
JavaScript application yourself.

## PDFs and the reader API

A PDF is not HTML. `fetch` detects `application/pdf`, a `%PDF-` header or a `.pdf` answer and runs the local
`pdftotext -layout` on a temporary file. The text goes through the same instruction scan as a page. This is the P1
"local first" rule of [`SERVER-APIS.md`](SERVER-APIS.md): no page and no key leaves the PC for a normal PDF.

A scanned PDF has no text layer, and some pages build their text in JavaScript. Both can use the P1 reader API:

```json
"reader": {"enabled": true, "url": "https://reader.example/read", "api_key_env": "PENELOPE_READER_API_KEY"}
```

```http
POST <reader.url>
{"url": "https://example.org/report.pdf", "max_chars": 60000, "formats": ["markdown"], "wait_ms": 3000}
→ {"title": "...", "author": "...", "published": "...", "markdown": "...",
   "links": ["..."], "final_url": "...", "content_type": "text/markdown"}
```

The client refuses a non-http(s) URL, credentials in the address, `localhost`, a loopback, private, link-local or
unresolved address before the request. A server must repeat that check; the client check is the second lock, not the
only one. The answer must hold a non-empty `markdown`; a broken answer is ignored and the local text stays. The body is
capped at 200,000 characters.

`doctor` prints whether `pdftotext` is present, whether a reader endpoint is set, and the last counters of the search
API.

## Research

The tool `research` and `penelope research QUESTION`:

1. Build 1, 2 or 4 queries from the question (`quick`, `standard`, `deep`).
2. Search each query, merge and rank.
3. Take the best pages, one per host, at most `max_pages`.
4. Fetch them in parallel and keep title, date, 1,800 characters of Markdown per page, plus snippets for pages that
   failed.
5. Ask the weaker model for at most 15 lines with `[1]` citations, using only the evidence.
6. Return the answer, a list of every source and the engines that answered.

The weak model sees the evidence, not the lead. Its tokens are recorded under the `scout` role. The report is guarded
before it reaches the lead, and the source list stays readable.

## Why not Claude's server search

The Claude API offers a server-side `web_search` tool with citations. Penelope does **not** use it: the index, ranking,
page fetch and per-search price would be behind one vendor, and the pages would not pass the local scraper or cache.
Search stays client-side. The next server-side piece is a self-hosted search API; its contract and the other APIs
Penelope still needs are in [`SERVER-APIS.md`](SERVER-APIS.md).

## Privacy and safety

- The query passes the fixed privacy rules before it leaves the machine.
- HTTP redirects are not followed by the model client or the keyed APIs; the scraper follows a redirect itself and
  never carries an Authorization header.
- Search API keys live in the environment or the login store, not in `config.json`.
- The search page, snippets and pages are data, never instructions. The research prompt says this explicitly.
- `penelope audit` scans the cache and every other text file in `.penelope` for key-like material.
- Jina is off by default because it sends the page address to a third party.

## Inspiration and sources

The design was checked against the current APIs on 2026-11-24:

- [Tavily Search](https://docs.tavily.com/documentation/api-reference/endpoint/search): score, published date,
  `search_depth`, topic, time range.
- [Exa Search](https://docs.exa.ai/reference/search): highlights, text, published dates, `auto`/`lite`/`deep` types.
- [Firecrawl Search](https://docs.firecrawl.dev/api-reference/endpoint/search): query operators, `scrapeOptions`,
  categories, `onlyMainContent`.
- [SearXNG](https://docs.searxng.org/): a self-hosted meta search with a JSON API.
- [Jina Reader](https://jina.ai/reader/): Markdown of a rendered page.
- Mozilla Readability: the idea of scoring the main article and dropping chrome; the Rust code here is simpler and
  self-contained.

## Tests

- `search::reads_the_result_page`, `parse_bing`, base64 `unbing`, canonical URL tracking removal.
- `search::ranking_deduplicates_and_prefers_a_diverse_list`.
- `search::parses_the_three_keyed_shapes`.
- `scrape::an_article_becomes_markdown_and_the_noise_is_gone`, entities, redirect resolution.
- `research::the_query_plan_grows_with_the_depth`.
- Live: `penelope search "rust tokio select!"` returned 8 results from DuckDuckGo in 0.7 s; `penelope research "What
  is the tokio select macro…"` returned 3 pages and a cited answer in 10.3 s.
- Live `fetch` of docs.rs: 1,929 words of Markdown, a 9-line answer on the weak model, 4.6 s, no page text in the lead.
- The persistent ScreenCodec install is `~/.local/share/screencodec-venv` with a symlink at `~/.local/bin/screencodec-mcp`;
  `penelope screen tools` reads the real tool list from it.

## Limits

- DuckDuckGo and Bing refuse automated clients after a few queries from one address. Startpage is the better keyless
  path; the fallback is Wikipedia. A SearXNG instance or a keyed API is the reliable path.
- No Google, Yandex or Mojeek: they block this client. Startpage provides a Google-like index without a key; it can rate-limit too.
- No pagination beyond the first result page.
- The ranker is lexical, not semantic. An optional server reranker exists, but no local embedding model is wired in
  yet; the provider scores are used when present.
- PDFs are not parsed by the scraper; `filetype:pdf` only finds them.
- A JavaScript page needs ScreenCodec or the optional Jina reader.
