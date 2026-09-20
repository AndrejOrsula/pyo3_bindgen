/// Error type for `pyo3_bindgen` operations.
#[derive(thiserror::Error, Debug)]
pub enum PyBindgenError {
    #[error(transparent)]
    IoError(#[from] std::io::Error),
    #[error(transparent)]
    PyError(#[from] pyo3::PyErr),
    #[error("Failed to cast Python object")]
    PyDowncastError,
    #[error(transparent)]
    SynError(#[from] syn::Error),
    #[error("Failed to parse Python code: {0}")]
    ParseError(String),
    #[error("Failed to generate Rust code: {0}")]
    CodegenError(String),
    #[error(transparent)]
    ResourceLimitExceeded(#[from] crate::resource::ResourceLimitExceeded),
    #[error(transparent)]
    ResourceMeasurementUnavailable(#[from] crate::resource::ResourceMeasurementUnavailable),
    #[error(transparent)]
    Infallible(#[from] std::convert::Infallible),
}
