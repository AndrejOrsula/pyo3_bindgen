use super::Path;
use crate::Result;

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize)]
pub(crate) struct Import {
    pub(crate) origin: Path,
    pub(crate) target: Path,
    pub(crate) import_type: ImportType,
}

impl Import {
    pub(crate) fn new(origin: Path, target: Path) -> Self {
        let import_type = ImportType::from_paths(&origin, &target);
        Self {
            origin,
            target,
            import_type,
        }
    }

    pub(crate) fn is_external(&self) -> bool {
        self.import_type == ImportType::ExternalImport
    }

    pub(crate) fn is_generated_reexport(&self) -> bool {
        self.import_type != ImportType::ExternalImport && self.origin != self.target
    }

    pub(crate) fn is_nonidentity(&self) -> bool {
        self.origin != self.target
    }

    pub(crate) fn generate(
        &self,
        owner_rust_path: &Path,
        emitted_origin: &Path,
        alias: &syn::Ident,
    ) -> Result<proc_macro2::TokenStream> {
        // Skip identity imports
        if self.origin == self.target {
            return Ok(proc_macro2::TokenStream::new());
        }

        // Generate the path to the target module
        let relative_path: std::result::Result<syn::Path, _> =
            owner_rust_path.relative_to(emitted_origin, true).try_into();
        if let Ok(relative_path) = relative_path {
            // Use alias for the target module if it has a different name than the last segment of its path
            let emitted_name: syn::Ident = emitted_origin.name().try_into()?;
            let maybe_alias = if emitted_name == *alias {
                proc_macro2::TokenStream::new()
            } else {
                quote::quote! { as #alias }
            };

            Ok(quote::quote! {
                pub use #relative_path #maybe_alias;
            })
        } else {
            Ok(proc_macro2::TokenStream::new())
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
pub(crate) enum ImportType {
    ExternalImport,
    PackageReexport,
    SubmoduleReexport,
}

impl ImportType {
    fn from_paths(origin: &Path, target: &Path) -> Self {
        let is_package_reexport = target
            .root()
            .is_some_and(|root_module| origin.starts_with(&root_module));
        let is_submodule_reexport = is_package_reexport
            && target
                .parent()
                .is_some_and(|parent_module| origin.starts_with(&parent_module));
        match (is_package_reexport, is_submodule_reexport) {
            (false, false) => Self::ExternalImport,
            (true, false) => Self::PackageReexport,
            (true, true) => Self::SubmoduleReexport,
            _ => unreachable!(),
        }
    }
}
