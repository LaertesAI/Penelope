#!/usr/bin/env python3
"""A3 summary: Claude Code alone against Penelope's K9 small model on the 28 dev tasks."""
import json
import math
from pathlib import Path

RATE = 0.92
HAIKU = (0.1, 0.5)
ROOT = Path("/tmp/opencode")
A3 = ROOT / "a3/runs"
K9 = ROOT / "evalroot-comm/eval/runs/cluster-1795778629608.json"


def eur(tin, tout):
    return (tin * HAIKU[0] + tout * HAIKU[1]) / 1_000_000.0 * RATE


def mcnemar(b, c):
    n = b + c
    if n == 0:
        return 1.0
    return min(1.0, 2 * sum(math.comb(n, k) for k in range(min(b, c) + 1)) / (2 ** n))


def main():
    a3 = {}
    for p in A3.glob("*.json"):
        d = json.loads(p.read_text())
        a3[d["id"]] = d
    k9_run = json.loads(K9.read_text())
    k9 = {r["id"]: r for r in k9_run["results"] if "scikit" not in r["id"]}
    ids = sorted(set(a3) & set(k9))
    rows = []
    for i in ids:
        a = a3[i]
        k = k9[i]
        rows.append({"id": i, "a3_pass": bool(a.get("passed")), "a3_infra": bool(a.get("infra")),
                     "a3_in": a.get("tokens_in", 0), "a3_out": a.get("tokens_out", 0),
                     "a3_ms": a.get("claude_wall_s", 0) * 1000 + a.get("check_wall_s", 0) * 1000,
                     "k9_pass": bool(k["passed"]), "k9_in": k["tokens_in"], "k9_out": k["tokens_out"],
                     "k9_eur": eur(k["tokens_in"], k["tokens_out"]), "k9_ms": k["wall_ms"]})
    b = [r["id"] for r in rows if r["a3_pass"] and not r["k9_pass"]]
    c = [r["id"] for r in rows if r["k9_pass"] and not r["a3_pass"]]
    print("| Aufgabe | Claude Code | Penelope K9 | A3 in | A3 out | A3 EUR | K9 EUR | A3 s | K9 s |")
    print("|---|---|---|---|---|---|---|---|---|")
    for r in rows:
        print("| `{}` | {} | {} | {} | {} | {:.4f} | {:.4f} | {:.0f} | {:.0f} |".format(
            r["id"], "ja" if r["a3_pass"] else ("infra" if r["a3_infra"] else "nein"),
            "ja" if r["k9_pass"] else "nein", r["a3_in"], r["a3_out"], eur(r["a3_in"], r["a3_out"]),
            r["k9_eur"], r["a3_ms"] / 1000, r["k9_ms"] / 1000))
    print()
    print("tasks", len(rows), "Claude Code pass", sum(r["a3_pass"] for r in rows),
          "Penelope K9 pass", sum(r["k9_pass"] for r in rows))
    print("A3 only (b):", len(b), b)
    print("K9 only (c):", len(c), c)
    print("exact McNemar p = {:.4f}".format(mcnemar(len(b), len(c))))
    print("A3 tokens", sum(r["a3_in"] for r in rows), sum(r["a3_out"] for r in rows),
          "EUR", round(sum(eur(r["a3_in"], r["a3_out"]) for r in rows), 4),
          "wall", round(sum(r["a3_ms"] for r in rows) / 1000), "s")
    print("K9 tokens", sum(r["k9_in"] for r in rows), sum(r["k9_out"] for r in rows),
          "EUR", round(sum(r["k9_eur"] for r in rows), 4),
          "wall", round(sum(r["k9_ms"] for r in rows) / 1000), "s")


if __name__ == "__main__":
    main()
