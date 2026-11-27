#!/usr/bin/env python3
"""Cluster-Messung Schritt 1: paired baseline, tokens, time, Eurykleia.

Reads the logs of the SWE-bench baseline under --root. No model, no network, no API cost.
The Eurykleia check mirrors cli/src/eurykleia.rs at commit a8febf1: E_NAME, E_REQ, E_TYPE,
E_EXTRA, E_FMT. The tool schemas come from `penelope mcp` tools/list.

Warning: calls.jsonl holds only a target summary, not the real arguments; the real arguments and results
were in tools-<part>.jsonl and are removed after the check. The Eurykleia flags in this script are an
artifact of that summary, not a TPR. See intern/Cluster-Messung Schritt 1.md.
"""
import argparse
import json
import os
import re
import subprocess
import sys
from collections import Counter, defaultdict
from pathlib import Path

FIRST_LARGE = "large-1795725602938"
FIRST_CLUSTER = "cluster-1795727879911"
RETRY_LARGE = "large-1795734391977"
RETRY_CLUSTER = "cluster-1795734922445"


def load(path):
    return json.loads(Path(path).read_text())


def run_results(path):
    return {x["id"]: x for x in load(path)["results"]}


def cluster_id(result):
    m = re.search(r"eval-small-(\d+)", result.get("answer", ""))
    return "eval-small-" + m.group(1) if m else ""


def join(path, key):
    return key if not path else path + "." + key


def fits(kind, v):
    if kind == "string":
        return isinstance(v, str)
    if kind == "integer":
        return isinstance(v, int) and not isinstance(v, bool)
    if kind == "number":
        return isinstance(v, (int, float)) and not isinstance(v, bool)
    if kind == "boolean":
        return isinstance(v, bool)
    if kind == "array":
        return isinstance(v, list)
    if kind == "object":
        return isinstance(v, dict)
    return True


