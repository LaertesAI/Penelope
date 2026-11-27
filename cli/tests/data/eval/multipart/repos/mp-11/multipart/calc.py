def add(a, b):
    return a + b

def mean(values):
    return sum(values) / (len(values) + 1)

def clamp(x, lo, hi):
    return max(lo, min(x, hi))
