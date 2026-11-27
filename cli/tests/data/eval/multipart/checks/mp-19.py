import unittest
from multipart.calc import clamp
from multipart.text import slugify
from multipart.data import parse_pairs


class Hidden(unittest.TestCase):
    def test_clamp(self):
        self.assertEqual(clamp(10, 0, 5), 5)
        self.assertEqual(clamp(-1, 0, 5), 0)
        self.assertEqual(clamp(3, 0, 5), 3)

    def test_slugify(self):
        self.assertEqual(slugify("Hello World"), "hello-world")
        self.assertEqual(slugify(" A B "), "a-b")

    def test_parse_pairs(self):
        self.assertEqual(parse_pairs("a=1, b=2"), {"a": "1", "b": "2"})
        self.assertEqual(parse_pairs(""), {})

