use super::Path;
use crate::{traits::Generate, Config, Result};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Import {
    pub origin: Path,
    pub target: Path,
    import_type: ImportType,
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
}

impl Generate for Import {
    fn generate(&self, _cfg: &Config) -> Result<proc_macro2::TokenStream> {
        todo!()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ImportType {
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
