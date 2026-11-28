#!/usr/bin/env python3
"""Writes hard_repo/ and the task files hard_calib.jsonl (20 tasks) and hard_holdout.jsonl (30 tasks).

Ten families of five tasks: bug fixes, renames across files, JSON edits, CSV sums, a search over 40 files, log counts,
new CLI commands, a new module with its test, pipelines with two bugs, and Markdown edits. Tasks 1 and 2 of a family
go to the calibration file, tasks 3 to 5 to the hold-out file.

Every check holds only a SHA-256 of the expected result, never the result itself: the model sees the check command.
`--verify` copies the repo for every task, runs the check on the untouched copy (it must fail), applies the reference
solution and runs the check again (it must pass).

Usage: python3 make_hard_tasks.py [--verify WORKDIR]
"""
import csv
import glob
import hashlib
import io
import json
import os
import random
import re
import shutil
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.join(HERE, "hard_repo")


def sha(text):
    return hashlib.sha256(text.encode()).hexdigest()


def norm_text(text):
    return "\n".join(line.rstrip() for line in text.strip().splitlines())


def check_text(path, expected):
    """The check: the file's lines, right-stripped, outer blank lines dropped, hash to the expected value."""
    h = sha(norm_text(expected))
    return ("python3 -c \"import hashlib,sys; t=open('%s').read(); n=chr(10).join(l.rstrip() for l in t.strip().splitlines()); "
            "sys.exit(0 if hashlib.sha256(n.encode()).hexdigest()=='%s' else 1)\"" % (path, h))


def check_json(path, expected):
    h = sha(json.dumps(expected, sort_keys=True))
    return ("python3 -c \"import hashlib,json,sys; d=json.load(open('%s')); "
            "sys.exit(0 if hashlib.sha256(json.dumps(d,sort_keys=True).encode()).hexdigest()=='%s' else 1)\"" % (path, h))


def check_output(command, expected):
    h = sha(norm_text(expected))
    return ("%s | python3 -c \"import hashlib,sys; t=sys.stdin.read(); n=chr(10).join(l.rstrip() for l in t.strip().splitlines()); "
            "sys.exit(0 if hashlib.sha256(n.encode()).hexdigest()=='%s' else 1)\"" % (command, h))


def check_file_unchanged(path, content):
    return ("python3 -c \"import hashlib,sys; sys.exit(0 if hashlib.sha256(open('%s','rb').read()).hexdigest()=='%s' else 1)\""
            % (path, hashlib.sha256(content.encode()).hexdigest()))


# ---------------------------------------------------------------- the repo

FILES = {}

FILES["README.md"] = """# shop-tools

Small tools around the shop data in data/. Run `python3 -m app.cli all` for the report.
Tests are plain scripts: `python3 tests/test_median.py`.
"""

FILES["app/__init__.py"] = '''"""shop-tools."""

VERSION = "1.4.2"
MODULES = ["store", "money", "names", "report", "export", "mathutil", "textutil"]
'''

FILES["app/store.py"] = '''import csv
import os

DATA = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "data")


def parse_qty(text):
    """A quantity cell as an int; an empty cell is 0."""
    text = text.strip()
    return int(text) if text else 0


def load_rows(path=None):
    """The rows of the sales file as dicts with region, product, qty and price_cents."""
    path = path or os.path.join(DATA, "sales.csv")
    rows = []
    with open(path, newline="") as f:
        for row in csv.DictReader(f):
            rows.append({"region": row["region"], "product": row["product"],
                         "qty": parse_qty(row["qty"]), "price_cents": int(row["price_cents"])})
    return rows
'''

FILES["app/money.py"] = '''def format_money(cents):
    """12345 becomes '123.45 EUR'."""
    return "%d.%02d EUR" % (cents // 100, cents % 100)
'''

FILES["app/names.py"] = '''def normalize_name(name):
    """' North ' becomes 'North'."""
    return name.strip().capitalize()
'''

FILES["app/report.py"] = '''from app.money import format_money
from app.names import normalize_name
from app.store import load_rows, parse_qty


def total_by_region(rows):
    """Revenue in cents per region."""
    out = {}
    for r in rows:
        out[r["region"]] = out.get(r["region"], 0) + r["qty"] * r["price_cents"]
    return out


def report_lines():
    rows = load_rows()
    lines = []
    for region, cents in sorted(total_by_region(rows).items()):
        lines.append("%s: %s" % (normalize_name(region), format_money(cents)))
    lines.append("check: %d" % parse_qty(" %d " % len(rows)))
    return lines
'''

FILES["app/export.py"] = '''from app.money import format_money
from app.names import normalize_name
from app.report import total_by_region
from app.store import load_rows


def export_lines():
    rows = load_rows()
    totals = total_by_region(rows)
    best = max(totals, key=totals.get)
    return ["best region: %s (%s)" % (normalize_name(best), format_money(totals[best])),
            "products: %d" % len({r["product"] for r in rows})]
'''

FILES["app/cli.py"] = '''import sys

from app.export import export_lines
from app.report import report_lines


def main(argv):
    if not argv or argv[0] == "all":
        for line in report_lines() + export_lines():
            print(line)
        return 0
    print("unknown command: %s" % argv[0])
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
'''

