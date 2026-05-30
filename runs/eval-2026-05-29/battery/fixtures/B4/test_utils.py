import unittest

from utils import clamp


class TestClamp(unittest.TestCase):
    def test_above(self):
        self.assertEqual(clamp(5, 0, 3), 3)

    def test_below(self):
        self.assertEqual(clamp(-1, 0, 3), 0)

    def test_within(self):
        self.assertEqual(clamp(2, 0, 3), 2)


if __name__ == "__main__":
    unittest.main()
