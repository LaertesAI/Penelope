def parse_pairs(text):
    return dict(p.split(":") for p in text.split(","))

def group_by(items, key):
    out = {}
    for item in items:
        out.setdefault(item[key], []).append(item)
    return out

def merge_dicts(a, b):
    return {**a, **b}
