use super::Type;
use crate::syntax::Path;
use itertools::Itertools;
use quote::quote;
use rustc_hash::FxHashMap as HashMap;
use std::rc::Rc;

impl Type {
    pub fn into_rs_owned(self, local_types: &HashMap<Path, Path>) -> proc_macro2::TokenStream {
        let owned = self.into_rs(local_types).owned;
        Rc::into_inner(owned).unwrap_or_else(|| unreachable!())
    }

    pub fn into_rs_borrowed(self, local_types: &HashMap<Path, Path>) -> proc_macro2::TokenStream {
        let borrowed = self.into_rs(local_types).borrowed;
        Rc::into_inner(borrowed).unwrap_or_else(|| unreachable!())
    }

    pub fn preprocess_borrowed(
        &self,
        ident: &syn::Ident,
        local_types: &HashMap<Path, Path>,
    ) -> proc_macro2::TokenStream {
        match self {
            Self::PyDict {
                key_type,
                value_type,
            } if !key_type.is_hashable()
                || value_type
                    .clone()
                    .into_rs(local_types)
                    .owned
                    .to_string()
                    .contains("PyAny") =>
            {
                quote! {
                    let #ident = ::pyo3::types::IntoPyDict::into_py_dict_bound(#ident, py);
                }
            }
            Self::PyTuple(inner_types) if inner_types.len() < 2 => {
                quote! {
                    let #ident = ::pyo3::IntoPy::<::pyo3::Py<::pyo3::types::PyTuple>>::into_py(#ident, py);
                    let #ident = #ident.bind(py);
                }
            }
            Self::PyAny
            | Self::Unknown
            | Self::Union(..)
            | Self::PyNone
            | Self::PyDelta
            | Self::PyEllipsis => {
                quote! {
                    let #ident = ::pyo3::IntoPy::<::pyo3::Py<::pyo3::types::PyAny>>::into_py(#ident, py);
                    let #ident = #ident.bind(py);
                }
            }
            #[cfg(not(all(not(Py_LIMITED_API), not(PyPy))))]
            Self::PyFunction { .. } => {
                quote! {
                    let #ident = ::pyo3::IntoPy::<::pyo3::Py<::pyo3::types::PyAny>>::into_py(#ident, py);
                    let #ident = #ident.bind(py);
                }
            }
            Self::Other(type_name)
                if Self::try_map_external_type(type_name).is_none()
                    && !local_types.contains_key(&Path::from_py(type_name)) =>
            {
                quote! {
                    let #ident = ::pyo3::IntoPy::<::pyo3::Py<::pyo3::types::PyAny>>::into_py(#ident, py);
                    let #ident = #ident.bind(py);
                }
            }
            Self::Optional(inner_type) => match inner_type.as_ref() {
                Self::PyDict {
                    key_type,
                    value_type,
                } if !key_type.is_hashable()
                    || value_type
                        .clone()
                        .into_rs(local_types)
                        .owned
                        .to_string()
                        .contains("PyAny") =>
                {
                    quote! {
                        let #ident = if let Some(#ident) = #ident {
                            ::pyo3::types::IntoPyDict::into_py_dict_bound(#ident, py)
                        } else {
                            ::pyo3::types::PyDict::new_bound(py)
                        };
                    }
                }
                _ => proc_macro2::TokenStream::new(),
            },
            _ => proc_macro2::TokenStream::new(),
        }
    }