FILES["app/__main__.py"] = '''import sys

from app.cli import main

sys.exit(main(sys.argv[1:]))
'''

FILES["app/mathutil.py"] = '''def median(values):
    """The median of a non-empty list of numbers. An even count gives the mean of the two middle values."""
    s = sorted(values)
    return s[len(s) // 2]


def clamp(x, lo, hi):
    """x limited to the range lo..hi."""
    return max(lo, min(x, lo))


def is_prime(n):
    """True for a prime number. Numbers below 2 are not prime."""
    for d in range(2, int(n ** 0.5)):
        if n % d == 0:
            return False
    return True
'''

FILES["app/textutil.py"] = '''def slugify(text):
    """Lower case, every run of characters other than a-z and 0-9 becomes one '-', no '-' at either end."""
    return text.lower().replace(" ", "-")


def truncate(text, n):
    """text when it has at most n characters, else its first n-3 characters followed by '...' (n characters in all)."""
    return text[:n] + "..."
'''

TESTS = {
    "median": ("mathutil", '''assert median([3, 1, 2]) == 2
assert median([4, 1, 3, 2]) == 2.5
assert median([7]) == 7
assert median([1, 1, 5, 9, 9, 10]) == 7
'''),
    "clamp": ("mathutil", '''assert clamp(5, 0, 10) == 5
assert clamp(-3, 0, 10) == 0
assert clamp(42, 0, 10) == 10
assert clamp(2.5, 1, 2) == 2
'''),
    "is_prime": ("mathutil", '''assert [n for n in range(-3, 30) if is_prime(n)] == [2, 3, 5, 7, 11, 13, 17, 19, 23, 29]
assert not is_prime(49) and not is_prime(121) and is_prime(97)
'''),
    "slugify": ("textutil", '''assert slugify("Hello World") == "hello-world"
assert slugify("  Hello,   World!  ") == "hello-world"
assert slugify("C++ & Rust: 2026") == "c-rust-2026"
assert slugify("---") == ""
'''),
    "truncate": ("textutil", '''assert truncate("short", 10) == "short"
assert truncate("exactly10!", 10) == "exactly10!"
assert truncate("this is far too long", 10) == "this is..."
assert len(truncate("abcdefghijklmnop", 8)) == 8
'''),
}
for name, (mod, body) in TESTS.items():
    FILES["tests/test_%s.py" % name] = ('import os\nimport sys\n\nsys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))\n\n'
                                        'from app.%s import %s  # noqa: E402\n\n%sprint("ok")\n' % (mod, name, body))

FIXES = {
    "median": ("app/mathutil.py", '''    s = sorted(values)
    return s[len(s) // 2]''', '''    s = sorted(values)
    m = len(s) // 2
    return s[m] if len(s) % 2 else (s[m - 1] + s[m]) / 2'''),
    "clamp": ("app/mathutil.py", "return max(lo, min(x, lo))", "return max(lo, min(x, hi))"),
    "is_prime": ("app/mathutil.py", '''    for d in range(2, int(n ** 0.5)):''', '''    if n < 2:
        return False
    for d in range(2, int(n ** 0.5) + 1):'''),
    "slugify": ("app/textutil.py", '''    return text.lower().replace(" ", "-")''', '''    import re
    return re.sub(r"[^a-z0-9]+", "-", text.lower()).strip("-")'''),
    "truncate": ("app/textutil.py", '''    return text[:n] + "..."''', '''    return text if len(text) <= n else text[:n - 3] + "..."'''),
}

rng = random.Random(20261127)
REGIONS = ["north", "south", "east", "west"]
PRODUCTS = ["apple", "bread", "cheese", "dates", "eggs", "flour"]
sales = io.StringIO()
w = csv.writer(sales, lineterminator="\n")
w.writerow(["region", "product", "qty", "price_cents"])
SALES = []
for i in range(36):
    region = REGIONS[i % 4] if i < 32 else "north"
    product = rng.choice(PRODUCTS[:-1]) if region != "west" else rng.choice(PRODUCTS[:3])
    qty = "" if i in (7, 22) else str(rng.randint(1, 40))
    price = rng.randint(50, 900)
    w.writerow([region, product, qty, price])
    SALES.append({"region": region, "product": product, "qty": int(qty) if qty else 0, "price_cents": price})
FILES["data/sales.csv"] = sales.getvalue()
FILES["data/numbers.txt"] = "\n".join("%.2f" % (rng.randint(100, 9999) / 100) for _ in range(25)) + "\n"

WAREHOUSES = ["berlin", "hamburg", "munich"]
TAGS = ["fragile", "cold", "bulk", "promo"]
INVENTORY = []
for i in range(40):
    sku = "SKU-%d" % (1001 + i)
    if i == 33:
        sku = "SKU-1018"          # the one duplicate
    wh = WAREHOUSES[rng.randrange(3)]
    count = 0 if i in (3, 11, 17, 26, 30, 38) else rng.randint(1, 60)
    tags = sorted(rng.sample(TAGS, rng.randint(0, 2)))
    item = {"sku": sku, "warehouse": wh, "count": count, "tags": tags}
    INVENTORY.append(item)
    FILES["data/inventory/item-%03d.json" % (i + 1)] = json.dumps(item, indent=2) + "\n"

