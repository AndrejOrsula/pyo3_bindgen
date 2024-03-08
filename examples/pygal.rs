//! Example demonstrating the use of the `import_python!` macro for the "pygal" module.
//!
//! Python equivalent:
//!
//! ```py
//! import pygal
//!
//! bar_chart = pygal.Bar(style=pygal.style.DarkStyle)
//! bar_chart.add("Fibonacci", [0, 1, 1, 2, 3, 5, 8, 13, 21, 34, 55])
//! bar_chart.add("Padovan", [1, 1, 1, 2, 2, 3, 4, 5, 7, 9, 12])
//! svg = bar_chart.render(false)
//! print(svg)
//! ```

pyo3_bindgen::import_python!("pygal");

fn main() -> pyo3::PyResult<()> {
    pyo3::Python::with_gil(|py| {
        let bar_chart = pygal::Bar::new(
            py,
            (),
            Some(pyo3::types::IntoPyDict::into_py_dict(
                [("style", pygal::style::DarkStyle::new(py, None)?)],
                py,
            )),
        )?;
        bar_chart.add(py, "Fibonacci", [0, 1, 1, 2, 3, 5, 8, 13, 21, 34, 55], None)?;
        bar_chart.add(py, "Padovan", [1, 1, 1, 2, 2, 3, 4, 5, 7, 9, 12], None)?;
        let svg = bar_chart.render(py, false, None)?;
        println!("{svg}");
        Ok(())
    })
}
