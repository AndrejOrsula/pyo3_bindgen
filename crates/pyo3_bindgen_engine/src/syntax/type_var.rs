use super::{Ident, Path};
use crate::{report::DomainDiagnostic, Config, Result};

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize)]
pub(crate) struct TypeVar {
    pub(crate) name: Path,
}

impl TypeVar {
    pub(crate) fn new(name: Path) -> Self {
        Self { name }
    }

    /// Construct a [`TypeVar`] from a generic IR name string with zero `pyo3` usage.
    pub(crate) fn from_ir(name: &str) -> Self {
        Self::new(Path::from_py(name))
    }

    pub(crate) fn generate(
        &self,
        _cfg: &Config,
        diagnostics: &mut Vec<DomainDiagnostic>,
    ) -> Result<proc_macro2::TokenStream> {
        let typevar_ident: syn::Ident = {
            let name = self.name.name();
            if let Some(ident) = super::naming::rust_ident(name, super::naming::SymbolKind::TypeVar)
            {
                if TryInto::<syn::Ident>::try_into(name).is_ok() {
                    ident
                } else {
                    // Sanitize the type variable name
                    let new_name = Ident::from_rs(&ident.to_string());
                    crate::report::warn(
                        diagnostics,
                        "type_var_invalid_ident_renamed",
                        self.name.to_py(),
                        format!(
                            "Type variable '{}' is an invalid Rust ident for a type alias name. Renamed to '{}'.",
                            self.name,
                            self.name.parent().unwrap_or_default().join(&new_name.into())
                        ),
                    );
                    ident
                }
            } else {
                crate::report::warn(
                        diagnostics,
                        "type_var_invalid_ident_skipped",
                        self.name.to_py(),
                        format!(
                            "Type variable '{}' is an invalid Rust ident for a type alias name. Renaming failed. Bindings will not be generated.",
                            self.name
                        ),
                    );
                return Ok(proc_macro2::TokenStream::new());
            }
        };
        Ok(quote::quote! {
            pub type #typevar_ident = ::pyo3::types::PyAny;
        })
    }
}
