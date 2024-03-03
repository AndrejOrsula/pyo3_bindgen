//! Procedural macros for automatic generation of Rust FFI bindings to Python modules.

mod parser;

/// Procedural macro for generating Rust bindings to Python modules in-place.
///
/// # Panics
///
/// Panics if the bindings cannot be generated.
///
/// # Example
///
/// ```ignore
/// import_python!("sys");
/// pub use sys::*;
///
/// // The top-level package is always included in the generated bindings for consistency
/// import_python!("mod.submod.subsubmod");
/// pub use mod::submod::subsubmod::*;
///
/// // The actual name of the package is always used, regardless of how it is aliased
/// import_python!("os.path");
/// pub use posixpath::*;
/// ```
#[proc_macro]
pub fn import_python(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let parser::Args { module_name } = syn::parse_macro_input!(input as parser::Args);

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
