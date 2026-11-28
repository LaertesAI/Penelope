import csv
import os

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))


def rows():
    with open(os.path.join(ROOT, "data", "sales.csv"), newline="") as f:
        reader = csv.reader(f)
        next(reader)
        next(reader)
        return [r for r in reader]
