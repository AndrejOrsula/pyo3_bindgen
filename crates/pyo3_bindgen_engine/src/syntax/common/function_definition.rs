pub(crate) enum FunctionImplementation {
    Function(proc_macro2::TokenStream),
    Method(TraitMethod),
}

impl FunctionImplementation {
    pub(crate) fn empty_function() -> Self {
        Self::Function(proc_macro2::TokenStream::new())
    }

    pub(crate) fn empty_method() -> Self {
        Self::Method(TraitMethod::empty())
    }
}

pub(crate) struct TraitMethod {
    pub(crate) trait_fn: proc_macro2::TokenStream,
    pub(crate) impl_fn: proc_macro2::TokenStream,
    /// External-IR path only: the data needed to emit an inherent forwarder for
    /// this member on the owning newtype. Empty on the live path and for members
    /// that produce no callable trait method. See [`Forwarder`] and the inherent
    /// forwarder emission in `Class::generate`.
    pub(crate) forwarders: Vec<Forwarder>,
}

impl TraitMethod {
    pub(crate) fn empty() -> Self {
        Self {
            trait_fn: proc_macro2::TokenStream::new(),
            impl_fn: proc_macro2::TokenStream::new(),
            forwarders: Vec::new(),
        }
    }
}

/// One inherent `pub fn` to emit on a generated newtype, delegating to a trait
/// method of the same name through UFCS.
///
/// An inherent method takes resolution precedence over every in-scope trait
/// method of the same name for a concrete receiver, so emitting exactly one of
/// these per callable name makes a call unambiguous no matter how many `*Methods`
/// traits the caller imports. The body is pure `<Self as SomeMethods>::ident(...)`
/// and never names the generic root accessor, so it is sound in a concrete impl.
pub(crate) struct Forwarder {
    /// The Rust method identifier, identical to the trait method it forwards to.
    pub(crate) ident: syn::Ident,
    /// The parameter list after `&self, py`, e.g. `p_a: T, p_b: U` (may be empty).
    pub(crate) params: proc_macro2::TokenStream,
    /// The forwarded argument list, e.g. `p_a, p_b` (may be empty).
    pub(crate) args: proc_macro2::TokenStream,
    /// The return type inside `::pyo3::PyResult<...>`.
    pub(crate) ret: proc_macro2::TokenStream,
}
