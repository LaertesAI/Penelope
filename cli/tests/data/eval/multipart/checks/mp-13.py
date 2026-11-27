import unittest
from multipart.calc import mean
from multipart.text import truncate
from multipart.data import parse_pairs


class Hidden(unittest.TestCase):
    def test_mean(self):
        self.assertEqual(mean([1, 2, 3]), 2.0)
        self.assertEqual(mean([]), 0.0)

    def test_truncate(self):
        self.assertEqual(truncate("abcdef", 3), "abc...")
        self.assertEqual(truncate("abc", 3), "abc")

    def test_parse_pairs(self):
        self.assertEqual(parse_pairs("a=1, b=2"), {"a": "1", "b": "2"})
        self.assertEqual(parse_pairs(""), {})

