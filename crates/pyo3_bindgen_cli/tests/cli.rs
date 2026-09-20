mod test_cli {
    use assert_cmd::cargo::cargo_bin_cmd;
    use predicates::prelude::*;
    use std::{
        fs,
        process::Stdio,
        thread,
        time::{Duration, Instant, SystemTime, UNIX_EPOCH},
    };

    const BIN_NAME: &str = "pyo3_bindgen";

    fn cli_command() -> assert_cmd::Command {
        cargo_bin_cmd!("pyo3_bindgen")
    }

    fn expected_file_publication_exit_code() -> i32 {
        if cfg!(unix) {
            0
        } else {
            3
        }
    }

    fn expected_failure_report_exit_code() -> i32 {
        if cfg!(unix) {
            1
        } else {
            3
        }
    }

    fn temp_dir(test_name: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "pyo3_bindgen_cli_{test_name}_{}_{}",
            std::process::id(),
            nanos
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn canonical_display_path(path: &std::path::Path) -> String {
        path.canonicalize().unwrap().display().to_string()
    }

    fn wait_for_path(path: &std::path::Path) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !path.exists() {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {}",
                path.display()
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn write_slow_python_module(python_dir: &std::path::Path) {
        fs::create_dir_all(python_dir).unwrap();
        fs::write(
            python_dir.join("slow_bindgen_module.py"),
            r#"
import os
import time

with open(os.environ["PYO3_BINDGEN_TEST_MARKER"], "w", encoding="utf-8") as marker:
    marker.write("importing")

time.sleep(float(os.environ.get("PYO3_BINDGEN_TEST_DELAY", "0")))

def ready():
    return True
"#,
        )
        .unwrap();
    }

    #[test]
    fn test_cli_help() {
        // Arrange
        let mut cmd = cli_command();

        // Act
        let assert = cmd.arg("-h").assert();

        // Assert
        assert.success().stdout(
            predicate::str::contains(format!("Usage: {BIN_NAME}"))
                .and(predicate::str::contains("Options:"))
                .and(predicate::str::contains("--module-name <MODULE_NAMES>"))
                .and(predicate::str::contains("--output <OUTPUT>"))
                .and(predicate::str::contains("--output-dir <OUTPUT_DIR>"))
                .and(predicate::str::contains("--report <REPORT>"))
                .and(predicate::str::contains("--check"))
                .and(predicate::str::contains("--include-private"))
                .and(predicate::str::contains("--no-traverse-submodules"))
                .and(predicate::str::contains("--strict-completeness"))
                .and(predicate::str::contains(
                    "--blocklist-name <BLOCKLIST_NAMES>",
                )),
        );
    }

    #[test]
    fn test_cli_default() {
        // Arrange
        let mut cmd = cli_command();

        // Act
        let assert = cmd.assert();

        // Assert
        assert.code(2).stderr(
            predicate::str::contains("ERROR[invalid_arguments]")
                .and(predicate::str::contains(
                    "error: the following required arguments",
                ))
                .and(predicate::str::contains("--module-name <MODULE_NAMES>"))
                .and(predicate::str::contains(format!("Usage: {BIN_NAME}"))),
        );
    }

    #[test]
    fn test_cli_invalid_check_delivers_safe_failure_report() {
        let temp_dir = temp_dir("invalid_check_safe_report");
        let report_path = temp_dir.join("failure.json");

        let mut cmd = cli_command();
        cmd.args(["--module-name", "math", "--check", "--report"])
            .arg(&report_path)
            .assert()
            .code(expected_failure_report_exit_code())
            .stdout(predicate::str::is_empty())
            .stderr(predicate::str::contains("ERROR[invalid_arguments]"));

        let report: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&report_path).unwrap()).unwrap();
        assert_eq!(report["schema_version"], 1);
        assert_eq!(report["outputs"], serde_json::json!([]));
        assert_eq!(report["diagnostics"][0]["code"], "invalid_arguments");
    }

    #[test]
    fn test_cli_bindgen_os() {
        // Arrange
        let mut cmd = cli_command();

        // Act
        let assert = cmd.arg("-m").arg("os").assert();

        // Assert: success and stdout contains expected generated item
        assert
            .success()
            .stdout(predicate::str::contains("pub mod os"));
    }

    #[test]
    fn test_cli_bindgen_sys() {
        // Arrange
        let mut cmd = cli_command();

        // Act
        let assert = cmd.arg("-m").arg("sys").assert();

        // Assert: success and stdout contains expected generated item
        assert
            .success()
            .stdout(predicate::str::contains("pub mod sys"));
    }

    #[test]
    fn rust_log_exposes_structured_generation_lifecycle() {
        let directory = temp_dir("structured_observability");
        let output = directory.join("bindings.rs");
        let report = directory.join("report.json");
        let ir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/minimal_ir.json");

        let assert = cli_command()
            .env("RUST_LOG", "pyo3_bindgen_cli=info,pyo3_bindgen_engine=info")
            // Non-terminal stderr is never colored. Remove ambient color controls so this test
            // proves terminal detection itself owns the default policy.
            .env_remove("NO_COLOR")
            .env_remove("FORCE_COLOR")
            .arg("--ir")
            .arg(ir)
            .arg("--output")
            .arg(output)
            .arg("--report")
            .arg(&report)
            .assert()
            .code(expected_file_publication_exit_code());
        let stderr = String::from_utf8_lossy(&assert.get_output().stderr);
        for required in [
            "pyo3_bindgen.cli.run",
            "source=\"external_ir\"",
            "operation_id=\"op-v1:fnv1a64:",
            "pyo3_bindgen.source.acquire",
            "generation completed",
            "pyo3_bindgen.publication.output",
            "artifact published",
            "output publication completed",
            "durability=",
            "hash=\"fnv1a64:",
            "time.busy=",
        ] {
            assert!(stderr.contains(required), "missing {required:?}: {stderr}");
        }
        assert!(!stderr.contains("\u{1b}["), "non-TTY stderr was colored");

        let report: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(report).unwrap()).unwrap();
        let operation_id = report["operation_id"].as_str().unwrap();
        let completion = stderr
            .lines()
            .find(|line| {
                line.contains(if cfg!(unix) {
                    "CLI run completed"
                } else {
                    "CLI run failed"
                })
            })
            .expect("CLI completion event should be emitted");
        assert!(
            completion.contains(operation_id),
            "CLI completion and report must share one operation ID: {completion}"
        );
        let expected_operation_code = if cfg!(unix) {
            "ok"
        } else {
            "output_durability_failed"
        };
        assert!(
            completion.contains(&format!("operation_code=\"{expected_operation_code}\"")),
            "CLI completion must carry a stable operation code: {completion}"
        );
    }

    #[test]
    fn test_cli_file_report_and_check_mode() {
        // Arrange
        let temp_dir = temp_dir("file_report_and_check_mode");
        let output = temp_dir.join("bindings.rs");
        let report = temp_dir.join("report.json");
        fs::write(&output, "old bindings\n").unwrap();
        fs::write(&report, "old report\n").unwrap();

        // Act
        let mut generate_cmd = cli_command();
        let generate_assert = generate_cmd
            .args(["-m", "sys", "--output"])
            .arg(&output)
            .args(["--report"])
            .arg(&report)
            .assert();

        // Assert
        generate_assert.code(expected_file_publication_exit_code());
        let output_bytes = fs::read(&output).unwrap();
        assert!(String::from_utf8_lossy(&output_bytes).contains("pub mod sys"));
        let report_json: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&report).unwrap()).unwrap();
        assert_eq!(report_json["schema_version"], 1);
        assert_eq!(report_json["outputs"][0]["kind"], "file");
        assert_eq!(
            report_json["outputs"][0]["path"],
            canonical_display_path(&output)
        );
        assert_eq!(report_json["outputs"][0]["bytes"], output_bytes.len());
        assert_eq!(
            report_json["outputs"][0]["hash"],
            pyo3_bindgen::stable_hash(&output_bytes)
        );

        let check_report = temp_dir.join("check_report.json");
        let mut check_cmd = cli_command();
        check_cmd
            .args(["-m", "sys", "--output"])
            .arg(&output)
            .arg("--check")
            .arg("--report")
            .arg(&check_report)
            .assert()
            .code(expected_file_publication_exit_code());
        let check_report_json: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&check_report).unwrap()).unwrap();
        assert_eq!(check_report_json["outputs"][0]["kind"], "file");
        assert_eq!(
            check_report_json["outputs"][0]["path"],
            canonical_display_path(&output)
        );
        assert_eq!(check_report_json["outputs"][0]["bytes"], output_bytes.len());
        assert_eq!(
            check_report_json["outputs"][0]["hash"],
            pyo3_bindgen::stable_hash(&output_bytes)
        );

        fs::write(&output, "stale\n").unwrap();
        let drift_report = temp_dir.join("drift_report.json");
        let mut drift_cmd = cli_command();
        drift_cmd
            .args(["-m", "sys", "--output"])
            .arg(&output)
            .arg("--check")
            .arg("--report")
            .arg(&drift_report)
            .assert()
            .failure()
            .stderr(predicate::str::contains("generated output drift detected"));
        let drift_report_json: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&drift_report).unwrap()).unwrap();
        assert!(drift_report_json["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|diagnostic| diagnostic["code"] == "output_drift"));
        assert_eq!(drift_report_json["outputs"], serde_json::json!([]));
    }

    #[cfg(unix)]
    #[test]
    fn test_cli_report_path_uses_canonical_symlinked_temp_root() {
        use std::os::unix::fs::symlink;

        let temp_dir = temp_dir("canonical_report_path_symlinked_root");
        let real_root = temp_dir.join("real-root");
        let alias_root = temp_dir.join("alias-root");
        fs::create_dir(&real_root).unwrap();
        symlink(&real_root, &alias_root).unwrap();

        let output = alias_root.join("bindings.rs");
        let report = alias_root.join("report.json");
        cli_command()
            .args(["-m", "sys", "--output"])
            .arg(&output)
            .args(["--report"])
            .arg(&report)
            .assert()
            .code(expected_file_publication_exit_code());

        let report_json: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&report).unwrap()).unwrap();
        let reported_path = report_json["outputs"][0]["path"].as_str().unwrap();
        assert_eq!(reported_path, canonical_display_path(&output));
        assert_ne!(reported_path, output.display().to_string());
    }

    #[test]
    fn test_cli_imported_enum_object_values_are_reproducible_across_processes() {
        let temp_dir = temp_dir("imported_enum_object_values_are_reproducible");
        let python_dir = temp_dir.join("python");
        fs::create_dir_all(&python_dir).unwrap();
        fs::write(
            python_dir.join("enum_object_values.py"),
            r#"
from enum import Enum

class Token:
    pass

class Choice(Enum):
    PRIMARY = Token()
    ALIAS = PRIMARY
"#,
        )
        .unwrap();

        let generate = |suffix: &str| {
            let output = temp_dir.join(format!("bindings-{suffix}.rs"));
            let report = temp_dir.join(format!("report-{suffix}.json"));
            let mut cmd = cli_command();
            cmd.env("PYTHONPATH", &python_dir)
                .args(["-m", "enum_object_values", "--output"])
                .arg(&output)
                .args(["--report"])
                .arg(&report)
                .assert()
                .code(expected_file_publication_exit_code());

            let report_json: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(report).unwrap()).unwrap();
            let module = report_json["modules"]
                .as_array()
                .unwrap()
                .iter()
                .find(|module| module["name"] == "enum_object_values")
                .unwrap();
            let enumeration = module["enums"]
                .as_array()
                .unwrap()
                .iter()
                .find(|enumeration| enumeration["name"] == "enum_object_values.Choice")
                .unwrap();
            (
                fs::read(output).unwrap(),
                module["input_hash"].as_str().unwrap().to_owned(),
                enumeration["variants"][0]["value_repr"]
                    .as_str()
                    .unwrap()
                    .to_owned(),
                enumeration["aliases"][0]["value_repr"]
                    .as_str()
                    .unwrap()
                    .to_owned(),
            )
        };

        let first = generate("first");
        let second = generate("second");

        assert_eq!(first.0, second.0);
        assert_eq!(first.1, second.1);
        assert_eq!(first.2, "<enum_object_values.Token instance>");
        assert_eq!(first.3, first.2);
        let bindings = String::from_utf8(first.0).unwrap();
        assert!(bindings.contains("<enum_object_values.Token instance>"));
        assert!(!bindings.contains("Token object at 0x"));
    }

    #[test]
    fn test_cli_check_reports_non_utf8_output_as_unreadable() {
        let temp_dir = temp_dir("check_non_utf8_output");
        let ir_path = temp_dir.join("schema.json");
        let output = temp_dir.join("bindings.rs");
        let report = temp_dir.join("report.json");
        fs::write(&ir_path, minimal_ir_json()).unwrap();
        fs::write(&output, [0xff]).unwrap();

        let mut cmd = cli_command();
        cmd.args(["--ir"])
            .arg(&ir_path)
            .args(["--output"])
            .arg(&output)
            .args(["--check", "--report"])
            .arg(&report)
            .assert()
            .failure()
            .stderr(predicate::str::contains(
                "failed to read existing generated output",
            ));

        let report_json: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&report).unwrap()).unwrap();
        assert!(report_json["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|diagnostic| diagnostic["code"] == "output_missing"));
        assert_eq!(report_json["outputs"], serde_json::json!([]));
    }

    /// The `--ir` failure paths run before any interpreter exists. Building their
    /// failure report must therefore never reach Python, or the process aborts
    /// instead of writing the report the caller asked for.
    #[test]
    fn test_cli_writes_failure_report_for_unparsable_ir_without_python() {
        let temp_dir = temp_dir("ir_parse_failed_report");
        let ir_path = temp_dir.join("schema.json");
        let output = temp_dir.join("bindings.rs");
        let report = temp_dir.join("report.json");
        fs::write(&ir_path, "{ not valid json").unwrap();

        let mut cmd = cli_command();
        cmd.args(["--ir"])
            .arg(&ir_path)
            .args(["--output"])
            .arg(&output)
            .arg("--report")
            .arg(&report)
            .assert()
            .failure()
            .stderr(predicate::str::contains("Failed to parse IR file"));

        let report_json: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&report).unwrap()).unwrap();
        assert!(report_json["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|diagnostic| diagnostic["code"] == "ir_parse_failed"));
        assert_eq!(report_json["python"], serde_json::Value::Null);
    }

    /// Same guarantee for an IR file that cannot even be read.
    #[test]
    fn test_cli_writes_failure_report_for_unreadable_ir_without_python() {
        let temp_dir = temp_dir("ir_read_failed_report");
        let ir_path = temp_dir.join("does-not-exist.json");
        let output = temp_dir.join("bindings.rs");
        let report = temp_dir.join("report.json");

        let mut cmd = cli_command();
        cmd.args(["--ir"])
            .arg(&ir_path)
            .args(["--output"])
            .arg(&output)
            .arg("--report")
            .arg(&report)
            .assert()
            .failure()
            .stderr(predicate::str::contains("Failed to read IR file"));

        let report_json: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&report).unwrap()).unwrap();
        assert!(report_json["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|diagnostic| diagnostic["code"] == "ir_read_failed"));
        assert_eq!(report_json["python"], serde_json::Value::Null);
    }

    #[test]
    fn test_cli_rejects_direct_ir_artifact_aliases_without_clobbering_input() {
        let temp_dir = temp_dir("rejects_direct_ir_artifact_aliases");
        let original = minimal_ir_json();

        for (case, write_argument) in [("output", "--output"), ("report", "--report")] {
            let ir_path = temp_dir.join(format!("{case}.json"));
            fs::write(&ir_path, &original).unwrap();

            let mut cmd = cli_command();
            cmd.arg("--ir")
                .arg(&ir_path)
                .arg(write_argument)
                .arg(&ir_path)
                .assert()
                .failure()
                .stderr(
                    predicate::str::contains("--ir")
                        .and(predicate::str::contains("must refer to different files")),
                );

            assert_eq!(
                fs::read_to_string(&ir_path).unwrap(),
                original,
                "{write_argument} must not replace its IR input"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn test_cli_rejects_symlinked_ir_artifact_alias_without_clobbering_input() {
        use std::os::unix::fs::symlink;

        let temp_dir = temp_dir("rejects_symlinked_ir_artifact_alias");
        let ir_path = temp_dir.join("schema.json");
        let ir_alias = temp_dir.join("schema-alias.json");
        let original = minimal_ir_json();
        fs::write(&ir_path, &original).unwrap();
        symlink(&ir_path, &ir_alias).unwrap();

        let mut cmd = cli_command();
        cmd.arg("--ir")
            .arg(&ir_alias)
            .arg("--output")
            .arg(&ir_path)
            .assert()
            .failure()
            .stderr(
                predicate::str::contains("--ir")
                    .and(predicate::str::contains("must refer to different files")),
            );

        assert_eq!(fs::read_to_string(&ir_path).unwrap(), original);
        assert!(fs::symlink_metadata(ir_alias)
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[test]
    fn test_cli_rejects_concurrent_writer_for_same_artifact_set() {
        // Arrange
        let temp_dir = temp_dir("rejects_concurrent_writer_for_same_artifact_set");
        let python_dir = temp_dir.join("python");
        let marker = temp_dir.join("import-started");
        let output = temp_dir.join("bindings.rs");
        let report = temp_dir.join("report.json");
        let first_temp = temp_dir.join("first-temp");
        let second_temp = temp_dir.join("second-temp");
        fs::create_dir_all(&first_temp).unwrap();
        fs::create_dir_all(&second_temp).unwrap();
        write_slow_python_module(&python_dir);

        let mut first = std::process::Command::new(assert_cmd::cargo::cargo_bin!("pyo3_bindgen"));
        let first = first
            .env("PYTHONPATH", &python_dir)
            .env("PYO3_BINDGEN_TEST_MARKER", &marker)
            .env("PYO3_BINDGEN_TEST_DELAY", "1.5")
            .env("TMPDIR", &first_temp)
            .env("TMP", &first_temp)
            .env("TEMP", &first_temp)
            .args(["-m", "slow_bindgen_module", "--output"])
            .arg(&output)
            .arg("--report")
            .arg(&report)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        wait_for_path(&marker);

        // Act: the first process is inside Python import and still owns both artifacts.
        let mut second = cli_command();
        let second_assert = second
            .env("PYTHONPATH", &python_dir)
            .env("PYO3_BINDGEN_TEST_MARKER", temp_dir.join("second-marker"))
            .env("PYO3_BINDGEN_TEST_DELAY", "0")
            .env("TMPDIR", &second_temp)
            .env("TMP", &second_temp)
            .env("TEMP", &second_temp)
            .args(["-m", "slow_bindgen_module", "--output"])
            .arg(&output)
            .arg("--report")
            .arg(&report)
            .assert();

        // Assert
        second_assert
            .failure()
            .stderr(predicate::str::contains("locked by active process"));
        let first_output = first.wait_with_output().unwrap();
        assert_eq!(
            first_output.status.code(),
            Some(expected_file_publication_exit_code()),
            "first writer failed: {}",
            String::from_utf8_lossy(&first_output.stderr)
        );
        let bindings = fs::read(&output).unwrap();
        let report_json: serde_json::Value =
            serde_json::from_slice(&fs::read(&report).unwrap()).unwrap();
        assert_eq!(
            report_json["outputs"][0]["hash"],
            pyo3_bindgen::stable_hash(&bindings)
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_cli_rejects_late_parent_symlink_before_any_artifact_delivery() {
        use std::os::unix::fs::symlink;

        let temp_dir = temp_dir("rejects_late_parent_symlink");
        let python_dir = temp_dir.join("python");
        let marker = temp_dir.join("import-started");
        let real_dir = temp_dir.join("real");
        let alias_dir = temp_dir.join("alias");
        let output = real_dir.join("bindings.rs");
        let report = alias_dir.join("bindings.rs");
        fs::create_dir_all(&real_dir).unwrap();
        write_slow_python_module(&python_dir);

        let mut child = std::process::Command::new(assert_cmd::cargo::cargo_bin!("pyo3_bindgen"));
        let child = child
            .env("PYTHONPATH", &python_dir)
            .env("PYO3_BINDGEN_TEST_MARKER", &marker)
            .env("PYO3_BINDGEN_TEST_DELAY", "1.5")
            .args(["-m", "slow_bindgen_module", "--output"])
            .arg(&output)
            .arg("--report")
            .arg(&report)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        wait_for_path(&marker);

        // The report parent was missing when identities and locks were captured.
        // Retarget it while generation is paused so it now aliases the output.
        symlink(&real_dir, &alias_dir).unwrap();

        let result = child.wait_with_output().unwrap();
        assert_eq!(result.status.code(), Some(1));
        assert!(
            String::from_utf8_lossy(&result.stderr).contains("changed identity during generation"),
            "unexpected stderr: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(
            !output.exists(),
            "the whole write set must be revalidated before output delivery"
        );
        assert!(
            !report.exists(),
            "an unsafe retargeted report must fail closed during recovery"
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_cli_revalidation_failure_recovers_distinct_report() {
        use std::os::unix::fs::symlink;

        let temp_dir = temp_dir("revalidation_recovers_distinct_report");
        let python_dir = temp_dir.join("python");
        let marker = temp_dir.join("import-started");
        let first_dir = temp_dir.join("first");
        let second_dir = temp_dir.join("second");
        let alias_dir = temp_dir.join("alias");
        let output = alias_dir.join("bindings.rs");
        let report = temp_dir.join("failure-report.json");
        fs::create_dir_all(&first_dir).unwrap();
        fs::create_dir_all(&second_dir).unwrap();
        symlink(&first_dir, &alias_dir).unwrap();
        write_slow_python_module(&python_dir);

        let mut child = std::process::Command::new(assert_cmd::cargo::cargo_bin!("pyo3_bindgen"));
        let child = child
            .env("PYTHONPATH", &python_dir)
            .env("PYO3_BINDGEN_TEST_MARKER", &marker)
            .env("PYO3_BINDGEN_TEST_DELAY", "1.5")
            .args(["-m", "slow_bindgen_module", "--output"])
            .arg(&output)
            .arg("--report")
            .arg(&report)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        wait_for_path(&marker);

        fs::remove_file(&alias_dir).unwrap();
        symlink(&second_dir, &alias_dir).unwrap();

        let result = child.wait_with_output().unwrap();
        assert_eq!(result.status.code(), Some(1));
        assert!(
            String::from_utf8_lossy(&result.stderr).contains("changed identity during generation"),
            "unexpected stderr: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(!first_dir.join("bindings.rs").exists());
        assert!(!second_dir.join("bindings.rs").exists());
        let report: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&report).unwrap()).unwrap();
        assert_eq!(report["schema_version"], 1);
        assert_eq!(report["outputs"], serde_json::json!([]));
        assert!(report["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|diagnostic| diagnostic["code"] == "artifact_revalidation_failed"));
    }

    #[cfg(unix)]
    #[test]
    fn test_cli_rejects_case_only_symlink_retarget_before_artifact_delivery() {
        use std::os::unix::fs::symlink;

        let temp_dir = temp_dir("rejects_case_only_symlink_retarget");
        let python_dir = temp_dir.join("python");
        let marker = temp_dir.join("import-started");
        let lower_dir = temp_dir.join("target");
        let upper_dir = temp_dir.join("TARGET");
        let alias_dir = temp_dir.join("alias");
        fs::create_dir_all(&lower_dir).unwrap();
        match fs::create_dir(&upper_dir) {
            Ok(()) => {}
            // A case-insensitive Unix volume cannot model this regression.
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => return,
            Err(error) => panic!("failed to create case-distinct fixture: {error}"),
        }
        if lower_dir.canonicalize().unwrap() == upper_dir.canonicalize().unwrap() {
            return;
        }
        symlink(&lower_dir, &alias_dir).unwrap();
        write_slow_python_module(&python_dir);
        let requested_output = alias_dir.join("bindings.rs");

        let mut child = std::process::Command::new(assert_cmd::cargo::cargo_bin!("pyo3_bindgen"));
        let child = child
            .env("PYTHONPATH", &python_dir)
            .env("PYO3_BINDGEN_TEST_MARKER", &marker)
            .env("PYO3_BINDGEN_TEST_DELAY", "1.5")
            .args(["-m", "slow_bindgen_module", "--output"])
            .arg(&requested_output)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        wait_for_path(&marker);

        fs::remove_file(&alias_dir).unwrap();
        symlink(&upper_dir, &alias_dir).unwrap();

        let result = child.wait_with_output().unwrap();
        assert!(!result.status.success());
        assert!(
            String::from_utf8_lossy(&result.stderr).contains("changed identity during generation"),
            "unexpected stderr: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(
            !lower_dir.join("bindings.rs").exists() && !upper_dir.join("bindings.rs").exists(),
            "a retargeted path must fail before writing either frozen or current destination"
        );
    }

    #[test]
    fn test_cli_rejects_invalid_report_targets_before_generation_or_side_effects() {
        let temp_dir = temp_dir("rejects_invalid_report_targets");
        let python_dir = temp_dir.join("python");
        write_slow_python_module(&python_dir);

        for (case, report, expected_error) in [
            (
                "trailing-separator",
                "report/",
                "must end in a normal file name",
            ),
            (
                "trailing-current-dir",
                "report/.",
                "must end in a normal file name",
            ),
            (
                "trailing-parent-dir",
                "created/..",
                "must end in a normal file name",
            ),
            (
                "existing-directory",
                "report-directory",
                "is not a regular file",
            ),
        ] {
            let case_dir = temp_dir.join(case);
            fs::create_dir_all(&case_dir).unwrap();
            if case == "existing-directory" {
                fs::create_dir(case_dir.join(report)).unwrap();
            }
            let marker = case_dir.join("import-started");
            let output = case_dir.join("bindings.rs");

            let mut cmd = cli_command();
            cmd.current_dir(&case_dir)
                .env("PYTHONPATH", &python_dir)
                .env("PYO3_BINDGEN_TEST_MARKER", &marker)
                .env("PYO3_BINDGEN_TEST_DELAY", "0")
                .args([
                    "-m",
                    "slow_bindgen_module",
                    "--output",
                    "bindings.rs",
                    "--report",
                    report,
                ])
                .assert()
                .failure()
                .stderr(predicate::str::contains(expected_error));

            assert!(!marker.exists(), "{case} reached Python generation");
            assert!(!output.exists(), "{case} delivered output");
            assert!(
                !case_dir.join("created").exists(),
                "{case} created a rejected parent directory"
            );
        }
    }

    #[test]
    fn test_cli_relative_artifacts_remain_bound_to_invocation_directory_after_python_chdir() {
        let temp_dir = temp_dir("relative_artifacts_survive_python_chdir");
        let invocation_dir = temp_dir.join("invocation");
        let changed_dir = temp_dir.join("changed");
        let python_dir = temp_dir.join("python");
        fs::create_dir_all(&invocation_dir).unwrap();
        fs::create_dir_all(&changed_dir).unwrap();
        fs::create_dir_all(&python_dir).unwrap();
        fs::write(
            python_dir.join("chdir_bindgen_module.py"),
            r#"
import os
os.chdir(os.environ["PYO3_BINDGEN_TEST_CHDIR"])

def ready():
    return True
"#,
        )
        .unwrap();

        let mut cmd = cli_command();
        cmd.current_dir(&invocation_dir)
            .env("PYTHONPATH", &python_dir)
            .env("PYO3_BINDGEN_TEST_CHDIR", &changed_dir)
            .args([
                "-m",
                "chdir_bindgen_module",
                "--output",
                "bindings.rs",
                "--report",
                "report.json",
            ])
            .assert()
            .code(expected_file_publication_exit_code());

        let output = invocation_dir.join("bindings.rs");
        let report = invocation_dir.join("report.json");
        assert!(fs::read_to_string(&output)
            .unwrap()
            .contains("pub fn ready"));
        let report_json: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(report).unwrap()).unwrap();
        assert_eq!(
            report_json["outputs"][0]["path"],
            canonical_display_path(&output)
        );
        assert!(!changed_dir.join("bindings.rs").exists());
        assert!(!changed_dir.join("report.json").exists());
    }

    #[cfg(windows)]
    #[test]
    fn test_cli_shares_artifact_lock_across_unicode_case_aliases() {
        let temp_dir = temp_dir("shares_lock_across_unicode_case_aliases");
        let python_dir = temp_dir.join("python");
        let marker = temp_dir.join("import-started");
        let sigma_output = temp_dir.join("Σ.rs");
        let final_sigma_output = temp_dir.join("ς.rs");
        write_slow_python_module(&python_dir);

        let mut first = std::process::Command::new(assert_cmd::cargo::cargo_bin!("pyo3_bindgen"));
        let first = first
            .env("PYTHONPATH", &python_dir)
            .env("PYO3_BINDGEN_TEST_MARKER", &marker)
            .env("PYO3_BINDGEN_TEST_DELAY", "1.5")
            .args(["-m", "slow_bindgen_module", "--output"])
            .arg(&sigma_output)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        wait_for_path(&marker);

        let mut second = cli_command();
        second
            .env("PYTHONPATH", &python_dir)
            .env("PYO3_BINDGEN_TEST_MARKER", temp_dir.join("second-marker"))
            .env("PYO3_BINDGEN_TEST_DELAY", "0")
            .args(["-m", "slow_bindgen_module", "--output"])
            .arg(&final_sigma_output)
            .assert()
            .failure()
            .stderr(predicate::str::contains("locked by active process"));

        let first_output = first.wait_with_output().unwrap();
        assert_eq!(
            first_output.status.code(),
            Some(expected_file_publication_exit_code()),
            "first writer failed: {}",
            String::from_utf8_lossy(&first_output.stderr)
        );
    }

    #[test]
    fn test_cli_recovers_artifact_locks_after_owner_dies() {
        // Arrange: terminate a writer while Python import is in progress, before
        // normal Drop cleanup can remove either artifact lock.
        let temp_dir = temp_dir("recovers_artifact_locks_after_owner_dies");
        let python_dir = temp_dir.join("python");
        let first_marker = temp_dir.join("first-import-started");
        let output = temp_dir.join("bindings.rs");
        let report = temp_dir.join("report.json");
        write_slow_python_module(&python_dir);

        let mut abandoned =
            std::process::Command::new(assert_cmd::cargo::cargo_bin!("pyo3_bindgen"));
        let mut abandoned = abandoned
            .env("PYTHONPATH", &python_dir)
            .env("PYO3_BINDGEN_TEST_MARKER", &first_marker)
            .env("PYO3_BINDGEN_TEST_DELAY", "30")
            .args(["-m", "slow_bindgen_module", "--output"])
            .arg(&output)
            .arg("--report")
            .arg(&report)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        wait_for_path(&first_marker);
        abandoned.kill().unwrap();
        abandoned.wait().unwrap();

        // Act
        let mut replacement = cli_command();
        let replacement_assert = replacement
            .env("PYTHONPATH", &python_dir)
            .env(
                "PYO3_BINDGEN_TEST_MARKER",
                temp_dir.join("replacement-import-started"),
            )
            .env("PYO3_BINDGEN_TEST_DELAY", "0")
            .args(["-m", "slow_bindgen_module", "--output"])
            .arg(&output)
            .arg("--report")
            .arg(&report)
            .assert();

        // Assert
        replacement_assert.code(expected_file_publication_exit_code());
        assert!(fs::read_to_string(output).unwrap().contains("pub fn ready"));
        let report_json: serde_json::Value =
            serde_json::from_slice(&fs::read(report).unwrap()).unwrap();
        assert_eq!(report_json["schema_version"], 1);
    }

    #[test]
    fn test_cli_check_rejects_report_directory_before_reading_output() {
        // An unusable report path is rejected before check-mode generation or reads.
        let temp_dir = temp_dir("check_error_is_not_masked_by_report_write_failure");
        let output = temp_dir.join("bindings.rs");
        let report_directory = temp_dir.join("report-directory");
        let original = "stale bindings\n";
        fs::write(&output, original).unwrap();
        fs::create_dir(&report_directory).unwrap();

        // Act
        let mut cmd = cli_command();
        let assert = cmd
            .args(["-m", "sys", "--output"])
            .arg(&output)
            .args(["--check", "--report"])
            .arg(&report_directory)
            .assert();

        assert
            .failure()
            .stderr(predicate::str::contains("is not a regular file"));
        assert_eq!(fs::read_to_string(output).unwrap(), original);
        assert!(report_directory.is_dir());
    }

    #[test]
    fn test_cli_rejects_output_directory_before_writing_report() {
        let temp_dir = temp_dir("output_write_failure_is_reported_instead_of_masked");
        let output_directory = temp_dir.join("bindings.rs");
        let report = temp_dir.join("report.json");
        fs::create_dir(&output_directory).unwrap();

        // Act
        let mut cmd = cli_command();
        let assert = cmd
            .args(["-m", "sys", "--output"])
            .arg(&output_directory)
            .args(["--report"])
            .arg(&report)
            .assert();

        assert
            .failure()
            .stderr(predicate::str::contains("is not a regular file"));
        assert!(!report.exists());
        assert!(output_directory.is_dir());
    }

    #[test]
    fn test_cli_rejects_output_report_alias_before_writing() {
        // Arrange
        let temp_dir = temp_dir("rejects_output_report_alias_before_writing");
        let output = temp_dir.join("bindings.rs");
        let original = "existing bindings\n";
        fs::write(&output, original).unwrap();

        // Act: both paths name the same file through different relative spellings.
        let mut cmd = cli_command();
        let assert = cmd
            .current_dir(&temp_dir)
            .args([
                "-m",
                "sys",
                "--output",
                "bindings.rs",
                "--report",
                "./bindings.rs",
            ])
            .assert();

        // Assert: validation happens before either artifact can be changed.
        assert.failure().stderr(predicate::str::contains(
            "--output/--output-dir and --report must refer to different files",
        ));
        assert_eq!(fs::read_to_string(output).unwrap(), original);
    }

    #[test]
    fn test_cli_check_rejects_output_dir_report_alias_before_writing() {
        // Arrange
        let temp_dir = temp_dir("check_rejects_output_dir_report_alias_before_writing");
        let output_dir = temp_dir.join("generated");
        fs::create_dir_all(&output_dir).unwrap();
        let output = output_dir.join("bindings.rs");
        let original = "existing bindings\n";
        fs::write(&output, original).unwrap();

        // Act: --output-dir resolves its target to generated/bindings.rs.
        let mut cmd = cli_command();
        let assert = cmd
            .current_dir(&temp_dir)
            .args([
                "-m",
                "sys",
                "--output-dir",
                "generated",
                "--check",
                "--report",
                "generated/./bindings.rs",
            ])
            .assert();

        // Assert: --check does not weaken collision validation or write a report.
        assert.failure().stderr(predicate::str::contains(
            "--output/--output-dir and --report must refer to different files",
        ));
        assert_eq!(fs::read_to_string(output).unwrap(), original);
    }

    #[test]
    fn test_cli_conservatively_rejects_case_varied_output_report_paths() {
        // Arrange: neither spelling exists, so collision detection cannot rely on a
        // pre-existing file identity. The mounted filesystem may be case-insensitive
        // even when the host OS normally uses case-sensitive volumes.
        let temp_dir = temp_dir("rejects_case_insensitive_output_report_alias");

        // Act
        let mut cmd = cli_command();
        let assert = cmd
            .current_dir(&temp_dir)
            .args([
                "-m",
                "sys",
                "--output",
                "Bindings.rs",
                "--report",
                "bindings.rs",
            ])
            .assert();

        // Assert
        assert.failure().stderr(predicate::str::contains(
            "--output/--output-dir and --report must refer to different files",
        ));
        assert!(!temp_dir.join("bindings.rs").exists());
    }

    #[cfg(unix)]
    #[test]
    fn test_cli_rejects_hard_linked_output_report_alias_before_writing() {
        let temp_dir = temp_dir("rejects_hard_linked_output_report_alias");
        let output = temp_dir.join("bindings.rs");
        let report = temp_dir.join("report.json");
        let original = "existing bindings\n";
        fs::write(&output, original).unwrap();
        fs::hard_link(&output, &report).unwrap();

        let mut cmd = cli_command();
        cmd.args(["-m", "sys", "--output"])
            .arg(&output)
            .args(["--report"])
            .arg(&report)
            .assert()
            .failure()
            .stderr(predicate::str::contains(
                "--output/--output-dir and --report must refer to different files",
            ));

        assert_eq!(fs::read_to_string(output).unwrap(), original);
        assert_eq!(fs::read_to_string(report).unwrap(), original);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_cli_rejects_missing_case_and_unicode_normalization_aliases_on_macos() {
        let temp_dir = temp_dir("rejects_macos_missing_artifact_aliases");

        for (case, output, report) in [
            ("case", "Bindings.rs", "bindings.rs"),
            ("normalization", "\u{e9}.rs", "e\u{301}.rs"),
        ] {
            let case_dir = temp_dir.join(case);
            fs::create_dir(&case_dir).unwrap();
            let mut cmd = cli_command();
            cmd.current_dir(&case_dir)
                .args(["-m", "sys", "--output", output, "--report", report])
                .assert()
                .failure()
                .stderr(predicate::str::contains(
                    "--output/--output-dir and --report must refer to different files",
                ));

            assert!(fs::read_dir(case_dir).unwrap().next().is_none());
        }
    }

    #[cfg(windows)]
    #[test]
    fn test_cli_rejects_unicode_output_report_alias_before_writing() {
        let temp_dir = temp_dir("rejects_unicode_output_report_alias");

        let mut cmd = cli_command();
        cmd.current_dir(&temp_dir)
            .args(["-m", "sys", "--output", "Σ.rs", "--report", "ς.rs"])
            .assert()
            .failure()
            .stderr(predicate::str::contains(
                "--output/--output-dir and --report must refer to different files",
            ));

        assert!(!temp_dir.join("Σ.rs").exists());
        assert!(!temp_dir.join("ς.rs").exists());
    }

    #[cfg(windows)]
    #[test]
    fn test_cli_rejects_trailing_dot_output_report_alias_before_writing() {
        // Arrange: Win32 strips the report's trailing dot, making both arguments
        // name the existing output file.
        let temp_dir = temp_dir("rejects_trailing_dot_output_report_alias");
        let output = temp_dir.join("bindings.rs");
        let original = "existing bindings\n";
        fs::write(&output, original).unwrap();

        // Act
        let mut cmd = cli_command();
        let assert = cmd
            .current_dir(&temp_dir)
            .args([
                "-m",
                "sys",
                "--output",
                "bindings.rs",
                "--report",
                "bindings.rs.",
            ])
            .assert();

        // Assert: lexical validation runs before identity, locks, or writes.
        assert
            .failure()
            .stderr(predicate::str::contains("not valid on Windows"));
        assert_eq!(fs::read_to_string(output).unwrap(), original);
    }

    #[cfg(windows)]
    #[test]
    fn test_cli_rejects_missing_parent_win32_alias_before_writing() {
        // Arrange: neither parent exists. Canonicalization therefore cannot expose
        // that Win32 aliases `generated.` to `generated`.
        let temp_dir = temp_dir("rejects_missing_parent_win32_alias");

        // Act
        let mut cmd = cli_command();
        let assert = cmd
            .current_dir(&temp_dir)
            .args([
                "-m",
                "sys",
                "--output",
                r"generated\bindings.rs",
                "--report",
                r"generated.\bindings.rs",
            ])
            .assert();

        // Assert
        assert
            .failure()
            .stderr(predicate::str::contains("not valid on Windows"));
        assert!(!temp_dir.join("generated").exists());
    }

    #[cfg(windows)]
    #[test]
    fn test_cli_rejects_reserved_device_artifact_paths() {
        let temp_dir = temp_dir("rejects_reserved_device_artifact_paths");

        for arguments in [
            ["-m", "sys", "--output", "NUL.rs"],
            ["-m", "sys", "--report", "CON.json"],
        ] {
            let mut cmd = cli_command();
            cmd.current_dir(&temp_dir)
                .args(arguments)
                .assert()
                .failure()
                .stderr(predicate::str::contains("reserved DOS device name"));
        }

        assert!(fs::read_dir(temp_dir).unwrap().next().is_none());
    }

    #[cfg(windows)]
    #[test]
    fn test_cli_rejects_unsupported_windows_namespaces_before_writing() {
        let temp_dir = temp_dir("rejects_unsupported_windows_namespaces");

        for arguments in [
            ["-m", "sys", "--output", r"\\.\NUL"],
            [
                "-m",
                "sys",
                "--report",
                r"\\?\GLOBALROOT\Device\HarddiskVolume1\report.json",
            ],
        ] {
            let mut cmd = cli_command();
            cmd.current_dir(&temp_dir)
                .args(arguments)
                .assert()
                .failure()
                .stderr(predicate::str::contains(
                    "unsupported Windows path namespace",
                ));
        }

        assert!(fs::read_dir(temp_dir).unwrap().next().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn test_cli_rejects_symlinked_output_report_alias_before_writing() {
        use std::os::unix::fs::symlink;

        // Arrange
        let temp_dir = temp_dir("rejects_symlinked_output_report_alias_before_writing");
        let real_dir = temp_dir.join("real");
        fs::create_dir_all(&real_dir).unwrap();
        symlink(&real_dir, temp_dir.join("alias")).unwrap();
        let output = real_dir.join("bindings.rs");

        // Act: real/ and alias/ are different relative paths to one directory.
        let mut cmd = cli_command();
        let assert = cmd
            .current_dir(&temp_dir)
            .args([
                "-m",
                "sys",
                "--output",
                "real/bindings.rs",
                "--report",
                "alias/bindings.rs",
            ])
            .assert();

        // Assert
        assert.failure().stderr(predicate::str::contains(
            "--output/--output-dir and --report must refer to different files",
        ));
        assert!(!output.exists());
    }

    /// A symlinked directory component COMBINED WITH `..` must still collapse to the same
    /// identity as its plain spelling. Lexically folding `..` before canonicalizing (a
    /// former regression) drops the symlink `link` from `link/../bindings.rs` and keys it
    /// differently from `sub/bindings.rs`, letting the report silently overwrite the
    /// bindings. Canonicalizing the raw path first (`realpath`) resolves both to one file.
    #[cfg(unix)]
    #[test]
    fn test_cli_rejects_symlinked_parent_dotdot_output_report_alias() {
        use std::os::unix::fs::symlink;

        // Arrange: `link` -> `sub/deep`, so `link/../bindings.rs` == `sub/bindings.rs`.
        let temp_dir = temp_dir("rejects_symlinked_parent_dotdot_output_report_alias");
        let deep_dir = temp_dir.join("sub").join("deep");
        fs::create_dir_all(&deep_dir).unwrap();
        symlink(&deep_dir, temp_dir.join("link")).unwrap();
        let output = temp_dir.join("sub").join("bindings.rs");

        // Act
        let mut cmd = cli_command();
        let assert = cmd
            .current_dir(&temp_dir)
            .args([
                "-m",
                "sys",
                "--output",
                "sub/bindings.rs",
                "--report",
                "link/../bindings.rs",
            ])
            .assert();

        // Assert
        assert.failure().stderr(predicate::str::contains(
            "--output/--output-dir and --report must refer to different files",
        ));
        assert!(!output.exists());
    }

    /// A missing component before `..` must not stop identity resolution before the
    /// symlink has been applied. Both arguments still name one not-yet-created file.
    #[cfg(unix)]
    #[test]
    fn test_cli_rejects_symlinked_missing_parent_dotdot_output_report_alias() {
        use std::os::unix::fs::symlink;

        let temp_dir = temp_dir("rejects_symlinked_missing_parent_dotdot_output_report_alias");
        let real_dir = temp_dir.join("real");
        fs::create_dir_all(&real_dir).unwrap();
        symlink(&real_dir, temp_dir.join("alias")).unwrap();
        let output = real_dir.join("bindings.rs");

        let mut cmd = cli_command();
        let assert = cmd
            .current_dir(&temp_dir)
            .args([
                "-m",
                "sys",
                "--output",
                "real/bindings.rs",
                "--report",
                "alias/missing/../bindings.rs",
            ])
            .assert();

        assert.failure().stderr(predicate::str::contains(
            "--output/--output-dir and --report must refer to different files",
        ));
        assert!(!output.exists());
    }

    /// `--include-private`, `--blocklist-name` and `--no-deduplicate-inherited-members`
    /// only affect live introspection; passing any with `--ir` is a usage error, not a
    /// silent no-op.
    #[test]
    fn test_cli_ir_conflicts_with_live_only_flags() {
        for flag in [
            ["--include-private"].as_slice(),
            ["--blocklist-name", "foo"].as_slice(),
            ["--no-deduplicate-inherited-members"].as_slice(),
        ] {
            let mut cmd = cli_command();
            cmd.args(["--ir", "some.json"])
                .args(flag)
                .assert()
                .failure()
                .stderr(predicate::str::contains("cannot be used with"));
        }
    }

    /// The dedup flag's functional effect (not just its `--ir` conflict): `pathlib` has a
    /// deep, documented class hierarchy that deduplicates by default (emitting the synthetic
    /// `PyObjectBase` root trait and shrinking the output), and
    /// `--no-deduplicate-inherited-members` turns that off.
    #[test]
    fn test_cli_deduplicates_by_default_and_flag_disables_it() {
        let default_assert = cli_command().args(["-m", "pathlib"]).assert().success();
        let default_stdout =
            String::from_utf8_lossy(&default_assert.get_output().stdout).into_owned();

        let flat_assert = cli_command()
            .args(["-m", "pathlib", "--no-deduplicate-inherited-members"])
            .assert()
            .success();
        let flat_stdout = String::from_utf8_lossy(&flat_assert.get_output().stdout).into_owned();

        assert!(
            default_stdout.contains("trait PyObjectBase"),
            "pathlib should deduplicate by default, emitting the root-support trait",
        );
        assert!(
            !flat_stdout.contains("trait PyObjectBase"),
            "--no-deduplicate-inherited-members should disable the deduplicated object model",
        );
        assert!(
            default_stdout.len() < flat_stdout.len(),
            "deduplicated pathlib ({}) should be smaller than the flat output ({})",
            default_stdout.len(),
            flat_stdout.len(),
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_cli_rejects_dangling_output_symlink_before_writing() {
        use std::os::unix::fs::symlink;

        // Arrange
        let temp_dir = temp_dir("rejects_dangling_output_symlink_before_writing");
        let target = temp_dir.join("missing-target.rs");
        let output = temp_dir.join("bindings.rs");
        symlink(&target, &output).unwrap();

        // Act
        let mut cmd = cli_command();
        let assert = cmd.args(["-m", "sys", "--output"]).arg(&output).assert();

        // Assert
        assert
            .failure()
            .stderr(predicate::str::contains("symbolic link destination"));
        assert!(fs::symlink_metadata(&output)
            .unwrap()
            .file_type()
            .is_symlink());
        assert!(!target.exists());
    }

    #[test]
    fn test_cli_output_dir() {
        // Arrange
        let temp_dir = temp_dir("output_dir");

        // Act
        let mut cmd = cli_command();
        let assert = cmd
            .args(["-m", "sys", "--output-dir"])
            .arg(&temp_dir)
            .assert();

        // Assert
        assert.code(expected_file_publication_exit_code());
        assert!(temp_dir.join("bindings.rs").exists());
    }

    #[cfg(unix)]
    #[test]
    fn test_cli_atomic_replacement_preserves_destination_permissions() {
        use std::os::unix::fs::PermissionsExt;

        // Arrange
        let temp_dir = temp_dir("atomic_replacement_preserves_destination_permissions");
        let output = temp_dir.join("bindings.rs");
        fs::write(&output, "old bindings\n").unwrap();
        fs::set_permissions(&output, fs::Permissions::from_mode(0o640)).unwrap();

        // Act
        let mut cmd = cli_command();
        cmd.args(["-m", "sys", "--output"])
            .arg(&output)
            .assert()
            .success();

        // Assert
        assert!(fs::read_to_string(&output).unwrap().contains("pub mod sys"));
        assert_eq!(
            fs::metadata(output).unwrap().permissions().mode() & 0o777,
            0o640
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_cli_rejects_existing_output_symlink_without_touching_target() {
        use std::os::unix::fs::symlink;

        // Arrange
        let temp_dir = temp_dir("rejects_existing_output_symlink");
        let target = temp_dir.join("real-bindings.rs");
        let output = temp_dir.join("bindings.rs");
        let original = "existing bindings\n";
        fs::write(&target, original).unwrap();
        symlink(&target, &output).unwrap();

        // Act
        let mut cmd = cli_command();
        let assert = cmd.args(["-m", "sys", "--output"]).arg(&output).assert();

        // Assert
        assert
            .failure()
            .stderr(predicate::str::contains("symbolic link destination"));
        assert_eq!(fs::read_to_string(target).unwrap(), original);
        assert!(fs::symlink_metadata(output)
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[cfg(unix)]
    #[test]
    fn test_cli_rejects_existing_report_symlink_without_touching_target() {
        use std::os::unix::fs::symlink;

        // Arrange
        let temp_dir = temp_dir("rejects_existing_report_symlink");
        let target = temp_dir.join("real-report.json");
        let report = temp_dir.join("report.json");
        let original = "existing report\n";
        fs::write(&target, original).unwrap();
        symlink(&target, &report).unwrap();

        // Act
        let mut cmd = cli_command();
        let assert = cmd.args(["-m", "sys", "--report"]).arg(&report).assert();

        // Assert
        assert
            .failure()
            .stderr(predicate::str::contains("symbolic link destination"));
        assert_eq!(fs::read_to_string(target).unwrap(), original);
        assert!(fs::symlink_metadata(report)
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[test]
    fn test_cli_warning_policy_report() {
        // Arrange
        let temp_dir = temp_dir("warning_policy_report");
        let python_dir = temp_dir.join("python");
        fs::create_dir_all(&python_dir).unwrap();
        fs::write(
            python_dir.join("warnmod.py"),
            r#"
from enum import Flag

class Permission(Flag):
    # A negative member cannot be mirrored as the u64 flag newtype, so this class keeps
    # the owning wrapper and emits the `enum_flag_fallback` warning.
    ALL = -1
"#,
        )
        .unwrap();
        let report = temp_dir.join("warning_report.json");

        // Act
        let mut cmd = cli_command();
        let assert = cmd
            .env("PYTHONPATH", &python_dir)
            .args(["-m", "warnmod", "--deny-warnings", "--report"])
            .arg(&report)
            .assert();

        // Assert
        assert
            .failure()
            .stderr(predicate::str::contains("--deny-warnings is set"));
        let report_json: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&report).unwrap()).unwrap();
        assert!(report_json["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|diagnostic| diagnostic["code"] == "enum_flag_fallback"));
        assert!(report_json["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|diagnostic| diagnostic["code"] == "warning_policy"));
        assert_eq!(report_json["outputs"], serde_json::json!([]));
    }

    #[test]
    fn test_cli_filters_private_blocklist_and_submodule_traversal() {
        // Arrange
        let temp_dir = temp_dir("filters_private_blocklist_and_submodule_traversal");
        let python_dir = temp_dir.join("python");
        let package_dir = python_dir.join("filterpkg");
        fs::create_dir_all(&package_dir).unwrap();
        fs::write(
            package_dir.join("__init__.py"),
            r#"
from . import child

def visible():
    pass

def _secret():
    pass

def blocked():
    pass
"#,
        )
        .unwrap();
        fs::write(
            package_dir.join("child.py"),
            r#"
def child_visible():
    pass
"#,
        )
        .unwrap();
        let default_output = temp_dir.join("default.rs");
        let filtered_output = temp_dir.join("filtered.rs");
        let filtered_report = temp_dir.join("filtered_report.json");

        // Act + assert default filtering keeps public attrs, skips private attrs, and traverses packages.
        let mut default_cmd = cli_command();
        default_cmd
            .env("PYTHONPATH", &python_dir)
            .args(["-m", "filterpkg", "--output"])
            .arg(&default_output)
            .assert()
            .code(expected_file_publication_exit_code());
        let default_bindings = fs::read_to_string(&default_output).unwrap();
        assert!(default_bindings.contains("pub fn visible"));
        assert!(default_bindings.contains("pub fn blocked"));
        assert!(default_bindings.contains("pub mod child"));
        assert!(!default_bindings.contains("pub fn _secret"));

        // Act + assert explicit flags include private attrs, skip the custom blocklist, and stop at the package root.
        let mut filtered_cmd = cli_command();
        filtered_cmd
            .env("PYTHONPATH", &python_dir)
            .args([
                "-m",
                "filterpkg",
                "--include-private",
                "--no-traverse-submodules",
                "--blocklist-name",
                "blocked",
                "--output",
            ])
            .arg(&filtered_output)
            .args(["--report"])
            .arg(&filtered_report)
            .assert()
            .code(expected_file_publication_exit_code());
        let filtered_bindings = fs::read_to_string(&filtered_output).unwrap();
        assert!(filtered_bindings.contains("pub fn visible"));
        assert!(filtered_bindings.contains("pub fn _secret"));
        assert!(!filtered_bindings.contains("pub fn blocked"));
        assert!(!filtered_bindings.contains("pub mod child"));
        let report_json: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&filtered_report).unwrap()).unwrap();
        assert_eq!(report_json["modules"].as_array().unwrap().len(), 1);
        assert_eq!(report_json["modules"][0]["name"], "filterpkg");
    }

    #[test]
    fn test_cli_strict_completeness_blocks_incomplete_submodule_output() {
        let temp_dir = temp_dir("strict_completeness_submodule");
        let python_dir = temp_dir.join("python");
        let package_dir = python_dir.join("incompletepkg");
        fs::create_dir_all(&package_dir).unwrap();
        fs::write(package_dir.join("__init__.py"), "").unwrap();
        fs::write(
            package_dir.join("good.py"),
            "def ready():\n    return True\n",
        )
        .unwrap();
        fs::write(
            package_dir.join("broken.py"),
            "raise RuntimeError('intentional discovery failure')\n",
        )
        .unwrap();

        let best_effort_output = temp_dir.join("best_effort.rs");
        let best_effort_report = temp_dir.join("best_effort.report.json");
        cli_command()
            .env("PYTHONPATH", &python_dir)
            .args(["--module-name", "incompletepkg", "--output"])
            .arg(&best_effort_output)
            .arg("--report")
            .arg(&best_effort_report)
            .assert()
            .code(expected_file_publication_exit_code());
        let report: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(best_effort_report).unwrap()).unwrap();
        assert_eq!(report["schema_version"], 1);
        for code in [
            "discovery_submodule_attempted",
            "discovery_submodule_emitted",
            "discovery_submodule_failed",
            "discovery_submodule_summary",
        ] {
            assert!(
                report["diagnostics"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|diagnostic| diagnostic["code"] == code),
                "missing {code}: {}",
                report["diagnostics"]
            );
        }

        let strict_output = temp_dir.join("strict.rs");
        let strict_report = temp_dir.join("strict.report.json");
        fs::write(&strict_output, "sentinel: preserve on strict failure\n").unwrap();
        cli_command()
            .env("PYTHONPATH", &python_dir)
            .args([
                "--module-name",
                "incompletepkg",
                "--strict-completeness",
                "--output",
            ])
            .arg(&strict_output)
            .arg("--report")
            .arg(&strict_report)
            .assert()
            .failure();
        assert_eq!(
            fs::read_to_string(strict_output).unwrap(),
            "sentinel: preserve on strict failure\n"
        );
        let report: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(strict_report).unwrap()).unwrap();
        assert_eq!(report["schema_version"], 1);
        assert_eq!(report["outputs"], serde_json::json!([]));
        for code in [
            "discovery_submodule_attempted",
            "discovery_submodule_failed",
            "discovery_submodule_summary",
            "strict_completeness_failed",
        ] {
            assert!(
                report["diagnostics"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|diagnostic| diagnostic["code"] == code),
                "strict failure report lost {code}: {}",
                report["diagnostics"]
            );
        }
    }

    #[test]
    fn test_cli_warning_budget() {
        // Arrange: a Python module that triggers at least one codegen warning
        // (an unsupported Flag enum falls back to a generic binding, emitting enum_flag_fallback)
        let temp_dir = temp_dir("warning_budget");
        let python_dir = temp_dir.join("python");
        fs::create_dir_all(&python_dir).unwrap();
        fs::write(
            python_dir.join("budgetmod.py"),
            r#"
from enum import Flag

class Permission(Flag):
    # A negative member cannot be mirrored as the u64 flag newtype, so this class keeps
    # the owning wrapper and emits the `enum_flag_fallback` warning.
    ALL = -1
"#,
        )
        .unwrap();

        // A generous budget must pass
        let mut pass_cmd = cli_command();
        pass_cmd
            .env("PYTHONPATH", &python_dir)
            .args(["-m", "budgetmod", "--warning-budget", "100"])
            .assert()
            .success();

        // A budget of 0 must fail because the module generates at least one warning
        let mut fail_cmd = cli_command();
        fail_cmd
            .env("PYTHONPATH", &python_dir)
            .args(["-m", "budgetmod", "--warning-budget", "0"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("warning budget"));
    }

    // --- GAP #23: --ir end-to-end tests ---

    /// Minimal valid SchemaIr JSON: one module ("mymod") with one class ("mymod.MyClass")
    /// and one free function ("mymod.greet").  Written to a temp file and fed to the CLI
    /// via `--ir`; no live Python introspection required.
    fn minimal_ir_json() -> String {
        serde_json::json!({
            "modules": [{
                "name": "mymod",
                "is_package": false,
                "classes": [{
                    "name": "mymod.MyClass",
                    "base_classes": [],
                    "methods": [{
                        "name": "mymod.MyClass.__init__",
                        "kind": { "Method": { "method_type": "Constructor" } },
                        "params": [
                            { "name": "self", "kind": "PositionalOrKeyword", "annotation": "", "has_default": false }
                        ],
                        "return_type": "",
                        "docstring": null
                    }]
                }],
                "functions": [{
                    "name": "mymod.greet",
                    "kind": "Function",
                    "params": [
                        { "name": "name", "kind": "PositionalOrKeyword", "annotation": "str", "has_default": false }
                    ],
                    "return_type": "str",
                    "docstring": null
                }]
            }]
        })
        .to_string()
    }

    #[cfg(unix)]
    #[test]
    fn test_cli_rejects_ir_with_dotdot_after_file_symlink() {
        use std::os::unix::fs::symlink;

        let temp_dir = temp_dir("rejects_ir_with_dotdot_after_file_symlink");
        let request_dir = temp_dir.join("request");
        let target_dir = temp_dir.join("target");
        fs::create_dir_all(&request_dir).unwrap();
        fs::create_dir_all(&target_dir).unwrap();
        fs::write(target_dir.join("file"), "not a directory").unwrap();
        fs::write(target_dir.join("schema.json"), minimal_ir_json()).unwrap();
        symlink(target_dir.join("file"), request_dir.join("link")).unwrap();

        let requested_ir = request_dir.join("link").join("..").join("schema.json");
        let report = temp_dir.join("report.json");
        let mut cmd = cli_command();
        cmd.arg("--ir")
            .arg(&requested_ir)
            .arg("--report")
            .arg(&report)
            .assert()
            .failure()
            .stdout(predicate::str::is_empty())
            .stderr(
                predicate::str::contains("ERROR[artifact_revalidation_failed]")
                    .and(predicate::str::contains("Failed to read IR file")),
            );

        let report_json: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(report).unwrap()).unwrap();
        assert!(report_json["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|diagnostic| diagnostic["code"] == "artifact_revalidation_failed"));
    }

    #[cfg(unix)]
    #[test]
    fn test_cli_accepts_ir_with_dotdot_after_directory_symlink() {
        use std::os::unix::fs::symlink;

        let temp_dir = temp_dir("accepts_ir_with_dotdot_after_directory_symlink");
        let request_dir = temp_dir.join("request");
        let target_dir = temp_dir.join("target");
        let deep_dir = target_dir.join("deep");
        fs::create_dir_all(&request_dir).unwrap();
        fs::create_dir_all(&deep_dir).unwrap();
        fs::write(target_dir.join("schema.json"), minimal_ir_json()).unwrap();
        symlink(&deep_dir, request_dir.join("link")).unwrap();

        let requested_ir = request_dir.join("link").join("..").join("schema.json");
        let mut cmd = cli_command();
        cmd.arg("--ir")
            .arg(&requested_ir)
            .assert()
            .success()
            .stdout(predicate::str::contains("pub mod mymod"));
    }

    #[test]
    fn test_cli_ir_generates_rust_to_stdout() {
        // Arrange: write a minimal SchemaIr JSON to a temp file
        let temp_dir = temp_dir("ir_generates_rust_to_stdout");
        let ir_path = temp_dir.join("schema.json");
        fs::write(&ir_path, minimal_ir_json()).unwrap();

        // Act: invoke CLI with --ir (no --module-name, no Python required)
        let mut cmd = cli_command();
        let assert = cmd.arg("--ir").arg(&ir_path).assert();

        // Assert: exits 0 and stdout contains the expected Rust module and items
        assert
            .success()
            .stdout(predicate::str::contains("pub mod mymod"))
            .stdout(predicate::str::contains("pub struct MyClass"))
            .stdout(predicate::str::contains("pub fn greet"));
    }

    #[test]
    fn test_cli_ir_writes_output_file() {
        // Arrange
        let temp_dir = temp_dir("ir_writes_output_file");
        let ir_path = temp_dir.join("schema.json");
        let output_path = temp_dir.join("bindings.rs");
        fs::write(&ir_path, minimal_ir_json()).unwrap();

        // Act
        let mut cmd = cli_command();
        cmd.arg("--ir")
            .arg(&ir_path)
            .arg("--output")
            .arg(&output_path)
            .assert()
            .code(expected_file_publication_exit_code());

        // Assert: output file exists and contains expected Rust items
        let bindings = fs::read_to_string(&output_path).unwrap();
        assert!(bindings.contains("pub mod mymod"), "expected pub mod mymod");
        assert!(
            bindings.contains("pub struct MyClass"),
            "expected MyClass wrapper"
        );
        assert!(bindings.contains("pub fn greet"), "expected greet fn");
    }

    #[test]
    fn test_cli_current_ir_envelope_preserves_legacy_generated_bytes() {
        let temp_dir = temp_dir("current_ir_envelope_preserves_legacy_bytes");
        let legacy_path = temp_dir.join("legacy.json");
        let current_path = temp_dir.join("current.json");
        let legacy_output = temp_dir.join("legacy.rs");
        let current_output = temp_dir.join("current.rs");
        let legacy: serde_json::Value = serde_json::from_str(&minimal_ir_json()).unwrap();
        fs::write(&legacy_path, serde_json::to_vec(&legacy).unwrap()).unwrap();
        fs::write(
            &current_path,
            serde_json::to_vec(&serde_json::json!({
                "schema_version": 1,
                "schema": legacy,
                "extensions": {"producer": {"name": "test"}}
            }))
            .unwrap(),
        )
        .unwrap();

        cli_command()
            .arg("--ir")
            .arg(&legacy_path)
            .arg("--output")
            .arg(&legacy_output)
            .assert()
            .code(expected_file_publication_exit_code());
        cli_command()
            .arg("--ir")
            .arg(&current_path)
            .arg("--output")
            .arg(&current_output)
            .assert()
            .code(expected_file_publication_exit_code());

        assert_eq!(
            fs::read(legacy_output).unwrap(),
            fs::read(current_output).unwrap()
        );
    }

    #[test]
    fn test_cli_legacy_unknown_field_warns_and_obeys_warning_policy() {
        let temp_dir = temp_dir("legacy_unknown_field_warning_policy");
        let ir_path = temp_dir.join("legacy.json");
        let output_path = temp_dir.join("bindings.rs");
        let report_path = temp_dir.join("report.json");
        let mut legacy: serde_json::Value = serde_json::from_str(&minimal_ir_json()).unwrap();
        legacy["modules"][0]["classes"][0]["legacy_typo"] = serde_json::json!(true);
        fs::write(&ir_path, serde_json::to_vec(&legacy).unwrap()).unwrap();

        cli_command()
            .arg("--ir")
            .arg(&ir_path)
            .arg("--output")
            .arg(&output_path)
            .arg("--report")
            .arg(&report_path)
            .assert()
            .code(expected_file_publication_exit_code());
        let report: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&report_path).unwrap()).unwrap();
        let warning = report["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .find(|diagnostic| diagnostic["code"] == "ir_legacy_unknown_field")
            .expect("legacy unknown field must be reported");
        assert_eq!(warning["path"], "modules.0.classes.0.legacy_typo");

        let denied_output = temp_dir.join("denied.rs");
        let denied_report = temp_dir.join("denied-report.json");
        cli_command()
            .arg("--ir")
            .arg(&ir_path)
            .arg("--deny-warnings")
            .arg("--output")
            .arg(&denied_output)
            .arg("--report")
            .arg(&denied_report)
            .assert()
            .failure();
        assert!(!denied_output.exists());
        let denied: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(denied_report).unwrap()).unwrap();
        assert!(denied["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|diagnostic| diagnostic["code"] == "warning_policy"));
    }

    #[test]
    fn test_cli_current_ir_rejects_unknown_field_before_output() {
        let temp_dir = temp_dir("current_ir_rejects_unknown_field");
        let ir_path = temp_dir.join("current.json");
        let output_path = temp_dir.join("bindings.rs");
        let report_path = temp_dir.join("report.json");
        let mut schema: serde_json::Value = serde_json::from_str(&minimal_ir_json()).unwrap();
        schema["modules"][0]["current_typo"] = serde_json::json!(true);
        fs::write(
            &ir_path,
            serde_json::to_vec(&serde_json::json!({
                "schema_version": 1,
                "schema": schema,
                "extensions": {}
            }))
            .unwrap(),
        )
        .unwrap();

        cli_command()
            .arg("--ir")
            .arg(&ir_path)
            .arg("--output")
            .arg(&output_path)
            .arg("--report")
            .arg(&report_path)
            .assert()
            .failure();
        assert!(!output_path.exists());
        let report: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(report_path).unwrap()).unwrap();
        let error = report["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .find(|diagnostic| diagnostic["code"] == "ir_unknown_field")
            .expect("strict current-version error must be structured");
        assert_eq!(error["path"], "schema.modules.0.current_typo");
    }

    #[test]
    fn test_cli_future_ir_version_fails_before_lowering() {
        let temp_dir = temp_dir("future_ir_version_fails_before_lowering");
        let ir_path = temp_dir.join("future.json");
        let output_path = temp_dir.join("bindings.rs");
        let report_path = temp_dir.join("report.json");
        fs::write(
            &ir_path,
            serde_json::to_vec(&serde_json::json!({
                "schema_version": 2,
                "schema": {"not_a_schema": true},
                "extensions": {}
            }))
            .unwrap(),
        )
        .unwrap();

        cli_command()
            .arg("--ir")
            .arg(&ir_path)
            .arg("--output")
            .arg(&output_path)
            .arg("--report")
            .arg(&report_path)
            .assert()
            .failure();
        assert!(!output_path.exists());
        let report: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(report_path).unwrap()).unwrap();
        let error = report["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .find(|diagnostic| diagnostic["code"] == "ir_unsupported_schema_version")
            .expect("future-version error must be structured");
        assert_eq!(error["path"], "schema_version");
    }

    #[test]
    fn test_checked_in_current_ir_schema_is_strict_and_complete() {
        let schema_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../schemas/schema_ir.v1.json");
        let schema: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(schema_path).unwrap()).unwrap();

        assert_eq!(
            schema["$schema"],
            "https://json-schema.org/draft/2020-12/schema"
        );
        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(schema["properties"]["schema_version"]["const"], 1);
        assert_eq!(
            schema["properties"]["extensions"]["additionalProperties"],
            true
        );
        assert_eq!(
            schema["required"],
            serde_json::json!(["schema_version", "schema"])
        );

        let definitions = schema["$defs"].as_object().unwrap();
        let expected = [
            "ClassIr",
            "EnumAliasIr",
            "EnumIr",
            "EnumKindIr",
            "EnumVariantIr",
            "FunctionIr",
            "FunctionKindIr",
            "ImportIr",
            "MethodKindIr",
            "ModuleIr",
            "ParameterIr",
            "ParameterKindIr",
            "PropertyIr",
            "SchemaIr",
        ];
        assert_eq!(definitions.len(), expected.len());
        for name in expected {
            assert!(
                definitions.contains_key(name),
                "missing schema definition {name}"
            );
        }
        for name in [
            "SchemaIr",
            "ModuleIr",
            "ClassIr",
            "FunctionIr",
            "ParameterIr",
            "PropertyIr",
            "EnumIr",
            "EnumVariantIr",
            "EnumAliasIr",
            "ImportIr",
        ] {
            assert_eq!(
                definitions[name]["additionalProperties"], false,
                "{name} must reject unknown fields in current-version input"
            );
        }
    }

    #[test]
    fn test_cli_ir_enforces_raw_byte_limit_before_parsing() {
        let temp_dir = temp_dir("ir_enforces_raw_byte_limit");
        let ir_path = temp_dir.join("schema.json");
        let output_path = temp_dir.join("bindings.rs");
        let report_path = temp_dir.join("report.json");
        let json = minimal_ir_json();
        fs::write(&ir_path, &json).unwrap();

        let mut cmd = cli_command();
        cmd.arg("--ir")
            .arg(&ir_path)
            .arg("--max-ir-bytes")
            .arg(json.len().saturating_sub(1).to_string())
            .arg("--output")
            .arg(&output_path)
            .arg("--report")
            .arg(&report_path)
            .assert()
            .failure()
            .stderr(predicate::str::contains("exceeds the configured limit"));

        assert!(
            !output_path.exists(),
            "oversized input must not publish output"
        );
        let report: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(report_path).unwrap()).unwrap();
        assert!(report["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|diagnostic| diagnostic["code"] == "ir_input_too_large"));
    }

    #[test]
    fn test_cli_ir_report_records_exact_source_sha256() {
        let temp_dir = temp_dir("ir_report_records_exact_source_sha256");
        let ir_path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/minimal_ir.json");
        let report_path = temp_dir.join("report.json");

        let mut cmd = cli_command();
        cmd.arg("--ir")
            .arg(&ir_path)
            .arg("--report")
            .arg(&report_path)
            .assert()
            .code(expected_file_publication_exit_code());

        let report: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(report_path).unwrap()).unwrap();
        assert_eq!(
            report["modules"][0]["source_hash"],
            "sha256:615c9eba3bf76cba6ab6d411413a9e32d131386ebbf5d45e8364e17146c2d710"
        );
        assert_eq!(
            report["modules"][0]["input_hash"],
            "fnv1a64:6ec329dfa8ae92a6"
        );
    }

    #[test]
    fn test_cli_empty_ir_failure_report_keeps_document_provenance() {
        let temp_dir = temp_dir("empty_ir_failure_report_keeps_document_provenance");
        let ir_path = temp_dir.join("empty.json");
        let report_path = temp_dir.join("report.json");
        fs::write(&ir_path, br#"{"modules":[]}"#).unwrap();

        let mut cmd = cli_command();
        cmd.arg("--ir")
            .arg(&ir_path)
            .arg("--report")
            .arg(&report_path)
            .assert()
            .failure();

        let report: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(report_path).unwrap()).unwrap();
        let provenance = report["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .find(|diagnostic| diagnostic["code"] == "external_ir_input")
            .expect("failure report must retain document provenance");
        assert_eq!(provenance["severity"], "info");
        assert_eq!(
            provenance["path"],
            "sha256:ce82d1c4a7e037dbe9609e03c3a605dd9f5d5e726048fa51185b27e5fdf6eb55"
        );
        assert!(provenance["message"].as_str().unwrap().contains("14 bytes"));
        assert!(
            provenance["message"]
                .as_str()
                .unwrap()
                .contains("67108864-byte acquisition limit"),
            "failure provenance must retain the bound applied by the source adapter"
        );
    }

    #[test]
    fn test_cli_ir_missing_file_exits_nonzero() {
        // Arrange: path that does not exist
        let temp_dir = temp_dir("ir_missing_file");
        let ir_path = temp_dir.join("nonexistent.json");

        // Act
        let mut cmd = cli_command();
        let assert = cmd.arg("--ir").arg(&ir_path).assert();

        // Assert: non-zero exit with a helpful error message
        assert.code(1).stderr(
            predicate::str::contains("ERROR[input_read_failed]")
                .and(predicate::str::contains("Failed to read IR file")),
        );
    }

    #[test]
    fn test_cli_ir_malformed_json_exits_nonzero() {
        // Arrange: syntactically invalid JSON
        let temp_dir = temp_dir("ir_malformed_json");
        let ir_path = temp_dir.join("bad.json");
        fs::write(&ir_path, "{ not valid json }").unwrap();

        // Act
        let mut cmd = cli_command();
        let assert = cmd.arg("--ir").arg(&ir_path).assert();

        // Assert: non-zero exit with a parse-error message
        assert
            .code(1)
            .stderr(predicate::str::contains("Failed to parse IR file"));
    }

    #[cfg(unix)]
    #[test]
    fn test_cli_broken_stdout_uses_partial_publication_exit_class() {
        use std::{os::fd::OwnedFd, os::unix::net::UnixStream};

        let temp_dir = temp_dir("broken_stdout_partial_exit");
        let report = temp_dir.join("report.json");
        let ir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/minimal_ir.json");
        let (stdout, peer) = UnixStream::pair().unwrap();
        drop(peer);
        let stdout: OwnedFd = stdout.into();

        let result = std::process::Command::new(assert_cmd::cargo::cargo_bin!("pyo3_bindgen"))
            .arg("--ir")
            .arg(ir)
            .arg("--report")
            .arg(&report)
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::piped())
            .output()
            .unwrap();

        assert_eq!(
            result.status.code(),
            Some(3),
            "unexpected stderr: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        let report: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(report).unwrap()).unwrap();
        assert_eq!(report["schema_version"], 1);
        assert_eq!(report["outputs"], serde_json::json!([]));
        assert!(report["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|diagnostic| diagnostic["code"] == "output_publication_uncertain"));
    }

    #[test]
    fn test_cli_ir_with_report() {
        // Arrange
        let temp_dir = temp_dir("ir_with_report");
        let ir_path = temp_dir.join("schema.json");
        let report_path = temp_dir.join("report.json");
        fs::write(&ir_path, minimal_ir_json()).unwrap();

        // Act
        let mut cmd = cli_command();
        let assert = cmd
            .arg("--ir")
            .arg(&ir_path)
            .arg("--report")
            .arg(&report_path)
            .assert()
            .code(expected_file_publication_exit_code());
        let stdout = &assert.get_output().stdout;

        // Assert: report JSON describes the exact bytes delivered to stdout.
        let report_json: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&report_path).unwrap()).unwrap();
        assert_eq!(report_json["schema_version"], 1);
        assert_eq!(report_json["outputs"][0]["kind"], "stdout");
        assert!(report_json["outputs"][0]["path"].is_null());
        assert_eq!(report_json["outputs"][0]["bytes"], stdout.len());
        assert_eq!(
            report_json["outputs"][0]["hash"],
            pyo3_bindgen::stable_hash(stdout)
        );
    }

    #[test]
    fn test_cli_parse_failure_writes_report() {
        // Arrange
        let temp_dir = temp_dir("parse_failure_writes_report");
        let report = temp_dir.join("failure_report.json");

        // Act
        let mut cmd = cli_command();
        let assert = cmd
            .args(["-m", "definitely_missing_bindgen_module", "--report"])
            .arg(&report)
            .assert();

        // Assert
        assert
            .failure()
            .stderr(predicate::str::contains("Failed to parse the content"));
        let report_json: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&report).unwrap()).unwrap();
        assert!(report_json["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|diagnostic| diagnostic["code"] == "module_parse_failed"
                && diagnostic["path"] == "definitely_missing_bindgen_module"));
    }

    #[test]
    fn test_cli_rejects_report_directory_before_live_introspection() {
        let temp_dir = temp_dir("generation_error_is_not_masked_by_report_write_failure");
        let report_directory = temp_dir.join("report-directory");
        fs::create_dir(&report_directory).unwrap();

        // Act
        let mut cmd = cli_command();
        let assert = cmd
            .args(["-m", "definitely_missing_bindgen_module", "--report"])
            .arg(&report_directory)
            .assert();

        assert
            .failure()
            .stderr(predicate::str::contains("is not a regular file"));
        assert!(report_directory.is_dir());
    }
}
