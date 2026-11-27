import unittest
from multipart.calc import mean
from multipart.text import word_count
from multipart.data import parse_pairs


class Hidden(unittest.TestCase):
    def test_mean(self):
        self.assertEqual(mean([1, 2, 3]), 2.0)
        self.assertEqual(mean([]), 0.0)

    def test_word_count(self):
        self.assertEqual(word_count("a b"), 2)
        self.assertEqual(word_count(""), 0)

    def test_parse_pairs(self):
        self.assertEqual(parse_pairs("a=1, b=2"), {"a": "1", "b": "2"})
        self.assertEqual(parse_pairs(""), {})

