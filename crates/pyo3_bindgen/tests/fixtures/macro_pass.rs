use pyo3_bindgen::import_python;

import_python!("math");

pub fn python_pi(py: ::pyo3::Python<'_>) -> ::pyo3::PyResult<f64> {
    math::pi(py)
}
