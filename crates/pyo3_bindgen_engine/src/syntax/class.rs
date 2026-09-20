use super::{
    AttributeVariant, Function, FunctionImplementation, FunctionType, Ident, ImportMode,
    MethodType, Path, Property, PropertyOwner,
};
use crate::{planning::Type, report::DomainDiagnostic, Config, Result};
use itertools::Itertools;
use pyo3::prelude::*;

/// Name of the synthetic, language-neutral universal base trait emitted once per
/// generation on the external-IR path (see [`generate_root_trait`]).
///
/// It is the common base of every generated Python-object newtype (modelling the
/// language/runtime universal object base). The name is deliberately **generic**:
/// no domain/framework concept appears in it.
pub(crate) const ROOT_TRAIT_NAME: &str = "PyObjectBase";

/// Name of the generic type-marker trait emitted once per generation on the
/// external-IR path (see [`generate_root_trait`]).
///
/// Each newtype implements it to expose its registered Python class name plus the
/// unchecked wrap/unwrap of the underlying handle, powering the generic
/// upcast/downcast machinery (CR3/CR4). The name is deliberately **generic**.
pub(crate) const PY_TYPE_MARKER_NAME: &str = "PyTypeMarker";

/// Name of the generic free helper that resolves a dotted Python class path
/// (e.g. `"pkg.sub.Class"`) to its runtime type object. Emitted once per
/// generation on the external-IR path, alongside the marker/root traits.
pub(crate) const RESOLVE_PY_TYPE_FN_NAME: &str = "__pyo3_bindgen_resolve_py_type";

/// Name of the generic, homogeneous collection-protocol wrapper struct emitted by
/// [`generate_collection_wrapper`] on the external-IR path -- but only when a
/// collection type is actually used (see `Codegen::has_any_collection`), so a
/// class-only crate never carries it.
///
/// A collection property (`Type::Collection`, lowered in `typing::into_rs`) binds
/// to `PyCollection<Element>`. The name is deliberately **generic**: it models any
/// Python `__len__`/`__iter__`/`__getitem__` container, with no domain concept.
/// `typing::into_rs` references it via a dedicated module-relative support path
/// carried by `LocalTypes`, separate from user-defined Python type names.
pub(crate) const COLLECTION_WRAPPER_NAME: &str = "PyCollection";

/// Emit the shared, language-neutral infrastructure declared once per generation on
/// the external-IR path: the synthetic universal base trait ([`ROOT_TRAIT_NAME`]),
/// the generic type-marker trait ([`PY_TYPE_MARKER_NAME`]), and the dotted-path →
/// runtime-type-object helper ([`RESOLVE_PY_TYPE_FN_NAME`]).
///
/// * The root trait exposes the wrapped Python handle via `__pyo3_obj()` so that the
///   per-class `*Methods` traits can carry their members as **default-bodied**
///   methods reaching the handle generically (`self.__pyo3_obj().bind(py)`); it also
///   carries the generic checked-downcast default methods (CR4) and (D3) the shared
///   handle accessors `bind`/`as_any`.
/// * The marker trait records each newtype's registered Python class name and the
///   unchecked wrap/unwrap used by the zero-cost upcast impls (CR3) and the
///   downcast rewrapping (CR4), plus (D3) the shared `clone_ref` wrapper
///   boilerplate, whose default body needs both `Self: Sized` and the handle
///   accessor -- hence the [`ROOT_TRAIT_NAME`] supertrait bound.
///
/// D3: `bind`, `as_any` and `clone_ref` are emitted ONCE here as `#[inline]` default
/// methods instead of once per generated newtype's inherent `impl` block. Consumers
/// must therefore have the root/marker trait in scope to call them; both are `pub`
/// at the generated root.
///
/// `into_py_any` is deliberately NOT hoisted (fix round 1): pyo3's
/// `IntoPyObjectExt` has a blanket `impl<T: IntoPyObject> IntoPyObjectExt for T`
/// that supplies a same-named method, so a trait-level `into_py_any` would make
/// `wrapper.into_py_any()` ambiguous (E0034) for any consumer that imports
/// `IntoPyObjectExt`. Per this crate's doctrine (see the E0034 note in
/// `Class::generate`), name ambiguity is resolved by INHERENT methods, never at the
/// trait layer -- an inherent method takes resolution precedence over every
/// same-named trait method -- so `into_py_any` stays a per-class inherent method
/// under both models. Generic code over the marker trait uses `__into_obj`, which
/// is exactly what the old default body forwarded to. `from_py_any` likewise stays
/// per-class: it names the concrete class and its runtime import.
///
/// All three live at the generated root, so the default-method bodies reference the
/// marker trait and the helper by their (root-scoped) bare names.
pub(crate) fn generate_root_trait() -> proc_macro2::TokenStream {
    let trait_ident = quote::format_ident!("{}", ROOT_TRAIT_NAME);
    let marker_ident = quote::format_ident!("{}", PY_TYPE_MARKER_NAME);
    let resolve_fn = quote::format_ident!("{}", RESOLVE_PY_TYPE_FN_NAME);
    quote::quote! {
        #[doc = "Synthetic universal base trait of every generated Python-object newtype."]
        #[doc = ""]
        #[doc = "Exposes the wrapped Python handle so default-bodied accessor/method"]
        #[doc = "traits can reach it generically. Also provides generic checked"]
        #[doc = "downcasts to any concrete newtype. Emitted once per generation."]
        pub trait #trait_ident {
            #[doc = "Borrow the wrapped Python object handle."]
            fn __pyo3_obj(&self) -> &::pyo3::Py<::pyo3::types::PyAny>;

            #[doc = "Borrow the wrapped Python object, bound to `py`."]
            #[inline]
            fn bind<'py>(
                &self,
                py: ::pyo3::marker::Python<'py>,
            ) -> &::pyo3::Bound<'py, ::pyo3::types::PyAny> {
                self.__pyo3_obj().bind(py)
            }

            #[doc = "Borrow the wrapped Python object as an untyped `PyAny`."]
            #[inline]
            fn as_any<'py>(
                &self,
                py: ::pyo3::marker::Python<'py>,
            ) -> &::pyo3::Bound<'py, ::pyo3::types::PyAny> {
                self.bind(py)
            }

            #[doc = "Checked downcast to a concrete newtype `U` via `isinstance`."]
            #[doc = ""]
            #[doc = "Returns `Ok(Some(U))` when the wrapped object is an instance of"]
            #[doc = "`U`'s Python class, `Ok(None)` otherwise."]
            fn try_downcast<U: #marker_ident>(
                &self,
                py: ::pyo3::marker::Python<'_>,
            ) -> ::pyo3::PyResult<::std::option::Option<U>> {
                let expected_type = #resolve_fn(py, <U as #marker_ident>::PY_CLASS)?;
                let obj = self.__pyo3_obj().bind(py);
                if ::pyo3::types::PyAnyMethods::is_instance(obj, expected_type.as_any())? {
                    ::std::result::Result::Ok(::std::option::Option::Some(
                        <U as #marker_ident>::__from_obj(self.__pyo3_obj().clone_ref(py)),
                    ))
                } else {
                    ::std::result::Result::Ok(::std::option::Option::None)
                }
            }

            #[doc = "Checked downcast to a concrete newtype `U`, erroring on mismatch."]
            #[doc = ""]
            #[doc = "Like [`try_downcast`](Self::try_downcast) but maps a type mismatch"]
            #[doc = "to a Python `TypeError`."]
            fn downcast<U: #marker_ident>(
                &self,
                py: ::pyo3::marker::Python<'_>,
            ) -> ::pyo3::PyResult<U> {
                match self.try_downcast::<U>(py)? {
                    ::std::option::Option::Some(value) => ::std::result::Result::Ok(value),
                    ::std::option::Option::None => ::std::result::Result::Err(
                        ::pyo3::exceptions::PyTypeError::new_err(
                            "checked downcast failed: object is not an instance of the target Python class",
                        ),
                    ),
                }
            }
        }

        #[doc = "Generic type marker for every generated Python-object newtype."]
        #[doc = ""]
        #[doc = "Records the registered Python class name and the unchecked"]
        #[doc = "wrap/unwrap of the underlying handle, powering the zero-cost upcast"]
        #[doc = "impls and the checked downcasts. Emitted once per generation."]
        pub trait #marker_ident: ::core::marker::Sized + #trait_ident {
            #[doc = "Fully-qualified, dotted Python class name of this newtype."]
            const PY_CLASS: &'static str;
            #[doc = "Wrap a Python handle as this newtype without any type check."]
            fn __from_obj(obj: ::pyo3::Py<::pyo3::types::PyAny>) -> Self;
            #[doc = "Unwrap this newtype back into its underlying Python handle."]
            fn __into_obj(self) -> ::pyo3::Py<::pyo3::types::PyAny>;

            #[doc = "Clone this newtype, sharing the underlying Python object."]
            #[inline]
            fn clone_ref(&self, py: ::pyo3::marker::Python<'_>) -> Self {
                Self::__from_obj(self.__pyo3_obj().clone_ref(py))
            }
        }

        #[doc = "Resolve a dotted Python class path to its runtime type object."]
        #[doc = ""]
        #[doc = "Imports the module prefix (everything before the final `.`) and"]
        #[doc = "`getattr`s the final segment. Generic helper emitted once per"]
        #[doc = "generation; powers the checked downcasts on [`PyObjectBase`]."]
        pub fn #resolve_fn<'py>(
            py: ::pyo3::marker::Python<'py>,
            py_class: &str,
        ) -> ::pyo3::PyResult<::pyo3::Bound<'py, ::pyo3::types::PyAny>> {
            match py_class.rsplit_once('.') {
                ::std::option::Option::Some((module_path, attr)) => {
                    let module = py.import(module_path)?;
                    ::std::result::Result::Ok(
                        ::pyo3::types::PyAnyMethods::getattr(module.as_any(), attr)?,
                    )
                }
                ::std::option::Option::None => {
                    ::std::result::Result::Ok(py.import(py_class)?.into_any())
                }
            }
        }
    }
}

/// Generic `PyCollection<T>` wrapper, emitted once per generation (external-IR path)
/// only when a collection-protocol type is actually used -- a class-only IR would
/// otherwise carry it as dead code.
pub(crate) fn generate_collection_wrapper() -> proc_macro2::TokenStream {
    let collection_ident = quote::format_ident!("{}", COLLECTION_WRAPPER_NAME);
    quote::quote! {
        #[doc = "Lazy, typed view over an opaque Python object."]
        #[doc = ""]
        #[doc = "Extraction performs no protocol or element validation; each operation"]
        #[doc = "checks the corresponding Python behavior when called. Provides length,"]
        #[doc = "integer indexing, string-keyed lookup, and iteration. Emitted once per"]
        #[doc = "generation; a collection property binds to `PyCollection<Element>`."]
        #[derive(Debug)]
        #[repr(transparent)]
        pub struct #collection_ident<T> {
            obj: ::pyo3::Py<::pyo3::types::PyAny>,
            _marker: ::core::marker::PhantomData<T>,
        }

        // `PyCollection<T>` deliberately keeps its OWN inherent `bind`/`clone_ref`: it
        // implements neither the root nor the marker trait (it is generic over `T` and is
        // itself emitted once per generation), so the D3 hoist does not apply to it.
        impl<T> #collection_ident<T> {
            #[doc = "Wrap a Python object as this typed collection without validation."]
            pub fn __from_obj(obj: ::pyo3::Py<::pyo3::types::PyAny>) -> Self {
                Self { obj, _marker: ::core::marker::PhantomData }
            }

            #[doc = "Borrow the wrapped Python collection handle."]
            pub fn bind<'py>(
                &self,
                py: ::pyo3::marker::Python<'py>,
            ) -> &::pyo3::Bound<'py, ::pyo3::types::PyAny> {
                self.obj.bind(py)
            }

            #[doc = "Number of elements in the collection (`len(obj)`)."]
            pub fn len(&self, py: ::pyo3::marker::Python<'_>) -> ::pyo3::PyResult<usize> {
                ::pyo3::types::PyAnyMethods::len(self.obj.bind(py))
            }

            #[doc = "Whether the collection is empty."]
            pub fn is_empty(&self, py: ::pyo3::marker::Python<'_>) -> ::pyo3::PyResult<bool> {
                ::std::result::Result::Ok(self.len(py)? == 0)
            }

            #[doc = "Clone the typed collection, sharing the underlying handle."]
            pub fn clone_ref(&self, py: ::pyo3::marker::Python<'_>) -> Self {
                Self { obj: self.obj.clone_ref(py), _marker: ::core::marker::PhantomData }
            }
        }

        impl<T> #collection_ident<T> {
            #[doc = "Element at integer `index`, or `None` only for Python `IndexError`."]
            #[doc = ""]
            #[doc = "Python `IndexError` becomes `None`; `KeyError`, `TypeError`, and"]
            #[doc = "all other Python errors propagate."]
            pub fn get<'py>(
                &self,
                py: ::pyo3::marker::Python<'py>,
                index: usize,
            ) -> ::pyo3::PyResult<::std::option::Option<T>>
            where
                T: ::pyo3::prelude::FromPyObjectOwned<'py>,
            {
                match ::pyo3::types::PyAnyMethods::get_item(self.obj.bind(py), index) {
                    ::std::result::Result::Ok(item) => ::std::result::Result::Ok(
                        ::std::option::Option::Some(
                            ::pyo3::types::PyAnyMethods::extract::<T>(&item)
                                .map_err(::std::convert::Into::<::pyo3::PyErr>::into)?,
                        ),
                    ),
                    ::std::result::Result::Err(err)
                        if err.is_instance_of::<::pyo3::exceptions::PyIndexError>(py) =>
                    {
                        ::std::result::Result::Ok(::std::option::Option::None)
                    }
                    ::std::result::Result::Err(err) => ::std::result::Result::Err(err),
                }
            }

            #[doc = "Element keyed by `key` (`obj[key]`), or `None` only for Python `KeyError`."]
            #[doc = ""]
            #[doc = "Python `KeyError` becomes `None`; `IndexError`, `TypeError`, and all"]
            #[doc = "other Python errors propagate."]
            pub fn get_by_key<'py>(
                &self,
                py: ::pyo3::marker::Python<'py>,
                key: &str,
            ) -> ::pyo3::PyResult<::std::option::Option<T>>
            where
                T: ::pyo3::prelude::FromPyObjectOwned<'py>,
            {
                match ::pyo3::types::PyAnyMethods::get_item(self.obj.bind(py), key) {
                    ::std::result::Result::Ok(item) => ::std::result::Result::Ok(
                        ::std::option::Option::Some(
                            ::pyo3::types::PyAnyMethods::extract::<T>(&item)
                                .map_err(::std::convert::Into::<::pyo3::PyErr>::into)?,
                        ),
                    ),
                    ::std::result::Result::Err(err)
                        if err.is_instance_of::<::pyo3::exceptions::PyKeyError>(py) =>
                    {
                        ::std::result::Result::Ok(::std::option::Option::None)
                    }
                    ::std::result::Result::Err(err) => ::std::result::Result::Err(err),
                }
            }

            #[doc = "Collect every element into a `Vec`, each wrapped as `T`."]
            pub fn iter<'py>(
                &self,
                py: ::pyo3::marker::Python<'py>,
            ) -> ::pyo3::PyResult<::std::vec::Vec<T>>
            where
                T: ::pyo3::prelude::FromPyObjectOwned<'py>,
            {
                let bound = self.obj.bind(py);
                let mut items = ::std::vec::Vec::new();
                for item in ::pyo3::types::PyAnyMethods::try_iter(bound)? {
                    items.push(
                        ::pyo3::types::PyAnyMethods::extract::<T>(&item?)
                            .map_err(::std::convert::Into::<::pyo3::PyErr>::into)?,
                    );
                }
                ::std::result::Result::Ok(items)
            }
        }

        #[automatically_derived]
        impl<'py, T> ::pyo3::FromPyObject<'_, 'py> for #collection_ident<T> {
            type Error = ::pyo3::PyErr;

            fn extract(
                obj: ::pyo3::Borrowed<'_, 'py, ::pyo3::types::PyAny>,
            ) -> ::std::result::Result<Self, Self::Error> {
                ::std::result::Result::Ok(Self {
                    obj: obj.to_owned().unbind(),
                    _marker: ::core::marker::PhantomData,
                })
            }
        }

        #[automatically_derived]
        impl<'py, T> ::pyo3::IntoPyObject<'py> for #collection_ident<T> {
            type Target = ::pyo3::types::PyAny;
            type Output = ::pyo3::Bound<'py, Self::Target>;
            type Error = ::std::convert::Infallible;

            fn into_pyobject(
                self,
                py: ::pyo3::marker::Python<'py>,
            ) -> ::std::result::Result<Self::Output, Self::Error> {
                ::std::result::Result::Ok(self.obj.into_bound(py))
            }
        }

        #[automatically_derived]
        impl<'a, 'py, T> ::pyo3::IntoPyObject<'py> for &'a #collection_ident<T> {
            type Target = ::pyo3::types::PyAny;
            type Output = ::pyo3::Borrowed<'a, 'py, Self::Target>;
            type Error = ::std::convert::Infallible;

            fn into_pyobject(
                self,
                py: ::pyo3::marker::Python<'py>,
            ) -> ::std::result::Result<Self::Output, Self::Error> {
                ::std::result::Result::Ok(self.obj.bind_borrowed(py))
            }
        }
    }
}

