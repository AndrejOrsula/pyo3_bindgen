use super::Path;
use crate::{Config, Result};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TypeVar {
    pub name: Path,
}

impl TypeVar {
    pub fn new(name: Path) -> Self {
        Self { name }
    }

    pub fn generate(&self, _cfg: &Config) -> Result<proc_macro2::TokenStream> {
        let typevar_ident: syn::Ident = self.name.name().try_into()?;
        Ok(quote::quote! {
            pub type #typevar_ident = ::pyo3::types::PyAny;
        })
    }
}
