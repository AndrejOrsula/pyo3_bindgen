use super::Type;
use crate::{PyBindgenError, Result};
use itertools::Itertools;
use pyo3::prelude::*;
use std::str::FromStr;

impl TryFrom<pyo3::Bound<'_, pyo3::types::PyAny>> for Type {
    type Error = PyBindgenError;
    fn try_from(value: pyo3::Bound<pyo3::types::PyAny>) -> Result<Self> {
        match value {
            // None -> Unknown type
            none if none.is_none() => Ok(Self::Unknown),
            // Handle PyType
            t if t.is_instance_of::<pyo3::types::PyType>() => {
                let x = t.downcast_into::<pyo3::types::PyType>().unwrap();
                Self::try_from(x)
            }
            // Handle typing
            typing
                if typing
                    .get_type()
                    .getattr(pyo3::intern!(value.py(), "__module__"))?
                    .to_string()
                    == "typing" =>
            {
                Self::from_typing(typing)
            }
            // Handle everything else as string
            _ => {
                if value.is_instance_of::<pyo3::types::PyString>() {
                    Self::from_str(
                        value
                            .downcast::<pyo3::types::PyString>()
                            .unwrap()
                            .to_str()?,
                    )
                } else {
                    Self::from_str(&value.to_string())
                }
            }
        }
    }
}

impl TryFrom<pyo3::Bound<'_, pyo3::types::PyType>> for Type {
    type Error = PyBindgenError;
    fn try_from(value: pyo3::Bound<pyo3::types::PyType>) -> Result<Self> {
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
                key_type: Box::new(Self::Unknown),
                value_type: Box::new(Self::Unknown),
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
            t if t.is_subclass_of::<pyo3::types::PyFunction>()? => Self::PyFunction {
                param_types: vec![Self::PyEllipsis],
                return_annotation: Box::new(Self::Unknown),
            },
            t if t.is_subclass_of::<pyo3::types::PyModule>()? => Self::PyModule,
            #[cfg(not(PyPy))]
            t if t.is_subclass_of::<pyo3::types::PySuper>()? => Self::PySuper,
            t if t.is_subclass_of::<pyo3::types::PyTraceback>()? => Self::PyTraceback,
            t if t.is_subclass_of::<pyo3::types::PyType>()? => Self::PyType,

            // Handle everything else as string
            _ => Self::from_str(&value.to_string())?,
        })
    }
}

impl Type {
    fn from_typing(value: pyo3::Bound<pyo3::types::PyAny>) -> Result<Self> {
        let py = value.py();
        debug_assert_eq!(
            value
                .get_type()
                .getattr(pyo3::intern!(py, "__module__"))?
                .to_string(),
            "typing"
        );

        if let Ok(wrapping_type) = value.getattr(pyo3::intern!(py, "__origin__")) {
            let wrapping_type = Self::try_from(wrapping_type)?;
            Ok(
                if let Ok(inner_types) =
                    value
                        .getattr(pyo3::intern!(py, "__args__"))
                        .and_then(|inner_types| {
                            Ok(inner_types.downcast_into::<pyo3::types::PyTuple>()?)
                        })
                {
                    let inner_types = inner_types
                        .iter()
                        .map(Self::try_from)
                        .collect::<Result<Vec<_>>>()?;
                    match wrapping_type {
                        Self::Union(..) => {
                            if inner_types.len() == 2 && inner_types.contains(&Self::PyNone) {
                                Self::Optional(Box::new(
                                    inner_types
                                        .iter()
                                        .find(|x| **x != Self::PyNone)
                                        .unwrap_or_else(|| unreachable!())
                                        .to_owned(),
                                ))
                            } else {
                                Self::Union(inner_types)
                            }
                        }
                        Self::Optional(..) => {
                            // debug_assert_eq!(inner_types.len(), 1);
                            Self::Optional(Box::new(inner_types[0].clone()))
                        }
                        Self::PyDict { .. } => {
                            // debug_assert_eq!(inner_types.len(), 2);
                            Self::PyDict {
                                key_type: Box::new(inner_types[0].clone()),
                                value_type: Box::new(inner_types[1].clone()),
                            }
                        }
                        Self::PyFrozenSet(..) => {
                            // debug_assert_eq!(inner_types.len(), 1);
                            Self::PyFrozenSet(Box::new(inner_types[0].clone()))
                        }
                        Self::PyList(..) => {
                            // debug_assert_eq!(inner_types.len(), 1);
                            Self::PyList(Box::new(inner_types[0].clone()))
                        }
                        Self::PySet(..) => {
                            // debug_assert_eq!(inner_types.len(), 1);
                            Self::PySet(Box::new(inner_types[0].clone()))
                        }
                        Self::PyTuple(..) => Self::PyTuple(inner_types),
                        Self::PyFunction { .. } => {
                            // debug_assert!(!inner_types.is_empty());
                            Self::PyFunction {
                                param_types: match inner_types.len() {
                                    1 => Vec::default(),
                                    _ => inner_types[..inner_types.len() - 1].to_owned(),
                                },
                                return_annotation: Box::new(
                                    inner_types
                                        .last()
                                        .unwrap_or_else(|| unreachable!())
                                        .to_owned(),
                                ),
                            }
                        }
                        Self::PyType => {
                            // debug_assert_eq!(inner_types.len(), 1);
                            inner_types[0].clone()
                        }
                        _ => {
                            // TODO: Handle other types with inner types if useful (e.g. Generator)
                            wrapping_type
                        }
                    }
                } else {
                    // If there are no inner types, return just the wrapping type
                    wrapping_type
                },
            )
        } else {
            // Handle everything else as string
            Type::from_str(&value.to_string())
        }
    }
}

