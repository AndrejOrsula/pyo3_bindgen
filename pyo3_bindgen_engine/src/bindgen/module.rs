use itertools::Itertools;

use crate::bindgen::{bind_attribute, bind_class, bind_function};

/// Generate a Rust module from a Python module. This function is called recursively to generate
/// bindings for all submodules. The generated module will contain all classes, functions, and
/// attributes of the Python module. During the first call, the `root_module` argument should be
/// the same as the `module` argument and the `processed_modules` argument should be an empty
/// `HashSet`.
pub fn bind_module<S: ::std::hash::BuildHasher>(
    py: pyo3::Python,
    root_module: &pyo3::types::PyModule,
    module: &pyo3::types::PyModule,
    processed_modules: &mut std::collections::HashSet<String, S>,
) -> Result<proc_macro2::TokenStream, pyo3::PyErr> {
    let inspect = py.import("inspect")?;

    // Extract the names of the modules
    let root_module_name = root_module.name()?;
    let full_module_name = module.name()?;
    let module_name: &str = full_module_name.split('.').last().unwrap();

    // Create the Rust module identifier (raw string if it is a keyword)
    let module_ident = if syn::parse_str::<syn::Ident>(module_name).is_ok() {
        quote::format_ident!("{module_name}")
    } else {
        quote::format_ident!("r#{module_name}")
    };

    // Iterate over all attributes of the module while updating the token stream
    let mut mod_token_stream = proc_macro2::TokenStream::new();
    module
        .dir()
        .iter()
        .map(|name| {
            let name = name.str().unwrap().to_str().unwrap();
            let attr = module.getattr(name).unwrap();
            let attr_type = attr.get_type();
            (name, attr, attr_type)
        })
        .filter(|&(_, _, attr_type)| {
            // Skip builtin functions
            !attr_type
                .is_subclass_of::<pyo3::types::PyCFunction>()
                .unwrap_or(false)
        })
        .filter(|&(name, _, _)| {
            // Skip private attributes
            !name.starts_with('_') || name == "__init__" || name == "__call__"
        })
        .filter(|(_, attr, attr_type)| {
            // Skip typing attributes
            !attr
                .getattr("__module__")
                .is_ok_and(|module| module.to_string().contains("typing"))
                && !attr_type.to_string().contains("typing")
        })
        .filter(|(_, attr, _)| {
            // Skip __future__ attributes
            !attr
                .getattr("__module__")
                .is_ok_and(|module| module.to_string().contains("__future__"))
        })
        .filter(|&(_, attr, _)| {
            // Skip classes and functions that are not part of the package
            // However, this should keep instances of classes and builtins even if they are builtins or from other packages
            if let Ok(module) = attr.getattr("__module__") {
                if module.to_string().starts_with(root_module_name) {
                    true
                } else {
                    !(inspect
                        .call_method1("isclass", (attr,))
                        .unwrap()
                        .is_true()
                        .unwrap()
                        || inspect
                            .call_method1("isfunction", (attr,))
                            .unwrap()
                            .is_true()
                            .unwrap())
                }
            } else {
                true
            }
        })
        .filter(|&(_, attr, attr_type)| {
            // Skip external modules
            if attr_type
                .is_subclass_of::<pyo3::types::PyModule>()
                .unwrap_or(false)
            {
                let is_submodule = attr
                    .getattr("__package__")
                    // Note: full_module_name is used here for comparison on purpose.
                    // It unseres that submodules are created in the correct scopes.
                    .is_ok_and(|package| package.to_string().starts_with(full_module_name));
                is_submodule
            } else {
                true
            }
        })
        .for_each(|(name, attr, attr_type)| {
            let is_internal = attr
                .getattr("__module__")
                .unwrap_or(pyo3::types::PyString::new(py, ""))
                .to_string()
                .starts_with(root_module_name);
            let is_reexport = is_internal
                && attr
                    .getattr("__module__")
                    .unwrap_or(pyo3::types::PyString::new(py, ""))
                    .to_string()
                    .ne(full_module_name);

            let is_module = attr_type
                .is_subclass_of::<pyo3::types::PyModule>()
                .unwrap_or(false);

            let is_class = attr_type
                .is_subclass_of::<pyo3::types::PyType>()
                .unwrap_or(false);

            let is_function = inspect
                .call_method1("isfunction", (attr,))
                .unwrap()
                .is_true()
                .unwrap()
                || inspect
                    .call_method1("ismethod", (attr,))
                    .unwrap()
                    .is_true()
                    .unwrap();

            // Make sure that only one of the three is true
            debug_assert!(![is_module, is_class, is_function].iter().all(|&v| v));

            // Process hidden modules (shadowed by re-exported attributes of the same name)
            if (is_class || is_function)
                && is_reexport
                && attr
                    .getattr("__module__")
                    .unwrap()
                    .to_string()
                    .split('.')
                    .last()
                    .unwrap()
                    == name
                && attr
                    .getattr("__module__")
                    .unwrap()
                    .to_string()
                    .split('.')
                    .take(full_module_name.split('.').count())
                    .join(".")
                    == full_module_name
            {
                let content = if is_class {
                    bind_class(py, root_module, attr.downcast().unwrap()).unwrap()
                } else if is_function {
                    bind_function(py, full_module_name, name, attr).unwrap()
                } else {
                    unreachable!()
                };

                let shadowed_module_name = attr.getattr("__module__").unwrap().to_string();
                let shadowed_module_name = shadowed_module_name.split('.').last().unwrap();
                let shadowed_module_ident =
                    if syn::parse_str::<syn::Ident>(shadowed_module_name).is_ok() {
                        quote::format_ident!("{}", shadowed_module_name)
                    } else {
                        quote::format_ident!("r#{}", shadowed_module_name)
                    };

                mod_token_stream.extend(quote::quote! {
                    pub mod #shadowed_module_ident {
                        #content
                    }
                });
            }

            if is_module {
                if processed_modules.insert(format!(
                    "{}.{}",
                    attr.getattr("__package__").unwrap(),
                    name
                )) {
                    mod_token_stream.extend(bind_module(
                        py,
                        root_module,
                        attr.downcast().unwrap(),
                        processed_modules,
                    ));
                }
            } else if is_reexport {
                mod_token_stream.extend(bind_reexport(full_module_name, name, attr));
            } else if is_class {
                mod_token_stream.extend(bind_class(py, root_module, attr.downcast().unwrap()));
            } else if is_function {
                mod_token_stream.extend(bind_function(py, full_module_name, name, attr));
            } else {
                mod_token_stream.extend(bind_attribute(
                    py,
                    Some(full_module_name),
                    name,
                    attr,
                    attr_type,
                ));
            }
        });

    let mut doc = module.getattr("__doc__")?.to_string();
    if doc == "None" {
        doc = String::new();
    };

    Ok(if module_name == root_module_name {
        quote::quote! {
            #mod_token_stream
        }
    } else {
        quote::quote! {
            #[doc = #doc]
            pub mod #module_ident {
                #mod_token_stream
            }
        }
    })
}

