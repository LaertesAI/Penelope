#!/usr/bin/env python3
"""A5: build 30 multipart tasks, each with three independent parts and its own hidden test.

The package `multipart` has three modules. Every task carries one bug per module. The model sees the
three symptoms and the package. The check copies one hidden unittest file into the task folder, runs it,
and removes it. The three parts are independent, so a planner can split them by module.

The 20 dev and 10 hold-out tasks are split by SHA-256 of the task id. The hold-out hash is printed.
"""
import hashlib
import json
import shutil
from pathlib import Path

HERE = Path(__file__).resolve().parents[1]
EVAL = HERE / "cli/tests/data/eval"
BASE = EVAL / "multipart"
REPOS = BASE / "repos"
CHECKS = BASE / "checks"

CORRECT = {
    "calc": {
        "add": "def add(a, b):\n    return a + b\n",
        "mean": "def mean(values):\n    if not values:\n        return 0.0\n    return sum(values) / len(values)\n",
        "clamp": "def clamp(x, lo, hi):\n    return max(lo, min(x, hi))\n",
    },
    "text": {
        "slugify": "def slugify(text):\n    return text.strip().lower().replace(\" \", \"-\")\n",
        "truncate": "def truncate(text, n):\n    return text if len(text) <= n else text[:n] + \"...\"\n",
        "word_count": "def word_count(text):\n    return len(text.split())\n",
    },
    "data": {
        "parse_pairs": (
            "def parse_pairs(text):\n"
            "    out = {}\n"
            "    for part in text.split(\",\"):\n"
            "        part = part.strip()\n"
            "        if not part:\n"
            "            continue\n"
            "        key, value = part.split(\"=\", 1)\n"
            "        out[key.strip()] = value.strip()\n"
            "    return out\n"
        ),
        "group_by": (
            "def group_by(items, key):\n"
            "    out = {}\n"
            "    for item in items:\n"
            "        out.setdefault(item[key], []).append(item)\n"
            "    return out\n"
        ),
        "merge_dicts": "def merge_dicts(a, b):\n    return {**a, **b}\n",
    },
}

BUGS = {
    "calc": [
        ("add", "return a - b", "`add(2, 3)` returns -1 instead of 5."),
        ("mean", "return sum(values) / (len(values) + 1)",
         "`mean([1, 2, 3])` returns 1.5 instead of 2.0."),
        ("clamp", "return max(x, lo)", "`clamp(10, 0, 5)` returns 10 instead of 5."),
    ],
    "text": [
        ("slugify", 'return text.lower().replace(" ", "_")',
         '`slugify("Hello World")` returns "hello_world" instead of "hello-world".'),
        ("truncate", "return text[:n]",
         '`truncate("abcdef", 3)` returns "abc" instead of "abc...".'),
        ("word_count", "return len(text)", '`word_count("a b")` returns 3 instead of 2.'),
    ],
    "data": [
        ("parse_pairs", 'return dict(p.split(":") for p in text.split(","))',
         '`parse_pairs("a=1, b=2")` raises instead of returning a dict.'),
        ("group_by", "return {item[key]: [item] for item in items}",
         '`group_by` keeps only one item per key.'),
        ("merge_dicts", "return {**b, **a}", '`merge_dicts` lets the first dict win instead of the second.'),
    ],
}

TESTS = {
    "add": ('from multipart.calc import add',
            '    def test_add(self):\n        self.assertEqual(add(2, 3), 5)\n        self.assertEqual(add(-1, 1), 0)\n'),
    "mean": ('from multipart.calc import mean',
             '    def test_mean(self):\n        self.assertEqual(mean([1, 2, 3]), 2.0)\n        self.assertEqual(mean([]), 0.0)\n'),
    "clamp": ('from multipart.calc import clamp',
              '    def test_clamp(self):\n        self.assertEqual(clamp(10, 0, 5), 5)\n'
              '        self.assertEqual(clamp(-1, 0, 5), 0)\n        self.assertEqual(clamp(3, 0, 5), 3)\n'),
    "slugify": ('from multipart.text import slugify',
                '    def test_slugify(self):\n        self.assertEqual(slugify("Hello World"), "hello-world")\n'
                '        self.assertEqual(slugify(" A B "), "a-b")\n'),
    "truncate": ('from multipart.text import truncate',
                 '    def test_truncate(self):\n        self.assertEqual(truncate("abcdef", 3), "abc...")\n'
                 '        self.assertEqual(truncate("abc", 3), "abc")\n'),
    "word_count": ('from multipart.text import word_count',
                   '    def test_word_count(self):\n        self.assertEqual(word_count("a b"), 2)\n'
                   '        self.assertEqual(word_count(""), 0)\n'),
    "parse_pairs": ('from multipart.data import parse_pairs',
                    '    def test_parse_pairs(self):\n'
                    '        self.assertEqual(parse_pairs("a=1, b=2"), {"a": "1", "b": "2"})\n'
                    '        self.assertEqual(parse_pairs(""), {})\n'),
    "group_by": ('from multipart.data import group_by',
                 '    def test_group_by(self):\n'
                 '        items = [{"k": "a", "v": 1}, {"k": "b", "v": 2}, {"k": "a", "v": 3}]\n'
                 '        self.assertEqual(group_by(items, "k"), {"a": [items[0], items[2]], "b": [items[1]]})\n'),
    "merge_dicts": ('from multipart.data import merge_dicts',
                    '    def test_merge_dicts(self):\n'
                    '        a = {"a": 1, "b": 2}\n'
                    '        b = {"b": 3, "c": 4}\n'
                    '        self.assertEqual(merge_dicts(a, b), {"a": 1, "b": 3, "c": 4})\n'
                    '        self.assertEqual(a, {"a": 1, "b": 2})\n'),
}


