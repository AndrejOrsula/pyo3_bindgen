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
//! ### <a href="#-option-1-build-script"><img src="https://rustacean.net/assets/rustacean-flat-noshadow.svg" width="16" height="16"></a> Option 1: Build script
//!
//! First, add `pyo3_bindgen` as a **build dependency** to your [`Cargo.toml`](https://doc.rust-lang.org/cargo/reference/manifest.html) manifest. To actually use the generated bindings, you will also need to add `pyo3` as a regular dependency (or use the re-exported `pyo3_bindgen::pyo3` module).
//!
//! ```toml
//! [build-dependencies]
//! pyo3_bindgen = { version = "0.5" }
//!
//! [dependencies]
//! pyo3 = { version = "0.21", features = ["auto-initialize"] }
//! ```
//!
//! Then, create a [`build.rs`](https://doc.rust-lang.org/cargo/reference/build-scripts.html) script in the root of your crate that generates bindings to the selected Python modules. In this example, the bindings are simultaneously generated for the "os", "posixpath", and "sys" Python modules. At the end of the generation process, the Rust bindings are written to `${OUT_DIR}/bindings.rs`.
//!
//! > With this approach, you can also customize the generation process via [`pyo3_bindgen::Config`](https://docs.rs/pyo3_bindgen/latest/pyo3_bindgen/struct.Config.html) that can be passed to `Codegen::new` constructor, e.g. `Codegen::new(Config::builder().include_private(true).build())`.
//!
//! ```no_run
//! //! build.rs
//! use pyo3_bindgen::Codegen;
//!
//! fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     Codegen::default()
//!         .module_names(["os", "posixpath", "sys"])?
//!         .build(format!("{}/bindings.rs", std::env::var("OUT_DIR")?))?;
//!     Ok(())
//! }
//! ```
//!
//! Afterwards, you can include the generated Rust code via the `include!` macro anywhere in your crate and use the generated bindings as regular Rust modules. However, the bindings must be used within the `pyo3::Python::with_gil` closure to ensure that Python [GIL](https://wiki.python.org/moin/GlobalInterpreterLock) is held.
//!
//! ```ignore
//! //! src/main.rs
//! include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
//!
//! fn main() -> pyo3::PyResult<()> {
//!     # pyo3::prepare_freethreaded_python();
//!     pyo3::Python::with_gil(|py| {
//!         // Get the path to the Python executable via "sys" Python module
//!         let python_exe_path = sys::executable(py)?;
//!         // Get the current working directory via "os" Python module
//!         let current_dir = os::getcwd(py)?;
//!         // Get the relative path to the Python executable via "posixpath" Python module
//!         let relpath_to_python_exe = posixpath::relpath(py, python_exe_path, current_dir)?;
//!
//!         println!("Relative path to Python executable: '{relpath_to_python_exe}'");
//!         Ok(())
//!     })
//! }
//! ```
//!
//! ### <a href="#-option-2-procedural-macros-experimental"><img src="https://www.svgrepo.com/show/269868/lab.svg" width="16" height="16"></a> Option 2: Procedural macros (experimental)
//!
//! As an alternative to build scripts, you can use procedural macros to generate the bindings in-place. First, add `pyo3_bindgen_macros` as a **regular dependency** to your [`Cargo.toml`](https://doc.rust-lang.org/cargo/reference/manifest.html) manifest and enable the `macros` feature.
//!
//! ```toml
//! [dependencies]
//! pyo3_bindgen = { version = "0.5", features = ["macros"] }
//! ```
//!
//! Subsequently, the `import_python!` macro can be used to generate Rust bindings for the selected Python modules anywhere in your crate. As demonstrated in the example below, Rust bindings are generated for the "math" Python module and can directly be used in the same scope. Similar to the previous approach, the generated bindings must be used within the `pyo3::Python::with_gil` closure to ensure that Python [GIL](https://wiki.python.org/moin/GlobalInterpreterLock) is held.
//!
//! > As opposed to using build scripts, this approach does not offer the same level of customization via `pyo3_bindgen::Config`. Furthermore, the procedural macro is quite experimental and might not work in all cases.
//!
//! ```ignore
//! use pyo3_bindgen::import_python;
//! import_python!("math");
//!
//! // Which Pi do you prefer?
//! // a) 🐍 Pi from Python "math" module
//! // b) 🦀 Pi from Rust standard library
//! // c) 🥧 Pi from your favourite bakery
//! # pyo3::prepare_freethreaded_python();
//! pyo3::Python::with_gil(|py| {
//!     let python_pi = math::pi(py).unwrap();
//!     let rust_pi = std::f64::consts::PI;
//!     assert_eq!(python_pi, rust_pi);
//! })
//! ```
//!
//! ### <a href="#-option-3-cli-tool"><img src="https://www.svgrepo.com/show/353478/bash-icon.svg" width="16" height="16"></a> Option 3: CLI tool
//!
//! For a quick start and testing purposes, you can use the `pyo3_bindgen` executable to generate and inspect bindings for the selected Python modules. The executable is available as a standalone package and can be installed via `cargo`.
//!
//! ```bash
//! cargo install --locked pyo3_bindgen_cli
//! ```
//!
//! Afterwards, run the `pyo3_bindgen` executable to generate Rust bindings for the selected Python modules. The generated bindings are printed to STDOUT by default, but they can also be written to a file via the `-o` option (see `pyo3_bindgen --help` for more options).
//!
//! ```bash
//! pyo3_bindgen -m os sys numpy -o bindings.rs
//! ```

// Public re-export of PyO3 for convenience
pub use pyo3;

// Public API re-exports from engine
pub use pyo3_bindgen_engine::{Codegen, Config, PyBindgenError, PyBindgenResult};

// Public API re-exports from macros
#[cfg(feature = "macros")]
pub use pyo3_bindgen_macros::import_python;
