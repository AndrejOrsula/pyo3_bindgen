//! Module for handling Rust, Python and `PyO3` types.
// TODO: Remove allow once impl is finished
#![allow(unused)]

use itertools::Itertools;
use std::str::FromStr;

/// Enum that maps Python types to Rust types.
///
/// Note that this is not a complete mapping at the moment. The public API is
/// subject to large changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Type {
    PyAny,
    Unhandled(String),
    Unknown,

    // Primitives
    PyBool,
    PyByteArray,
    PyBytes,
    PyFloat,
    PyLong,
    PyString,

    // Enums
    Optional(Box<Type>),
    Union(Vec<Type>),
    PyNone,

    // Collections
    PyDict {
        t_key: Box<Type>,
        t_value: Box<Type>,
    },
    PyFrozenSet(Box<Type>),
    PyList(Box<Type>),
    PySet(Box<Type>),
    PyTuple(Vec<Type>),

    // Additional types - std
    IpV4Addr,
    IpV6Addr,
    Path,
    // TODO: Map `PySlice` to `std::ops::Range` if possible
    PySlice,

    // Additional types - num-complex
    // TODO: Support conversion of `PyComplex`` to `num_complex::Complex` if enabled via `num-complex` feature
    PyComplex,

    // Additional types - datetime
    #[cfg(not(Py_LIMITED_API))]
    PyDate,
    #[cfg(not(Py_LIMITED_API))]
    PyDateTime,
    PyDelta,
    #[cfg(not(Py_LIMITED_API))]
    PyTime,
    #[cfg(not(Py_LIMITED_API))]
    PyTzInfo,

    // Python-specific types
    PyCapsule,
    PyCFunction,
    #[cfg(not(Py_LIMITED_API))]
    PyCode,
    PyEllipsis,
    #[cfg(all(not(Py_LIMITED_API), not(PyPy)))]
    PyFrame,
    PyFunction,
    PyModule,
    #[cfg(not(PyPy))]
    PySuper,
    PyTraceback,
    PyType,
}

impl TryFrom<&pyo3::types::PyAny> for Type {
    type Error = pyo3::PyErr;
    fn try_from(value: &pyo3::types::PyAny) -> Result<Self, Self::Error> {
        Ok(match value {
            t if t.is_instance_of::<pyo3::types::PyType>() => {
                let t = t.downcast::<pyo3::types::PyType>()?;
                Self::try_from(t)?
            }
            s if s.is_instance_of::<pyo3::types::PyString>() => {
                let s = s.downcast::<pyo3::types::PyString>()?;
                Self::from_str(s.to_str()?)?
            }
            typing if typing.get_type().getattr("__module__")?.to_string() == "typing" => {
                Self::from_typing(typing)?
            }
            none if none.is_none() => Self::Unknown,
            // Unknown | Handle as string if possible
            _ => {
                let value = value.to_string();
                match &value {
                    _class if value.starts_with("<class '") && value.ends_with("'>") => {
                        let value = value
                            .strip_prefix("<class '")
                            .unwrap()
                            .strip_suffix("'>")
                            .unwrap();
                        Self::from_str(value)?
                    }
                    _enum if value.starts_with("<enum '") && value.ends_with("'>") => {
                        let value = value
                            .strip_prefix("<enum '")
                            .unwrap()
                            .strip_suffix("'>")
                            .unwrap();
                        Self::from_str(value)?
                    }
                    _ => Self::from_str(&value)?,
                }
            }
        })
    }
}

