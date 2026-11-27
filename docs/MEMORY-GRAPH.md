# Memory graph

The memory of Penelope is a tree of waypoints with notes. The graph view turns it into a graph with cross-links, shows it in the terminal, exports it, and gives models a pointer tool. Nothing here calls a model or the network.

## Nodes

`graph.rs` builds the graph from three sources:

- the project memory (`.penelope/memory`): one node per waypoint, its notes are the text;
- the shared memory (`~/.penelope/global-memory`, or `global_memory` in the settings): the same, scope `global`;
- the cluster logs (`.penelope/clusters/ID/memory.log`): one node per entry, the newest 500 lines of each log.

| kind | what it is | origin level |
|---|---|---|
| note | a waypoint of the project or shared memory | 3 |
| rule | a waypoint under `rules/`, or one with a lasting-rule note ("always", "never", ...) | 3 |
| cluster-entry | one line of a `memory.log`, titled by its ENT field | the `L` of the line |
| file | a file path named in some node's text | 1 |
| link | a URL named in some node's text | 0 |

Notes are level 3 because `memory_go` is level 3 in `mitos.rs`: the person wrote them, or said the rule they come from.

Ids: `p:rust/errors` (project), `g:rules/style` (shared), `c:CLUSTER:LINE` (cluster entry, the same CLUSTER:LINE that `memory_get` reads), `f:src/main.rs`, `u:https://...`.

Every node has a title, the scope (project, global, cluster, derived), the level, the use count of its waypoint, the time it was added, the time of its last change and a path (notes file, log line, file path or URL). Added is the creation time of the notes file where the system knows it, else the last change.

## Edges

| kind | from, to |
|---|---|
| child | a waypoint to its child waypoint |
| mention | a node that writes `[[name]]`, the id of another waypoint (`rust/errors`), or the title of exactly one other waypoint (5 letters or more) |
| file | a node to the file node of a path in its text. Two nodes that name the same path are two steps apart through that file node |
| link | a node to the link node of a URL in its text |

`[[name]]` resolves by waypoint id, by the vault form `rust · errors`, or by title. An alias (`[[name|alias]]`) and a heading (`[[name#h]]`) are cut off. A name that matches nothing makes no edge.

A path counts when it has a slash and an extension, or a known extension (`rs`, `md`, `toml`, `json`, `py`, `ts`, ...). `..` paths and URLs are never file nodes.

## Limits

The graph holds at most 3000 waypoints and entries (the waypoints first, then the newest entries), at most 1500 file and link nodes and 12000 edges. When something is cut, `truncated` is true and the view says how many exist. The list in `/graph` is paged by the window; the search runs over the whole graph.

## `/graph`

The overlay lists the nodes. The search runs as you type and matches title, id, kind and text; a title hit comes before a text hit, then the newest change. The header shows nodes, edges, the count per kind and the nodes added in the last 7 days.

| key | list | walk |
|---|---|---|
| type | search | go back to the list and search |
| up, down | move | choose a neighbour |
| enter | open the node | go to the chosen neighbour |
| backspace | delete a letter | one step back; the last step returns to the list |
| esc | clear the search, then close | back to the list |

The right side shows the text, the neighbours (`→` out, `←` in, with the edge kind) and a picture with the node in the middle and up to 8 neighbours around it. The arrows show the direction of each edge.

## Export

```
penelope memory export --format json PATH
penelope memory export --format obsidian PATH
penelope memory export --format json --include-person PATH
```

- `json` writes `{"penelope_graph": 1, "generated", "counts", "truncated", "nodes", "edges"}`. A node has `id, title, kind, scope, level, uses, added, changed, path, text`; an edge has `from, to, kind`. If PATH is a folder the file is `memory-graph.json`; `-` prints to the terminal. An existing file that is not a Penelope graph file is not overwritten.
- `obsidian` writes one Markdown file per node into PATH, with front matter, the text, `## Links` and `## Linked from` with `[[links]]`, and `penelope · graph.md` as the index. Open PATH as a vault and the graph view of Obsidian draws the same graph.

The Obsidian export writes only inside PATH. File names are made from the node and cleaned: every character that is not a letter, digit, space, `-`, `_`, `.` or `·` becomes `-`, so a waypoint called `../../evil` becomes `project · .. · .. · evil.md` inside PATH. A file that already exists in PATH and was not written by Penelope (no `penelope_node` front matter) stays as it is, and so does a symbolic link.

Level 3 nodes are left out unless `--include-person` is given. With the flag omitted the export holds the cluster entries of level 2 and below and the file and link nodes they name. Edges to left-out nodes and file or link nodes that only those nodes named are dropped too. Because project and shared notes are level 3, a default export of a project without cluster logs is empty.

## `memory_find`, the pointer

`memory_find(query, limit)` answers where the memory holds something, without the text of the note:

```
1. p:rust/errors · errors · "Use anyhow in the binary." · memory/notes/rust__errors.md · L3 project · score 0.67 · read: memory_go "rust errors"
```

Each pointer has the id, the title, one line (the line of the node that holds most query words, 120 characters), the path, the origin level and the score. The tool is built in and part of the Kerkis MCP server (`penelope mcp serve kerkis`), so Claude Code and OpenCode can call it. Its description is one sentence, so the Mitos index stays short.

Score: per query word the best place counts, title 3, id or path 2, a word of the text 1, a word of the text with the same first 5 letters 0.5. The sum is divided by 3 times the number of words, so 1.0 means every word is in a title. A node needs at least 0.2. A one-word query that is only in the text scores 0.33, a two-word query with one word in the text scores 0.17 and returns nothing. Ties go to the node with more uses, then the newer one.

If nothing reaches 0.2 the tool answers:

```
unknown: nothing stored about "<query>"; note it after the session with memory_add or /note
```

The Kerkis server says `memory_add` only, since it has no `/note`. It points at project and shared notes (level 3) only when started with `--allow-person`; without the flag it points at cluster entries and the files and links they name. A pointer from the Kerkis server carries `_meta["laertes/mitos"].l`, and the call carries the lowest level among its pointers.

`penelope memory find QUERY` runs the same search in the terminal.

## Not part of this feature

The nightly COAA cassettes for local models are not built here.
