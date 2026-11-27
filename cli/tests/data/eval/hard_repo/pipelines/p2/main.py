import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from read import numbers  # noqa: E402
from calc import mean  # noqa: E402

print("mean: %.3f" % mean(numbers()))