/// Generate a re-export of an attribute from a submodule. This is commonly used in Python to
/// re-export attributes from submodules in the parent module. For example, `from os import path`
/// makes the `os.path` submodule available in the current module as just `path`.
pub fn bind_reexport(
    module_name: &str,
    name: &str,
    attr: &pyo3::PyAny,
) -> Result<proc_macro2::TokenStream, pyo3::PyErr> {
    let attr_origin_module = attr.getattr("__module__")?.to_string();
    let n_common_ancestors = module_name
        .split('.')
        .zip(attr_origin_module.split('.'))
        .take_while(|(a, b)| a == b)
        .count();
    let current_module_depth = module_name.split('.').count();
    let reexport_path = if (current_module_depth - n_common_ancestors) > 0 {
        std::iter::repeat("super".to_string()).take(current_module_depth - n_common_ancestors)
    } else {
        std::iter::repeat("self".to_string()).take(1)
    };
    let reexport_path: String = reexport_path
        .chain(
            attr_origin_module
                .split('.')
                .skip(n_common_ancestors)
                .map(|s| {
                    if syn::parse_str::<syn::Ident>(s).is_ok() {
                        s.to_owned()
                    } else {
                        format!("r#{s}")
                    }
                }),
        )
        .chain(std::iter::once(name).map(|s| {
            if syn::parse_str::<syn::Ident>(s).is_ok() {
                s.to_owned()
            } else {
                format!("r#{s}")
            }
        }))
        .join("::");

    // The path contains both ident and "::", combine into something that can be quoted
    let reexport_path = syn::parse_str::<syn::Path>(&reexport_path).unwrap();

    Ok(quote::quote! {
        pub use #reexport_path;
    })
}
