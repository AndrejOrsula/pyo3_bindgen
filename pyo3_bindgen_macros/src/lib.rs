//! Procedural macros for automatic generation of Rust FFI bindings to Python modules.

mod parser;
mod utils;

/// Procedural macro for generating Rust bindings to Python modules in-place.
///
/// # Panics
///
/// Panics if the bindings cannot be generated.
///
/// # Examples
///
/// Here is a simple example of how to use the macro to generate bindings for the `sys` module.
///
/// ```
/// # use pyo3_bindgen_macros::import_python;
/// import_python!("sys");
/// pub use sys::*;
/// ```
///
/// For consistency, the top-level package is always included in the generated bindings.
///
/// ```
/// # use pyo3_bindgen_macros::import_python;
/// import_python!("html.parser");
/// pub use html::parser::*;
/// ```
///
/// Furthermore, the actual name of the package is always used regardless of how it is aliased.
///
/// ```
/// # use pyo3_bindgen_macros::import_python;
/// import_python!("os.path");
/// pub use posixpath::*;
/// ```
#[proc_macro]
pub fn import_python(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    // Parse the macro arguments
    let parser::Args { module_name } = syn::parse_macro_input!(input as parser::Args);

    // On Unix systems, ensure that the symbols of the libpython shared library are loaded globally
    #[cfg(unix)]
    utils::try_load_libpython_symbols().unwrap_or_else(|err| {
        eprintln!(
            "Failed to load libpython symbols, code generation might not work as expected:\n{err}"
        );
    });

    // Generate the bindings
    pyo3_bindgen_engine::Codegen::default()
        .module_name(&module_name)
        .unwrap_or_else(|err| {
            panic!("Failed to parse the content of '{module_name}' Python module:\n{err}")
        })
        .generate()
        .unwrap_or_else(|err| {
            panic!("Failed to generate bindings for '{module_name}' Python module:\n{err}")
        })
        .into()
}
