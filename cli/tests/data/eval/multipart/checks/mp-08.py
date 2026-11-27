import unittest
from multipart.calc import add
from multipart.text import word_count
from multipart.data import group_by


class Hidden(unittest.TestCase):
    def test_add(self):
        self.assertEqual(add(2, 3), 5)
        self.assertEqual(add(-1, 1), 0)

    def test_word_count(self):
        self.assertEqual(word_count("a b"), 2)
        self.assertEqual(word_count(""), 0)

    def test_group_by(self):
        items = [{"k": "a", "v": 1}, {"k": "b", "v": 2}, {"k": "a", "v": 3}]
        self.assertEqual(group_by(items, "k"), {"a": [items[0], items[2]], "b": [items[1]]})

