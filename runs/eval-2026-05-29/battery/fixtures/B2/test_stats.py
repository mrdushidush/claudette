import unittest

from stats import variance


class TestVariance(unittest.TestCase):
    def test_variance(self):
        self.assertEqual(variance([2, 4, 6]), 4.0)


if __name__ == "__main__":
    unittest.main()
