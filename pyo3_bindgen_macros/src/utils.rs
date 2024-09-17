#[cfg(unix)]
use pyo3::prelude::*;

/// Ensure that the symbols of the libpython shared library are loaded globally.
///
/// # Explanation
///
/// On Unix, rustc loads proc-macro crates with `RTLD_LOCAL`, which (at least
/// on Linux) means all their dependencies (in this case: libpython) don't
/// get their symbols made available globally either. This means that loading
/// Python modules might fail, as these modules refer to symbols of libpython.
/// This function tries to (re)load the right version of libpython, but this
/// time with `RTLD_GLOBAL` enabled.
///
/// # Disclaimer
///
/// This function is adapted from the [inline-python](https://crates.io/crates/inline-python) crate
/// ([source code](https://github.com/fusion-engineering/inline-python/blob/24b04b59c0e7f059bbf319e7227054023b3fba55/macros/src/run.rs#L6-L25)).
#[cfg(unix)]
pub fn try_load_libpython_symbols() -> pyo3::PyResult<()> {
    #[cfg(not(PyPy))]
    pyo3::prepare_freethreaded_python();
    pyo3::Python::with_gil(|py| {
        let fn_get_config_var = py
            .import_bound(pyo3::intern!(py, "sysconfig"))?
            .getattr(pyo3::intern!(py, "get_config_var"))?;
        let libpython_dir = fn_get_config_var.call1(("LIBDIR",))?.to_string();
        let libpython_so_name = fn_get_config_var.call1(("INSTSONAME",))?.to_string();
        let libpython_so_path = std::path::Path::new(&libpython_dir).join(libpython_so_name);
        unsafe {
            libc::dlopen(
                std::ffi::CString::new(libpython_so_path.to_string_lossy().as_ref())?.as_ptr(),
                libc::RTLD_GLOBAL | libc::RTLD_NOW,
            );
        }
        Ok(())
    })
}
