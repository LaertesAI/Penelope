import unittest
from multipart.calc import clamp
from multipart.text import truncate
from multipart.data import group_by


class Hidden(unittest.TestCase):
    def test_clamp(self):
        self.assertEqual(clamp(10, 0, 5), 5)
        self.assertEqual(clamp(-1, 0, 5), 0)
        self.assertEqual(clamp(3, 0, 5), 3)

    def test_truncate(self):
        self.assertEqual(truncate("abcdef", 3), "abc...")
        self.assertEqual(truncate("abc", 3), "abc")

    def test_group_by(self):
        items = [{"k": "a", "v": 1}, {"k": "b", "v": 2}, {"k": "a", "v": 3}]
        self.assertEqual(group_by(items, "k"), {"a": [items[0], items[2]], "b": [items[1]]})