impl TryFrom<&pyo3::types::PyType> for Type {
    type Error = pyo3::PyErr;
    fn try_from(value: &pyo3::types::PyType) -> Result<Self, Self::Error> {
        Ok(match value {
            // Primitives
            t if t.is_subclass_of::<pyo3::types::PyBool>()? => Self::PyBool,
            t if t.is_subclass_of::<pyo3::types::PyByteArray>()? => Self::PyByteArray,
            t if t.is_subclass_of::<pyo3::types::PyBytes>()? => Self::PyBytes,
            t if t.is_subclass_of::<pyo3::types::PyFloat>()? => Self::PyFloat,
            t if t.is_subclass_of::<pyo3::types::PyLong>()? => Self::PyLong,
            t if t.is_subclass_of::<pyo3::types::PyString>()? => Self::PyString,

            // Collections
            t if t.is_subclass_of::<pyo3::types::PyDict>()? => Self::PyDict {
                t_key: Box::new(Self::Unknown),
                t_value: Box::new(Self::Unknown),
            },
            t if t.is_subclass_of::<pyo3::types::PyFrozenSet>()? => {
                Self::PyFrozenSet(Box::new(Self::Unknown))
            }
            t if t.is_subclass_of::<pyo3::types::PyList>()? => {
                Self::PyList(Box::new(Self::Unknown))
            }
            t if t.is_subclass_of::<pyo3::types::PySet>()? => Self::PySet(Box::new(Self::Unknown)),
            t if t.is_subclass_of::<pyo3::types::PyTuple>()? => Self::PyTuple(vec![Self::Unknown]),

            // Additional types - std
            t if t.is_subclass_of::<pyo3::types::PySlice>()? => Self::PySlice,

            // Additional types - num-complex
            t if t.is_subclass_of::<pyo3::types::PyComplex>()? => Self::PyComplex,

            // Additional types - datetime
            #[cfg(not(Py_LIMITED_API))]
            t if t.is_subclass_of::<pyo3::types::PyDate>()? => Self::PyDate,
            #[cfg(not(Py_LIMITED_API))]
            t if t.is_subclass_of::<pyo3::types::PyDateTime>()? => Self::PyDateTime,
            #[cfg(not(Py_LIMITED_API))]
            t if t.is_subclass_of::<pyo3::types::PyDelta>()? => Self::PyDelta,
            #[cfg(not(Py_LIMITED_API))]
            t if t.is_subclass_of::<pyo3::types::PyTime>()? => Self::PyTime,
            #[cfg(not(Py_LIMITED_API))]
            t if t.is_subclass_of::<pyo3::types::PyTzInfo>()? => Self::PyTzInfo,

            // Python-specific types
            t if t.is_subclass_of::<pyo3::types::PyCapsule>()? => Self::PyCapsule,
            t if t.is_subclass_of::<pyo3::types::PyCFunction>()? => Self::PyCFunction,
            #[cfg(not(Py_LIMITED_API))]
            t if t.is_subclass_of::<pyo3::types::PyCode>()? => Self::PyCode,
            #[cfg(all(not(Py_LIMITED_API), not(PyPy)))]
            t if t.is_subclass_of::<pyo3::types::PyFrame>()? => Self::PyFrame,
            #[cfg(all(not(Py_LIMITED_API), not(PyPy)))]
            t if t.is_subclass_of::<pyo3::types::PyFunction>()? => Self::PyFunction,
            t if t.is_subclass_of::<pyo3::types::PyModule>()? => Self::PyModule,
            #[cfg(not(PyPy))]
            t if t.is_subclass_of::<pyo3::types::PySuper>()? => Self::PySuper,
            t if t.is_subclass_of::<pyo3::types::PyTraceback>()? => Self::PyTraceback,
            t if t.is_subclass_of::<pyo3::types::PyType>()? => Self::PyType,

            // Unknown | Handle as string if possible
            _ => {
                let value = value.to_string();
                match &value {
                    _class if value.starts_with("<class '") && value.ends_with("'>") => {
                        let value = value
                            .strip_prefix("<class '")
                            .unwrap()
                            .strip_suffix("'>")
                            .unwrap();
                        Self::from_str(value)?
                    }
                    _enum if value.starts_with("<enum '") && value.ends_with("'>") => {
                        let value = value
                            .strip_prefix("<enum '")
                            .unwrap()
                            .strip_suffix("'>")
                            .unwrap();
                        Self::from_str(value)?
                    }
                    _ => Self::Unhandled(value),
                }
            }
        })
    }
}

