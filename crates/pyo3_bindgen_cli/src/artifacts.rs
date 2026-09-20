//! Test-only adapter for the engine-owned publication path planner.
//!
//! The implementation deliberately lives in the engine. Production callers use opaque
//! publication plan/session descriptors; these test-only re-exports preserve focused path-policy
//! coverage without creating a second identity implementation.

#[cfg(test)]
pub(crate) use pyo3_bindgen::{
    artifact_identity_key, artifact_identity_path, artifact_revalidation_identity_matches,
    conservative_casefold_identity_key, validate_windows_artifact_component, ArtifactPath,
    ArtifactPathKind, IrArtifactOpenError,
};

#[cfg(all(test, windows))]
pub(crate) use pyo3_bindgen::{
    validate_windows_artifact_path, windows_file_id_identity,
    windows_legacy_file_identity_is_sound, windows_lexically_normalized_absolute_path,
};
