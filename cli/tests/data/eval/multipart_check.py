#!/usr/bin/env python3
"""Check one A5 multipart task. Copies the hidden unittest into the task folder, runs it, removes it.

Exit 0 means all three parts pass. Exit 1 means a part failed. Exit 2 is an infrastructure problem.
"""
import subprocess
import sys
from pathlib import Path


def main():
    if len(sys.argv) != 2:
        print("usage: multipart_check.py TASK_ID")
        return 2
    iid = sys.argv[1]
    root = Path(__file__).resolve().parent
    test = root / "multipart" / "checks" / (iid + ".py")
    if not test.is_file():
        print("infra: no hidden test for {}".format(iid))
        return 2
    repo = Path.cwd()
    dest = repo / "test_hidden.py"
    dest.write_bytes(test.read_bytes())
    try:
        p = subprocess.run([sys.executable, "-m", "unittest", "-q", "test_hidden"],
                           cwd=str(repo), text=True, capture_output=True)
        print(p.stdout + p.stderr)
        return 0 if p.returncode == 0 else 1
    finally:
        dest.unlink(missing_ok=True)


if __name__ == "__main__":
    sys.exit(main())
