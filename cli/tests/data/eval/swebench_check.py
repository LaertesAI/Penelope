#!/usr/bin/env python3
"""Check one SWE-bench Lite task in a prepared repo.

The model saw the issue and the repo, not the test patch. This script applies the test patch, runs every
FAIL_TO_PASS test and then every PASS_TO_PASS test with the environment of the repo and version, and takes
the test patch back. Exit 0 means resolved. Exit 2 is an infrastructure problem, not a model failure.

Environment:
  SWEBENCH_SPECS   default ~/.cache/penelope/swebench/specs.json
  SWEBENCH_ENVS    default ~/.cache/penelope/swebench/envs.json
"""
import json
import os
import shlex
import shutil
import subprocess
import sys
from pathlib import Path

CACHE = Path(os.environ.get("SWEBENCH_CACHE", "~/.cache/penelope/swebench")).expanduser()
SPECS = Path(os.environ.get("SWEBENCH_SPECS", CACHE / "specs.json")).expanduser()
ENVS = Path(os.environ.get("SWEBENCH_ENVS", CACHE / "envs.json")).expanduser()


def run(args, cwd, env=None, text=None):
    return subprocess.run(args, cwd=str(cwd), env=env, input=text, text=True, capture_output=True)


def copy_extensions(build: Path, repo: Path):
    """Compiled repos keep their .so files in the build worktree; the task worktree gets a fresh checkout."""
    if not build.is_dir():
        return
    for pattern in ("*.so", "*_version.py"):
        for f in build.rglob(pattern):
            rel = f.relative_to(build)
            dest = repo / rel
            if not dest.exists():
                dest.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(f, dest)


def directive(test, repo):
    """Django lists a unittest as `test_name (module.Class)`; runtests.py wants `module.Class.test_name`."""
    if repo == "django/django" and " (" in test and test.rstrip().endswith(")"):
        name, rest = test.split(" (", 1)
        return rest[:-1] + "." + name
    return test


def main():
    if len(sys.argv) != 2:
        print("usage: swebench_check.py INSTANCE_ID")
        return 2
    iid = sys.argv[1]
    try:
        specs = json.loads(SPECS.read_text())
        envs = json.loads(ENVS.read_text())
    except Exception as e:
        print("infra: cannot read specs or envs: {}".format(e))
        return 2
    spec = specs.get(iid)
    if not spec:
        print("infra: no spec for {}".format(iid))
        return 2
    key = spec["repo"].replace("/", "__") + "-" + spec.get("version", "")
    env = envs.get(key)
    if not env or not env.get("venv"):
        print("infra: no environment for {}".format(key))
        return 2
    venv = Path(env["venv"])
    build = Path(env.get("build", ""))
    if not (venv / "bin" / "python").is_file():
        print("infra: no python at {}".format(venv / "bin" / "python"))
        return 2
    repo = Path.cwd()
    copy_extensions(build, repo)
    cmd = env.get("test_cmd") or "python -m pytest -q"
    if spec["repo"] == "sphinx-doc/sphinx":
        cmd = "pytest -rA"
    patch = spec["test_patch"]
    applied = run(["git", "apply", "--whitespace=nowarn", "-"], repo, text=patch)
    if applied.returncode != 0:
        print("infra: the test patch did not apply")
        print(applied.stderr[-1200:])
        return 2
    envp = os.environ.copy()
    envp["PATH"] = str(venv / "bin") + os.pathsep + envp.get("PATH", "")
    # Some repos keep the package under src/ or lib/; the task worktree must win over the build install.
    roots = [str(repo / "src"), str(repo / "lib"), str(repo)]
    envp["PYTHONPATH"] = os.pathsep.join(roots) + os.pathsep + envp.get("PYTHONPATH", "")
    failed = []
    try:
        for raw in spec.get("fail_to_pass", []):
            test = directive(raw, spec["repo"])
            r = run(["bash", "-c", cmd + " " + shlex.quote(test)], repo, env=envp)
            if r.returncode != 0:
                failed.append((test, (r.stdout + r.stderr)[-800:]))
        if not failed and spec.get("pass_to_pass"):
            joined = " ".join(shlex.quote(directive(t, spec["repo"])) for t in spec["pass_to_pass"])
            r = run(["bash", "-c", cmd + " " + joined], repo, env=envp)
            if r.returncode != 0:
                failed.append(("pass_to_pass", (r.stdout + r.stderr)[-800:]))
    finally:
        run(["git", "apply", "-R", "--whitespace=nowarn", "-"], repo, text=patch)
    if failed:
        print("FAIL: {} of {} test groups failed".format(len(failed), len(spec.get("fail_to_pass", [])) + 1))
        for test, out in failed[:3]:
            print(test)
            print(out)
        return 1
    print("PASS: {} fail-to-pass and {} pass-to-pass tests".format(
        len(spec.get("fail_to_pass", [])), len(spec.get("pass_to_pass", []))))
    return 0


if __name__ == "__main__":
    sys.exit(main())
