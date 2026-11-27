import unittest
from multipart.calc import add
from multipart.text import slugify
from multipart.data import merge_dicts


class Hidden(unittest.TestCase):
    def test_add(self):
        self.assertEqual(add(2, 3), 5)
        self.assertEqual(add(-1, 1), 0)

    def test_slugify(self):
        self.assertEqual(slugify("Hello World"), "hello-world")
        self.assertEqual(slugify(" A B "), "a-b")

    def test_merge_dicts(self):
        a = {"a": 1, "b": 2}
        b = {"b": 3, "c": 4}
        self.assertEqual(merge_dicts(a, b), {"a": 1, "b": 3, "c": 4})
        self.assertEqual(a, {"a": 1, "b": 2})