impl std::str::FromStr for Type {
    type Err = pyo3::PyErr;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Ok(match value {
            "Any" => Self::PyAny,

            // Primitives
            "bool" => Self::PyBool,
            "bytearray" => Self::PyByteArray,
            "bytes" => Self::PyBytes,
            "float" => Self::PyFloat,
            "int" => Self::PyLong,
            "str" => Self::PyString,

            // Enums
            optional
                if optional.matches('|').count() == 1 && optional.matches("None").count() == 1 =>
            {
                let t = optional
                    .split('|')
                    .map(str::trim)
                    .find(|x| *x != "None")
                    .unwrap();
                Self::Optional(Box::new(Self::from_str(t)?))
            }
            r#union if r#union.contains('|') => {
                let mut t_sequence = r#union
                    .split('|')
                    .map(|x| x.trim().to_string())
                    .collect::<Vec<_>>();
                ugly_hack_repair_complex_split_sequence(&mut t_sequence);
                Self::Union(
                    t_sequence
                        .iter()
                        .map(|x| Self::from_str(x))
                        .collect::<Result<Vec<_>, _>>()?,
                )
            }
            "None" | "NoneType" => Self::PyNone,

            // Collections
            dict if dict.starts_with("dict[") && dict.ends_with(']') => {
                let (key, value) = dict
                    .strip_prefix("dict[")
                    .unwrap()
                    .strip_suffix(']')
                    .unwrap()
                    .split_once(',')
                    .unwrap();
                let key = key.trim();
                let value = value.trim();
                Self::PyDict {
                    t_key: Box::new(Self::from_str(key)?),
                    t_value: Box::new(Self::from_str(value)?),
                }
            }
            "dict" | "Dict" => Self::PyDict {
                t_key: Box::new(Self::Unknown),
                t_value: Box::new(Self::Unknown),
            },
            frozenset if frozenset.starts_with("frozenset[") && frozenset.ends_with(']') => {
                let t = frozenset
                    .strip_prefix("frozenset[")
                    .unwrap()
                    .strip_suffix(']')
                    .unwrap();
                Self::PyFrozenSet(Box::new(Self::from_str(t)?))
            }
            list if list.starts_with("list[") && list.ends_with(']') => {
                let t = list
                    .strip_prefix("list[")
                    .unwrap()
                    .strip_suffix(']')
                    .unwrap();
                Self::PyList(Box::new(Self::from_str(t)?))
            }
            "list" => Self::PyList(Box::new(Self::Unknown)),
            sequence if sequence.starts_with("Sequence[") && sequence.ends_with(']') => {
                let t = sequence
                    .strip_prefix("Sequence[")
                    .unwrap()
                    .strip_suffix(']')
                    .unwrap();
                Self::PyList(Box::new(Self::from_str(t)?))
            }
            set if set.starts_with("set[") && set.ends_with(']') => {
                let t = set.strip_prefix("set[").unwrap().strip_suffix(']').unwrap();
                Self::PySet(Box::new(Self::from_str(t)?))
            }
            tuple if tuple.starts_with("tuple[") && tuple.ends_with(']') => {
                let mut t_sequence = tuple
                    .strip_prefix("tuple[")
                    .unwrap()
                    .strip_suffix(']')
                    .unwrap()
                    .split(',')
                    .map(|x| x.trim().to_string())
                    .collect::<Vec<_>>();
                ugly_hack_repair_complex_split_sequence(&mut t_sequence);
                Self::PyTuple(
                    t_sequence
                        .iter()
                        .map(|x| Self::from_str(x))
                        .collect::<Result<Vec<_>, _>>()?,
                )
            }

            // Additional types - std
            "ipaddress.IPv4Address" => Self::IpV4Addr,
            "ipaddress.IPv6Address" => Self::IpV6Addr,
            "os.PathLike" | "pathlib.Path" => Self::Path,
            "slice" => Self::PySlice,

            // Additional types - num-complex
            "complex" => Self::PyComplex,

            // Additional types - datetime
            #[cfg(not(Py_LIMITED_API))]
            "datetime.date" => Self::PyDate,
            #[cfg(not(Py_LIMITED_API))]
            "datetime.datetime" => Self::PyDateTime,
            "timedelta" => Self::PyDelta,
            #[cfg(not(Py_LIMITED_API))]
            "datetime.time" => Self::PyTime,
            #[cfg(not(Py_LIMITED_API))]
            "datetime.tzinfo" => Self::PyTzInfo,

            // Python-specific types
            "capsule" => Self::PyCapsule,
            "cfunction" => Self::PyCFunction,
            #[cfg(not(Py_LIMITED_API))]
            "code" => Self::PyCode,
            "Ellipsis" | "..." => Self::PyEllipsis,
            #[cfg(all(not(Py_LIMITED_API), not(PyPy)))]
            "frame" => Self::PyFrame,
            "function" => Self::PyFunction,
            callable if callable.starts_with("Callable[") && callable.ends_with(']') => {
                // TODO: Use callable types for something if useful
                // let (args, return_value) = callable
                //     .strip_prefix("Callable[")
                //     .unwrap()
                //     .strip_suffix(']')
                //     .unwrap()
                //     .split_once(',')
                //     .unwrap();
                // let args = args
                //     .strip_prefix("[")
                //     .unwrap()
                //     .strip_suffix("]")
                //     .unwrap()
                //     .split(',')
                //     .map(|x| x.trim())
                //     .collect::<Vec<_>>();
                // let return_value = return_value.trim();
                Self::PyFunction
            }
            "Callable" | "callable" => Self::PyFunction,
            "module" => Self::PyModule,
            #[cfg(not(PyPy))]
            "super" => Self::PySuper,
            "traceback" => Self::PyTraceback,
            typ if typ.starts_with("type[") && typ.ends_with(']') => {
                // TODO: Use inner type for something if useful
                // let t = typ
                //     .strip_prefix("type[")
                //     .unwrap()
                //     .strip_suffix(']')
                //     .unwrap();
                Self::PyType
            }

            // typing
            typing if typing.starts_with("typing.") => {
                let s = typing.strip_prefix("typing.").unwrap();
                Self::from_str(s)?
            }

            // collection.abc
            collection if collection.starts_with("collection.abc.") => {
                let s = collection.strip_prefix("collection.abc.").unwrap();
                Self::from_str(s)?
            }

            unhandled => Self::Unhandled(unhandled.to_owned()),
        })
    }
}

