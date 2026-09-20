use super::Type;
use crate::planning::{LocalTypes, Path};
use itertools::Itertools;
use quote::quote;
use std::rc::Rc;

impl Type {
    pub(crate) fn into_rs_owned(self, local_types: &LocalTypes) -> proc_macro2::TokenStream {
        let owned = self.into_rs(local_types).owned;
        Rc::into_inner(owned).unwrap_or_else(|| unreachable!())
    }

    pub(crate) fn into_rs_borrowed(self, local_types: &LocalTypes) -> proc_macro2::TokenStream {
        let borrowed = self.into_rs(local_types).borrowed;
        Rc::into_inner(borrowed).unwrap_or_else(|| unreachable!())
    }

    /// Emit any per-parameter preconversion needed before the value is forwarded to
    /// Python.
    ///
    /// `is_var_keyword` marks the synthetic `**kwargs` parameter, whose downstream call
    /// path (`Some(&kwargs)`) requires a materialized `Bound<PyDict>` rather than an
    /// `Option`. It carries the same `Optional<PyDict>` type as a genuine
    /// `Optional[dict]` parameter but must NOT receive the (#14) `None`-preserving
    /// rewrite, which would leave it an `Option` and break the call.
    pub(crate) fn preprocess_borrowed(
        &self,
        ident: &syn::Ident,
        local_types: &LocalTypes,
        is_var_keyword: bool,
    ) -> proc_macro2::TokenStream {
        match self {
            Self::PyDict {
                key_type,
                value_type,
            } if Self::dict_requires_py_dict(key_type, value_type, local_types) => {
                quote! {
                    let #ident = ::pyo3::types::IntoPyDict::into_py_dict(#ident, py)?;
                }
            }
            // Pass-through types already reach the call sink as a raw `impl IntoPyObject`,
            // which converts them exactly once. A preparatory `into_bound_py_any` here would
            // only turn `Bound<PyAny>` into itself -- a no-op at runtime but dead `let`
            // bindings and error branches the downstream compiler must still parse. Emit
            // nothing and let the single conversion happen at the sink.
            Self::PyAny
            | Self::Unknown
            | Self::Union(..)
            | Self::PyNone
            | Self::PyDelta
            | Self::PyEllipsis => proc_macro2::TokenStream::new(),
            #[cfg(not(all(not(Py_LIMITED_API), not(PyPy))))]
            Self::PyFunction { .. } => proc_macro2::TokenStream::new(),
            Self::Other(type_name)
                if Self::try_map_external_type(type_name).is_none()
                    && !Self::local_type_path(type_name)
                        .is_some_and(|path| local_types.contains_key(&path)) =>
            {
                proc_macro2::TokenStream::new()
            }
            Self::Optional(inner_type) => match inner_type.as_ref() {
                Self::PyDict {
                    key_type,
                    value_type,
                } if Self::dict_requires_py_dict(key_type, value_type, local_types) => {
                    if is_var_keyword {
                        // The synthetic `**kwargs` param is forwarded as `Some(&kwargs)`,
                        // which needs a materialized dict; a missing value becomes an
                        // empty `{}` (a call with no keyword arguments).
                        quote! {
                            let #ident = if let Some(#ident) = #ident {
                                ::pyo3::types::IntoPyDict::into_py_dict(#ident, py)?
                            } else {
                                ::pyo3::types::PyDict::new(py)
                            };
                        }
                    } else {
                        // (#14) A caller-supplied `None` must stay `None`, not be
                        // materialized into an empty `{}` (which changes call semantics
                        // for any callee that distinguishes "omitted" from "empty").
                        // Convert only the `Some` case and keep the `Option` wrapper;
                        // `Option<T: IntoPyObject>` forwards `None` to Python `None`.
                        quote! {
                            let #ident = #ident
                                .map(|#ident| ::pyo3::types::IntoPyDict::into_py_dict(#ident, py))
                                .transpose()?;
                        }
                    }
                }
                _ => proc_macro2::TokenStream::new(),
            },
            _ => proc_macro2::TokenStream::new(),
        }
    }

    fn dict_requires_py_dict(key_type: &Self, value_type: &Self, local_types: &LocalTypes) -> bool {
        !key_type.is_hashable_key(local_types)
            || value_type.clone().into_rs(local_types).contains_py_any
    }

    fn into_rs(self, local_types: &LocalTypes) -> OutputType {
        match self {
            Self::PyAny | Self::Unknown => OutputType::new_dynamic(
                quote!(::pyo3::Bound<'py, ::pyo3::types::PyAny>),
                quote!(impl ::pyo3::IntoPyObject<'py>),
            ),
            Self::Other(..) => self.map_type(local_types),

            // Primitives
            Self::PyBool => OutputType::new_identical(quote!(bool)),
            Self::PyBytes => OutputType::new(quote!(Vec<u8>), quote!(&[u8])),
            // `&[u8]: IntoPyObject` always builds an immutable `PyBytes`, so lowering
            // `bytearray` to the same `Vec<u8>`/`&[u8]` shape as `bytes` would silently
            // forward a Python `bytearray` parameter as `bytes` instead of a mutable
            // `bytearray`. Fall back to the native `Bound<'py, PyByteArray>` handle
            // (same pattern as `PySlice` below) so it round-trips as a genuine mutable
            // bytearray in both directions.
            Self::PyByteArray => OutputType::new(
                quote!(::pyo3::Bound<'py, ::pyo3::types::PyByteArray>),
                quote!(&::pyo3::Bound<'py, ::pyo3::types::PyByteArray>),
            ),
            Self::PyFloat => OutputType::new_identical(quote!(f64)),
            Self::PyInt => OutputType::new_identical(quote!(i64)),
            Self::PyString => OutputType::new(quote!(::std::string::String), quote!(&str)),

            // Enums
            Self::Optional(inner_type) => {
                let inner_type = inner_type.into_rs(local_types);
                let contains_py_any = inner_type.contains_py_any;
                let inner_type = inner_type.owned;
                OutputType::new_identical_with_dynamic(
                    quote!(::std::option::Option<#inner_type>),
                    contains_py_any,
                )
            }
            Self::Union(_inner_types) => {
                // TODO: Support Rust enums where possible | alternatively, overload functions for each variant
                OutputType::new_dynamic(
                    quote!(::pyo3::Bound<'py, ::pyo3::types::PyAny>),
                    quote!(impl ::pyo3::IntoPyObject<'py>),
                )
            }
            Self::PyNone => {
                // TODO: Determine if PyNone is even possible
                OutputType::new_dynamic(
                    quote!(::pyo3::Bound<'py, ::pyo3::types::PyAny>),
                    quote!(impl ::pyo3::IntoPyObject<'py>),
                )
            }

            // Collections
            // CR5: a collection-protocol container lowers to the generic
            // `PyCollection<T>` wrapper emitted once on the external-IR path. The
            // element type resolves through `local_types` exactly like any other class
            // reference. The wrapper itself is referenced via a module-relative path
            // carried separately from Python-name resolution in `LocalTypes` (see
            // `Module::generate`); if that support path is absent, fall back safely to
            // a dynamic handle.
            Self::Collection(inner_type) => {
                // The generic `PyCollection<T>` wrapper exists only when the external-IR
                // path emits it (its module-relative path is carried separately in
                // `local_types`). When that path is absent — the live path never
                // emits the wrapper, and never produces `Type::Collection` either — fall
                // back to the untyped handle instead of referencing an undefined type.
                let wrapper: Option<syn::Path> = local_types
                    .collection_wrapper()
                    .map(|local_type| local_type.rust_path().clone());
                match wrapper {
                    Some(wrapper) => {
                        let inner_type = inner_type.into_rs(local_types);
                        let contains_py_any = inner_type.contains_py_any;
                        let inner_type = inner_type.owned;
                        OutputType::new_with_dynamic(
                            quote!(#wrapper<#inner_type>),
                            quote!(&#wrapper<#inner_type>),
                            contains_py_any,
                        )
                    }
                    None => OutputType::new_dynamic(
                        quote!(::pyo3::Bound<'py, ::pyo3::types::PyAny>),
                        quote!(impl ::pyo3::IntoPyObject<'py>),
                    ),
                }
            }
            // CR7: lower a fixed-shape numeric array to a Rust fixed-size array.
            // Row-major nesting: the first dimension is the outermost, so
            // `dims == [M, N]` yields `[[T; N]; M]`. The element type maps exactly
            // like a scalar of the same Python type.
            Self::FixedArray(elem, dims) => {
                let elem_ty = elem.into_rs(local_types);
                let contains_py_any = elem_ty.contains_py_any;
                let elem_ty = elem_ty.owned;
                let mut owned = quote!(#elem_ty);
                for &dim in dims.iter().rev() {
                    let len = proc_macro2::Literal::usize_unsuffixed(dim);
                    owned = quote!([#owned; #len]);
                }
                OutputType::new_with_dynamic(quote!(#owned), quote!(&#owned), contains_py_any)
            }
            Self::PyDict {
                key_type,
                value_type,
            } => {
                let value_type = value_type.into_rs(local_types);
                if key_type.is_hashable_key(local_types) && !value_type.contains_py_any {
                    let key_type = key_type.into_rs(local_types).owned;
                    let value_type = value_type.owned;
                    OutputType::new(
                        quote!(::std::collections::HashMap<#key_type, #value_type>),
                        quote!(&::std::collections::HashMap<#key_type, #value_type>),
                    )
                } else {
                    OutputType::new(
                        quote!(::pyo3::Bound<'py, ::pyo3::types::PyDict>),
                        quote!(impl ::pyo3::types::IntoPyDict<'py>),
                    )
                }
            }
            // `&HashSet<T>: IntoPyObject` always builds a *mutable* `PySet` via
            // `PySet::new`, so lowering a hashable-element `frozenset[T]` to `HashSet<T>`
            // would silently forward it as a mutable `set`, breaking hashing/isinstance
            // on the Python side. Always fall back to the native
            // `Bound<'py, PyFrozenSet>` handle, regardless of whether the element type
            // is Rust-hashable, so it round-trips as a genuine frozenset in both
            // directions.
            Self::PyFrozenSet(_inner_type) => OutputType::new(
                quote!(::pyo3::Bound<'py, ::pyo3::types::PyFrozenSet>),
                quote!(&::pyo3::Bound<'py, ::pyo3::types::PyFrozenSet>),
            ),
            Self::PyList(inner_type) => {
                let inner_type = inner_type.into_rs(local_types);
                let contains_py_any = inner_type.contains_py_any;
                let inner_type = inner_type.owned;
                OutputType::new_with_dynamic(
                    quote!(Vec<#inner_type>),
                    quote!(&[#inner_type]),
                    contains_py_any,
                )
            }
            Self::PySet(inner_type) => {
                // Only lower to a typed `HashSet<T>` when the element is known to be
                // Rust-hashable: a primitive, or a generated mirror `enum` (which derives
                // `Eq`/`Hash`). A generated *class* newtype is not hashable (it derives
                // only `Debug`), so a `set[SomeClass]` must fall back to the untyped
                // `Bound<PySet>` rather than a `HashSet<SomeClass>` that fails to compile
                // (`SomeClass: Eq + Hash` unsatisfied). The enum case is resolved via
                // `LocalType::is_enum` (the resolver now carries type kinds), so
                // `set[SomeEnum]` lowers to `HashSet<Enum>`.
                if inner_type.is_hashable_key(local_types) {
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
                if inner_types.len() < 2
                    || (inner_types.len() == 2
                        && *inner_types.last().unwrap_or_else(|| unreachable!())
                            == Self::PyEllipsis)
                {
                    OutputType::new(
                        quote!(::pyo3::Bound<'py, ::pyo3::types::PyTuple>),
                        quote!(&::pyo3::Bound<'py, ::pyo3::types::PyTuple>),
                    )
                } else {
                    let inner_types = inner_types
                        .into_iter()
                        .map(|inner_type| inner_type.into_rs(local_types))
                        .collect_vec();
                    let contains_py_any = inner_types
                        .iter()
                        .any(|inner_type| inner_type.contains_py_any);
                    let inner_types = inner_types
                        .into_iter()
                        .map(|inner_type| inner_type.owned)
                        .collect_vec();
                    OutputType::new_identical_with_dynamic(
                        quote!((#(#inner_types),*)),
                        contains_py_any,
                    )
                }
            }

            // Additional types - std
            // pyo3 0.29 only implements `FromPyObject` for the combined
            // `::std::net::IpAddr` enum (see `pyo3::conversions::std::ipaddr`), not for
            // `Ipv4Addr`/`Ipv6Addr` individually, so an owned/return-position type of a
            // bare `Ipv4Addr`/`Ipv6Addr` would fail to compile wherever it is extracted
            // (`PyAnyMethods::extract`). Fall back to the shared `IpAddr` for the owned
            // side. `IntoPyObject` IS implemented for `Ipv4Addr`/`Ipv6Addr` directly, so
            // the parameter/into-python (borrowed) side keeps the precise type.
            Self::IpV4Addr => {
                OutputType::new(quote!(::std::net::IpAddr), quote!(::std::net::Ipv4Addr))
            }
            Self::IpV6Addr => {
                OutputType::new(quote!(::std::net::IpAddr), quote!(::std::net::Ipv6Addr))
            }
            Self::Path => OutputType::new(quote!(::std::path::PathBuf), quote!(&::std::path::Path)),
            // TODO: Map `PySlice` to `std::ops::Range` if possible
            Self::PySlice => OutputType::new(
                quote!(::pyo3::Bound<'py, ::pyo3::types::PySlice>),
                quote!(&::pyo3::Bound<'py, ::pyo3::types::PySlice>),
            ),

            // Additional types - num-complex
            #[cfg(feature = "num-complex")]
            Self::PyComplex => OutputType::new_identical(quote!(::num_complex::Complex64)),
            #[cfg(not(feature = "num-complex"))]
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
                // `Duration` does not round-trip cleanly through PyO3 conversion traits yet.
                // OutputType::new_identical(quote!(::std::time::Duration))
                OutputType::new_dynamic(
                    quote!(::pyo3::Bound<'py, ::pyo3::types::PyAny>),
                    quote!(impl ::pyo3::IntoPyObject<'py>),
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
                OutputType::new_dynamic(
                    quote!(::pyo3::Bound<'py, ::pyo3::types::PyAny>),
                    quote!(impl ::pyo3::IntoPyObject<'py>),
                )
            }
            #[cfg(all(not(Py_LIMITED_API), not(any(PyPy, GraalPy))))]
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
            Self::PyFunction { .. } => OutputType::new_dynamic(
                quote!(::pyo3::Bound<'py, ::pyo3::types::PyAny>),
                quote!(impl ::pyo3::IntoPyObject<'py>),
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

    /// Resolve a Python type name to its lookup [`Path`], unless it cannot name a
    /// local type at all.
    ///
    /// A Python type name never legitimately contains `::`, the Rust path separator.
    /// But a string annotation on the live path is captured verbatim and is never
    /// validated as an expression (no `eval_str`/`get_type_hints`), so a member such
    /// as `x: "Foo::Bar"` reaches type lowering unchanged. Feeding it to
    /// [`Path::from_py`] -- which asserts the absence of `::` in debug builds, the
    /// profile Cargo uses to compile build scripts -- would abort generation. Such a
    /// name can never match a generated local type, so treat it as absent and let the
    /// caller fall back to a dynamic `PyAny`.
    fn local_type_path(type_name: &str) -> Option<Path> {
        // Ignore any subscript (`Foo[int]` -> `Foo`) so a parameterized reference to a
        // generated local type resolves to that type -- matching `map_type` -- instead
        // of being treated as non-local and redundantly re-preconverted in
        // `preprocess_borrowed`. A path bearing `::` is never a single-segment local type.
        let base = type_name
            .split_once('[')
            .map_or(type_name, |(head, _)| head);
        (!base.contains("::")).then(|| Path::from_py(base))
    }

    /// Whether this type references a locally-generated mirror `enum`. A mirror enum
    /// derives `Eq`/`Hash` (unlike a class newtype, which derives only `Debug`), so a
    /// `set[Enum]` element can lower to a typed `HashSet<Enum>` instead of `Bound<PySet>`.
    /// Resolution mirrors `map_type`: a subscript is ignored and the base must be a
    /// single-segment local type registered as an enum.
    fn is_local_enum(&self, local_types: &LocalTypes) -> bool {
        let Self::Other(type_name) = self else {
            return false;
        };
        Self::local_type_path(type_name)
            .and_then(|path| local_types.get(&path))
            .is_some_and(|local_type| local_type.is_enum)
    }

    /// Whether this type lowers to a Rust type usable as a `HashMap`/`HashSet` key:
    /// a primitive with `Eq + Hash`, or a locally-generated mirror `enum` (which
    /// derives `Eq + Hash`; see `syntax::enumeration`). A generated *class* newtype
    /// derives only `Debug` and is deliberately excluded, so a `dict`/`set` keyed on a
    /// class degrades to the untyped `Bound<PyDict>`/`Bound<PySet>`. Shared by the dict
    /// and set gates so the two can never drift.
    fn is_hashable_key(&self, local_types: &LocalTypes) -> bool {
        self.is_hashable() || self.is_local_enum(local_types)
    }

    fn map_type(self, local_types: &LocalTypes) -> OutputType {
        // Get the inner name of the type
        let Self::Other(type_name) = self else {
            unreachable!()
        };

        // Try to map the local types
        let type_name_without_delimiters = type_name
            .split_once('[')
            .map_or(type_name.as_ref(), |s| s.0);
        if let Some(relative_path) = Self::local_type_path(type_name_without_delimiters)
            .and_then(|path| local_types.get(&path))
        {
            let relative_path = relative_path.rust_path();
            return OutputType::new(quote!(#relative_path), quote!(&#relative_path));
        }

        // External adapters are fallback mappings. An explicitly generated local type
        // with the same spelling remains the source of truth.
        if let Some(external_type) = Self::try_map_external_type(&type_name) {
            return external_type;
        }

        // Unhandled types
        OutputType::new_dynamic(
            quote!(::pyo3::Bound<'py, ::pyo3::types::PyAny>),
            quote!(impl ::pyo3::IntoPyObject<'py>),
        )
    }

    fn try_map_external_type(type_name: &str) -> Option<OutputType> {
        // TODO: Handle types from other packages with Rust bindings here
        let base_type = type_name
            .split_once('[')
            .map_or(type_name, |(base_type, _)| base_type);
        match base_type {
            #[cfg(feature = "numpy")]
            "numpy.ndarray" => Some(OutputType::new(
                quote!(::pyo3::Bound<'py, ::numpy::PyUntypedArray>),
                quote!(&::pyo3::Bound<'py, ::numpy::PyUntypedArray>),
            )),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
struct OutputType {
    owned: Rc<proc_macro2::TokenStream>,
    borrowed: Rc<proc_macro2::TokenStream>,
    contains_py_any: bool,
}

impl OutputType {
    fn new(owned: proc_macro2::TokenStream, borrowed: proc_macro2::TokenStream) -> Self {
        Self {
            owned: Rc::new(owned),
            borrowed: Rc::new(borrowed),
            contains_py_any: false,
        }
    }

    fn new_dynamic(owned: proc_macro2::TokenStream, borrowed: proc_macro2::TokenStream) -> Self {
        Self {
            owned: Rc::new(owned),
            borrowed: Rc::new(borrowed),
            contains_py_any: true,
        }
    }

    fn new_with_dynamic(
        owned: proc_macro2::TokenStream,
        borrowed: proc_macro2::TokenStream,
        contains_py_any: bool,
    ) -> Self {
        Self {
            owned: Rc::new(owned),
            borrowed: Rc::new(borrowed),
            contains_py_any,
        }
    }

    fn new_identical(output_type: proc_macro2::TokenStream) -> Self {
        let output_type = Rc::new(output_type);
        Self {
            owned: output_type.clone(),
            borrowed: output_type,
            contains_py_any: false,
        }
    }

    fn new_identical_with_dynamic(
        output_type: proc_macro2::TokenStream,
        contains_py_any: bool,
    ) -> Self {
        let output_type = Rc::new(output_type);
        Self {
            owned: output_type.clone(),
            borrowed: output_type,
            contains_py_any,
        }
    }
}

#[cfg(all(test, any(feature = "num-complex", feature = "numpy")))]
mod tests {
    use super::*;

    #[cfg(feature = "num-complex")]
    #[test]
    fn num_complex_feature_maps_python_complex_to_complex64() {
        let output = Type::PyComplex.into_rs(&LocalTypes::default());
        assert!(
            output
                .owned
                .to_string()
                .contains("num_complex :: Complex64"),
            "unexpected owned type: {}",
            output.owned,
        );
        assert_eq!(output.owned.to_string(), output.borrowed.to_string());
    }

    #[cfg(feature = "numpy")]
    #[test]
    fn numpy_feature_maps_ndarray_to_untyped_array() {
        let output = Type::Other("numpy.ndarray".to_owned()).into_rs(&LocalTypes::default());
        assert!(
            output.owned.to_string().contains("numpy :: PyUntypedArray"),
            "unexpected owned type: {}",
            output.owned,
        );
    }
}

#[cfg(test)]
mod colon_bearing_annotation_tests {
    use super::*;

    /// A live-path string annotation is captured verbatim and never validated as an
    /// expression, so a member such as `x: "Foo::Bar"` reaches type lowering
    /// unchanged. It must degrade to a dynamic `PyAny` rather than abort the
    /// (debug-profile) build through `Path::from_py`'s `::` assertion.
    #[test]
    fn colon_bearing_type_name_falls_back_to_pyany() {
        let output = Type::Other("Foo::Bar".to_owned()).into_rs(&LocalTypes::default());
        assert!(
            output.owned.to_string().contains("PyAny"),
            "a non-Python type name must degrade to PyAny: {}",
            output.owned,
        );
    }

    /// The same name flowing through the preconversion path must not panic either. An
    /// unmapped `Other` type is a pass-through, so its single `IntoPyObject` conversion now
    /// happens at the call sink and `preprocess_borrowed` emits nothing (rather than a
    /// redundant `into_bound_py_any` binding); the property under test is that a
    /// colon-bearing name is handled without panicking.
    #[test]
    fn colon_bearing_type_name_preprocess_does_not_panic() {
        let ident = quote::format_ident!("value");
        let tokens = Type::Other("Foo::Bar".to_owned()).preprocess_borrowed(
            &ident,
            &LocalTypes::default(),
            false,
        );
        assert!(
            tokens.is_empty(),
            "an unmapped Other type is a pass-through with no preprocessing: {tokens}",
        );
    }
}