/// Resolve a base-class [`Path`] to its `<Base>Methods` trait path, relative to the
/// module currently being generated.
///
/// The base type's relative path is looked up in `local_types` (the same map used
/// for all other cross-type resolution); the last segment is suffixed with
/// `Methods`. Returns [`None`] when the base is not in scope (its caller drops it
/// from the supertrait list / ancestor impls with a diagnostic).
fn methods_trait_path(
    base: &Path,
    local_types: &super::LocalTypes,
    enum_types: &rustc_hash::FxHashSet<Path>,
) -> Option<syn::Path> {
    // A base that resolves to an enum (or anything that is not a generated class) has
    // no `<Base>Methods` trait; skip it rather than emit a reference to a nonexistent
    // trait.
    if enum_types.contains(base) {
        return None;
    }
    let mut segments = local_types.get(base)?.path.clone();
    let n = segments.len();
    if n == 0 {
        return None;
    }
    // (#19) The trait name must be derived from the SANITIZED newtype identifier so it
    // matches the definition site (which emits `<sanitized>Methods`); using the raw
    // Python name here produced a dangling `<Base>Methods` reference for a class whose
    // name was itself sanitized (e.g. `Self` -> `s_Self` -> `s_SelfMethods`).
    let sanitized = super::naming::rust_ident(segments.name(), super::naming::SymbolKind::Class)?;
    segments[n - 1] = Ident::from_rs(&format!("{sanitized}Methods"));
    (&segments).try_into().ok()
}

/// Resolve a base-class [`Path`] to its in-scope **newtype** path, relative to the
/// module currently being generated.
///
/// Unlike [`methods_trait_path`], the last segment is left untouched: this yields the
/// ancestor wrapper struct itself (consumed by the transitive upcast impls, CR3).
/// Returns [`None`] when the base is not in scope.
fn newtype_path(
    base: &Path,
    local_types: &super::LocalTypes,
    enum_types: &rustc_hash::FxHashSet<Path>,
) -> Option<syn::Path> {
    // Only a generated class newtype is `#[repr(transparent)]` over `Py<PyAny>`, which
    // is what makes the upcast pointer-retag sound; never emit an unsafe cast to a
    // non-class base (e.g. an enum).
    if enum_types.contains(base) {
        return None;
    }
    // (#19) Resolve to the SANITIZED newtype identifier so the reference matches the
    // definition site for a class whose name required sanitization (`Self` -> `s_Self`).
    let mut segments = local_types.get(base)?.path.clone();
    let n = segments.len();
    if n == 0 {
        return None;
    }
    let sanitized = super::naming::rust_ident(segments.name(), super::naming::SymbolKind::Class)?;
    segments[n - 1] = Ident::from_rs(&sanitized.to_string());
    (&segments).try_into().ok()
}

/// The transitive live class hierarchy captured for the deduplicated path:
/// `(base_classes, member_owners)` -- see [`Class::capture_live_hierarchy`].
type CapturedHierarchy = (Vec<Path>, Vec<(String, Path)>);

/// The tokens produced by generating one class's own members.
struct GeneratedMembers {
    /// Inherent `impl` items for members that are not trait methods (e.g. static
    /// methods, constructors, module-owned helpers).
    struct_impl: proc_macro2::TokenStream,
    /// Trait method declarations (and default bodies on the external-IR path).
    method_defs: proc_macro2::TokenStream,
    /// Per-type trait method bodies (the live path; empty under external IR).
    method_impls: proc_macro2::TokenStream,
    /// `(python name, forwarders)` per own member, declaration order. Empty on the
    /// live path.
    forwarders: Vec<(String, Vec<super::Forwarder>)>,
    /// `(Python name, emitted Rust identifier)` for this class's own members, including
    /// inherent constructors/static/class methods whose Rust ident may differ from the
    /// Python name. The origin name is required to distinguish a real override from a
    /// canonically equivalent but distinct inherited member. Empty on the live path.
    resolved_members: Vec<(String, String)>,
}

/// Candidate-local, token-free selection facts for inherited callable names.
/// Name allocation and fallible signature lowering remain at their emission sites.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct InheritedMemberPlan {
    collisions: Vec<InheritedMemberCollision>,
    winners: rustc_hash::FxHashMap<Path, rustc_hash::FxHashSet<String>>,
}

#[derive(Debug, PartialEq, Eq)]
struct InheritedMemberCollision {
    name: String,
    winner: Path,
    declarer_count: usize,
}

impl InheritedMemberPlan {
    pub(crate) fn build(class: &Class, all_classes: &rustc_hash::FxHashMap<Path, &Class>) -> Self {
        let own_names: rustc_hash::FxHashSet<&str> = class
            .methods
            .iter()
            .map(|method| method.name.name().as_py())
            .chain(
                class
                    .properties
                    .iter()
                    .map(|property| property.name.name().as_py()),
            )
            .collect();
        let mut indices = rustc_hash::FxHashMap::default();
        let mut collisions: Vec<InheritedMemberCollision> = Vec::new();
        for base in &class.base_classes {
            let Some(ancestor) = all_classes.get(base) else {
                continue;
            };
            for name in ancestor
                .methods
                .iter()
                .map(|method| method.name.name().as_py())
                .chain(
                    ancestor
                        .properties
                        .iter()
                        .map(|property| property.name.name().as_py()),
                )
            {
                let index = *indices.entry(name).or_insert_with(|| {
                    collisions.push(InheritedMemberCollision {
                        name: name.to_owned(),
                        winner: base.clone(),
                        declarer_count: 0,
                    });
                    collisions.len() - 1
                });
                collisions[index].declarer_count += 1;
            }
        }
        collisions
            .retain(|entry| entry.declarer_count >= 2 && !own_names.contains(entry.name.as_str()));
        let mut winners: rustc_hash::FxHashMap<Path, rustc_hash::FxHashSet<String>> =
            rustc_hash::FxHashMap::default();
        for entry in &collisions {
            winners
                .entry(entry.winner.clone())
                .or_default()
                .insert(entry.name.clone());
        }
        Self {
            collisions,
            winners,
        }
    }
}

/// Forwarder signatures already lowered by ordered ancestor collision validation.
/// This immutable emission value is local to one descendant's namespace. It is
/// deliberately separate from the token-free normalized plan and drops unused bodies.
#[derive(Default)]
struct ValidatedAncestorForwarders {
    members: rustc_hash::FxHashMap<Path, Vec<(String, Vec<super::Forwarder>)>>,
}

