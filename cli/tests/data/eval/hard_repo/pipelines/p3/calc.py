def top(words, n=3):
    counts = {}
    for w in words:
        counts[w] = counts.get(w, 0) + 1
    ranked = sorted(counts.items(), key=lambda kv: (kv[1], kv[0]))
    return ranked[:n]
