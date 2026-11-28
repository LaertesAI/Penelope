import csv
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
