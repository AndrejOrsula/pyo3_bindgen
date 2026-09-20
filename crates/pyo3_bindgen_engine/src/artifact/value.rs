//! Pure construction, formatting, and content access for generated artifacts.
//!
//! This module performs no filesystem or publication operations. The public artifact
//! types remain defined in the parent module so their existing identities and compatibility
//! re-exports do not change. Filesystem checks, replacement, and durability adapters stay in the
//! parent module.

use super::{RustArtifact, TextArtifact};
use crate::{stable_hash, PyBindgenResult};
use proc_macro2::TokenStream;

impl TextArtifact {
    /// Construct an artifact from UTF-8 text.
    pub fn new(contents: impl Into<String>) -> Self {
        Self {
            contents: contents.into(),
        }
    }

    /// Return the artifact contents.
    pub fn contents(&self) -> &str {
        &self.contents
    }

    /// Return the artifact contents as bytes.
    pub fn bytes(&self) -> &[u8] {
        self.contents.as_bytes()
    }

    /// Return the artifact size in bytes.
    pub fn len(&self) -> usize {
        self.contents.len()
    }

    /// Return whether the artifact is empty.
    pub fn is_empty(&self) -> bool {
        self.contents.is_empty()
    }

    /// Return the stable content hash used by generation reports.
    pub fn hash(&self) -> String {
        stable_hash(self.bytes())
    }
}

impl AsRef<str> for TextArtifact {
    fn as_ref(&self) -> &str {
        self.contents()
    }
}

impl RustArtifact {
    /// Parse and canonically format generated Rust tokens.
    ///
    /// Deliberately single-threaded, and it must stay that way. Sharding the crate across
    /// threads was measured and rejected on three independent grounds:
    /// 1. Real targets carry 4-10 top-level items with essentially all mass inside one
    ///    `pub mod`, so a depth-0 split is byte-identical but buys no parallelism at all.
    /// 2. `prettyplease`'s line breaking is depth-sensitive (margin 89, indent 4), so
    ///    formatting a *nested* item standalone and re-indenting it is **not**
    ///    byte-identical -- any signature whose rendered width lands in the band
    ///    `(89 - 4 * depth, 89]` wraps differently in place than it does in isolation.
    /// 3. `proc_macro2::TokenStream` and `syn`'s AST types are `!Send`, so every shard
    ///    boundary would additionally cost a full `String` round-trip.
    pub fn from_tokens(tokens: TokenStream) -> PyBindgenResult<Self> {
        let file = syn::parse2(tokens)?;
        Ok(Self {
            text: TextArtifact::new(prettyplease::unparse(&file)),
        })
    }

    /// Return the formatted Rust source.
    pub fn contents(&self) -> &str {
        self.text.contents()
    }

    /// Return the formatted Rust source as bytes.
    pub fn bytes(&self) -> &[u8] {
        self.text.bytes()
    }

    /// Return the formatted source size in bytes.
    pub fn len(&self) -> usize {
        self.text.len()
    }

    /// Return whether the formatted source is empty.
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Return the stable hash of the formatted source.
    pub fn hash(&self) -> String {
        self.text.hash()
    }
}

impl AsRef<str> for RustArtifact {
    fn as_ref(&self) -> &str {
        self.contents()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_value_operations_are_filesystem_free() {
        let artifact = TextArtifact::new("héllo");

        assert_eq!(artifact.contents(), "héllo");
        assert_eq!(artifact.bytes(), "héllo".as_bytes());
        assert_eq!(artifact.len(), "héllo".len());
        assert!(!artifact.is_empty());
        assert_eq!(artifact.hash(), stable_hash(artifact.bytes()));
        assert_eq!(AsRef::<str>::as_ref(&artifact), "héllo");
    }

    #[test]
    fn rust_value_operations_preserve_canonical_format_and_hash() {
        let artifact = RustArtifact::from_tokens(quote::quote! {
            pub fn value() -> i32 { 1 }
        })
        .expect("valid Rust tokens must format");

        assert_eq!(artifact.contents(), "pub fn value() -> i32 {\n    1\n}\n");
        assert_eq!(artifact.bytes(), artifact.contents().as_bytes());
        assert_eq!(artifact.len(), artifact.contents().len());
        assert!(!artifact.is_empty());
        assert_eq!(artifact.hash(), stable_hash(artifact.bytes()));
        assert_eq!(AsRef::<str>::as_ref(&artifact), artifact.contents());
    }

    #[test]
    fn rust_value_construction_keeps_parse_errors() {
        let error = RustArtifact::from_tokens(quote::quote! { pub fn broken() -> })
            .expect_err("malformed tokens must remain a value-construction error");

        assert!(matches!(error, crate::PyBindgenError::SynError(_)));
    }

    #[test]
    fn empty_values_keep_empty_bytes_and_the_stable_empty_hash() {
        let text = TextArtifact::new("");
        let rust = RustArtifact::from_tokens(TokenStream::new()).unwrap();
        for (contents, bytes, length, empty, hash) in [
            (
                text.contents(),
                text.bytes(),
                text.len(),
                text.is_empty(),
                text.hash(),
            ),
            (
                rust.contents(),
                rust.bytes(),
                rust.len(),
                rust.is_empty(),
                rust.hash(),
            ),
        ] {
            assert_eq!(contents, "");
            assert!(bytes.is_empty());
            assert_eq!(length, 0);
            assert!(empty);
            assert_eq!(hash, stable_hash(b""));
        }
    }
}
