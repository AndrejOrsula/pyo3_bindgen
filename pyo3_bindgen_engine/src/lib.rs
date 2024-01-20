//! Engine for automatic generation of Rust FFI bindings to Python modules.

pub mod bindgen;
pub mod build_utils;
pub mod types;

pub use bindgen::{generate_bindings, generate_bindings_for_module, generate_bindings_from_str};
pub use build_utils::build_bindings;