SERVICES = ["auth", "billing", "search", "mail"]
LOG_LINES = {k: [] for k in range(1, 5)}
ALL_LOGS = []
for k in range(1, 5):
    for j in range(45):
        day = rng.randint(1, 4)
        hour = rng.choice([8, 9, 9, 10, 13, 14, 14, 14, 17, 22])
        stamp = "2026-11-%02d %02d:%02d:%02d" % (day, hour, rng.randint(0, 59), rng.randint(0, 59))
        level = rng.choice(["INFO", "INFO", "INFO", "WARN", "ERROR"])
        svc = rng.choice(SERVICES)
        if svc == "auth" and rng.random() < 0.4:
            msg = "login failed for user=u%03d" % rng.randint(1, 30)
        else:
            msg = rng.choice(["request done", "cache miss", "retrying upstream", "queue full", "timeout after 30s",
                              "invoice %d stored" % rng.randint(100, 999), "disk at %d%%" % rng.randint(50, 99)])
        LOG_LINES[k].append((stamp, level, svc, msg))
    LOG_LINES[k].sort()
    for e in LOG_LINES[k]:
        ALL_LOGS.append(e)
    FILES["logs/app-%d.log" % k] = "".join("%s %s %s: %s\n" % e for e in LOG_LINES[k])

FILES["config/settings.json"] = json.dumps({
    "server": {"host": "127.0.0.1", "port": 8080, "workers": 4},
    "features": ["search", "export"],
    "debug": {"trace": True, "level": "info"},
    "storage": {"path": "data/", "retention_days": 30},
    "users": [{"name": "ana", "role": "admin"}, {"name": "ben", "role": "viewer"}],
}, indent=2) + "\n"

FILES["docs/guide.md"] = """# Guide

The colour of a status badge shows the state of a job. Version 1.4.2 added the grey colour.

## Install

Copy the folder and run the report:

```sh
# the colour flag is a real flag of the tool, keep it
python3 -m app.cli all --colour never
```

## Colours

Pick a colour per region. The colour table lives in config/settings.json.

```json
{"colour": "red"}
```

## Report format

Each line holds a region and its revenue. The colour of the line follows the region.
"""

FILES["docs/style.md"] = """# Style

Rules for the text in this project, version 1.4.2.

## Steps

1. Write the change.
3. Run the tests.
3. Update the guide.
7. Ask for a review.
2. Merge.

## Words

- zebra crossing
- Apple
- mango
- banana
- Cherry
- kiwi

## Tone

Short sentences. One idea per sentence.
"""

FILES["CHANGELOG.md"] = """# Changelog

## 1.4.2

- grey colour for jobs

## 1.4.1

- first public version, released after 1.4.0
"""

# Pipelines with two bugs each.
PIPES = {}
PIPES["p1"] = {
    "read.py": '''import csv
import os

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))


def rows():
    with open(os.path.join(ROOT, "data", "sales.csv"), newline="") as f:
        reader = csv.reader(f)
        next(reader)
        next(reader)
        return [r for r in reader]
''',
    "calc.py": '''def north_qty(rows):
    total = 0
    for region, product, qty, price in rows:
        if region == "north" and qty:
            total += int(price)
    return total
''',
    "main.py": '''import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from read import rows  # noqa: E402
from calc import north_qty  # noqa: E402

print("north qty: %d" % north_qty(rows()))
''',
}
PIPES["p2"] = {
    "read.py": '''import os

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))


def numbers():
    with open(os.path.join(ROOT, "data", "numbers.txt")) as f:
        return [int(float(line)) for line in f if line.strip()]
''',
    "calc.py": '''def mean(values):
    return sum(values) / (len(values) + 1)
''',
    "main.py": '''import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from read import numbers  # noqa: E402
from calc import mean  # noqa: E402

print("mean: %.3f" % mean(numbers()))
''',
}
PIPES["p3"] = {
    "read.py": '''import os
import re

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))


def words():
    with open(os.path.join(ROOT, "docs", "guide.md")) as f:
        return re.findall(r"[A-Za-z]+", f.read())
''',
    "calc.py": '''def top(words, n=3):
    counts = {}
    for w in words:
        counts[w] = counts.get(w, 0) + 1
    ranked = sorted(counts.items(), key=lambda kv: (kv[1], kv[0]))
    return ranked[:n]
''',
    "main.py": '''import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from read import words  # noqa: E402
from calc import top  # noqa: E402

for word, count in top(words()):
    print("%s %d" % (word, count))
''',
}
PIPES["p4"] = {
    "read.py": '''import glob
import json
import os

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))


def items():
    paths = sorted(glob.glob(os.path.join(ROOT, "data", "inventory", "*.json")))[:10]
    return [json.load(open(p)) for p in paths]
''',
    "calc.py": '''def hamburg_total(items):
    return sum(i["count"] for i in items if i["warehouse"] == "Hamburg")
''',
    "main.py": '''import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from read import items  # noqa: E402
from calc import hamburg_total  # noqa: E402

print("hamburg: %d" % hamburg_total(items()))
''',
}
PIPES["p5"] = {
    "read.py": '''import os

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))


def lines():
    out = []
    for name in ["app-1.log"]:
        with open(os.path.join(ROOT, "logs", name)) as f:
            out.extend(f.read().splitlines())
    return out
''',
    "calc.py": '''def error_count(lines):
    return sum(1 for line in lines if line.split()[2] == "WARN")
''',
    "main.py": '''import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from read import lines  # noqa: E402
from calc import error_count  # noqa: E402

print("errors: %d" % error_count(lines()))
''',
}
PIPE_FIXES = {
    "p1": [("read.py", "        next(reader)\n        next(reader)\n", "        next(reader)\n"), ("calc.py", "total += int(price)", "total += int(qty)")],
    "p2": [("read.py", "int(float(line))", "float(line)"), ("calc.py", "(len(values) + 1)", "len(values)")],
    "p3": [("read.py", "f.read())", "f.read().lower())"), ("calc.py", "key=lambda kv: (kv[1], kv[0])", "key=lambda kv: (-kv[1], kv[0])")],
    "p4": [("read.py", "))[:10]", "))"), ("calc.py", '"Hamburg"', '"hamburg"')],
    "p5": [("read.py", '["app-1.log"]', '["app-1.log", "app-2.log", "app-3.log", "app-4.log"]'), ("calc.py", '"WARN"', '"ERROR"')],
}
for p, files in PIPES.items():
    for name, text in files.items():
        FILES["pipelines/%s/%s" % (p, name)] = text


