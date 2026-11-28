from app.money import format_money
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
