from app.money import format_money
from app.names import normalize_name
from app.report import total_by_region
from app.store import load_rows


def export_lines():
    rows = load_rows()
    totals = total_by_region(rows)
    best = max(totals, key=totals.get)
    return ["best region: %s (%s)" % (normalize_name(best), format_money(totals[best])),
            "products: %d" % len({r["product"] for r in rows})]
