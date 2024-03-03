use super::Path;
use crate::{Config, Result};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Import {
    pub origin: Path,
    pub target: Path,
    pub import_type: ImportType,
}

impl Import {
    pub fn new(origin: Path, target: Path) -> Result<Self> {
        let import_type = ImportType::from_paths(&origin, &target);
        Ok(Self {
            origin,
            target,
            import_type,
        })
    }

    pub fn is_external(&self) -> bool {
        self.import_type == ImportType::ExternalImport
    }

    pub fn generate(&self, cfg: &Config) -> Result<proc_macro2::TokenStream> {
        // Skip external imports if their generation is disabled
        if !cfg.generate_dependencies && self.import_type == ImportType::ExternalImport {
            return Ok(proc_macro2::TokenStream::new());
        }

        // Skip identity imports
        if self.origin == self.target {
            return Ok(proc_macro2::TokenStream::new());
        }

        // Determine the visibility of the import based on its type
        let visibility = match self.import_type {
            ImportType::ExternalImport => proc_macro2::TokenStream::new(),
            ImportType::Reexport | ImportType::ScopedReexport => quote::quote! { pub },
        };

        // Generate the path to the target module
        let relative_path: std::result::Result<syn::Path, _> = self
            .target
            .parent()
            .unwrap_or_default()
            .relative_to(&self.origin)
            .try_into();
        if let Ok(relative_path) = relative_path {
            // Use alias for the target module if it has a different name than the last segment of its path
            let maybe_alias = if self.origin.name() != self.target.name() {
                let alias: syn::Ident = self.target.name().try_into()?;
                quote::quote! { as #alias }
            } else {
                proc_macro2::TokenStream::new()
            };

            Ok(quote::quote! {
                #visibility use #relative_path #maybe_alias;
            })
        } else {
            Ok(proc_macro2::TokenStream::new())
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ImportType {
    ExternalImport,
    Reexport,
    ScopedReexport,
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
            (true, false) => Self::Reexport,
            (true, true) => Self::ScopedReexport,
            _ => unreachable!(),
        }
    }
}
