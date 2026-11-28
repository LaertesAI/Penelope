import os
import re

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))


def words():
    with open(os.path.join(ROOT, "docs", "guide.md")) as f:
        return re.findall(r"[A-Za-z]+", f.read())
