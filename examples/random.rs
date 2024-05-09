//! Example demonstrating the use of the `import_python!` macro for the "random" module.
//!
//! Python equivalent:
//!
//! ```py
//! import random
//!
//! rand_f64 = random.random()
//! assert 0.0 <= rand_f64 <= 1.0
//! print(f"Random f64: {rand_f64}")
//! ```

pyo3_bindgen::import_python!("random");

fn main() -> pyo3::PyResult<()> {
    pyo3::Python::with_gil(|py| {
        use ::pyo3::types::PyAnyMethods;
        let rand_f64: f64 = random::random(py)?.extract()?;
        assert!((0.0..=1.0).contains(&rand_f64));
        println!("Random f64: {}", rand_f64);
        Ok(())
    })
}
