# Server-side APIs Penelope needs

Penelope searches **client-side first**: the query goes to engines the user chose, the results are ranked locally, and
the page is read by the local scraper. That stays the default. This file says which server-side APIs are still needed,
what each one must return, and what is deliberately not used.

The first server is now in this repository: `penelope serve` implements search, read, embed, rerank and redact with
no model call. The endpoint shapes, limits and security rules are in [`SERVER.md`](SERVER.md). The contracts below
remain the target for the endpoints that are not built yet.

**Claude's `web_search` server tool is not the path.** It is a good product, but it hides the index, the ranking and the
page fetch behind one vendor and bills per search. Penelope does not use it, and has no Claude search backend at all.

## P0 — the search API

The client-side chain is fragile because the keyless HTML engines (DuckDuckGo, Bing, Startpage) rate-limit automated
clients. A self-hosted meta-search or a search API fixes that without a model vendor.

### What it must do

- one endpoint, JSON in and JSON out;
- query, language, country, safe search, date range, page number, result count;
- results with `title`, `url`, `snippet`, `engine`, `published`, optional `score`;
- no query text in a URL that lands in a proxy log, HTTPS only, no tracking parameters;
- a documented rate limit that fits a single developer plus a few agents;
- response time under 1.5 s at 10 results for 95 % of queries;
- a cache header that lets Penelope keep a result for an hour;
- attribution rules that fit the sources;
- an operator who can be asked to raise the limit.

### Preferred shape

A self-hosted **SearXNG** instance is the first choice: it is open source, has a JSON API and can be run on the same PC
or a small server.

```http
GET /search?q=tokio+select+macro&format=json&language=en&safesearch=1&pageno=1
200 application/json
{
  "query": "tokio select macro",
  "number_of_results": 42,
  "results": [
    {"title": "select in tokio", "url": "https://docs.rs/tokio/latest/tokio/macro.select.html",
     "content": "The select! macro ...", "engine": "brave", "publishedDate": "2026-11-24", "score": 0.91}
  ]
}
```

Fallbacks, in order: MetaGer, Brave Search API, Tavily, Exa, Firecrawl. All except SearXNG are paid or rate-limited and
need a privacy review before they become a default.

**Status:** built. `penelope serve` answers this shape, `search.api_url` sends the POST body above,
`search.server_api_key_env` names the bearer key, and `search.urls` tries several SearXNG instances.
`penelope search health` records calls, failures, latency and the last error; `penelope doctor` prints the endpoint
and its last success. The server clears its own `api_url`, so it cannot call itself.

### Acceptance

- 100 queries in a row, at least 98 with a result;
- p95 latency < 1.5 s and cache hit latency < 20 ms;
- the query never appears in a server log of Penelope;
- `penelope doctor` says which search API is live and when it was last checked.

## P1 — read and render API

The local scraper reads static HTML. A page that needs JavaScript, a PDF, a scan or a login needs a renderer.

- **Local first:** ScreenCodec with its own Chromium is already installed and can read a page as text.
- **Server fallback:** `POST /read {url, max_chars, formats:["markdown"], wait_ms}` returns
  `{title, author, published, markdown, links, final_url, content_type}`.
- Must enforce an allowlist, block private addresses (SSRF), carry no cookies or Authorization header, cap the page at
  5 MB, time out after 30 s and never store the page unless the user asks.
- PDF and OCR: `pdftotext` locally first; a server parser only for a scan or a broken PDF.
- Acceptance: a JavaScript-only page and a 20-page PDF both come back as Markdown; a `file://` or `10.0.0.1` address is
  refused; no request carries a key.

**Status:** built. `penelope serve` answers this shape. `reader.url` sends the POST body above, `pdftotext -layout`
handles a normal PDF locally, and `scrape::target_is_public` refuses loopback, private, link-local, credential-bearing
and unresolved targets before the request. The server repeats the check.

## P1 — embedding and rerank API

Memory is a tree; search is lexical. Both improve with vectors.

- `POST /embed {texts:[...], model}` → vectors; local model preferred (e.g. a small embedding server on this PC).
- `POST /rerank {query, documents:[...], top_k}` → indices and scores.
- Accept only the query and the candidate snippets, never a whole transcript.
- Acceptance: a synonym query finds the note with another word; 20 search hits become 8 with a measured precision gain;
  the endpoint has a token budget and a price per call.

