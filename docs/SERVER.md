# The Penelope server

`penelope serve` is the local HTTP API of the same binary. It turns the contracts of
[`SERVER-APIS.md`](SERVER-APIS.md) into five working endpoints:

| Endpoint | What it does | Model-free |
|---|---|---|
| `GET /health` | version, uptime, search sources, auth mode, limits | yes |
| `GET /models` | the two local models this server offers | yes |
| `GET /whoami` | the service identity and whether the bearer token is valid | yes |
| `POST /search` | meta-search over the configured engines, with cache and ranking | yes |
| `POST /read` | HTML or PDF to Markdown, with SSRF protection | yes, local `pdftotext` |
| `POST /embed` | 256-dimensional deterministic token/bigram vectors | yes |
| `POST /rerank` | BM25 over the supplied documents | yes |
| `POST /redact` | fixed-rule secret and PII masking with byte offsets | yes |

The health answer lists `GET /whoami` among the endpoints. `auth verify` is the client-side counterpart for model
providers and does not use this endpoint.

No endpoint calls a language model. Search may call a SearXNG instance or a keyed search API if the settings
configure one, but it never calls itself: the server clears `search.api_url`, `search.rerank` and `reader.url` in its
own copy of the configuration.

## Start

```bash
penelope serve                         # 127.0.0.1:8765
penelope serve --bind 127.0.0.1:9000
penelope serve --print-config          # the client settings as JSON
```

The default bind is loopback. A non-loopback bind is refused unless both are true:

```bash
export PENELOPE_SERVER_TOKEN="$(python3 -c 'import secrets; print(secrets.token_urlsafe(32))')"
penelope serve --bind 0.0.0.0:8765 --allow-remote
```

With a token, every `POST` needs `Authorization: Bearer $PENELOPE_SERVER_TOKEN`. `GET /health` stays open so a
supervisor can probe it without the secret. `GET /whoami` requires the token and answers `{"token_valid": true}`;
without it the answer is `401`. That is the verification step for a client, not a login page. There is no TLS in the binary: a remote deployment belongs behind a
reverse proxy that terminates TLS.

The rate limit is 120 requests per minute per client address. A client over it gets `429` and a JSON error.

## Search

```http
POST /search
Content-Type: application/json

{"query": "tokio select macro", "limit": 8, "page": 1,
 "language": "en", "safe": true, "recency": "month", "fresh": false}
```

`limit` is 1 to 20, `page` 1 to 5, `recency` is empty, `day`, `week`, `month` or `year`. The client sends the query in
the body; no query text lands in the URL of this API. `fresh` skips the local cache.

```json
{
  "query": "tokio select macro", "page": 1, "limit": 8, "number_of_results": 8,
  "engines": ["startpage", "duckduckgo", "bing"],
  "results": [
    {"title": "select in tokio - Rust", "url": "https://docs.rs/tokio/latest/tokio/macro.select.html",
     "snippet": "The tokio::select! macro ...", "engine": "duckduckgo",
     "published": "", "score": 1.42}
  ]
}
```

If every engine refuses, the answer is `502` with the engine errors. The server uses the same cache, canonical URLs
and diversity rule as the client. It does not call a reranker; the client can point `search.rerank.url` at this
server's `/rerank` and reorder the answer. It never calls its own `/search`: `api_url` is cleared in the server copy.

## Read

```http
POST /read
{"url": "https://example.org/report.pdf", "max_chars": 60000}
```

The answer:

```json
{"url": "...", "final_url": "...", "title": "...", "description": "...", "author": "...",
 "published": "...", "markdown": "...", "links": ["..."], "words": 1234,
 "content_type": "text/html", "warnings": []}
```

- `http` and `https` only.
- The target must resolve to a public address. Loopback, private, link-local, credential-bearing and unresolved
  targets are refused before a request is made. Every redirect is checked again, so a public URL cannot bounce into
  the local network. A server must repeat that check; this is the second lock.
- HTML becomes Markdown through `scrape.rs`; a PDF goes through the local `pdftotext -layout` first.
- The page is capped at 5 MB and `markdown` at `max_chars` (1,000 to 200,000, default 60,000).
- No cookies, no Authorization header and no renderer are used. A JavaScript-only page is still the job of
  ScreenCodec or the optional reader API on the client side.

