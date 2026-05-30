import unittest

from geometry.circle import PI, area
from geometry.shapes import ring_area


class TestGeometry(unittest.TestCase):
    def test_pi_precise(self):
        # PI must be precise to 5 places; 3.14 fails this.
        self.assertAlmostEqual(PI, 3.14159, places=5)

    def test_area(self):
        self.assertAlmostEqual(area(1), 3.14159, places=5)

    def test_ring(self):
        # outer area minus inner area, still consistent through shapes.py
        self.assertAlmostEqual(ring_area(2, 1), 3.0 * 3.14159, places=4)


if __name__ == "__main__":
    unittest.main()
