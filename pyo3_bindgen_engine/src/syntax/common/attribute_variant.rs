use crate::{
    syntax::{Ident, Path},
    Result,
};

pub enum AttributeVariant {
    Import,
    Module,
    Class,
    Function,
    Method,
    Closure,
    TypeVar,
    Property,
}

impl AttributeVariant {
    pub fn determine(
        py: pyo3::prelude::Python,
        attr: &pyo3::prelude::PyAny,
        attr_type: &pyo3::types::PyType,
        attr_module: &Path,
        owner_name: &Path,
        consider_imported: bool,
    ) -> Result<Self> {
        let inspect = py.import("inspect")?;

        // Get the name and module of the attribute type
        let attr_type_name = Ident::from_py(attr_type.name().unwrap_or_default());
        let attr_type_module = Path::from_py(
            &attr_type
                .getattr(pyo3::intern!(py, "__module__"))
                .map(std::string::ToString::to_string)
                .unwrap_or_default(),
        );

        // Determine the type of the attribute
        let is_submodule = attr_type
            .is_subclass_of::<pyo3::types::PyModule>()
            .unwrap_or(false);
        let is_class = attr_type
            .is_subclass_of::<pyo3::types::PyType>()
            .unwrap_or(false);
        let is_function = inspect
            .call_method1(pyo3::intern!(py, "isfunction"), (attr,))?
            .is_true()?;
        let is_method = inspect
            .call_method1(pyo3::intern!(py, "ismethod"), (attr,))?
            .is_true()?;
        let is_closure =
            attr_type_module.to_py().as_str() == "functools" && attr_type_name.as_py() == "partial";
        let is_type = ["typing", "types"].contains(&attr_type_module.to_py().as_str());
        let is_external = attr_module != owner_name;
        let is_imported = is_external && (is_submodule || is_class || is_function || is_method);

        Ok(if consider_imported && is_imported {
            AttributeVariant::Import
        } else if is_submodule {
            AttributeVariant::Module
        } else if is_class {
            AttributeVariant::Class
        } else if is_function {
            AttributeVariant::Function
        } else if is_method {
            AttributeVariant::Method
        } else if is_closure {
            AttributeVariant::Closure
        } else if is_type {
            AttributeVariant::TypeVar
        } else {
            AttributeVariant::Property
        })
    }
}
