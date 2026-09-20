#[path = "../build_support.rs"]
mod build_support;

#[test]
fn build_script_resolves_manifest_directory_at_execution_time() {
    let build_script =
        std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("build.rs"))
            .expect("engine build script should exist");

    assert!(
        build_script.contains("cargo:rerun-if-env-changed=CARGO_MANIFEST_DIR"),
        "shared target directories must invalidate build output when the checkout changes"
    );
    assert!(
        build_script.contains(r#"std::env::var_os("CARGO_MANIFEST_DIR")"#),
        "the build script must resolve its checkout when it executes"
    );
    assert!(
        !build_script.contains(r#"env!("CARGO_MANIFEST_DIR")"#),
        "compile-time checkout paths poison reused build-script binaries"
    );
}

#[test]
fn pyo3_version_parser_supports_workspace_and_packaged_manifests() {
    let workspace = r#"pyo3 = { version = "=0.29.0", default-features = false }"#;
    let packaged = r#"
        [dependencies.pyo3]
        version = "=0.29.0"
        default-features = false
    "#;

    assert_eq!(build_support::parse_pyo3_version(workspace), Some("0.29.0"));
    assert_eq!(build_support::parse_pyo3_version(packaged), Some("0.29.0"));
}

#[test]
fn pyo3_version_parser_rejects_non_exact_requirements() {
    for requirement in ["0.29", "0.29.0", "^0.29.0", "~0.29.0", ">=0.29.0"] {
        let manifest = format!(r#"pyo3 = {{ version = "{requirement}" }}"#);
        assert_eq!(
            build_support::parse_pyo3_version(&manifest),
            None,
            "{requirement} is not an exact resolved version"
        );
    }
}

#[test]
fn compiled_pyo3_version_matches_resolved_manifest() {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let expected = manifest_dir.ancestors().find_map(|directory| {
        let path = directory.join("Cargo.toml");
        let manifest = std::fs::read_to_string(path).ok()?;
        build_support::parse_pyo3_version(&manifest).map(str::to_owned)
    });

    assert_eq!(
        Some(env!("PYO3_BINDGEN_PYO3_VERSION")),
        expected.as_deref(),
        "compiled report provenance must match the package or workspace manifest"
    );
}
