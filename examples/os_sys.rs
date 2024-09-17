//! Example demonstrating the use of the `pyo3_bindgen` crate via build script for
//! the "os", "posixpath", and "sys" Python modules.
//!
//! See `build.rs` for more details about the generation.
//!
//! Python equivalent:
//!
//! ```py
//! import os
//! import posixpath
//! import sys
//!
//! python_exe_path = sys.executable
//! current_dir = os.getcwd()
//! relpath_to_python_exe = posixpath.relpath(python_exe_path, current_dir)
//!
//! print(f"Relative path to Python executable: '{relpath_to_python_exe}'")
//! ```

include!(concat!(env!("OUT_DIR"), "/bindings.rs"));

fn main() -> pyo3::PyResult<()> {
    pyo3::Python::with_gil(|py| {
        // Get the path to the Python executable via "sys" Python module
        let python_exe_path = sys::executable(py)?;
        // Get the current working directory via "os" Python module
        let current_dir = os::getcwd(py)?;
        // Get the relative path to the Python executable via "posixpath" Python module
        let relpath_to_python_exe = posixpath::relpath(py, python_exe_path, Some(current_dir))?;

        println!("Relative path to Python executable: '{relpath_to_python_exe}'");
        Ok(())
    })
}
