def slugify(text):
    return text.lower().replace(" ", "_")

def truncate(text, n):
    return text[:n]

def word_count(text):
    return len(text)
