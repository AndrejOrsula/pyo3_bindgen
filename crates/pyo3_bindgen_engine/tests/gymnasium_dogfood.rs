use std::{fs, path::Path, process::Command};

#[path = "../test_support/bounded_consumer.rs"]
mod bounded_consumer;

fn gymnasium_available() -> bool {
    pyo3::Python::initialize();
    pyo3::Python::attach(|py| py.import("gymnasium.envs.registration").is_ok())
}

fn formatted_gymnasium_registration_bindings() -> pyo3_bindgen_engine::GeneratedBindings {
    pyo3_bindgen_engine::Codegen::default()
        .module_name("gymnasium.envs.registration")
        .unwrap()
        .generate_with_report()
        .unwrap()
}

fn formatted_tokens(tokens: proc_macro2::TokenStream) -> String {
    prettyplease::unparse(&syn::parse2(tokens).unwrap())
}

fn write_temp_crate(root: &Path, bindings: &str) {
    let pyo3_version = env!("PYO3_BINDGEN_PYO3_VERSION");
    fs::write(
        root.join("Cargo.toml"),
        format!(
            r#"[package]
name = "pyo3_bindgen_gymnasium_dogfood"
version = "0.0.0"
edition = "2021"

[dependencies]
pyo3 = {{ version = "={pyo3_version}", default-features = false, features = ["auto-initialize"] }}
"#
        ),
    )
    .unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("bindings.rs"), bindings).unwrap();
    fs::write(
        root.join("src/lib.rs"),
        r#"include!(concat!(env!("CARGO_MANIFEST_DIR"), "/bindings.rs"));

#[cfg(test)]
mod tests {
    use pyo3::types::PyAnyMethods;

    #[test]
    fn generated_gymnasium_registration_bindings_run() -> pyo3::PyResult<()> {
        pyo3::Python::attach(|py| {
            let mode = crate::gymnasium::envs::registration::VectorizeMode::Sync.into_py_any(py)?;
            let mode_name: String = mode.getattr("name")?.extract()?;
            assert_eq!(mode_name, "SYNC");

            let env = crate::gymnasium::envs::registration::make(
                py,
                "CartPole-v1",
                None,
                Some(true),
                None,
            )?;
            let action_space = env.getattr("action_space")?;
            let _sample = action_space.call_method0("sample")?;
            Ok(())
        })
    }
}
"#,
    )
    .unwrap();
}

/// Name of the env var the coordinator sets in CI to enforce that gymnasium is present.
/// When set (to any value), a missing gymnasium package causes the test to **fail**
/// rather than skip.  In local development the variable is unset, so the test skips
/// gracefully.
const REQUIRE_GYMNASIUM_ENV: &str = "PYO3_BINDGEN_REQUIRE_GYMNASIUM";

#[test]
fn dogfoods_gymnasium_registration_bindings_compile_and_run() {
    if !gymnasium_available() {
        eprintln!(
            "SKIPPED: gymnasium not installed — \
             set {REQUIRE_GYMNASIUM_ENV}=1 in CI to turn this into a hard failure"
        );
        if std::env::var(REQUIRE_GYMNASIUM_ENV).is_ok() {
            panic!(
                "{REQUIRE_GYMNASIUM_ENV} is set but the Python `gymnasium` package is not \
                 importable; install it (e.g. `pip install gymnasium`) before running this test \
                 suite"
            );
        }
        return;
    }

    let generated = formatted_gymnasium_registration_bindings();
    let bindings = formatted_tokens(generated.tokens.clone());
    let regenerated = formatted_gymnasium_registration_bindings();
    let regenerated_bindings = formatted_tokens(regenerated.tokens);

    assert_eq!(bindings, regenerated_bindings);
    assert!(bindings.contains("pub enum VectorizeMode"));
    assert!(bindings.contains("pub fn make"));
    assert!(bindings.contains("pub fn make_vec"));

    let registration_report = generated
        .report
        .modules
        .iter()
        .find(|module| module.name == "gymnasium.envs.registration")
        .unwrap();
    assert!(registration_report
        .enums
        .iter()
        .any(|py_enum| py_enum.name == "gymnasium.envs.registration.VectorizeMode"));
    assert!(registration_report.counts.classes >= 2);
    assert!(registration_report.counts.functions >= 2);
    assert!(generated.report.outputs[0].hash.starts_with("fnv1a64:"));

    let mut crate_guard =
        bounded_consumer::OwnedTempDir::allocate("pyo3_bindgen_gymnasium_registration_dogfood")
            .unwrap_or_else(|error| {
                panic!("temporary directory must be allocated exclusively: {error}")
            });
    let crate_root = crate_guard.path().to_owned();
    write_temp_crate(&crate_root, &bindings);
    let mut command = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    command.arg("test").arg("-q").current_dir(&crate_root);
    let output = match crate_guard.run(&mut command, bounded_consumer::CARGO_COMMAND_TIMEOUT) {
        Ok(output) => output,
        Err(error) => {
            panic!(
                "generated Gymnasium dogfood crate could not complete: {error}\nstdout:\n{}\nstderr:\n{}",
                error.stdout(),
                error.stderr(),
            );
        }
    };
    assert!(
        output.status.success(),
        "generated Gymnasium dogfood crate failed\nstatus: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        output.stdout,
        output.stderr
    );
}

#[test]
fn bounded_consumer_timeout_reaps_direct_child_and_retains_directory() {
    bounded_consumer::assert_timeout_regression(
        "bounded_consumer_timeout_reaps_direct_child_and_retains_directory",
    );
}
