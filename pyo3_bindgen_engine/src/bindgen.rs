//! Module for handling the binding generation process.

pub mod attribute;
pub mod class;
pub mod function;
pub mod module;

pub use attribute::bind_attribute;
pub use class::bind_class;
pub use function::bind_function;
pub use module::{bind_module, bind_reexport};

// TODO: Refactor everything into a large configurable struct that keeps track of all the
//       important information needed to properly generate the bindings
//  - Use builder pattern for the configuration of the struct
//  - Keep track of all the types/classes that have been generated
//  - Keep track of all imports to understand where each type is coming from
//  - Keep track of all the external types that are used as parameters/return types and consider generating bindings for them as well

// TODO: Ensure there are no duplicate entries in the generated code

/// Generate Rust bindings to a Python module specified by its name. Generating bindings to
/// submodules such as `os.path` is also supported as long as the module can be directly imported
/// from the Python interpreter via `import os.path`.
///
/// # Arguments
///
/// * `module_name` - Name of the Python module to generate bindings for.
///
/// # Returns
///
/// `Result` containing the generated bindings as a `proc_macro2::TokenStream` on success, or a
/// `pyo3::PyErr` on failure.
///
/// # Example
///
/// ```
/// // use pyo3_bindgen::generate_bindings;
/// use pyo3_bindgen_engine::generate_bindings;
///
/// fn main() -> Result<(), pyo3::PyErr> {
///     let bindings: proc_macro2::TokenStream = generate_bindings("os")?;
///     Ok(())
/// }
/// ```
pub fn generate_bindings(module_name: &str) -> Result<proc_macro2::TokenStream, pyo3::PyErr> {
    #[cfg(not(PyPy))]
    pyo3::prepare_freethreaded_python();

    pyo3::Python::with_gil(|py| {
        let module = py.import(module_name)?;
        generate_bindings_for_module(py, module)
    })
}

/// Generate Rust bindings to an instance of `pyo3::types::PyModule` Python module.
///
/// # Arguments
///
/// * `py` - Python interpreter instance.
/// * `module` - Python module to generate bindings for.
///
/// # Returns
///
/// `Result` containing the generated bindings as a `proc_macro2::TokenStream` on success, or a
/// `pyo3::PyErr` on failure.
///
/// # Example
///
/// ```
/// // use pyo3_bindgen::generate_bindings_for_module;
/// use pyo3_bindgen_engine::generate_bindings_for_module;
///
/// fn main() -> Result<(), pyo3::PyErr> {
///     pyo3::prepare_freethreaded_python();
///     let bindings: proc_macro2::TokenStream = pyo3::Python::with_gil(|py| {
///         let module = py.import("os")?;
///         generate_bindings_for_module(py, module)
///     })?;
///     Ok(())
/// }
/// ```
pub fn generate_bindings_for_module(
    py: pyo3::Python,
    module: &pyo3::types::PyModule,
) -> Result<proc_macro2::TokenStream, pyo3::PyErr> {
    let all_types = module::collect_types_of_module(
        py,
        module,
        module,
        &mut std::collections::HashSet::new(),
        &mut std::collections::HashSet::default(),
    )?;

    bind_module(
        py,
        module,
        module,
        &mut std::collections::HashSet::new(),
        &all_types,
    )
}

/// Generate Rust bindings to a Python module specified by its `source_code`. The module will be
/// named `new_module_name` in the generated bindings. However, the generated bindings might not
/// be immediately functional if the module represented by its `source_code` is not a known Python
/// module in the current Python interpreter.
///
/// # Arguments
///
/// * `source_code` - Source code of the Python module to generate bindings for.
/// * `new_module_name` - Name of the Python module to generate bindings for.
///
/// # Returns
///
/// `Result` containing the generated bindings as a `proc_macro2::TokenStream` on success, or a
/// `pyo3::PyErr` on failure.
///
/// # Example
///
/// ```
/// // use pyo3_bindgen::generate_bindings_from_str;
/// use pyo3_bindgen_engine::generate_bindings_from_str;
///
/// fn main() -> Result<(), pyo3::PyErr> {
///     const PYTHON_SOURCE_CODE: &str = r#"
/// def string_length(string: str) -> int:
///     return len(string)
/// "#;
///     let bindings = generate_bindings_from_str(PYTHON_SOURCE_CODE, "utils")?;
///     Ok(())
/// }
/// ```
pub fn generate_bindings_from_str(
    source_code: &str,
    new_module_name: &str,
) -> Result<proc_macro2::TokenStream, pyo3::PyErr> {
    #[cfg(not(PyPy))]
    pyo3::prepare_freethreaded_python();

    pyo3::Python::with_gil(|py| {
        let module = pyo3::types::PyModule::from_code(
            py,
            source_code,
            &format!("{new_module_name}/__init__.py"),
            new_module_name,
        )?;
        generate_bindings_for_module(py, module)
    })
}
