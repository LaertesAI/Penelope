# Mitos, version 0

Mitos is an extension of MCP for clusters of small models. Every Mitos server stays a valid MCP server, and every MCP client can use it without knowing Mitos. All Mitos fields travel where MCP leaves room: `capabilities.experimental` and `_meta`.

## Handshake

Both sides put the same manifest into `initialize`:

```json
{"capabilities": {"experimental": {"laertes/mitos": {"version": 0, "features": ["index", "describe", "listHash", "origin"]}}}}
```

A server answers with the index only when the client sent the manifest.

## Index

For a Mitos client, `tools/list` returns one entry per tool: the name, the first sentence of the description (at most 120 characters) and `{"type": "object"}` as the schema. The list carries `_meta["laertes/mitos"] = {"lazy": true, "listHash": "..."}`.

Measured on Penelope's own specs (`mitos::tests::the_index_saves_most_bytes_of_a_long_list`):

| Tools | Full list | Index | Saved |
|---|---|---|---|
| 8 | 3,069 bytes | 1,269 bytes | 58.7 % |
| 40 | 16,218 bytes | 5,793 bytes | 64.3 % |
| 128 | 57,581 bytes | 18,534 bytes | 67.8 % |

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

## Not in version 0

- Attachments stay in the brief extension (`laertes/brief`), which Mitos does not change.
- Binding a retry to the skill version (Kerkis rule R4) and the status line `KRK:1` stay in Kerkis.
- No other client than Penelope speaks Mitos yet.
