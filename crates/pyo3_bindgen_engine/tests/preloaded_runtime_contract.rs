use pyo3::types::PyAnyMethods;
use pyo3_bindgen_engine::Codegen;
use std::{process::Command, time::Duration};

#[path = "../test_support/bounded_consumer.rs"]
mod bounded_consumer;

const CHILD_ENV: &str = "PYO3_BINDGEN_PRELOADED_RUNTIME_CHILD";
const CHILD_ROLE: &str = "pyo3-bindgen-preloaded-runtime-child-v1";
const CHILD_SUCCESS_MARKER: &str = "PYO3_BINDGEN_PRELOADED_RUNTIME_CHILD_SUCCESS";
const CHILD_TIMEOUT: Duration = Duration::from_secs(120);

#[test]
fn preloaded_module_contract_is_enforced_in_a_fresh_process() {
    let mut working_directory =
        bounded_consumer::OwnedTempDir::allocate("preloaded-runtime-contract")
            .expect("preloaded runtime contract directory must be allocated exclusively");
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--ignored",
            "--exact",
            "preloaded_module_contract_child",
            "--nocapture",
        ])
        .env(CHILD_ENV, CHILD_ROLE);
    let output = match working_directory.run(&mut command, CHILD_TIMEOUT) {
        Ok(output) => output,
        Err(error) => panic!(
            "fresh-process contract probe failed before completion: {error}\n\
             working directory: {}\n\
             timeout: {}\n\
             termination_succeeded: {}\n\
             stdout report: {}\n\
             stderr report: {}\n\
             stdout:\n{}\n\
             stderr:\n{}",
            working_directory.path().display(),
            error.is_timeout(),
            error.termination_succeeded(),
            error.stdout_path().display(),
            error.stderr_path().display(),
            error.stdout(),
            error.stderr(),
        ),
    };
    let stdout = &output.stdout;
    let success_markers = stdout
        .split_whitespace()
        .filter(|token| *token == CHILD_SUCCESS_MARKER)
        .count();
    assert!(
        output.status.success()
            && success_markers == 1,
        "fresh-process contract probe failed:\nstatus: {}\nworking directory: {}\nstdout report: {}\nstderr report: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        working_directory.path().display(),
        output.stdout_path.display(),
        output.stderr_path.display(),
        stdout,
        output.stderr,
    );
}

#[test]
#[ignore = "internal child; invoked by preloaded_module_contract_is_enforced_in_a_fresh_process"]
fn preloaded_module_contract_child() {
    let role = std::env::var(CHILD_ENV)
        .unwrap_or_else(|_| panic!("internal child requires {CHILD_ENV}={CHILD_ROLE}"));
    assert_eq!(
        role, CHILD_ROLE,
        "internal child received an unexpected {CHILD_ENV} value"
    );

    pyo3::Python::initialize();
    pyo3::Python::attach(|py| {
        let unprimed = pyo3::types::PyModule::new(py, "unprimed_target").unwrap();
        let error = match Codegen::default().module(&unprimed) {
            Ok(_) => panic!("unprimed preloaded module was accepted"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("prime_live_runtime"));

        let inspect = py.import("inspect").unwrap();
        let original_signature = inspect.getattr("signature").unwrap();
        inspect
            .setattr("signature", pyo3::types::PyDict::new(py))
            .unwrap();
        let error = Codegen::prime_live_runtime(py)
            .expect_err("a replaced stdlib anchor must not become the live baseline");
        assert!(
            error.to_string().contains("inspect.signature"),
            "unexpected priming error: {error}",
        );
        inspect.setattr("signature", original_signature).unwrap();

        Codegen::prime_live_runtime(py).unwrap();
        let primed = pyo3::types::PyModule::new(py, "primed_target").unwrap();
        Codegen::default()
            .module(&primed)
            .expect("explicitly primed preloaded module must be accepted");
    });
    println!("{CHILD_SUCCESS_MARKER}");
}
