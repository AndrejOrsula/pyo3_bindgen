//! Engine for automatic generation of Rust FFI bindings to Python modules.

mod codegen;
mod config;
mod syntax;
mod typing;
mod utils;

// Internal re-exports for convenience
use utils::io as io_utils;
use utils::result::Result;

// Public API re-exports
pub use codegen::Codegen;
pub use config::Config;
pub use utils::{error::PyBindgenError, result::PyBindgenResult};
