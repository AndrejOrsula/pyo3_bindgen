use crate::bindgen::{bind_attribute, bind_function};

/// Generate Rust bindings to a Python class with all its methods and attributes (properties).
/// This function will call itself recursively to generate bindings to all nested classes.
pub fn bind_class<S: ::std::hash::BuildHasher + Default>(
    py: pyo3::Python,
    root_module: &pyo3::types::PyModule,
    class: &pyo3::types::PyType,
    all_types: &std::collections::HashSet<String, S>,
) -> Result<proc_macro2::TokenStream, pyo3::PyErr> {
    let inspect = py.import("inspect")?;

    // Extract the names of the modules
    let root_module_name = root_module.name()?;
    let class_full_name = class.name()?;
    let class_name = class_full_name.split('.').last().unwrap();
    let class_module_name = class.getattr("__module__")?.to_string();

    // Create the Rust class identifier (raw string if it is a keyword)
    let class_ident = if syn::parse_str::<syn::Ident>(class_name).is_ok() {
        quote::format_ident!("{class_name}")
    } else {
        quote::format_ident!("r#{class_name}")
    };

    // let mut fn_names = Vec::new();

    let mut impl_token_stream = proc_macro2::TokenStream::new();

    // Implement new()
    if class.hasattr("__init__")? {
        for i in 0.. {
            let new_fn_name = if i == 0 {
                "new".to_string()
            } else {
                format!("new{i}")
            };
            if !class.hasattr(new_fn_name.as_str())? {
                impl_token_stream.extend(bind_function(
                    py,
                    &class_module_name,
                    &new_fn_name,
                    class.getattr("__init__")?,
                    all_types,
                    Some(class),
                ));
                break;
            }
        }
    }
    // Implement call() method
    if class.hasattr("__call__")? {
        for i in 0.. {
            let call_fn_name = if i == 0 {
                "call".to_string()
            } else {
                format!("call{i}")
            };
            if !class.hasattr(call_fn_name.as_str())? {
                impl_token_stream.extend(bind_function(
                    py,
                    &class_module_name,
                    &call_fn_name,
                    class.getattr("__call__")?,
                    all_types,
                    Some(class),
                ));
                break;
            }
        }
    }

    // Iterate over all attributes of the module while updating the token stream
    class
        .dir()
        .iter()
        .filter_map(|name| {
            let name = name.str().unwrap().to_str().unwrap();
            if let Ok(attr) = class.getattr(name) {
                let attr_type = attr.get_type();
                Some((name, attr, attr_type))
            } else {
                None
            }
        })
        .filter(|&(_, _, attr_type)| {
            // Skip builtin functions
            !attr_type
                .is_subclass_of::<pyo3::types::PyCFunction>()
                .unwrap_or(false)
        })
        .filter(|&(name, _, _)| {
            // Skip private attributes
            !name.starts_with('_')
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
                    .is_ok_and(|package| package.to_string().starts_with(root_module_name));
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
                    .ne(&class_module_name);

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
            debug_assert!(![is_class, is_function].iter().all(|&v| v));

            if is_class && !is_reexport {
                // TODO: Properly handle nested classes
                // impl_token_stream.extend(bind_class(
                //     py,
                //     root_module,
                //     attr.downcast().unwrap(),
                //     all_types,
                // ));
            } else if is_function {
                // fn_names.push(name.to_string());
                impl_token_stream.extend(bind_function(
                    py,
                    &class_module_name,
                    name,
                    attr,
                    all_types,
                    Some(class),
                ));
            } else if !name.starts_with('_') {
                impl_token_stream.extend(bind_attribute(
                    py,
                    &class_module_name,
                    true,
                    name,
                    attr,
                    attr_type,
                    all_types,
                ));
            }
        });

    let mut doc = class.getattr("__doc__")?.to_string();
    if doc == "None" {
        doc = String::new();
    };

    let object_name = format!("{class_module_name}.{class_name}");

    Ok(quote::quote! {
        #[doc = #doc]
        #[repr(transparent)]
        pub struct #class_ident(::pyo3::PyAny);
        // Note: Using these macros is probably not the best idea, but it makes possible wrapping around ::pyo3::PyAny instead of ::pyo3::PyObject, which improves usability
        ::pyo3::pyobject_native_type_named!(#class_ident);
        ::pyo3::pyobject_native_type_info!(#class_ident, ::pyo3::pyobject_native_static_type_object!(::pyo3::ffi::PyBaseObject_Type), ::std::option::Option::Some(#object_name));
        ::pyo3::pyobject_native_type_extract!(#class_ident);
        #[automatically_derived]
        impl #class_ident {
            #impl_token_stream
        }
    })

    // Ok(quote::quote! {
    //     #[doc = #doc]
    //     #[repr(transparent)]
    //     #[derive(Clone, Debug)]
    //     pub struct #class_ident(pub ::pyo3::PyObject);
    //     #[automatically_derived]
    //     impl ::std::ops::Deref for #class_ident {
    //         type Target = ::pyo3::PyObject;
    //         fn deref(&self) -> &Self::Target {
    //             &self.0
    //         }
    //     }
    // #[automatically_derived]
    // impl ::std::ops::DerefMut for #class_ident {
    //     fn deref_mut(&mut self) -> &mut Self::Target {
    //         &mut self.0
    //     }
    // }
    //     #[automatically_derived]
    //     impl<'py> ::pyo3::FromPyObject<'py> for #class_ident {
    //         fn extract(value: &'py ::pyo3::PyAny) -> ::pyo3::PyResult<Self> {
    //             Ok(Self(value.into()))
    //         }
    //     }
    //     #[automatically_derived]
    //     impl ::pyo3::ToPyObject for #class_ident {
    //         fn to_object<'py>(&'py self, py: ::pyo3::Python<'py>) -> ::pyo3::PyObject {
    //             self.as_ref(py).to_object(py)
    //         }
    //     }
    //     #[automatically_derived]
    //     impl From<::pyo3::PyObject> for #class_ident {
    //         fn from(value: ::pyo3::PyObject) -> Self {
    //             Self(value)
    //         }
    //     }
    //     #[automatically_derived]
    //     impl<'py> From<&'py ::pyo3::PyAny> for #class_ident {
    //         fn from(value: &'py ::pyo3::PyAny) -> Self {
    //             Self(value.into())
    //         }
    //     }
    //     #[automatically_derived]
    //     impl #class_ident {
    //         #impl_token_stream
    //     }
    // })
}
