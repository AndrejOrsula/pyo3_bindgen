//! Input adapters for the binding-generation pipeline.
//!
//! The source seam owns only acquisition. Registration, validation, lowering,
//! canonicalization, and emission remain behind [`crate::Codegen`], so adapters do not
//! depend on the orchestrator or its private domain model.

use crate::{PyBindgenResult, SchemaIr};
use serde::{Deserialize, Serialize};

/// Exact transport facts for a bounded external-IR acquisition.
///
/// Materialized [`SchemaIr`] callers do not necessarily have byte-addressed transport facts and
/// continue to use [`crate::Codegen::modules_from_ir`]. The CLI uses this descriptor so the exact
/// retained bytes, acquisition limit, and adapter provenance cross the engine boundary together.
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ExternalIrSourceContext {
    sha256: String,
    content_bytes: usize,
    byte_limit: usize,
    provenance: String,
}

impl ExternalIrSourceContext {
    /// Validate transport facts before they can become normalized source identity.
    pub fn bounded(
        sha256: impl Into<String>,
        content_bytes: usize,
        byte_limit: usize,
        provenance: impl Into<String>,
    ) -> PyBindgenResult<Self> {
        let sha256 = sha256.into();
        let provenance = provenance.into();
        let digest = sha256.strip_prefix("sha256:").unwrap_or_default();
        if digest.len() != 64
            || !digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(crate::PyBindgenError::CodegenError(
                "external-IR source identity must be a lowercase sha256:<64 hex digits> value"
                    .to_owned(),
            ));
        }
        if content_bytes > byte_limit {
            return Err(crate::PyBindgenError::CodegenError(format!(
                "external-IR source contains {content_bytes} bytes, exceeding its {byte_limit}-byte acquisition limit"
            )));
        }
        if provenance.is_empty() {
            return Err(crate::PyBindgenError::CodegenError(
                "external-IR source provenance must not be empty".to_owned(),
            ));
        }
        Ok(Self {
            sha256,
            content_bytes,
            byte_limit,
            provenance,
        })
    }

    pub(crate) fn sha256(&self) -> &str {
        &self.sha256
    }

    pub(crate) const fn content_bytes(&self) -> usize {
        self.content_bytes
    }

    pub(crate) const fn byte_limit(&self) -> usize {
        self.byte_limit
    }

    pub(crate) fn provenance(&self) -> &str {
        &self.provenance
    }
}

/// A source that can acquire one input for the binding-generation pipeline.
///
/// Implementations consume themselves because source adapters may own files, network
/// responses, interpreter handles, or other one-shot acquisition state. [`BindingSource::load`]
/// is a synchronous, whole-document seam: it returns a complete, materialized
/// [`SchemaSource`], not a stream or transport handle. The result carries no transport receipt
/// such as raw byte count, acquisition limit, digest, or retained raw bytes. An adapter that
/// needs those facts must bound and materialize its transport outside this trait before returning
/// the source.
///
/// The engine remains solely responsible for applying source-mode invariants and lowering the
/// materialized source into its private model.
///
/// The engine ships two adapters at this seam: [`LiveModules`] and [`SchemaIr`]. Custom
/// adapters can acquire data through another transport, but must complete acquisition and
/// materialization before returning and do so without receiving a [`crate::Codegen`]. Their
/// acquisition, parsing, trust, resource, and side-effect behavior is outside the engine's
/// built-in external-IR execution-free guarantee.
pub trait BindingSource {
    /// Acquire and classify one complete, materialized input for the engine.
    ///
    /// No transport receipt or raw-input provenance crosses this method. Adapters that need
    /// those facts must enforce their own transport bound and retain the required provenance
    /// before returning the [`SchemaSource`]. The built-in [`SchemaIr`] adapter is execution-free;
    /// a custom adapter may perform arbitrary acquisition before this method returns.
    fn load(self) -> PyBindgenResult<SchemaSource>;
}

/// Live-introspection source adapter backed by Python module-name descriptors.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct LiveModules {
    names: Vec<String>,
}

impl LiveModules {
    /// Create a live source from Python module names, preserving their order.
    #[must_use]
    pub fn new<I, S>(names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            names: names.into_iter().map(Into::into).collect(),
        }
    }

    /// Return the configured Python module names in registration order.
    #[must_use]
    pub fn names(&self) -> &[String] {
        &self.names
    }
}

impl BindingSource for LiveModules {
    fn load(self) -> PyBindgenResult<SchemaSource> {
        Ok(SchemaSource::Live(self.names))
    }
}

impl BindingSource for SchemaIr {
    fn load(self) -> PyBindgenResult<SchemaSource> {
        Ok(SchemaSource::Ir(self))
    }
}

/// Compatibility discriminator for callers that choose a source mode at runtime.
///
/// Generic callers should prefer [`BindingSource`] so custom acquisition adapters can
/// participate without extending this enum.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SchemaSource {
    /// Live source descriptors; `Codegen` resolves and imports the named modules during
    /// registration.
    Live(Vec<String>),
    /// A pre-built, materialized external schema.
    ///
    /// Transport bytes, acquisition limits, and raw transport provenance are not represented by
    /// this discriminator.
    Ir(SchemaIr),
}

impl BindingSource for SchemaSource {
    fn load(self) -> PyBindgenResult<SchemaSource> {
        Ok(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_external_context_rejects_unproven_transport_facts() {
        assert!(ExternalIrSourceContext::bounded("fnv1a64:deadbeef", 1, 1, "test").is_err());
        assert!(ExternalIrSourceContext::bounded(
            format!("sha256:{}", "A".repeat(64)),
            1,
            1,
            "test"
        )
        .is_err());
        assert!(ExternalIrSourceContext::bounded(
            format!("sha256:{}", "a".repeat(64)),
            2,
            1,
            "test"
        )
        .is_err());
        assert!(
            ExternalIrSourceContext::bounded(format!("sha256:{}", "a".repeat(64)), 1, 1, "")
                .is_err()
        );
    }
}
