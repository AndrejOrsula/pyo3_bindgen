#![cfg(feature = "macros")]

use std::path::Path;

#[path = "test_support/bounded_consumer.rs"]
#[allow(dead_code)]
mod bounded_consumer;

#[test]
fn facade_proc_macro_pass_consumer_compiles() {
    let Some(root) = bounded_consumer::workspace_root("facade_proc_macro_pass_consumer_compiles")
    else {
        return;
    };
    let mut consumer = bounded_consumer::ConsumerWorkspace::new("macro_pass");
    write_consumer_manifest(consumer.path(), &root, "macro_pass_consumer");
    copy_fixture(consumer.path(), "macro_pass.rs");

    let output = bounded_consumer::cargo_check(&mut consumer).unwrap_or_else(|error| {
        panic!(
            "macro pass consumer cargo check did not finish cleanly: {error}\nstdout:\n{}\nstderr:\n{}",
            error.stdout(),
            error.stderr()
        )
    });
    assert!(
        output.status.success(),
        "public import_python! pass consumer should compile\nstatus: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        output.stdout,
        output.stderr,
    );
}

#[test]
fn facade_proc_macro_rejects_invalid_input_deterministically() {
    let Some(root) = bounded_consumer::workspace_root(
        "facade_proc_macro_rejects_invalid_input_deterministically",
    ) else {
        return;
    };
    let mut consumer = bounded_consumer::ConsumerWorkspace::new("macro_compile_fail");
    write_consumer_manifest(consumer.path(), &root, "macro_compile_fail_consumer");
    copy_fixture(consumer.path(), "macro_compile_fail.rs");

    let output = bounded_consumer::cargo_check(&mut consumer).unwrap_or_else(|error| {
        panic!(
            "macro compile-fail consumer cargo check did not finish cleanly: {error}\nstdout:\n{}\nstderr:\n{}",
            error.stdout(),
            error.stderr()
        )
    });
    assert!(
        !output.status.success(),
        "invalid import_python! input should fail to compile\nstdout:\n{}\nstderr:\n{}",
        output.stdout,
        output.stderr,
    );
    let diagnostics = format!("{}\n{}", output.stdout, output.stderr);
    assert!(
        diagnostics.contains("expected string literal"),
        "invalid import_python! input should retain the stable parser diagnostic:\n{diagnostics}"
    );
}

fn write_consumer_manifest(path: &Path, root: &Path, name: &str) {
    let facade_path = root.join("crates/pyo3_bindgen");
    let manifest = format!(
        "[package]\nname = \"{name}\"\nversion = \"0.0.0\"\nedition = \"2021\"\npublish = false\n\n[workspace]\n\n[dependencies]\npyo3 = {{ version = \"=0.29.2\", default-features = false, features = [\"py-clone\"] }}\npyo3_bindgen = {{ path = {:?}, features = [\"macros\"] }}\n",
        facade_path.to_string_lossy().to_string(),
    );
    std::fs::write(path.join("Cargo.toml"), manifest).expect("macro consumer manifest writable");
}

fn copy_fixture(path: &Path, fixture: &str) {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(fixture);
    std::fs::copy(source, path.join("src/lib.rs")).expect("macro fixture should be copied");
}