impl Type {
    pub fn from_typing(value: &pyo3::types::PyAny) -> pyo3::PyResult<Self> {
        if let (Ok(t), Ok(t_inner)) = (value.getattr("__origin__"), value.getattr("__args__")) {
            let t_inner = t_inner.downcast::<pyo3::types::PyTuple>()?;

            if t.is_instance_of::<pyo3::types::PyType>() {
                let t = t.downcast::<pyo3::types::PyType>()?;
                match Self::try_from(t)? {
                    Self::PyDict { .. } => {
                        let (t_key, t_value) = (
                            Self::try_from(t_inner.get_item(0)?)?,
                            Self::try_from(t_inner.get_item(1)?)?,
                        );
                        return Ok(Self::PyDict {
                            t_key: Box::new(t_key),
                            t_value: Box::new(t_value),
                        });
                    }
                    Self::PyList(..) => {
                        let t_inner = Self::try_from(t_inner.get_item(0)?)?;
                        return Ok(Self::PyList(Box::new(t_inner)));
                    }
                    Self::PyTuple(..) => {
                        let t_sequence = t_inner
                            .iter()
                            .map(Self::try_from)
                            .collect::<Result<Vec<_>, _>>()?;
                        return Ok(Self::PyTuple(t_sequence));
                    }
                    Self::PyType => {
                        // TODO: See if the inner type is useful for something here
                        return Ok(Self::PyType);
                    }
                    _ => {
                        // Noop - processed as string below
                        // eprintln!(
                        //     "Warning: Unexpected type encountered: {value}\n \
                        //      Bindings could be improved by handling the type here \
                        //      Please report this as a bug. [scope: Type::from_typing()]",
                        // );
                    }
                }
            }

            let t = t.to_string();
            Ok(match &t {
                _typing if t.starts_with("typing.") => {
                    let t = t.strip_prefix("typing.").unwrap();
                    match t {
                        "Union" => {
                            let t_sequence = t_inner
                                .iter()
                                .map(Self::try_from)
                                .collect::<Result<Vec<_>, _>>()?;

                            if t_sequence.len() == 2 && t_sequence.contains(&Self::PyNone) {
                                let t = t_sequence
                                    .iter()
                                    .find(|x| **x != Self::PyNone)
                                    .unwrap()
                                    .clone();
                                Self::Optional(Box::new(t))
                            } else {
                                Self::Union(t_sequence)
                            }
                        }
                        _ => Self::Unhandled(value.to_string()),
                    }
                }
                _collections if t.starts_with("<class 'collections.abc") && t.ends_with("'>") => {
                    let t = t
                        .strip_prefix("<class 'collections.abc.")
                        .unwrap()
                        .strip_suffix("'>")
                        .unwrap();
                    match t {
                        "Iterable" | "Sequence" => {
                            let t_inner = Self::try_from(t_inner.get_item(0)?)?;
                            Self::PyList(Box::new(t_inner))
                        }
                        "Callable" => {
                            // TODO: Use callable types for something if useful (t_inner)
                            Self::PyFunction
                        }
                        _ => Self::Unhandled(value.to_string()),
                    }
                }
                // Unknown | Handle the type as string if possible
                _ => {
                    // TODO: Handle also the inner type here if possible
                    let t = t.to_string();
                    match &t {
                        _class if t.starts_with("<class '") && t.ends_with("'>") => {
                            let t = t
                                .strip_prefix("<class '")
                                .unwrap()
                                .strip_suffix("'>")
                                .unwrap();
                            Self::from_str(t)?
                        }
                        _enum if t.starts_with("<enum '") && t.ends_with("'>") => {
                            let t = t
                                .strip_prefix("<enum '")
                                .unwrap()
                                .strip_suffix("'>")
                                .unwrap();
                            Self::from_str(t)?
                        }
                        _ => Self::from_str(&t)?,
                    }
                }
            })
        } else {
            let value = value.to_string();
            Type::from_str(&value)
        }
    }

