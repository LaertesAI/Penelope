#!/usr/bin/env python3
"""A3: the same 28 dev tasks with Claude Code alone, Haiku at low effort, same check.

No Penelope MCP server. The Claude Code CLI uses its own tools in a throwaway git worktree.
The check is cli/tests/data/eval/swebench_check.py, the same hidden test as the Penelope runs.
No paid API: the command uses the Claude subscription behind the `claude` CLI.
"""
import argparse
import json
import os
import shutil
import signal
import subprocess
import time
from pathlib import Path

ROOT = Path("/tmp/opencode")
PENELOPE = ROOT / "penelope-real"
TASKS = ROOT / "penelope-comm/cli/tests/data/eval/swebench_dev30.jsonl"
WORK = ROOT / "a3" / "work"
OUT = ROOT / "a3" / "runs"
CHECK = PENELOPE / "cli/tests/data/eval/swebench_check.py"
SPECS = Path("/home/rene/.cache/penelope/swebench/specs.json")
ENVS = Path("/home/rene/.cache/penelope/swebench/envs.json")


def as_text(data):
    """TimeoutExpired carries bytes even with text=True; a row must stay JSON text."""
    if data is None:
        return ""
    if isinstance(data, bytes):
        return data.decode("utf-8", errors="replace")
    return data


def run(cmd, cwd, timeout, env=None):
    """Runs one command in its own session. On a timeout the whole group goes, pytest and tool processes too."""
    start = time.time()
    p = subprocess.Popen(cmd, cwd=str(cwd), text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                         env={**os.environ, **(env or {})}, start_new_session=True)
    try:
        out, err = p.communicate(timeout=timeout)
        return p.returncode, out, err, time.time() - start
    except subprocess.TimeoutExpired as e:
        try:
            os.killpg(p.pid, signal.SIGKILL)
        except (ProcessLookupError, PermissionError):
            p.kill()
        out, err = p.communicate()
        return 124, as_text(out or e.stdout), as_text(err or e.stderr), time.time() - start


def worktree(repo, commit, dest):
    if dest.exists():
        subprocess.run(["git", "-C", str(repo), "worktree", "remove", "--force", str(dest)],
                       capture_output=True, text=True)
        shutil.rmtree(dest, ignore_errors=True)
    dest.parent.mkdir(parents=True, exist_ok=True)
    p = subprocess.run(["git", "-C", str(repo), "worktree", "add", "--detach", str(dest), commit],
                       capture_output=True, text=True)
    if p.returncode != 0:
        raise RuntimeError(p.stderr.strip())


def remove_worktree(repo, dest):
    subprocess.run(["git", "-C", str(repo), "worktree", "remove", "--force", str(dest)],
                   capture_output=True, text=True)
    shutil.rmtree(dest, ignore_errors=True)


def usage_of(v):
    u = v.get("usage") or {}
    tin = int(u.get("input_tokens") or 0) + int(u.get("cache_creation_input_tokens") or 0)
    tout = int(u.get("output_tokens") or 0)
    cost = v.get("total_cost_usd")
    return tin, tout, cost


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--tasks", default=str(TASKS))
    ap.add_argument("--limit", type=int, default=0)
    ap.add_argument("--timeout", type=int, default=1800)
    ap.add_argument("--resume", action="store_true")
    a = ap.parse_args()
    OUT.mkdir(parents=True, exist_ok=True)
    WORK.mkdir(parents=True, exist_ok=True)
    specs = json.loads(SPECS.read_text())
    envs = json.loads(ENVS.read_text())
    tasks = [json.loads(line) for line in Path(a.tasks).read_text().splitlines() if line.strip()]
    # The two scikit tasks are infra in this environment. The comparison is the 28 model tasks.
    tasks = [t for t in tasks if "scikit" not in t["id"]]
    if a.limit:
        tasks = tasks[:a.limit]
    for i, task in enumerate(tasks, 1):
        iid = task["id"]
        dest = OUT / (iid + ".json")
        if a.resume and dest.is_file():
            print(f"{i}/{len(tasks)} {iid}: kept", flush=True)
            continue
        repo = Path(task["setup"]["repo"])
        commit = task["base_commit"]
        wt = WORK / iid
        row = {"id": iid, "started": int(time.time())}
        try:
            worktree(repo, commit, wt)
        except Exception as e:
            row.update({"error": "worktree: " + str(e)})
            dest.write_text(json.dumps(row, indent=1))
            print(f"{i}/{len(tasks)} {iid}: worktree error {e}", flush=True)
            continue
        spec = specs[iid]
        key = spec["repo"].replace("/", "__") + "-" + spec.get("version", "")
        venv = Path(envs[key]["venv"])
        prompt = (task["prompt"] + "\n\nThe repository is the current directory. Fix the issue and run a quick check. "
                  "A prebuilt virtualenv for this project is active: `python` and `pytest` are ready. "
                  "Do not build the project from source.")
        claude_env = os.environ.copy()
        claude_env["VIRTUAL_ENV"] = str(venv)
        claude_env["PATH"] = str(venv / "bin") + os.pathsep + claude_env.get("PATH", "")
        claude_env["PYTHONPATH"] = os.pathsep.join([str(wt / "src"), str(wt / "lib"), str(wt)])
        cmd = ["claude", "-p", prompt, "--output-format", "json", "--model", "haiku",
               "--effort", "low", "--dangerously-skip-permissions",
               "--strict-mcp-config", "--mcp-config", '{"mcpServers":{}}']
        code, out, err, wall = run(cmd, wt, a.timeout, env=claude_env)
        row.update({"claude_exit": code, "claude_wall_s": round(wall, 1), "claude_stderr": err[-2000:]})
        try:
            v = json.loads(out)
            row["claude"] = v
            row["tokens_in"], row["tokens_out"], row["total_cost_usd"] = usage_of(v)
            row["is_error"] = bool(v.get("is_error"))
            row["result"] = (v.get("result") or "")[:2000]
        except Exception:
            row["claude_raw"] = out[-4000:]
            row["tokens_in"] = row["tokens_out"] = 0
            row["is_error"] = code != 0
        ccode, cout, cerr, cwall = run(["python3", str(CHECK), iid], wt, a.timeout,
                                       env={"PENELOPE_ROOT": str(PENELOPE)})
        row.update({"check_exit": ccode, "check_wall_s": round(cwall, 1),
                    "check_output": (cout + "\n" + cerr)[-4000:],
                    "passed": ccode == 0, "infra": ccode == 2})
        remove_worktree(repo, wt)
        dest.write_text(json.dumps(row, indent=1))
        print(f"{i}/{len(tasks)} {iid}: pass={row['passed']} infra={row['infra']} "
              f"tok={row['tokens_in']}/{row['tokens_out']} wall={row['claude_wall_s']}s", flush=True)


if __name__ == "__main__":
    main()