#[cfg(test)]
thread_local! {
    static MEMBER_GENERATION_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

struct LiveClassAnnotation {
    name: Ident,
    annotation: Type,
    is_instance_field: bool,
    owner: Path,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InstanceAssignmentPermission {
    Allowed,
    Denied,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize)]
pub(crate) struct Class {
    pub(crate) name: Path,
    /// Author-supplied direct bases. Kept separate from the expanded ancestor
    /// closure so multiple direct interfaces remain visible on `*Methods`.
    ///
    /// Included in imported-module fingerprints: changing direct-base order can change
    /// Python's MRO and the emitted supertrait/upcast surface without changing anything
    /// returned by `dir()`. It is captured regardless of the deduplication toggle so the
    /// fingerprint remains a property of the Python input, not of generation config.
    /// External IR uses its source-schema hash instead.
    pub(crate) direct_base_classes: Vec<Path>,
    /// Live-dedup generation scratch: the minimal in-scope ancestor frontier used as
    /// supertraits after out-of-scope direct bases are removed. Kept separate from the
    /// captured direct bases so imported-module identity remains the inspected Python
    /// input rather than a generation optimization.
    #[serde(skip)]
    effective_direct_base_classes: Option<Vec<Path>>,
    /// Base-class paths, ordered nearest-first (empty for a root).
    ///
    /// Populated on the external-IR path (from the schema, expanded to the full
    /// transitive closure) and on the deduplicated live path (from Python's `__mro__`,
    /// filtered to in-scope class newtypes). Left empty on the non-deduplicated live
    /// path. Consumed by [`Self::generate`] to emit the supertrait bound, the empty
    /// ancestor impls, and the zero-cost upcast impls.
    ///
    /// Excluded from the serialized fingerprint because it is the transitive,
    /// generation-only expansion of the serialized direct-base input above.
    #[serde(skip)]
    pub(crate) base_classes: Vec<Path>,
    /// Deduplicated live path only: for each member gathered from `dir()` (by Python
    /// name), the dotted `module.qualname` path of the nearest MRO class that owns it in
    /// its `__dict__`. Consumed by [`Self::apply_inheritance_dedup`] to drop members that
    /// an in-scope ancestor already carries on its `*Methods` supertrait. Empty on the
    /// external-IR and non-deduplicated live paths.
    ///
    /// Transient generation scratch, not part of the class's identity: excluded from the
    /// serialized fingerprint.
    #[serde(skip)]
    member_owners: Vec<(String, Path)>,
    methods: Vec<Function>,
    properties: Vec<Property>,
    docstring: Option<String>,
    /// Deduplicated live path only: whether this class participates in in-scope
    /// inheritance (has an in-scope ancestor, or is the ancestor of an in-scope class)
    /// and therefore uses the deduplicated object model. An isolated class stays on the
    /// flat model so it never pays the model's per-class overhead (root/marker impls,
    /// forwarders, supertrait indirection) for a hierarchy it cannot exploit -- making
    /// deduplication a strict size win. Set by normalized live-inheritance planning;
    /// always `false` on the external-IR path (which uses the deduplicated model
    /// unconditionally) and the non-deduplicated live path.
    ///
    /// Transient generation scratch, not part of the class's identity: excluded from the
    /// serialized fingerprint (the decision is derived deterministically from the class
    /// hierarchy the fingerprint already captures).
    #[serde(skip)]
    is_dedup_participant: bool,
}

impl Class {
    pub(crate) fn resource_shape(&self) -> crate::resource::StructuralResources {
        let mut resources = crate::resource::StructuralResources {
            definitions: 1,
            recursion_depth: 1 + self.base_classes.len(),
            model_nodes: 1 + self.direct_base_classes.len() + self.base_classes.len(),
        };
        for method in &self.methods {
            let shape = method.resource_shape();
            resources.definitions += shape.definitions;
            resources.model_nodes += shape.model_nodes;
            resources.recursion_depth = resources.recursion_depth.max(shape.recursion_depth);
        }
        for property in &self.properties {
            let shape = property.resource_shape();
            resources.definitions += shape.definitions;
            resources.model_nodes += shape.model_nodes;
            resources.recursion_depth = resources.recursion_depth.max(shape.recursion_depth);
        }
        resources
    }

    /// Exact Python object paths resolved while emitting this class and its enabled members.
    pub(crate) fn required_import_paths(&self, cfg: &Config) -> Vec<Path> {
        let mut paths = vec![self.name.clone()];
        if cfg.generate_functions {
            paths.extend(self.methods.iter().map(Function::import_path));
        }
        if cfg.generate_properties {
            paths.extend(self.properties.iter().map(Property::import_path));
        }
        paths
    }

    pub(crate) fn parse(
        cfg: &Config,
        class: &pyo3::Bound<pyo3::types::PyType>,
        name: Path,
        diagnostics: &mut Vec<DomainDiagnostic>,
    ) -> Result<Self> {
        let py = class.py();

        // Initialize lists for all members of the class
        let mut methods = Vec::new();
        let mut properties = Vec::new();
        let annotations = Self::parse_live_annotations(cfg, class, &name, diagnostics);
        let annotation_indices = annotations
            .iter()
            .enumerate()
            .map(|(index, annotation)| (annotation.name.as_py().to_owned(), index))
            .collect::<rustc_hash::FxHashMap<_, _>>();
        let mut used_property_annotation_owners = rustc_hash::FxHashMap::default();

        // Extract the list of all attribute names in the module.
        let attr_names = class
            .dir()?
            .iter()
            .map(|attr_name| {
                let attr_name = attr_name.to_string();
                Ident::try_from_py(&attr_name, || {
                    format!("class '{name}' __dir__ returned an empty attribute name")
                })
            })
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .unique()
            .collect_vec();
        let dir_names = attr_names
            .iter()
            .map(|name| name.as_py().to_owned())
            .collect::<rustc_hash::FxHashSet<_>>();
        let getattr_static = crate::runtime_sentinels::inspect_getattr_static(py).ok();

        for attr_name in attr_names {
            // `__init__`/`__call__` are always kept (they seed the constructor and the
            // callable protocol) even though their leading underscore fails the name check;
            // every other attribute rejected by name alone is dropped before dynamic access.
            let is_constructor_or_callable = ["__init__", "__call__"].contains(&attr_name.as_py());
            if !is_constructor_or_callable && !cfg.is_name_allowed(&attr_name) {
                continue;
            }

            // Resolve the raw member without invoking the target metaclass first. A value
            // whose type has no `__get__` cannot change under class descriptor binding and
            // is safe to parse directly. Descriptors (and any uncertain/static-miss case)
            // retain real class access so staticmethod/classmethod/partialmethod and custom
            // descriptor semantics remain unchanged.
            let static_attr = getattr_static
                .as_ref()
                .and_then(|getattr_static| getattr_static.call1((class, attr_name.as_py())).ok());
            let attr = match static_attr {
                Some(static_attr)
                    if getattr_static.as_ref().is_some_and(|getattr_static| {
                        !Self::raw_class_member_requires_binding(py, getattr_static, &static_attr)
                    }) =>
                {
                    Ok(static_attr)
                }
                _ => class.getattr(attr_name.as_py()),
            };
            let Ok(attr) = attr else {
                let attr_name_full = name.join(&attr_name.clone().into());
                crate::report::warn(
                    diagnostics,
                    "class_attribute_inaccessible",
                    attr_name_full.to_py(),
                    format!(
                        "Cannot get attribute '{attr_name}' of '{name}' even though it is listed in its `__dir__`. Bindings will not be generated.",
                    ),
                );
                continue;
            };
            let declared_annotation = annotation_indices
                .get(attr_name.as_py())
                .map(|index| &annotations[*index]);

            let attr_module = Path::from_py(
                &attr
                    .getattr(pyo3::intern!(py, "__module__"))
                    .map(|a| a.to_string())
                    .unwrap_or_default(),
            );
            let attr_type = attr.get_type();
            let runtime_variant =
                AttributeVariant::determine(py, &attr, &attr_type, &attr_module, &name, false)?;
            let partialmethod_type = matches!(
                runtime_variant,
                AttributeVariant::Function | AttributeVariant::Method | AttributeVariant::Closure
            )
            .then(|| {
                Self::exact_raw_partialmethod_type(py, class, &attr_name, &attr, &runtime_variant)
            })
            .flatten();
            let annotation_order = declared_annotation.and_then(|annotation| {
                Self::annotation_runtime_order(class, attr_name.as_py(), &annotation.owner)
            });
            let runtime_is_callable = matches!(
                runtime_variant,
                AttributeVariant::Function | AttributeVariant::Method
            ) || partialmethod_type.is_some();
            if declared_annotation.is_some_and(|annotation| !annotation.is_instance_field) {
                let runtime_is_property_object =
                    matches!(runtime_variant, AttributeVariant::Property)
                        && Property::is_python_property_object(&attr)?;
                // ClassVar/InitVar/KW_ONLY are declaration metadata, not runtime
                // attributes. A callable or real property still visible through the MRO
                // therefore remains an instance API even when the pseudo annotation is
                // declared on a nearer subclass. (`None` means ownership could not be
                // established, so retain the conservative skip.)
                let runtime_override_wins = annotation_order.is_some()
                    && (runtime_is_callable || runtime_is_property_object);
                if !runtime_override_wins {
                    continue;
                }
            }
            let annotation_declares_effective_field = declared_annotation
                .is_some_and(|annotation| annotation.is_instance_field)
                && match annotation_order {
                    Some(std::cmp::Ordering::Less) => true,
                    Some(std::cmp::Ordering::Equal) => {
                        !runtime_is_callable
                            || declared_annotation.is_some_and(|annotation| {
                                Self::is_exact_dataclass_field(
                                    class,
                                    &annotation.owner,
                                    attr_name.as_py(),
                                )
                            })
                    }
                    Some(std::cmp::Ordering::Greater) | None => false,
                };

            if !is_constructor_or_callable
                && !annotation_declares_effective_field
                && !cfg.is_module_allowed(&attr_module)
            {
                continue;
            }

            let attr_name_full = name.join(&attr_name.clone().into());
            // An own instance-field declaration is authoritative over the class/default
            // value's runtime category. This matters for dataclass fields whose default is
            // a function/class/module and for fields shadowing an inherited callable:
            // the generated `__init__` stores an ordinary instance value even though
            // `getattr(Class, name)` still sees the callable descriptor/default.
            let variant = if annotation_declares_effective_field && !is_constructor_or_callable {
                AttributeVariant::Property
            } else if partialmethod_type.is_some() {
                // `partialmethod` applies the descriptor protocol before returning the
                // class attribute. Static/class sources therefore appear here as an
                // exact `functools.partial`, normally classified as a class closure and
                // dropped. Exact raw-descriptor evidence proves this partial is instead
                // a real class API method; arbitrary partial-valued class attributes
                // remain unsupported closures.
                AttributeVariant::Method
            } else {
                runtime_variant
            };
            match variant {
                AttributeVariant::Import => {
                    crate::report::warn(
                        diagnostics,
                        "class_import_skipped",
                        attr_name_full.to_py(),
                        format!(
                            "Imports in classes are not supported: '{name}.{attr_name}'. Bindings will not be generated."
                        ),
                    );
                }
                AttributeVariant::Module => {
                    crate::report::warn(
                        diagnostics,
                        "class_submodule_skipped",
                        attr_name_full.to_py(),
                        format!(
                            "Submodules in classes are not supported: '{name}.{attr_name}'. Bindings will not be generated.",
                        ),
                    );
                }
                AttributeVariant::Class => {
                    // let subclass = Self::parse(cfg, attr.cast()?, attr_name_full)?;
                    // subclasses.push(subclass);
                    crate::report::warn(
                        diagnostics,
                        "class_subclass_skipped",
                        attr_name_full.to_py(),
                        format!(
                            "Subclasses in classes are not supported: '{name}.{attr_name}'. Bindings will not be generated.",
                        ),
                    );
                }
                AttributeVariant::Function | AttributeVariant::Method => {
                    let method = Function::parse(
                        cfg,
                        &attr,
                        attr_name_full,
                        FunctionType::Method {
                            class_path: name.clone(),
                            typ: partialmethod_type.clone().unwrap_or_else(|| {
                                match attr_name.as_py() {
                                    "__init__" => MethodType::Constructor,
                                    "__call__" => MethodType::Callable,
                                    _ => MethodType::Unknown,
                                }
                            }),
                        },
                        diagnostics,
                    )?;
                    methods.push(method);
                }
                AttributeVariant::Closure => {
                    crate::report::warn(
                        diagnostics,
                        "class_closure_skipped",
                        attr_name_full.to_py(),
                        format!(
                            "Closures are not supported in classes: '{attr_name}'. Bindings will not be generated."
                        ),
                    );
                }
                AttributeVariant::TypeVar => {
                    crate::report::warn(
                        diagnostics,
                        "class_type_var_skipped",
                        attr_name_full.to_py(),
                        format!(
                            "TypeVars are not supported in classes: '{attr_name}'. Bindings will not be generated."
                        ),
                    );
                }
                AttributeVariant::Property => {
                    let parsed_property = Property::parse_with_live_annotation(
                        cfg,
                        &attr,
                        attr_name_full,
                        PropertyOwner::Class,
                        declared_annotation.map(|annotation| &annotation.annotation),
                        diagnostics,
                    )?;
                    let mut property = parsed_property.property;
                    let assignment_permission =
                        Self::instance_assignment_permission(py, class, attr_name.as_py());
                    // A Python `property` object's getter/setter signatures are the
                    // authoritative annotations. Plain annotated values and slot/member
                    // descriptors have no such accessor signatures, so use the owning
                    // class's declaration instead of the descriptor/value object's type.
                    let has_property_accessor = Property::is_python_property_object(&attr)?;
                    if parsed_property.used_declared_annotation {
                        if let Some(annotation) = declared_annotation {
                            used_property_annotation_owners
                                .insert(attr_name.as_py().to_owned(), annotation.owner.clone());
                        }
                    }
                    if !has_property_accessor {
                        if let Some(annotation) = declared_annotation {
                            // A class annotation makes a plain class/default value an
                            // instance field, but cannot make an inherited readonly data
                            // descriptor writable: data descriptors take precedence over
                            // the instance dictionary during assignment. Writable slots
                            // retain the mutability already proven by `Property::parse`.
                            let allow_instance_assignment =
                                Self::annotated_field_is_mutable(py, class, attr_name.as_py())
                                    && Self::annotation_allows_instance_assignment(
                                        class,
                                        attr_name.as_py(),
                                    );
                            property.apply_live_annotation(
                                annotation.annotation.clone(),
                                allow_instance_assignment,
                            );
                        }
                    }
                    // Explicit property setters and proven writable slot descriptors are
                    // strong runtime API evidence, so an arbitrary custom `__setattr__`
                    // remains fail-soft (`PyResult` reports any runtime rejection). A
                    // generated frozen-dataclass setter is recognizable, however, and
                    // deterministically rejects exact-class or frozen-field assignment.
                    property.restrict_live_mutability(!matches!(
                        assignment_permission,
                        InstanceAssignmentPermission::Denied
                    ));
                    properties.push(property);
                }
            }
        }

        // Plain and dataclass fields may exist only in `__annotations__`: `dir(Class)`
        // does not expose them until an instance receives a value. Add those declarations
        // after ordinary attributes, preserving annotation order while avoiding duplicate
        // accessors for descriptors/default values already handled above.
        for annotation in annotations {
            if !annotation.is_instance_field || dir_names.contains(annotation.name.as_py()) {
                continue;
            }
            used_property_annotation_owners
                .insert(annotation.name.as_py().to_owned(), annotation.owner.clone());
            let is_mutable = Self::annotated_field_is_mutable(py, class, annotation.name.as_py());
            properties.push(Property::from_live_annotation(
                name.join(&annotation.name.into()),
                annotation.annotation,
                is_mutable,
            ));
        }

        // Extract the docstring of the class
        let docstring = {
            let docstring = class.getattr(pyo3::intern!(py, "__doc__"))?.to_string();
            if docstring.is_empty() || docstring == "None" {
                None
            } else {
                Some(docstring)
            }
        };

        // Direct bases are input identity, not generation scratch: capture them under
        // every config so imported-module fingerprints detect MRO-only changes. The
        // transitive MRO/member-owner data remains deduplication-only.
        let direct_base_classes = Self::capture_live_direct_bases(class)?;
        let (base_classes, member_owners) = if cfg.deduplicate_inherited_members {
            Self::capture_live_hierarchy(
                class,
                &methods,
                &properties,
                &used_property_annotation_owners,
            )?
        } else {
            (Vec::new(), Vec::new())
        };

        Ok(Self {
            name,
            direct_base_classes,
            effective_direct_base_classes: None,
            base_classes,
            member_owners,
            methods,
            properties,
            docstring,
            // Set by normalized live-inheritance planning once the full class set is known.
            is_dedup_participant: false,
        })
    }

    /// Whether class access may transform a raw member through the descriptor protocol.
    ///
    /// Inspect the raw value's type statically as well: a user-defined metaclass for that
    /// type must not turn this safety check into another dynamic callback. Any lookup error
    /// other than an ordinary missing `__get__` is treated conservatively as requiring real
    /// class access.
    fn raw_class_member_requires_binding(
        py: Python<'_>,
        getattr_static: &Bound<'_, PyAny>,
        raw_attr: &Bound<'_, PyAny>,
    ) -> bool {
        let raw_type = raw_attr.get_type();
        match getattr_static.call1((&raw_type, pyo3::intern!(py, "__get__"))) {
            Ok(_) => true,
            Err(error) if error.is_instance_of::<pyo3::exceptions::PyAttributeError>(py) => false,
            Err(_) => true,
        }
    }

    /// Method binding declared by an exact raw `functools.partialmethod` descriptor.
    ///
    /// Attribute access erases the descriptor for static/class sources and yields an
    /// ordinary `functools.partial`; classification must therefore consult the raw class
    /// member. Requiring exact descriptor-type identity keeps arbitrary partial-valued
    /// class attributes, and partialmethod subclasses with custom semantics, out of this
    /// method-only path.
    fn exact_raw_partialmethod_type(
        py: Python<'_>,
        class: &Bound<'_, pyo3::types::PyType>,
        attr_name: &Ident,
        bound_attr: &Bound<'_, PyAny>,
        runtime_variant: &AttributeVariant,
    ) -> Option<MethodType> {
        let raw = crate::runtime_sentinels::inspect_getattr_static(py)
            .ok()?
            .call1((class, attr_name.as_py()))
            .ok()?;
        if !AttributeVariant::is_exact_functools_partialmethod(py, &raw).ok()? {
            return None;
        }

        // Ordinary instance partialmethods expose a helper function on the class.
        // Static/class sources instead expose an exact `functools.partial` whose
        // captured callable is respectively unbound or genuinely bound to the class.
        // Inspect the result of Python's real descriptor binding, avoiding mutable
        // `builtins.staticmethod`/`builtins.classmethod` names and user-spoofable
        // descriptor type metadata.
        match runtime_variant {
            AttributeVariant::Function | AttributeVariant::Method => {
                Some(MethodType::InstanceMethod)
            }
            AttributeVariant::Closure
                if AttributeVariant::is_exact_functools_partial(py, bound_attr).ok()? =>
            {
                let source = bound_attr.getattr(pyo3::intern!(py, "func")).ok()?;
                Some(
                    if Function::exact_bound_method_function(&source).is_some() {
                        MethodType::ClassMethod
                    } else {
                        MethodType::StaticMethod
                    },
                )
            }
            _ => None,
        }
    }

    fn parse_live_annotations(
        cfg: &Config,
        class: &pyo3::Bound<pyo3::types::PyType>,
        class_name: &Path,
        diagnostics: &mut Vec<DomainDiagnostic>,
    ) -> Vec<LiveClassAnnotation> {
        let py = class.py();
        let Ok(mro) = class
            .getattr(pyo3::intern!(py, "__mro__"))
            .and_then(|mro| mro.try_iter())
        else {
            return Vec::new();
        };

        let mut annotations = Vec::new();
        let mut seen = rustc_hash::FxHashSet::default();
        // `__annotations__` itself is deliberately not inherited on modern Python.
        // Walk the MRO nearest-first so a subclass declaration shadows its base, while
        // annotation-only base fields (which `dir(Subclass)` omits) remain reachable on
        // flat wrappers as well as deduplicated ones.
        for owner in mro.filter_map(std::result::Result::ok) {
            let owner_path =
                Self::canonical_live_class_path(&owner).unwrap_or_else(|| class_name.clone());
            let Ok(class_dict) = owner.getattr(pyo3::intern!(py, "__dict__")) else {
                continue;
            };
            let is_dataclass = class_dict
                .get_item(pyo3::intern!(py, "__dataclass_fields__"))
                .is_ok();
            // Under PEP 649 the public STRING path must be tried *first*. Reading
            // `__annotations__` on 3.14 requests VALUE format and may execute annotation
            // metadata before the generator can inspect it; guarding `annotationlib` behind
            // that read failing would make the requested path unreachable. STRING preserves
            // textual annotations, but public introspection may still execute metadata while
            // producing those strings. `Function::inspect_signature` already requests STRING
            // for callables.
            let raw_annotations = if py.version_info() >= (3, 14) {
                let lazy_annotations = (|| {
                    let kwargs = pyo3::types::PyDict::new(py);
                    kwargs.set_item(
                        pyo3::intern!(py, "format"),
                        Function::annotationlib_string_format(py)?,
                    )?;
                    crate::runtime_sentinels::attr(
                        py,
                        crate::runtime_sentinels::StdlibModule::Annotationlib,
                        "get_annotations",
                    )?
                    .call((&owner,), Some(&kwargs))
                })();
                match lazy_annotations {
                    Ok(annotations) => annotations,
                    Err(_) => {
                        crate::report::warn(
                            diagnostics,
                            "class_annotations_unreadable",
                            owner_path.to_py(),
                            format!(
                                "Class '{owner_path}' has unreadable deferred annotations. \
                                     Its annotated fields will not be generated for \
                                     '{class_name}'."
                            ),
                        );
                        continue;
                    }
                }
            } else {
                match class_dict.get_item(pyo3::intern!(py, "__annotations__")) {
                    Ok(raw_annotations) => raw_annotations,
                    Err(_) => continue,
                }
            };
            let Ok(items) = raw_annotations
                .call_method0(pyo3::intern!(py, "items"))
                .and_then(|items| items.try_iter())
            else {
                crate::report::warn(
                    diagnostics,
                    "class_annotations_unreadable",
                    owner_path.to_py(),
                    format!(
                        "Class '{owner_path}' has an unreadable `__annotations__` mapping. \
                         Its annotated fields will not be generated for '{class_name}'."
                    ),
                );
                continue;
            };
            let annotation_globals = Self::live_class_globals(&owner);
            for item in items {
                let Ok(item) = item else {
                    crate::report::warn(
                        diagnostics,
                        "class_annotation_unreadable",
                        owner_path.to_py(),
                        format!(
                            "Class '{owner_path}' yielded an unreadable annotation entry. \
                             The entry was skipped."
                        ),
                    );
                    continue;
                };
                let Ok((raw_name, annotation)) = item.extract::<(String, Py<PyAny>)>() else {
                    crate::report::warn(
                        diagnostics,
                        "class_annotation_unreadable",
                        owner_path.to_py(),
                        format!(
                            "Class '{owner_path}' has an annotation entry without a string name. \
                             The entry was skipped."
                        ),
                    );
                    continue;
                };
                let Ok(name) = Ident::try_from_py(&raw_name, || {
                    format!("class '{owner_path}' has an empty annotation name")
                }) else {
                    crate::report::warn(
                        diagnostics,
                        "class_annotation_name_invalid",
                        owner_path.to_py(),
                        format!(
                            "Class '{owner_path}' has an empty annotation name. The entry was skipped."
                        ),
                    );
                    continue;
                };
                if !seen.insert(raw_name) || !cfg.is_name_allowed(&name) {
                    continue;
                }

                let annotation = annotation.bind(py);
                let is_instance_field = !Self::is_non_instance_annotation(
                    py,
                    annotation,
                    annotation_globals.as_ref(),
                    is_dataclass,
                );
                let annotation = if !is_instance_field {
                    Type::Unknown
                } else {
                    match Function::parse_live_annotation(
                        annotation.clone(),
                        annotation_globals.as_ref(),
                    ) {
                        Ok(annotation) => annotation,
                        Err(error) => {
                            let field_name = class_name.join(&name.clone().into());
                            crate::report::warn(
                                diagnostics,
                                "class_annotation_parse_fallback",
                                field_name.to_py(),
                                format!(
                                    "Class field annotation '{field_name}' could not be parsed: \
                                     {error}. Falling back to an untyped Python object."
                                ),
                            );
                            Type::Unknown
                        }
                    }
                };
                annotations.push(LiveClassAnnotation {
                    name,
                    annotation,
                    is_instance_field,
                    owner: owner_path.clone(),
                });
            }
        }
        annotations
    }

    fn live_class_globals<'py>(
        class: &pyo3::Bound<'py, PyAny>,
    ) -> Option<pyo3::Bound<'py, pyo3::types::PyDict>> {
        let py = class.py();
        let module_name = class
            .getattr(pyo3::intern!(py, "__module__"))
            .ok()?
            .extract::<String>()
            .ok()?;
        let modules = py
            .import(pyo3::intern!(py, "sys"))
            .ok()?
            .getattr(pyo3::intern!(py, "modules"))
            .ok()?
            .cast_into::<pyo3::types::PyDict>()
            .ok()?;
        let module = modules
            .get_item(module_name.as_str())
            .ok()
            .flatten()?
            .cast_into::<pyo3::types::PyModule>()
            .ok()?;
        Some(pyo3::types::PyModuleMethods::dict(&module))
    }

    fn is_non_instance_annotation(
        py: Python<'_>,
        annotation: &Bound<'_, PyAny>,
        globals: Option<&Bound<'_, pyo3::types::PyDict>>,
        is_dataclass: bool,
    ) -> bool {
        use crate::runtime_sentinels::{self, StdlibModule};

        let class_var = runtime_sentinels::attr(py, StdlibModule::Typing, "ClassVar").ok();
        let init_var = runtime_sentinels::attr(py, StdlibModule::Dataclasses, "InitVar").ok();
        let kw_only = runtime_sentinels::optional_attr(py, StdlibModule::Dataclasses, "KW_ONLY")
            .ok()
            .flatten();

        if let Ok(value) = annotation.extract::<String>() {
            let Some(head) = Self::textual_annotation_head(&value) else {
                return false;
            };
            let is_class_var = |value: &Bound<'_, PyAny>| {
                class_var.as_ref().is_some_and(|class_var| {
                    value.is(class_var)
                        || value
                            .getattr(pyo3::intern!(py, "__origin__"))
                            .is_ok_and(|origin| origin.is(class_var))
                })
            };
            let is_init_var = |value: &Bound<'_, PyAny>| {
                init_var.as_ref().is_some_and(|init_var| {
                    value.is(init_var) || value.is_instance(init_var).unwrap_or(false)
                })
            };
            if let Some(binding) =
                globals.and_then(|globals| Self::resolve_textual_annotation_binding(globals, &head))
            {
                return is_class_var(&binding)
                    || (is_dataclass
                        && (is_init_var(&binding)
                            || kw_only
                                .as_ref()
                                .is_some_and(|sentinel| binding.is(sentinel))));
            }
            // With globals available, a missing or shadowed binding is not proven to be
            // a pseudo-field. This also prevents a local `typing` object from making the
            // textual spelling `typing.ClassVar[...]` disappear from the instance API.
            if globals.is_some() {
                return false;
            }
            // When no globals dictionary is available, retain conventional spellings as
            // a best-effort fallback.
            return matches!(
                head.as_str(),
                "ClassVar" | "typing.ClassVar" | "typing_extensions.ClassVar"
            ) || (is_dataclass
                && matches!(
                    head.as_str(),
                    "InitVar" | "KW_ONLY" | "dataclasses.InitVar" | "dataclasses.KW_ONLY"
                ));
        }

        let is_class_var = class_var.as_ref().is_some_and(|class_var| {
            annotation.is(class_var)
                || annotation
                    .getattr(pyo3::intern!(py, "__origin__"))
                    .is_ok_and(|origin| origin.is(class_var))
        });
        let is_init_var = init_var.as_ref().is_some_and(|init_var| {
            annotation.is(init_var) || annotation.is_instance(init_var).unwrap_or(false)
        });
        is_class_var
            || (is_dataclass
                && (is_init_var
                    || kw_only
                        .as_ref()
                        .is_some_and(|kw_only| annotation.is(kw_only))))
    }

    /// Resolve an annotation head through exact module dictionaries only.
    ///
    /// This recognizes imported aliases such as `t.ClassVar` and `dc.InitVar` without
    /// executing user-controlled `__getattr__` hooks. Every resolved value is still
    /// checked by identity against the canonical sentinel by the caller.
    fn resolve_textual_annotation_binding<'py>(
        globals: &Bound<'py, pyo3::types::PyDict>,
        head: &str,
    ) -> Option<Bound<'py, PyAny>> {
        let mut components = head.split('.');
        let root = components.next()?;
        if root.is_empty() {
            return None;
        }
        let mut binding = globals.get_item(root).ok().flatten()?;
        for component in components {
            if component.is_empty() {
                return None;
            }
            let module = binding.cast_into::<pyo3::types::PyModule>().ok()?;
            binding = pyo3::types::PyModuleMethods::dict(&module)
                .get_item(component)
                .ok()
                .flatten()?;
        }
        Some(binding)
    }

    fn textual_annotation_head(value: &str) -> Option<String> {
        let mut value = value.trim();
        while value.len() >= 2 {
            let bytes = value.as_bytes();
            if !matches!(
                (bytes.first(), bytes.last()),
                (Some(b'\''), Some(b'\'')) | (Some(b'"'), Some(b'"'))
            ) {
                break;
            }
            value = value[1..value.len() - 1].trim();
        }
        let compact = value
            .chars()
            .filter(|character| !character.is_whitespace())
            .collect::<String>();
        if compact.is_empty() {
            return None;
        }
        if let Some((head, tail)) = compact.split_once('[') {
            if !tail.ends_with(']') {
                return None;
            }
            (!head.is_empty()).then(|| head.to_owned())
        } else {
            Some(compact)
        }
    }

    fn is_exact_dataclass_field(
        class: &pyo3::Bound<pyo3::types::PyType>,
        owner_path: &Path,
        name: &str,
    ) -> bool {
        let py = class.py();
        let Ok(mro) = class
            .getattr(pyo3::intern!(py, "__mro__"))
            .and_then(|mro| mro.try_iter())
        else {
            return false;
        };
        mro.filter_map(std::result::Result::ok)
            .find(|owner| Self::canonical_live_class_path(owner).as_ref() == Some(owner_path))
            .and_then(|owner| owner.getattr(pyo3::intern!(py, "__dict__")).ok())
            .and_then(|dict| {
                dict.get_item(pyo3::intern!(py, "__dataclass_fields__"))
                    .ok()
            })
            .is_some_and(|fields| fields.contains(name).unwrap_or(false))
    }

    fn exact_frozen_dataclass(py: Python<'_>, class: &pyo3::Bound<'_, PyAny>) -> bool {
        class
            .getattr(pyo3::intern!(py, "__dict__"))
            .and_then(|dict| dict.get_item(pyo3::intern!(py, "__dataclass_params__")))
            .and_then(|params| params.getattr(pyo3::intern!(py, "frozen")))
            .and_then(|frozen| frozen.is_truthy())
            .unwrap_or(false)
    }

    fn exact_dataclass_instance_field(
        py: Python<'_>,
        class: &pyo3::Bound<'_, PyAny>,
        name: &str,
    ) -> bool {
        crate::runtime_sentinels::attr(
            py,
            crate::runtime_sentinels::StdlibModule::Dataclasses,
            "fields",
        )
        .and_then(|fields| fields.call1((class,)))
        .and_then(|fields| fields.try_iter())
        .is_ok_and(|fields| {
            fields.filter_map(std::result::Result::ok).any(|field| {
                field
                    .getattr(pyo3::intern!(py, "name"))
                    .and_then(|name| name.extract::<String>())
                    .is_ok_and(|field_name| field_name == name)
            })
        })
    }

    /// Classify whether `obj.<name> = value` reaches ordinary assignment.
    ///
    /// `object.__setattr__` is a positive proof. Frozen dataclasses generate a
    /// recognizable chain: an exact frozen class rejects every name, while a subclass
    /// passes only names absent from every frozen base's real instance fields. Multiple
    /// frozen bases are checked until the chain reaches `object.__setattr__`. Arbitrary
    /// custom setters remain unknown because introspection cannot prove their behavior.
    fn instance_assignment_permission(
        py: Python<'_>,
        class: &pyo3::Bound<pyo3::types::PyType>,
        name: &str,
    ) -> InstanceAssignmentPermission {
        let Ok(actual_setattr) = class.getattr(pyo3::intern!(py, "__setattr__")) else {
            return InstanceAssignmentPermission::Unknown;
        };
        let Ok(default_setattr) = py
            .get_type::<PyAny>()
            .getattr(pyo3::intern!(py, "__setattr__"))
        else {
            return InstanceAssignmentPermission::Unknown;
        };
        if actual_setattr.is(&default_setattr) {
            return InstanceAssignmentPermission::Allowed;
        }

        let Ok(mro) = class
            .getattr(pyo3::intern!(py, "__mro__"))
            .and_then(|mro| mro.try_iter())
        else {
            return InstanceAssignmentPermission::Unknown;
        };
        let mut reached_actual = false;
        for owner in mro.filter_map(std::result::Result::ok) {
            let Ok(owner_dict) = owner.getattr(pyo3::intern!(py, "__dict__")) else {
                continue;
            };
            let Ok(raw_setattr) = owner_dict.get_item(pyo3::intern!(py, "__setattr__")) else {
                continue;
            };
            if !reached_actual {
                if !raw_setattr.is(&actual_setattr) {
                    continue;
                }
                reached_actual = true;
            }
            if raw_setattr.is(&default_setattr) {
                return InstanceAssignmentPermission::Allowed;
            }
            if !Self::exact_frozen_dataclass(py, &owner) {
                return InstanceAssignmentPermission::Unknown;
            }
            if owner.is(class) || Self::exact_dataclass_instance_field(py, &owner, name) {
                return InstanceAssignmentPermission::Denied;
            }
            // A generated frozen setter delegates non-field names through
            // `super(owner, self).__setattr__`; continue through every later setter.
        }
        InstanceAssignmentPermission::Unknown
    }

    fn annotated_field_is_mutable(
        py: Python<'_>,
        class: &pyo3::Bound<pyo3::types::PyType>,
        name: &str,
    ) -> bool {
        let has_instance_dict = class
            .getattr(pyo3::intern!(py, "__dictoffset__"))
            .and_then(|offset| offset.extract::<isize>())
            .is_ok_and(|offset| offset != 0);
        if !has_instance_dict {
            return false;
        }
        matches!(
            Self::instance_assignment_permission(py, class, name),
            InstanceAssignmentPermission::Allowed
        )
    }

    fn annotation_allows_instance_assignment(
        class: &pyo3::Bound<pyo3::types::PyType>,
        name: &str,
    ) -> bool {
        let py = class.py();
        let Ok(mro) = class
            .getattr(pyo3::intern!(py, "__mro__"))
            .and_then(|mro| mro.try_iter())
        else {
            return false;
        };
        for owner in mro.filter_map(std::result::Result::ok) {
            let Ok(owner_dict) = owner.getattr(pyo3::intern!(py, "__dict__")) else {
                return false;
            };
            let Ok(raw_attribute) = owner_dict.get_item(name) else {
                continue;
            };
            // Python data descriptors take precedence over the instance dictionary.
            // Inspect the raw MRO entry rather than `getattr(class, name)`: a custom
            // descriptor's `__get__` may otherwise hide the rejecting `__set__`.
            let descriptor_type = raw_attribute.get_type();
            let is_data_descriptor = ["__set__", "__delete__"]
                .iter()
                .any(|method| descriptor_type.hasattr(*method).unwrap_or(true));
            return !is_data_descriptor;
        }
        false
    }

    /// Relative MRO order of the nearest annotation owner and raw runtime-member owner.
    ///
    /// An inherited dataclass/plain field whose default is a function must remain a
    /// field on flat subclass wrappers. Conversely, a subclass method/property that
    /// overrides an older inherited annotation is a real runtime override and keeps its
    /// own classification.
    fn annotation_runtime_order(
        class: &pyo3::Bound<pyo3::types::PyType>,
        name: &str,
        annotation_owner: &Path,
    ) -> Option<std::cmp::Ordering> {
        let py = class.py();
        let Ok(mro) = class
            .getattr(pyo3::intern!(py, "__mro__"))
            .and_then(|mro| mro.try_iter())
        else {
            return None;
        };
        let mut annotation_rank = None;
        let mut runtime_rank = None;
        for (rank, owner) in mro.filter_map(std::result::Result::ok).enumerate() {
            if annotation_rank.is_none()
                && Self::canonical_live_class_path(&owner).as_ref() == Some(annotation_owner)
            {
                annotation_rank = Some(rank);
            }
            if runtime_rank.is_none()
                && owner
                    .getattr(pyo3::intern!(py, "__dict__"))
                    .and_then(|dict| dict.contains(name))
                    .unwrap_or(false)
            {
                runtime_rank = Some(rank);
            }
            if annotation_rank.is_some() && runtime_rank.is_some() {
                break;
            }
        }
        match (annotation_rank, runtime_rank) {
            (Some(annotation_rank), Some(runtime_rank)) => Some(annotation_rank.cmp(&runtime_rank)),
            (Some(_), None) => Some(std::cmp::Ordering::Less),
            _ => None,
        }
    }

    fn canonical_live_class_path(typ: &pyo3::Bound<pyo3::types::PyAny>) -> Option<Path> {
        let py = typ.py();
        let module = typ
            .getattr(pyo3::intern!(py, "__module__"))
            .ok()?
            .extract::<String>()
            .ok()?;
        let qualname = typ
            .getattr(pyo3::intern!(py, "__qualname__"))
            .ok()?
            .extract::<String>()
            .ok()?;
        let dotted = if module.is_empty() || module == "builtins" {
            qualname
        } else {
            format!("{module}.{qualname}")
        };
        (!dotted.is_empty()).then(|| Path::from_py(&dotted))
    }

    fn capture_live_direct_bases(class: &pyo3::Bound<pyo3::types::PyType>) -> Result<Vec<Path>> {
        let py = class.py();
        Ok(class
            .getattr(pyo3::intern!(py, "__bases__"))?
            .try_iter()?
            .filter_map(|base| base.ok())
            .filter_map(|base| Self::canonical_live_class_path(&base))
            .collect())
    }

    /// Capture the transitive live class hierarchy for the deduplicated path:
    /// `__mro__` minus the class itself (nearest-first) and, per member, the dotted
    /// `module.qualname` of the effective declaration. For properties, an annotation
    /// actually used by an emitted accessor wins only when its declaration is nearer in
    /// the MRO than the raw runtime owner; otherwise the nearest `__dict__` owner wins.
    /// Normalized live-inheritance planning reconciles these against generated
    /// class names, so out-of-scope ancestors keep their members redeclared.
    ///
    fn capture_live_hierarchy(
        class: &pyo3::Bound<pyo3::types::PyType>,
        methods: &[Function],
        properties: &[Property],
        used_property_annotation_owners: &rustc_hash::FxHashMap<String, Path>,
    ) -> Result<CapturedHierarchy> {
        let py = class.py();

        // Full MRO (nearest-first, includes the class itself at index 0).
        let mro: Vec<pyo3::Bound<pyo3::types::PyAny>> = class
            .getattr(pyo3::intern!(py, "__mro__"))?
            .try_iter()?
            .filter_map(|typ| typ.ok())
            .collect();

        // Ancestor closure: the MRO minus the class head.
        let base_classes: Vec<Path> = mro
            .iter()
            .skip(1)
            .filter_map(Self::canonical_live_class_path)
            .collect();

        // For each MRO class, its canonical path plus the names it owns directly
        // (`__dict__`); a member's owner is then the first MRO class that carries it.
        let mro_dicts: Vec<(Path, rustc_hash::FxHashSet<String>)> = mro
            .iter()
            .filter_map(|typ| {
                let path = Self::canonical_live_class_path(typ)?;
                let names = typ
                    .getattr(pyo3::intern!(py, "__dict__"))
                    .ok()?
                    .try_iter()
                    .ok()?
                    .filter_map(|key| key.ok())
                    .filter_map(|key| key.extract::<String>().ok())
                    .collect();
                Some((path, names))
            })
            .collect();

        let owner_of = |py_name: &str| -> Option<Path> {
            mro_dicts
                .iter()
                .find(|(_, names)| names.contains(py_name))
                .map(|(path, _)| path.clone())
        };
        let nearest_owner = |runtime_owner: Option<Path>, annotation_owner: Option<Path>| match (
            runtime_owner,
            annotation_owner,
        ) {
            (Some(runtime_owner), Some(annotation_owner)) => {
                let runtime_rank = mro_dicts
                    .iter()
                    .position(|(path, _)| path == &runtime_owner);
                let annotation_rank = mro_dicts
                    .iter()
                    .position(|(path, _)| path == &annotation_owner);
                if annotation_rank
                    .zip(runtime_rank)
                    .is_some_and(|(annotation_rank, runtime_rank)| annotation_rank < runtime_rank)
                {
                    Some(annotation_owner)
                } else {
                    Some(runtime_owner)
                }
            }
            (runtime_owner, annotation_owner) => runtime_owner.or(annotation_owner),
        };
        let mut member_owners: Vec<(String, Path)> = Vec::new();
        for method in methods {
            let py_name = method.name.name().as_py();
            if let Some(owner) = owner_of(py_name) {
                member_owners.push((py_name.to_owned(), owner));
            }
        }
        for property in properties {
            let py_name = property.name.name().as_py();
            if let Some(owner) = nearest_owner(
                owner_of(py_name),
                used_property_annotation_owners.get(py_name).cloned(),
            ) {
                member_owners.push((py_name.to_owned(), owner));
            }
        }

        Ok((base_classes, member_owners))
    }

    /// Deduplicated live path: reconcile the captured hierarchy against the set of
    /// generated class newtype paths. Drops out-of-scope entries from the base lists and
    /// removes any member whose owner is an in-scope ancestor -- that member stays
    /// callable through the ancestor's `*Methods` supertrait, so nothing is lost. A
    /// member whose owner is out of scope (e.g. `object`, a C-level base, or a class not
    /// being generated) is kept and redeclared, exactly as on the non-deduplicated path.
    ///
    /// Returns the number of members dropped -- the direct size saving this class gets
    /// from the deduplicated model, weighed against the model's per-class overhead when
    /// Normalized live-inheritance planning decides whether the class's inheritance
    /// component is worth deduplicating.
    ///
    /// Deduplicated live path, step 1: filter the captured hierarchy to the in-scope
    /// classes and report `(dropped_forwarder_units, estimated_saving_bytes)` for the
    /// members deduplication would drop -- their forwarder-unit count (a mutable property
    /// counts as two; subtracted from [`Self::forwarder_units`] for the surviving-own
    /// tally) and the estimated bytes the flat model would otherwise re-emit for them,
    /// measured against `cfg` so a docs-disabled or generation-gated build is not credited
    /// bytes it never emits (the direct size saving the per-component cost model weighs).
    /// Members are NOT dropped yet; a class only drops
    /// them (via [`Self::apply_inheritance_dedup`]) once its whole component is judged worth
    /// deduplicating, because a flat class has no supertrait to inherit them through.
    pub(crate) fn filter_bases_report_droppable(
        &mut self,
        class_paths: &rustc_hash::FxHashSet<Path>,
        cfg: &Config,
    ) -> (usize, i64) {
        self.base_classes.retain(|base| class_paths.contains(base));

        let inherited = self.inherited_member_names();
        let mut dropped_units = 0usize;
        let mut saving = 0i64;
        for method in &self.methods {
            if inherited.contains(method.name.name().as_py()) {
                dropped_units += 1;
                saving += method.estimated_flat_size(cfg);
            }
        }
        for property in &self.properties {
            if inherited.contains(property.name.name().as_py()) {
                dropped_units += property.forwarder_units();
                saving += property.estimated_flat_size(cfg);
            }
        }
        (dropped_units, saving)
    }

    /// Deduplicated live path, step 2: drop the members reachable through an in-scope
    /// ancestor's `*Methods` supertrait. Called only once this class's whole inheritance
    /// component has been judged worth deduplicating -- a flat class must keep every
    /// member, since it has no supertrait through which to inherit them.
    pub(crate) fn apply_inheritance_dedup(&mut self) {
        let inherited = self.inherited_member_names();
        self.methods
            .retain(|method| !inherited.contains(method.name.name().as_py()));
        self.properties
            .retain(|property| !inherited.contains(property.name.name().as_py()));
    }

    /// Names of members whose nearest MRO owner is an in-scope ancestor AND that are
    /// carried on a `*Methods` supertrait, so a subclass inherits them rather than
    /// redeclaring them.
    ///
    /// Only instance/callable methods and properties lower to trait items (see
    /// [`super::Function::generate`] / [`super::Property::generate`]); static methods,
    /// class methods and constructors (`__init__`) lower to INHERENT `impl` items, which
    /// Rust does not inherit across newtypes. Such a member must stay redeclared on the
    /// subclass -- exactly as the flat model does -- or it would be silently uncallable
    /// (e.g. a subclass with no own `__init__` would lose its constructor). Restricting
    /// the drop to trait-carried members is what keeps the partition lossless.
    fn inherited_member_names(&self) -> rustc_hash::FxHashSet<String> {
        let ancestors: rustc_hash::FxHashSet<Path> = self.base_classes.iter().cloned().collect();
        let mut trait_carried: rustc_hash::FxHashSet<&str> = self
            .methods
            .iter()
            .filter(|method| {
                matches!(
                    method.typ,
                    FunctionType::Method {
                        typ: MethodType::InstanceMethod | MethodType::Callable,
                        ..
                    }
                )
            })
            .map(|method| method.name.name().as_py())
            .collect();
        for property in &self.properties {
            trait_carried.insert(property.name.name().as_py());
        }
        self.member_owners
            .iter()
            .filter(|(name, owner)| {
                ancestors.contains(owner) && trait_carried.contains(name.as_str())
            })
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// Number of inherent forwarders the deduplicated model emits for this class's own
    /// members: one per method and per emitted property accessor. The cost model charges
    /// the per-surviving-member forwarder tax against this.
    pub(crate) fn forwarder_units(&self) -> usize {
        self.methods.len()
            + self
                .properties
                .iter()
                .map(Property::forwarder_units)
                .sum::<usize>()
    }

    /// Number of in-scope ancestors (after
    /// [`Self::filter_bases_report_droppable`] filtering).
    pub(crate) fn ancestor_count(&self) -> usize {
        self.base_classes.len()
    }

    /// Direct + transitive in-scope bases, used to union a class with its ancestors into
    /// one inheritance component.
    pub(crate) fn base_class_paths(&self) -> &[Path] {
        &self.base_classes
    }

    /// Deduplicated live path: record whether this class's inheritance component was
    /// judged worth deduplicating by normalized live-inheritance planning. Every
    /// class in one component shares the same decision -- a deduplicated class cannot
    /// inherit from a flat one -- so the choice is made per component, not per class.
    ///
    pub(crate) fn set_dedup_participant(&mut self, participate: bool) {
        self.is_dedup_participant = participate;
    }

    pub(crate) fn set_effective_direct_bases(&mut self, bases: Vec<Path>) {
        self.effective_direct_base_classes = Some(bases);
    }

    /// Whether this class emits the deduplicated object model (and thus references the
    /// synthetic root trait). Always `false` on the external-IR path, which gates the root
    /// trait on [`crate::Codegen`]'s IR check instead.
    pub(crate) fn is_dedup_participant(&self) -> bool {
        self.is_dedup_participant
    }

    /// Whether this candidate emits the deduplicated model for this class.
    pub(crate) fn uses_dedup_model(&self, import_mode: &ImportMode) -> bool {
        import_mode.uses_dedup_model()
            && (!import_mode.is_live_deduplicated() || self.is_dedup_participant)
    }

    /// Construct a [`Class`] from the generic IR with zero `pyo3` usage.
    ///
    /// Builds the same struct as [`Self::parse`] but from strings: no `dir()`,
    /// `getattr()`, or `AttributeVariant::determine()`.
    pub(crate) fn from_ir(
        cfg: &Config,
        ir: &crate::schema::ClassIr,
        diagnostics: &mut Vec<DomainDiagnostic>,
        local_annotation_types: &super::IrAnnotationTypes,
    ) -> Result<Self> {
        let name = Path::from_py(&ir.name);
        let base_classes: Vec<Path> = ir
            .base_classes
            .iter()
            .map(|base| Path::from_py(base))
            .collect();
        let methods = ir
            .methods
            .iter()
            .map(|method| {
                let typ = FunctionType::from_ir_kind(&method.kind, Some(&name));
                Function::from_ir(cfg, method, typ, diagnostics, local_annotation_types)
            })
            .collect::<Result<Vec<_>>>()?;
        let properties = ir
            .properties
            .iter()
            .map(|property| {
                Property::from_ir(cfg, property, PropertyOwner::Class, local_annotation_types)
            })
            .collect::<Result<Vec<_>>>()?;
        let docstring = ir.docstring.clone();

        Ok(Self {
            name,
            direct_base_classes: base_classes.clone(),
            effective_direct_base_classes: None,
            base_classes,
            member_owners: Vec::new(),
            methods,
            properties,
            docstring,
            // Unused on the external-IR path (it uses the deduplicated model unconditionally).
            is_dedup_participant: false,
        })
    }

    /// Whether any method or property of this class references a collection-protocol
    /// type that lowers to the generic `PyCollection<T>` wrapper.
    pub(crate) fn mentions_enabled_collection(&self, cfg: &Config) -> bool {
        (cfg.generate_functions
            && self
                .methods
                .iter()
                .any(super::Function::mentions_collection))
            || (cfg.generate_properties
                && self
                    .properties
                    .iter()
                    .any(super::Property::mentions_collection))
    }

    /// This class's own generated members: inherent `impl` items, trait method
    /// declarations/bodies, per-type impl bodies, and (external-IR path only) the
    /// data needed to emit one inherent forwarder per callable name, keyed by the
    /// member's Python name.
    fn generate_members(
        &self,
        cfg: &Config,
        local_types: &super::LocalTypes,
        diagnostics: &mut Vec<DomainDiagnostic>,
        import_mode: &ImportMode,
    ) -> Result<GeneratedMembers> {
        #[cfg(test)]
        MEMBER_GENERATION_CALLS.with(|calls| calls.set(calls.get() + 1));
        // Get the names of all methods to avoid name clashes
        let mut scoped_function_names = if cfg.generate_functions {
            super::naming::NameAllocator::from_python_names(
                self.methods.iter().map(|method| method.name.name()),
                super::naming::SymbolKind::Function,
            )
        } else {
            super::naming::NameAllocator::default()
        };
        scoped_function_names.reserve_inherent_class_methods();
        if import_mode.uses_dedup_model() {
            // The root/marker trait methods (`downcast`, `__pyo3_obj`, ...) exist only
            // on the external-IR path; reserve their names so a same-named Python
            // member is renamed instead of shadowing the generic handle/downcast API.
            scoped_function_names.reserve_root_trait_methods();
        }

        let mut struct_impl = proc_macro2::TokenStream::new();
        let mut method_defs = proc_macro2::TokenStream::new();
        let mut method_impls = proc_macro2::TokenStream::new();
        // (name, forwarders) per own member, in declaration order. Empty on the live
        // path; on the external-IR path each entry drives an inherent forwarder that
        // takes resolution precedence over every same-named ancestor trait method.
        let mut forwarders: Vec<(String, Vec<super::Forwarder>)> = Vec::new();
        // Methods
        if cfg.generate_functions {
            for method in &self.methods {
                let def = method.generate(
                    cfg,
                    &mut scoped_function_names,
                    local_types,
                    diagnostics,
                    import_mode,
                )?;
                match def {
                    FunctionImplementation::Function(impl_fn) => {
                        struct_impl.extend(impl_fn);
                    }
                    FunctionImplementation::Method(tm) => {
                        if !tm.forwarders.is_empty() {
                            forwarders.push((method.name.name().as_py().to_owned(), tm.forwarders));
                        }
                        method_defs.extend(tm.trait_fn);
                        method_impls.extend(tm.impl_fn);
                    }
                }
            }
        }
        // Properties
        if cfg.generate_properties {
            for property in &self.properties {
                let def =
                    property.generate(cfg, &mut scoped_function_names, local_types, import_mode)?;
                match def {
                    FunctionImplementation::Function(impl_fn) => {
                        struct_impl.extend(impl_fn);
                    }
                    FunctionImplementation::Method(tm) => {
                        if !tm.forwarders.is_empty() {
                            forwarders
                                .push((property.name.name().as_py().to_owned(), tm.forwarders));
                        }
                        method_defs.extend(tm.trait_fn);
                        method_impls.extend(tm.impl_fn);
                    }
                }
            }
        }
        scoped_function_names.validate_resolutions()?;

        Ok(GeneratedMembers {
            struct_impl,
            method_defs,
            method_impls,
            forwarders,
            resolved_members: scoped_function_names.resolved_members(),
        })
    }

    /// Reject an inherited trait surface on which two distinct Python members become
    /// the same Rust method after rustc's Unicode NFC/raw-identifier canonicalization.
    ///
    /// Each ancestor owns a separate `*Methods` trait and therefore allocates member
    /// names independently. Two spellings such as NFD `e\u{301}` and NFC `é` can be
    /// distinct Python attributes but the same Rust method. Letting both through makes
    /// a descendant bound by both traits ambiguous (E0034); adding an inherent
    /// forwarder would merely hide one of the Python attributes. This is therefore an
    /// irreconcilable code-generation collision, not a recoverable diamond. Own
    /// inherent/forwarding members are deliberately excluded: inherent lookup can
    /// resolve those, and existing constructor-vs-inherited-method surfaces rely on
    /// explicit UFCS for the shadowed trait method.
    #[allow(clippy::too_many_arguments)]
    fn validate_forwarder_ident_collisions(
        &self,
        local_types: &super::LocalTypes,
        enum_types: &rustc_hash::FxHashSet<Path>,
        all_classes: &rustc_hash::FxHashMap<Path, &Class>,
        cfg: &Config,
        import_mode: &ImportMode,
        inherited_members: &InheritedMemberPlan,
    ) -> Result<ValidatedAncestorForwarders> {
        let mut validated = ValidatedAncestorForwarders::default();
        // canonical Rust ident -> (Python name, defining class)
        let mut emitted: rustc_hash::FxHashMap<String, (String, String)> =
            rustc_hash::FxHashMap::default();
        let class_name = self.name.to_py();
        let mut register = |python_name: &str, rust_ident: &str, owner: &str| -> Result<()> {
            let canonical_ident = super::naming::canonical_rust_name(rust_ident);
            if let Some((previous_python_name, previous_owner)) = emitted.get(&canonical_ident) {
                if previous_python_name != python_name {
                    return Err(crate::PyBindgenError::CodegenError(format!(
                        "Class '{class_name}' exposes distinct Python members \
                         '{previous_python_name}' (from '{previous_owner}') and \
                         '{python_name}' (from '{owner}') as the same canonical Rust method \
                         identifier '{canonical_ident}'; generated callers cannot address \
                         both members unambiguously"
                    )));
                }
                return Ok(());
            }
            emitted.insert(canonical_ident, (python_name.to_owned(), owner.to_owned()));
            Ok(())
        };

        // Only resolvable class traits contribute methods to this descendant. Replay
        // each ancestor's own member allocation to compare the exact emitted idents,
        // including deterministic getter/setter and reserved-name renames.
        for base in &self.base_classes {
            if methods_trait_path(base, local_types, enum_types).is_none() {
                continue;
            }
            let Some(ancestor) = all_classes.get(base) else {
                continue;
            };
            // Replay each trait in the same owner-module annotation namespace used by
            // its real emission. Besides identifiers, retain selected winner signatures;
            // member generation still lowers complete signatures; using the descendant's
            // short aliases could rebind a type or make validation fail before rule (b)
            // gets a chance to synthesize the forwarder.
            let ancestor_local_types =
                local_types.scoped_to(ancestor.name.parent().unwrap_or_default());
            let ancestor_members = ancestor.generate_members(
                cfg,
                &ancestor_local_types,
                &mut Vec::new(),
                import_mode,
            )?;
            let owner = base.to_py();
            for (python_name, forwarders) in &ancestor_members.forwarders {
                for forwarder in forwarders {
                    register(python_name.as_str(), &forwarder.ident.to_string(), &owner)?;
                }
            }
            // Validation must still run in ancestor order, including when forwarders
            // are suppressed. Retain only signatures needed by enabled emission;
            // the generated trait bodies can be dropped immediately.
            if cfg.generate_inherent_forwarders {
                if let Some(names) = inherited_members.winners.get(base) {
                    validated.members.insert(
                        base.clone(),
                        ancestor_members
                            .forwarders
                            .into_iter()
                            .filter(|(name, _)| names.contains(name))
                            .collect(),
                    );
                }
            }
        }

        Ok(validated)
    }

    /// Build the inherent `pub fn` forwarders emitted on this newtype's own inherent
    /// `impl` (external-IR path only). See the mechanism note in [`Self::generate`].
    ///
    /// Two sources contribute:
    /// - Rule (a): this class's OWN members (`member_forwarders`), each delegating to
    ///   `<Self as CMethods>::name`.
    /// - Rule (b): a name declared by two or more of this class's ancestors that this
    ///   class does NOT itself declare (a diamond). Without an inherent method such a
    ///   name is ambiguous (E0034) or uncallable (E0599) on a concrete newtype for some
    ///   caller import combinations. The nearest ancestor (MRO order) wins; the
    ///   forwarder delegates to `<Self as WinnerMethods>::name`. Ancestors are resolved
    ///   through `all_classes`, the GLOBAL class registry, so a diamond whose colliding
    ///   ancestors live in DIFFERENT modules is handled identically to a same-module
    ///   one: the winner trait is nameable from the subclass's module (module-relative
    ///   via `local_types`), and the forwarder compiles across the one generated crate.
    ///
    /// Limitations (each keeps generation total and warns rather than failing):
    /// - When the MRO-nearest declarer's trait is not resolvable (an enum, a type-var, or
    ///   out of the generated scope), no forwarder can be synthesized; the name stays
    ///   ambiguous for a caller importing the other ancestors' traits. Reported as
    ///   `class_diamond_winner_unresolved`.
    /// - When the MRO-nearest declarer carries the name as a NON-forwarding member
    ///   (static/class method or constructor), there is no trait method to delegate to,
    ///   so no forwarder is emitted; a caller importing two other (instance-method)
    ///   ancestor traits can still hit E0034. Reported as
    ///   `class_forwarder_winner_not_callable`.
    /// - A generic receiver bounded only by `T: ChildMethods` has no access to Child's
    ///   inherent methods. Same-named ancestor supertrait methods therefore still require
    ///   explicit UFCS (`<T as WinnerMethods>::name(...)`) in generic code.
    #[allow(clippy::too_many_arguments)]
    fn inherent_forwarders(
        &self,
        struct_ident: &syn::Ident,
        trait_ident: &syn::Ident,
        member_forwarders: &[(String, Vec<super::Forwarder>)],
        own_resolved_members: &[(String, String)],
        local_types: &super::LocalTypes,
        enum_types: &rustc_hash::FxHashSet<Path>,
        inherited_members: &InheritedMemberPlan,
        validated: &ValidatedAncestorForwarders,
        diagnostics: &mut Vec<DomainDiagnostic>,
    ) -> Result<proc_macro2::TokenStream> {
        // Every inherent identifier already present on the newtype, so a synthesized
        // rule-(b) forwarder can never collide (which would be E0201). Seed with the
        // boilerplate inherent methods every newtype carries, the always-in-scope
        // root/marker trait method names, and this class's OWN inherent methods --
        // constructors, static and class methods land on the inherent `impl` under a
        // Rust ident (`__init__` -> `new`) that `owns_name` (which compares Python
        // names) cannot see.
        let mut emitted: rustc_hash::FxHashSet<String> = super::naming::INHERENT_CLASS_METHODS
            .iter()
            .chain(super::naming::RESERVED_ROOT_TRAIT_METHODS.iter())
            .map(|name| super::naming::canonical_rust_name(name))
            .chain(
                own_resolved_members
                    .iter()
                    .map(|(_, name)| super::naming::canonical_rust_name(name)),
            )
            .collect();

        let render = |forwarder: &super::Forwarder, trait_path: &proc_macro2::TokenStream| {
            let super::Forwarder {
                ident,
                params,
                args,
                ret,
            } = forwarder;
            quote::quote! {
                #[inline]
                pub fn #ident<'py>(
                    &self,
                    py: ::pyo3::marker::Python<'py>,
                    #params
                ) -> ::pyo3::PyResult<#ret> {
                    <Self as #trait_path>::#ident(self, py, #args)
                }
            }
        };

        let mut forwarders_tokens = proc_macro2::TokenStream::new();

        // Rule (a): this class's own members. Their idents come from the same
        // allocator that reserved the boilerplate, so they are collision-free.
        let own_trait_path = quote::quote! { #trait_ident };
        for (_py_name, fwds) in member_forwarders {
            for forwarder in fwds {
                emitted.insert(super::naming::canonical_rust_name(
                    &forwarder.ident.to_string(),
                ));
                forwarders_tokens.extend(render(forwarder, &own_trait_path));
            }
        }

        // Rule (b): consume candidate-local, nearest-first collision facts.
        for collision in &inherited_members.collisions {
            let name = &collision.name;
            let winner_base = &collision.winner;
            let Some(winner_trait_path) = methods_trait_path(winner_base, local_types, enum_types)
            else {
                // Avenue B: a genuine multi-ancestor collision whose MRO-nearest declarer
                // does not resolve to a `*Methods` trait (it is an enum, a type-var, or
                // otherwise out of the generated scope), so no inherent forwarder can be
                // synthesized. The name stays ambiguous (E0034) for a caller importing the
                // other resolvable ancestors' traits. Surface it instead of dropping it
                // silently. Byte-neutral: `warn` only appends to the diagnostics vec.
                crate::report::warn(
                    diagnostics,
                    "class_diamond_winner_unresolved",
                    self.name.to_py(),
                    format!(
                        "Inherited member '{name}' of '{}' is declared by {} ancestors, but its MRO-nearest declarer '{}' is not a resolvable class trait (out of scope, an enum, or a type-var); no inherent forwarder is emitted, so calling `.{name}(...)` may be ambiguous across the other ancestors' `*Methods` traits.",
                        self.name,
                        collision.declarer_count,
                        winner_base,
                    ),
                );
                continue;
            };
            let winner_trait_tokens = quote::quote! { #winner_trait_path };
            let winner_members = validated.members.get(winner_base).ok_or_else(|| {
                crate::PyBindgenError::CodegenError(format!(
                    "inherited-member planning invariant violated: no validated signatures for winner '{}' of class '{}'",
                    winner_base, self.name
                ))
            })?;
            // Ordered ancestor validation already lowered these exact signatures in
            // the winner's owner-module annotation scope with descendant-relative
            // Rust paths. Reusing them preserves renames and avoids regenerating all
            // of the winner's members once for every colliding Python name.
            let Some((_, fwds)) = winner_members
                .iter()
                .find(|(member_name, _)| member_name == name)
            else {
                // The C3-nearest declarer carries this name as a NON-forwarding member
                // (a static/class method or constructor -- these live on the inherent
                // `impl`, not a `*Methods` trait, so there is nothing to delegate to).
                // Python's MRO binds the name there, shadowing the other ancestors'
                // instance members; but those still emit `impl *Methods for Self {}`, so
                // a caller importing two of them and writing `x.name(...)` hits an
                // ambiguity (E0034). Surface it instead of leaving the diamond silently
                // unresolved -- the call must go through the resolving type or a specific
                // trait, e.g. `<Winner>::name(...)`.
                crate::report::warn(
                    diagnostics,
                    "class_forwarder_winner_not_callable",
                    self.name.to_py(),
                    format!(
                        "Inherited member '{name}' of '{}' resolves by MRO to '{}', which declares it as a static/class method or constructor; no inherent forwarder is emitted, so calling `.{name}(...)` may be ambiguous across the other ancestors' `*Methods` traits. Call it through '{}' or the specific trait.",
                        self.name,
                        winner_base.name(),
                        winner_base.name(),
                    ),
                );
                continue;
            };
            for forwarder in fwds {
                let ident = forwarder.ident.to_string();
                if !emitted.insert(super::naming::canonical_rust_name(&ident)) {
                    // An inherited name whose Rust ident already exists on the newtype
                    // (another forwarder or boilerplate). Skip rather than emit a
                    // duplicate inherent method (E0201).
                    crate::report::warn(
                        diagnostics,
                        "class_forwarder_ident_conflict",
                        self.name.to_py(),
                        format!(
                            "Inherited member '{name}' of '{}' maps to Rust identifier '{}', which is already defined on the wrapper; its inherent forwarder is skipped and the name must be called through an explicit trait, e.g. `<{} as {}Methods>::{}`.",
                            self.name,
                            forwarder.ident,
                            struct_ident,
                            winner_base.name(),
                            forwarder.ident,
                        ),
                    );
                    continue;
                }
                forwarders_tokens.extend(render(forwarder, &winner_trait_tokens));
            }
        }

        Ok(forwarders_tokens)
    }

    /// Detection-only counterpart to [`Self::inherent_forwarders`] rule (b), used when
    /// inherent forwarders are suppressed (`generate_inherent_forwarders = false`).
    /// No inherent method is emitted in that mode, so every name declared by two or
    /// more ancestors (resolved through the global `all_classes` registry, so cross-module
    /// diamonds count too) is ambiguous (E0034) for a consumer importing more than one
    /// ancestor `*Methods` trait and calling it on the concrete receiver. This walks the
    /// same ancestor-collision set and warns once per such name. Emits NO tokens -- it is a
    /// pure diagnostic pass, keeping the suppressed-forwarder output byte-identical.
    fn detect_ambiguous_without_forwarders(
        &self,
        inherited_members: &InheritedMemberPlan,
        diagnostics: &mut Vec<DomainDiagnostic>,
    ) {
        for collision in &inherited_members.collisions {
            let name = &collision.name;
            crate::report::warn(
                    diagnostics,
                    "class_ambiguous_without_forwarders",
                    self.name.to_py(),
                    format!(
                        "Inherited member '{name}' of '{}' is declared by multiple ancestors, but inherent forwarders are disabled, so calling `.{name}(...)` on a '{}' is ambiguous (E0034) for a consumer importing more than one ancestor `*Methods` trait. Call it through a specific trait, e.g. `<{} as ...Methods>::{name}(...)`.",
                        self.name,
                        self.name.name(),
                        self.name.name(),
                    ),
                );
        }
    }

    /// Generate the bindings for this class.
    ///
    /// `all_classes` is the GLOBAL class registry (full Python path -> [`Class`],
    /// spanning every module), consulted only on the deduplicated object model to
    /// synthesize inherent forwarders for names inherited from two or more ancestors
    /// (see the inherent-forwarder handling below). Because it is global, a diamond
    /// whose colliding ancestors span different modules is resolved exactly like a
    /// same-module one. When inherent forwarders are suppressed, it still drives a
    /// detection-only ambiguity diagnostic (no tokens emitted).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn generate(
        &self,
        cfg: &Config,
        local_types: &super::LocalTypes,
        enum_types: &rustc_hash::FxHashSet<Path>,
        diagnostics: &mut Vec<DomainDiagnostic>,
        import_mode: &ImportMode,
        all_classes: &rustc_hash::FxHashMap<Path, &Class>,
        inherited_members: &InheritedMemberPlan,
    ) -> Result<proc_macro2::TokenStream> {
        let mut output = proc_macro2::TokenStream::new();

        // Adaptive live path: an isolated class -- one that neither has an in-scope
        // ancestor nor is one -- gains nothing from the deduplicated object model, so it
        // emits with the flat model instead and avoids that model's per-class overhead
        // (root/marker impls, inherent forwarders, supertrait indirection). Participants
        // and the external-IR path keep the deduplicated model. Both live model choices use
        // the same pre-resolved prefixes, so this only changes emitted object-model tokens.
        let flat_mode = import_mode.with_flat_live_model();
        let import_mode = if self.uses_dedup_model(import_mode) {
            import_mode
        } else {
            &flat_mode
        };

        // Documentation
        if cfg.generate_docs {
            if let Some(mut docstring) = self.docstring.clone() {
                crate::utils::text::format_docstring(&mut docstring);
                output.extend(quote::quote! {
                    #[doc = #docstring]
                });
            }
        }

        // Generate the struct
        //
        // (#19) The struct identifier is derived through the SHARED `sanitized_struct_ident`
        // transform, the same one every cross-reference site uses, so the definition and its
        // references (`<Base>Methods` supertraits, ancestor upcasts) stay in lock-step for a
        // class whose Python name requires sanitization (`Self` -> `s_Self`).
        let name = self.name.name();
        let struct_ident: syn::Ident = match super::naming::rust_ident(
            name,
            super::naming::SymbolKind::Class,
        ) {
            Some(ident) => {
                if TryInto::<syn::Ident>::try_into(name).is_err() {
                    crate::report::warn(
                        diagnostics,
                        "class_invalid_ident_renamed",
                        self.name.to_py(),
                        format!(
                            "Struct '{}' is an invalid Rust ident for a struct name. Renamed to '{ident}'.",
                            self.name,
                        ),
                    );
                }
                ident
            }
            None => {
                crate::report::warn(
                    diagnostics,
                    "class_invalid_ident_skipped",
                    self.name.to_py(),
                    format!(
                        "Struct '{}' is an invalid Rust ident for a struct name. Renaming failed. Bindings will not be generated.",
                        self.name
                    ),
                );
                return Ok(proc_macro2::TokenStream::new());
            }
        };
        let object_name = self.name.to_py();
        let object_import = import_mode.resolve(&self.name)?;

        // Deduplicated model only (external-IR and live participants): the newtype is a
        // `#[repr(transparent)]` wrapper over its single `Py<PyAny>` field, which is what
        // makes the zero-cost upcast pointer-retag (CR3) sound. A flat class (a live
        // isolated class) omits the attr, staying byte-identical to the non-deduplicated
        // live output.
        let uses_dedup_model = import_mode.uses_dedup_model();
        let struct_repr_attr = if uses_dedup_model {
            quote::quote! { #[repr(transparent)] }
        } else {
            proc_macro2::TokenStream::new()
        };
        // (D3) `bind`/`as_any`/`clone_ref` have bodies identical modulo `Self`, so on the
        // deduplicated model -- where every newtype already implements the root and marker
        // traits -- they are carried ONCE as `#[inline]` default methods on those traits
        // instead of being re-emitted on each newtype's inherent `impl`. A flat class
        // implements neither trait (that is precisely what keeps the flat model
        // byte-identical to the non-deduplicated build), so it keeps the inherent copies.
        //
        // `into_py_any` and `from_py_any` stay per-class INHERENT under both models:
        // `from_py_any` names the concrete class and its runtime import expression, and
        // `into_py_any` must stay inherent because pyo3's `IntoPyObjectExt` blanket impl
        // supplies a same-named method -- hoisting it to the marker trait made
        // `wrapper.into_py_any()` ambiguous (E0034) as soon as a consumer imported
        // `pyo3::conversion::IntoPyObjectExt`. An inherent method wins resolution against
        // every same-named trait method, which is exactly this crate's ambiguity doctrine.
        let shared_wrapper_boilerplate = if uses_dedup_model {
            proc_macro2::TokenStream::new()
        } else {
            quote::quote! {
                #[inline]
                pub fn bind<'py>(
                    &self,
                    py: ::pyo3::marker::Python<'py>,
                ) -> &::pyo3::Bound<'py, ::pyo3::types::PyAny> {
                    self.obj.bind(py)
                }

                #[inline]
                pub fn as_any<'py>(
                    &self,
                    py: ::pyo3::marker::Python<'py>,
                ) -> &::pyo3::Bound<'py, ::pyo3::types::PyAny> {
                    self.bind(py)
                }

                #[inline]
                pub fn clone_ref(&self, py: ::pyo3::marker::Python<'_>) -> Self {
                    Self {
                        obj: self.obj.clone_ref(py),
                    }
                }
            }
        };
        output.extend(quote::quote! {
            #[derive(Debug)]
            #struct_repr_attr
            pub struct #struct_ident {
                obj: ::pyo3::Py<::pyo3::types::PyAny>,
            }

            #[automatically_derived]
            impl #struct_ident {
                pub fn from_py_any<'py>(
                    obj: ::pyo3::Bound<'py, ::pyo3::types::PyAny>,
                ) -> ::pyo3::PyResult<Self> {
                    let py = obj.py();
                    let expected_type = #object_import;
                    if !::pyo3::types::PyAnyMethods::is_instance(
                        obj.as_any(),
                        expected_type.as_any(),
                    )? {
                        return ::std::result::Result::Err(
                            ::pyo3::exceptions::PyTypeError::new_err(
                                ::std::format!("expected Python instance of '{}'", #object_name),
                            ),
                        );
                    }
                    ::std::result::Result::Ok(Self { obj: obj.unbind() })
                }

                #shared_wrapper_boilerplate

                #[inline]
                pub fn into_py_any(self) -> ::pyo3::Py<::pyo3::types::PyAny> {
                    self.obj
                }
            }

            #[automatically_derived]
            impl<'py> ::pyo3::FromPyObject<'_, 'py> for #struct_ident {
                type Error = ::pyo3::PyErr;

                fn extract(
                    obj: ::pyo3::Borrowed<'_, 'py, ::pyo3::types::PyAny>,
                ) -> ::std::result::Result<Self, Self::Error> {
                    Self::from_py_any(obj.to_owned())
                }
            }

            #[automatically_derived]
            impl<'py> ::pyo3::IntoPyObject<'py> for #struct_ident {
                type Target = ::pyo3::types::PyAny;
                type Output = ::pyo3::Bound<'py, Self::Target>;
                type Error = ::std::convert::Infallible;

                fn into_pyobject(
                    self,
                    py: ::pyo3::marker::Python<'py>,
                ) -> ::std::result::Result<Self::Output, Self::Error> {
                    ::std::result::Result::Ok(self.obj.into_bound(py))
                }
            }

            #[automatically_derived]
            impl<'a, 'py> ::pyo3::IntoPyObject<'py> for &'a #struct_ident {
                type Target = ::pyo3::types::PyAny;
                type Output = ::pyo3::Borrowed<'a, 'py, Self::Target>;
                type Error = ::std::convert::Infallible;

                fn into_pyobject(
                    self,
                    py: ::pyo3::marker::Python<'py>,
                ) -> ::std::result::Result<Self::Output, Self::Error> {
                    ::std::result::Result::Ok(self.obj.bind_borrowed(py))
                }
            }
        });

        // Generate this class's own members (methods + properties). Ordered ancestor
        // validation below uses the same lowering and retains its forwarder signatures.
        let GeneratedMembers {
            struct_impl,
            method_defs,
            method_impls,
            forwarders: member_forwarders,
            resolved_members: member_resolved_members,
        } = self.generate_members(cfg, local_types, diagnostics, import_mode)?;

        // Add the implementation block for the struct
        output.extend(quote::quote! {
            #[automatically_derived]
            impl #struct_ident {
                #struct_impl
            }
        });

        // Add the trait and implementation block for bounded struct
        let trait_ident: syn::Ident =
            Ident::from_py(&format!("{struct_ident}Methods")).try_into()?;
        let struct_ident_str = struct_ident.to_string();

        if import_mode.uses_dedup_model() {
            let validated = self.validate_forwarder_ident_collisions(
                local_types,
                enum_types,
                all_classes,
                cfg,
                import_mode,
                inherited_members,
            )?;

            // External-IR path: emit a supertrait hierarchy so a descendant newtype
            // inherits its ancestors' methods/properties. This type's OWN members are
            // carried as default-bodied methods (emitted by `Function::generate` /
            // `Property::generate` in this mode), reaching the Python handle
            // generically via the synthetic root trait's `__pyo3_obj()` accessor.
            // Every ancestor (the full transitive chain) plus the root receive an
            // impl satisfying its supertrait bound; those impls are UNCONDITIONALLY
            // empty (`method_impls` is empty in this mode).
            //
            // Ambiguity between same-named members declared on more than one in-scope
            // `*Methods` trait (E0034) is NOT resolved at the trait layer. Instead the
            // newtype carries an INHERENT forwarder per callable name (built below): an
            // inherent method takes resolution precedence over every same-named trait
            // method for a concrete receiver, so a call binds to exactly one item no
            // matter which ancestor traits the caller imports. Each forwarder delegates
            // through UFCS (`<Self as SomeMethods>::name(...)`) and never names
            // `__pyo3_obj`, so it stays sound in a concrete impl.

            // Module-relative path to the synthetic universal base trait.
            let root_trait_path: syn::Path = {
                let module_path = self.name.parent().unwrap_or_default();
                let relative = module_path.relative_to(&Path::from_rs(ROOT_TRAIT_NAME), false);
                (&relative).try_into()?
            };
            // Module-relative path to the generic type-marker trait.
            let marker_trait_path: syn::Path = {
                let module_path = self.name.parent().unwrap_or_default();
                let relative = module_path.relative_to(&Path::from_rs(PY_TYPE_MARKER_NAME), false);
                (&relative).try_into()?
            };

            // Every effective direct base contributes its `Methods` interface. On the
            // static path, traversal may omit an authored direct base while retaining an
            // in-scope grandparent. Promote the minimal emitted ancestor frontier so
            // `T: DerivedMethods` still implies every reachable emitted base trait.
            let static_effective_bases = import_mode.is_static().then(|| {
                let candidates = self
                    .base_classes
                    .iter()
                    .filter(|base| all_classes.contains_key(*base))
                    .collect::<Vec<_>>();
                candidates
                    .iter()
                    .copied()
                    .filter(|base| {
                        !candidates.iter().copied().any(|nearer| {
                            nearer != *base
                                && all_classes
                                    .get(nearer)
                                    .is_some_and(|class| class.base_classes.contains(*base))
                        })
                    })
                    .collect::<Vec<_>>()
            });
            let direct_supertrait_bases = static_effective_bases.unwrap_or_else(|| {
                self.effective_direct_base_classes
                    .as_deref()
                    .unwrap_or(&self.direct_base_classes)
                    .iter()
                    .collect()
            });
            if import_mode.is_static() {
                for authored_base in &self.direct_base_classes {
                    if !all_classes.contains_key(authored_base) {
                        crate::report::warn(
                            diagnostics,
                            "base_class_out_of_scope",
                            self.name.to_py(),
                            format!(
                                "Base class '{authored_base}' of '{}' is outside the emitted \
                                 traversal scope; promoting its nearest emitted ancestors.",
                                self.name
                            ),
                        );
                    }
                }
            }
            let direct_supertraits = direct_supertrait_bases
                .iter()
                .filter_map(|direct_base| {
                    let resolved = methods_trait_path(direct_base, local_types, enum_types);
                    if resolved.is_none() {
                        crate::report::warn(
                            diagnostics,
                            "base_class_out_of_scope",
                            self.name.to_py(),
                            format!(
                                "Base class '{direct_base}' of '{}' is not a resolvable class (out of scope or not a class); dropping it from the supertrait list.",
                                self.name
                            ),
                        );
                    }
                    resolved
                })
                .collect::<Vec<_>>();
            let supertrait = if direct_supertraits.is_empty() {
                quote::quote! { #root_trait_path }
            } else {
                quote::quote! { #(#direct_supertraits)+* }
            };

            // Per-ancestor impls for the full transitive chain, satisfying each
            // ancestor `*Methods` supertrait bound. These are unconditionally empty:
            // every callable name is served by the inherent forwarders below, so no
            // trait method is overridden here.
            let ancestor_impls: proc_macro2::TokenStream = self
                .base_classes
                .iter()
                .filter_map(|base| methods_trait_path(base, local_types, enum_types))
                .map(|trait_path| {
                    quote::quote! {
                        #[automatically_derived]
                        impl #trait_path for #struct_ident {}
                    }
                })
                .collect();

            // Inherent forwarders. Emitting one `pub fn` per callable name on the
            // newtype makes that name resolve to a single inherent item regardless of
            // how many ancestor `*Methods` traits are in scope at the call site. Gated
            // by `generate_inherent_forwarders` (default on): when disabled, the whole
            // inherent `impl` block is omitted and members are reached only through the
            // `*Methods` traits -- a large size reduction for wide hierarchies. Leaving
            // the block (empty or not) unchanged when enabled keeps the default output
            // byte-identical.
            let inherent_forwarder_impl = if cfg.generate_inherent_forwarders {
                let inherent_forwarders = self.inherent_forwarders(
                    &struct_ident,
                    &trait_ident,
                    &member_forwarders,
                    &member_resolved_members,
                    local_types,
                    enum_types,
                    inherited_members,
                    &validated,
                    diagnostics,
                )?;
                quote::quote! {
                    #[automatically_derived]
                    impl #struct_ident {
                        #inherent_forwarders
                    }
                }
            } else {
                // Inherent forwarders suppressed (`--no-inherent-forwarders` mode):
                // the whole inherent `impl` block is omitted, so a name inherited from two
                // or more ancestors has no inherent method to disambiguate it and stays
                // ambiguous (E0034) for a consumer importing more than one ancestor
                // `*Methods` trait. Run a detection-only pass that warns per such name.
                // Byte-neutral by construction: it emits no tokens (the branch's value is
                // the empty stream below), only appends diagnostics.
                self.detect_ambiguous_without_forwarders(inherited_members, diagnostics);
                proc_macro2::TokenStream::new()
            };

            output.extend(quote::quote! {
                /// These methods are defined on the owning Python object wrapper.
                #[doc(alias = #struct_ident_str)]
                #[automatically_derived]
                pub trait #trait_ident: #supertrait {
                    #method_defs
                }
                #[automatically_derived]
                impl #root_trait_path for #struct_ident {
                    #[inline]
                    fn __pyo3_obj(&self) -> &::pyo3::Py<::pyo3::types::PyAny> {
                        &self.obj
                    }
                }
                #[automatically_derived]
                impl #trait_ident for #struct_ident {}
                #ancestor_impls
                #inherent_forwarder_impl
            });

            // Stage A: generic type marker for this newtype. Records the registered
            // Python class name and the unchecked wrap/unwrap of the underlying
            // handle (sound because the newtype is `#[repr(transparent)]`).
            output.extend(quote::quote! {
                #[automatically_derived]
                impl #marker_trait_path for #struct_ident {
                    const PY_CLASS: &'static str = #object_name;
                    #[inline]
                    fn __from_obj(obj: ::pyo3::Py<::pyo3::types::PyAny>) -> Self {
                        Self { obj }
                    }
                    #[inline]
                    fn __into_obj(self) -> ::pyo3::Py<::pyo3::types::PyAny> {
                        self.obj
                    }
                }
            });

            // CR3: transitive zero-cost upcasts to EVERY ancestor in the full
            // closure (resolved via `local_types`, the same source of truth as the
            // CR2 supertrait chain). `AsRef` borrows across the cast; `From`
            // consumes and re-wraps via the marker trait.
            let upcast_impls: proc_macro2::TokenStream = self
                .base_classes
                .iter()
                .filter_map(|base| newtype_path(base, local_types, enum_types))
                .map(|ancestor_path| {
                    quote::quote! {
                        // SAFETY: both `#struct_ident` and the ancestor newtype are
                        // `#[repr(transparent)]` wrappers around the same
                        // `Py<PyAny>`, hence identical in layout; reinterpreting a
                        // reference to one as a reference to the other is sound.
                        #[automatically_derived]
                        impl ::core::convert::AsRef<#ancestor_path> for #struct_ident {
                            #[inline]
                            fn as_ref(&self) -> &#ancestor_path {
                                unsafe { &*(self as *const Self as *const #ancestor_path) }
                            }
                        }
                        #[automatically_derived]
                        impl ::core::convert::From<#struct_ident> for #ancestor_path {
                            #[inline]
                            fn from(value: #struct_ident) -> Self {
                                <#ancestor_path as #marker_trait_path>::__from_obj(
                                    <#struct_ident as #marker_trait_path>::__into_obj(value),
                                )
                            }
                        }
                    }
                })
                .collect();
            output.extend(upcast_impls);
        } else {
            output.extend(quote::quote! {
                /// These methods are defined on the owning Python object wrapper.
                #[doc(alias = #struct_ident_str)]
                #[automatically_derived]
                pub trait #trait_ident {
                    #method_defs
                }
                #[automatically_derived]
                impl #trait_ident for #struct_ident {
                    #method_impls
                }
            });
        }

        Ok(output)
    }
}

