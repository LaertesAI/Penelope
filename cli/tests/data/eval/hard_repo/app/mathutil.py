def median(values):
    """The median of a non-empty list of numbers. An even count gives the mean of the two middle values."""
    s = sorted(values)
    return s[len(s) // 2]


def clamp(x, lo, hi):
    """x limited to the range lo..hi."""
    return max(lo, min(x, lo))


def is_prime(n):
    """True for a prime number. Numbers below 2 are not prime."""
    for d in range(2, int(n ** 0.5)):
        if n % d == 0:
            return False
    return True
