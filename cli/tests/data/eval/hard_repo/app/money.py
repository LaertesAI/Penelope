def format_money(cents):
    """12345 becomes '123.45 EUR'."""
    return "%d.%02d EUR" % (cents // 100, cents % 100)
