#!/usr/bin/env python3
"""Offline estimate of Mitos v1 attachments on kept tool logs. Reads tools-*.jsonl, writes nothing.

Each log line is {"name", "args", "ok", "result"}; the result is cut at 4000 characters when the log is written
(cli/src/mcp.rs, call_raw). A result that reached the cap is a result of at least 4000 characters of unknown length,
so the estimate has two modes:
  strict   a result counts as attached when its logged size is over the threshold.
  capped   strict, and a result that reached the 4000 character cap counts as attached too. Its size is the logged
           size, so its saving is a lower bound.
Tokens are bytes / 4. A referenced result costs one compact line: the fixed words, a summary of at most 200
characters cut from its first lines, and a 64 character uri. The model pays more when it fetches a range.
"""
import argparse, glob, json, sys, collections

ATTACHABLE = {"run", "run_detail", "read", "grep", "find", "tree", "outline", "git_diff", "json_query",
              "http_request", "job_read", "lsp"}
LOG_CAP_CHARS = 4000


def compact_line_bytes(tool, text):
    """Size of the compact line cli/src/attach.rs would show for this text."""
    raw = text.encode()
    lines = text.count("\n") + (0 if text.endswith("\n") or not text else 1)
    first = [l.strip()[:70] for l in text.splitlines() if l.strip()][:3]
    summary = (" | ".join(first) + f" · {lines} lines, text")[:200]
    line = (f"[attachment {tool}-{'0' * 8} · text/plain · {len(raw)} bytes · {lines} lines · 3 links · l1] "
            f"{summary} → attachment_read uri=mitos://attachment/{'0' * 64}")
    return len(line.encode())


def load(pattern):
    rows, files = [], sorted(glob.glob(pattern))
    for path in files:
        with open(path, encoding="utf-8", errors="replace") as f:
            for raw in f:
                try:
                    d = json.loads(raw)
                except ValueError:
                    continue
                if isinstance(d.get("result"), str) and d.get("name"):
                    rows.append(d)
    return files, rows


def estimate(rows, threshold, capped):
    per = collections.OrderedDict()
    for d in rows:
        tool, text = d["name"], d["result"]
        size = len(text.encode())
        at_cap = len(text) >= LOG_CAP_CHARS
        over = size > threshold or (capped and at_cap and size >= LOG_CAP_CHARS)
        attach = tool in ATTACHABLE and threshold > 0 and over
        row = per.setdefault(tool, {"calls": 0, "inline": 0, "attached": 0, "attached_inline": 0, "referenced": 0, "at_cap": 0})
        row["calls"] += 1
        row["inline"] += size
        row["at_cap"] += 1 if at_cap else 0
        if attach:
            row["attached"] += 1
            row["attached_inline"] += size
            row["referenced"] += compact_line_bytes(tool, text)
    return per


def report(per, label):
    out = [f"\n{label}", f"{'tool':<11}{'calls':>6}{'at cap':>8}{'attached':>9}{'inline B':>10}{'after B':>9}{'inline tok':>11}{'after tok':>10}{'saved tok':>10}"]
    tot = collections.Counter()
    for tool, r in sorted(per.items(), key=lambda kv: -kv[1]["inline"]):
        after = r["inline"] - r["attached_inline"] + r["referenced"]
        out.append(f"{tool:<11}{r['calls']:>6}{r['at_cap']:>8}{r['attached']:>9}{r['inline']:>10}{after:>9}{r['inline'] // 4:>11}{after // 4:>10}{(r['inline'] - after) // 4:>10}")
        tot.update({"calls": r["calls"], "at_cap": r["at_cap"], "attached": r["attached"], "inline": r["inline"], "after": after})
    saved = tot["inline"] - tot["after"]
    pct = 100.0 * saved / tot["inline"] if tot["inline"] else 0.0
    out.append(f"{'total':<11}{tot['calls']:>6}{tot['at_cap']:>8}{tot['attached']:>9}{tot['inline']:>10}{tot['after']:>9}{tot['inline'] // 4:>11}{tot['after'] // 4:>10}{saved // 4:>10}")
    out.append(f"saved {saved} bytes, about {saved // 4} tokens, {pct:.1f} % of the tool result bytes in these logs")
    return "\n".join(out)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--glob", default="/tmp/opencode/evalroot-comm/clusters/*/tools-*.jsonl")
    ap.add_argument("--threshold", type=int, action="append", help="bytes; repeatable; default 4096, then 2048 and 1024")
    args = ap.parse_args()
    files, rows = load(args.glob)
    if not rows:
        sys.exit(f"no tool log lines under {args.glob}")
    print(f"{len(files)} files, {len(rows)} tool calls, {sum(len(r['result'].encode()) for r in rows)} result bytes")
    for t in args.threshold or [4096, 2048, 1024]:
        print(report(estimate(rows, t, False), f"threshold {t} bytes, strict"))
        print(report(estimate(rows, t, True), f"threshold {t} bytes, results at the 4000 character log cap counted as over"))


if __name__ == "__main__":
    main()
