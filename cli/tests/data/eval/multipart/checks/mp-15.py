import unittest
from multipart.calc import mean
from multipart.text import truncate
from multipart.data import merge_dicts


class Hidden(unittest.TestCase):
    def test_mean(self):
        self.assertEqual(mean([1, 2, 3]), 2.0)
        self.assertEqual(mean([]), 0.0)

    def test_truncate(self):
        self.assertEqual(truncate("abcdef", 3), "abc...")
        self.assertEqual(truncate("abc", 3), "abc")

    def test_merge_dicts(self):
        a = {"a": 1, "b": 2}
        b = {"b": 3, "c": 4}
        self.assertEqual(merge_dicts(a, b), {"a": 1, "b": 3, "c": 4})
        self.assertEqual(a, {"a": 1, "b": 2})

