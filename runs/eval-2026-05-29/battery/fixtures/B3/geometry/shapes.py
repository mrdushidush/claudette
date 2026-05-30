from .circle import area, PI


def ring_area(outer, inner):
    return area(outer) - area(inner)
