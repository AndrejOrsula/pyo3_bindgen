pub enum FunctionImplementation {
    Function(proc_macro2::TokenStream),
    Method(TraitMethod),
}

impl FunctionImplementation {
    pub fn empty_function() -> Self {
        Self::Function(proc_macro2::TokenStream::new())
    }

    pub fn empty_method() -> Self {
        Self::Method(TraitMethod::empty())
    }
}

pub struct TraitMethod {
    pub trait_fn: proc_macro2::TokenStream,
    pub impl_fn: proc_macro2::TokenStream,
}

impl TraitMethod {
    pub fn empty() -> Self {
        Self {
            trait_fn: proc_macro2::TokenStream::new(),
            impl_fn: proc_macro2::TokenStream::new(),
        }
    }
}
