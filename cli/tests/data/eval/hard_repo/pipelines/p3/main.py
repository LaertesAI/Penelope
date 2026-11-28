import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from read import words  # noqa: E402
from calc import top  # noqa: E402

for word, count in top(words()):
    print("%s %d" % (word, count))
