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
    return {item[key]: [item] for item in items}

def merge_dicts(a, b):
    return {**a, **b}
