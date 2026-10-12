export const DEFAULT_EXAMPLE = 'greeting';

export const EXAMPLES = {
  greeting: {
    title: 'Greeting',
    label: 'Greeting: optional arguments',
    description: 'Change a greeting and see optional Python arguments become a Rust call builder.',
    concepts: ['Functions', 'Optional arguments', 'Call builders'],
    note: 'This example turns on call builders, which let Rust leave out optional arguments: see greet_builder in the bindings.',
    options: { call_builders: true, traverse_submodules: false },
    starter: {
      body: `let greeting = greeting::greet_builder(py, "Ada")?.call()?;
assert_eq!(greeting, "Hello, Ada!");
println!("greeting: {greeting}");`,
      experimentBody: `let names = [String::from("Ada"), String::from("Grace")];
let greetings = greeting::greet_many(py, &names)?;
assert_eq!(greetings, [String::from("Hello, Ada!"), String::from("Hello, Grace!")]);
println!("greeting: {}", greetings.join(" | "));`,
    },
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
    starter: {
      body: `use shapes::CircleMethods;
let circle = shapes::Circle::new(py, 2.0)?;
let description = circle.describe(py)?;
assert_eq!(description, "Circle with area 12.57");
println!("shapes: {description}");`,
      experimentBody: `use shapes::RectangleMethods;
let rectangle = shapes::Rectangle::new(py, 3.0, 4.0)?;
let area = rectangle.area(py)?;
let description = rectangle.describe(py)?;
assert_eq!(area, 12.0);
assert_eq!(description, "Rectangle with area 12.00");
println!("shapes: {description}");`,
    },
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
    starter: {
      body: `let red = palette::Colour::Red;
let green = palette::Colour::Green;
let mixture = palette::mix(py, &red, &green)?;
assert_eq!(mixture, "yellow");
println!("palette: {mixture}");`,
      experimentBody: `let red = palette::Colour::Red;
let warm = palette::is_warm(py, &red)?;
assert!(warm);
println!("palette: {}", if warm { "warm" } else { "cool" });`,
    },
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
    starter: {
      body: `let items = [
    inventory::Item::new(py, "apple", 2.5, &[])?,
    inventory::Item::new(py, "tea", 4.0, &[])?,
];
let total = inventory::total(py, &items)?;
assert_eq!(total, 6.5);
println!("inventory: {total:.2} total");`,
      experimentBody: `use inventory::ItemMethods;
let items = [
    inventory::Item::new(py, "apple", 2.5, &[])?,
    inventory::Item::new(py, "tea", 4.0, &[])?,
    inventory::Item::new(py, "lamp", 18.0, &[])?,
];
let total = inventory::total(py, &items)?;
let affordable = inventory::affordable(py, &items, 4.0)?;
assert_eq!(total, 24.5);
let names = affordable.iter().map(|item| item.name(py)).collect::<pyo3::PyResult<Vec<_>>>()?;
assert_eq!(names, [String::from("apple"), String::from("tea")]);
println!("inventory: {} (total {total:.2})", names.join(", "));`,
    },
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
    starter: {
      body: `use orbit::OrbitMethods;
let orbit = orbit::Orbit::new(py, 6_771_000.0, 0.1)?;
let apoapsis = orbit.apoapsis(py)?;
let period = orbit.period(py)?;
assert!((apoapsis - 7_448_100.0).abs() < 0.001);
assert!((5_000.0..6_000.0).contains(&period));
println!("orbit: apoapsis {:.0} m; period {:.0} s", apoapsis, period);`,
      experimentBody: `use orbit::OrbitMethods;
let orbit = orbit::low_earth_orbit(py, 400_000.0)?;
let apoapsis = orbit.apoapsis(py)?;
let period = orbit.period(py)?;
assert_eq!(apoapsis, 6_771_000.0);
assert!((5_000.0..6_000.0).contains(&period));
println!("orbit: LEO apoapsis {:.0} m; period {:.0} s", apoapsis, period);`,
    },
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

const PAGE_ONLY_OPTIONS = new Set(['analysis', 'max_output_bytes']);

function qualifiedOptions(options, defaults) {
  if (!options || !defaults || typeof options !== 'object' || typeof defaults !== 'object') return null;
  if (Object.keys(options).some((key) => !PAGE_ONLY_OPTIONS.has(key) && !Object.hasOwn(defaults, key))) return null;
  return Object.fromEntries(
    Object.keys(defaults)
      .filter((key) => !PAGE_ONLY_OPTIONS.has(key))
      .map((key) => [key, Object.hasOwn(options, key) ? options[key] : defaults[key]]),
  );
}

const sameOptions = (left, right) => JSON.stringify(left) === JSON.stringify(right);

// Return an executable body only for the exact built-in fixture or experiment
// and its qualified generator options. All values are captured by the caller
// from the same generation request; page-only display limits do not qualify.
export function selectStarterBody(source, module, options, defaults, baseFixture) {
  const example = EXAMPLES[module];
  if (!example?.starter || typeof baseFixture !== 'string') return null;
  const base = source === baseFixture;
  const experiment = Boolean(
    example.experiment &&
    source === `${baseFixture.trimEnd()}\n\n${example.experiment.append}`,
  );
  if (!base && !experiment) return null;
  const actual = qualifiedOptions(options, defaults);
  const expected = qualifiedOptions({ ...defaults, ...example.options }, defaults);
  if (!actual || !expected || !sameOptions(actual, expected)) return null;
  return base ? example.starter.body : example.starter.experimentBody;
}
