use crate::{
    syntax::{Ident, Import, Module, Path},
    Config, Result,
};
use itertools::Itertools;
use rustc_hash::FxHashSet as HashSet;

/// Engine for automatic generation of Rust FFI bindings to Python modules.
///
/// # Examples
///
/// Here is a simple example of how to use the `Codegen` engine to generate
/// Rust FFI bindings for the full `os` and `sys` Python modules. With the
/// default configuration, all submodules, classes, functions, and parameters
/// will be recursively parsed and included in the generated bindings.
///
/// ```
/// # use pyo3_bindgen_engine::{Codegen, Config};
/// fn main() -> Result<(), Box<dyn std::error::Error>> {
///     Codegen::new(Config::default())?
///         .module_name("os")?
///         .module_name("sys")?
///         .generate()?;
///     Ok(())
/// }
/// ```
///
/// For more focused generation, paths to specific submodules can be provided.
/// In the following example, only the `entities` and `parser` submodules of the
/// `html` module will be included in the generated bindings alongside their
/// respective submodules, classes, functions, and parameters. No direct attributes
/// or submodules of the `html` top-level module will be included.
///
/// ```
/// # use pyo3_bindgen_engine::{Codegen, Config};
/// fn main() -> Result<(), Box<dyn std::error::Error>> {
///     Codegen::new(Config::default())?
///         .module_names(&["html.entities", "html.parser"])?
///         .generate()?;
///     Ok(())
/// }
/// ```
#[derive(Debug, Default, Clone)]
pub struct Codegen {
    cfg: Config,
    modules: Vec<Module>,
}

impl Codegen {
    /// Create a new `Codegen` engine with the given configuration.
    pub fn new(cfg: Config) -> Result<Self> {
        Ok(Self {
            cfg,
            ..Default::default()
        })
    }

    /// Add a Python module to the list of modules for which to generate bindings.
    pub fn module(mut self, module: &pyo3::types::PyModule) -> Result<Self> {
        crate::io_utils::with_suppressed_python_output(
            module.py(),
            self.cfg.suppress_python_stdout,
            self.cfg.suppress_python_stderr,
            || {
                self.modules.push(Module::parse(&self.cfg, module)?);
                Ok(())
            },
        )?;
        Ok(self)
    }

    /// Add a Python module by its name to the list of modules for which to generate bindings.
    pub fn module_name(self, module_name: &str) -> Result<Self> {
        #[cfg(not(PyPy))]
        pyo3::prepare_freethreaded_python();
        pyo3::Python::with_gil(|py| {
            let module = py.import(module_name)?;
            self.module(module)
        })
    }

    /// Add a Python module from its source code and name to the list of modules for which to generate bindings.
    pub fn module_from_str(self, source_code: &str, new_module_name: &str) -> Result<Self> {
        #[cfg(not(PyPy))]
        pyo3::prepare_freethreaded_python();
        pyo3::Python::with_gil(|py| {
            let module = pyo3::types::PyModule::from_code(
                py,
                source_code,
                &format!("{new_module_name}/__init__.py"),
                new_module_name,
            )?;
            self.module(module)
        })
    }

    /// Add multiple Python modules to the list of modules for which to generate bindings.
    pub fn modules(mut self, modules: &[&pyo3::types::PyModule]) -> Result<Self> {
        self.modules.reserve(modules.len());
        for module in modules {
            self = self.module(module)?;
        }
        Ok(self)
    }

    /// Add multiple Python modules by their names to the list of modules for which to generate bindings.
    pub fn module_names(mut self, module_names: &[&str]) -> Result<Self> {
        self.modules.reserve(module_names.len());
        for module_name in module_names {
            self = self.module_name(module_name)?;
        }
        Ok(self)
    }

    /// Generate the Rust FFI bindings for all modules added to the engine.
    pub fn generate(mut self) -> Result<proc_macro2::TokenStream> {
        assert!(
            !self.modules.is_empty(),
            "There are no modules for which to generate bindings"
        );

        // Parse external modules (if enabled)
        if self.cfg.generate_dependencies {
            self.parse_dependencies()?;
        }

        // Canonicalize the module tree
        self.canonicalize();

        // Generate the bindings for all modules
        self.modules
            .iter()
            .map(|module| module.generate(&self.cfg, &self.modules, &self.get_all_types()))
            .collect::<Result<_>>()
    }

    /// Generate the Rust FFI bindings for all modules added to the engine and write them to the given file.
    /// This is a convenience method that combines `generate` and `std::fs::write`.
    pub fn build(self, output_path: impl AsRef<std::path::Path>) -> Result<()> {
        Ok(std::fs::write(output_path, self.generate()?.to_string())?)
    }

