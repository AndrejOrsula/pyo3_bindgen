//! Module for handling Rust, Python and `PyO3` types.
// TODO: Remove allow once impl is finished
#![allow(unused)]

use std::str::FromStr;

/// Enum that maps Python types to Rust types.
///
/// Note that this is not a complete mapping at the moment. The public API is
/// subject to large changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Type {
    PyAny,
    Unknown,

    // Primitives
    PyBool,
    PyByteArray,
    PyBytes,
    PyFloat,
    PyLong,
    PyString,

    // Enums
    // TODO: Optional causes issues when passed as a position-only argument to a function. Fix!
    Optional(Box<Type>),
    Union(Vec<Type>),

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
            _unknown => Self::Unknown,
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
                        let s = value
                            .strip_prefix("<class '")
                            .unwrap()
                            .strip_suffix("'>")
                            .unwrap();
                        Self::from_str(s)?
                    }
                    _enum if value.starts_with("<enum '") && value.ends_with("'>") => {
                        let s = value
                            .strip_prefix("<enum '")
                            .unwrap()
                            .strip_suffix("'>")
                            .unwrap();
                        Self::from_str(s)?
                    }
                    _ => {
                        eprintln!(
                            "Warning: Unexpected type {value} encountered. \
                             Please report this as a bug.",
                        );
                        Self::Unknown
                    }
                }
            }
        })
    }
}

impl std::str::FromStr for Type {
    type Err = pyo3::PyErr;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let x = Ok(match value {
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
                let (t1, t2) = optional.split_once('|').unwrap();
                let t1 = t1.trim();
                let t2 = t2.trim();
                let t = if t2 == "None" { t1 } else { t2 };
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

            // Collections
            dict if dict.starts_with("dict[") => {
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
            frozenset if frozenset.starts_with("frozenset[") => {
                let t = frozenset
                    .strip_prefix("frozenset[")
                    .unwrap()
                    .strip_suffix(']')
                    .unwrap();
                Self::PyFrozenSet(Box::new(Self::from_str(t)?))
            }
            list if list.starts_with("list[") => {
                let t = list
                    .strip_prefix("list[")
                    .unwrap()
                    .strip_suffix(']')
                    .unwrap();
                Self::PyList(Box::new(Self::from_str(t)?))
            }
            sequence if sequence.starts_with("Sequence[") => {
                let t = sequence
                    .strip_prefix("Sequence[")
                    .unwrap()
                    .strip_suffix(']')
                    .unwrap();
                Self::PyList(Box::new(Self::from_str(t)?))
            }
            set if set.starts_with("set[") => {
                let t = set.strip_prefix("set[").unwrap().strip_suffix(']').unwrap();
                Self::PySet(Box::new(Self::from_str(t)?))
            }
            tuple if tuple.starts_with("tuple[") => {
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
            #[cfg(all(not(Py_LIMITED_API), not(PyPy)))]
            "frame" => Self::PyFrame,
            "function" => Self::PyFunction,
            callable if callable.starts_with("Callable[") => {
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
            "module" => Self::PyModule,
            #[cfg(not(PyPy))]
            "super" => Self::PySuper,
            "traceback" => Self::PyTraceback,
            r#type if r#type.starts_with("type[") => {
                // TODO: Use inner type for something if useful
                // let t = r#type
                //     .strip_prefix("type[")
                //     .unwrap()
                //     .strip_suffix(']')
                //     .unwrap();
                Self::PyType
            }

            // TODO: Handle classes and other types
            _unknown => Self::Unknown,
        });

        // let y = x.as_ref().unwrap();
        // if y == &Self::Unknown {
        //     let value = value.to_string();
        //     if value != "property" {
        //         dbg!(value);
        //     }
        // }

        x
    }
}

impl Type {
    pub fn from_typing(value: &pyo3::types::PyAny) -> pyo3::PyResult<Self> {
        Ok(Self::Unknown)
    }

    #[must_use]
    pub fn into_rs(self, owned: bool) -> proc_macro2::TokenStream {
        if owned {
            self.into_rs_owned()
        } else {
            self.into_rs_borrowed()
        }
    }

    #[must_use]
    pub fn into_rs_owned(self) -> proc_macro2::TokenStream {
        match self {
            Self::PyAny => {
                quote::quote! {&'py ::pyo3::types::PyAny}
            }
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
                let inner = t.into_rs_owned();
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

            // Collections
            Self::PyDict { t_key, t_value } => {
                if t_key.is_owned_hashable() {
                    let t_key = t_key.into_rs_owned();
                    let t_value = t_value.into_rs_owned();
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
                let inner = t.into_rs_owned();
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
    pub fn into_rs_borrowed(self) -> proc_macro2::TokenStream {
        match self {
            Self::PyAny => {
                quote::quote! {&'py ::pyo3::types::PyAny}
            }
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
                let inner = t.into_rs_borrowed();
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

            // Collections
            Self::PyDict { t_key, t_value } => {
                if t_key.is_owned_hashable() {
                    let t_key = t_key.into_rs_owned();
                    let t_value = t_value.into_rs_owned();
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
                let inner = t.into_rs_owned();
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
    let mut traversed_elements = false;
    while !traversed_elements {
        traversed_elements = true;
        'outer: for i in 0..(sequence.len() - 1) {
            if sequence[i].contains('[') {
                for j in (i + 1)..sequence.len() {
                    if sequence[j].contains(']') {
                        let mut new_element = String::new();
                        for inner_sequences in sequence.iter().take(j + 1).skip(i) {
                            new_element = format!("{},{}", new_element, inner_sequences);
                        }
                        sequence[i] = new_element;
                        sequence.drain((i + 1)..=j);
                        traversed_elements = false;
                        break 'outer;
                    }
                }
            }
        }
    }
}
