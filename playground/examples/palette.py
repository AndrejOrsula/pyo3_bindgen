"""Colours as enums, which become Rust enums."""

import enum


class Colour(enum.Enum):
    """A primary colour."""

    RED = 1
    GREEN = 2
    BLUE = 3


class Finish(enum.StrEnum):
    """How a surface reflects light."""

    MATTE = "matte"
    GLOSS = "gloss"


MIXES = {
    frozenset({Colour.RED, Colour.GREEN}): "yellow",
    frozenset({Colour.GREEN, Colour.BLUE}): "cyan",
    frozenset({Colour.RED, Colour.BLUE}): "magenta",
}


def mix(first: Colour, second: Colour) -> str:
    """Names the colour that two primaries of light make together."""
    return MIXES.get(frozenset({first, second}), first.name.lower())


def paint(colour: Colour, finish: Finish = Finish.MATTE) -> str:
    """Describes a coat of paint."""
    return f"{finish.value} {colour.name.lower()}"
