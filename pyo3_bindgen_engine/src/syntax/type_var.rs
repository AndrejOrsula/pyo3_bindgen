use super::Path;
use crate::{traits::Generate, Config, Result};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TypeVar {
    pub name: Path,
}

impl TypeVar {
    pub fn new(name: Path) -> Result<Self> {
        Ok(Self { name })
    }
}

impl Generate for TypeVar {
    fn generate(&self, _cfg: &Config) -> Result<proc_macro2::TokenStream> {
        todo!()
    }
}
