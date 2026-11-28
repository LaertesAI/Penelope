import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from read import rows  # noqa: E402
from calc import north_qty  # noqa: E402

print("north qty: %d" % north_qty(rows()))