## Embed

```http
POST /embed
{"texts": ["the deployment runs in Frankfurt", "der Server läuft in Frankfurt"], "model": "penelope-hash-256"}
```

```json
{"model": "penelope-hash-256", "dimensions": 256,
 "vectors": [[0.0, -0.31, ...], [0.0, 0.12, ...]]}
```

This is a deterministic feature-hash embedding: lower-cased words and word pairs are hashed into 256 dimensions with
signed weights and the vector is L2-normalised. It is useful for lexical similarity and a local memory index, and it
never downloads a model. It is **not** a semantic transformer embedding: paraphrases with no shared word stay far
apart. An embedding server with a real model can replace it behind the same shape.

Limits: 1 to 64 texts, each at most 20,000 characters.

## Rerank

```http
POST /rerank
{"query": "rust error handling", "top_k": 3,
 "documents": ["cooking pasta", {"title": "Rust error handling with anyhow", "snippet": "..."}]}
```

```json
{"results": [{"index": 1, "score": 1.0}, {"index": 0, "score": 0.0}]}
```

The score is BM25 over the query words and the candidate text, with a phrase bonus. A document may be a string or an
object with `title`, `snippet`, `content`, `text` and `url`. The scores are normalised so the best hit is `1.0`.
This is lexical: it improves a list that already holds the answer, it does not find synonyms.

Limits: 1 to 100 documents, `top_k` 1 to 100.

## Redact

```http
POST /redact
{"text": "password=hunter2 mail a@b.com", "level": "pii"}
```

```json
{"text": "password=[masked:secret] mail [masked:email]",
 "items": [{"kind": "secret", "start": 9, "end": 16},
           {"kind": "email", "start": 22, "end": 29}],
 "masked": true}
```

- `level`: `off`, `rules` (default) or `pii`.
- `rules` masks private-key blocks, known key prefixes (`sk-`, `ghp_`, `AIza`, `AKIA`, …), JWTs, bearer tokens and
  values behind `password`, `secret`, `token`, `api_key` and similar names.
- `pii` adds e-mail addresses and IBANs. It does not know names, phone numbers or addresses yet.
- `start` and `end` are byte offsets into the original text, so a caller can build its own masked copy.
- This is a fixed-rule filter. It is the floor, not a proof; a model-based Utis endpoint would be a later step.

## Client settings

`penelope serve --print-config` prints:

```json
{
  "search": {
    "kind": "api",
    "api_url": "http://127.0.0.1:8765/search",
    "server_api_key_env": "PENELOPE_SERVER_TOKEN",
    "rerank": {"enabled": true, "url": "http://127.0.0.1:8765/rerank",
               "api_key_env": "PENELOPE_SERVER_TOKEN"}
  },
  "reader": {"enabled": true, "url": "http://127.0.0.1:8765/read",
             "api_key_env": "PENELOPE_SERVER_TOKEN"}
}
```

Merge those keys into `.penelope/config.json` and the client uses the server. With no token the key variables can be
empty. `penelope doctor` then shows the search API and its last health row; `penelope search health` shows every
engine the server uses.

## Evidence

- `cargo test` covers the token check, health, deterministic vectors, BM25 order, redaction offsets, private-target
  refusal and empty bodies.
- Live: `/search` returned 8 hits from Startpage, DuckDuckGo and Bing; `/read` returned the Tokio select page as
  Markdown; a PDF came back as 24 words; `/embed` and `/rerank` answered; `/read` refused `http://127.0.0.1`.
- The client ran through the server with `search.kind: "api"` and `search.rerank.enabled: true` and printed 8 hits
  with engine `api` and score `1.00`.

## Not built

- Model endpoints for the eight families (`/chat/completions`, `/route`, `/check`, `/redact` with a model). The
  server is deterministic on purpose; a model server is a separate deployment.
- TLS, OAuth, multi-user auth, quotas and an audit log. A reverse proxy and one bearer token are the current answer.
- An update server, plugin registry and auth broker. Their contracts stay in [`SERVER-APIS.md`](SERVER-APIS.md).
- A semantic embedding model. The hash vectors are a working local floor, not a replacement for one.
