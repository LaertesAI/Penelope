# Tool description scan

Penelope reads the name, description and input schema of every MCP tool before the model sees it. The rules are the
prose rules of `vet.rs` for `SKILL.md`, plus rules for tool poisoning. A blocked list never reaches the model, and
no tool of that server can be called.

## Why

Paper 44 left this open: the install check read `SKILL.md` and `.mcp.json`, but a server's tool descriptions arrive
only in its `tools/list` answer, after the install. Published numbers on that path, 2025 to 2026:

| Source | What was measured | Number |
| --- | --- | --- |
| MCPTox, arXiv 2508.14925 (AAAI 2026) | 1,312 poisoned cases on 353 tools of 45 real servers, 20 agents | attack success up to 72.8 % (o1-mini); refusals under 3 % |
| MCPTox, per paradigm | explicit hijack, implicit hijack, parameter tampering | 36.7 %, 26.7 %, 46.7 % mean success |
| Cross-framework study, arXiv 2611.03555 | 9,720 cells: 12 frameworks, 18 models, 45 scenarios | 22.9 % overall; per model 0.4 % to 49.7 % |
| same study, regex description scanner | 45 attack scenarios, 125 benign descriptions | 20 of 45 rejected (44.4 %), 7.2 % false positives |
| same study, duplicate tool names rejected (Pydantic AI) | | success falls to 16.6 % |
| Malicious MCP servers, arXiv 2604.01905 | 114 servers, description to built-in tool (path P1) | 0 % (Claude Desktop) to 66.7 % (Cursor with Sonnet 4.0) |
| same paper, description to tool code (P3) | | 66.7 % to 100 % |
| Invariant Labs, April 2025 | tool poisoning, shadowing of another server's tool, rug pull | demonstration, no rate |
| Trail of Bits, April 2025 | ANSI escape codes and line jumping in descriptions | demonstration; Claude Code 0.2.76 did not filter |
| CyberArk, May 2025 | full-schema poisoning: instructions in parameter names, `required`, extra fields | partial success |
| Paper 44 source [6] | 8 MCP scanners on known CVEs | mean recall 24.17 % |

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
- H5. The scan costs under 1 ms per tool.

## Rules

Blocking, each with tool, field and line in the refusal:

- invisible Unicode (tags, supplementary variation selectors, bidi controls, two zero-width characters in a line);
- ANSI escape sequences and other control characters (Trail of Bits);
- an override phrase aimed at the model ("ignore previous instructions", "do not tell the user", "this is very
  important", "otherwise the tool will not work"). In a `SKILL.md` such a phrase only warns; a tool description has
  no reason to address the model about the user;
- a tag that frames hidden instructions (`<IMPORTANT>`, `<SYSTEM>`, `<instructions>`) when its text gives orders;
- a secret path (`~/.ssh/`, `.env`, `mcp.json`, ...) together with a verb that reads or passes it on in one sentence;
- the conversation, the system prompt or earlier messages passed into a parameter (Trail of Bits);
- a precondition that orders a privileged step: "before using this tool, you must first ..." with a file read, a
  shell command, a send or an address (MCPTox paradigms 1 and 2);
- a rule for another tool: a name with an underscore that the server does not own, together with "must", "always",
  "instead", "whenever" or "every" (shadowing, MCPTox paradigm 3);
- a literal address outside the example domains together with a send verb and "all", "also", "always" or "instead";
- an order to avoid every other tool ("never use any other tool");
- every prose rule of `vet.rs`: a download piped into a shell, a password-locked archive, a raw IP address, a secret
  sent over the network.

Warnings: a single zero-width character, a reference to another server's tool without an order, an override phrase
the rules above do not cover, padding of 200 spaces or 20 blank lines, a base64 block of 200 characters.

## When it runs

- On the first `tools/list` of a server in a session, inside `probe`, `describe` and `call`. A blocked list stops all
  three; tool search skips the server.
- When the list changes. The list hash (SHA-256 over the specs sorted by name, the same as the Mitos `listHash`) is
  kept in `<root>/mcp/toolscan.json`. A changed hash scans again. A changed tool that gains a warning after the
  person approved the server holds the server until `penelope mcp allow NAME` again (rug pull).
- On `penelope mcp allow NAME`, `penelope mcp add ... --allow` and `/mcp allow NAME`: the server is started once and
  its list scanned; a blocked list sets the server back to denied.
- On `/tools install N`: the registry name and description of the candidate are scanned before anything waits for
  a yes. A repository install scans every string of each server object in `.mcp.json`.
- `penelope mcp scan NAME` and `/mcp scan NAME` print the findings and warnings.

## Results (2026-11-27)

| Set | Size | Blocked | Warned | Hypothesis |
| --- | --- | --- | --- | --- |
| Written attack set (`cli/src/testdata/toolscan_attacks.json`) | 44 | 44 (100 %) | 0 | H1 holds |
| Published payloads from MCPTox and MCP-ITP, before the last three rules | 10 | 7 (70 %) | 0 | independent check |
| same, after the rules for camelCase names, "locked until" and argument tampering | 10 | 10 (100 %) | 0 | tuned on this set |
| Paraphrases without trigger words (`toolscan_paraphrases.json`) | 12 | 1 (8.3 %) | 0 | H3 holds: the limit of static rules |
| Offline tool lists, 15 servers, first run | 442 | 8 (1.8 %) | 11 | H2 failed |
| same, after word boundaries and the adjacency rule | 442 | 1 (0.23 %) | 9 (2.0 %) | H2 holds, tuned on this set |
| Rug pull: changed description after approval | 3 tests, 1 end-to-end run | held every time | | H4 holds |
| Cost, release build | 442 tools | 110 ms, 249 us per tool | | H5 holds |

The written set reaches 100 % because the rules were written next to it. The published payloads are the honest
number: 7 of 10 before tuning. The gap of 30 points is the overfit of rules written beside their own test set.

The offline lists: Penelope's own servers (vet 2 tools, Kerkis 4, the 60 built-in tools in a unit test), ScreenCodec
13, Roblox Studio 6, Playwright 25, Context7 2, Dart 14, lean-memory 2, ElevenLabs 27, and the cached lists of 6
remote servers in the Claude desktop app (347 tools). The one block is a real vendor tool: its description tells the
model not to mention the call to the user and to call it silently. That is the concealment pattern of tool
poisoning in a harmless tool. `penelope mcp trust NAME` accepts such a list.

For comparison, the regex scanner in arXiv 2611.03555 rejects 44.4 % of 45 scenarios at 7.2 % false positives.

## Next path for paraphrases

Static rules see words, so a paraphrase with no trigger word passes (11 of 12). The next step is a source and sink
rule: a description that names a private source (home folder, configuration, environment file, the dialogue, the
operator's guidance) and a sink (a parameter, a mailbox, an address) in one sentence. Above that, a local judge reads
only lists that changed or carry a warning, so it runs on about 2 % of tools. The number that shows it works: at least
9 of 12 paraphrases blocked while the 442 offline tools keep at most 1 % blocked.

## Commands

```
TMPDIR=... cargo test toolscan
PENELOPE_TOOLSCAN_CORPUS=DIR cargo test toolscan_corpus -- --ignored --nocapture
PENELOPE_TOOLSCAN_EXTERNAL=FILE cargo test toolscan_external -- --ignored --nocapture
```

`DIR` holds one JSON file per server, each a `tools/list` array. `FILE` holds published payloads with the tool names
of the other servers. The third-party lists and the published texts stay on the machine; only the counts are here.
