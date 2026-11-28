import glob
import json
import os

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))


def items():
    paths = sorted(glob.glob(os.path.join(ROOT, "data", "inventory", "*.json")))[:10]
    return [json.load(open(p)) for p in paths]
