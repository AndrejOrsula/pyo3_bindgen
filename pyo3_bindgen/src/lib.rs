//! Public API library for automatic generation of Rust FFI bindings to Python modules.
//!
//! ## Instructions
//!
//! Add `pyo3` as a dependency and `pyo3_bindgen` as a build dependency to your [`Cargo.toml`](https://doc.rust-lang.org/cargo/reference/manifest.html) manifest (`auto-initialize` feature of `pyo3` is optional and shown here for your convenience).
//!
//! ```toml
//! [dependencies]
//! pyo3 = { version = "0.20", features = ["auto-initialize"] }
//!
//! [build-dependencies]
//! pyo3_bindgen = { version = "0.1" }
//! ```
//!
//! ### <a href="#-option-1-build-script"><img src="https://rustacean.net/assets/rustacean-flat-noshadow.svg" width="16" height="16"></a> Option 1: Build script
//!
//! Create a [`build.rs`](https://doc.rust-lang.org/cargo/reference/build-scripts.html) script in the root of your crate that generates bindings to the `target_module` Python module.
//!
//! ```rs
//! // build.rs
//!
//! fn main() {
//!     // Generate Rust bindings to the Python module
//!     pyo3_bindgen::build_bindings(
//!         "target_module",
//!         std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("bindings.rs"),
//!     )
//!     .unwrap();
//! }
//! ```
//!
//! Afterwards, include the generated bindings anywhere in your crate.
//!
//! ```rs
//! include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
//! pub use target_module::*;
//! ```
//!
//! ### <a href="#-option-2-cli-tool"><img src="https://www.svgrepo.com/show/353478/bash-icon.svg" width="16" height="16"></a> Option 2: CLI tool
//!
//! Install the `pyo3_bindgen` executable with `cargo`.
//!
//! ```bash
//! cargo install --locked pyo3_bindgen_cli
//! ```
//!
//! Afterwards, run the `pyo3_bindgen` executable while passing the name of the target Python module.
//!
//! ```bash
//! # Pass `--help` to show the usage and available options
//! pyo3_bindgen -m target_module -o bindings.rs
//! ```
//!
//! ### <a href="#-option-3-experimental-procedural-macros"><img src="https://www.svgrepo.com/show/269868/lab.svg" width="16" height="16"></a> Option 3 \[Experimental\]: Procedural macros
//!
//! > **Note:** This feature is experimental and will probably fail in many cases. It is recommended to use build scripts instead.
//!
//! Enable the `macros` feature of `pyo3_bindgen`.
//!
//! ```toml
//! [build-dependencies]
//! pyo3_bindgen = { version = "0.1", features = ["macros"] }
//! ```
//!
//! Then, you can call the `import_python!` macro anywhere in your crate.
//!
//! ```rs
//! pyo3_bindgen::import_python!("target_module");
//! pub use target_module::*;
//! ```

pub use pyo3_bindgen_engine::{
    self as engine, build_bindings, generate_bindings, generate_bindings_for_module,
    generate_bindings_from_str,
};

#[cfg(feature = "macros")]
pub use pyo3_bindgen_macros::{self as macros, import_python};
