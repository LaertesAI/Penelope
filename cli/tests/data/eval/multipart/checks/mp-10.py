import unittest
from multipart.calc import mean
from multipart.text import slugify
from multipart.data import parse_pairs


class Hidden(unittest.TestCase):
    def test_mean(self):
        self.assertEqual(mean([1, 2, 3]), 2.0)
        self.assertEqual(mean([]), 0.0)

    def test_slugify(self):
        self.assertEqual(slugify("Hello World"), "hello-world")
        self.assertEqual(slugify(" A B "), "a-b")

    def test_parse_pairs(self):
        self.assertEqual(parse_pairs("a=1, b=2"), {"a": "1", "b": "2"})
        self.assertEqual(parse_pairs(""), {})

