use super::{
    AttributeVariant, Function, FunctionType, Ident, MethodType, Path, Property, PropertyOwner,
};
use crate::{
    traits::{Canonicalize, Generate},
    Config, Result,
};
use itertools::Itertools;

#[derive(Debug, Clone)]
pub struct Class {
    pub name: Path,
    pub subclasses: Vec<Class>,
    pub methods: Vec<Function>,
    pub properties: Vec<Property>,
    pub docstring: Option<String>,
}

impl Class {
    pub fn parse(cfg: &Config, class: &pyo3::types::PyType, name: Path) -> Result<Self> {
        let py = class.py();

        // Initialize lists for all members of the class
        let mut subclasses = Vec::new();
        let mut methods = Vec::new();
        let mut properties = Vec::new();

        // Extract the list of all attribute names in the module
        class
            .dir()
            .iter()
            // Convert each attribute name to an identifier
            .map(|attr_name| Ident::from_py(&attr_name.to_string()))
            // Expand each attribute to a tuple of (attr, attr_name, attr_module, attr_type)
            .map(|attr_name| {
                let attr = class.getattr(attr_name.as_py()).unwrap_or_else(|_| {
                    unreachable!(
                        "Python object must always have attributes listed in its `__dir__`: {}",
                        attr_name
                    )
                });
                let attr_module = Path::from_py(
                    &attr
                        .getattr(pyo3::intern!(py, "__module__"))
                        .map(std::string::ToString::to_string)
                        .unwrap_or_default(),
                );
                let attr_type = attr.get_type();

                (attr, attr_name, attr_module, attr_type)
            })
            // Filter attributes based on various configurable conditions
            .filter(|(attr, attr_name, attr_module, attr_type)| {
                cfg.is_attr_allowed(attr, attr_name, attr_module, attr_type)
                    || ["__init__", "__call__"].contains(&attr_name.as_py())
            })
            // Iterate over the remaining attributes and parse them
            .try_for_each(|(attr, attr_name, attr_module, attr_type)| {
                match AttributeVariant::determine(py, attr, attr_type, &attr_module, &name, false)
                    .unwrap()
                {
                    AttributeVariant::Import => {
                        eprintln!("WARN: Imports in classes are not supported: {attr_name}");
                    }
                    AttributeVariant::Module => {
                        eprintln!("WARN: Submodules in classes are not supported: {attr_name}");
                    }
                    AttributeVariant::Class => {
                        let subclass =
                            Self::parse(cfg, attr.downcast().unwrap(), name.join(&attr_name))
                                .unwrap();
                        subclasses.push(subclass);
                    }
                    AttributeVariant::Function | AttributeVariant::Method => {
                        let method = Function::parse(
                            cfg,
                            attr,
                            name.join(&attr_name),
                            FunctionType::Method {
                                class_path: name.clone(),
                                typ: match attr_name.as_py() {
                                    "__init__" => MethodType::Constructor,
                                    "__call__" => MethodType::Call,
                                    _ => MethodType::Regular,
                                },
                            },
                        )
                        .unwrap();
                        methods.push(method);
                    }
                    AttributeVariant::Closure => {
                        eprintln!("WARN: Closures are not supported in classes: {attr_name}");
                    }
                    AttributeVariant::TypeVar => {
                        eprintln!("WARN: TypesVars are not supported in classes: {attr_name}");
                    }
                    AttributeVariant::Property => {
                        let property = Property::parse(
                            cfg,
                            attr,
                            name.join(&attr_name),
                            PropertyOwner::Class(name.clone()),
                        )
                        .unwrap();
                        properties.push(property);
                    }
                }
                Result::Ok(())
            })?;

        // Extract the docstring of the class
        let docstring = {
            let docstring = class.getattr(pyo3::intern!(py, "__doc__"))?.to_string();
            if docstring.is_empty() || docstring == "None" {
                None
            } else {
                Some(docstring)
            }
        };

        Ok(Self {
            name,
            subclasses,
            methods,
            properties,
            docstring,
        })
    }
}

impl Generate for Class {
    fn generate(&self, _cfg: &Config) -> Result<proc_macro2::TokenStream> {
        todo!()
    }
}

impl Canonicalize for Class {
    fn canonicalize(&mut self) {
        todo!()
    }
}
