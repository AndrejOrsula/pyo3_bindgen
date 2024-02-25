/// Error type for `pyo3_bindgen` operations.
#[derive(Debug, thiserror::Error)]
pub enum PyBindgenError {
    #[error(transparent)]
    PyError(#[from] pyo3::PyErr),
    #[error("Failed to convert `pyo3::PyAny` to a more specific Python type: {0}")]
    PyDowncastError(String),
    #[error(transparent)]
    IoError(#[from] std::io::Error),
    #[error(transparent)]
    SynError(#[from] syn::Error),
}

impl<'py> From<pyo3::PyDowncastError<'py>> for PyBindgenError {
    fn from(value: pyo3::PyDowncastError) -> Self {
        PyBindgenError::PyDowncastError(value.to_string())
    }
}
