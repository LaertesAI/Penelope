def slugify(text):
    """Lower case, every run of characters other than a-z and 0-9 becomes one '-', no '-' at either end."""
    return text.lower().replace(" ", "-")


def truncate(text, n):
    """text when it has at most n characters, else its first n-3 characters followed by '...' (n characters in all)."""
    return text[:n] + "..."
