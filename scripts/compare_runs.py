#!/usr/bin/env python3
"""Paired hold-out table and exact McNemar test. No model, no network.

Reads two eval run JSONs from --root, drops infra tasks, and prints the table for docs/RUNS.md.
"""
import argparse
import json
import math
from pathlib import Path


def load_run(root, run_id):
    path = Path(root) / "eval" / "runs" / (run_id + ".json")
    return json.loads(path.read_text())


def load_many(root, ids):
    runs = [load_run(root, x.strip()) for x in ids.split(",") if x.strip()]
    merged = {"results": [], "model": runs[0].get("model") if runs else None}
    for run in runs:
        merged["results"].extend(run["results"])
    return merged


def by_id(run):
    return {r["id"]: r for r in run["results"]}


def is_infra(r):
    return (r.get("check_output") or "").strip().lower().startswith("infra")


def mcnemar(b, c):
    n = b + c
    if n == 0:
        return 1.0
    tail = sum(math.comb(n, k) for k in range(0, min(b, c) + 1)) / (2 ** n)
    return min(1.0, 2 * tail)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--root", default="/tmp/opencode/evalroot-comm")
    ap.add_argument("--large", required=True)
    ap.add_argument("--cluster", required=True)
    a = ap.parse_args()
    large_run = load_many(a.root, a.large)
    cluster_run = load_many(a.root, a.cluster)
    l = by_id(large_run)
    c = by_id(cluster_run)
    ids = [i for i in l if i in c and not is_infra(l[i]) and not is_infra(c[i])]
    infra = [i for i in l if i in c and (is_infra(l[i]) or is_infra(c[i]))]
    rows = []
    for i in ids:
        rows.append({"id": i, "large": l[i], "cluster": c[i]})
    b_ids = [r["id"] for r in rows if r["cluster"]["passed"] and not r["large"]["passed"]]
    c_ids = [r["id"] for r in rows if r["large"]["passed"] and not r["cluster"]["passed"]]
    both = [r["id"] for r in rows if r["large"]["passed"] and r["cluster"]["passed"]]
    neither = [r["id"] for r in rows if not r["large"]["passed"] and not r["cluster"]["passed"]]
    sums = {
        "large_eur": sum(r["large"]["eur"] for r in rows),
        "cluster_eur": sum(r["cluster"]["eur"] for r in rows),
        "large_in": sum(r["large"]["tokens_in"] for r in rows),
        "cluster_in": sum(r["cluster"]["tokens_in"] for r in rows),
        "large_out": sum(r["large"]["tokens_out"] for r in rows),
        "cluster_out": sum(r["cluster"]["tokens_out"] for r in rows),
        "large_ms": sum(r["large"]["wall_ms"] for r in rows),
        "cluster_ms": sum(r["cluster"]["wall_ms"] for r in rows),
    }
    print("| Aufgabe | large | cluster | EUR L | EUR C | s L | s C | in L | in C | out L | out C |")
    print("|---|---|---|---|---|---|---|---|---|---|---|")
    for r in rows:
        print("| `{}` | {} | {} | {:.4f} | {:.4f} | {:.0f} | {:.0f} | {} | {} | {} | {} |".format(
            r["id"], "ja" if r["large"]["passed"] else "nein", "ja" if r["cluster"]["passed"] else "nein",
            r["large"]["eur"], r["cluster"]["eur"], r["large"]["wall_ms"] / 1000, r["cluster"]["wall_ms"] / 1000,
            r["large"]["tokens_in"], r["cluster"]["tokens_in"],
            r["large"]["tokens_out"], r["cluster"]["tokens_out"]))
    print()
    print("| Kennzahl | large | cluster | cluster/large |")
    print("|---|---|---|---|")
    print("| Treffer | {} / {} | {} / {} | {:.2f} |".format(
        sum(r["large"]["passed"] for r in rows), len(rows),
        sum(r["cluster"]["passed"] for r in rows), len(rows),
        (sum(r["cluster"]["passed"] for r in rows) / sum(r["large"]["passed"] for r in rows))
        if sum(r["large"]["passed"] for r in rows) else 0.0))
    print("| Eingabetokens | {} | {} | {:.2f} |".format(
        sums["large_in"], sums["cluster_in"], sums["cluster_in"] / sums["large_in"] if sums["large_in"] else 0.0))
    print("| Ausgabetokens | {} | {} | {:.2f} |".format(
        sums["large_out"], sums["cluster_out"], sums["cluster_out"] / sums["large_out"] if sums["large_out"] else 0.0))
    print("| EUR | {:.4f} | {:.4f} | {:.2f} |".format(
        sums["large_eur"], sums["cluster_eur"], sums["cluster_eur"] / sums["large_eur"] if sums["large_eur"] else 0.0))
    print("| Zeit | {:.0f} s | {:.0f} s | {:.2f} |".format(
        sums["large_ms"] / 1000, sums["cluster_ms"] / 1000,
        sums["cluster_ms"] / sums["large_ms"] if sums["large_ms"] else 0.0))
    print()
    print("large pass, cluster fail (c):", len(c_ids), c_ids)
    print("cluster pass, large fail (b):", len(b_ids), b_ids)
    print("both pass:", len(both))
    print("neither:", len(neither))
    print("exact McNemar two-sided p = {:.4f}".format(mcnemar(len(b_ids), len(c_ids))))
    print("infra:", infra)
    print("models:", large_run.get("model"), "/", cluster_run.get("model"))
    print("runs:", a.large, a.cluster)


if __name__ == "__main__":
    main()
