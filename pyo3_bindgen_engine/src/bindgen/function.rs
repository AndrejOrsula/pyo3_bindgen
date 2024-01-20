use itertools::Itertools;
use pyo3::PyTypeInfo;

use crate::types::map_attr_type;

/// Generate Rust bindings to a Python function. The function can be a standalone function or a
/// method of a class.
pub fn bind_function(
    py: pyo3::Python,
    module_name: &str,
    name: &str,
    function: &pyo3::PyAny,
) -> Result<proc_macro2::TokenStream, pyo3::PyErr> {
    let inspect = py.import("inspect")?;

    let signature = inspect.call_method1("signature", (function,))?;

    let empty_return_annotation = signature.getattr("empty")?;

    let parameters = signature.getattr("parameters")?;
    let return_annotation = signature.getattr("return_annotation")?;

    let return_annotation = if return_annotation.is(empty_return_annotation) {
        None
    } else {
        Some(return_annotation)
    };

    let mut positional_args_idents = Vec::new();
    let mut keyword_args_idents = Vec::new();
    let mut keyword_args_names = Vec::new();
    let mut var_positional_ident = None;
    let mut var_keyword_ident = None;

    let parameters = parameters
        .call_method0("values")?
        .iter()?
        .map(|parameter| {
            let parameter = parameter.unwrap();

            let empty_param_annotation = parameter.getattr("empty").unwrap();

            let param_name = parameter.getattr("name").unwrap().to_string();

            let param_default = parameter.getattr("default").unwrap();
            let param_annotation = parameter.getattr("annotation").unwrap();
            let param_kind = parameter.getattr("kind").unwrap();

            let param_annotation = if param_annotation.is(empty_param_annotation) {
                None
            } else {
                Some(param_annotation)
            };
            let param_default = if param_default.is(empty_param_annotation) {
                None
            } else {
                Some(param_default)
            };
            // TODO: Turn into enum or process in-place
            // TODO: Fully support positional-only parameters
            let param_kind = match param_kind.extract::<usize>().unwrap() {
                0 => "POSITIONAL_ONLY",
                1 => "POSITIONAL_OR_KEYWORD",
                2 => "VAR_POSITIONAL", // args
                3 => "KEYWORD_ONLY",
                4 => "VAR_KEYWORD", // kwargs
                _ => unreachable!(),
            };

            if param_name != "self" {
                match param_kind {
                    "POSITIONAL_ONLY" | "POSITIONAL_OR_KEYWORD" => {
                        positional_args_idents.push(
                            if syn::parse_str::<syn::Ident>(&param_name).is_ok() {
                                quote::format_ident!("{}", param_name)
                            } else {
                                quote::format_ident!("r#{}", param_name)
                            },
                        );
                    }
                    "KEYWORD_ONLY" => {
                        keyword_args_idents.push(
                            if syn::parse_str::<syn::Ident>(&param_name).is_ok() {
                                quote::format_ident!("{}", param_name)
                            } else {
                                quote::format_ident!("r#{}", param_name)
                            },
                        );
                        keyword_args_names.push(param_name.clone());
                    }
                    "VAR_POSITIONAL" => {
                        var_positional_ident =
                            Some(if syn::parse_str::<syn::Ident>(&param_name).is_ok() {
                                quote::format_ident!("{}", param_name)
                            } else {
                                quote::format_ident!("r#{}", param_name)
                            });
                        positional_args_idents.push(
                            if syn::parse_str::<syn::Ident>(&param_name).is_ok() {
                                quote::format_ident!("{}", param_name)
                            } else {
                                quote::format_ident!("r#{}", param_name)
                            },
                        );
                    }
                    "VAR_KEYWORD" => {
                        var_keyword_ident =
                            Some(if syn::parse_str::<syn::Ident>(&param_name).is_ok() {
                                quote::format_ident!("{}", param_name)
                            } else {
                                quote::format_ident!("r#{}", param_name)
                            });
                    }
                    _ => unreachable!(),
                }
            }

            let param_annotation = match param_kind {
                "VAR_POSITIONAL" => Some(pyo3::types::PyTuple::type_object(py).downcast().unwrap()),
                "VAR_KEYWORD" => Some(pyo3::types::PyDict::type_object(py).downcast().unwrap()),
                _ => param_annotation,
            };

            (param_name, param_annotation, param_default, param_kind)
        })
        .collect_vec();

    let function_ident = if syn::parse_str::<syn::Ident>(name).is_ok() {
        quote::format_ident!("{}", name)
    } else {
        quote::format_ident!("r#{}", name)
    };
    let function_name = function.getattr("__name__")?.to_string();

    // Check if `self` is the first parameter
    let has_self_param = parameters
        .iter()
        .any(|(param_name, _, _, _)| param_name == "self");

    let param_idents = parameters
        .iter()
        .skip(usize::from(has_self_param))
        .map(|(param_name, _, _, _)| {
            if syn::parse_str::<syn::Ident>(param_name).is_ok() {
                quote::format_ident!("{}", param_name)
            } else {
                quote::format_ident!("r#{}", param_name)
            }
        })
        .collect_vec();
    let pynone = py.None();
    let pynone = pynone.as_ref(py);
    let param_types = parameters
        .iter()
        .skip(usize::from(has_self_param))
        .map(|(_, param_annotation, _, _)| {
            map_attr_type(param_annotation.unwrap_or_else(|| pynone), false).unwrap()
        })
        .collect_vec();
    let return_annotation = map_attr_type(return_annotation.unwrap_or(pynone), true)?;

    let mut doc = function.getattr("__doc__")?.to_string();
    if doc == "None" {
        doc = String::new();
    };

    // TODO: Use `call_method0` and `call_method1`` where appropriate
    Ok(if has_self_param {
        if let Some(var_keyword_ident) = var_keyword_ident {
            quote::quote! {
                    #[doc = #doc]
                    pub fn #function_ident<'py>(
                    &'py mut self,
                    py: ::pyo3::marker::Python<'py>,
                    #(#param_idents: #param_types),*
                ) -> ::pyo3::PyResult<#return_annotation> {
                    #[allow(unused_imports)]
                    use ::pyo3::IntoPy;
                    let __internal_args = (
                        #({
                            let #positional_args_idents: ::pyo3::PyObject = #positional_args_idents.into_py(py);
                            #positional_args_idents
                        },)*
                    );
                    let __internal_kwargs = #var_keyword_ident;
                    #(__internal_kwargs.set_item(::pyo3::intern!(py, #keyword_args_names), #keyword_args_idents)?;)*
                    self.as_ref(py).call_method(::pyo3::intern!(py, #function_name), __internal_args, Some(__internal_kwargs))?.extract()
                }
            }
        } else {
            quote::quote! {
                    #[doc = #doc]
                    pub fn #function_ident<'py>(
                    &'py mut self,
                    py: ::pyo3::marker::Python<'py>,
                    #(#param_idents: #param_types),*
                ) -> ::pyo3::PyResult<#return_annotation> {
                    #[allow(unused_imports)]
                    use ::pyo3::IntoPy;
                    let __internal_args = (
                        #({
                            let #positional_args_idents: ::pyo3::PyObject = #positional_args_idents.into_py(py);
                            #positional_args_idents
                        },)*
                    );
                    let __internal_kwargs = ::pyo3::types::PyDict::new(py);
                    #(__internal_kwargs.set_item(::pyo3::intern!(py, #keyword_args_names), #keyword_args_idents)?;)*
                    self.as_ref(py).call_method(::pyo3::intern!(py, #function_name), __internal_args, Some(__internal_kwargs))?.extract()
                }
            }
        }
    } else if let Some(var_keyword_ident) = var_keyword_ident {
        quote::quote! {
            #[doc = #doc]
            pub fn #function_ident<'py>(
                py: ::pyo3::marker::Python<'py>,
                #(#param_idents: #param_types),*
            ) -> ::pyo3::PyResult<#return_annotation> {
                #[allow(unused_imports)]
                use ::pyo3::IntoPy;
                let __internal_args = (
                    #({
                        let #positional_args_idents: ::pyo3::PyObject = #positional_args_idents.into_py(py);
                        #positional_args_idents
                    },)*
                );
                let __internal_kwargs = #var_keyword_ident;
                #(__internal_kwargs.set_item(::pyo3::intern!(py, #keyword_args_names), #keyword_args_idents)?;)*
                py.import(::pyo3::intern!(py, #module_name))?.call_method(::pyo3::intern!(py, #function_name), __internal_args, Some(__internal_kwargs))?.extract()
            }
        }
    } else {
        quote::quote! {
            #[doc = #doc]
            pub fn #function_ident<'py>(
                py: ::pyo3::marker::Python<'py>,
                #(#param_idents: #param_types),*
            ) -> ::pyo3::PyResult<#return_annotation> {
                #[allow(unused_imports)]
                use ::pyo3::IntoPy;
                let __internal_args = (
                    #({
                        let #positional_args_idents: ::pyo3::PyObject = #positional_args_idents.into_py(py);
                        #positional_args_idents
                    },)*
                );
                let __internal_kwargs = ::pyo3::types::PyDict::new(py);
                #(__internal_kwargs.set_item(::pyo3::intern!(py, #keyword_args_names), #keyword_args_idents)?;)*
                py.import(::pyo3::intern!(py, #module_name))?.call_method(::pyo3::intern!(py, #function_name), __internal_args, Some(__internal_kwargs))?.extract()
            }
        }
    })
}
