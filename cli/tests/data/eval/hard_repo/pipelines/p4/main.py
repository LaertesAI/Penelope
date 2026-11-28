import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from read import items  # noqa: E402
from calc import hamburg_total  # noqa: E402

print("hamburg: %d" % hamburg_total(items()))