#[cfg(test)]
mod forwarder_planning_tests {
    use super::*;
    use crate::{
        ClassIr, FunctionIr, FunctionKindIr, MethodKindIr, ModuleIr, ParameterIr, ParameterKindIr,
        SchemaIr,
    };

    fn fixture(method_count: usize) -> ModuleIr {
        let classes = ["A", "B", "C"]
            .into_iter()
            .map(|name| {
                let qualified = format!("forwarder_work.{name}");
                ClassIr {
                    name: qualified.clone(),
                    base_classes: if name == "C" {
                        vec!["forwarder_work.A".into(), "forwarder_work.B".into()]
                    } else {
                        vec![]
                    },
                    docstring: None,
                    methods: if name == "C" {
                        vec![]
                    } else {
                        (0..method_count)
                            .map(|index| FunctionIr {
                                name: format!("{qualified}.member_{index}"),
                                kind: FunctionKindIr::Method {
                                    method_type: MethodKindIr::Instance,
                                },
                                params: vec![ParameterIr {
                                    name: "self".into(),
                                    kind: ParameterKindIr::PositionalOrKeyword,
                                    annotation: String::new(),
                                    has_default: false,
                                }],
                                return_type: if index % 2 == 0 { "int" } else { "str" }.into(),
                                docstring: None,
                            })
                            .collect()
                    },
                    properties: vec![],
                }
            })
            .collect();
        ModuleIr {
            name: "forwarder_work".into(),
            docstring: None,
            is_package: false,
            classes,
            enums: vec![],
            functions: vec![],
            properties: vec![],
            submodules: vec![],
            prelude: vec![],
            imports: vec![],
            type_vars: vec![],
        }
    }