    fn into_rs(self, local_types: &HashMap<Path, Path>) -> OutputType {
        match self {
            Self::PyAny | Self::Unknown => OutputType::new(
                quote!(::pyo3::Bound<'py, ::pyo3::types::PyAny>),
                quote!(impl ::pyo3::IntoPy<::pyo3::Py<::pyo3::types::PyAny>>),
            ),
            Self::Other(..) => self.map_type(local_types),

            // Primitives
            Self::PyBool => OutputType::new_identical(quote!(bool)),
            Self::PyByteArray | Self::PyBytes => OutputType::new(quote!(Vec<u8>), quote!(&[u8])),
            Self::PyFloat => OutputType::new_identical(quote!(f64)),
            Self::PyLong => OutputType::new_identical(quote!(i64)),
            Self::PyString => OutputType::new(quote!(::std::string::String), quote!(&str)),

            // Enums
            Self::Optional(inner_type) => {
                let inner_type = inner_type.into_rs(local_types).owned;
                OutputType::new_identical(quote!(::std::option::Option<#inner_type>))
            }
            Self::Union(_inner_types) => {
                // TODO: Support Rust enums where possible | alternatively, overload functions for each variant
                OutputType::new(
                    quote!(::pyo3::Bound<'py, ::pyo3::types::PyAny>),
                    quote!(impl ::pyo3::IntoPy<::pyo3::Py<::pyo3::types::PyAny>>),
                )
            }
            Self::PyNone => {
                // TODO: Determine if PyNone is even possible
                OutputType::new(
                    quote!(::pyo3::Bound<'py, ::pyo3::types::PyAny>),
                    quote!(impl ::pyo3::IntoPy<::pyo3::Py<::pyo3::types::PyAny>>),
                )
            }

            // Collections
            Self::PyDict {
                key_type,
                value_type,
            } => {
                let value_type = value_type.into_rs(local_types).owned;
                if key_type.is_hashable() && !value_type.to_string().contains("PyAny") {
                    let key_type = key_type.into_rs(local_types).owned;
                    OutputType::new(
                        quote!(::std::collections::HashMap<#key_type, #value_type>),
                        quote!(&::std::collections::HashMap<#key_type, #value_type>),
                    )
                } else {
                    OutputType::new(
                        quote!(::pyo3::Bound<'py, ::pyo3::types::PyDict>),
                        quote!(impl ::pyo3::types::IntoPyDict),
                    )
                }
            }
            Self::PyFrozenSet(inner_type) => {
                if inner_type.is_hashable() {
                    let inner_type = inner_type.into_rs(local_types).owned;
                    OutputType::new(
                        quote!(::std::collections::HashSet<#inner_type>),
                        quote!(&::std::collections::HashSet<#inner_type>),
                    )
                } else {
                    OutputType::new(
                        quote!(::pyo3::Bound<'py, ::pyo3::types::PyFrozenSet>),
                        quote!(&::pyo3::Bound<'py, ::pyo3::types::PyFrozenSet>),
                    )
                }
            }
            Self::PyList(inner_type) => {
                let inner_type = inner_type.into_rs(local_types).owned;
                OutputType::new(quote!(Vec<#inner_type>), quote!(&[#inner_type]))
            }
            Self::PySet(inner_type) => {
                if inner_type.is_hashable() {
                    let inner_type = inner_type.into_rs(local_types).owned;
                    OutputType::new(
                        quote!(::std::collections::HashSet<#inner_type>),
                        quote!(&::std::collections::HashSet<#inner_type>),
                    )
                } else {
                    OutputType::new(
                        quote!(::pyo3::Bound<'py, ::pyo3::types::PySet>),
                        quote!(&::pyo3::Bound<'py, ::pyo3::types::PySet>),
                    )
                }
            }
            Self::PyTuple(inner_types) => {
                if inner_types.len() < 2 {
                    OutputType::new(
                        quote!(::pyo3::Bound<'py, ::pyo3::types::PyTuple>),
                        quote!(impl ::pyo3::IntoPy<::pyo3::Py<::pyo3::types::PyTuple>>),
                    )
                } else if inner_types.len() == 2
                    && *inner_types.last().unwrap_or_else(|| unreachable!()) == Self::PyEllipsis
                {
                    Self::PyList(Box::new(inner_types[0].clone())).into_rs(local_types)
                } else {
                    let inner_types = inner_types
                        .into_iter()
                        .map(|inner_type| inner_type.into_rs(local_types).owned)
                        .collect_vec();
                    OutputType::new_identical(quote!((#(#inner_types),*)))
                }
            }

            // Additional types - std
            Self::IpV4Addr => OutputType::new_identical(quote!(::std::net::IpV4Addr)),
            Self::IpV6Addr => OutputType::new_identical(quote!(::std::net::IpV6Addr)),
            Self::Path => OutputType::new(quote!(::std::path::PathBuf), quote!(&::std::path::Path)),
            // TODO: Map `PySlice` to `std::ops::Range` if possible
            Self::PySlice => OutputType::new(
                quote!(::pyo3::Bound<'py, ::pyo3::types::PySlice>),
                quote!(&::pyo3::Bound<'py, ::pyo3::types::PySlice>),
            ),

            // Additional types - num-complex
            // TODO: Support conversion of `PyComplex` to `num_complex::Complex` if enabled via `num-complex` feature
            Self::PyComplex => OutputType::new(
                quote!(::pyo3::Bound<'py, ::pyo3::types::PyComplex>),
                quote!(&::pyo3::Bound<'py, ::pyo3::types::PyComplex>),
            ),

            // Additional types - datetime
            #[cfg(not(Py_LIMITED_API))]
            Self::PyDate => OutputType::new(
                quote!(::pyo3::Bound<'py, ::pyo3::types::PyDate>),
                quote!(&::pyo3::Bound<'py, ::pyo3::types::PyDate>),
            ),
            #[cfg(not(Py_LIMITED_API))]
            Self::PyDateTime => OutputType::new(
                quote!(::pyo3::Bound<'py, ::pyo3::types::PyDateTime>),
                quote!(&::pyo3::Bound<'py, ::pyo3::types::PyDateTime>),
            ),
            Self::PyDelta => {
                // The trait `ToPyObject` is not implemented for `Duration`, so we can't use it here yet
                // OutputType::new_identical(quote!(::std::time::Duration))
                OutputType::new(
                    quote!(::pyo3::Bound<'py, ::pyo3::types::PyAny>),
                    quote!(impl ::pyo3::IntoPy<::pyo3::Py<::pyo3::types::PyAny>>),
                )
            }
            #[cfg(not(Py_LIMITED_API))]
            Self::PyTime => OutputType::new(
                quote!(::pyo3::Bound<'py, ::pyo3::types::PyTime>),
                quote!(&::pyo3::Bound<'py, ::pyo3::types::PyTime>),
            ),
            #[cfg(not(Py_LIMITED_API))]
            Self::PyTzInfo => OutputType::new(
                quote!(::pyo3::Bound<'py, ::pyo3::types::PyTzInfo>),
                quote!(&::pyo3::Bound<'py, ::pyo3::types::PyTzInfo>),
            ),

            // Python-specific types
            Self::PyCapsule => OutputType::new(
                quote!(::pyo3::Bound<'py, ::pyo3::types::PyCapsule>),
                quote!(&::pyo3::Bound<'py, ::pyo3::types::PyCapsule>),
            ),
            Self::PyCFunction => OutputType::new(
                quote!(::pyo3::Bound<'py, ::pyo3::types::PyCFunction>),
                quote!(&::pyo3::Bound<'py, ::pyo3::types::PyCFunction>),
            ),
            #[cfg(not(Py_LIMITED_API))]
            Self::PyCode => OutputType::new(
                quote!(::pyo3::Bound<'py, ::pyo3::types::PyCode>),
                quote!(&::pyo3::Bound<'py, ::pyo3::types::PyCode>),
            ),
            Self::PyEllipsis => {
                // TODO: Determine if PyEllipsis is even possible
                OutputType::new(
                    quote!(::pyo3::Bound<'py, ::pyo3::types::PyAny>),
                    quote!(impl ::pyo3::IntoPy<::pyo3::Py<::pyo3::types::PyAny>>),
                )
            }
            #[cfg(all(not(Py_LIMITED_API), not(PyPy)))]
            Self::PyFrame => OutputType::new(
                quote!(::pyo3::Bound<'py, ::pyo3::types::PyFrame>),
                quote!(&::pyo3::Bound<'py, ::pyo3::types::PyFrame>),
            ),
            #[cfg(all(not(Py_LIMITED_API), not(PyPy)))]
            Self::PyFunction { .. } => OutputType::new(
                quote!(::pyo3::Bound<'py, ::pyo3::types::PyFunction>),
                quote!(&::pyo3::Bound<'py, ::pyo3::types::PyFunction>),
            ),
            #[cfg(not(all(not(Py_LIMITED_API), not(PyPy))))]
            Self::PyFunction { .. } => OutputType::new(
                quote!(::pyo3::Bound<'py, ::pyo3::types::PyAny>),
                quote!(impl ::pyo3::IntoPy<::pyo3::Py<::pyo3::types::PyAny>>),
            ),
            Self::PyModule => OutputType::new(
                quote!(::pyo3::Bound<'py, ::pyo3::types::PyModule>),
                quote!(&::pyo3::Bound<'py, ::pyo3::types::PyModule>),
            ),
            #[cfg(not(PyPy))]
            Self::PySuper => OutputType::new(
                quote!(::pyo3::Bound<'py, ::pyo3::types::PySuper>),
                quote!(&::pyo3::Bound<'py, ::pyo3::types::PySuper>),
            ),
            Self::PyTraceback => OutputType::new(
                quote!(::pyo3::Bound<'py, ::pyo3::types::PyTraceback>),
                quote!(&::pyo3::Bound<'py, ::pyo3::types::PyTraceback>),
            ),
            Self::PyType => OutputType::new(
                quote!(::pyo3::Bound<'py, ::pyo3::types::PyType>),
                quote!(&::pyo3::Bound<'py, ::pyo3::types::PyType>),
            ),
        }
    }

    fn map_type(self, local_types: &HashMap<Path, Path>) -> OutputType {
        // Get the inner name of the type
        let Self::Other(type_name) = self else {
            unreachable!()
        };

        // Try to map the external types
        if let Some(external_type) = Self::try_map_external_type(&type_name) {
            return external_type;
        }

        // Try to map the local types
        let type_name_without_delimiters =
            type_name.split_once('[').map(|s| s.0).unwrap_or(&type_name);
        if let Some(relative_path) = local_types.get(&Path::from_py(type_name_without_delimiters)) {
            let relative_path: syn::Path = relative_path.try_into().unwrap();
            return OutputType::new(
                quote!(::pyo3::Bound<'py, #relative_path>),
                quote!(&::pyo3::Bound<'py, #relative_path>),
            );
        }

        // Unhandled types
        OutputType::new(
            quote!(::pyo3::Bound<'py, ::pyo3::types::PyAny>),
            quote!(impl ::pyo3::IntoPy<::pyo3::Py<::pyo3::types::PyAny>>),
        )
    }

    fn try_map_external_type(type_name: &str) -> Option<OutputType> {
        // TODO: Handle types from other packages with Rust bindings here
        match type_name {
            #[cfg(feature = "numpy")]
            numpy_ndarray
                if numpy_ndarray
                    .split_once('[')
                    .map(|s| s.0)
                    .unwrap_or(numpy_ndarray)
                    .split('.')
                    .last()
                    .unwrap_or(numpy_ndarray)
                    .to_lowercase()
                    == "ndarray" =>
            {
                Some(OutputType::new(
                    quote!(
                        ::pyo3::Bound<
                            'py,
                            ::numpy::PyArray<::pyo3::Py<::pyo3::types::PyAny>, ::numpy::IxDyn>,
                        >
                    ),
                    quote!(
                        &::pyo3::Bound<
                            'py,
                            ::numpy::PyArray<::pyo3::Py<::pyo3::types::PyAny>, ::numpy::IxDyn>,
                        >
                    ),
                ))
            }
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
struct OutputType {
    owned: Rc<proc_macro2::TokenStream>,
    borrowed: Rc<proc_macro2::TokenStream>,
}

impl OutputType {
    fn new(owned: proc_macro2::TokenStream, borrowed: proc_macro2::TokenStream) -> Self {
        Self {
            owned: Rc::new(owned),
            borrowed: Rc::new(borrowed),
        }
    }

    fn new_identical(output_type: proc_macro2::TokenStream) -> Self {
        let output_type = Rc::new(output_type);
        Self {
            owned: output_type.clone(),
            borrowed: output_type,
        }
    }
}
