def parse_pairs(text):
    return dict(p.split(":") for p in text.split(","))

def group_by(items, key):
    return {item[key]: [item] for item in items}

def merge_dicts(a, b):
    return {**b, **a}
