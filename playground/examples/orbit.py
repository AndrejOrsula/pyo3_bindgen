"""Two-body orbits around the Earth."""

import math

MU_EARTH = 3.986004418e14


def period(semi_major_axis: float) -> float:
    """Orbital period in seconds for a semi-major axis in metres."""
    return 2.0 * math.pi * math.sqrt(semi_major_axis**3 / MU_EARTH)


class Orbit:
    """A Keplerian orbit around the Earth."""

    def __init__(self, semi_major_axis: float, eccentricity: float = 0.0) -> None:
        self.semi_major_axis = semi_major_axis
        self.eccentricity = eccentricity

    @property
    def apoapsis(self) -> float:
        """Farthest distance from the Earth's centre in metres."""
        return self.semi_major_axis * (1.0 + self.eccentricity)

    def period(self) -> float:
        """Orbital period in seconds."""
        return period(self.semi_major_axis)
