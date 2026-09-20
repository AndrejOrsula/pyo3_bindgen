//! Example demonstrating the use of the `import_python!` macro for the "math" module.
//!
//! Python equivalent:
//!
//! ```py
//! import math
//!
//! python_pi = math.pi
//! assert python_pi == 3.141592653589793
//! print(f"Python Pi: {python_pi}")
//! ```

pyo3_bindgen::import_python!("math");

fn main() {
    // Which Pi do you prefer?
    // a) 🐍 Pi from Python "math" module
    // b) 🦀 Pi from Rust standard library
    // c) 🥧 Pi from your favorite bakery
    pyo3::Python::attach(|py| {
        let python_pi = math::pi(py).unwrap();
        let rust_pi = std::f64::consts::PI;
        assert_eq!(python_pi, rust_pi);
        println!("Python Pi: {}", python_pi);
    })
}
