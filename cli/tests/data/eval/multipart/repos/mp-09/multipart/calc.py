def add(a, b):
    return a - b

def mean(values):
    if not values:
        return 0.0
    return sum(values) / len(values)

def clamp(x, lo, hi):
    return max(lo, min(x, hi))