impl std::str::FromStr for Type {
    type Err = PyBindgenError;
    fn from_str(value: &str) -> Result<Self> {
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
                let inner_type = Self::from_str(
                    optional
                        .split('|')
                        .map(str::trim)
                        .find(|x| *x != "None")
                        .unwrap_or_else(|| unreachable!()),
                )?;
                Self::Optional(Box::new(inner_type))
            }
            r#union if r#union.contains('|') => {
                let mut inner_types = r#union
                    .split('|')
                    .map(|x| x.trim().to_owned())
                    .collect_vec();
                repair_complex_sequence(&mut inner_types, ',');
                let inner_types = inner_types
                    .iter()
                    .map(|x| Self::from_str(x))
                    .collect::<Result<_>>()?;
                Self::Union(inner_types)
            }
            "Union" => Self::Union(vec![Self::Unknown]),
            "" | "None" | "NoneType" => Self::PyNone,

            // Collections
            dict if dict.starts_with("dict[") && dict.ends_with(']') => {
                let mut inner_types = dict
                    .strip_prefix("dict[")
                    .unwrap_or_else(|| unreachable!())
                    .strip_suffix(']')
                    .unwrap_or_else(|| unreachable!())
                    .split(',')
                    .map(|x| x.trim().to_owned())
                    .collect_vec();
                repair_complex_sequence(&mut inner_types, ',');
                // debug_assert_eq!(inner_types.len(), 2);
                let inner_types = inner_types
                    .iter()
                    .map(|x| Self::from_str(x))
                    .collect::<Result<Vec<_>>>()?;
                Self::PyDict {
                    key_type: Box::new(inner_types[0].clone()),
                    value_type: Box::new(inner_types[1].clone()),
                }
            }
            "dict" | "Dict" | "Mapping" => Self::PyDict {
                key_type: Box::new(Self::Unknown),
                value_type: Box::new(Self::Unknown),
            },
            frozenset if frozenset.starts_with("frozenset[") && frozenset.ends_with(']') => {
                let inner_type = Self::from_str(
                    frozenset
                        .strip_prefix("frozenset[")
                        .unwrap_or_else(|| unreachable!())
                        .strip_suffix(']')
                        .unwrap_or_else(|| unreachable!()),
                )?;
                Self::PyFrozenSet(Box::new(inner_type))
            }
            list if list.starts_with("list[") && list.ends_with(']') => {
                let inner_type = Self::from_str(
                    list.strip_prefix("list[")
                        .unwrap_or_else(|| unreachable!())
                        .strip_suffix(']')
                        .unwrap_or_else(|| unreachable!()),
                )?;
                Self::PyList(Box::new(inner_type))
            }
            "list" => Self::PyList(Box::new(Self::Unknown)),
            sequence if sequence.starts_with("Sequence[") && sequence.ends_with(']') => {
                let inner_type = Self::from_str(
                    sequence
                        .strip_prefix("Sequence[")
                        .unwrap_or_else(|| unreachable!())
                        .strip_suffix(']')
                        .unwrap_or_else(|| unreachable!()),
                )?;
                Self::PyList(Box::new(inner_type))
            }
            "Sequence" | "Iterable" | "Iterator" => Self::PyList(Box::new(Self::Unknown)),
            iterable if iterable.starts_with("Iterable[") && iterable.ends_with(']') => {
                let inner_type = Self::from_str(
                    iterable
                        .strip_prefix("Iterable[")
                        .unwrap_or_else(|| unreachable!())
                        .strip_suffix(']')
                        .unwrap_or_else(|| unreachable!()),
                )?;
                Self::PyList(Box::new(inner_type))
            }
            iterator if iterator.starts_with("Iterator[") && iterator.ends_with(']') => {
                let inner_type = Self::from_str(
                    iterator
                        .strip_prefix("Iterator[")
                        .unwrap_or_else(|| unreachable!())
                        .strip_suffix(']')
                        .unwrap_or_else(|| unreachable!()),
                )?;
                Self::PyList(Box::new(inner_type))
            }
            set if set.starts_with("set[") && set.ends_with(']') => {
                let inner_type = Self::from_str(
                    set.strip_prefix("set[")
                        .unwrap_or_else(|| unreachable!())
                        .strip_suffix(']')
                        .unwrap_or_else(|| unreachable!()),
                )?;
                Self::PySet(Box::new(inner_type))
            }
            tuple if tuple.starts_with("tuple[") && tuple.ends_with(']') => {
                let mut inner_types = tuple
                    .strip_prefix("tuple[")
                    .unwrap_or_else(|| unreachable!())
                    .strip_suffix(']')
                    .unwrap_or_else(|| unreachable!())
                    .split(',')
                    .map(|x| x.trim().to_owned())
                    .collect_vec();
                repair_complex_sequence(&mut inner_types, ',');
                let inner_types = inner_types
                    .iter()
                    .map(|x| Self::from_str(x))
                    .collect::<Result<_>>()?;
                Self::PyTuple(inner_types)
            }
            "tuple" => Self::PyTuple(vec![Self::Unknown]),

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
            "function" => Self::PyFunction {
                param_types: vec![Self::PyEllipsis],
                return_annotation: Box::new(Self::Unknown),
            },
            callable if callable.starts_with("Callable[") && callable.ends_with(']') => {
                let mut inner_types = callable
                    .strip_prefix("Callable[")
                    .unwrap_or_else(|| unreachable!())
                    .strip_suffix(']')
                    .unwrap_or_else(|| unreachable!())
                    .split(',')
                    .map(|x| x.trim().to_owned())
                    .collect_vec();
                repair_complex_sequence(&mut inner_types, ',');
                // debug_assert!(!inner_types.is_empty());
                let inner_types = inner_types
                    .iter()
                    .map(|x| Self::from_str(x))
                    .collect::<Result<Vec<_>>>()?;
                Self::PyFunction {
                    param_types: match inner_types.len() {
                        1 => Vec::default(),
                        _ => inner_types[..inner_types.len() - 1].to_owned(),
                    },
                    return_annotation: Box::new(
                        inner_types
                            .last()
                            .unwrap_or_else(|| unreachable!())
                            .to_owned(),
                    ),
                }
            }
            "Callable" | "callable" => Self::PyFunction {
                param_types: vec![Self::PyEllipsis],
                return_annotation: Box::new(Self::Unknown),
            },
            "module" => Self::PyModule,
            #[cfg(not(PyPy))]
            "super" => Self::PySuper,
            "traceback" => Self::PyTraceback,
            typ if typ.starts_with("type[") && typ.ends_with(']') => Self::from_str(
                typ.strip_prefix("type[")
                    .unwrap_or_else(|| unreachable!())
                    .strip_suffix(']')
                    .unwrap_or_else(|| unreachable!()),
            )?,

            // classes
            class if class.starts_with("<class '") && class.ends_with("'>") => Self::from_str(
                class
                    .strip_prefix("<class '")
                    .unwrap_or_else(|| unreachable!())
                    .strip_suffix("'>")
                    .unwrap_or_else(|| unreachable!()),
            )?,

            // enums
            enume if enume.starts_with("<enum '") && enume.ends_with("'>") => Self::from_str(
                enume
                    .strip_prefix("<enum '")
                    .unwrap_or_else(|| unreachable!())
                    .strip_suffix("'>")
                    .unwrap_or_else(|| unreachable!()),
            )?,

            // typing
            typing if typing.starts_with("typing.") => Self::from_str(
                typing
                    .strip_prefix("typing.")
                    .unwrap_or_else(|| unreachable!()),
            )?,

            // collections.abc
            collections_abc if collections_abc.starts_with("collections.abc.") => Self::from_str(
                collections_abc
                    .strip_prefix("collections.abc.")
                    .unwrap_or_else(|| unreachable!()),
            )?,
            // collections
            collections if collections.starts_with("collections.") => Self::from_str(
                collections
                    .strip_prefix("collections.")
                    .unwrap_or_else(|| unreachable!()),
            )?,

            // Forbidden types
            forbidden if crate::config::FORBIDDEN_TYPE_NAMES.contains(&forbidden) => Self::PyAny,

            // Other types, that might be known (custom types of modules)
            other => Self::Other(other.to_owned()),
        })
    }
}

// TODO: Refactor `repair_complex_sequence()` into something more sensible
/// Repairs complex wrapped sequences.
fn repair_complex_sequence(sequence: &mut Vec<String>, separator: char) {
    // debug_assert!(!sequence.is_empty());
    // debug_assert!({
    //     let merged_sequence = sequence.iter().join("");
    //     merged_sequence.matches('[').count() == merged_sequence.matches(']').count()
    // });

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
                        new_element = format!("{new_element}{separator}{relevant_element}");
                    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_repair_complex_sequence() {
        // Arrange
        let mut sequence = vec!["dict[str".to_string(), "Any]".to_string()];

        // Act
        repair_complex_sequence(&mut sequence, ',');

        // Assert
        assert_eq!(sequence, vec!["dict[str,Any]".to_string()]);
    }
}
