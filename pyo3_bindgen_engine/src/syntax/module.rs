use super::{
    AttributeVariant, Class, Function, FunctionType, Ident, Import, Path, Property, PropertyOwner,
    TypeVar,
};
use crate::{Config, Result};
use itertools::Itertools;
use rustc_hash::FxHashSet as HashSet;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Module {
    pub name: Path,
    pub prelude: Vec<Ident>,
    pub imports: Vec<Import>,
    pub submodules: Vec<Module>,
    pub classes: Vec<Class>,
    pub type_vars: Vec<TypeVar>,
    pub functions: Vec<Function>,
    pub properties: Vec<Property>,
    pub docstring: Option<String>,
    pub is_package: bool,
}

impl Module {
    pub fn empty(py: pyo3::Python, name: Path) -> Result<Self> {
        let module = py.import(name.to_py().as_str())?;

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
            prelude: Vec::default(),
            imports: Vec::default(),
            submodules: Vec::default(),
            classes: Vec::default(),
            type_vars: Vec::default(),
            functions: Vec::default(),
            properties: Vec::default(),
            docstring,
            is_package: true,
        })
    }

    pub fn parse(cfg: &Config, module: &pyo3::types::PyModule) -> Result<Self> {
        let py = module.py();

        // Extract the name of the module
        let name = Path::from_py(module.name()?);

        // Extract the index of the module as prelude (if enabled)
        let prelude = if cfg.generate_preludes {
            Self::extract_prelude(cfg, module, &name)
        } else {
            Vec::new()
        };

        // Determine if the module is a package that contains submodules
        let is_package = module.hasattr(pyo3::intern!(py, "__path__"))?;

        // Extract the list of all submodules for packages
        let mut submodules_to_process = if is_package {
            Self::extract_submodules(cfg, module)?
        } else {
            HashSet::default()
        };

        // Initialize lists for all other members of the module
        let mut imports = Vec::new();
        let mut conflicting_imports = Vec::new();
        let mut classes: Vec<Class> = Vec::new();
        let mut type_vars = Vec::new();
        let mut functions = Vec::new();
        let mut properties = Vec::new();

        // Extract the list of all attribute names in the module
        module
            .dir()
            .iter()
            // Convert each attribute name to an identifier
            .map(|attr_name| Ident::from_py(&attr_name.to_string()))
            // Remove duplicates
            .unique()
            // TODO: Try to first access the attribute via __dict__ because Python's descriptor protocol might change the attributes obtained via getattr()
            //       - For example, classmethod and staticmethod are converted to method/function
            //       - However, this might also change some of the parsing and it would need to be fixed
            // Expand each attribute to a tuple of (attr, attr_name, attr_module, attr_type)
            .filter_map(|attr_name| {
                if let Ok(attr) = module.getattr(attr_name.as_py()) {

                    let attr_module = Path::from_py(
                        &attr
                        .getattr(pyo3::intern!(py, "__module__"))
                        .map(std::string::ToString::to_string)
                        .unwrap_or_default(),
                    );
                    let attr_type = attr.get_type();

                    Some((attr, attr_name, attr_module, attr_type))
                } else {
                    eprintln!(
                        "WARN: Cannot get attribute '{attr_name}' of '{name}' even though it is listed in its `__dir__`. Bindings will not be generated.",
                    );
                    None
                }
            })
            // Filter attributes based on various configurable conditions
            .filter(|(_attr, attr_name, attr_module, attr_type)| {
                cfg.is_attr_allowed(attr_name, attr_module, attr_type)
            })
            // Iterate over the remaining attributes and parse them
            .try_for_each(|(attr, attr_name, attr_module, attr_type)| {
                let attr_name_full = name.join(&attr_name.clone().into());
                match AttributeVariant::determine(py, attr, attr_type, &attr_module, &name, true)
                    ?
                {
                    AttributeVariant::Import => {
                        let origin = attr_module.join(&Path::from_py(
                            &attr
                                .getattr(pyo3::intern!(py, "__name__"))
                                .map(std::string::ToString::to_string)
                                .unwrap_or(attr_name.as_py().to_owned()),
                        ));

                        // Skip if the origin is the same as the target
                        if origin == attr_name_full {
                            return Ok(());
                        }

                        // Make sure the origin attribute is allowed (each segment of the path)
                        let is_origin_attr_allowed = (0..origin.len()).all(|i| {
                            let attr_name = &origin[i];
                            let attr_module = origin[..i].into();
                            let attr_type = if i == origin.len() - 1 {
                                attr_type
                            } else {
                                py.get_type::<pyo3::types::PyModule>()
                            };
                            cfg.is_attr_allowed(attr_name, &attr_module, attr_type)
                        });
                        if !is_origin_attr_allowed {
                            return Ok(());
                        }

                        // Determine if the import overwrites a submodule
                        let import_overwrites_submodule = submodules_to_process.contains(&attr_name);

                        // Generate the import
                        let import = Import::new(origin, attr_name_full);

                        // Add the import to the appropriate list
                        if import_overwrites_submodule {
                            conflicting_imports.push(import);
                        } else {
                            imports.push(import);
                        }
                    }
                    AttributeVariant::Module => {
                        // Note: This should technically not be necessary as `Self::extract_submodules` is supposed to extract all submodules
                        submodules_to_process.insert(attr_name.clone());
                    }
                    AttributeVariant::Class => {
                        let class =
                            Class::parse(cfg, attr.downcast().unwrap_or_else(|_| unreachable!(
                                "The attribute is known to be a class at this point"
                            )), attr_name_full)?;
                        classes.push(class);
                    }
                    AttributeVariant::TypeVar => {
                        let type_var = TypeVar::new(attr_name_full);
                        type_vars.push(type_var);
                    }
                    AttributeVariant::Function => {
                        let function =
                            Function::parse(cfg, attr, attr_name_full, FunctionType::Function)
                                ?;
                        functions.push(function);
                    }
                    AttributeVariant::Method => {
                        eprintln!("WARN: Methods in modules are not supported: '{name}.{attr_name}'. Bindings will not be generated.");
                    }
                    AttributeVariant::Closure => {
                        let function =
                            Function::parse(cfg, attr, attr_name_full, FunctionType::Closure)
                                ?;
                        functions.push(function);
                    }
                    AttributeVariant::Property => {
                        let property = Property::parse(
                            cfg,
                            attr,
                            attr_name_full,
                            PropertyOwner::Module,
                        )
                        ?;
                        properties.push(property);
                    }
                }
                Result::Ok(())
            })?;

        // Process submodules
        let submodules = if cfg.traverse_submodules {
            submodules_to_process
                .into_iter()
                .filter_map(|submodule_name| {
                    let full_submodule_name = name.join(&submodule_name.clone().into());

                    // Handle submodules that are overwritten by imports separately
                    if let Some(conflicting_import) = conflicting_imports
                        .iter()
                        .find(|import| import.target == full_submodule_name)
                    {
                        if let Ok(submodule) = py
                            .import(full_submodule_name.to_py().as_str())
                            .map_err(crate::PyBindgenError::from)
                            .and_then(|attr| Ok(attr.downcast::<pyo3::types::PyModule>()?))
                            .and_then(|module| Self::parse(cfg, module))
                        {
                            // It could be any attribute, so all of them need to be checked
                            if let Some(mut import) = submodule
                                .imports
                                .into_iter()
                                .find(|import| import.target == conflicting_import.origin)
                            {
                                import.target = conflicting_import.target.clone();
                                imports.push(import);
                            }
                            if let Some(mut class) = submodule
                                .classes
                                .into_iter()
                                .find(|class| class.name == conflicting_import.origin)
                            {
                                class.name = conflicting_import.target.clone();
                                classes.push(class);
                            }
                            if let Some(mut type_var) = submodule
                                .type_vars
                                .into_iter()
                                .find(|type_var| type_var.name == conflicting_import.origin)
                            {
                                type_var.name = conflicting_import.target.clone();
                                type_vars.push(type_var);
                            }
                            if let Some(mut function) = submodule
                                .functions
                                .into_iter()
                                .find(|function| function.name == conflicting_import.origin)
                            {
                                function.name = conflicting_import.target.clone();
                                functions.push(function);
                            }
                            if let Some(mut property) = submodule
                                .properties
                                .into_iter()
                                .find(|property| property.name == conflicting_import.origin)
                            {
                                property.name = conflicting_import.target.clone();
                                properties.push(property);
                            }
                        }
                        return None;
                    }

                    // Try to import both as a package and as a attribute of the current module
                    py.import(full_submodule_name.to_py().as_str())
                        .or_else(|_| {
                            module
                                .getattr(submodule_name.as_py())
                                .and_then(|attr| Ok(attr.downcast::<pyo3::types::PyModule>()?))
                        })
                        .ok()
                })
                .map(|submodule| Self::parse(cfg, submodule))
                .collect::<Result<_>>()?
        } else {
            Vec::default()
        };

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
            type_vars,
            functions,
            properties,
            docstring,
            is_package,
        })
    }

    pub fn generate(
        &self,
        cfg: &Config,
        top_level_modules: &[Self],
        all_types: &[Path],
    ) -> Result<proc_macro2::TokenStream> {
        let mut output = proc_macro2::TokenStream::new();

        // Extra configuration for top-level modules
        let is_top_level = top_level_modules.contains(self);
        if is_top_level {
            output.extend(quote::quote! {
                #[allow(
                    clippy::all,
                    clippy::nursery,
                    clippy::pedantic,
                    non_camel_case_types,
                    non_snake_case,
                    non_upper_case_globals,
                    unused
                )]
            });
        }

        // Documentation
        if cfg.generate_docs {
            if let Some(mut docstring) = self.docstring.clone() {
                crate::utils::text::format_docstring(&mut docstring);
                output.extend(quote::quote! {
                    #[doc = #docstring]
                });
            }
        }

        // Get the names of all functions to avoid name clashes
        let scoped_function_idents = self
            .functions
            .iter()
            .map(|function| function.name.name())
            .collect::<Vec<_>>();

        // Get all local types mapped to the full path
        let local_types = all_types
            .iter()
            .cloned()
            .map(|path| {
                let relative_path = self.name.relative_to(&path, false);
                (path, relative_path)
            })
            .chain(self.imports.iter().flat_map(|import| {
                all_types
                    .iter()
                    .filter(|&path| path.starts_with(&import.origin))
                    .cloned()
                    .map(|path| {
                        let imported_path = {
                            if let Some(stripped_path) = path
                                .to_py()
                                .strip_prefix(&format!("{}.", import.origin.to_py()))
                            {
                                let mut path = Path::from_py(stripped_path);
                                // Overwrite the first segment with the target name to support aliasing
                                path[0] = import.target.name().to_owned();
                                path
                            } else {
                                import.target.name().to_owned().into()
                            }
                        };
                        let relative_path = self.name.relative_to(&path, false);
                        (imported_path, relative_path)
                    })
            }))
            .collect();

        // Generate the module content
        let mut module_content = proc_macro2::TokenStream::new();
        // Imports
        if cfg.generate_imports {
            module_content.extend(
                self.imports
                    .iter()
                    .filter(|import| {
                        top_level_modules
                            .iter()
                            .any(|module| module.check_path_exists_recursive(&import.origin, false))
                    })
                    .map(|import| import.generate(cfg))
                    .collect::<Result<proc_macro2::TokenStream>>()?,
            );
        }
        // Prelude
        if cfg.generate_preludes {
            module_content.extend(self.generate_prelude());
        }
        // Type variables
        if cfg.generate_type_vars {
            module_content.extend(
                self.type_vars
                    .iter()
                    .map(|type_var| type_var.generate(cfg))
                    .collect::<Result<proc_macro2::TokenStream>>()?,
            );
        }
        // Classes
        if cfg.generate_classes {
            module_content.extend(
                self.classes
                    .iter()
                    .map(|class| class.generate(cfg, &local_types))
                    .collect::<Result<proc_macro2::TokenStream>>()?,
            );
        }
        // Functions
        if cfg.generate_functions {
            module_content.extend(
                self.functions
                    .iter()
                    .map(|function| function.generate(cfg, &scoped_function_idents, &local_types))
                    .collect::<Result<proc_macro2::TokenStream>>()?,
            );
        }
        // Properties
        if cfg.generate_properties {
            module_content.extend(
                self.properties
                    .iter()
                    .map(|property| property.generate(cfg, &scoped_function_idents, &local_types))
                    .collect::<Result<proc_macro2::TokenStream>>()?,
            );
        }
        // Submodules
        if cfg.traverse_submodules {
            module_content.extend(
                self.submodules
                    .iter()
                    .map(|module| module.generate(cfg, top_level_modules, all_types))
                    .collect::<Result<proc_macro2::TokenStream>>()?,
            );
        }

        // Finalize the module with its content
        let module_ident: syn::Ident = self.name.name().try_into()?;
        output.extend(quote::quote! {
            pub mod #module_ident {
                #module_content
            }
        });

        Ok(output)
    }

    fn extract_submodules(cfg: &Config, module: &pyo3::types::PyModule) -> Result<HashSet<Ident>> {
        let py = module.py();
        let pkgutil = py.import(pyo3::intern!(py, "pkgutil"))?;

        // Extract the paths of the module
        let module_paths = module
            .getattr(pyo3::intern!(py, "__path__"))?
            .extract::<&pyo3::types::PyList>()?
            .iter()
            .map(|x| std::path::PathBuf::from(x.to_string()))
            .collect_vec();

        // Extract the names of all submodules via `pkgutil.iter_modules`
        let module_name = Path::from_py(module.name()?);
        pkgutil
            .call_method1(pyo3::intern!(py, "iter_modules"), (module_paths,))?
            .iter()?
            .map(|submodule| {
                Ok(Ident::from_py(
                    &submodule?.getattr(pyo3::intern!(py, "name"))?.to_string(),
                ))
            })
            // Filter based on various configurable conditions
            .filter(|submodule_name| {
                submodule_name.as_ref().is_ok_and(|submodule_name| {
                    cfg.is_attr_allowed(
                        submodule_name,
                        &module_name,
                        py.get_type::<pyo3::types::PyModule>(),
                    )
                })
            })
            .collect()
    }

    fn extract_prelude(
        cfg: &Config,
        module: &pyo3::types::PyModule,
        module_name: &Path,
    ) -> Vec<Ident> {
        // Extract the index (__all__) of the module if it exists
        let mut index_attr_names = if let Ok(index) = module.index() {
            index
                .iter()
                .map(|x| Ident::from_py(&x.to_string()))
                .unique()
                .collect()
        } else {
            Vec::default()
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
            let index_attr_names_set: HashSet<_> = index_attr_names.iter().cloned().collect();

            if index_attr_names_set == public_attr_names_set {
                return Vec::new();
            }
        }

        // If the generation of dependencies is disabled, retain only reexports
        if !cfg.generate_dependencies {
            index_attr_names.retain(|attr_name| {
                if let Ok(attr) = module.getattr(attr_name.as_py()) {
                    let is_reexport = module_name.root().is_some_and(|root_module| {
                        let attr_module = Path::from_py(
                            &attr
                                .getattr(pyo3::intern!(module.py(), "__module__"))
                                .map(std::string::ToString::to_string)
                                .unwrap_or_default(),
                        );
                        attr_module.starts_with(&root_module)
                    });
                    is_reexport
                } else {
                    false
                }
            });
        }

        // Retain only allowed attributes
        index_attr_names.retain(|attr_name| {
            if let Ok(attr) = module.getattr(attr_name.as_py()) {
                let attr_type = attr.get_type();
                cfg.is_attr_allowed(attr_name, module_name, attr_type)
            } else {
                false
            }
        });

        index_attr_names
    }

    fn generate_prelude(&self) -> Result<proc_macro2::TokenStream> {
        // Skip if the prelude is empty
        if self.prelude.is_empty() {
            return Ok(proc_macro2::TokenStream::new());
        }

        // Generate the prelude content (re-export all prelude items)
        let exports = self
            .prelude
            .iter()
            // Retain only attributes that are within self.modules, self.classes, self.functions, self.type_vars, self.properties
            .filter(|&ident| self.check_ident_exists_immediate(ident, false))
            .map(|ident| {
                let ident: syn::Ident = ident.try_into()?;
                Ok(quote::quote! {
                    #ident,
                })
            })
            .collect::<Result<proc_macro2::TokenStream>>()?;

        // Return empty prelude if there are no exports
        if exports.is_empty() {
            return Ok(proc_macro2::TokenStream::new());
        }

        // Finalize the prelude with its content
        let prelude_ident: syn::Ident = {
            let mut i = 0;
            loop {
                let ident = Ident::from_py(&format!(
                    "call{}",
                    (i > 0).then(|| i.to_string()).unwrap_or_default()
                ));
                if !self.check_ident_exists_immediate(&ident, true) {
                    break ident;
                }
                i += 1;
            }
        }
        .try_into()?;
        Ok(quote::quote! {
            pub mod #prelude_ident {
                pub use super::{#exports};
            }
        })
    }

    fn check_path_exists_recursive(&self, path: &Path, consider_imports: bool) -> bool {
        (consider_imports && self.imports.iter().any(|import| import.target == *path))
            || self.submodules.iter().any(|module| module.name == *path)
            || self.classes.iter().any(|class| class.name == *path)
            || self.functions.iter().any(|function| function.name == *path)
            || self.type_vars.iter().any(|type_var| type_var.name == *path)
            || self
                .properties
                .iter()
                .any(|property| property.name == *path)
            || self
                .submodules
                .iter()
                .any(|module| module.check_path_exists_recursive(path, consider_imports))
    }

    fn check_ident_exists_immediate(&self, ident: &Ident, consider_imports: bool) -> bool {
        (consider_imports
            && self
                .imports
                .iter()
                .any(|import| import.target.name() == ident))
            || self
                .submodules
                .iter()
                .any(|module| module.name.name() == ident)
            || self.classes.iter().any(|class| class.name.name() == ident)
            || self
                .functions
                .iter()
                .any(|function| function.name.name() == ident)
            || self
                .type_vars
                .iter()
                .any(|type_var| type_var.name.name() == ident)
            || self
                .properties
                .iter()
                .any(|property| property.name.name() == ident)
    }
}
