use crate::syntax::{Ident, Path};

#[derive(Debug, Clone, PartialEq, Eq, Hash, typed_builder::TypedBuilder)]
pub struct Config {
    #[builder(default = true)]
    pub generate_dependencies: bool,
    #[builder(default = true)]
    pub generate_preludes: bool,
    #[builder(default = true)]
    pub suppress_python_stdout: bool,
    // TODO: Default to false
    #[builder(default = true)]
    pub suppress_python_stderr: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self::builder().build()
    }
}

impl Config {
    pub fn is_attr_allowed(
        &self,
        _attr: &pyo3::types::PyAny,
        attr_name: &Ident,
        attr_module: &Path,
        attr_type: &pyo3::types::PyType,
    ) -> bool {
        if
        // Skip private attributes
        attr_name.as_py().starts_with('_') ||
        // Skip builtin functions
        attr_type.is_subclass_of::<pyo3::types::PyCFunction>().unwrap_or(false) ||
        // Skip `__future__` attributes
        attr_module.iter().any(|segment| segment.as_py() == "__future__") ||
        // Skip `typing` attributes
        attr_module.iter().any(|segment| segment.as_py() == "typing")
        {
            false
        } else {
            true
        }
    }
}
