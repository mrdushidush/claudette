import unittest

from stats import mean


class TestMean(unittest.TestCase):
    def test_mean(self):
        self.assertEqual(mean([10, 20, 30]), 20.0)


if __name__ == "__main__":
    unittest.main()
