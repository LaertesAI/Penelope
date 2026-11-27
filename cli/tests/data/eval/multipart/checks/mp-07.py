import unittest
from multipart.calc import add
from multipart.text import word_count
from multipart.data import parse_pairs


class Hidden(unittest.TestCase):
    def test_add(self):
        self.assertEqual(add(2, 3), 5)
        self.assertEqual(add(-1, 1), 0)

    def test_word_count(self):
        self.assertEqual(word_count("a b"), 2)
        self.assertEqual(word_count(""), 0)

    def test_parse_pairs(self):
        self.assertEqual(parse_pairs("a=1, b=2"), {"a": "1", "b": "2"})
        self.assertEqual(parse_pairs(""), {})

