export const DEFAULT_EXAMPLE = 'greeting';

export const EXAMPLES = {
  greeting: {
    title: 'Greeting',
    label: 'Greeting: optional arguments',
    description: 'Change a greeting and see optional Python arguments become a Rust call builder.',
    concepts: ['Functions', 'Optional arguments', 'Call builders'],
    note: 'This example turns on call builders, which let Rust leave out optional arguments: see greet_builder in the bindings.',
    options: { call_builders: true, traverse_submodules: false },
    experiment: {
      label: 'Add greet_many',
      description: 'Inspect the generated API for a function with typed list input and output.',
      append: `def greet_many(names: list[str]) -> list[str]:
    """Greet each person in order."""
    return [greet(name) for name in names]
`,
      item: 'greet_many',
    },
  },
  shapes: {
    title: 'Shapes',
    label: 'Shapes: inheritance',
    description: 'Add a shape that inherits the shared description behavior.',
    concepts: ['Classes', 'Inheritance', 'Methods'],
    options: { call_builders: false, traverse_submodules: false },
    experiment: {
      label: 'Add Rectangle',
      description: 'Inspect the Rectangle wrapper and inherited Shape methods.',
      append: `class Rectangle(Shape):
    """A rectangle given by its width and height."""

    def __init__(self, width: float, height: float) -> None:
        self.width = width
        self.height = height

    def area(self) -> float:
        """Area in square units."""
        return self.width * self.height
`,
      item: 'Rectangle',
    },
  },
  palette: {
    title: 'Palette',
    label: 'Palette: enums',
    description: 'Add a function that accepts the existing Colour enum.',
    concepts: ['Enums', 'Typed arguments', 'Functions'],
    options: { call_builders: false, traverse_submodules: false },
    experiment: {
      label: 'Add is_warm',
      description: 'Inspect a function that takes Colour and returns bool.',
      append: `def is_warm(colour: Colour) -> bool:
    """Whether a colour is warm."""
    return colour is Colour.RED
`,
      item: 'is_warm',
    },
  },
  inventory: {
    title: 'Inventory',
    label: 'Inventory: dataclasses',
    description: 'Explore dataclass fields and functions over typed containers.',
    concepts: ['Dataclasses', 'Lists', 'Typed functions'],
    options: { call_builders: false, traverse_submodules: false },
    experiment: {
      label: 'Add affordable',
      description: 'Inspect a function that filters a typed Item list.',
      append: `def affordable(items: list[Item], ceiling: float) -> list[Item]:
    """Return items priced at or below the ceiling."""
    return [item for item in items if item.price <= ceiling]
`,
      item: 'affordable',
    },
  },
  orbit: {
    title: 'Orbit',
    label: 'Orbit: functions and classes',
    description: 'Explore a physical constant, a function, a class, and a property.',
    concepts: ['Constants', 'Classes', 'Properties'],
    options: { call_builders: false, traverse_submodules: false },
    experiment: {
      label: 'Add low_earth_orbit',
      description: 'Inspect a function that returns an Orbit.',
      append: `def low_earth_orbit(altitude: float) -> Orbit:
    """Create a circular orbit at altitude above Earth's mean radius."""
    return Orbit(6_371_000.0 + altitude)
`,
      item: 'low_earth_orbit',
    },
  },
};
