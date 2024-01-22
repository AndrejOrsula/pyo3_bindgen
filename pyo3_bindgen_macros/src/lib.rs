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
/// // use pyo3_bindgen::import_python;
/// use pyo3_bindgen_macros::import_python;
///
/// import_python!("sys");
/// pub use sys::*;
///
/// import_python!("os.path");
/// pub use path::*;
/// ```
#[proc_macro]
pub fn import_python(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let parser::Args { module_name_py } = syn::parse_macro_input!(input as parser::Args);

    // Generate the bindings
    pyo3_bindgen_engine::generate_bindings(&module_name_py)
        .unwrap_or_else(|_| panic!("Failed to generate bindings for module: {module_name_py}"))
        .into()
}
