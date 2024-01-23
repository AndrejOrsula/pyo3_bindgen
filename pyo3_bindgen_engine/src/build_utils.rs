//! Module with utilities for generating bindings in build scripts.

/// Convenience function for generating bindings in build scripts. This function is equivalent to
/// calling `generate_bindings` and writing the result to a file.
///
/// # Arguments
///
/// * `module_name` - Name of the Python module to generate bindings for.
/// * `output_path` - Path to write the generated bindings to.
///
/// # Returns
///
/// `Result` containing `std::io::Error` on failure.
///
/// # Example
///
/// 1. Generate bindings using `build.rs` script.
///
/// ```ignore
/// // build.rs
///
/// // use pyo3_bindgen::build_bindings;
/// use pyo3_bindgen_engine::build_bindings;
///
/// fn main() {
///     build_bindings(
///         "os",
///         std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("bindings.rs"),
///     )
///     .unwrap();
/// }
/// ```
///
/// 2. Include the generated bindings in `src/lib.rs`.
///
/// ```ignore
/// // src/lib.rs
///
/// include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
/// pub use os::*;
/// ```
// TODO: Add `println!("cargo:rerun-if-changed={}.py");` for all files of the target Python module
pub fn build_bindings(
    module_name: &str,
    output_path: impl AsRef<std::path::Path>,
) -> std::io::Result<()> {
    let bindings = crate::generate_bindings(module_name).map_err(|err| {
        std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("Failed to generate bindings for Python module '{module_name}': {err}"),
        )
    })?;
    std::fs::write(output_path, bindings.to_string())
}