    fn parse_dependencies(&mut self) -> Result<()> {
        fn get_imports_recursive(input: &[Module]) -> Vec<Import> {
            let mut imports = Vec::new();
            for module in input {
                imports.extend(
                    module
                        .imports
                        .iter()
                        .filter(|import| import.is_external())
                        .cloned(),
                );
                imports.extend(get_imports_recursive(&module.submodules));
            }
            imports
        }

        // Get a unique list of all external imports (these could be modules, classes, functions, etc.)
        let external_imports = get_imports_recursive(&self.modules)
            .into_iter()
            .filter(super::syntax::import::Import::is_external)
            .map(|import| import.origin.clone())
            .unique()
            .collect_vec();

        // Parse the external imports and add them to the module tree
        pyo3::Python::with_gil(|py| {
            external_imports
                .iter()
                // Get the last valid module within the path of the import
                .map(|import| {
                    let mut last_module = py
                        .import(
                            import
                                .root()
                                .unwrap_or_else(|| unreachable!())
                                .to_py()
                                .as_str(),
                        )
                        .unwrap();
                    for path in &import[1..] {
                        if let Ok(attr) = last_module.getattr(path.as_py()) {
                            if let Ok(module) = attr.extract::<&pyo3::types::PyModule>() {
                                last_module = module;
                            } else {
                                break;
                            }
                        } else {
                            break;
                        }
                    }
                    last_module
                })
                // Parse the module and add it to the module tree
                .unique_by(|module| module.name().unwrap().to_string())
                // Filter attributes based on various configurable conditions
                .filter(|module| {
                    self.cfg.is_attr_allowed(
                        &Ident::from_py(module.name().unwrap()),
                        &Path::from_py(
                            &module
                                .getattr(pyo3::intern!(py, "__module__"))
                                .map(std::string::ToString::to_string)
                                .unwrap_or_default(),
                        ),
                        py.get_type::<pyo3::types::PyModule>(),
                    )
                })
                .try_for_each(|module| {
                    crate::io_utils::with_suppressed_python_output(
                        module.py(),
                        self.cfg.suppress_python_stdout,
                        self.cfg.suppress_python_stderr,
                        || {
                            self.modules.push(Module::parse(&self.cfg, module)?);
                            Ok(())
                        },
                    )
                })?;
            Ok(())
        })
    }

    fn canonicalize(&mut self) {
        // Canonicalize the module tree, such that no submodules remain at the top-level
        // Example: If `mod.submod.subsubmod` is currently top-level, it will be embedded as submodule into `mod.submod`
        //          and `mod.submod` will be embedded in top-level `mod`
        pyo3::Python::with_gil(|py| {
            self.modules.iter_mut().for_each(|module| {
                if module.name.len() > 1 {
                    *module =
                        (0..module.name.len() - 1)
                            .rev()
                            .fold(module.clone(), |package, i| {
                                let name = Path::from(&module.name[0..=i]);
                                let mut parent_package =
                                    Module::empty(py, name).unwrap_or_else(|_| unreachable!());
                                parent_package.submodules.push(package);
                                parent_package
                            });
                }
            });
        });

        // Merge duplicate modules in the tree
        self.merge_duplicate_modules();
    }

    fn merge_duplicate_modules(&mut self) {
        fn get_duplicate_modules(modules: &mut [Module]) -> Vec<std::ops::Range<usize>> {
            modules.sort_by(|a, b| a.name.cmp(&b.name));
            let mut i = 0;
            let mut duplicates = Vec::new();
            while i < modules.len() {
                let name = modules[i].name.clone();
                let span = modules
                    .iter()
                    .skip(i)
                    .take_while(|module| module.name == name)
                    .count();
                if span > 1 {
                    duplicates.push(i..i + span);
                }
                i += span;
            }
            duplicates
        }

        fn merge_duplicate_submodules_recursive(input: &[Module]) -> Module {
            Module {
                prelude: input
                    .iter()
                    .fold(HashSet::default(), |mut prelude, module| {
                        prelude.extend(module.prelude.iter().cloned());
                        prelude
                    })
                    .into_iter()
                    .collect(),
                imports: input
                    .iter()
                    .fold(HashSet::default(), |mut prelude, module| {
                        prelude.extend(module.imports.iter().cloned());
                        prelude
                    })
                    .into_iter()
                    .collect(),
                submodules: {
                    let mut submodules =
                        input.iter().fold(Vec::default(), |mut submodule, module| {
                            submodule.extend(module.submodules.iter().cloned());
                            submodule
                        });
                    get_duplicate_modules(&mut submodules)
                        .into_iter()
                        .rev()
                        .for_each(|range| {
                            submodules[range.start] =
                                merge_duplicate_submodules_recursive(&submodules[range.clone()]);
                            submodules.drain(range.start + 1..range.end);
                        });
                    submodules
                },
                classes: input
                    .iter()
                    .fold(HashSet::default(), |mut prelude, module| {
                        prelude.extend(module.classes.iter().cloned());
                        prelude
                    })
                    .into_iter()
                    .collect(),
                functions: input
                    .iter()
                    .fold(HashSet::default(), |mut prelude, module| {
                        prelude.extend(module.functions.iter().cloned());
                        prelude
                    })
                    .into_iter()
                    .collect(),
                type_vars: input
                    .iter()
                    .fold(HashSet::default(), |mut prelude, module| {
                        prelude.extend(module.type_vars.iter().cloned());
                        prelude
                    })
                    .into_iter()
                    .collect(),
                properties: input
                    .iter()
                    .fold(HashSet::default(), |mut prelude, module| {
                        prelude.extend(module.properties.iter().cloned());
                        prelude
                    })
                    .into_iter()
                    .collect(),
                ..input[0].clone()
            }
        }

        get_duplicate_modules(&mut self.modules)
            .into_iter()
            .rev()
            .for_each(|range| {
                self.modules[range.start] =
                    merge_duplicate_submodules_recursive(&self.modules[range.clone()]);
                self.modules.drain(range.start + 1..range.end);
            });
    }

    fn get_all_types(&self) -> Vec<Path> {
        fn get_types_recursive(input: &[Module]) -> Vec<Path> {
            let mut types = Vec::new();
            for module in input {
                types.extend(module.classes.iter().map(|class| class.name.clone()));
                types.extend(
                    module
                        .type_vars
                        .iter()
                        .map(|type_var| type_var.name.clone()),
                );
                types.extend(get_types_recursive(&module.submodules));
            }
            types
        }

        get_types_recursive(&self.modules)
            .into_iter()
            .unique()
            .collect()
    }
}
