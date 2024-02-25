/// Result wrapper for `PyBindgenError`.
pub type PyBindgenResult<T> = std::result::Result<T, super::error::PyBindgenError>;

/// Crate-local alias for `PyBindgenResult`.
pub(crate) type Result<T> = PyBindgenResult<T>;
