def slugify(text):
    return text.lower().replace(" ", "_")

def truncate(text, n):
    return text if len(text) <= n else text[:n] + "..."

def word_count(text):
    return len(text.split())
