#!/usr/bin/env python3
"""Eurykleia on the real tool logs of the thinking run. No model, no network.

Uses tools-<part>.jsonl from the new run, which holds the real arguments and the result flag.
The check mirrors cli/src/eurykleia.rs at a8febf1. The tool schemas come from `penelope mcp`
with PENELOPE_ROLE=worker.
"""
import json
import os
import re
import subprocess
import sys
from collections import Counter
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from cluster_messung_schritt1 import eurykleia  # noqa: E402

ROOT = Path("/tmp/opencode/evalroot-comm")
RUN = "cluster-1795778629608"
BINARY = "/tmp/opencode/target-comm/release/penelope"


def tool_schemas(binary, root):
    proc = subprocess.Popen([binary, "--root", root, "mcp"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                            stderr=subprocess.DEVNULL, text=True, bufsize=1,
                            env={**os.environ, "PENELOPE_ROLE": "worker"})
    def rpc(obj):
        proc.stdin.write(json.dumps(obj) + "\n")
        proc.stdin.flush()
        while True:
            line = proc.stdout.readline()
            if not line:
                raise RuntimeError("mcp closed")
            try:
                msg = json.loads(line)
            except json.JSONDecodeError:
                continue
            if msg.get("id") == obj.get("id"):
                return msg
    try:
        rpc({"jsonrpc": "2.0", "id": 1, "method": "initialize",
             "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "1b", "version": "0"}}})
        proc.stdin.write(json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized"}) + "\n")
        proc.stdin.flush()
        tools = rpc({"jsonrpc": "2.0", "id": 2, "method": "tools/list"})["result"]["tools"]
        return {t["name"]: t.get("inputSchema") or {"type": "object"} for t in tools}, tools
    finally:
        proc.terminate()


def main():
    schemas, tools = tool_schemas(BINARY, str(ROOT))
    run = json.loads((ROOT / "eval" / "runs" / (RUN + ".json")).read_text())
    rows = []
    for r in run["results"]:
        if "scikit" in r["id"]:
            continue
        m = re.search(r"eval-small-(\d+)", r.get("answer", ""))
        cid = "eval-small-" + m.group(1) if m else ""
        calls = []
        for f in (ROOT / "clusters" / cid).glob("tools-*.jsonl"):
            for line in f.read_text().splitlines():
                if not line.strip():
                    continue
                try:
                    v = json.loads(line)
                except json.JSONDecodeError:
                    continue
                calls.append(v)
        flags = []
        for c in calls:
            f = eurykleia(schemas.get(c.get("name")), c.get("args"))
            if f:
                flags.append((c.get("name"), f[0], f[1]))
        bad = [c for c in calls if not c.get("ok")]
        good = [c for c in calls if c.get("ok")]
        flagged_bad = sum(1 for c in calls if not c.get("ok") and eurykleia(schemas.get(c.get("name")), c.get("args")))
        flagged_good = sum(1 for c in calls if c.get("ok") and eurykleia(schemas.get(c.get("name")), c.get("args")))
        rows.append({"id": r["id"], "passed": r["passed"], "calls": len(calls), "bad": len(bad), "good": len(good),
                     "flags": len(flags), "flagged_bad": flagged_bad, "flagged_good": flagged_good,
                     "codes": Counter(x[1] for x in flags)})
    calls = sum(r["calls"] for r in rows)
    bad = sum(r["bad"] for r in rows)
    good = sum(r["good"] for r in rows)
    fb = sum(r["flagged_bad"] for r in rows)
    fg = sum(r["flagged_good"] for r in rows)
    failed = [r for r in rows if not r["passed"]]
    passed = [r for r in rows if r["passed"]]
    task_flags_failed = sum(1 for r in failed if r["flags"])
    task_flags_passed = sum(1 for r in passed if r["flags"])
    print("tools", len(tools), "tasks", len(rows), "calls", calls, "bad", bad, "good", good)
    print("flagged_bad", fb, "flagged_good", fg, "call TPR", round(fb / bad, 4) if bad else None,
          "call FPR", round(fg / good, 4) if good else None)
    print("tasks failed", len(failed), "passed", len(passed), "task flags failed", task_flags_failed,
          "task flags passed", task_flags_passed)
    print("task TPR", round(task_flags_failed / len(failed), 4) if failed else None,
          "task FPR", round(task_flags_passed / len(passed), 4) if passed else None)
    print("codes", sum((r["codes"] for r in rows), Counter()).most_common())
    for r in rows:
        if r["bad"] or r["flags"]:
            print(r["id"], "calls", r["calls"], "bad", r["bad"], "flags", r["flags"], "fb", r["flagged_bad"], "fg", r["flagged_good"], dict(r["codes"]))


if __name__ == "__main__":
    main()
