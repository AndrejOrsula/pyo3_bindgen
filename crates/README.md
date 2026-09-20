# Workspace packages

This directory contains every publishable Cargo package in the workspace.
Runnable examples stay in the top-level [`examples/`](../examples/) package and
are intentionally excluded from default workspace commands and publication.

| Package | Role | Internal dependencies |
|---|---|---|
| [`pyo3_bindgen_engine`](pyo3_bindgen_engine/) | Source adapters, introspection, external-IR validation/lowering, Rust generation, reports, tracing events, and artifacts | None |
| [`pyo3_bindgen_macros`](pyo3_bindgen_macros/) | Procedural macro entry point | `pyo3_bindgen_engine` |
| [`pyo3_bindgen`](pyo3_bindgen/) | Public library facade | `pyo3_bindgen_engine`, optional `pyo3_bindgen_macros` |
| [`pyo3_bindgen_cli`](pyo3_bindgen_cli/) | Reusable command workflow plus the thin `pyo3_bindgen` binary | `pyo3_bindgen` |

Shared versions, metadata, and dependency declarations belong in the root
[`Cargo.toml`](../Cargo.toml). Package-specific targets, features, and platform
dependencies stay in each package manifest.
