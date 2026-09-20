//! Procedural macros for automatic generation of Rust FFI bindings to Python modules.

mod parser;
mod utils;

/// Procedural macro for generating Rust bindings to Python modules in-place.
///
/// # Errors
///
/// If the module cannot be introspected or the bindings cannot be generated, the
/// macro expands to a `compile_error!` spanned at the call site (rather than panicking
/// the compiler), so the failure surfaces as an ordinary, located compile error.
///
/// # Security
///
/// `import_python!` imports and introspects Python during compile time. Module
/// initialization, descriptors, package hooks, and annotations can execute arbitrary
/// code with the compiler process's privileges. Use this macro only with trusted
/// Python environments; use a reviewed external `SchemaIr` when compile-time Python
/// execution is not acceptable. The macro has no external-IR mode.
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
/// // `os.path` resolves to `posixpath` on Unix and `ntpath` on Windows.
/// #[cfg(unix)]
/// pub use posixpath::*;
/// #[cfg(windows)]
/// pub use ntpath::*;
/// ```
#[proc_macro]
pub fn import_python(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let args = syn::parse_macro_input!(input as parser::Args);
    // A codegen failure becomes a `compile_error!` located at the call site instead of
    // an opaque proc-macro panic.
    expand(&args)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// Introspect the requested module and generate its bindings, mapping any engine error
/// to a call-site-spanned [`syn::Error`].
fn expand(args: &parser::Args) -> syn::Result<proc_macro2::TokenStream> {
    // On Unix, best-effort: (re)load libpython's symbols globally so C-extension modules
    // can resolve them. A failure here is NOT fatal — many modules (builtins, pure-Python
    // packages) need no global libpython symbols; if the requested module genuinely needs
    // them, the introspection below fails with its own call-site-located error. Surface
    // the preload failure as a warning rather than swallowing it silently or aborting.
    #[cfg(unix)]
    if let Err(err) = utils::try_load_libpython_symbols() {
        // On PyPy/GraalPy this global-symbol preload is inapplicable and fails on every
        // call, so a warning there is pure noise on every macro expansion rather than a
        // signal of a real problem. Surface it only on interpreters where it is
        // meaningful (CPython).
        #[cfg(not(any(PyPy, GraalPy)))]
        eprintln!("warning: pyo3_bindgen: failed to preload libpython symbols: {err}");
        #[cfg(any(PyPy, GraalPy))]
        let _ = &err;
    }

    let module_name = &args.module_name;
    let codegen = pyo3_bindgen_engine::Codegen::default()
        .module_name(module_name)
        .map_err(|err| {
            syn::Error::new_spanned(
                &args.literal,
                format!("failed to parse the content of the '{module_name}' Python module:\n{err}"),
            )
        })?;
    codegen.generate().map_err(|err| {
        syn::Error::new_spanned(
            &args.literal,
            format!("failed to generate bindings for the '{module_name}' Python module:\n{err}"),
        )
    })
}
