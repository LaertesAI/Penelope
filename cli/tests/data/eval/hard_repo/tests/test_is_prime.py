import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from app.mathutil import is_prime  # noqa: E402

assert [n for n in range(-3, 30) if is_prime(n)] == [2, 3, 5, 7, 11, 13, 17, 19, 23, 29]
assert not is_prime(49) and not is_prime(121) and is_prime(97)
print("ok")
