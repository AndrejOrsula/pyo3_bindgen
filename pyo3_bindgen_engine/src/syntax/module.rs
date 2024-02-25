use super::{
    AttributeVariant, Class, Function, FunctionType, Ident, Import, Path, Property, PropertyOwner,
    TypeVar,
};
use crate::{
    traits::{Canonicalize, Generate},
    Config, Result,
};
use itertools::Itertools;
use rustc_hash::FxHashSet as HashSet;

#[derive(Debug, Clone)]
pub struct Module {
    pub name: Path,
    pub prelude: Vec<Ident>,
    pub imports: Vec<Import>,
    pub submodules: Vec<Module>,
    pub classes: Vec<Class>,
    pub functions: Vec<Function>,
    pub type_vars: Vec<TypeVar>,
    pub properties: Vec<Property>,
    pub docstring: Option<String>,
}

impl Module {
    pub fn parse(cfg: &Config, module: &pyo3::types::PyModule) -> Result<Self> {
        let py = module.py();

        // Extract the name of the module
        let name = Path::from_py(module.name().unwrap());

        // Extract the index of the module as prelude (if enabled)
        let prelude = if cfg.generate_preludes {
            Self::extract_prelude(module)
        } else {
            Vec::new()
        };

        // Extract the list of all submodules in the module
        let mut submodules_to_process = Self::extract_submodules(module).unwrap();

        // Initialize lists for all other members of the module
        let mut imports = Vec::new();
        let mut classes = Vec::new();
        let mut functions = Vec::new();
        let mut type_vars = Vec::new();
        let mut properties = Vec::new();

        // Extract the list of all attribute names in the module
        module
            .dir()
            .iter()
            // Convert each attribute name to an identifier
            .map(|attr_name| Ident::from_py(&attr_name.to_string()))
            // Expand each attribute to a tuple of (attr, attr_name, attr_module, attr_type)
            .map(|attr_name| {
                let attr = module.getattr(attr_name.as_py()).unwrap_or_else(|_| {
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
            })
            // Iterate over the remaining attributes and parse them
            .try_for_each(|(attr, attr_name, attr_module, attr_type)| {
                match AttributeVariant::determine(py, attr, attr_type, &attr_module, &name, true)
                    .unwrap()
                {
                    AttributeVariant::Import => {
                        let import = Import::new(
                            attr_module.join(&Ident::from_py(
                                &attr
                                    .getattr(pyo3::intern!(py, "__name__"))
                                    .map(std::string::ToString::to_string)
                                    .unwrap_or(attr_name.as_py().to_owned()),
                            )),
                            name.join(&attr_name),
                        )
                        .unwrap();
                        imports.push(import);
                    }
                    AttributeVariant::Module => {
                        // Note: This should technically not be necessary as `Self::extract_submodules` is supposed to extract all submodules
                        submodules_to_process.insert(attr_name);
                    }
                    AttributeVariant::Class => {
                        let class =
                            Class::parse(cfg, attr.downcast().unwrap(), name.join(&attr_name))
                                .unwrap();
                        classes.push(class);
                    }
                    AttributeVariant::Function | AttributeVariant::Method => {
                        let function = Function::parse(
                            cfg,
                            attr,
                            name.join(&attr_name),
                            FunctionType::Function,
                        )
                        .unwrap();
                        functions.push(function);
                    }
                    AttributeVariant::Closure => {
                        let function = Function::parse(
                            cfg,
                            attr,
                            name.join(&attr_name),
                            FunctionType::Closure(name.join(&attr_name)),
                        )
                        .unwrap();
                        functions.push(function);
                    }
                    AttributeVariant::TypeVar => {
                        let type_var = TypeVar::new(name.join(&attr_name)).unwrap();
                        type_vars.push(type_var);
                    }
                    AttributeVariant::Property => {
                        let property = Property::parse(
                            cfg,
                            attr,
                            name.join(&attr_name),
                            PropertyOwner::Module(name.clone()),
                        )
                        .unwrap();
                        properties.push(property);
                    }
                }
                Result::Ok(())
            })?;

        // Process submodules
        let submodules = submodules_to_process
            .into_iter()
            .filter_map(|submodule_name| {
                py.import(name.join(&submodule_name).to_py().as_str()).ok()
            })
            .map(|submodule| Self::parse(cfg, submodule))
            .collect::<Result<_>>()
            .unwrap();

        // Extract the docstring of the module
        let docstring = {
            let docstring = module.getattr(pyo3::intern!(py, "__doc__"))?.to_string();
            if docstring.is_empty() || docstring == "None" {
                None
            } else {
                Some(docstring)
            }
        };

        Ok(Self {
            name,
            prelude,
            imports,
            submodules,
            classes,
            functions,
            type_vars,
            properties,
            docstring,
        })
    }

    fn extract_prelude(module: &pyo3::prelude::PyModule) -> Vec<Ident> {
        // Extract the index (__all__) of the module if it exists
        let index_attr_names = if let Ok(index) = module.index() {
            index
                .iter()
                .map(|x| Ident::from_py(&x.to_string()))
                .collect_vec()
        } else {
            Vec::new()
        };

        // Compare the index with public attrs of the module
        // Return an empty vector if they are identical (no need to generate a prelude)
        {
            let public_attr_names_set: HashSet<_> = module
                .dir()
                .iter()
                .map(|attr_name| Ident::from_py(&attr_name.to_string()))
                .filter(|attr_name| !attr_name.as_py().starts_with('_'))
                .collect();
            let index_attr_names_set = index_attr_names.iter().cloned().collect::<HashSet<_>>();

            if index_attr_names_set == public_attr_names_set {
                return Vec::new();
            }
        }

        index_attr_names
    }

    fn extract_submodules(module: &pyo3::prelude::PyModule) -> Result<HashSet<Ident>> {
        let py = module.py();
        let pkgutil = py.import(pyo3::intern!(py, "pkgutil")).unwrap();

        // Determine if the module is a package that contains submodules
        let module_name = Path::from_py(module.name().unwrap());
        let is_pkg = module
            .getattr(pyo3::intern!(py, "__package__"))
            .map(|package| Path::from_py(&package.to_string()))
            .is_ok_and(|package_name| package_name == module_name);

        // If the module is not a package, return an empty set
        if !is_pkg {
            return Ok(HashSet::default());
        }

        // Extract the paths of the module
        let module_paths = module
            .getattr(pyo3::intern!(py, "__path__"))
            .unwrap()
            .extract::<&pyo3::types::PyList>()
            .unwrap()
            .iter()
            .map(|x| std::path::PathBuf::from(x.to_string()))
            .collect_vec();

        // Extract the names of all submodules via `pkgutil.iter_modules`
        pkgutil
            .call_method1(pyo3::intern!(py, "iter_modules"), (module_paths,))
            .unwrap()
            .iter()
            .unwrap()
            .map(|submodule| {
                Ok(Ident::from_py(
                    &submodule
                        .unwrap()
                        .getattr(pyo3::intern!(py, "name"))
                        .unwrap()
                        .to_string(),
                ))
            })
            .collect()
    }
}

impl Generate for Module {
    fn generate(&self, _cfg: &Config) -> Result<proc_macro2::TokenStream> {
        todo!()
    }
}

impl Canonicalize for Module {
    fn canonicalize(&mut self) {
        todo!()
    }
}
