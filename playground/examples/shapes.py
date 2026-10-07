"""Shapes that share behaviour through inheritance."""

import math


class Shape:
    """A plane figure."""

    def area(self) -> float:
        """Area in square units."""
        raise NotImplementedError

    def describe(self) -> str:
        """A short description of the shape."""
        return f"{type(self).__name__} with area {self.area():.2f}"


class Circle(Shape):
    """A circle given by its radius."""

    def __init__(self, radius: float) -> None:
        self.radius = radius

    def area(self) -> float:
        return math.pi * self.radius**2


class Square(Shape):
    """A square given by its side."""

    def __init__(self, side: float) -> None:
        self.side = side

    def area(self) -> float:
        return self.side**2
