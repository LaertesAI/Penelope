import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from read import lines  # noqa: E402
from calc import error_count  # noqa: E402

print("errors: %d" % error_count(lines()))
