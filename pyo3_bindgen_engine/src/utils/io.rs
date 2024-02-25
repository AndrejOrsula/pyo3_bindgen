use crate::Result;

pub fn with_suppressed_python_output<T>(
    py: pyo3::Python,
    suppress_stdout: bool,
    suppress_stderr: bool,
    f: impl FnOnce() -> Result<T>,
) -> Result<T> {
    // If both stdout and stderr are suppressed, there's no need to do anything
    if !suppress_stdout && !suppress_stderr {
        return f();
    }

    let sys = py.import(pyo3::intern!(py, "sys"))?;
    let stdout_ident = pyo3::intern!(py, "stdout");
    let stderr_ident = pyo3::intern!(py, "stderr");

    // Record the original stdout and stderr
    let original_stdout = sys.getattr(stdout_ident)?;
    let original_stderr = sys.getattr(stderr_ident)?;

    // Suppress the output
    let supressed_output = py.eval(r"lambda: type('SupressedOutput', (), {'write': lambda self, x: None, 'flush': lambda self: None})", None, None)?;
    if suppress_stdout {
        sys.setattr(stdout_ident, supressed_output)?;
    }
    if suppress_stderr {
        sys.setattr(stderr_ident, supressed_output)?;
    }

    // Run the function
    let ret = f()?;

    // Restore the original stdout and stderr
    if suppress_stdout {
        sys.setattr(stdout_ident, original_stdout)?;
    }
    if suppress_stderr {
        sys.setattr(stderr_ident, original_stderr)?;
    }

    Ok(ret)
}
