/// Error type for `pyo3_bindgen` operations.
#[derive(thiserror::Error, Debug)]
pub enum PyBindgenError {
    #[error(transparent)]
    IoError(#[from] std::io::Error),
    #[error(transparent)]
    PyError(#[from] pyo3::PyErr),
    #[error(transparent)]
    SynError(#[from] syn::Error),
}
