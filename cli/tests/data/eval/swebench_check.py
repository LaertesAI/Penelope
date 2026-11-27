#!/usr/bin/env python3
"""Check one SWE-bench Lite task in a prepared repo.

The model saw the issue and the repo, not the test patch. This script applies the test patch, runs every
FAIL_TO_PASS test and then every PASS_TO_PASS test in the repo's own environment, and takes the test patch
back. Exit 0 means resolved. Exit 2 is an infrastructure problem, not a model failure.

Environment:
  SWEBENCH_SPECS   default ~/.cache/penelope/swebench/specs.json
  SWEBENCH_VENVS   default ~/.cache/penelope/swebench/venvs
"""
import json
import os
import subprocess
import sys
from pathlib import Path

CACHE = Path(os.environ.get("SWEBENCH_CACHE", "~/.cache/penelope/swebench")).expanduser()
SPECS = Path(os.environ.get("SWEBENCH_SPECS", CACHE / "specs.json")).expanduser()
VENVS = Path(os.environ.get("SWEBENCH_VENVS", CACHE / "venvs")).expanduser()


def run(args, cwd, text=None):
    return subprocess.run(args, cwd=str(cwd), input=text, text=True, capture_output=True)


def main():
    if len(sys.argv) != 2:
        print("usage: swebench_check.py INSTANCE_ID")
        return 2
    iid = sys.argv[1]
    try:
        specs = json.loads(SPECS.read_text())
    except Exception as e:
        print("infra: cannot read specs {}: {}".format(SPECS, e))
        return 2
    spec = specs.get(iid)
    if not spec:
        print("infra: no spec for {}".format(iid))
        return 2
    repo = Path.cwd()
    venv = VENVS / spec["repo"].replace("/", "__") / "bin" / "python"
    if not venv.is_file():
        print("infra: no environment at {}".format(venv))
        return 2
    patch = spec["test_patch"]
    applied = run(["git", "apply", "--whitespace=nowarn", "-"], repo, patch)
    if applied.returncode != 0:
        print("infra: the test patch did not apply")
        print(applied.stderr[-1200:])
        return 2
    failed = []
    try:
        for test in spec.get("fail_to_pass", []):
            r = run([str(venv), "-m", "pytest", "-q", "--no-header", test], repo)
            if r.returncode != 0:
                failed.append((test, (r.stdout + r.stderr)[-800:]))
        if not failed and spec.get("pass_to_pass"):
            r = run([str(venv), "-m", "pytest", "-q", "--no-header"] + spec["pass_to_pass"], repo)
            if r.returncode != 0:
                failed.append(("pass_to_pass", (r.stdout + r.stderr)[-800:]))
    finally:
        run(["git", "apply", "-R", "--whitespace=nowarn", "-"], repo, patch)
    if failed:
        print("FAIL: {} of {} tests failed".format(len(failed), len(spec.get("fail_to_pass", [])) + 1))
        for test, out in failed[:3]:
            print(test)
            print(out)
        return 1
    print("PASS: {} fail-to-pass and {} pass-to-pass tests".format(
        len(spec.get("fail_to_pass", [])), len(spec.get("pass_to_pass", []))))
    return 0


if __name__ == "__main__":
    sys.exit(main())
