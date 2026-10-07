"""Typed containers, optional values and dataclasses."""

from dataclasses import dataclass, field


@dataclass
class Item:
    """A stocked item."""

    name: str
    price: float
    tags: list[str] = field(default_factory=list)


def total(items: list[Item]) -> float:
    """The summed price of the items."""
    return sum(item.price for item in items)


def by_tag(items: list[Item]) -> dict[str, list[str]]:
    """Item names grouped by tag."""
    groups: dict[str, list[str]] = {}
    for item in items:
        for tag in item.tags:
            groups.setdefault(tag, []).append(item.name)
    return groups


def find(items: list[Item], name: str) -> Item | None:
    """The item with this name, if any."""
    return next((item for item in items if item.name == name), None)


def bounds(prices: tuple[float, ...]) -> tuple[float, float]:
    """The lowest and highest price."""
    return min(prices), max(prices)