def is_date(s):
    m = re.fullmatch(r"(\d{4})-(\d{2})-(\d{2})", s)
    if not m:
        return False
    y, mo, d = map(int, m.groups())
    leap = (y % 4 == 0 and y % 100 != 0) or y % 400 == 0
    days = [31, 29 if leap else 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    return 1 <= mo <= 12 and 1 <= d <= days[mo - 1]


def is_datetime(s):
    m = re.match(r"^(\d{4}-\d{2}-\d{2})[Tt ](\d{2}):(\d{2})(?::(\d{2})(?:\.\d+)?)?(Z|z|[+-]\d{2}:?\d{2})?$", s)
    if not m or not is_date(m.group(1)):
        return False
    h, mi = int(m.group(2)), int(m.group(3))
    sec = int(m.group(4)) if m.group(4) else 0
    zone = m.group(5) or ""
    if h > 23 or mi > 59 or sec > 60:
        return False
    if zone and zone not in ("Z", "z"):
        body = zone[1:].replace(":", "")
        if len(body) != 4:
            return False
        zh, zm = int(body[:2]), int(body[2:])
        if zh > 23 or zm > 59:
            return False
    return True


def is_city(s):
    parts = [p.strip() for p in s.split(",")]
    return len(parts) == 2 and all(p and any(c.isalpha() for c in p) for p in parts)


def is_email(s):
    if s.count("@") != 1 or any(c.isspace() for c in s):
        return False
    local, domain = s.split("@")
    return bool(local) and "." in domain and not domain.startswith(".") and not domain.endswith(".") and ".." not in domain


def is_url(s):
    if "://" not in s or any(c.isspace() for c in s):
        return False
    scheme, rest = s.split("://", 1)
    host = re.split(r"[/?#]", rest, 1)[0]
    return bool(scheme) and all(c.isalnum() or c in "+-." for c in scheme) and bool(host)


def hint(schema):
    fmt = schema.get("format", "")
    if fmt == "date":
        return "date"
    if fmt == "date-time":
        return "datetime"
    if fmt == "email":
        return "email"
    if fmt in ("uri", "url"):
        return "url"
    d = (schema.get("description") or "").lower()
    if "yyyy-mm-ddt" in d or "iso 8601 date-time" in d or "iso date-time" in d:
        return "datetime"
    if "yyyy-mm-dd" in d:
        return "date"
    if "city, state" in d or "city, country" in d or "city and state" in d:
        return "city"
    if "email address" in d or "e-mail address" in d:
        return "email"
    if any(w in ("url", "uri") for w in re.split(r"[^a-z0-9]+", d)):
        return "url"
    return ""


def value_format(schema, value, path):
    if "enum" in schema and isinstance(schema["enum"], list):
        if not any(v == value for v in schema["enum"]):
            return ("E_FMT", path)
    if isinstance(value, (int, float)) and not isinstance(value, bool):
        if isinstance(schema.get("minimum"), (int, float)) and value < schema["minimum"]:
            return ("E_FMT", path)
        if isinstance(schema.get("maximum"), (int, float)) and value > schema["maximum"]:
            return ("E_FMT", path)
    if isinstance(value, str) and value:
        h = hint(schema)
        if h == "date" and not is_date(value):
            return ("E_FMT", path)
        if h == "datetime" and not is_datetime(value):
            return ("E_FMT", path)
        if h == "city" and not is_city(value):
            return ("E_FMT", path)
        if h == "email" and not is_email(value):
            return ("E_FMT", path)
        if h == "url" and not is_url(value):
            return ("E_FMT", path)
    return None


def walk(pass_, schema, value, path):
    kind = schema.get("type", "")
    if pass_ == "type" and kind and not fits(kind, value):
        return ("E_TYPE", path)
    if pass_ == "fmt":
        f = value_format(schema, value, path)
        if f:
            return f
    if isinstance(value, dict):
        props = schema.get("properties") or {}
        if pass_ == "req":
            for key in schema.get("required") or []:
                v = value.get(key)
                if v is None or (isinstance(v, str) and v == ""):
                    return ("E_REQ", join(path, key))
        if pass_ == "extra" and isinstance(props, dict):
            ap = schema.get("additionalProperties")
            if ap is not True and not isinstance(ap, dict):
                for key in value:
                    if key not in props:
                        return ("E_EXTRA", join(path, key))
        if isinstance(props, dict):
            for key, sub in props.items():
                if key in value and value[key] is not None:
                    f = walk(pass_, sub, value[key], join(path, key))
                    if f:
                        return f
    if isinstance(value, list) and isinstance(schema.get("items"), dict):
        for i, item in enumerate(value):
            f = walk(pass_, schema["items"], item, "{}[{}]".format(path, i))
            if f:
                return f
    return None


def eurykleia(schema, args):
    if schema is None:
        return ("E_NAME", "")
    if not isinstance(args, dict):
        return ("E_TYPE", "arguments")
    for pass_ in ("req", "type", "extra", "fmt"):
        f = walk(pass_, schema, args, "")
        if f:
            return f
    return None


def tool_schemas(binary, root):
    proc = subprocess.Popen([binary, "--root", root, "mcp"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                            stderr=subprocess.DEVNULL, text=True, bufsize=1)
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
             "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "step1", "version": "0"}}})
        proc.stdin.write(json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized"}) + "\n")
        proc.stdin.flush()
        result = rpc({"jsonrpc": "2.0", "id": 2, "method": "tools/list"})["result"]
        tools = result["tools"]
        return {t["name"]: t.get("inputSchema") or {"type": "object"} for t in tools}, tools
    finally:
        proc.terminate()


def parse_calls(path):
    out = []
    if not path.exists():
        return out
    for line in path.read_text().splitlines():
        if not line.strip():
            continue
        t, part, name, rest = line.split(" ", 3)
        args_text, result_hash = rest.rsplit(" ", 1)
        try:
            args = json.loads(args_text)
        except json.JSONDecodeError:
            args = {}
        out.append({"t": int(t), "part": part, "name": name, "args": args, "hash": result_hash, "raw": args_text})
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--root", default="/tmp/opencode/evalroot-comm")
    ap.add_argument("--binary", default="/tmp/opencode/target-comm/release/penelope")
    ap.add_argument("--out", default="/tmp/opencode/step1.json")
    a = ap.parse_args()
    root = Path(a.root)
    runs = root / "eval" / "runs"
    first_l = run_results(runs / (FIRST_LARGE + ".json"))
    first_c = run_results(runs / (FIRST_CLUSTER + ".json"))
    retry_l = run_results(runs / (RETRY_LARGE + ".json"))
    retry_c = run_results(runs / (RETRY_CLUSTER + ".json"))
    schemas, tools = tool_schemas(a.binary, str(root))
    tool_bytes = len(json.dumps(tools, separators=(",", ":"), ensure_ascii=False))
    retry_ids = set(retry_l)
    excluded = [i for i in first_l if "scikit" in i]
    ids = [i for i in first_l if i not in excluded and (i in retry_ids or not (first_l[i].get("check_output") or "").lower().startswith("infra"))]
    rows = []
    for i in ids:
        l = retry_l[i] if i in retry_ids else first_l[i]
        c = retry_c[i] if i in retry_ids else first_c[i]
        rows.append({"id": i, "large": l, "cluster": c, "cluster_id": cluster_id(c), "retry": i in retry_ids})
    b = [r["id"] for r in rows if r["cluster"]["passed"] and not r["large"]["passed"]]
    c = [r["id"] for r in rows if r["large"]["passed"] and not r["cluster"]["passed"]]
    n = len(rows)
    # cluster logs
    total_calls = 0
    total_args_chars = 0
    total_final_chars = 0
    total_memory_chars = 0
    total_rounds = 0
    duplicates = 0
    duplicate_chars = 0
    cluster_t = 0
    wall_c = 0
    wall_l = 0
    part_counts = Counter()
    validator_seen = 0
    flagged_tasks = []
    failed_flagged = 0
    passed_flagged = 0
    failed = [r for r in rows if not r["cluster"]["passed"]]
    passed = [r for r in rows if r["cluster"]["passed"]]
    memory_error_tasks = 0
    memory_error_flagged = 0
    no_error_flagged = 0
    validator_noticed = 0
    per_task = []
    for r in rows:
        cid = r["cluster_id"]
        state = load(root / "clusters" / (cid + ".json")) if cid else {}
        calls = parse_calls(root / "clusters" / cid / "calls.jsonl") if cid else []
        memory_path = root / "clusters" / cid / "memory.md" if cid else None
        memory = memory_path.read_text() if memory_path and memory_path.exists() else ""
        parts = state.get("parts") or []
        part_counts[len(parts)] += 1
        if len(parts) > 1 and any(p.get("state") == "validated" for p in parts):
            validator_seen += 1
        t = int(state.get("updated", 0)) - int(state.get("started", 0))
        cluster_t += t
        wall_c += r["cluster"]["wall_ms"]
        wall_l += r["large"]["wall_ms"]
        flags = []
        for call in calls:
            f = eurykleia(schemas.get(call["name"]), call["args"])
            if f:
                flags.append((call["name"], f[0], f[1]))
        total_calls += len(calls)
        total_rounds += len(calls) + 1
        total_args_chars += sum(len(x["raw"]) for x in calls)
        seen = {}
        for call in calls:
            key = (call["part"], call["name"], call["raw"], call["hash"])
            if key in seen:
                duplicates += 1
                duplicate_chars += len(call["raw"])
            seen[key] = True
        total_final_chars += sum(len(p.get("output") or "") for p in parts)
        total_memory_chars += len(memory)
        mem_error = bool(re.search(r"exit code [1-9]|NOT run|Traceback|not found|failed|error", memory, re.I))
        if mem_error:
            memory_error_tasks += 1
        if flags:
            flagged_tasks.append(r["id"])
            if mem_error:
                memory_error_flagged += 1
            if not mem_error:
                no_error_flagged += 1
            if not r["cluster"]["passed"]:
                failed_flagged += 1
            else:
                passed_flagged += 1
        # validator noticed a failed test in the part output
        if not r["cluster"]["passed"] and any(p.get("state") == "validated" for p in parts):
            validator_noticed += 1
        per_task.append({"id": r["id"], "retry": r["retry"], "large_passed": r["large"]["passed"],
                         "cluster_passed": r["cluster"]["passed"], "large_eur": r["large"]["eur"],
                         "cluster_eur": r["cluster"]["eur"], "large_ms": r["large"]["wall_ms"],
                         "cluster_ms": r["cluster"]["wall_ms"], "large_in": r["large"]["tokens_in"],
                         "cluster_in": r["cluster"]["tokens_in"], "large_out": r["large"]["tokens_out"],
                         "cluster_out": r["cluster"]["tokens_out"], "cluster_t": t, "calls": len(calls),
                         "parts": len(parts), "flags": flags, "memory_error": mem_error})
    tpr = failed_flagged / len(failed) if failed else 0.0
    fpr = passed_flagged / len(passed) if passed else 0.0
    tpr_mem = memory_error_flagged / memory_error_tasks if memory_error_tasks else 0.0
    fpr_mem = no_error_flagged / (n - memory_error_tasks) if n - memory_error_tasks else 0.0
    out = {
        "n": n, "b": b, "c": c, "b_share": len(b) / n, "tpr": tpr, "fpr": fpr,
        "tpr_memory": tpr_mem, "fpr_memory": fpr_mem,
        "flagged_tasks": flagged_tasks, "failed": len(failed), "passed": len(passed),
        "memory_error_tasks": memory_error_tasks, "excluded": excluded,
        "sums": {"large_eur": sum(r["large"]["eur"] for r in rows), "cluster_eur": sum(r["cluster"]["eur"] for r in rows),
                 "large_ms": wall_l, "cluster_ms": wall_c, "cluster_t_s": cluster_t,
                 "large_in": sum(r["large"]["tokens_in"] for r in rows), "cluster_in": sum(r["cluster"]["tokens_in"] for r in rows),
                 "large_out": sum(r["large"]["tokens_out"] for r in rows), "cluster_out": sum(r["cluster"]["tokens_out"] for r in rows),
                 "calls": total_calls, "rounds": total_rounds, "args_chars": total_args_chars,
                 "final_chars": total_final_chars, "memory_chars": total_memory_chars, "duplicates": duplicates,
                 "duplicate_chars": duplicate_chars},
        "tool_count": len(tools), "tool_bytes": tool_bytes,
        "part_counts": part_counts, "validator_seen": validator_seen, "validator_noticed": validator_noticed,
        "per_task": per_task,
    }
    Path(a.out).write_text(json.dumps(out, indent=1, ensure_ascii=False) + "\n")
    print(json.dumps({k: v for k, v in out.items() if k != "per_task"}, indent=1, ensure_ascii=False))


if __name__ == "__main__":
    main()