def write_repo(dest):
    if os.path.exists(dest):
        shutil.rmtree(dest)
    for rel, text in FILES.items():
        path = os.path.join(dest, rel)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w") as f:
            f.write(text)


# ---------------------------------------------------------------- reference values

def cli_all_output():
    totals = {}
    for r in SALES:
        totals[r["region"]] = totals.get(r["region"], 0) + r["qty"] * r["price_cents"]
    money = lambda c: "%d.%02d EUR" % (c // 100, c % 100)
    lines = ["%s: %s" % (k.capitalize(), money(v)) for k, v in sorted(totals.items())]
    lines.append("check: %d" % len(SALES))
    best = max(totals, key=totals.get)
    lines += ["best region: %s (%s)" % (best.capitalize(), money(totals[best])), "products: %d" % len({r["product"] for r in SALES})]
    return "\n".join(lines)


def revenue():
    t = {}
    for r in SALES:
        t[r["region"]] = t.get(r["region"], 0) + r["qty"] * r["price_cents"]
    return t


def qty_by_product():
    t = {}
    for r in SALES:
        t[r["product"]] = t.get(r["product"], 0) + r["qty"]
    return t


# ---------------------------------------------------------------- tasks

TASKS = []  # (family, index, task dict, solve function(dir))


def task(family, idx, tid, prompt, check, solve, steps=12):
    TASKS.append((family, idx, {"id": tid, "prompt": prompt, "setup": {"repo": "hard_repo", "cwd": "", "copy": True},
                                "check": check, "check_timeout_seconds": 60, "max_steps": steps}, solve))


def edit(d, rel, old, new):
    p = os.path.join(d, rel)
    t = open(p).read()
    assert old in t, (rel, old)
    open(p, "w").write(t.replace(old, new))


def put(d, rel, text):
    p = os.path.join(d, rel)
    os.makedirs(os.path.dirname(p), exist_ok=True)
    open(p, "w").write(text)


# F1 bug fixes
for i, name in enumerate(["median", "clamp", "is_prime", "slugify", "truncate"], 1):
    test = "tests/test_%s.py" % name
    mod = FIXES[name][0]
    task("bugfix", i, "hard-fix-%s" % name.replace("_", "-"),
         "The test %s fails. Fix the function %s in %s so that `python3 %s` passes. Do not change any file under tests/." % (test, name, mod, test),
         "python3 %s && %s" % (test, check_file_unchanged(test, FILES[test])),
         lambda d, n=name: edit(d, FIXES[n][0], FIXES[n][1], FIXES[n][2]))

# F2 renames across files
RENAMES = [("load_rows", "read_rows", "app/store.py"), ("format_money", "format_eur", "app/money.py"),
           ("total_by_region", "region_totals", "app/report.py"), ("normalize_name", "clean_name", "app/names.py"),
           ("parse_qty", "to_quantity", "app/store.py")]
ALL_OUT = cli_all_output()
for i, (old, new, home) in enumerate(RENAMES, 1):
    def solve(d, old=old, new=new):
        for p in glob.glob(os.path.join(d, "app", "*.py")):
            t = open(p).read()
            open(p, "w").write(re.sub(r"\b%s\b" % old, new, t))
    task("rename", i, "hard-rename-%s" % old.replace("_", "-"),
         "Rename the function %s (defined in %s) to %s in every file under app/, including every import and call. "
         "The output of `python3 -m app.cli all` must stay exactly the same." % (old, home, new),
         "! grep -rnw --include=*.py %s app && grep -q 'def %s(' %s && %s" % (old, new, home, check_output("python3 -m app.cli all", ALL_OUT)),
         solve)

# F3 JSON edits
BASE_SETTINGS = json.loads(FILES["config/settings.json"])


def settings_variant(i):
    s = json.loads(json.dumps(BASE_SETTINGS))
    if i == 1:
        s["server"]["port"] = 8081
        s["features"].append("metrics")
        del s["debug"]["trace"]
    elif i == 2:
        s["server"]["workers"] = 8
        s["storage"]["retention_days"] = 90
        s["users"][1]["role"] = "editor"
    elif i == 3:
        s["users"].append({"name": "cara", "role": "viewer"})
        s["debug"]["level"] = "warn"
        del s["storage"]["path"]
    elif i == 4:
        s["server"]["host"] = "0.0.0.0"
        s["features"].remove("export")
        s["server"]["tls"] = {"enabled": True, "cert": "certs/server.pem"}
    elif i == 5:
        s["storage"]["keep_days"] = s["storage"].pop("retention_days")
        s["features"] = sorted(s["features"] + ["audit"])
        s["debug"] = {"level": "error"}
    return s


JSON_PROMPTS = {
    1: "set server.port to 8081, append \"metrics\" at the end of the features list, and remove the key trace from debug",
    2: "set server.workers to 8, set storage.retention_days to 90, and change the role of the user ben to \"editor\"",
    3: "append the user {\"name\": \"cara\", \"role\": \"viewer\"} at the end of users, set debug.level to \"warn\", and remove the key path from storage",
    4: "set server.host to \"0.0.0.0\", remove \"export\" from the features list, and add server.tls as {\"enabled\": true, \"cert\": \"certs/server.pem\"}",
    5: "rename the key storage.retention_days to storage.keep_days with the same value, add \"audit\" to features and sort the features list alphabetically, "
       "and replace the whole debug object with {\"level\": \"error\"}",
}
for i in range(1, 6):
    exp = settings_variant(i)
    task("json", i, "hard-json-%d" % i,
         "Edit config/settings.json: %s. Keep every other key and value as it is. The file must stay valid JSON." % JSON_PROMPTS[i],
         check_json("config/settings.json", exp),
         lambda d, e=exp: put(d, "config/settings.json", json.dumps(e, indent=2) + "\n"))

# F4 CSV sums
rev = revenue()
qp = qty_by_product()
CSV_TASKS = [
    ("revenue", "the revenue per region in cents (qty times price_cents, an empty qty counts as 0) as a JSON object {region: cents}", rev),
    ("qty", "the total qty per product (an empty qty counts as 0) as a JSON object {product: qty}", qp),
    ("best", "the region with the highest revenue (qty times price_cents, an empty qty counts as 0) as {\"region\": NAME, \"revenue\": CENTS}",
     {"region": max(rev, key=rev.get), "revenue": max(rev.values())}),
    ("avgprice", "the mean price_cents per product over its rows, rounded to 2 decimals, as a JSON object {product: mean}",
     {p: round(sum(r["price_cents"] for r in SALES if r["product"] == p) / len([r for r in SALES if r["product"] == p]), 2)
      for p in sorted({r["product"] for r in SALES})}),
    ("distinct", "the number of distinct products per region as a JSON object {region: count}",
     {g: len({r["product"] for r in SALES if r["region"] == g}) for g in REGIONS}),
]
assert sorted(rev.values())[-1] != sorted(rev.values())[-2]
for i, (tid, what, exp) in enumerate(CSV_TASKS, 1):
    task("csv", i, "hard-csv-%s" % tid,
         "Read data/sales.csv and write out/result.json with %s. Create the folder out if it is missing." % what,
         check_json("out/result.json", exp),
         lambda d, e=exp: put(d, "out/result.json", json.dumps(e)))

# F5 search over 40 inventory files
low = sorted(i["sku"] for i in INVENTORY if i["warehouse"] == "berlin" and i["count"] < 10)
totals = {w_: sum(i["count"] for i in INVENTORY if i["warehouse"] == w_) for w_ in WAREHOUSES}
fragile = sorted(i["sku"] for i in INVENTORY if "fragile" in i["tags"] and i["count"] >= 20)
zeros = {w_: sum(1 for i in INVENTORY if i["warehouse"] == w_ and i["count"] == 0) for w_ in WAREHOUSES}
zs = sorted(zeros.values())
assert zs[-1] != zs[-2], zeros
dup = "SKU-1018"
INV_TASKS = [
    ("low", "out/low.txt", "the SKUs stored in the warehouse berlin with a count below 10, sorted, one per line", "\n".join(low), "text"),
    ("totals", "out/totals.json", "the total count per warehouse as a JSON object {warehouse: total}", totals, "json"),
    ("fragile", "out/fragile.txt", "the SKUs tagged fragile with a count of at least 20, sorted, one per line", "\n".join(fragile), "text"),
    ("zeros", "out/answer.txt", "the name of the warehouse that holds the most items with count 0, as one word", max(zeros, key=zeros.get), "text"),
    ("dup", "out/dup.txt", "the SKU that appears in two different files, as one line", dup, "text"),
]
for i, (tid, path, what, exp, kind) in enumerate(INV_TASKS, 1):
    chk = check_json(path, exp) if kind == "json" else check_text(path, exp)
    task("inventory", i, "hard-inv-%s" % tid,
         "The folder data/inventory holds 40 JSON files, one item each (sku, warehouse, count, tags). Write %s with %s. Create the folder out if it is missing." % (path, what),
         chk, lambda d, p=path, e=exp, k=kind: put(d, p, json.dumps(e) if k == "json" else e + "\n"))

# F6 logs
err = {}
for (stamp, level, svc, msg) in ALL_LOGS:
    if level == "ERROR":
        err[svc] = err.get(svc, 0) + 1
err_lines = "\n".join("%s %d" % kv for kv in sorted(err.items(), key=lambda kv: (-kv[1], kv[0])))
warn = {}
for (stamp, level, svc, msg) in ALL_LOGS:
    if level == "WARN":
        warn[stamp[:10]] = warn.get(stamp[:10], 0) + 1
users = sorted({re.search(r"user=(u\d+)", m).group(1) for (_, _, s, m) in ALL_LOGS if "login failed" in m})
hours = {}
for (stamp, _, _, _) in ALL_LOGS:
    hours[stamp[11:13]] = hours.get(stamp[11:13], 0) + 1
hs = sorted(hours.values())
assert hs[-1] != hs[-2], hours
first_billing = min((e for e in ALL_LOGS if e[1] == "ERROR" and e[2] == "billing"), key=lambda e: e[0])[3]
LOG_TASKS = [
    ("errors", "out/errors.txt", "the number of ERROR lines per service over all four files, one line `SERVICE COUNT` per service, sorted by count descending and then by name", err_lines, "text"),
    ("warn", "out/warn.json", "the number of WARN lines per day over all four files as a JSON object {\"YYYY-MM-DD\": count}", warn, "json"),
    ("users", "out/users.txt", "every distinct user id (like u007) that appears in a `login failed` line, sorted, one per line", "\n".join(users), "text"),
    ("hour", "out/hour.txt", "the hour of the day (two digits, 00 to 23) with the most log lines over all four files, as one line", max(hours, key=hours.get), "text"),
    ("first", "out/first.txt", "the message text (everything after `billing: `) of the earliest ERROR line of the service billing over all four files, as one line", first_billing, "text"),
]
for i, (tid, path, what, exp, kind) in enumerate(LOG_TASKS, 1):
    chk = check_json(path, exp) if kind == "json" else check_text(path, exp)
    task("logs", i, "hard-log-%s" % tid,
         "The folder logs holds four files app-1.log to app-4.log; each line is `YYYY-MM-DD HH:MM:SS LEVEL SERVICE: MESSAGE`. Write %s with %s. Create the folder out if it is missing." % (path, what),
         chk, lambda d, p=path, e=exp, k=kind: put(d, p, json.dumps(e) if k == "json" else e + "\n"))

# F7 new CLI commands
regions_out = "\n".join(sorted({r["region"] for r in SALES}))
top3 = "\n".join("%s %d" % kv for kv in sorted(qp.items(), key=lambda kv: (-kv[1], kv[0]))[:3])
rev_east = "%d.%02d EUR" % (rev["east"] // 100, rev["east"] % 100)
minq = "\n".join(sorted(p for p, q in qp.items() if q >= 100))
CLI_TASKS = [
    ("regions", "regions", "prints the distinct regions of data/sales.csv, sorted, one per line", "regions", regions_out,
     '''    if argv[0] == "regions":
        from app.store import load_rows
        for r in sorted({x["region"] for x in load_rows()}):
            print(r)
        return 0
'''),
    ("top", "top", "takes a number N and prints the N products with the highest total qty, one line `PRODUCT QTY` each, highest first, ties by name",
     "top 3", top3,
     '''    if argv[0] == "top":
        from app.store import load_rows
        t = {}
        for x in load_rows():
            t[x["product"]] = t.get(x["product"], 0) + x["qty"]
        for p, q in sorted(t.items(), key=lambda kv: (-kv[1], kv[0]))[:int(argv[1])]:
            print("%s %d" % (p, q))
        return 0
'''),
    ("revenue", "revenue", "takes a region name and prints its revenue formatted with app.money.format_money (qty times price_cents summed)",
     "revenue east", rev_east,
     '''    if argv[0] == "revenue":
        from app.money import format_money
        from app.report import total_by_region
        from app.store import load_rows
        print(format_money(total_by_region(load_rows()).get(argv[1], 0)))
        return 0
'''),
    ("count", "count", "prints the number of data rows in data/sales.csv (without the header) and nothing else", "count", str(len(SALES)),
     '''    if argv[0] == "count":
        from app.store import load_rows
        print(len(load_rows()))
        return 0
'''),
    ("products", "products", "takes the option --min-qty N and prints the products whose total qty is at least N, sorted, one per line",
     "products --min-qty 100", minq,
     '''    if argv[0] == "products":
        from app.store import load_rows
        n = int(argv[argv.index("--min-qty") + 1])
        t = {}
        for x in load_rows():
            t[x["product"]] = t.get(x["product"], 0) + x["qty"]
        for p in sorted(p for p, q in t.items() if q >= n):
            print(p)
        return 0
'''),
]
assert minq and len(minq.splitlines()) < len(qp)
for i, (tid, sub, what, call, exp, code) in enumerate(CLI_TASKS, 1):
    task("cli", i, "hard-cli-%s" % tid,
         "Add a command `%s` to app/cli.py: `python3 -m app.cli %s ...` %s. Keep the command all unchanged." % (sub, sub, what),
         "%s && %s" % (check_output("python3 -m app.cli %s" % call, exp), check_output("python3 -m app.cli all", ALL_OUT)),
         lambda d, c=code: edit(d, "app/cli.py", '    print("unknown command', c + '    print("unknown command'))

# F8 new module with test and registry entry
MODS = [
    ("chunks", "chunk(seq, n)", "returns the list split into lists of n items; the last list may be shorter",
     "from app.chunks import chunk; assert chunk([1,2,3,4,5],2)==[[1,2],[3,4],[5]] and chunk([],3)==[] and chunk('abcd',4)==[['a','b','c','d']]",
     "def chunk(seq, n):\n    seq = list(seq)\n    return [seq[i:i + n] for i in range(0, len(seq), n)]\n"),
    ("flat", "flatten(nested)", "returns a flat list of all items of nested lists at any depth, in order",
     "from app.flat import flatten; assert flatten([1,[2,[3,[4]]],5])==[1,2,3,4,5] and flatten([])==[] and flatten([[[]]])==[]",
     "def flatten(nested):\n    out = []\n    for x in nested:\n        out.extend(flatten(x) if isinstance(x, list) else [x])\n    return out\n"),
    ("dedupe", "dedupe(seq)", "returns a list of the items without repeats, keeping the first occurrence of each, in order",
     "from app.dedupe import dedupe; assert dedupe([3,1,3,2,1])==[3,1,2] and dedupe('abca')==['a','b','c'] and dedupe([])==[]",
     "def dedupe(seq):\n    seen = []\n    for x in seq:\n        if x not in seen:\n            seen.append(x)\n    return seen\n"),
    ("freq", "word_freq(text)", "returns a dict that maps each lower-case word (runs of letters a-z) to its count",
     "from app.freq import word_freq; assert word_freq('The cat, the HAT.')=={'the':2,'cat':1,'hat':1} and word_freq('')=={} and word_freq('a1a')=={'a':2}",
     "import re\n\n\ndef word_freq(text):\n    out = {}\n    for w in re.findall(r'[a-z]+', text.lower()):\n        out[w] = out.get(w, 0) + 1\n    return out\n"),
    ("rot", "rot13(text)", "returns the text with every ASCII letter rotated by 13 places, case kept, other characters unchanged",
     "from app.rot import rot13; assert rot13('Hello, World!')=='Uryyb, Jbeyq!' and rot13(rot13('abcXYZ'))=='abcXYZ' and rot13('123')=='123'",
     "def rot13(text):\n    import codecs\n    return codecs.encode(text, 'rot13')\n"),
]
for i, (mod, sig, what, asserts, code) in enumerate(MODS, 1):
    test = "tests/test_%s.py" % mod
    chk = ("python3 -c \"%s\" && test -f %s && grep -q assert %s && python3 %s && "
           "python3 -c \"import app,sys; m=app.MODULES; sys.exit(0 if '%s' in m and m[:7]==['store','money','names','report','export','mathutil','textutil'] else 1)\""
           % (asserts, test, test, test, mod))

    def solve(d, mod=mod, code=code, asserts=asserts, test=test):
        put(d, "app/%s.py" % mod, code)
        put(d, test, "import os\nimport sys\n\nsys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))\n%s\n" % asserts.replace("; assert", "\nassert"))
        edit(d, "app/__init__.py", '"textutil"]', '"textutil", "%s"]' % mod)
    task("module", i, "hard-mod-%s" % mod,
         "Create the module app/%s.py with a function %s that %s. Add a test script %s with assert statements that passes with `python3 %s` "
         "(the other tests show how to import from app). Add \"%s\" at the end of the list MODULES in app/__init__.py." % (mod, sig, what, test, test, mod),
         chk, solve)

# F9 pipelines with two bugs


def run_fixed_pipe(p):
    tmp = os.path.join(os.environ.get("HARD_TMP", HERE), ".pipe-%s" % p)
    if os.path.exists(tmp):
        shutil.rmtree(tmp)
    write_repo(tmp)
    for rel, old, new in PIPE_FIXES[p]:
        edit(tmp, "pipelines/%s/%s" % (p, rel), old, new)
    out = subprocess.run([sys.executable, os.path.join(tmp, "pipelines", p, "main.py")], capture_output=True, text=True, check=True).stdout
    shutil.rmtree(tmp)
    return out.strip()


PIPE_WHAT = {"p1": "the total qty of the region north in data/sales.csv (rows with an empty qty count as 0)",
             "p2": "the mean of all numbers in data/numbers.txt with three decimals",
             "p3": "the three most frequent words of docs/guide.md, lower case, by count descending and then by word",
             "p4": "the total count of all items of the warehouse hamburg in data/inventory",
             "p5": "the number of ERROR lines over all four files in logs/"}
for i, p in enumerate(["p1", "p2", "p3", "p4", "p5"], 1):
    exp = run_fixed_pipe(p)
    main = "pipelines/%s/main.py" % p
    task("pipeline", i, "hard-pipe-%s" % p,
         "`python3 %s` should print %s, but its output is wrong. The bugs are in the other files of pipelines/%s. Fix them; do not change main.py." % (main, PIPE_WHAT[p], p),
         "%s && %s" % (check_output("python3 %s" % main, exp), check_file_unchanged(main, FILES[main])),
         lambda d, p=p: [edit(d, "pipelines/%s/%s" % (p, rel), old, new) for rel, old, new in PIPE_FIXES[p]])

# F10 Markdown edits


def outside_code(text, fn):
    out, code = [], False
    for line in text.split("\n"):
        if line.startswith("```"):
            code = not code
            out.append(line)
        else:
            out.append(line if code else fn(line))
    return "\n".join(out)


guide = FILES["docs/guide.md"]
style = FILES["docs/style.md"]
guide_color = outside_code(guide, lambda l: l.replace("colour", "color").replace("Colour", "Color"))
style_steps = style.replace("1. Write the change.\n3. Run the tests.\n3. Update the guide.\n7. Ask for a review.\n2. Merge.",
                            "1. Write the change.\n2. Run the tests.\n3. Update the guide.\n4. Ask for a review.\n5. Merge.")
heads = [l[3:] for l in guide.split("\n") if l.startswith("## ")]
anchor = lambda h: re.sub(r"[^a-z0-9-]", "", h.lower().replace(" ", "-"))
toc = "\n".join("- [%s](#%s)" % (h, anchor(h)) for h in heads)
guide_toc = guide.replace("# Guide\n\n", "# Guide\n\n%s\n\n" % toc, 1)
words = ["zebra crossing", "Apple", "mango", "banana", "Cherry", "kiwi"]
style_words = style.replace("\n".join("- " + w_ for w_ in words), "\n".join("- " + w_ for w_ in sorted(words, key=str.lower)))
DOC_TASKS = [
    ("colour", "In docs/guide.md, replace every colour with color and every Colour with Color, but only outside the fenced code blocks; the code blocks stay as they are.",
     {"docs/guide.md": guide_color}),
    ("steps", "The numbered list under `## Steps` in docs/style.md has wrong numbers. Renumber it 1, 2, 3, ... in its current order. Change nothing else.",
     {"docs/style.md": style_steps}),
    ("toc", "Add a table of contents to docs/guide.md: after the line `# Guide` and its blank line, insert one bullet `- [HEADING](#ANCHOR)` per `## ` heading in order, "
            "then one blank line. ANCHOR is the heading in lower case with spaces replaced by - and every character other than a-z, 0-9 and - removed.",
     {"docs/guide.md": guide_toc}),
    ("version", "Change the version 1.4.2 to 1.5.0 in app/__init__.py, docs/guide.md and docs/style.md. CHANGELOG.md keeps its history: do not change it.",
     {"app/__init__.py": FILES["app/__init__.py"].replace("1.4.2", "1.5.0"), "docs/guide.md": guide.replace("1.4.2", "1.5.0"),
      "docs/style.md": style.replace("1.4.2", "1.5.0"), "CHANGELOG.md": FILES["CHANGELOG.md"]}),
    ("words", "Sort the bullet list under `## Words` in docs/style.md alphabetically, ignoring case, keeping each item's text as it is. Change nothing else.",
     {"docs/style.md": style_words}),
]
for i, (tid, prompt, files) in enumerate(DOC_TASKS, 1):
    chk = " && ".join(check_text(p, t) for p, t in sorted(files.items()))
    task("docs", i, "hard-doc-%s" % tid, prompt, chk, lambda d, fs=files: [put(d, p, t) for p, t in fs.items()])


def main():
    write_repo(REPO)
    calib = [t for (f, i, t, s) in TASKS if i <= 2]
    hold = [t for (f, i, t, s) in TASKS if i > 2]
    assert len(calib) == 20 and len(hold) == 30, (len(calib), len(hold))
    for name, items in (("hard_calib.jsonl", calib), ("hard_holdout.jsonl", hold)):
        with open(os.path.join(HERE, name), "w") as f:
            for t in items:
                f.write(json.dumps(t) + "\n")
    if "--verify" in sys.argv:
        work = sys.argv[sys.argv.index("--verify") + 1]
        bad = 0
        for (fam, i, t, solve) in TASKS:
            d = os.path.join(work, t["id"])
            if os.path.exists(d):
                shutil.rmtree(d)
            shutil.copytree(REPO, d)
            before = subprocess.run(["sh", "-c", t["check"]], cwd=d, capture_output=True).returncode
            solve(d)
            after = subprocess.run(["sh", "-c", t["check"]], cwd=d, capture_output=True, text=True)
            ok = before != 0 and after.returncode == 0
            bad += not ok
            print("%-24s untouched %s, solved %s %s" % (t["id"], "fail" if before else "PASS?", "pass" if after.returncode == 0 else "FAIL", "" if ok else "<-- BAD " + after.stderr[-300:]))
            shutil.rmtree(d)
        print("%d of %d tasks verified" % (len(TASKS) - bad, len(TASKS)))
        sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
