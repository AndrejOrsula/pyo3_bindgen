use super::{FunctionImplementation, Ident, Path, TraitMethod};
use crate::{typing::Type, Config, Result};
use itertools::Itertools;
use proc_macro2::TokenStream;
use pyo3::{prelude::*, types::IntoPyDict, ToPyObject};
use rustc_hash::FxHashMap as HashMap;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Function {
    pub name: Path,
    pub typ: FunctionType,
    parameters: Vec<Parameter>,
    return_annotation: Type,
    docstring: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum FunctionType {
    Function,
    Method { class_path: Path, typ: MethodType },
    Closure,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum MethodType {
    InstanceMethod,
    ClassMethod,
    StaticMethod,
    Constructor,
    Callable,
    Unknown,
}

impl Function {
    pub fn parse(
        _cfg: &Config,
        function: &pyo3::Bound<pyo3::types::PyAny>,
        name: Path,
        mut typ: FunctionType,
    ) -> Result<Self> {
        let py = function.py();

        // Extract the docstring of the function
        let docstring = {
            let docstring = function.getattr(pyo3::intern!(py, "__doc__"))?.to_string();
            if docstring.is_empty() || docstring == "None" {
                None
            } else {
                Some(docstring)
            }
        };

        // Extract the signature of the function
        if let Ok(function_signature) = py
            .import_bound(pyo3::intern!(py, "inspect"))?
            .call_method1(pyo3::intern!(py, "signature"), (function,))
        {
            // Extract the parameters of the function
            let mut parameters = function_signature
                .getattr(pyo3::intern!(py, "parameters"))?
                .call_method0(pyo3::intern!(py, "values"))?
                .iter()?
                .map(|param| {
                    let param = param?;

                    let name =
                        Ident::from_py(&param.getattr(pyo3::intern!(py, "name"))?.to_string());
                    let kind = ParameterKind::from(
                        param.getattr(pyo3::intern!(py, "kind"))?.extract::<u8>()?,
                    );
                    let annotation = match kind {
                        ParameterKind::VarPositional => Type::PyTuple(vec![Type::Unknown]),
                        ParameterKind::VarKeyword => Type::Optional(Box::new(Type::PyDict {
                            key_type: Box::new(Type::Unknown),
                            value_type: Box::new(Type::Unknown),
                        })),
                        _ => {
                            let annotation = param.getattr(pyo3::intern!(py, "annotation"))?;
                            if annotation.is(&param.getattr(pyo3::intern!(py, "empty"))?) {
                                Type::Unknown
                            } else {
                                annotation.try_into()?
                            }
                        }
                    };

                    let default = {
                        let default = param.getattr(pyo3::intern!(py, "default"))?;
                        if default.is(&param.getattr(pyo3::intern!(py, "empty"))?) {
                            None
                        } else {
                            Some(default.to_object(py))
                        }
                    };

                    Result::Ok(Parameter {
                        name,
                        kind,
                        annotation,
                        default,
                    })
                })
                .collect::<Result<Vec<_>>>()?;

            // Retain only used parameters (discard unused `_` parameters)
            parameters.retain(|param| param.name.as_rs() != "r#_");

            // Extract the return annotation of the function
            let return_annotation = {
                let return_annotation =
                    function_signature.getattr(pyo3::intern!(py, "return_annotation"))?;
                if return_annotation.is(&function_signature.getattr(pyo3::intern!(py, "empty"))?) {
                    Type::Unknown
                } else {
                    return_annotation.try_into()?
                }
            };

            // If marked as an unknown method, try to infer the method type
            match &typ {
                FunctionType::Method {
                    class_path,
                    typ: method_typ,
                } if *method_typ == MethodType::Unknown => {
                    // Get the class object from its class path
                    let class = py
                        .import_bound(
                            class_path
                                .root()
                                .unwrap_or_else(|| unreachable!())
                                .to_py()
                                .as_str(),
                        )
                        .and_then(|root_module| {
                            class_path.iter().skip(1).try_fold(
                                root_module.extract::<&pyo3::types::PyAny>()?,
                                |module, name| module.getattr(name.as_py()),
                            )
                        });

                    // Try to get the static object of the method (from __dict__), which still contains information about what kind of method it is
                    if let Ok(static_fn_obj) = class.and_then(|class| {
                        class
                            .getattr(pyo3::intern!(py, "__dict__"))?
                            .get_item(name.name().as_py())
                    }) {
                        let locals = [("obj", static_fn_obj)].into_py_dict_bound(py);
                        let method_type = if py
                            .eval_bound("isinstance(obj, classmethod)", None, Some(&locals))?
                            .is_truthy()?
                        {
                            MethodType::ClassMethod
                        } else if py
                            .eval_bound("isinstance(obj, staticmethod)", None, Some(&locals))?
                            .is_truthy()?
                        {
                            MethodType::StaticMethod
                        } else {
                            MethodType::InstanceMethod
                        };
                        typ = FunctionType::Method {
                            class_path: class_path.clone(),
                            typ: method_type,
                        };
                    } else {
                        // Cannot determine the method type, default to static method (will be changed to instance method if the first parameter is named 'self')
                        typ = FunctionType::Method {
                            class_path: class_path.clone(),
                            typ: MethodType::StaticMethod,
                        };
                    }
                }
                _ => {}
            };

            // As a final step in determining the method type, check parameters for all non-instance/callable methods
            // Note: This is not 100% reliable, because Python does not enforce the first parameter to be named "self"
            // TODO: See if there is a better way to infer the method type from parameters alone
            match &typ {
                FunctionType::Method {
                    typ: MethodType::InstanceMethod | MethodType::Constructor | MethodType::Callable,
                    ..
                } => {}
                FunctionType::Method { class_path, typ: _ } => {
                    if parameters.first().map(|p| p.name.as_rs()) == Some("r#self") {
                        typ = FunctionType::Method {
                            class_path: class_path.clone(),
                            typ: MethodType::InstanceMethod,
                        };
                    }
                }
                FunctionType::Function | FunctionType::Closure => {
                    if parameters.first().map(|p| p.name.as_rs()) == Some("r#self") {
                        if [
                            ParameterKind::PositionalOnly,
                            ParameterKind::PositionalOrKeyword,
                        ]
                        .contains(&parameters[0].kind)
                        {
                            eprintln!(
                                "WARN: Function '{name}' has the first parameter named 'self', but is not marked as a method. The parameter is renamed to '__unknown_self__'."
                            );
                            parameters[0].name = Ident::from_rs("__unknown_self__");
                            parameters[0].annotation = Type::Unknown;
                        } else {
                            eprintln!(
                                "WARN: Function '{name}' has the first parameter named 'self', but is not marked as a method. All parameters are replaced with '*args' and '**kwargs'."
                            );
                            parameters = vec![
                                Parameter {
                                    name: Ident::from_rs("args"),
                                    kind: ParameterKind::VarPositional,
                                    annotation: Type::PyTuple(vec![Type::Unknown]),
                                    default: None,
                                },
                                Parameter {
                                    name: Ident::from_rs("kwargs"),
                                    kind: ParameterKind::VarKeyword,
                                    annotation: Type::Optional(Box::new(Type::PyDict {
                                        key_type: Box::new(Type::Unknown),
                                        value_type: Box::new(Type::Unknown),
                                    })),
                                    default: None,
                                },
                            ];
                        }
                    }
                }
            };

            // Hack: Reassign InstanceMethod with no parameter to StaticMethod
            // This should not be necessary as every InstanceMethod should have at least one parameter (self), but it does for certain complex Python modules
            if let FunctionType::Method {
                typ: MethodType::InstanceMethod,
                ..
            } = &typ
            {
                if parameters.is_empty() {
                    eprintln!(
                            "WARN: Method '{name}' is marked as an instance method, but has no parameters. Changed to static method.",
                        );
                    typ = FunctionType::Method {
                        class_path: name.clone(),
                        typ: MethodType::StaticMethod,
                    };
                }
            };

            // Skip the first parameter if it's an instance method (or `__init__`/`__call__`)
            if let FunctionType::Method {
                typ: MethodType::InstanceMethod | MethodType::Constructor | MethodType::Callable,
                ..
            } = typ
            {
                parameters.remove(0);
            };

            // If any of the parameters is still called 'self', do not handle the parameters
            if parameters
                .iter()
                .any(|param| param.name.as_rs() == "r#self")
            {
                eprintln!(
                    "WARN: Method '{name}' has a non-first parameter named 'self'. All parameters are replaced with '*args' and '**kwargs'.",

                );
                parameters = vec![
                    Parameter {
                        name: Ident::from_rs("args"),
                        kind: ParameterKind::VarPositional,
                        annotation: Type::PyTuple(vec![Type::Unknown]),
                        default: None,
                    },
                    Parameter {
                        name: Ident::from_rs("kwargs"),
                        kind: ParameterKind::VarKeyword,
                        annotation: Type::Optional(Box::new(Type::PyDict {
                            key_type: Box::new(Type::Unknown),
                            value_type: Box::new(Type::Unknown),
                        })),
                        default: None,
                    },
                ];
            }

            Ok(Self {
                name,
                typ,
                parameters,
                return_annotation,
                docstring,
            })
        } else {
            Ok(Self {
                name,
                typ,
                parameters: vec![
                    Parameter {
                        name: Ident::from_rs("args"),
                        kind: ParameterKind::VarPositional,
                        annotation: Type::PyTuple(vec![Type::Unknown]),
                        default: None,
                    },
                    Parameter {
                        name: Ident::from_rs("kwargs"),
                        kind: ParameterKind::VarKeyword,
                        annotation: Type::Optional(Box::new(Type::PyDict {
                            key_type: Box::new(Type::Unknown),
                            value_type: Box::new(Type::Unknown),
                        })),
                        default: None,
                    },
                ],
                return_annotation: Type::Unknown,
                docstring,
            })
        }
    }

    pub fn generate(
        &self,
        cfg: &Config,
        scoped_function_idents: &[&Ident],
        local_types: &HashMap<Path, Path>,
    ) -> Result<FunctionImplementation> {
        let mut impl_fn = proc_macro2::TokenStream::new();

        // Documentation
        if cfg.generate_docs {
            if let Some(mut docstring) = self.docstring.clone() {
                crate::utils::text::format_docstring(&mut docstring);
                impl_fn.extend(quote::quote! {
                    #[doc = #docstring]
                });
            }
        }

        // Function signature
        let function_ident: syn::Ident = {
            let name = self.name.name();
            if let Ok(ident) = name.try_into() {
                if crate::config::FORBIDDEN_FUNCTION_NAMES.contains(&name.as_py()) {
                    return Ok(FunctionImplementation::empty_function());
                } else {
                    ident
                }
            } else {
                // Sanitize the function name
                let new_name = Ident::from_py(&format!(
                    "f_{}",
                    name.as_py().replace(|c: char| !c.is_alphanumeric(), "_")
                ));
                if let Ok(sanitized_ident) = new_name.clone().try_into() {
                    eprintln!(
                        "WARN: Function '{}' is an invalid Rust ident for a function name. Renamed to '{}'.",
                        self.name, self.name.parent().unwrap_or_default().join(&new_name.into())
                    );
                    sanitized_ident
                } else {
                    eprintln!(
                        "WARN: Function '{}' is an invalid Rust ident for a function name. Renaming failed. Bindings will not be generated.",
                        self.name
                    );
                    return Ok(FunctionImplementation::empty_function());
                }
            }
        };
        let param_idents: Vec<syn::Ident> = self
            .parameters
            .iter()
            .map(|param| Ok(Ident::from_py(&format!("p_{}", param.name)).try_into()?))
            .collect::<Result<Vec<_>>>()?;
        // Pre-process parameters that require it
        let param_preprocessing: proc_macro2::TokenStream = self
            .parameters
            .iter()
            .zip(param_idents.iter())
            .map(|(param, param_ident)| {
                let bind = param
                    .annotation
                    .preprocess_borrowed(param_ident, local_types);

                if param.default.is_some() {
                    let option_ident = quote::format_ident!("optional_{}", param_ident);
                    quote::quote! {
                        let #option_ident = #param_ident.is_some();
                        #bind
                    }
                } else {
                    bind
                }
            })
            .collect();
        let param_types: Vec<proc_macro2::TokenStream> = self
            .parameters
            .iter()
            .map(|param| {
                let local_type = param.annotation.clone().into_rs_borrowed(local_types);
                let res = if param.default.is_some() {
                    quote::quote! {
                        Option<#local_type>
                    }
                } else {
                    local_type
                };
                Result::Ok(res)}
            )
            .collect::<Result<Vec<_>>>()?;
        let return_type = self.return_annotation.clone().into_rs_owned(local_types);
        let fn_contract = match &self.typ {
            FunctionType::Method {
                typ: MethodType::InstanceMethod,
                ..
            } => {
                quote::quote! {
                    fn #function_ident<'py>(
                        &'py self,
                        #(#param_idents: #param_types),*
                    ) -> ::pyo3::PyResult<#return_type>
                }
            }
            FunctionType::Method {
                typ: MethodType::Callable,
                ..
            } => {
                let call_fn_ident: syn::Ident = {
                    let mut i = 0;
                    loop {
                        let ident = Ident::from_py(&format!(
                            "call{}",
                            (i > 0).then(|| i.to_string()).unwrap_or_default()
                        ));
                        if !scoped_function_idents.contains(&&ident) {
                            break ident;
                        }
                        i += 1;
                    }
                }
                .try_into()?;
                quote::quote! {
                    fn #call_fn_ident<'py>(
                        &'py self,
                        #(#param_idents: #param_types),*
                    ) -> ::pyo3::PyResult<#return_type>
                }
            }
            FunctionType::Method {
                typ: MethodType::Constructor,
                ..
            } => {
                let new_fn_ident: syn::Ident = {
                    let mut i = 0;
                    loop {
                        let ident = Ident::from_py(&format!(
                            "new{}",
                            (i > 0).then(|| i.to_string()).unwrap_or_default()
                        ));
                        if !scoped_function_idents.contains(&&ident) {
                            break ident;
                        }
                        i += 1;
                    }
                }
                .try_into()?;
                quote::quote! {
                    pub fn #new_fn_ident<'py>(
                        py: ::pyo3::marker::Python<'py>,
                        #(#param_idents: #param_types),*
                    ) -> ::pyo3::PyResult<::pyo3::Bound<'py, Self>>
                }
            }
            _ => {
                quote::quote! {
                    pub fn #function_ident<'py>(
                        py: ::pyo3::marker::Python<'py>,
                        #(#param_idents: #param_types),*
                    ) -> ::pyo3::PyResult<#return_type>
                }
            }
        };
        impl_fn.extend(fn_contract.clone());

        // If the function is a method with `self` as a parameter, extract the Python marker from `self`
        let maybe_extract_py = match &self.typ {
            FunctionType::Method {
                typ: MethodType::InstanceMethod | MethodType::Callable,
                ..
            } => quote::quote! {
                let py = self.py();
            },
            _ => TokenStream::new(),
        };

        // Function body (function dispatcher)
        let function_dispatcher = match &self.typ {
            FunctionType::Function | FunctionType::Closure => pyo3::Python::with_gil(|py| {
                self.name
                    .parent()
                    .unwrap_or_else(|| unreachable!())
                    .import_quote(py)
            }),
            FunctionType::Method {
                class_path,
                typ: MethodType::ClassMethod | MethodType::StaticMethod | MethodType::Constructor,
            } => pyo3::Python::with_gil(|py| class_path.import_quote(py)),
            FunctionType::Method {
                typ: MethodType::InstanceMethod | MethodType::Callable,
                ..
            } => {
                quote::quote! {
                    self
                }
            }
            FunctionType::Method {
                typ: MethodType::Unknown,
                ..
            } => {
                eprintln!(
                    "WARN: Method '{}' has an unknown type. Bindings will not be generated.",
                    self.name
                );
                return Ok(FunctionImplementation::empty_method());
            }
        };

        // Function body: positional args
        let positional_args_idents: Vec<syn::Ident> = self
            .parameters
            .iter()
            .filter(|param| {
                [
                    ParameterKind::PositionalOnly,
                    ParameterKind::PositionalOrKeyword,
                ]
                .contains(&param.kind)
            })
            .map(|param| Ok(Ident::from_py(&format!("p_{}", param.name)).try_into()?))
            .collect::<Result<_>>()?;
        let var_positional_args_ident: Option<syn::Ident> = self
            .parameters
            .iter()
            .find(|param| param.kind == ParameterKind::VarPositional)
            .and_then(|param| Ident::from_py(&format!("p_{}", param.name)).try_into().ok());
        let has_positional_args =
            !positional_args_idents.is_empty() || var_positional_args_ident.is_some();
        let positional_args = if let Some(var_positional_args_ident) = var_positional_args_ident {
            if positional_args_idents.is_empty() {
                quote::quote! {
                    #var_positional_args_ident
                }
            } else {
                let n_args_fixed = positional_args_idents.len();
                quote::quote! {
                    {
                        let mut __internal__args = Vec::with_capacity(#n_args_fixed + ::pyo3::types::PyTupleMethods::len(#var_positional_args_ident));
                        __internal__args.extend([#(::pyo3::ToPyObject::to_object(&#positional_args_idents, py),)*]);
                        __internal__args.extend(::pyo3::types::PyTupleMethods::iter(#var_positional_args_ident).map(|__internal__arg| ::pyo3::ToPyObject::to_object(&__internal__arg, py)));
                        ::pyo3::types::PyTuple::new_bound(
                            py,
                            __internal__args,
                        )
                    }
                }
            }
        } else if positional_args_idents.is_empty() {
            quote::quote! {
                ()
            }
        } else {
            quote::quote! {
                ::pyo3::types::PyTuple::new_bound(
                    py,
                    [#(::pyo3::ToPyObject::to_object(&#positional_args_idents, py),)*],
                )
            }
        };
        // Function body: keyword args
        let keyword_args: Vec<&Parameter> = self
            .parameters
            .iter()
            .filter(|param| [ParameterKind::KeywordOnly].contains(&param.kind))
            .collect_vec();
        let keyword_args_names: Vec<&str> = keyword_args
            .iter()
            .map(|param| param.name.as_py())
            .collect();
        let keyword_args_idents: Vec<syn::Ident> = keyword_args
            .iter()
            .map(|param| Ok(Ident::from_py(&format!("p_{}", param.name)).try_into()?))
            .collect::<Result<_>>()?;
        let keyword_args_idents_optional: Vec<syn::Ident> = keyword_args_idents
            .iter()
            .map(|param| quote::format_ident!("optional_{}", param))
            .collect::<_>();
        let var_keyword_args_ident: Option<syn::Ident> = self
            .parameters
            .iter()
            .find(|param| param.kind == ParameterKind::VarKeyword)
            .and_then(|param| Ident::from_py(&format!("p_{}", param.name)).try_into().ok());
        let has_keyword_args = !keyword_args_idents.is_empty() || var_keyword_args_ident.is_some();
        let keyword_args = if let Some(var_keyword_args_ident) = var_keyword_args_ident {
            if keyword_args_idents.is_empty() {
                quote::quote! {
                    #var_keyword_args_ident
                }
            } else {
                //let option_ident: syn::Ident = Ident::from_py(&format!("optional_{}", param.name)).try_into().unwrap();
                quote::quote! {
                    {
                        let __internal__kwargs = #var_keyword_args_ident;
                        #(
                            if format_ident!("optional{}", keyword_args_idents) {
                                ::pyo3::types::PyDictMethods::set_item(&__internal__kwargs, ::pyo3::intern!(py, #keyword_args_names), #keyword_args_idents);
                            };
                        )*
                        __internal__kwargs
                    }
                }
            }
        } else if keyword_args_idents.is_empty() {
            quote::quote! {
                ::pyo3::types::PyDict::new_bound(py)
            }
        } else {
            quote::quote! {
                {
                    let __internal__kwargs = ::pyo3::types::PyDict::new_bound(py);
                    #(
                        if #keyword_args_idents_optional {
                            ::pyo3::types::PyDictMethods::set_item(&__internal__kwargs, ::pyo3::intern!(py, #keyword_args_names), #keyword_args_idents);
                        };
                    )*
                    __internal__kwargs
                }
            }
        };
        // Function body: call
        let call = if let FunctionType::Method {
            typ: MethodType::Constructor | MethodType::Callable,
            ..
        } = &self.typ
        {
            if has_keyword_args {
                quote::quote! {
                    ::pyo3::types::PyAnyMethods::call(#function_dispatcher.as_any(), #positional_args, Some(&#keyword_args))
                }
            } else if has_positional_args {
                quote::quote! {
                    ::pyo3::types::PyAnyMethods::call1(#function_dispatcher.as_any(), #positional_args)
                }
            } else {
                quote::quote! {
                    ::pyo3::types::PyAnyMethods::call0(#function_dispatcher.as_any())
                }
            }
        } else {
            let method_name = self.name.name().as_py();
            if has_keyword_args {
                quote::quote! {
                    ::pyo3::types::PyAnyMethods::call_method(#function_dispatcher.as_any(), ::pyo3::intern!(py, #method_name), #positional_args, Some(&#keyword_args))
                }
            } else if has_positional_args {
                quote::quote! {
                    ::pyo3::types::PyAnyMethods::call_method1(#function_dispatcher.as_any(), ::pyo3::intern!(py, #method_name), #positional_args)
                }
            } else {
                quote::quote! {
                    ::pyo3::types::PyAnyMethods::call_method0(#function_dispatcher.as_any(), ::pyo3::intern!(py, #method_name))
                }
            }
        };

        // Function body
        impl_fn.extend(quote::quote! {
            {
                #maybe_extract_py
                #param_preprocessing
                ::pyo3::types::PyAnyMethods::extract(
                    &#call?
                )
            }
        });

        Ok(match &self.typ {
            FunctionType::Method {
                typ: MethodType::InstanceMethod | MethodType::Callable,
                ..
            } => FunctionImplementation::Method(TraitMethod {
                trait_fn: quote::quote! { #fn_contract ; },
                impl_fn,
            }),
            _ => FunctionImplementation::Function(impl_fn),
        })
    }
}

#[derive(Debug, Clone)]
struct Parameter {
    name: Ident,
    kind: ParameterKind,
    annotation: Type,
    default: Option<pyo3::Py<pyo3::types::PyAny>>,
}

impl PartialEq for Parameter {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
            && self.kind == other.kind
            && self.annotation == other.annotation
            && self.default.is_some() == other.default.is_some()
    }
}

impl Eq for Parameter {}

impl std::hash::Hash for Parameter {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.name.hash(state);
        self.kind.hash(state);
        self.annotation.hash(state);
        self.default.is_some().hash(state);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ParameterKind {
    PositionalOnly,
    PositionalOrKeyword,
    VarPositional,
    KeywordOnly,
    VarKeyword,
}

impl From<u8> for ParameterKind {
    fn from(kind: u8) -> Self {
        match kind {
            0 => Self::PositionalOnly,
            1 => Self::PositionalOrKeyword,
            2 => Self::VarPositional,
            3 => Self::KeywordOnly,
            4 => Self::VarKeyword,
            _ => unreachable!(),
        }
    }
}
