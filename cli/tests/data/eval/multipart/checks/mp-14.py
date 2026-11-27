import unittest
from multipart.calc import mean
from multipart.text import truncate
from multipart.data import group_by


class Hidden(unittest.TestCase):
    def test_mean(self):
        self.assertEqual(mean([1, 2, 3]), 2.0)
        self.assertEqual(mean([]), 0.0)

    def test_truncate(self):
        self.assertEqual(truncate("abcdef", 3), "abc...")
        self.assertEqual(truncate("abc", 3), "abc")

    def test_group_by(self):
        items = [{"k": "a", "v": 1}, {"k": "b", "v": 2}, {"k": "a", "v": 3}]
        self.assertEqual(group_by(items, "k"), {"a": [items[0], items[2]], "b": [items[1]]})

