# MCP servers in Penelope

`penelope mcp` is the default server with all of Penelope's tools. `penelope mcp serve NAME` starts a smaller server with one purpose. Both extra servers speak Mitos version 0 (see `MITOS.md`) and stay valid MCP servers for a client that does not know Mitos.

| Command | Tools | Writes |
|---|---|---|
| `penelope mcp serve kerkis [--allow-person]` | `memory_search`, `memory_get`, `memory_add`, `memory_status` | `memory.md` and `memory.log` of a cluster, only through `memory_add` |
| `penelope mcp serve vet [--scan-root DIR]` | `vet_scan_path`, `vet_scan_text` | nothing |

The global option `--root DIR` still names the `.penelope` folder, for both servers. An unknown profile or option exits with code 2 before the server starts, so a typo such as `--allow-persons` cannot leave a guard off.

Errors follow JSON-RPC. A line that is not JSON gets -32700. A request without a method gets -32600. An unknown method gets -32601. Bad arguments or an unknown tool get -32602. A valid call that the server refuses returns a result with `isError: true` and a text that starts with `NOT`. The server keeps running after every one of these.

## Kerkis memory server

It reads the `memory.log` files that clusters write under `.penelope/clusters/ID/` and appends new entries to them.

A log line is one JSON object: `t` (seconds), `C` (cluster id), `L` (origin level) and `text`. This is the format `clusters.rs` writes. An entry from `memory_add` stores four lines in `text`: `ENT`, `ATT`, `VAL` and `SRC`. An entry written by a cluster has no such lines, so its `ENT`, `ATT` and `VAL` are `null` in the answer. `memory_add` writes `memory.md` and `memory.log` the same way `clusters::mem_put` does. That function needs a full cluster record and takes no file lock, so the server repeats its two appends under a lock file (`memory.lock`, created atomically, removed after 30 seconds if its owner died). The id is the line number counted under the lock, so two writers never share one. A torn last line is closed with a newline before the new entry. Values lose control characters, so a value cannot add a field or a log line.

Origin levels: 0 outside data, 1 agent, 2 checked tool, 3 person. The caller states the level of an entry, so the server does not trust it: `memory_add` caps it at 1, writes `SRC: mcp`, and keeps the caller's source in `REF`. Only a person's entry reaches 3, and only with `--allow-person`. No entry written through MCP is level 2. `memory_status` reports level 1, because an agent wrote the line. Every result carries `_meta["laertes/mitos"].l`. For `memory_search` it is the lowest level among the returned entries, so a gate that stops level 0 text sees it. Each entry also carries its own `_meta`. 

An entry id is `CLUSTER:LINE`, for example `kerkis-memory:3`.

### Tools

| Tool | Arguments | Answer |
|---|---|---|
| `memory_search` | `query` (all words must match, case-insensitive), optional `l_max` (0 to 3, keep entries with L at most this), `cluster`, `limit` (1 to 100, default 20) | `total`, `returned`, `entries`, newest first |
| `memory_get` | `id` | one entry |
| `memory_add` | `ENT`, `ATT`, `VAL`, `level` (0 to 3), `source`, optional `cluster` (default `kerkis-memory`) | the new entry with its id |
| `memory_status` | optional `cluster` | the newest `KRK:1` line from `report.krk`, parsed and rendered again, or `no KRK:1 status line yet` |

`memory_add` with `level` 2 is stored as 1. `memory_add` with `level` 3 marks the claim as written by a person. It is refused with `NOT added` unless the server was started with `--allow-person`. A cluster id with a leading dot is refused with -32602, so an id cannot leave the `clusters` folder.

### Claude Code

```
claude mcp add kerkis -- penelope mcp serve kerkis
claude mcp add kerkis-person -- penelope mcp serve kerkis --allow-person
```

Run the command in the project folder, or add `--root /path/to/project/.penelope` after `penelope`.

### OpenCode

```json
{
  "mcp": {
    "kerkis": {
      "type": "local",
      "command": ["penelope", "mcp", "serve", "kerkis"],
      "enabled": true
    }
  }
}
```

### Mitos byte savings

Compact JSON of the `tools/list` result, same method as `MITOS.md` (`mcpmem::tests::the_index_is_smaller_than_the_full_list`).

| Tools | Full list | Index | Saved |
|---|---|---|---|
| 4 | 2,042 bytes | 728 bytes | 64.3 % |

## vet server

It runs the prose rules of `vet.rs` over `SKILL.md`, `skills/*.md`, `commands/*.md` and `agents/*.md`, and the invisible-Unicode rule over small text files. The rules are the ones `penelope plugin` applies before it registers a plugin.

The server is read-only. It reads files and compares text. It never installs a plugin, runs a script or follows a link, and it opens no network connection.

The folder the server starts in is the workspace only when it is a project: it, or a parent below the home folder, holds `.git` or `.penelope`. Started in the home folder, in `/` or in a plain folder, the server refuses to start unless `--scan-root` names a folder; then only the named folders are allowed. A path must lie inside the workspace or inside a folder given with `--scan-root DIR` (repeatable). The server resolves the path first, so `..` and a symlink that points out of the allowed folders are refused with `NOT scanned`. A `--scan-root` that does not exist stops the start. The option is called `--scan-root` because `--root` already names the `.penelope` folder.

The findings quote lines of the scanned text, so every result has origin level 0.

### Tools

| Tool | Arguments | Answer |
|---|---|---|
| `vet_scan_path` | `path`: a folder or a file | `blocked`, `blocking` (at most one, the first match), `warnings` (first 100), `warnings_total`, `prose_files`; each finding has `rule`, `file`, `line` and `text` (at most 120 characters of the line) |
| `vet_scan_text` | `text` (at most 512 KiB), optional `name` (a label, default `SKILL.md`) | the same fields |

A file that is binary or larger than 512 KiB is reported as a warning, not skipped silently.

### Claude Code

```
claude mcp add vet -- penelope mcp serve vet
claude mcp add vet-all -- penelope mcp serve vet --scan-root ~/skills-to-check
```

### OpenCode

```json
{
  "mcp": {
    "vet": {
      "type": "local",
      "command": ["penelope", "mcp", "serve", "vet", "--scan-root", "/home/me/skills-to-check"],
      "enabled": true
    }
  }
}
```

### Mitos byte savings

Same method (`mcpvet::tests::initialize_with_and_without_mitos_and_the_index_is_smaller`):

| Tools | Full list | Index | Saved |
|---|---|---|---|
| 2 | 867 bytes | 502 bytes | 42.1 % |

The saving grows with the number of tools, as the table in `MITOS.md` shows for 8, 40 and 128 tools. With 2 tools the fixed `_meta` block takes a large share of the index.

## Tool server with `--root`

`penelope mcp --root DIR` serves Penelope's own tools (read, find, grep, tree, outline, lsp, json_query, git_diff, write, edit, unzip). With `--root`, a call whose `path`, `dir`, `cwd`, `archive` or `into` lies outside the project folder (the parent of `DIR` when it is named `.penelope`) is refused with `NOT called`. The check resolves `..` and symlinks first. Allowed besides the project: the folders in `brief.roots`, `plugin_dirs` and `mcp.roots` of `config.json`. `"mcp": {"allow_outside_root": true}` switches the check off for a person who wants a client to read anywhere. Without `--root` nothing changes. The server has no interactive main session, so there is no per-role exception.

A line that is not UTF-8, not JSON or not an object gets -32700 or -32600, and the server keeps running.
