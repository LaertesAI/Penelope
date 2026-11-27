import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from app.textutil import truncate  # noqa: E402

assert truncate("short", 10) == "short"
assert truncate("exactly10!", 10) == "exactly10!"
assert truncate("this is far too long", 10) == "this is..."
assert len(truncate("abcdefghijklmnop", 8)) == 8
print("ok")
