//! Engine for automatic generation of Rust FFI bindings to Python modules.

mod codegen;
mod config;
mod syntax;
mod traits;
mod types;
mod utils;

pub use codegen::Codegen;
pub use config::Config;
pub use utils::{build::build_bindings, error::PyBindgenError, result::PyBindgenResult};

use utils::io as io_utils;
use utils::result::Result;
