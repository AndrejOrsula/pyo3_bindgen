use crate::syntax::{Ident, Path};

/// Array of forbidden attribute names that are reserved for internal use by derived traits
pub const FORBIDDEN_FUNCTION_NAMES: [&str; 5] = ["get_type", "obj", "py", "repr", "str"];
/// Array of forbidden type names
pub const FORBIDDEN_TYPE_NAMES: [&str; 7] = [
    "_collections._tuplegetter",
    "AsyncState",
    "getset_descriptor",
    "member_descriptor",
    "method_descriptor",
    "property",
    "py",
];

/// Default array of blocklisted attribute names
const DEFAULT_BLOCKLIST_ATTRIBUTE_NAMES: [&str; 4] = ["builtins", "testing", "tests", "test"];

/// Configuration for `Codegen` engine.
#[derive(Debug, Clone, PartialEq, Eq, Hash, typed_builder::TypedBuilder)]
pub struct Config {
    /// Flag that determines whether to recursively generate code for all submodules of the target modules.
    #[builder(default = true)]
    pub traverse_submodules: bool,

    /// Flag that determines whether to generate code for prelude modules (Python `__all__` attribute).
    #[builder(default = true)]
    pub generate_preludes: bool,
    /// Flag that determines whether to generate code for imports.
    #[builder(default = true)]
    pub generate_imports: bool,
    /// Flag that determines whether to generate code for classes.
    #[builder(default = true)]
    pub generate_classes: bool,
    /// Flag that determines whether to generate code for type variables.
    #[builder(default = true)]
    pub generate_type_vars: bool,
    /// Flag that determines whether to generate code for functions.
    #[builder(default = true)]
    pub generate_functions: bool,
    /// Flag that determines whether to generate code for properties.
    #[builder(default = true)]
    pub generate_properties: bool,
    /// Flag that determines whether to documentation for the generate code.
    /// The documentation is based on Python docstrings.
    #[builder(default = true)]
    pub generate_docs: bool,

    /// List of blocklisted attribute names that are skipped during the code generation.
    #[builder(default = DEFAULT_BLOCKLIST_ATTRIBUTE_NAMES.iter().map(|&s| s.to_string()).collect())]
    pub blocklist_names: Vec<String>,
    /// Flag that determines whether private attributes are considered while parsing the Python code.
    #[builder(default = false)]
    pub include_private: bool,

    /// Flag that determines whether to generate code for all dependencies of the target modules.
    /// The list of dependent modules is derived from the imports of the target modules.
    ///
    /// Warning: This feature is not fully supported yet.
    #[builder(default = false)]
    pub generate_dependencies: bool,

    /// Flag that suppresses the generation of Python STDOUT while parsing the Python code.
    #[builder(default = true)]
    pub suppress_python_stdout: bool,
    /// Flag that suppresses the generation of Python STDERR while parsing the Python code.
    #[builder(default = true)]
    pub suppress_python_stderr: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self::builder().build()
    }
}

impl Config {
    pub(crate) fn is_attr_allowed(
        &self,
        attr_name: &Ident,
        attr_module: &Path,
        _attr_type: &pyo3::types::PyType,
    ) -> bool {
        if
        // Skip always forbidden attribute names
        FORBIDDEN_FUNCTION_NAMES.contains(&attr_name.as_py()) ||
        // Skip private attributes if `include_private` is disabled
        (!self.include_private &&
            (attr_name.as_py().starts_with('_') ||
             attr_module.iter().any(|segment| segment.as_py().starts_with('_')))) ||
        // Skip blocklisted attributes
        self.blocklist_names.iter().any(|blocklist_match| {
            attr_name.as_py() == blocklist_match
        }) ||
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
