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
//! ## Optional generated types
//!
//! The `numpy` feature maps unknown-dtype `numpy.ndarray` annotations to
//! `numpy::PyUntypedArray`; the
//! `num-complex` feature maps Python `complex` to `num_complex::Complex64`. Because
//! these names appear in generated code, the consuming crate must declare `numpy =
//! "0.29"` and/or `num-complex = "0.4"` directly. Complex consumers must also enable
//! PyO3's `num-complex` feature.
//!
//! ## Instructions
//!
//! **Security:** live generation imports and introspects Python modules, so module
//! initialization and descriptors execute with the generator's privileges. Only
//! introspect trusted environments; use reviewed external `SchemaIr` input when code
//! execution is not acceptable.
//!
//! Live APIs support one main CPython interpreter identity for the lifetime of a process. Do not
//! finalize or reinitialize that interpreter, switch interpreter identities, or use subinterpreters
//! with live generation. Use a fresh process when interpreter-lifecycle isolation is required.
//! Multiple callers and threads are supported only while attached to that same interpreter
//! identity. Process-static `Py<T>` caches are not keyed by interpreter identity. Process-wide
//! coordination protects known engine-managed globals, including output streams and restored
//! standard-library sentinels; it is not a transaction or sandbox for arbitrary Python imports,
//! descriptors, callbacks, finalizers, or other Python effects.
//!
//! The built-in external-IR path accepts a materialized [`SchemaIr`] without initializing,
//! attaching to, importing from, or executing a Python interpreter. Custom [`BindingSource`]
//! adapters acquire and materialize input outside that execution-free guarantee; their transport,
//! parser, trust, and side-effect behavior remains the adapter's responsibility.
//!
//! ### <a href="#-option-1-build-script"><img src="https://rustacean.net/assets/rustacean-flat-noshadow.svg" width="16" height="16"></a> Option 1: Build script
//!
//! First, add `pyo3_bindgen` as a **build dependency** to your [`Cargo.toml`](https://doc.rust-lang.org/cargo/reference/manifest.html) manifest. To compile the generated bindings, you must also add `pyo3` as a regular dependency because generated paths refer to the `pyo3` crate directly.
//!
//! ```toml
//! [build-dependencies]
//! pyo3_bindgen = { version = "0.6" }
//!
//! [dependencies]
//! pyo3 = { version = "0.29", features = ["auto-initialize"] }
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
//! Afterwards, you can include the generated Rust code via the `include!` macro anywhere in your crate and use the generated bindings as regular Rust modules. The bindings take a `pyo3::Python<'_>` token and can be called from a `pyo3::Python::attach` closure.
//!
//! ```ignore
//! //! src/main.rs
//! include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
//!
//! fn main() -> pyo3::PyResult<()> {
//!     pyo3::Python::attach(|py| {
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
//! As an alternative to build scripts, you can use procedural macros to generate the bindings in-place. First, add `pyo3_bindgen` as a **regular dependency** to your [`Cargo.toml`](https://doc.rust-lang.org/cargo/reference/manifest.html) manifest and enable the `macros` feature. Add `pyo3` directly because the generated paths refer to that crate.
//!
//! ```toml
//! [dependencies]
//! pyo3 = { version = "0.29", features = ["auto-initialize"] }
//! pyo3_bindgen = { version = "0.6", features = ["macros"] }
//! ```
//!
//! Subsequently, the `import_python!` macro can be used to generate Rust bindings for the selected Python modules anywhere in your crate. As demonstrated in the example below, Rust bindings are generated for the "math" Python module and can directly be used in the same scope. Similar to the previous approach, call the generated bindings while attached to Python via `pyo3::Python::attach`.
//!
//! > As opposed to using build scripts, this approach does not offer the same level of customization via `pyo3_bindgen::Config`. Furthermore, the procedural macro is quite experimental and might not work in all cases.
//!
//! > **Security:** `import_python!` imports and introspects Python during compile time. Module
//! > initialization, descriptors, package hooks, and annotations can execute arbitrary code
//! > with the compiler process's privileges. Use this macro only with trusted Python
//! > environments; use a reviewed external `SchemaIr` when compile-time Python execution is
//! > not acceptable.
//!
//! ```ignore
//! use pyo3_bindgen::import_python;
//! import_python!("math");
//!
//! // Which Pi do you prefer?
//! // a) 🐍 Pi from Python "math" module
//! // b) 🦀 Pi from Rust standard library
//! // c) 🥧 Pi from your favourite bakery
//! pyo3::Python::attach(|py| {
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

pub mod publication;

// Public API re-exports from engine
pub use pyo3_bindgen_engine::{
    enum_variant_ident, plan_enum_variants, stable_hash, ArtifactDurability, ArtifactStatus,
    ArtifactWriteFailure, ArtifactWriteReceipt, BindingSource, ClassIr, Codegen, Config,
    DiagnosticSeverity, EnumAliasIr, EnumAliasReport, EnumIr, EnumKindIr, EnumReport,
    EnumVariantIr, EnumVariantPlan, EnumVariantReport, FunctionIr, FunctionKindIr,
    GeneratedArtifact, GeneratedArtifactFailure, GeneratedBindings, GeneratedCounts,
    GenerationBudget, GenerationDiagnostic, GenerationReport, GenerationReportDocument,
    GenerationWork, GeneratorMetadata, ImportIr, LiveModules, MethodKindIr, ModuleIr, ModuleReport,
    OutputMetadata, ParameterIr, ParameterKindIr, PropertyIr, PyBindgenError, PyBindgenResult,
    PythonMetadata, ResourceKind, ResourceLimitExceeded, ResourceMeasurementUnavailable,
    ResourcePhase, RustArtifact, SchemaIr, SchemaSource, TextArtifact,
};

#[doc(hidden)]
pub use pyo3_bindgen_engine::ExternalIrSourceContext;

#[doc(hidden)]
pub use pyo3_bindgen_engine::{
    artifact_identity_key, artifact_identity_path, artifact_lock_keys, artifact_lock_path,
    artifact_revalidation_identity_matches, conservative_casefold_identity_key,
    existing_file_identity, open_artifact_lock, open_ir_artifact_file, publish_report,
    validate_windows_artifact_component, ArtifactLockSet, ArtifactPath, ArtifactPathKind,
    ArtifactPaths, CurrentArtifactIdentity, DeliveryDurability, DeliveryKind, DeliveryReceipt,
    DeliveryState, ExitClass, IrArtifactOpenError, OperationCode, OperationFailure,
    PublicationAttempt, PublicationPlan, PublicationPolicy, PublicationSession, ReportWriteAttempt,
    RunFailure, RunOutcome, RunState,
};

/// Re-exported only for this workspace's own tests; see the engine's `internal-test-hooks` feature.
#[cfg(feature = "internal-test-hooks")]
#[doc(hidden)]
pub use pyo3_bindgen_engine::{
    inject_output_publication_failure_once, inject_report_publication_failure_once,
    TestPublicationFailure, LOCK_OWNER_COUNTER,
};

#[cfg(windows)]
#[doc(hidden)]
pub use pyo3_bindgen_engine::{
    validate_windows_artifact_path, windows_file_id_identity,
    windows_legacy_file_identity_is_sound, windows_lexically_normalized_absolute_path,
};

// Public API re-exports from macros
#[cfg(feature = "macros")]
pub use pyo3_bindgen_macros::import_python;