    #[test]
    fn additional_collisions_do_not_regenerate_winner_members() {
        for method_count in [1, 2, 8] {
            for enabled in [true, false] {
                let codegen = crate::Codegen::new(
                    Config::builder()
                        .generate_inherent_forwarders(enabled)
                        .build(),
                )
                .modules_from_ir(SchemaIr {
                    modules: vec![fixture(method_count)],
                })
                .unwrap();
                MEMBER_GENERATION_CALLS.with(|calls| calls.set(0));
                let generated = codegen.generate_with_report().unwrap();
                assert_eq!(MEMBER_GENERATION_CALLS.with(std::cell::Cell::get), 5,
                    "three own-member generations plus two ordered ancestor validations; {method_count} collisions, forwarders={enabled}");
                assert_eq!(generated.report.diagnostics.iter().filter(|diagnostic| diagnostic.code == "class_ambiguous_without_forwarders").count(),
                    if enabled { 0 } else { method_count });
            }
        }
    }

    #[test]
    fn inherited_plan_preserves_reference_scan_order_and_overrides() {
        let mut ir = fixture(3);
        // An own declaration suppresses the inherited name without reordering others.
        let overridden = ir.classes[0].methods[1].clone();
        ir.classes[2].methods.push(FunctionIr {
            name: "forwarder_work.C.member_1".into(),
            ..overridden
        });
        let module = super::super::Module::from_ir(&Config::default(), &ir).unwrap();
        let all_classes = module
            .classes
            .iter()
            .map(|class| (class.name.clone(), class))
            .collect();
        let class = &module.classes[2];
        let plan = InheritedMemberPlan::build(class, &all_classes);
        assert_eq!(
            plan.collisions,
            vec![
                InheritedMemberCollision {
                    name: "member_0".into(),
                    winner: Path::from_py("forwarder_work.A"),
                    declarer_count: 2
                },
                InheritedMemberCollision {
                    name: "member_2".into(),
                    winner: Path::from_py("forwarder_work.A"),
                    declarer_count: 2
                },
            ]
        );

        // Reference implementation of the removed production scan. Include duplicate
        // and missing raw ancestor entries to pin its private-input multiplicity rule.
        let reference = |class: &Class| {
            let mut names = Vec::new();
            let mut declarers: rustc_hash::FxHashMap<String, Vec<Path>> =
                rustc_hash::FxHashMap::default();
            for base in &class.base_classes {
                let Some(ancestor) = all_classes.get(base) else {
                    continue;
                };
                for name in ancestor
                    .methods
                    .iter()
                    .map(|method| method.name.name().as_py())
                    .chain(
                        ancestor
                            .properties
                            .iter()
                            .map(|property| property.name.name().as_py()),
                    )
                {
                    declarers
                        .entry(name.to_owned())
                        .or_insert_with(|| {
                            names.push(name.to_owned());
                            Vec::new()
                        })
                        .push(base.clone());
                }
            }
            names
                .into_iter()
                .filter_map(|name| {
                    let bases = &declarers[&name];
                    let owns = class
                        .methods
                        .iter()
                        .any(|method| method.name.name().as_py() == name)
                        || class
                            .properties
                            .iter()
                            .any(|property| property.name.name().as_py() == name);
                    (bases.len() >= 2 && !owns).then(|| InheritedMemberCollision {
                        name,
                        winner: bases[0].clone(),
                        declarer_count: bases.len(),
                    })
                })
                .collect::<Vec<_>>()
        };
        for bases in [
            vec!["A", "B"],
            vec!["B", "A"],
            vec!["A", "Missing", "B", "A"],
            vec!["A"],
        ] {
            let mut class = class.clone();
            class.base_classes = bases
                .iter()
                .map(|name| Path::from_py(&format!("forwarder_work.{name}")))
                .collect();
            assert_eq!(
                InheritedMemberPlan::build(&class, &all_classes).collisions,
                reference(&class)
            );
        }
    }

