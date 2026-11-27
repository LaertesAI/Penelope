def parse_pairs(text):
    out = {}
    for part in text.split(","):
        part = part.strip()
        if not part:
            continue
        key, value = part.split("=", 1)
        out[key.strip()] = value.strip()
    return out

def group_by(items, key):
    out = {}
    for item in items:
        out.setdefault(item[key], []).append(item)
    return out

def merge_dicts(a, b):
    return {**b, **a}
