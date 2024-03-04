/// Error type for `pyo3_bindgen` operations.
#[derive(thiserror::Error, Debug)]
pub enum PyBindgenError {
    #[error(transparent)]
    IoError(#[from] std::io::Error),
    #[error(transparent)]
    PyError(#[from] pyo3::PyErr),
    #[error("Failed to downcast Python object")]
    PyDowncastError,
    #[error(transparent)]
    SynError(#[from] syn::Error),
}

impl From<pyo3::PyDowncastError<'_>> for PyBindgenError {
    fn from(value: pyo3::PyDowncastError) -> Self {
        pyo3::PyErr::from(value).into()
    }
}
