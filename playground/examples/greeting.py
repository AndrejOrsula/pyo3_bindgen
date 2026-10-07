"""Optional arguments, which call builders let Rust leave out."""


def greet(
    name: str, greeting: str = "Hello", punctuation: str = "!", shout: bool = False
) -> str:
    """Greets someone, by default politely."""
    text = f"{greeting}, {name}{punctuation}"
    return text.upper() if shout else text


def tally(*scores: int, bonus: int = 0) -> int:
    """Sums any number of scores, plus a bonus."""
    return sum(scores) + bonus
