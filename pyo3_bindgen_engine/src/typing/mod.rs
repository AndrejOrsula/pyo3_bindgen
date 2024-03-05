pub(crate) mod from_py;
pub(crate) mod into_rs;

/// Enum that maps Python types to Rust types.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Type {
    PyAny,
    Other(String),
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
        key_type: Box<Type>,
        value_type: Box<Type>,
    },
    PyFrozenSet(Box<Type>),
    PyList(Box<Type>),
    PySet(Box<Type>),
    PyTuple(Vec<Type>),

    // Additional types - std
    IpV4Addr,
    IpV6Addr,
    Path,
    PySlice,

    // Additional types - num-complex
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
    PyFunction {
        param_types: Vec<Type>,
        return_annotation: Box<Type>,
    },
    PyModule,
    #[cfg(not(PyPy))]
    PySuper,
    PyTraceback,
    #[allow(clippy::enum_variant_names)]
    PyType,
}

impl Type {
    fn is_hashable(&self) -> bool {
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
