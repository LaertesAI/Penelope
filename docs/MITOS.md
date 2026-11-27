# Mitos, versions 0 and 1

Version 0 shrinks the tool list. Version 1 (section "Version 1: attachments") shrinks tool results.

Mitos is an extension of MCP for clusters of small models. Every Mitos server stays a valid MCP server, and every MCP client can use it without knowing Mitos. All Mitos fields travel where MCP leaves room: `capabilities.experimental` and `_meta`.

## Handshake

Both sides put the same manifest into `initialize`:

```json
{"capabilities": {"experimental": {"laertes/mitos": {"version": 1, "features": ["index", "describe", "listHash", "origin", "attachments"]}}}}
```

A server answers with the index only when the client sent the manifest. A peer that announces version 0, or no `attachments` feature, gets version 0 behaviour for results: they stay inline. The memory and vet servers of Penelope announce version 0.

## Index

For a Mitos client, `tools/list` returns one entry per tool: the name, the first sentence of the description (at most 120 characters) and `{"type": "object"}` as the schema. The list carries `_meta["laertes/mitos"] = {"lazy": true, "listHash": "..."}`.

Measured on Penelope's own specs (`mitos::tests::the_index_saves_most_bytes_of_a_long_list`):

| Tools | Full list | Index | Saved |
|---|---|---|---|
| 8 | 3,069 bytes | 1,269 bytes | 58.7 % |
| 40 | 16,810 bytes | 5,764 bytes | 65.7 % |
| 128 | 58,134 bytes | 18,393 bytes | 68.4 % |

## Describe

`mitos/describe` with `{"names": ["read"]}` returns the full specs of the named tools and lists unknown names under `unknown`. The Penelope client asks only when it needs the arguments, and caches the answer for the life of the server process. The model reaches it through `mcp_servers` with `server` and `tool`.

## List hash

`_meta["laertes/mitos"].listHash` is SHA-256 over the full specs, sorted by name, as compact JSON. It is the same for a Mitos client and a plain client. A changed tool description changes the hash, so a client sees a silent swap of a description.

## Origin level

Every `tools/call` result carries `_meta["laertes/mitos"].l`, the origin level of its text, as in Kerkis:

| Level | Text |
|---|---|
| 0 | from outside: web pages, command output, other MCP servers, the screen |
| 1 | project files |
| 3 | notes the person asked to keep |

A gate can refuse to pass level 0 text to an outside sink, such as a mail or a push.

## Version 1: attachments

Tool results were about 77 % of the input tokens in the lab's cluster run (about 837,000 of 1.08 million, with the history that carries them from round to round). A result that stays in the history is paid for in every later round. Version 1 stores a long result once and puts one short line into the history. The model reads a range when it needs one.

### Capability

`capabilities.experimental["laertes/mitos"]` carries `version: 1` and the feature `attachments`. Both sides send it. A side that sees no such announcement from its peer behaves as in version 0: the server returns text, the client never sends `mitos/fetch`. A `resource_link` with Mitos meta from a server that did not announce `attachments` is shown as a plain link.

### Server side

A `tools/call` result goes out as an MCP `resource_link` content item, instead of its text, when the text is longer than `mitos.attach_over_bytes` (default 4,096; 0 switches attachments off) or the content is binary (image, PDF). Shorter results stay inline as in version 0. The server stores the content and answers:

```json
{"type": "resource_link", "uri": "mitos://attachment/<sha256>", "name": "run-3fa9c1d2", "mimeType": "text/plain",
 "_meta": {"laertes/mitos": {"sha256": "...", "bytes": 18300, "mime": "text/plain", "lines": 412,
   "summary": "first lines | structure hint", "l": 0, "links": ["https://...", "src/main.rs"]}}}
```

- `summary`: at most 200 characters. For text, the first three non-empty lines (70 characters each) and a hint: `412 lines, json object, 18 keys`, `diff, 3 files`, `log, 7 error lines, 2 warnings`, `source, 14 definitions` or `text`. For an image, the type, the size in pixels (PNG) and the bytes. No model writes it in this version.
- `l`: the origin level of the result, as in version 0. The result's own `_meta["laertes/mitos"].l` stays.
- `links`: URLs and file paths found in the text, in order of first occurrence, without repeats, at most 50.
- `lines` appears for text only.
- A level 3 result carries a fixed summary and no links, so hash, size and type are the only facts that leave the store without a fetch.
- `isError` is kept. A refusal (`NOT ...`) is short and stays inline.

### Fetch

`mitos/fetch` takes `uri` and optionally `lines: [first, last]` (counted from 1, both included) or `bytes: [start, end)` (counted from 0), not both. The answer is a normal content list plus `_meta["laertes/mitos"]` with `sha256`, `bytes` (of the whole attachment), `range`, `returned`, `truncated` and, when cut, `next` with the range to ask for next.