    #[test]
    fn validated_signatures_match_legacy_winner_replay() {
        let cfg = Config::default();
        let mut ir = fixture(3);
        let mut unselected = ir.classes[0].methods[0].clone();
        unselected.name = "forwarder_work.A.unique_member".into();
        ir.classes[0].methods.push(unselected);
        let module = super::super::Module::from_ir(&cfg, &ir).unwrap();
        let all_classes = module
            .classes
            .iter()
            .map(|class| (class.name.clone(), class))
            .collect();
        let class = &module.classes[2];
        let plan = InheritedMemberPlan::build(class, &all_classes);
        let global = std::rc::Rc::new(
            module
                .classes
                .iter()
                .map(|class| {
                    (
                        class.name.clone(),
                        crate::planning::LocalType::try_from_path(class.name.clone(), false)
                            .unwrap(),
                    )
                })
                .collect(),
        );
        let local_types =
            super::super::LocalTypes::from_maps(std::rc::Rc::clone(&global), global, None);
        let mode = ImportMode::Static([module.name.clone()].into_iter().collect());
        let enum_types = rustc_hash::FxHashSet::default();
        let mut validated = class
            .validate_forwarder_ident_collisions(
                &local_types,
                &enum_types,
                &all_classes,
                &cfg,
                &mode,
                &plan,
            )
            .unwrap();
        assert_eq!(
            validated.members.len(),
            1,
            "only the selected winner needs retained signatures"
        );
        assert_eq!(
            validated.members[&Path::from_py("forwarder_work.A")].len(),
            3,
            "a winner's unrelated signatures must not be retained"
        );
        let render = |members: &[(String, Vec<super::super::Forwarder>)]| {
            members
                .iter()
                .map(|(name, forwarders)| {
                    (
                        name.clone(),
                        forwarders
                            .iter()
                            .map(|forwarder| {
                                let super::super::Forwarder {
                                    ident,
                                    params,
                                    args,
                                    ret,
                                } = forwarder;
                                quote::quote! { #ident (#params) (#args) -> #ret }.to_string()
                            })
                            .collect::<Vec<_>>(),
                    )
                })
                .collect::<Vec<_>>()
        };
        for winner in plan.winners.keys() {
            let ancestor = all_classes[winner];
            let scope = local_types.scoped_to(ancestor.name.parent().unwrap_or_default());
            let mut legacy = ancestor
                .generate_members(&cfg, &scope, &mut Vec::new(), &mode)
                .unwrap();
            legacy.forwarders.retain(|(name, _)| {
                plan.collisions
                    .iter()
                    .any(|collision| &collision.winner == winner && &collision.name == name)
            });
            assert_eq!(
                render(&validated.members[winner]),
                render(&legacy.forwarders)
            );
        }

        validated.members.clear();
        let error = class
            .inherent_forwarders(
                &quote::format_ident!("C"),
                &quote::format_ident!("CMethods"),
                &[],
                &[],
                &local_types,
                &enum_types,
                &plan,
                &validated,
                &mut Vec::new(),
            )
            .expect_err("a lost validated winner must not silently erase forwarders");
        assert!(
            error.to_string().contains(
                "no validated signatures for winner 'forwarder_work.A' of class 'forwarder_work.C'"
            ),
            "{error}"
        );
    }
}
