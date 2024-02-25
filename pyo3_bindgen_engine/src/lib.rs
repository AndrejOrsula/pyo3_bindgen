//! Engine for automatic generation of Rust FFI bindings to Python modules.

mod codegen;
mod config;
mod syntax;
mod types;
mod utils;

// Re-export the public API
pub use codegen::Codegen;
pub use config::Config;
pub use utils::{error::PyBindgenError, result::PyBindgenResult};

// Re-export pyo3 for convenience
pub use pyo3;

// Internal re-exports for convenience
use utils::io as io_utils;
use utils::result::Result;
