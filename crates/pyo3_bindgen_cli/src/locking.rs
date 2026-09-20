//! Test-only adapter for the engine-owned ordered artifact locks.

#[cfg(test)]
pub(crate) use pyo3_bindgen::{
    artifact_lock_keys, artifact_lock_path, open_artifact_lock, LOCK_OWNER_COUNTER,
};
