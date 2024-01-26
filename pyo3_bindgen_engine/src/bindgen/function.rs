use itertools::Itertools;
use pyo3::PyTypeInfo;

use crate::types::Type;

/// Generate Rust bindings to a Python function. The function can be a standalone function or a
/// method of a class.
pub fn bind_function<S: ::std::hash::BuildHasher + Default>(
    py: pyo3::Python,
    module_name: &str,
    name: &str,
    function: &pyo3::PyAny,
    all_types: &std::collections::HashSet<String, S>,
    method_of_class: Option<&pyo3::types::PyType>,
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
                    "POSITIONAL_ONLY" => {
                        positional_args_idents.push(
                            if syn::parse_str::<syn::Ident>(&param_name).is_ok() {
                                quote::format_ident!("{}", param_name)
                            } else {
                                quote::format_ident!("r#{}", param_name)
                            },
                        );
                    }
                    "KEYWORD_ONLY" | "POSITIONAL_OR_KEYWORD" => {
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
    let is_class_method =
        method_of_class.is_some() && (!has_self_param || function_name == "__init__");

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
            Type::try_from(param_annotation.unwrap_or_else(|| pynone))
                .unwrap()
                .into_rs_borrowed(module_name, all_types)
        })
        .collect_vec();

    let mut doc = function.getattr("__doc__")?.to_string();
    if doc == "None" {
        doc = String::new();
    };

    let (has_self_param, is_class_method) = if function_name == "__call__" {
        (true, false)
    } else {
        (has_self_param, is_class_method)
    };

    let (maybe_ref_self, callable_object) = match (has_self_param, is_class_method) {
        (true, false) => (quote::quote! { &'py self, }, quote::quote! { self }),
        (_, true) => {
            let class_name = method_of_class.unwrap().name().unwrap();
            (
                quote::quote! {},
                quote::quote! { py.import(::pyo3::intern!(py, #module_name))?.getattr(::pyo3::intern!(py, #class_name))?},
            )
        }
        _ => (
            quote::quote! {},
            quote::quote! { py.import(::pyo3::intern!(py, #module_name))? },
        ),
    };

    let has_positional_args = !positional_args_idents.is_empty();
    let set_args = match (
        positional_args_idents.len() > 1,
        var_positional_ident.is_some(),
    ) {
        (true, _) => {
            quote::quote! {
                let __internal_args = ::pyo3::types::PyTuple::new(
                    py,
                    [#(::pyo3::IntoPy::<::pyo3::PyObject>::into_py(#positional_args_idents.to_owned(), py).as_ref(py),)*]
                );
            }
        }
        (false, true) => {
            let var_positional_ident = var_positional_ident.unwrap();
            quote::quote! {
                let __internal_args = #var_positional_ident;
            }
        }
        (false, false) => {
            quote::quote! { let __internal_args = (); }
        }
    };

    let has_kwargs = !keyword_args_idents.is_empty();
    let kwargs_initial = if let Some(var_keyword_ident) = var_keyword_ident {
        quote::quote! { #var_keyword_ident }
    } else {
        quote::quote! { ::pyo3::types::PyDict::new(py) }
    };
    let set_kwargs = quote::quote! {
       let __internal_kwargs = #kwargs_initial;
       #(__internal_kwargs.set_item(::pyo3::intern!(py, #keyword_args_names), #keyword_args_idents)?;)*
    };

    let is_init_fn = function_name == "__init__";

    let return_annotation = if is_init_fn && method_of_class.is_some() {
        quote::quote! {
           &'py Self
        }
    } else {
        Type::try_from(return_annotation.unwrap_or(pynone))?.into_rs_owned(module_name, all_types)
    };

    let call_method = match (is_init_fn, has_positional_args, has_kwargs) {
        (true, _, true) => {
            quote::quote! {
                #set_args
                #set_kwargs
                #callable_object.call(__internal_args, Some(__internal_kwargs))?
            }
        }
        (true, true, false) => {
            quote::quote! {
                #set_args
                #callable_object.call1(__internal_args)?
            }
        }
        (true, false, false) => {
            quote::quote! {
                #callable_object.call0()?
            }
        }
        (false, _, true) => {
            quote::quote! {
                #set_args
                #set_kwargs
                #callable_object.call_method(::pyo3::intern!(py, #function_name), __internal_args, Some(__internal_kwargs))?
            }
        }
        (false, true, false) => {
            quote::quote! {
                #set_args
                #callable_object.call_method1(::pyo3::intern!(py, #function_name), __internal_args)?
            }
        }
        (false, false, false) => {
            quote::quote! {
                #callable_object.call_method0(::pyo3::intern!(py, #function_name))?
            }
        }
    };

    Ok(quote::quote! {
            #[doc = #doc]
            pub fn #function_ident<'py>(
            #maybe_ref_self
            py: ::pyo3::marker::Python<'py>,
            #(#param_idents: #param_types),*
        ) -> ::pyo3::PyResult<#return_annotation> {
            #call_method.extract()
        }
    })
}
