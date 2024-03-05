//! <p align="left">
//!   <a href="https://crates.io/crates/pyo3_bindgen">                                   <img alt="crates.io"  src="https://img.shields.io/crates/v/pyo3_bindgen.svg"></a>
//!   <a href="https://docs.rs/pyo3_bindgen">                                            <img alt="docs.rs"    src="https://docs.rs/pyo3_bindgen/badge.svg"></a>
//!   <a href="https://github.com/AndrejOrsula/pyo3_bindgen/actions/workflows/rust.yml"> <img alt="Rust"       src="https://github.com/AndrejOrsula/pyo3_bindgen/actions/workflows/rust.yml/badge.svg"></a>
//!   <a href="https://deps.rs/repo/github/AndrejOrsula/pyo3_bindgen">                   <img alt="deps.rs"    src="https://deps.rs/repo/github/AndrejOrsula/pyo3_bindgen/status.svg"></a>
//!   <a href="https://codecov.io/gh/AndrejOrsula/pyo3_bindgen">                         <img alt="codecov.io" src="https://codecov.io/gh/AndrejOrsula/pyo3_bindgen/branch/main/graph/badge.svg"></a>
//! </p>
//!
//! Automatic generation of Rust FFI bindings to Python modules via [PyO3](https://pyo3.rs). Python modules are analyzed recursively to generate Rust bindings with an identical structure for all public classes, functions, properties, and constants. Any available docstrings and type annotations are also preserved in their Rust equivalents.
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
//! pyo3_bindgen = { version = "0.3" }
//! ```
//!
//! ### <a href="#-option-1-build-script"><img src="https://rustacean.net/assets/rustacean-flat-noshadow.svg" width="16" height="16"></a> Option 1: Build script
//!
//! Create a [`build.rs`](https://doc.rust-lang.org/cargo/reference/build-scripts.html) script in the root of your crate that generates bindings to the `py_module` Python module.
//!
//! ```no_run
//! // build.rs
//! use pyo3_bindgen::{Codegen, Config};
//!
//! fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // Generate Rust bindings to Python modules
//!     Codegen::new(Config::default())?
//!         .module_name("py_module")?
//!         .build(std::path::Path::new(&std::env::var("OUT_DIR")?).join("bindings.rs"))?;
//!     Ok(())
//! }
//! ```
//!
//! Afterwards, include the generated bindings anywhere in your crate.
//!
//! ```ignore
//! include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
//! pub use py_module::*;
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
//! pyo3_bindgen -m py_module -o bindings.rs
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
//! pyo3_bindgen = { version = "0.3", features = ["macros"] }
//! ```
//!
//! Then, you can call the `import_python!` macro anywhere in your crate.
//!
//! ```ignore
//! pyo3_bindgen::import_python!("py_module");
//! pub use py_module::*;
//! ```

// Public API re-exports from engine
pub use pyo3_bindgen_engine::{pyo3, Codegen, Config, PyBindgenError, PyBindgenResult};

// Public API re-exports from macros
#[cfg(feature = "macros")]
pub use pyo3_bindgen_macros::import_python;
