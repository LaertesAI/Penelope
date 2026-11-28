import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from app.textutil import slugify  # noqa: E402

assert slugify("Hello World") == "hello-world"
assert slugify("  Hello,   World!  ") == "hello-world"
assert slugify("C++ & Rust: 2026") == "c-rust-2026"
assert slugify("---") == ""
print("ok")
