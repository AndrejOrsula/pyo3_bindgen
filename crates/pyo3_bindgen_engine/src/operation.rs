//! Deterministic operation correlation identity.

use crate::{normalized::NormalizedGraph, Config, Result};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct OperationId(String);

impl OperationId {
    pub(crate) fn for_graph(graph: &NormalizedGraph, config: &Config) -> Result<Self> {
        #[derive(serde::Serialize)]
        struct Seed<'a> {
            algorithm: &'static str,
            generator_version: &'static str,
            source_kind: &'static str,
            roots: Vec<String>,
            input_hashes: Vec<(String, String)>,
            transport_sha256: Option<&'a str>,
            transport_bytes: Option<usize>,
            transport_byte_limit: Option<usize>,
            provenance: &'a [String],
            config: Config,
        }

        let mut config = config.clone();
        config.enum_raw_fallback_paths.sort();
        config.enum_raw_fallback_paths.dedup();
        config.blocklist_names.sort();
        config.blocklist_names.dedup();
        let source = graph.source();
        let seed = Seed {
            algorithm: "pyo3_bindgen.operation.v1",
            generator_version: env!("CARGO_PKG_VERSION"),
            source_kind: source.kind().as_str(),
            roots: graph.operation_roots(),
            input_hashes: graph.operation_input_hashes(),
            transport_sha256: source.transport_sha256(),
            transport_bytes: source.transport_bytes(),
            transport_byte_limit: source.transport_byte_limit(),
            provenance: source.provenance(),
            config,
        };
        let bytes = serde_json::to_vec(&seed).map_err(|error| {
            crate::PyBindgenError::CodegenError(format!(
                "failed to derive operation correlation identity: {error}"
            ))
        })?;
        Ok(Self(format!("op-v1:{}", crate::stable_hash(bytes))))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn correlation_prefix_is_versioned() {
        let value = format!("op-v1:{}", crate::stable_hash(b"seed"));
        assert!(value.starts_with("op-v1:fnv1a64:"));
        assert_eq!(value.len(), "op-v1:fnv1a64:".len() + 16);
    }
}
