mod build_support;

fn main() {
    // Expose #[cfg] flags of pyo3
    pyo3_build_config::use_pyo3_cfgs();

    // Expose the resolved PyO3 requirement for generation reports. Cargo rewrites
    // workspace dependencies into table-form dependencies in packaged manifests,
    // so inspect the package manifest first and the workspace root as a fallback.
    // A shared target directory can reuse this build-script binary across checkouts.
    // Resolve the manifest root when the binary runs instead of embedding the checkout
    // that originally compiled it, and invalidate cached output when that root changes.
    println!("cargo:rerun-if-env-changed=CARGO_MANIFEST_DIR");
    let manifest_dir = std::env::var_os("CARGO_MANIFEST_DIR")
        .map(std::path::PathBuf::from)
        .expect("Cargo must provide CARGO_MANIFEST_DIR to build scripts");
    for directory in manifest_dir.ancestors() {
        let manifest_path = directory.join("Cargo.toml");
        println!("cargo:rerun-if-changed={}", manifest_path.display());
        let Ok(manifest) = std::fs::read_to_string(&manifest_path) else {
            continue;
        };
        if let Some(version) = build_support::parse_pyo3_version(&manifest) {
            println!("cargo:rustc-env=PYO3_BINDGEN_PYO3_VERSION={version}");
            break;
        }
    }
}
