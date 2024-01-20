//! Module for handling Rust, Python and `PyO3` types.

/// Map a Python type to a Rust type.
///
/// Note that this is not a complete mapping at the moment. The public API is
/// subject to large changes.
///
/// # Arguments
///
/// * `attr_type` - The Python type to map (either a `PyType` or a `PyString`).
/// * `owned` - Whether the Rust type should be owned or not (e.g. `String` vs `&str`).
///
/// # Returns
///
/// The Rust type as a `TokenStream`.
// TODO: Support more complex type conversions
// TODO: Support module-wide classes/types for which bindings are generated
// TODO: Return `syn::Type` instead
// TODO: Refactor into something more elegant
pub fn map_attr_type(
    attr_type: &pyo3::PyAny,
    owned: bool,
) -> Result<proc_macro2::TokenStream, pyo3::PyErr> {
    Ok(
        if let Ok(attr_type) = attr_type.downcast::<pyo3::types::PyType>() {
            match attr_type {
                _string
                    if attr_type.is_subclass_of::<pyo3::types::PyString>()?
                        || attr_type.is_subclass_of::<pyo3::types::PyUnicode>()? =>
                {
                    if owned {
                        quote::quote! {
                            ::std::string::String
                        }
                    } else {
                        quote::quote! {
                            &str
                        }
                    }
                }
                _bytes if attr_type.is_subclass_of::<pyo3::types::PyBytes>()? => {
                    if owned {
                        quote::quote! {
                            Vec<u8>
                        }
                    } else {
                        quote::quote! {
                            &[u8]
                        }
                    }
                }
                _bool if attr_type.is_subclass_of::<pyo3::types::PyBool>()? => {
                    quote::quote! {
                        bool
                    }
                }
                _int if attr_type.is_subclass_of::<pyo3::types::PyLong>()? => {
                    quote::quote! {
                        i64
                    }
                }
                _float if attr_type.is_subclass_of::<pyo3::types::PyFloat>()? => {
                    quote::quote! {
                        f64
                    }
                }
                // complex if attr_type.is_subclass_of::<pyo3::types::PyComplex>()? => {
                //     quote::quote! {
                //         todo!()
                //     }
                // }
                _list if attr_type.is_subclass_of::<pyo3::types::PyList>()? => {
                    quote::quote! {
                        Vec<&'py ::pyo3::types::PyAny>
                    }
                }
                _dict if attr_type.is_subclass_of::<pyo3::types::PyDict>()? => {
                    quote::quote! {
                        &'py ::pyo3::types::PyDict
                    }
                }
                _tuple if attr_type.is_subclass_of::<pyo3::types::PyTuple>()? => {
                    quote::quote! {
                        &'py ::pyo3::types::PyTuple
                    }
                }
                _set if attr_type.is_subclass_of::<pyo3::types::PySet>()? => {
                    quote::quote! {
                        &'py ::pyo3::types::PySet
                    }
                }
                _frozenset if attr_type.is_subclass_of::<pyo3::types::PyFrozenSet>()? => {
                    quote::quote! {
                        &'py ::pyo3::types::PyFrozenSet
                    }
                }
                _bytearray if attr_type.is_subclass_of::<pyo3::types::PyByteArray>()? => {
                    if owned {
                        quote::quote! {
                            Vec<u8>
                        }
                    } else {
                        quote::quote! {
                            &[u8]
                        }
                    }
                }
                _slice if attr_type.is_subclass_of::<pyo3::types::PySlice>()? => {
                    quote::quote! {
                        &'py ::pyo3::types::PySlice
                    }
                }
                _type if attr_type.is_subclass_of::<pyo3::types::PyType>()? => {
                    quote::quote! {
                        &'py ::pyo3::types::PyType
                    }
                }
                _module if attr_type.is_subclass_of::<pyo3::types::PyModule>()? => {
                    quote::quote! {
                        &'py ::pyo3::types::PyModule
                    }
                }
                // collections_abc_Buffer
                //     if attr_type.is_subclass_of::<pyo3::types::PyBuffer<T>>()? =>
                // {
                //     quote::quote! {
                //         &'py ::pyo3::types::PyBuffer<T>
                //     }
                // }
                #[cfg(not(Py_LIMITED_API))]
                _datetime_datetime if attr_type.is_subclass_of::<pyo3::types::PyDateTime>()? => {
                    quote::quote! {
                        &'py ::pyo3::types::PyDateTime
                    }
                }
                #[cfg(not(Py_LIMITED_API))]
                _datetime_date if attr_type.is_subclass_of::<pyo3::types::PyDate>()? => {
                    quote::quote! {
                        &'py ::pyo3::types::PyDate
                    }
                }
                #[cfg(not(Py_LIMITED_API))]
                _datetime_time if attr_type.is_subclass_of::<pyo3::types::PyTime>()? => {
                    quote::quote! {
                        &'py ::pyo3::types::PyTime
                    }
                }
                #[cfg(not(Py_LIMITED_API))]
                _datetime_tzinfo if attr_type.is_subclass_of::<pyo3::types::PyTzInfo>()? => {
                    quote::quote! {
                        &'py ::pyo3::types::PyTzInfo
                    }
                }
                #[cfg(not(Py_LIMITED_API))]
                _timedelta if attr_type.is_subclass_of::<pyo3::types::PyDelta>()? => {
                    quote::quote! {
                        ::std::time::Duration
                    }
                }
                _unknown => {
                    quote::quote! {
                        &'py ::pyo3::types::PyAny
                    }
                }
            }
        } else if let Ok(attr_type) = attr_type.downcast::<pyo3::types::PyString>() {
            let attr_type = attr_type.to_str()?;
            match attr_type {
                "str" => {
                    if owned {
                        quote::quote! {
                            ::std::string::String
                        }
                    } else {
                        quote::quote! {
                            &str
                        }
                    }
                }
                "bytes" => {
                    if owned {
                        quote::quote! {
                            Vec<u8>
                        }
                    } else {
                        quote::quote! {
                            &[u8]
                        }
                    }
                }
                "bool" => {
                    quote::quote! {
                        bool
                    }
                }
                "int" => {
                    quote::quote! {
                        i64
                    }
                }
                "float" => {
                    quote::quote! {
                        f64
                    }
                }
                "complex" => {
                    quote::quote! {
                        &'py ::pyo3::types::PyComplex
                    }
                }
                list if list.starts_with("list[") && list.ends_with(']') => {
                    // TODO: Concrete type parsing for lists
                    quote::quote! {
                        Vec<&'py ::pyo3::types::PyAny>
                    }
                }
                dict if dict.starts_with("dict[") && dict.ends_with(']') => {
                    let (key, value) = dict
                        .strip_prefix("dict[")
                        .unwrap()
                        .strip_suffix(']')
                        .unwrap()
                        .split_once(',')
                        .unwrap();
                    match (key, value) {
                        ("str", "Any") => {
                            quote::quote! {
                                ::std::collections::HashMap<String, &'py ::pyo3::types::PyAny>
                            }
                        }
                        ("str", "str") => {
                            quote::quote! {
                                ::std::collections::HashMap<String, String>
                            }
                        }
                        ("str", "bool") => {
                            quote::quote! {
                                ::std::collections::HashMap<String, bool>
                            }
                        }
                        ("str", "int") => {
                            quote::quote! {
                                ::std::collections::HashMap<String, i64>
                            }
                        }
                        ("str", "float") => {
                            quote::quote! {
                                ::std::collections::HashMap<String, f64>
                            }
                        }
                        _unknown => {
                            quote::quote! {
                                &'py ::pyo3::types::PyDict
                            }
                        }
                    }
                }
                tuple if tuple.starts_with("tuple[") && tuple.ends_with(']') => {
                    // TODO: Concrete type parsing for tuple
                    quote::quote! {
                        &'py ::pyo3::types::PyTuple
                    }
                }
                set if set.starts_with("set[") && set.ends_with(']') => {
                    // TODO: Concrete type parsing for set
                    quote::quote! {
                        &'py ::pyo3::types::PySet
                    }
                }
                frozenset if frozenset.starts_with("frozenset[") && frozenset.ends_with(']') => {
                    // TODO: Concrete type parsing for frozenset
                    quote::quote! {
                        &'py ::pyo3::types::PyFrozenSet
                    }
                }
                "bytearray" => {
                    if owned {
                        quote::quote! {
                            Vec<u8>
                        }
                    } else {
                        quote::quote! {
                            &[u8]
                        }
                    }
                }
                "slice" => {
                    quote::quote! {
                        &'py ::pyo3::types::PySlice
                    }
                }
                "type" => {
                    quote::quote! {
                        &'py ::pyo3::types::PyType
                    }
                }
                "module" => {
                    quote::quote! {
                        &'py ::pyo3::types::PyModule
                    }
                }
                // "collections.abc.Buffer" => {
                //     quote::quote! {
                //         todo!()
                //     }
                // }
                "datetime.datetime" => {
                    quote::quote! {
                        &'py ::pyo3::types::PyDateTime
                    }
                }
                "datetime.date" => {
                    quote::quote! {
                        &'py ::pyo3::types::PyDate
                    }
                }
                "datetime.time" => {
                    quote::quote! {
                        &'py ::pyo3::types::PyTime
                    }
                }
                "datetime.tzinfo" => {
                    quote::quote! {
                        &'py ::pyo3::types::PyTzInfo
                    }
                }
                "timedelta" => {
                    quote::quote! {
                        ::std::time::Duration
                    }
                }
                // "decimal.Decimal" => {
                //     quote::quote! {
                //         todo!()
                //     }
                // }
                "ipaddress.IPv4Address" => {
                    quote::quote! {
                        ::std::net::IpV4Addr
                    }
                }
                "ipaddress.IPv6Address" => {
                    quote::quote! {
                        ::std::net::IpV6Addr
                    }
                }
                "os.PathLike" | "pathlib.Path" => {
                    if owned {
                        quote::quote! {
                            ::std::path::PathBuf
                        }
                    } else {
                        quote::quote! {
                            &::std::path::Path
                        }
                    }
                }
                optional if optional.ends_with(" | None") => {
                    let optional_type = optional.split_once('|').unwrap().0.trim_end();
                    match optional_type {
                        "str" => {
                            if owned {
                                quote::quote! {
                                    ::std::option::Option<String>
                                }
                            } else {
                                quote::quote! {
                                    ::std::option::Option<&str>
                                }
                            }
                        }
                        "bool" => {
                            quote::quote! {
                                ::std::option::Option<bool>
                            }
                        }
                        "int" => {
                            quote::quote! {
                                ::std::option::Option<i64>
                            }
                        }
                        "float" => {
                            quote::quote! {
                                ::std::option::Option<f64>
                            }
                        }
                        _unknown => {
                            quote::quote! {
                                ::std::option::Option<&'py ::pyo3::types::PyAny>
                            }
                        }
                    }
                }
                _unknown => {
                    quote::quote! {
                        &'py ::pyo3::types::PyAny
                    }
                }
            }
        } else {
            quote::quote! {
                &'py ::pyo3::types::PyAny
            }
        },
    )
}
