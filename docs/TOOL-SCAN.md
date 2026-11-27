# Tool description scan

Penelope reads the name, description and input schema of every MCP tool before the model sees it. The rules are the
prose rules of `vet.rs` for `SKILL.md`, plus rules for tool poisoning. In Penelope's own MCP client (the HTTP
backends, `mcp_servers`, `mcp_call`, tool search) every list read passes one gate, and a blocked list stops every
tool of that server. On the claude command backend, the claude command reads the lists itself; Penelope keeps
blocked, held, unvetted and unreadable servers out of its `--mcp-config`, with the 24 hour gap named under "When it
runs".

## Why

Paper 44 left this open: the install check read `SKILL.md` and `.mcp.json`, but a server's tool descriptions arrive
only in its `tools/list` answer, after the install. Published numbers on that path, 2025 to 2026:

| Source | What was measured | Number |
| --- | --- | --- |
| MCPTox, arXiv 2508.14925 (AAAI 2026) | 1,348 poisoned cases on 353 tools of 45 real servers, 20 agents | attack success up to 72.8 % (o1-mini); refusals under 3 % |
| MCPTox, per paradigm (the paper's sec. 4.4) | explicit hijack, implicit hijack, parameter tampering | 36.7 %, 26.7 %, 46.7 % mean success |
| Cross-framework study, arXiv 2611.03555 | 9,720 cells: 12 frameworks, 18 models, 45 scenarios | 22.9 % overall; per model 0.4 % to 49.7 % |
| same study, regex description scanner | 45 attack scenarios, 125 benign descriptions | 20 of 45 rejected (44.4 %), 7.2 % false positives |
| same study, Pydantic AI | framework-level attack success | 16.6 % against the 22.9 % mean; the paper attributes the gap to duplicate-name rejection and reports no before and after measurement |
| Malicious MCP servers, arXiv 2604.01905 | 114 servers, description to built-in tool (path P1) | 0 % (Claude Desktop) to 66.7 % (Cursor with Sonnet 4.0) |
| same paper, description to tool code (P3) | | 66.7 % to 100 % |
| MCP-ITP, arXiv 2601.07395 | implicit tool poisoning against detectors | detector miss rate down to 0.3 %; its LLM oracle detector misses 22.0 % to 31.4 % |
| Invariant Labs, April 2025 | tool poisoning, shadowing of another server's tool, rug pull | demonstration, no rate |
| Trail of Bits, April 2025 | ANSI escape codes and line jumping in descriptions | demonstration; Claude Code 0.2.76 did not filter |
| CyberArk, May 2025 | full-schema poisoning: instructions in parameter names, `required`, extra fields | partial success |
| Paper 44 source [6] | 8 MCP scanners on known CVEs | mean recall 24.17 % (not checked again here) |

The attack works on first contact, before any call: the description enters the context when the list is read.
A rug pull swaps the description after the person approved the server, so the scan has to run again on every change.

## Hypotheses, written before the measurement

- H1. The rules block at least 80 % of a written attack set of 36 or more cases (at least 29 of 36). Block or warn
  reaches at least 95 %. The set grew to 44 cases during the work.
- H2. On the tool lists the machine can read offline (about 450 tools), at most 1 % of tools are blocked and at most
  5 % warned.
- H3. A paraphrase set with no trigger words (no "ignore", no tags, no secret paths named literally) is blocked at
  50 % or less. This is the limit of static rules; the next path is named in the result.
- H4. Every changed description after approval is seen (100 %), because the list hash covers every byte of every spec.
  Restated after review: every changed list that Penelope reads is scanned and recorded, and the person sees it
  (see the hold below).
- H5. The scan costs under 1 ms per tool.

## Rules

Blocking, each with tool, field and line in the refusal:

- invisible Unicode (tags, supplementary variation selectors, bidi controls, two zero-width characters in a line);
- ANSI escape sequences and other control characters (Trail of Bits);
- an override phrase aimed at the model ("ignore previous instructions", "this is very important", "otherwise the
  tool will not work"). In a `SKILL.md` such a phrase only warns;
- a phrase that only keeps a call from the person ("do not mention", "do not tell", "hide this"), when the same text
  also names a private source, a sink (a send verb, an address, a URL), an exec word or another server's tool. Alone
  it warns: a harmless vendor tool tells the model "do not mention or narrate this call to the user";
- a tag that frames hidden instructions (`<IMPORTANT>`, `<SYSTEM>`, `<instructions>`) when its text gives orders;
- a secret path (`~/.ssh/`, `.env`, `mcp.json`, ...) together with a verb that reads or passes it on in one sentence;
- the conversation, the system prompt or earlier messages passed into a parameter (Trail of Bits). "When your system
  prompt indicates ..." is a condition and passes;
- a precondition that orders a privileged step: "before using this tool, you must first ..." with a file read, a
  shell command, a send or an address (MCPTox paradigms 1 and 2);
- a rule for another tool: a name with an underscore that the server does not own, together with "must", "always",
  "whenever", "every" or a gating phrase such as "locked until" (shadowing, MCPTox paradigm 3, MCP-ITP);
- a literal address outside the example domains together with a send verb and "all", "also", "always" or "instead";
- an order to avoid every other tool ("never use any other tool");
- text behind padding: a run of 200 blanks or a run of blanks with 8 line breaks, followed by an order, a secret or a
  send verb. Every Unicode blank counts: no-break and wide spaces, tabs, the braille blank U+2800, the Hangul
  fillers. The 442 offline tools have at most 15 blanks and 4 line breaks in one run;
- a single tool text over 64 KB. The longest of the 442 offline descriptions has 8,653 characters. Reading only the
  first 64 KB would leave the rest unread;
- every prose rule of `vet.rs`: a download piped into a shell, a password-locked archive, a raw IP address, a secret
  sent over the network.

Warnings: a single zero-width character, a reference to another server's tool without an order, a concealment
phrase alone, padding without an order, a base64 block of 200 characters.

## When it runs

- On every read of a server's tool list in Penelope's client. `Server::tools()` in `mcpclient.rs` is the one gate
  point: `probe`, `describe`, `call` and tool search all read the list through it, cached or new. `call` reads and
  gates the list before the first call of each process, so an allowed server with no verdict yet, a restarted process
  or a changed signature never runs unscanned. Tool search gates each cached list again before listing it.
- On `mitos/describe`: the full spec of a lazy index entry is scanned with the names of the whole list as the
  server's own, before it is cached. A finding is recorded as a block of the server.
- On `notifications/tools/list_changed`: the reader thread marks the list, and the next read asks the server again.
- When the list changes. The verdict is cached under list hash (SHA-256 over the specs sorted by name, the same as
  the Mitos `listHash`) + `RULES_VERSION` + hash of the foreign tool names. A change of any of the three scans again:
  a server scanned before the server it shadows is caught once that server registers, and a Penelope update with
  new rules rescans every list. A blocked server adds no foreign names; `penelope mcp remove NAME` deletes its record.
- Every changed tool of a server with an earlier record holds the server until `penelope mcp trust NAME` accepts the
  exact new list, whether or not a warning fired. `penelope mcp scan NAME` shows the change word by word
  (`[-old-] {+new+}`). `"toolscan": {"changes": "warn"}` in config.json holds only a changed tool that carries a
  warning, the old behaviour.
- On `penelope mcp allow NAME`, `penelope mcp add ... --allow` and `/mcp allow NAME`: the server is started once and
  its list scanned; a blocked list sets the server back to denied.
- On the claude command backend, before each turn: plugin and standalone servers with a blocked or held list, the
  `unvetted` mark from tool search, or a list that cannot be read stay out of `--mcp-config`, so the claude command
  neither reads their descriptions nor runs their package. A server with no verdict, a verdict of older rules or one
  older than 24 hours is started once by Penelope and scanned first. Gap: a list that changes within those 24 hours
  reaches the claude command unscanned until the next refresh. The agent and scout configurations hold only
  Penelope's own server.
- On `/tools install N`: the registry name and description of the candidate are scanned before anything waits for
  a yes. A repository install scans every string of each server object in `.mcp.json`; a finding quotes no
  environment value and nothing after `Bearer`, `Authorization` or `token=`.

The records live in `~/.penelope/mcp/toolscan/<root hash>/`, never in a project folder: a cloned repository with
`.penelope/mcp/toolscan.json` could otherwise ship a record that trusts its own poisoned list. `write` and `edit`
refuse that folder. Writes take a file lock and replace the file by rename, so two Penelope processes keep each
other's records (8 threads, 8 servers, 8 records in the unit test). An unreadable record file means "no verdict" and
scans again; it never means clean. Still open: the model's `write` tool can write other files under a project's
`.penelope/`, such as `mcp.json`, after the person's yes.

## Results (2026-11-27)

Confidence intervals are two-sided 95 % Clopper-Pearson.

| Set | Size | Blocked | Warned | Hypothesis |
| --- | --- | --- | --- | --- |
| Written attack set (`cli/src/testdata/toolscan_attacks.json`) | 44 | 44 (100 %, CI 92.0 to 100 %) | 0 | H1 holds; written beside the rules, so it shows fit, not generalisation |
| Published payloads from MCPTox and MCP-ITP, before the last three rules | 10 | 7 (70 %, CI 34.8 to 93.3 %) | 0 | independent check |
| same, after the rules for camelCase names, "locked until" and argument tampering | 10 | 10 (100 %) | 0 | tuned on this set |
| Paraphrases without trigger words (`toolscan_paraphrases.json`) | 12 | 1 (8.3 %, CI 0.2 to 38.5 %) | 0 | H3 holds; the set was written to dodge the rules |
| Offline tool lists, 15 servers, first run | 442 | 8 (1.8 %) | 11 | H2 failed |
| same, after word boundaries and the adjacency rule | 442 | 1 (0.23 %, CI 0.01 to 1.25 %) | 9 (2.0 %) | H2 holds as a point estimate, tuned on this set |
| same, after the concealment and condition rules of this review | 442 | 0 (CI 0 to 0.83 %) | 10 (2.3 %, CI 1.1 to 4.1 %) | H2 holds, one-sided upper bound 0.68 %, still tuned on this set |
| Rug pull: changed description after a scan | 3 unit tests; 10 of 10 swaps on `list_changed` in an end-to-end test; 12 of 12 paraphrases held as version 2 | held or refused every time | | H4 holds per list read |
| Cost, release build, before this review | 442 tools | 110 ms, 249 us per tool | | H5 holds |
| Cost, debug build, after this review | 442 tools | 795 ms, 1,799 us per tool | | release not run again |

The written set reaches 100 % because the rules were written next to it. The published payloads gave 7 of 10 before
tuning. That gap of 30 points mixes distribution shift with overfit, on n = 10: the two sets differ in source and
difficulty as well as in tuning. 7 of 10 before against 10 of 10 after gives Fisher p = 0.21, and the after number
is measured on the set it was tuned on. That static scanners miss independent payloads is already reported (MCP-ITP,
arXiv 2611.03555); the gap is no new finding.

The rule for MCP-ITP payloads: after tuning, ITP1, 3 and 5 block through the precondition rule and ITP2 and 4
through the shadow rule, on the 5 cases that were also the tuning set. Both need the foreign tool names. The
claim that only foreign names plus gating words can catch them is wrong: MCP-ITP's own LLM oracle detects some
cases without a tool inventory.

The offline lists: Penelope's own servers (vet 2 tools, Kerkis 4, the 57 built-in tools in a unit test), ScreenCodec
13, Roblox Studio 6, Playwright 25, Context7 2, Dart 14, lean-memory 2, ElevenLabs 27, and the cached lists of 6
remote servers in the Claude desktop app (347 tools). The one block of the second run was a vendor tool whose
description tells the model not to mention the call to the user and to call it silently. With the concealment rule
of this review that description warns, and a TOX4-style payload, which pairs concealment with a source and a sink,
still blocks (44 of 44, 10 of 10). Its parameter text "when your system prompt indicates a mobile client" was a
second block hidden behind the first; the condition rule passes it. Both changes were tuned on this corpus.

## Next path

- Paraphrases. Static rules see words, so a paraphrase with no trigger word passes (11 of 12). The hold on every
  change covers the rug pull case: all 12 paraphrases as version 2 of a clean version 1 are held, against 1 of 12 by
  the rules alone. A paraphrase in a server's first list is not covered. The next rule is a source and sink table: a
  sentence that names a private source (home folder, configuration, environment, the dialogue, the system prompt,
  credentials) and a sink (a parameter of this tool's own schema, a mailbox, a URL, a foreign tool). The number that
  shows it works: at least 14 of a fresh set of 18 paraphrases blocked (lower bound above 52 %), written by someone
  who has not read the rules, while the 442 tools keep at most 4 blocked.
- A local judge on CPU (a small Qwen model through llama.cpp, or an OpenCode Go model on public descriptions only).
  It reads only single tools that are warned or changed, 10 of 442 tools (2.3 %). Judging the whole lists that
  carry a warning would cover 297 of 442 tools (67.2 %). Target: rules plus judge block at least 80 % of the fresh
  paraphrases plus a published hold-out, flag at most 3 of the 442 tools, median under 2 s per tool.
- A hold-out of 30 or more published payloads (MCPTox cases not used so far, MCP-ITP, CyberArk, 2604.01905 P1),
  frozen before the next rule change, with only hashes and counts in the repository. 24 of 30 blocked puts the lower
  bound above 61 %.
- H2 with confidence on untuned data: about 650 tools that the rules never saw; pass with 2 or fewer blocked.
- How often the hold fires: count holds per server per week over 30 days of real list changes. Target 2 or fewer.

## Secrets in server configurations

`penelope mcp secrets` lists secret values in Penelope's MCP servers and, read only, in `~/.claude.json`: environment
values under names with KEY, TOKEN, SECRET, PASSWORD or AUTH, headers, `Authorization` and `Bearer` in arguments,
query keys and the value after `--token` or `--api-key`. It prints the length only. On the development machine it
found 2: a bearer header in an argument of a Claude Code project server and an `Authorization` header of a plugin
server. A key inside a URL path is not detected.

## Commands

```
TMPDIR=... cargo test toolscan
PENELOPE_TOOLSCAN_CORPUS=DIR cargo test toolscan_corpus -- --ignored --nocapture
PENELOPE_TOOLSCAN_EXTERNAL=FILE cargo test toolscan_external -- --ignored --nocapture
penelope mcp scan NAME | trust NAME | secrets
```

`DIR` holds one JSON file per server, each a `tools/list` array. `FILE` holds published payloads with the tool names
of the other servers. The third-party lists and the published texts stay on the machine; only the counts are here.