**Status:** built, but deterministic. `penelope serve` answers `/rerank` with BM25 and `/embed` with 256-dimensional
feature-hash vectors. `search.rerank` sends the query and the top 20 hits, accepts only `{results:[{index, score}]}`,
and keeps the local order when the answer is broken. A semantic embedding model is still open.

## P1 — privacy / PII API (Utis)

The fixed rules and the optional `roles.filter` model are the current floor. A dedicated redaction service would be
better than one more chat model.

- `POST /redact {text, level}` → `{text, items:[{kind, start, end}], masked}` with byte offsets.
- Strict JSON, no prose, deterministic versioning, local deployment preferred.
- Acceptance: a test set of 200 planted secrets, names, addresses and IBANs; recall > 0.98, false positives < 2 %;
  a broken response falls back to the fixed rules and is counted.

**Status:** partly built. `penelope serve` answers `/redact` with fixed rules, byte offsets and the levels `off`,
`rules` and `pii`. Names, phone numbers and addresses are not recognised yet, and no model is involved.

## P1 — model endpoints for the families

All of these can already be an OpenAI-compatible endpoint; the list is what a server must expose per family.

| Family | Role | Endpoint | Contract |
|---|---|---|---|
| Nostos | tool calling | `/chat/completions` with tools | strict tool names and JSON schemas, one call per request |
| Tiresias | router | `/route` or chat with JSON | a plan with roles and limits, fallback on invalid JSON |
| Aiolos | condenser | `/chat/completions` | max output tokens, no tools, privacy filter before the call |
| Argos | judge | `/check` or chat | `PASS/FAIL` plus one reason, evidence only |
| Telemach | coding | `/chat/completions` with tools | tool loop, streaming, stop reasons |
| Utis | privacy | `/redact` | see above |
| Klea | data filter | `/filter` | keep/drop plus reason, no generation |
| Proteus | ML engineering | `/train` | job id, status, loss, checkpoint |

Common requirements: model name, context size and price in a `/models` response; a health endpoint; an error shape that
names the request id; a timeout; and a token count that Penelope can put into `cost`.

## P2 — update server

`docs/AUTOUPDATE.md` already defines the manifest. The missing server is one static folder:

- `stable.json`, `beta.json`, one artifact per platform;
- SHA-256 in every manifest, minisign signature, a public key in `config.json`;
- artifact first, manifest second; a broken manifest never points at a missing file;
- rollback by replacing the manifest with the previous version.

## P2 — plugin registry

`/plugins install` accepts a Git URL today. A registry would make discovery and trust better:

- `GET /index.json` → plugins with name, description, source, version, signature, permissions;
- permissions per tool, not per plugin: `read`, `network`, `write`, `browser`, `desktop`;
- a signed plugin archive and a SHA-256; a revoked-version list;
- Claude Code marketplace files stay installable without the registry.

## P2 — auth broker

The Secret Service is enough for one PC. A team needs refresh:

- OAuth device flow for Claude, OpenAI and the other providers;
- token refresh with a scope and an expiry, stored in the OS keyring;
- a `/whoami` that returns the provider, plan and expiry, never the token;
- a local fallback when no browser is available.

## Not needed

- A Claude server search backend. It is explicitly out of the design.
- A proprietary index of the whole web. A self-hosted meta-search over engines that allow it is enough for one developer
  and a few agents.
- A cloud vector database. A local embedding server and a file of vectors are enough for the memory of one person.
- Telemetry that sends prompts. A cost counter can be local; the content stays here.

## Order

1. **Search API** — the one that unblocks daily work.
2. **Reader/render API** — the one that unblocks JavaScript and PDFs.
3. **Embedding/rerank** — the one that makes memory feel intelligent.
4. **Privacy API** — the one that makes the privacy promise measurable.
5. **Model endpoints** — one at a time, each behind the same role contract.
6. **Update, registry, auth** — the ones that make distribution and a team possible.

Every item gets a contract test before it is called done; a server that answers with the wrong shape is worse than no
server.