    #[must_use]
    pub fn into_rs<S: ::std::hash::BuildHasher + Default>(
        self,
        owned: bool,
        module_name: &str,
        all_types: &std::collections::HashSet<String, S>,
    ) -> proc_macro2::TokenStream {
        if owned {
            self.into_rs_owned(module_name, all_types)
        } else {
            self.into_rs_borrowed(module_name, all_types)
        }
    }

    #[must_use]
    pub fn into_rs_owned<S: ::std::hash::BuildHasher + Default>(
        self,
        module_name: &str,
        all_types: &std::collections::HashSet<String, S>,
    ) -> proc_macro2::TokenStream {
        match self {
            Self::PyAny => {
                quote::quote! {&'py ::pyo3::types::PyAny}
            }
            Self::Unhandled(..) => self.try_into_module_path(module_name, all_types),

            Self::Unknown => {
                quote::quote! {&'py ::pyo3::types::PyAny}
            }

            // Primitives
            Self::PyBool => {
                quote::quote! {bool}
            }
            Self::PyByteArray | Self::PyBytes => {
                quote::quote! {Vec<u8>}
            }
            Self::PyFloat => {
                quote::quote! {f64}
            }
            Self::PyLong => {
                quote::quote! {i64}
            }
            Self::PyString => {
                quote::quote! {::std::string::String}
            }

            // Enums
            Self::Optional(t) => {
                let inner = t.into_rs_owned(module_name, all_types);
                quote::quote! {
                    ::std::option::Option<#inner>
                }
            }
            Self::Union(t_alternatives) => {
                // TODO: Support Rust enum where possible
                quote::quote! {
                    &'py ::pyo3::types::PyAny
                }
            }
            Self::PyNone => {
                // TODO: Not sure what to do with None
                quote::quote! {
                    &'py ::pyo3::types::PyAny
                }
            }

            // Collections
            Self::PyDict { t_key, t_value } => {
                if t_key.is_owned_hashable() {
                    let t_key = t_key.into_rs_owned(module_name, all_types);
                    let t_value = t_value.into_rs_owned(module_name, all_types);
                    quote::quote! {
                        ::std::collections::HashMap<#t_key, #t_value>
                    }
                } else {
                    quote::quote! {
                        &'py ::pyo3::types::PyDict
                    }
                }
            }
            Self::PyFrozenSet(t) => {
                // TODO: Support Rust HashSet where possible
                quote::quote! {
                    &'py ::pyo3::types::PyFrozenSet
                }
            }
            Self::PyList(t) => {
                let inner = t.into_rs_owned(module_name, all_types);
                quote::quote! {
                    Vec<#inner>
                }
            }
            Self::PySet(t) => {
                // TODO: Support Rust HashSet where possible
                quote::quote! {
                    &'py ::pyo3::types::PySet
                }
            }
            Self::PyTuple(t_sequence) => {
                // TODO: Support Rust tuple where possible
                quote::quote! {
                    &'py ::pyo3::types::PyTuple
                }
            }

            // Additional types - std
            Self::IpV4Addr => {
                quote::quote! {::std::net::IpV4Addr}
            }
            Self::IpV6Addr => {
                quote::quote! {::std::net::IpV6Addr}
            }
            Self::Path => {
                quote::quote! {::std::path::PathBuf}
            }
            Self::PySlice => {
                quote::quote! {&'py ::pyo3::types::PySlice}
            }

            // Additional types - num-complex
            Self::PyComplex => {
                quote::quote! {&'py ::pyo3::types::PyComplex}
            }

            // Additional types - datetime
            #[cfg(not(Py_LIMITED_API))]
            Self::PyDate => {
                quote::quote! {&'py ::pyo3::types::PyDate}
            }
            #[cfg(not(Py_LIMITED_API))]
            Self::PyDateTime => {
                quote::quote! {&'py ::pyo3::types::PyDateTime}
            }
            Self::PyDelta => {
                quote::quote! {::std::time::Duration}
            }
            #[cfg(not(Py_LIMITED_API))]
            Self::PyTime => {
                quote::quote! {&'py ::pyo3::types::PyTime}
            }
            #[cfg(not(Py_LIMITED_API))]
            Self::PyTzInfo => {
                quote::quote! {&'py ::pyo3::types::PyTzInfo}
            }

            // Python-specific types
            Self::PyCapsule => {
                quote::quote! {&'py ::pyo3::types::PyCapsule}
            }
            Self::PyCFunction => {
                quote::quote! {&'py ::pyo3::types::PyCFunction}
            }
            #[cfg(not(Py_LIMITED_API))]
            Self::PyCode => {
                quote::quote! {&'py ::pyo3::types::PyCode}
            }
            Self::PyEllipsis => {
                // TODO: Not sure what to do with ellipsis
                quote::quote! {
                    &'py ::pyo3::types::PyAny
                }
            }
            #[cfg(all(not(Py_LIMITED_API), not(PyPy)))]
            Self::PyFrame => {
                quote::quote! {&'py ::pyo3::types::PyFrame}
            }
            #[cfg(all(not(Py_LIMITED_API), not(PyPy)))]
            Self::PyFunction => {
                quote::quote! {&'py ::pyo3::types::PyFunction}
            }
            #[cfg(not(all(not(Py_LIMITED_API), not(PyPy))))]
            Self::PyFunction => {
                quote::quote! {&'py ::pyo3::types::PyAny}
            }
            Self::PyModule => {
                quote::quote! {&'py ::pyo3::types::PyModule}
            }
            #[cfg(not(PyPy))]
            Self::PySuper => {
                quote::quote! {&'py ::pyo3::types::PySuper}
            }
            Self::PyTraceback => {
                quote::quote! {&'py ::pyo3::types::PyTraceback}
            }
            Self::PyType => {
                quote::quote! {&'py ::pyo3::types::PyType}
            }
        }
    }

    #[must_use]
    pub fn into_rs_borrowed<S: ::std::hash::BuildHasher + Default>(
        self,
        module_name: &str,
        all_types: &std::collections::HashSet<String, S>,
    ) -> proc_macro2::TokenStream {
        match self {
            Self::PyAny => {
                quote::quote! {&'py ::pyo3::types::PyAny}
            }
            Self::Unhandled(..) => self.try_into_module_path(module_name, all_types),
            Self::Unknown => {
                quote::quote! {&'py ::pyo3::types::PyAny}
            }

            // Primitives
            Self::PyBool => {
                quote::quote! {bool}
            }
            Self::PyByteArray | Self::PyBytes => {
                quote::quote! {&[u8]}
            }
            Self::PyFloat => {
                quote::quote! {f64}
            }
            Self::PyLong => {
                quote::quote! {i64}
            }
            Self::PyString => {
                quote::quote! {&str}
            }

            // Enums
            Self::Optional(t) => {
                let inner = t.into_rs_owned(module_name, all_types);
                quote::quote! {
                    ::std::option::Option<#inner>
                }
            }
            Self::Union(t_alternatives) => {
                // TODO: Support Rust enum where possible
                quote::quote! {
                    &'py ::pyo3::types::PyAny
                }
            }
            Self::PyNone => {
                // TODO: Not sure what to do with None
                quote::quote! {
                    &'py ::pyo3::types::PyAny
                }
            }

            // Collections
            Self::PyDict { t_key, t_value } => {
                if t_key.is_owned_hashable() {
                    let t_key = t_key.into_rs_owned(module_name, all_types);
                    let t_value = t_value.into_rs_owned(module_name, all_types);
                    quote::quote! {
                        &::std::collections::HashMap<#t_key, #t_value>
                    }
                } else {
                    quote::quote! {
                        &'py ::pyo3::types::PyDict
                    }
                }
            }
            Self::PyFrozenSet(t) => {
                // TODO: Support Rust HashSet where possible
                quote::quote! {
                    &'py ::pyo3::types::PyFrozenSet
                }
            }
            Self::PyList(t) => {
                let inner = t.into_rs_owned(module_name, all_types);
                quote::quote! {
                    &[#inner]
                }
            }
            Self::PySet(t) => {
                // TODO: Support Rust HashSet where possible
                quote::quote! {
                    &'py ::pyo3::types::PySet
                }
            }
            Self::PyTuple(t_sequence) => {
                // TODO: Support Rust tuple where possible
                quote::quote! {
                    &'py ::pyo3::types::PyTuple
                }
            }

            // Additional types - std
            Self::IpV4Addr => {
                quote::quote! {::std::net::IpV4Addr}
            }
            Self::IpV6Addr => {
                quote::quote! {::std::net::IpV6Addr}
            }
            Self::Path => {
                quote::quote! {::std::path::PathBuf}
            }
            Self::PySlice => {
                quote::quote! {&'py ::pyo3::types::PySlice}
            }

            // Additional types - num-complex
            Self::PyComplex => {
                quote::quote! {&'py ::pyo3::types::PyComplex}
            }

            // Additional types - datetime
            #[cfg(not(Py_LIMITED_API))]
            Self::PyDate => {
                quote::quote! {&'py ::pyo3::types::PyDate}
            }
            #[cfg(not(Py_LIMITED_API))]
            Self::PyDateTime => {
                quote::quote! {&'py ::pyo3::types::PyDateTime}
            }
            Self::PyDelta => {
                quote::quote! {::std::time::Duration}
            }
            #[cfg(not(Py_LIMITED_API))]
            Self::PyTime => {
                quote::quote! {&'py ::pyo3::types::PyTime}
            }
            #[cfg(not(Py_LIMITED_API))]
            Self::PyTzInfo => {
                quote::quote! {&'py ::pyo3::types::PyTzInfo}
            }

            // Python-specific types
            Self::PyCapsule => {
                quote::quote! {&'py ::pyo3::types::PyCapsule}
            }
            Self::PyCFunction => {
                quote::quote! {&'py ::pyo3::types::PyCFunction}
            }
            #[cfg(not(Py_LIMITED_API))]
            Self::PyCode => {
                quote::quote! {&'py ::pyo3::types::PyCode}
            }
            Self::PyEllipsis => {
                // TODO: Not sure what to do with ellipsis
                quote::quote! {
                    &'py ::pyo3::types::PyAny
                }
            }
            #[cfg(all(not(Py_LIMITED_API), not(PyPy)))]
            Self::PyFrame => {
                quote::quote! {&'py ::pyo3::types::PyFrame}
            }
            #[cfg(all(not(Py_LIMITED_API), not(PyPy)))]
            Self::PyFunction => {
                quote::quote! {&'py ::pyo3::types::PyFunction}
            }
            #[cfg(not(all(not(Py_LIMITED_API), not(PyPy))))]
            Self::PyFunction => {
                quote::quote! {&'py ::pyo3::types::PyAny}
            }
            Self::PyModule => {
                quote::quote! {&'py ::pyo3::types::PyModule}
            }
            #[cfg(not(PyPy))]
            Self::PySuper => {
                quote::quote! {&'py ::pyo3::types::PySuper}
            }
            Self::PyTraceback => {
                quote::quote! {&'py ::pyo3::types::PyTraceback}
            }
            Self::PyType => {
                quote::quote! {&'py ::pyo3::types::PyType}
            }
        }
    }

    fn try_into_module_path<S: ::std::hash::BuildHasher + Default>(
        self,
        module_name: &str,
        all_types: &std::collections::HashSet<String, S>,
    ) -> proc_macro2::TokenStream {
        let Self::Unhandled(value) = self else {
            unreachable!()
        };
        let module_root = if module_name.contains('.') {
            module_name.split('.').next().unwrap()
        } else {
            module_name
        };
        match value.as_str() {
            // Ignorelist
            "property"
            | "member_descriptor"
            | "method_descriptor"
            | "getset_descriptor"
            | "_collections._tuplegetter"
            | "AsyncState" => {
                quote::quote! {&'py ::pyo3::types::PyAny}
            }
            module_member_full if module_member_full.starts_with(module_root) => {
                // Ignore unknown types
                if !all_types.contains(module_member_full) {
                    return quote::quote! {&'py ::pyo3::types::PyAny};
                }

                let value_name = module_member_full.split('.').last().unwrap();

                let n_common_ancestors = module_name
                    .split('.')
                    .zip(module_member_full.split('.'))
                    .take_while(|(a, b)| a == b)
                    .count();
                let current_module_depth = module_name.split('.').count();
                let reexport_path = if (current_module_depth - n_common_ancestors) > 0 {
                    std::iter::repeat("super".to_string())
                        .take(current_module_depth - n_common_ancestors)
                } else {
                    std::iter::repeat("self".to_string()).take(1)
                };
                let reexport_path: String = reexport_path
                    .chain(
                        module_member_full
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
                    .join("::");

                // The path contains both ident and "::", combine into something that can be quoted
                let reexport_path = syn::parse_str::<syn::Path>(&reexport_path).unwrap();
                quote::quote! {
                    &'py #reexport_path
                }
            }
            _ => {
                let value_without_brackets = value.split_once('[').unwrap_or((&value, "")).0;
                let module_scopes = value_without_brackets.split('.');
                let n_module_scopes = module_scopes.clone().count();

                // Approach: Find types without a module scope (no dot) and check if the type is local (or imported in the current module)
                if !value_without_brackets.contains('.') {
                    if let Some(member) = all_types
                        .iter()
                        .filter(|member| {
                            member
                                .split('.')
                                .take(member.split('.').count() - 1)
                                .join(".")
                                == module_name
                        })
                        .find(|&member| {
                            member.trim_start_matches(&format!("{module_name}."))
                                == value_without_brackets
                        })
                    {
                        return Self::Unhandled(member.to_owned())
                            .try_into_module_path(module_name, all_types);
                    }
                }

                // Approach: Find the shallowest match that contains the value
                // TODO: Fix this! The matching might be wrong in many cases
                let mut possible_matches = std::collections::HashSet::<String, S>::default();
                for i in 0..n_module_scopes {
                    let module_member_scopes_end = module_scopes.clone().skip(i).join(".");
                    all_types
                        .iter()
                        .filter(|member| member.ends_with(&module_member_scopes_end))
                        .for_each(|member| {
                            possible_matches.insert(member.to_owned());
                        });
                    if !possible_matches.is_empty() {
                        let shallowest_match = possible_matches
                            .iter()
                            .min_by(|m1, m2| m1.split('.').count().cmp(&m2.split('.').count()))
                            .unwrap();
                        return Self::Unhandled(shallowest_match.to_owned())
                            .try_into_module_path(module_name, all_types);
                    }
                }

                // Unsupported
                // TODO: Support more types
                // dbg!(value);
                quote::quote! {&'py ::pyo3::types::PyAny}
            }
        }
    }

    fn is_owned_hashable(&self) -> bool {
        matches!(
            self,
            Self::PyBool
                | Self::IpV4Addr
                | Self::IpV6Addr
                | Self::Path
                | Self::PyDelta
                | Self::PyDict { .. }
                | Self::PyFrozenSet(..)
                | Self::PyLong
                | Self::PySet(..)
                | Self::PyString
        )
    }
}

// TODO: Replace this with something more sensible
fn ugly_hack_repair_complex_split_sequence(sequence: &mut Vec<String>) {
    let mut traversed_all_elements = false;
    let mut start_index = 0;
    'outer: while !traversed_all_elements {
        traversed_all_elements = true;
        'inner: for i in start_index..(sequence.len() - 1) {
            let mut n_scopes = sequence[i].matches('[').count() - sequence[i].matches(']').count();
            if n_scopes == 0 {
                continue;
            }
            for j in (i + 1)..sequence.len() {
                n_scopes += sequence[j].matches('[').count();
                n_scopes -= sequence[j].matches(']').count();
                if n_scopes == 0 {
                    let mut new_element = sequence[i].clone();
                    for relevant_element in sequence.iter().take(j + 1).skip(i + 1) {
                        new_element = format!("{new_element},{relevant_element}");
                    }

                    // Update sequence and remove the elements that were merged
                    sequence[i] = new_element;
                    sequence.drain((i + 1)..=j);

                    if j < sequence.len() - 1 {
                        traversed_all_elements = false;
                        start_index = i;
                        break 'inner;
                    } else {
                        break 'outer;
                    }
                }
            }
        }
    }
}
