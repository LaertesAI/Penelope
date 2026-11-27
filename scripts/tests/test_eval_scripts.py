"""Tests for the SWE-bench check and the A3 harness. Run: python3 -m unittest discover -s scripts/tests"""
import importlib.util
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


check = load("swebench_check", ROOT / "cli/tests/data/eval/swebench_check.py")
a3 = load("a3_claude_code", ROOT / "scripts/a3_claude_code.py")

BASE = "def test_a():\n    assert True\n"
PATCH = """diff --git a/tests/test_x.py b/tests/test_x.py
--- a/tests/test_x.py
+++ b/tests/test_x.py
@@ -1,2 +1,5 @@
 def test_a():
     assert True
+
+def test_patched():
+    assert True
"""


def git(repo, *args):
    subprocess.run(["git", "-C", str(repo), *args], check=True, capture_output=True, text=True)


class CheckTest(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="penelope-swecheck-"))
        self.repo = self.tmp / "repo"
        (self.repo / "tests").mkdir(parents=True)
        (self.repo / "tests/test_x.py").write_text(BASE)
        git(self.repo, "init", "-q")
        git(self.repo, "add", ".")
        git(self.repo, "-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "base")
        venv = self.tmp / "venv"
        (venv / "bin").mkdir(parents=True)
        (venv / "bin/python").write_text("")
        specs = {"o__r-1": {"repo": "o/r", "version": "1", "test_patch": PATCH,
                            "fail_to_pass": ["tests/test_x.py"], "pass_to_pass": []}}
        # The test command passes only when the test patch is in the file.
        envs = {"o__r-1": {"venv": str(venv), "test_cmd": "grep -q test_patched tests/test_x.py && true"}}
        (self.tmp / "specs.json").write_text(json.dumps(specs))
        (self.tmp / "envs.json").write_text(json.dumps(envs))
        check.SPECS = self.tmp / "specs.json"
        check.ENVS = self.tmp / "envs.json"
        self.cwd = os.getcwd()
        os.chdir(self.repo)

    def tearDown(self):
        os.chdir(self.cwd)
        subprocess.run(["rm", "-rf", str(self.tmp)])

    def run_check(self):
        old = sys.argv
        sys.argv = ["swebench_check.py", "o__r-1"]
        try:
            return check.main()
        finally:
            sys.argv = old

    def test_a_model_edit_in_a_patched_test_file_is_no_infra_failure(self):
        model = "def test_a():\n    assert True\n\ndef test_model():\n    assert True\n"
        (self.repo / "tests/test_x.py").write_text(model)
        self.assertEqual(self.run_check(), 0)
        self.assertEqual((self.repo / "tests/test_x.py").read_text(), model, "the model's version comes back")

    def test_an_untouched_repo_passes_and_stays_clean(self):
        self.assertEqual(self.run_check(), 0)
        self.assertEqual((self.repo / "tests/test_x.py").read_text(), BASE)


class A3RunTest(unittest.TestCase):
    def test_a_timeout_returns_text_and_kills_the_group(self):
        tmp = Path(tempfile.mkdtemp(prefix="penelope-a3-"))
        marker = tmp / "child.pid"
        script = "import os,subprocess,sys,time; print('started', flush=True); " \
                 "c=subprocess.Popen(['sleep','30']); open(sys.argv[1],'w').write(str(c.pid)); time.sleep(30)"
        code, out, err, wall = a3.run([sys.executable, "-c", script, str(marker)], tmp, 2)
        self.assertEqual(code, 124)
        self.assertIsInstance(out, str)
        self.assertIsInstance(err, str)
        json.dumps({"out": out, "err": err})
        self.assertIn("started", out)
        self.assertLess(wall, 20)
        pid = int(marker.read_text())
        alive = subprocess.run(["kill", "-0", str(pid)], capture_output=True).returncode == 0
        if alive:
            # A zombie of a killed child still answers kill -0 until its parent reaps it; check its state.
            state = Path("/proc/{}/stat".format(pid))
            alive = state.is_file() and state.read_text().split()[2] != "Z"
        self.assertFalse(alive, "the grandchild must die with the group")
        subprocess.run(["rm", "-rf", str(tmp)])

    def test_bytes_become_text(self):
        self.assertEqual(a3.as_text(b"ab\xff"), "ab�")
        self.assertEqual(a3.as_text(None), "")
        self.assertEqual(a3.as_text("x"), "x")


if __name__ == "__main__":
    unittest.main()
