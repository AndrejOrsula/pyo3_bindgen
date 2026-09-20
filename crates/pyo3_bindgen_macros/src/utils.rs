#[cfg(all(unix, not(any(PyPy, GraalPy))))]
use pyo3::prelude::*;

#[cfg(all(unix, not(any(PyPy, GraalPy))))]
use std::os::unix::ffi::OsStrExt;

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
#[cfg(all(unix, not(any(PyPy, GraalPy))))]
pub fn try_load_libpython_symbols() -> pyo3::PyResult<()> {
    static PRELOAD: std::sync::OnceLock<Result<(), String>> = std::sync::OnceLock::new();

    match PRELOAD.get_or_init(preload_libpython_symbols) {
        Ok(()) => Ok(()),
        Err(error) => Err(pyo3::exceptions::PyRuntimeError::new_err(error.clone())),
    }
}

#[cfg(all(unix, any(PyPy, GraalPy)))]
pub fn try_load_libpython_symbols() -> pyo3::PyResult<()> {
    let implementation = if cfg!(PyPy) { "PyPy" } else { "GraalPy" };
    Err(pyo3::exceptions::PyRuntimeError::new_err(format!(
        "libpython symbol preloading requires an embedded CPython interpreter; {implementation} live embedding is unsupported"
    )))
}

#[cfg(all(unix, not(any(PyPy, GraalPy))))]
fn preload_libpython_symbols() -> Result<(), String> {
    pyo3::Python::initialize();
    pyo3::Python::attach(|py| {
        let get_config_var = py
            .import(pyo3::intern!(py, "sysconfig"))
            .and_then(|module| module.getattr(pyo3::intern!(py, "get_config_var")))
            .map_err(|error| format!("failed to inspect Python's sysconfig: {error}"))?;

        let lib_dir = config_var_string(&get_config_var, "LIBDIR")?;
        let install_name = config_var_string(&get_config_var, "INSTSONAME")?;
        let framework = config_var_string(&get_config_var, "PYTHONFRAMEWORK")?;
        let framework_prefix = config_var_string(&get_config_var, "PYTHONFRAMEWORKPREFIX")?;
        let path = libpython_path(
            lib_dir.as_deref(),
            install_name.as_deref(),
            framework.as_deref(),
            framework_prefix.as_deref(),
        )?;

        load_libpython_global(&path)
    })
}

#[cfg(all(unix, not(any(PyPy, GraalPy))))]
fn config_var_string(
    get_config_var: &pyo3::Bound<'_, pyo3::types::PyAny>,
    name: &str,
) -> Result<Option<String>, String> {
    let value = get_config_var
        .call1((name,))
        .map_err(|error| format!("sysconfig.get_config_var({name:?}) failed: {error}"))?;
    if value.is_none() {
        Ok(None)
    } else {
        value
            .extract::<String>()
            .map(Some)
            .map_err(|error| format!("sysconfig variable {name} is not a string: {error}"))
    }
}

#[cfg(any(test, all(unix, not(any(PyPy, GraalPy)))))]
fn libpython_path(
    lib_dir: Option<&str>,
    install_name: Option<&str>,
    framework: Option<&str>,
    framework_prefix: Option<&str>,
) -> Result<std::path::PathBuf, String> {
    let install_name = install_name
        .filter(|name| !name.is_empty())
        .ok_or_else(|| "Python sysconfig did not provide INSTSONAME".to_owned())?;
    let install_name = std::path::Path::new(install_name);

    if install_name.extension() == Some(std::ffi::OsStr::new("a")) {
        return Err(format!(
            "Python exposes only the static libpython archive '{}'; static archives cannot be preloaded with dlopen",
            install_name.display()
        ));
    }

    if install_name.is_absolute() {
        return Ok(install_name.to_owned());
    }

    if framework.is_some_and(|name| !name.is_empty()) {
        let prefix = framework_prefix
            .filter(|prefix| !prefix.is_empty())
            .ok_or_else(|| {
                "Python is a macOS framework build but sysconfig did not provide PYTHONFRAMEWORKPREFIX"
                    .to_owned()
            })?;
        return Ok(std::path::Path::new(prefix).join(install_name));
    }

    let lib_dir = lib_dir
        .filter(|directory| !directory.is_empty())
        .ok_or_else(|| "Python sysconfig did not provide LIBDIR".to_owned())?;
    Ok(std::path::Path::new(lib_dir).join(install_name))
}

#[cfg(all(unix, not(any(PyPy, GraalPy))))]
fn load_libpython_global(path: &std::path::Path) -> Result<(), String> {
    let path_c = std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|_| {
        format!(
            "libpython path '{}' contains an interior NUL byte",
            path.display()
        )
    })?;

    // SAFETY: `path_c` is NUL-terminated and remains alive for the call. The successful
    // handle is intentionally retained for the process lifetime: `dlclose` could remove
    // the global symbols while subsequently imported Python extension modules use them.
    let handle = unsafe { libc::dlopen(path_c.as_ptr(), libc::RTLD_GLOBAL | libc::RTLD_NOW) };
    if handle.is_null() {
        // SAFETY: `dlerror` returns either null or a NUL-terminated thread-local error string.
        let detail = unsafe {
            let error = libc::dlerror();
            if error.is_null() {
                "unknown dynamic-loader error".to_owned()
            } else {
                std::ffi::CStr::from_ptr(error)
                    .to_string_lossy()
                    .into_owned()
            }
        };
        return Err(format!(
            "dlopen failed to load libpython from '{}': {detail}",
            path.display()
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linux_shared_library_uses_libdir() {
        assert_eq!(
            libpython_path(Some("/usr/lib"), Some("libpython3.14.so.1.0"), None, None,).unwrap(),
            std::path::PathBuf::from("/usr/lib/libpython3.14.so.1.0")
        );
    }

    #[test]
    fn macos_framework_uses_framework_prefix_for_install_name() {
        assert_eq!(
            libpython_path(
                Some("/Library/Frameworks/Python.framework/Versions/3.14/lib"),
                Some("Python.framework/Versions/3.14/Python"),
                Some("Python"),
                Some("/Library/Frameworks"),
            )
            .unwrap(),
            std::path::PathBuf::from("/Library/Frameworks/Python.framework/Versions/3.14/Python")
        );
    }

    #[test]
    fn absolute_install_name_does_not_require_libdir() {
        assert_eq!(
            libpython_path(None, Some("/opt/python/lib/libpython3.14.so"), None, None).unwrap(),
            std::path::PathBuf::from("/opt/python/lib/libpython3.14.so")
        );
    }

    #[test]
    fn static_archive_fails_informatively() {
        let error = libpython_path(Some("/opt/python/lib"), Some("libpython3.14.a"), None, None)
            .unwrap_err();

        assert!(error.contains("static libpython archive"), "{error}");
    }

    #[test]
    fn incomplete_framework_metadata_fails_informatively() {
        let error = libpython_path(
            Some("/Library/Frameworks/Python.framework/Versions/3.14/lib"),
            Some("Python.framework/Versions/3.14/Python"),
            Some("Python"),
            None,
        )
        .unwrap_err();

        assert!(error.contains("PYTHONFRAMEWORKPREFIX"), "{error}");
    }
}
