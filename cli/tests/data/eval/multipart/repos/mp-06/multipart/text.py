def slugify(text):
    return text.strip().lower().replace(" ", "-")

def truncate(text, n):
    return text[:n]

def word_count(text):
    return len(text.split())
