use crate::{Config, Result};

pub trait Generate {
    fn generate(&self, cfg: &Config) -> Result<proc_macro2::TokenStream>;
}

pub trait Canonicalize: Sized {
    fn canonicalize(&mut self);
}
