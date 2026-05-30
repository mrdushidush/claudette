import unittest

from core import add, mul, sub


class TestCore(unittest.TestCase):
    def test_add(self):
        self.assertEqual(add(2, 3), 5)

    def test_sub(self):
        self.assertEqual(sub(10, 4), 6)

    def test_mul(self):
        self.assertEqual(mul(3, 7), 21)


if __name__ == "__main__":
    unittest.main()
