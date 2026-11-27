#!/usr/bin/env python3
"""Preflight the 30 A5 multipart tasks: every buggy repo fails, every gold fix passes."""
import json
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parents[1]
EVAL = HERE / "cli/tests/data/eval"
BASE = EVAL / "multipart"
CHECK = EVAL / "multipart_check.py"
sys.path.insert(0, str(HERE / "scripts"))
from make_multipart import module  # noqa: E402


def run(check, repo, iid):
    return subprocess.run([sys.executable, str(check), iid], cwd=str(repo), text=True, capture_output=True)


def main():
    rows = []
    for name in ("tasks_dev20.jsonl", "tasks_holdout10.jsonl"):
        rows += [json.loads(line) for line in (BASE / name).read_text().splitlines() if line.strip()]
    bad = 0
    with tempfile.TemporaryDirectory(prefix="mp-preflight-") as tmp:
        for task in rows:
            iid = task["id"]
            src = HERE / "cli" / task["setup"]["repo"]
            work = Path(tmp) / iid
            shutil.copytree(src, work)
            buggy = run(CHECK, work, iid)
            for mod in ("calc", "text", "data"):
                (work / "multipart" / (mod + ".py")).write_text(module(mod, []))
            fixed = run(CHECK, work, iid)
            ok = buggy.returncode == 1 and fixed.returncode == 0
            if not ok:
                bad += 1
                print("FAIL", iid, "buggy", buggy.returncode, "fixed", fixed.returncode)
                print((buggy.stdout + buggy.stderr)[-400:])
                print((fixed.stdout + fixed.stderr)[-400:])
            else:
                print("ok", iid)
    print("tasks", len(rows), "bad", bad)
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
