import os

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))


def lines():
    out = []
    for name in ["app-1.log"]:
        with open(os.path.join(ROOT, "logs", name)) as f:
            out.extend(f.read().splitlines())
    return out
