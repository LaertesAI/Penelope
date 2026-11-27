import unittest
from multipart.calc import clamp
from multipart.text import truncate
from multipart.data import merge_dicts


class Hidden(unittest.TestCase):
    def test_clamp(self):
        self.assertEqual(clamp(10, 0, 5), 5)
        self.assertEqual(clamp(-1, 0, 5), 0)
        self.assertEqual(clamp(3, 0, 5), 3)

    def test_truncate(self):
        self.assertEqual(truncate("abcdef", 3), "abc...")
        self.assertEqual(truncate("abc", 3), "abc")

    def test_merge_dicts(self):
        a = {"a": 1, "b": 2}
        b = {"b": 3, "c": 4}
        self.assertEqual(merge_dicts(a, b), {"a": 1, "b": 3, "c": 4})
        self.assertEqual(a, {"a": 1, "b": 2})