def module(name, bugs):
    text = "\n"
    for fn, body in CORRECT[name].items():
        bug = next((b for b in bugs if b[0] == fn), None)
        if bug:
            text += "def {}({}):\n    {}\n\n".format(fn, {"add": "a, b", "mean": "values", "clamp": "x, lo, hi",
                                                          "slugify": "text", "truncate": "text, n",
                                                          "word_count": "text", "parse_pairs": "text",
                                                          "group_by": "items, key", "merge_dicts": "a, b"}[fn],
                                                     bug[1])
        else:
            text += body + "\n"
    return text.strip() + "\n"


def build_task(i, combo):
    tid = "mp-{:02d}".format(i)
    bugs = []
    for name in ("calc", "text", "data"):
        fn, body, symptom = combo[name]
        bugs.append((name, fn, body, symptom))
    repo = REPOS / tid
    if repo.exists():
        shutil.rmtree(repo)
    pkg = repo / "multipart"
    pkg.mkdir(parents=True)
    (pkg / "__init__.py").write_text("")
    for name in ("calc", "text", "data"):
        (pkg / (name + ".py")).write_text(module(name, [(f, b, s) for n, f, b, s in bugs if n == name]))
    (repo / "README.md").write_text("# multipart\n\nA small Python package.\n")
    imports = []
    methods = []
    for name, fn, body, symptom in bugs:
        imports.append(TESTS[fn][0])
        methods.append(TESTS[fn][1])
    check = "import unittest\n" + "\n".join(imports) + "\n\n\nclass Hidden(unittest.TestCase):\n" + "\n".join(methods) + "\n"
    (CHECKS / (tid + ".py")).write_text(check)
    prompt = ("The package `multipart` has three independent bugs.\n\n" +
              "\n".join("{}. In `multipart/{}.py`, {}".format(n + 1, name, symptom)
                        for n, (name, fn, body, symptom) in enumerate(bugs)) +
              "\n\nFix all three. The check runs hidden tests. Do not change the package layout.")
    return {
        "id": tid,
        "prompt": prompt,
        "setup": {"repo": "tests/data/eval/multipart/repos/" + tid},
        "check": 'python3 "$PENELOPE_ROOT/cli/tests/data/eval/multipart_check.py" ' + tid,
        "check_timeout_seconds": 120,
        "max_steps": 20,
    }


def main():
    if BASE.exists():
        shutil.rmtree(BASE)
    REPOS.mkdir(parents=True)
    CHECKS.mkdir(parents=True)
    combos = []
    for c in BUGS["calc"]:
        for t in BUGS["text"]:
            for d in BUGS["data"]:
                combos.append({"calc": c, "text": t, "data": d})
    tasks = []
    for i, combo in enumerate(combos + combos[:3], 1):
        tasks.append(build_task(i, combo))
    ordered = sorted(tasks, key=lambda x: hashlib.sha256(x["id"].encode()).hexdigest())
    dev = ordered[:20]
    hold = ordered[20:]
    for name, rows in (("tasks_dev20.jsonl", dev), ("tasks_holdout10.jsonl", hold)):
        path = BASE / name
        path.write_text("".join(json.dumps(r, ensure_ascii=False) + "\n" for r in rows))
    h = hashlib.sha256((BASE / "tasks_holdout10.jsonl").read_bytes()).hexdigest()
    print("tasks", len(tasks), "dev", len(dev), "holdout", len(hold))
    print("holdout", [r["id"] for r in hold])
    print("holdout sha256", h)
    print("dev", [r["id"] for r in dev])


if __name__ == "__main__":
    main()