- Hard cap per call: `mitos.fetch_cap_bytes`, default 16,384. A range over the cap is cut at the cap. A line range is cut after the last whole line that fits; a single line over the cap is cut at a character boundary and `next` is a byte range. A call without a range returns the first cap bytes.
- The hash of the stored bytes is checked on every fetch. A mismatch is refused.
- A uri the server did not issue is refused with a JSON-RPC error (-32602): a wrong prefix, a hash that is not 64 lowercase hex characters, a path, or a file in the store folder that no store call wrote (it has no meta entry).
- Binary content comes back as a base64 `resource` blob for a byte range. A line range of binary content is refused.
- A server that did not get the `attachments` announcement answers `mitos/fetch` with -32601, as for any unknown method.

### Storage

`<project>/.penelope/attachments/<first 2 hex of the sha256>/<sha256>`, with `<sha256>.json` next to it (type, size, summary, links, level, name, created). Content-addressed: the same bytes are stored once, the first name stays, and the level never goes down. Writes go through a temporary file and a rename.

When the store is over `mitos.store_max_bytes` (default 500 MB), the oldest entries are removed first; the entry just written stays. Cleanup counts go to `attachments/cleanup.log` in the project.

Level 3 (the person): the attachment is written below the project root only, with mode 0600 on Unix. A cleanup report that leaves the machine (`CleanupReport::export`) lists neither the entries nor their bytes. The local log keeps a count.

### Client side

When a plugin server's result holds a `resource_link` with Mitos meta, and that server announced `attachments`, Penelope's tool loop shows one line per attachment:

```
[attachment run-3fa9c1d2 · text/plain · 18300 bytes · 412 lines · 7 links · l0] summary → attachment_read uri=mitos://attachment/<sha256> server=docs
```

Name, type, size, summary and the number of links are in it, plus the call that reads it. The tool `attachment_read` takes `uri`, `lines` or `bytes`, and `server` when the line names one. A uri that a plugin server issued goes to that server through `mitos/fetch`; every other uri is read from the local store with the same checks. The model sees the answer with one trailer line (`[N of M bytes]`, and `next` when it was cut).

Penelope's own tools use the same path without any server: `run`, `run_detail`, `read`, `grep`, `find`, `tree`, `outline`, `git_diff`, `json_query`, `http_request`, `job_read` and `lsp` return the compact line when their result is over the threshold. These stay inline: results under the threshold, refusals (`NOT ...`), the test-loop stop line, and reports the model has to read whole (`scout`, `scouts`, `research`, `fetch`, the cluster tools, the memory tools).

Settings (all under `mitos` in `config.json`): `attach_over_bytes` 4096, `fetch_cap_bytes` 16384, `store_max_bytes` 524288000.

### Estimate on kept logs

Method: `scripts/mitos_attach_estimate.py` reads the kept tool logs `tools-*.jsonl` of the K9 run (read only), attaches every result of a tool above that list that is over the threshold, and counts bytes and tokens (bytes / 4) inline against one compact line per attachment (summary and uri included).

Caveat for the numbers: the log keeps only the first 4,000 characters of a result, so no logged result is over 4,096 bytes and the default threshold finds nothing in the log as it is. The table therefore shows the default threshold with the 24 results that hit the log cap counted as over (their real size is larger, so their saving is a lower bound), and lower thresholds as sensitivity. The logs held 45 files, 438 calls and 418,496 result bytes (104,624 tokens) when measured; the run was still writing.

| Threshold | Counted as attached | Inline tokens | After tokens | Saved |
|---|---|---|---|---|
| 4,096 bytes, log as is | 0 calls | 104,624 | 104,624 | 0 % |
| 4,096 bytes, log-cap hits counted | 24 calls (read 19, grep 3, outline 2) | 104,624 | 82,736 | 21,887 tokens, 20.9 % |
| 2,048 bytes | 73 calls (read 56, grep 15, outline 2) | 104,624 | 53,127 | 51,496 tokens, 49.2 % |
| 1,024 bytes | 144 calls (read 97, grep 21, run 18, git_diff 4, outline 3, run_detail 1) | 104,624 | 34,613 | 70,010 tokens, 66.9 % |

Per tool at the default threshold with log-cap hits counted: read 63,057 to 45,700 tokens, grep 18,687 to 15,974, outline 2,344 to 528; run, edit, git_diff and the rest do not change.

What the numbers do not say: the real saving depends on how often the model fetches the content. A result the model fetches in full costs the line plus the content, so more than inline. The log holds each result once; the history re-sends it every round, so a result that is not fetched saves its size times the rounds left. A model that reads the content after every `read` saves nothing and pays one more call. The effect has to be measured in a run with `mitos.attach_over_bytes` at 4,096 against 0.

## Not in versions 0 and 1

- The attachments of a brief stay in the brief extension (`laertes/brief`), which Mitos does not change. Version 1 attachments are tool results.
- Summaries written by a model are not part of version 1.
- The ScreenCodec server announces no `attachments` feature yet, so Penelope reads its results inline.
- Binding a retry to the skill version (Kerkis rule R4) and the status line `KRK:1` stay in Kerkis.
- No other client than Penelope speaks Mitos yet.
