//! CLI tool for automatic generation of Rust FFI bindings to Python modules.

mod args;
mod artifacts;
mod input;
mod locking;
mod name_plan;
mod observability;
mod outcome;
mod reports;

pub use args::{Args, CliArgs};
#[doc(hidden)]
pub use name_plan::run_name_plan;
pub use observability::init_observability;
#[doc(hidden)]
pub use outcome::{
    DeliveryDurability, DeliveryKind, DeliveryReceipt, DeliveryState, ExitClass, OperationCode,
    OperationFailure, RunFailure, RunOutcome, RunState,
};

#[cfg(test)]
use artifacts::{
    artifact_identity_key, artifact_identity_path, artifact_revalidation_identity_matches,
    conservative_casefold_identity_key, validate_windows_artifact_component, ArtifactPath,
    ArtifactPathKind, IrArtifactOpenError,
};
#[cfg(all(test, windows))]
use artifacts::{
    validate_windows_artifact_path, windows_file_id_identity,
    windows_legacy_file_identity_is_sound, windows_lexically_normalized_absolute_path,
};
#[cfg(test)]
use input::external_ir_open_failure;
#[cfg(test)]
use locking::{artifact_lock_keys, artifact_lock_path, open_artifact_lock, LOCK_OWNER_COUNTER};
use pyo3_bindgen::{
    publication::{Outcome, Request, Session},
    DiagnosticSeverity, GenerationReport, OutputMetadata,
};
#[cfg(test)]
use pyo3_bindgen::{PublicationPlan, PublicationPolicy};
#[cfg(all(test, unix))]
use reports::revalidate_ir_artifacts_for_read;
use reports::{
    apply_warning_policy, attach_external_ir_provenance, attach_external_ir_warnings, check_output,
};
#[cfg(test)]
use reports::{
    inject_report_directory_sync_failure_once, inject_report_platform_unsupported_once,
    inject_report_write_failure_once,
};

#[cfg(test)]
use clap::Parser;
#[cfg(all(test, windows))]
use std::io;
#[cfg(test)]
use std::{
    path::{Path, PathBuf},
    sync::atomic::Ordering,
};

pub fn run(args: Args) -> Result<(), String> {
    run_with_process_policy(args, args::DEFAULT_MAX_IR_BYTES, false).into_legacy_result()
}

#[doc(hidden)]
pub fn run_cli(args: CliArgs) -> Result<(), String> {
    run_cli_detailed(args).into_legacy_result()
}

#[doc(hidden)]
pub fn run_cli_detailed(args: CliArgs) -> RunOutcome {
    let (args, max_ir_bytes, strict_completeness) = args.into_parts();
    run_with_process_policy(args, max_ir_bytes, strict_completeness).into()
}

fn run_with_process_policy(args: Args, max_ir_bytes: usize, strict_completeness: bool) -> Outcome {
    let source = if args.ir.is_some() {
        "external_ir"
    } else {
        "live"
    };
    let span = tracing::info_span!(
        "pyo3_bindgen.cli.run",
        observability_schema = 1,
        operation_id = tracing::field::Empty,
        source,
        check = args.check,
        output_requested = args.output.is_some() || args.output_dir.is_some(),
        report_requested = args.report.is_some(),
    );
    let _entered = span.enter();
    tracing::info!(operation_id = "unavailable", "CLI run started");
    let outcome = run_inner(args, max_ir_bytes, strict_completeness);
    span.record("operation_id", outcome.operation_id());
    match outcome.failure() {
        None => tracing::info!(
            operation_id = outcome.operation_id(),
            exit_class = outcome.exit_class().code(),
            operation_code = "ok",
            "CLI run completed"
        ),
        Some(failure) => {
            tracing::error!(
                operation_id = outcome.operation_id(),
                exit_class = outcome.exit_class().code(),
                operation_code = failure.primary.code.as_str(),
                "CLI run failed"
            );
            tracing::debug!(error = %failure.message(), "CLI failure detail");
        }
    }
    outcome
}

fn finish_failure_report(
    publication: Session,
    mut report: GenerationReport,
    primary: OperationFailure,
    diagnostic_code: &'static str,
    path: Option<String>,
) -> Outcome {
    let operation_id = report.operation_id.clone();
    report.outputs.clear();
    report.push_error(diagnostic_code, path, primary.message.clone());
    let outcome = publication.finish_failure(primary, report);
    tracing::error!(
        observability_schema = 1,
        operation_id = operation_id.as_deref().unwrap_or("unavailable"),
        exit_class = outcome.exit_class().code(),
        outcome = "failure",
        operation_code = outcome.failure().unwrap().primary.code.as_str(),
        "publication operation completed"
    );
    outcome
}

#[cfg(test)]
#[derive(Clone, Copy)]
enum InjectedOutputWrite {
    NotPublished,
    DirectorySyncFailed,
    PlatformUnsupported,
}

#[cfg(test)]
fn inject_output_write_once(failure: InjectedOutputWrite) {
    let failure = match failure {
        InjectedOutputWrite::NotPublished => pyo3_bindgen::TestPublicationFailure::NotPublished,
        InjectedOutputWrite::DirectorySyncFailed => {
            pyo3_bindgen::TestPublicationFailure::DirectorySyncFailed
        }
        InjectedOutputWrite::PlatformUnsupported => {
            pyo3_bindgen::TestPublicationFailure::PlatformUnsupported
        }
    };
    pyo3_bindgen::inject_output_publication_failure_once(failure);
}

fn run_inner(args: Args, max_ir_bytes: usize, strict_completeness: bool) -> Outcome {
    let requested_output = match args.output_target() {
        Ok(output) => output,
        Err(message) => {
            return Request::new(args.ir.clone(), None, args.report.clone(), args.check).reject(
                OperationFailure::new(OperationCode::InvalidArguments, message),
            );
        }
    };
    let request = Request::new(
        args.ir.clone(),
        requested_output.clone(),
        args.report.clone(),
        args.check,
    );
    if args.check && requested_output.is_none() {
        let primary = OperationFailure::new(
            OperationCode::InvalidArguments,
            "--check requires --output or --output-dir",
        );
        let mut report = GenerationReport::empty();
        report.push_error("invalid_arguments", None, primary.message.clone());
        return request.reject_with_report(primary, report);
    }
    let mut publication = match request.start() {
        Ok(publication) => publication,
        Err(outcome) => return outcome,
    };
    tracing::debug!("artifact locks acquired");
    // The reusable engine policy returns an error before exposing its successful candidate
    // report. The CLI enforces the same strictness after generation so `--report` can retain
    // every per-origin discovery fact while still failing before artifact publication.
    let cfg = args.config();
    let mut external_ir_source = None;

    // Generate the bindings either from a pre-built IR file (`--ir`) or by live
    // introspection of the modules specified by `--module-name`.
    let mut codegen = pyo3_bindgen::Codegen::new(cfg);
    if let Some(ir_target) = publication.ir_target() {
        let input::ExternalIrSnapshot {
            schema,
            raw_bytes,
            sha256,
            byte_limit,
            compatibility_warnings,
        } = match input::load_external_schema_snapshot(&mut publication, max_ir_bytes) {
            Ok(snapshot) => snapshot,
            Err(failure) => {
                return publication.finish(vec![failure.operation]);
            }
        };
        tracing::debug!(
            input_bytes = raw_bytes.len(),
            input_byte_limit = byte_limit,
            input_sha256 = %sha256,
            "bounded external IR source acquired"
        );
        external_ir_source = Some((raw_bytes, sha256, byte_limit, compatibility_warnings));
        let source_context = {
            let (raw_bytes, sha256, byte_limit, _) = external_ir_source
                .as_ref()
                .expect("bounded external IR facts were just retained");
            pyo3_bindgen::ExternalIrSourceContext::bounded(
                sha256.clone(),
                raw_bytes.len(),
                *byte_limit,
                "cli:bounded_file_snapshot",
            )
        };
        codegen = match source_context.and_then(|source_context| {
            codegen.modules_from_ir_with_source_context(schema, source_context)
        }) {
            Ok(codegen) => codegen,
            Err(err) => {
                let message = format!(
                    "Failed to build bindings from IR file {}:\n{err}",
                    ir_target.requested_path().display()
                );
                let (raw_bytes, sha256, byte_limit, compatibility_warnings) = external_ir_source
                    .as_ref()
                    .expect("external IR provenance is retained before lowering");
                let mut failure_report = GenerationReport::empty();
                attach_external_ir_provenance(
                    &mut failure_report,
                    raw_bytes.len(),
                    sha256,
                    *byte_limit,
                );
                attach_external_ir_warnings(&mut failure_report, compatibility_warnings);
                return finish_failure_report(
                    publication,
                    failure_report,
                    OperationFailure::new(OperationCode::GenerationFailed, message),
                    "ir_build_failed",
                    None,
                );
            }
        };
    } else {
        for module_name in &args.module_names {
            codegen = match codegen.module_name(module_name) {
                Ok(codegen) => codegen,
                Err(err) => {
                    let message = format!(
                        "Failed to parse the content of '{module_name}' Python module:\n{err}"
                    );
                    return finish_failure_report(
                        publication,
                        GenerationReport::empty(),
                        OperationFailure::new(OperationCode::ModuleParseFailed, message),
                        "module_parse_failed",
                        Some(module_name.clone()),
                    );
                }
            };
        }
    }
    let generated = match codegen.generate_artifact_with_report_detailed() {
        Ok(generated) => generated,
        Err(failure) => {
            let (err, mut failure_report) = failure.into_parts();
            let message = format!("Failed to generate bindings for Python modules:\n{err}");
            if let Some((raw_bytes, sha256, byte_limit, compatibility_warnings)) =
                external_ir_source.as_ref()
            {
                attach_external_ir_provenance(
                    &mut failure_report,
                    raw_bytes.len(),
                    sha256,
                    *byte_limit,
                );
                attach_external_ir_warnings(&mut failure_report, compatibility_warnings);
            }
            publication.observe_report(&failure_report);
            return finish_failure_report(
                publication,
                failure_report,
                OperationFailure::new(OperationCode::GenerationFailed, message),
                "generation_failed",
                None,
            );
        }
    };
    publication.observe_report(&generated.report);
    tracing::info!(
        observability_schema = 1,
        operation_id = generated
            .report
            .operation_id
            .as_deref()
            .unwrap_or("unavailable"),
        modules = generated.report.modules.len(),
        diagnostics = generated.report.diagnostics.len(),
        "bindings generated"
    );

    let mut report = generated.report;
    if let Some((raw_bytes, sha256, byte_limit, compatibility_warnings)) =
        external_ir_source.as_ref()
    {
        attach_external_ir_provenance(&mut report, raw_bytes.len(), sha256, *byte_limit);
        attach_external_ir_warnings(&mut report, compatibility_warnings);
        tracing::debug!(bytes = raw_bytes.len(), sha256 = %sha256, "external IR provenance attached");
    }
    // CLI reports describe delivered artifacts, not internal generation stages.
    // Clear token-stream metadata before canonicalization so every later failure
    // report remains explicit that nothing reached its destination.
    report.outputs.clear();
    if strict_completeness {
        let summary = |code: &str| {
            report
                .diagnostics
                .iter()
                .find(|diagnostic| diagnostic.code == code)
                .map(|diagnostic| diagnostic.message.as_str())
                .unwrap_or("attempted=0;emitted=0;skipped=0;failed=0;truncated=0")
        };
        let incomplete = report.diagnostics.iter().any(|diagnostic| {
            matches!(
                diagnostic.code.as_str(),
                "discovery_submodule_skipped"
                    | "discovery_submodule_failed"
                    | "discovery_submodule_truncated"
                    | "discovery_dependency_skipped"
                    | "discovery_dependency_failed"
                    | "discovery_dependency_truncated"
            )
        });
        if incomplete {
            let message = format!(
                "strict completeness failed: submodule={};dependency={}",
                summary("discovery_submodule_summary"),
                summary("discovery_dependency_summary"),
            );
            return finish_failure_report(
                publication,
                report,
                OperationFailure::new(OperationCode::StrictCompletenessFailed, message),
                "strict_completeness_failed",
                None,
            );
        }
    }
    let artifact = generated.artifact;
    let output_path = publication.output_path();
    let output_target = output_path.as_deref();
    let delivered_output = || {
        OutputMetadata::from_path_bytes(
            output_target.map(|_| "file").unwrap_or("stdout"),
            output_target,
            artifact.bytes(),
        )
    };

    let mut failed = false;
    apply_warning_policy(&args, &mut report, &mut failed);
    let warning_failed = failed;
    let mut operation_failures = Vec::new();
    if warning_failed {
        let message = report
            .diagnostics
            .iter()
            .filter(|diagnostic| {
                matches!(
                    diagnostic.code.as_str(),
                    "warning_policy" | "warning_budget_exceeded"
                )
            })
            .map(|diagnostic| diagnostic.message.as_str())
            .collect::<Vec<_>>()
            .join("; ");
        operation_failures.push(OperationFailure::new(
            OperationCode::WarningPolicyFailed,
            message,
        ));
    }

    if args.check {
        if let Err(error) = publication.revalidate() {
            let message = format!("Artifact revalidation failed before output check: {error}");
            return finish_failure_report(
                publication,
                report,
                OperationFailure::new(OperationCode::ArtifactRevalidationFailed, message),
                "artifact_revalidation_failed",
                None,
            );
        }
        let diagnostics_before_check = report.diagnostics.len();
        check_output(&mut publication, &artifact, &mut report, &mut failed);
        if report.diagnostics.len() > diagnostics_before_check {
            let message = report.diagnostics[diagnostics_before_check..]
                .iter()
                .filter(|diagnostic| diagnostic.severity == DiagnosticSeverity::Error)
                .map(|diagnostic| diagnostic.message.as_str())
                .collect::<Vec<_>>()
                .join("; ");
            operation_failures.push(OperationFailure::new(
                OperationCode::OutputCheckFailed,
                message,
            ));
        }
        if !failed {
            report.outputs.push(delivered_output());
        }
    }

    if !args.check && !failed {
        if let Err(error) = publication.revalidate() {
            let message =
                format!("Artifact revalidation failed before output publication: {error}");
            return finish_failure_report(
                publication,
                report,
                OperationFailure::new(OperationCode::ArtifactRevalidationFailed, message),
                "artifact_revalidation_failed",
                None,
            );
        }
        // Write the bindings to a file if the `--output` argument is provided
        if let Some(output) = output_target {
            if let Err(failure) = publication.publish_output(&artifact) {
                match failure.code {
                    OperationCode::OutputDurabilityFailed => {
                        report.push_error(
                            "output_durability_failed",
                            Some(output.display().to_string()),
                            failure.message.clone(),
                        );
                        operation_failures.push(failure);
                        failed = true;
                    }
                    OperationCode::ArtifactRevalidationFailed => {
                        return finish_failure_report(
                            publication,
                            report,
                            failure,
                            "artifact_revalidation_failed",
                            Some(output.display().to_string()),
                        );
                    }
                    OperationCode::OutputWriteFailed => {
                        return finish_failure_report(
                            publication,
                            report,
                            failure,
                            "output_write_failed",
                            Some(output.display().to_string()),
                        );
                    }
                    OperationCode::OutputPublicationUncertain => {
                        return finish_failure_report(
                            publication,
                            report,
                            failure,
                            "output_publication_uncertain",
                            Some(output.display().to_string()),
                        );
                    }
                    _ => {
                        return finish_failure_report(
                            publication,
                            report,
                            failure,
                            "output_write_failed",
                            Some(output.display().to_string()),
                        );
                    }
                }
            }
            if publication.output_published() {
                report.outputs.push(delivered_output());
            }
        } else {
            if let Err(failure) = publication.write_stdout(&artifact, &mut std::io::stdout()) {
                let diagnostic = if failure.code == OperationCode::ArtifactRevalidationFailed {
                    "artifact_revalidation_failed"
                } else {
                    "output_publication_uncertain"
                };
                return finish_failure_report(publication, report, failure, diagnostic, None);
            }
            report.outputs.push(delivered_output());
        }
        tracing::info!(
            observability_schema = 1,
            operation_id = report.operation_id.as_deref().unwrap_or("unavailable"),
            target_kind = if output_target.is_some() {
                "output_file"
            } else {
                "stdout"
            },
            bytes = artifact.len(),
            "bindings delivered"
        );
    }

    debug_assert_eq!(failed, !operation_failures.is_empty());
    let report_failure = if operation_failures.is_empty() {
        publication.publish_report(&report)
    } else {
        publication.publish_report_recovery(&report)
    };
    if operation_failures.is_empty() {
        if let Some(failure) = report_failure
            .filter(|failure| failure.code == OperationCode::ArtifactRevalidationFailed)
        {
            report.push_error(
                "artifact_revalidation_failed",
                None,
                failure.message.clone(),
            );
            operation_failures.push(failure);
            publication.publish_report_recovery(&report);
        }
    }

    let operation_id = report.operation_id.clone();
    let outcome = publication.finish(operation_failures);
    tracing::info!(
        observability_schema = 1,
        operation_id = operation_id.as_deref().unwrap_or("unavailable"),
        exit_class = outcome.exit_class().code(),
        outcome = if outcome.failure().is_none() {
            "success"
        } else {
            "failure"
        },
        operation_code = outcome
            .failure()
            .map_or("none", |failure| failure.primary.code.as_str()),
        "publication operation completed"
    );
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    fn artifact_test_directory(name: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "pyo3_bindgen_{name}_{}_{}",
            std::process::id(),
            LOCK_OWNER_COUNTER.fetch_add(1, Ordering::Relaxed),
        ));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        // Publication reports the resolved path, so the expected paths derived from this directory
        // must be resolved too. On macOS `env::temp_dir()` sits under `/var`, a symlink to
        // `/private/var`, which otherwise makes every delivery-path assertion fail there alone.
        std::fs::canonicalize(&directory).unwrap_or(directory)
    }

    fn expected_file_publication_exit_class() -> ExitClass {
        if cfg!(unix) {
            ExitClass::Success
        } else {
            ExitClass::PartialPublication
        }
    }

    fn expected_failure_report_exit_class() -> ExitClass {
        if cfg!(unix) {
            ExitClass::Failure
        } else {
            ExitClass::PartialPublication
        }
    }

    fn external_ir_cli_args(output: &Path, report: &Path) -> CliArgs {
        let ir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/minimal_ir.json");
        CliArgs::try_parse_from(vec![
            "pyo3_bindgen".to_owned(),
            "--ir".to_owned(),
            ir.display().to_string(),
            "--output".to_owned(),
            output.display().to_string(),
            "--report".to_owned(),
            report.display().to_string(),
        ])
        .unwrap()
    }

    fn external_ir_report_only_cli_args(report: &Path) -> CliArgs {
        let ir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/minimal_ir.json");
        CliArgs::try_parse_from(vec![
            "pyo3_bindgen".to_owned(),
            "--ir".to_owned(),
            ir.display().to_string(),
            "--report".to_owned(),
            report.display().to_string(),
        ])
        .unwrap()
    }

    fn external_ir_check_cli_args(output: &Path, report: &Path) -> CliArgs {
        let ir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/minimal_ir.json");
        CliArgs::try_parse_from(vec![
            "pyo3_bindgen".to_owned(),
            "--ir".to_owned(),
            ir.display().to_string(),
            "--output".to_owned(),
            output.display().to_string(),
            "--check".to_owned(),
            "--report".to_owned(),
            report.display().to_string(),
        ])
        .unwrap()
    }

    #[test]
    fn invalid_reusable_command_has_typed_validation_failure() {
        let args =
            CliArgs::try_parse_from(["pyo3_bindgen", "--module-name", "math", "--check"]).unwrap();

        let outcome = run_cli_detailed(args);

        assert_eq!(outcome.exit_class(), ExitClass::Failure);
        assert_eq!(
            outcome.failure().unwrap().primary.code,
            OperationCode::InvalidArguments
        );
    }

    #[test]
    fn invalid_check_command_delivers_distinct_safe_failure_report() {
        let directory = artifact_test_directory("typed_invalid_report");
        let report = directory.join("report.json");
        let args = CliArgs::try_parse_from(vec![
            "pyo3_bindgen".to_owned(),
            "--module-name".to_owned(),
            "math".to_owned(),
            "--check".to_owned(),
            "--report".to_owned(),
            report.display().to_string(),
        ])
        .unwrap();

        let outcome = run_cli_detailed(args);

        assert_eq!(outcome.exit_class(), expected_failure_report_exit_class());
        assert_eq!(
            outcome.failure().unwrap().primary.code,
            OperationCode::InvalidArguments
        );
        assert!(matches!(
            outcome.state().report,
            DeliveryState::Published(DeliveryReceipt {
                kind: DeliveryKind::Report,
                ..
            })
        ));
        let report: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&report).unwrap()).unwrap();
        assert_eq!(report["schema_version"], 1);
        assert_eq!(report["outputs"], serde_json::json!([]));
        assert_eq!(report["diagnostics"][0]["code"], "invalid_arguments");
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn successful_command_has_complete_typed_delivery_state() {
        let directory = artifact_test_directory("typed_success");
        let output = directory.join("bindings.rs");
        let report = directory.join("report.json");

        let outcome = run_cli_detailed(external_ir_cli_args(&output, &report));

        assert_eq!(outcome.exit_class(), expected_file_publication_exit_class());
        if cfg!(unix) {
            assert!(outcome.failure().is_none());
        } else {
            assert_eq!(
                outcome.failure().unwrap().primary.code,
                OperationCode::OutputDurabilityFailed
            );
        }
        assert!(matches!(
            outcome.state().output,
            DeliveryState::Published(DeliveryReceipt {
                kind: DeliveryKind::OutputFile,
                ..
            })
        ));
        assert!(matches!(
            outcome.state().report,
            DeliveryState::Published(DeliveryReceipt {
                kind: DeliveryKind::Report,
                ..
            })
        ));
        assert!(output.exists());
        assert!(report.exists());
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn malformed_input_has_typed_parse_failure_and_report_receipt() {
        let directory = artifact_test_directory("typed_input_parse");
        let ir = directory.join("bad.json");
        let output = directory.join("bindings.rs");
        let report = directory.join("report.json");
        std::fs::write(&ir, "{not-json").unwrap();
        let args = CliArgs::try_parse_from(vec![
            "pyo3_bindgen".to_owned(),
            "--ir".to_owned(),
            ir.display().to_string(),
            "--output".to_owned(),
            output.display().to_string(),
            "--report".to_owned(),
            report.display().to_string(),
        ])
        .unwrap();

        let outcome = run_cli_detailed(args);

        assert_eq!(outcome.exit_class(), expected_failure_report_exit_class());
        assert_eq!(
            outcome.failure().unwrap().primary.code,
            OperationCode::InputParseFailed
        );
        assert!(matches!(
            outcome.state().report,
            DeliveryState::Published(DeliveryReceipt {
                kind: DeliveryKind::Report,
                ..
            })
        ));
        assert!(!output.exists());
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn colliding_artifacts_have_typed_unsafe_path_failure() {
        let directory = artifact_test_directory("typed_unsafe_path");
        let collision = directory.join("same.json");

        let outcome = run_cli_detailed(external_ir_cli_args(&collision, &collision));

        assert_eq!(outcome.exit_class(), ExitClass::Failure);
        assert_eq!(
            outcome.failure().unwrap().primary.code,
            OperationCode::UnsafeArtifactPath
        );
        assert!(!collision.exists());
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn lock_contention_has_typed_lock_failure() {
        let directory = artifact_test_directory("typed_lock_failure");
        let output = directory.join("bindings.rs");
        let report = directory.join("report.json");
        let ir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/minimal_ir.json");
        let _publication = PublicationPlan::snapshot(
            PublicationPolicy::cli(),
            Some(&ir),
            Some(&output),
            Some(&report),
        )
        .unwrap()
        .acquire()
        .unwrap();

        let outcome = run_cli_detailed(external_ir_cli_args(&output, &report));

        assert_eq!(outcome.exit_class(), ExitClass::Failure);
        assert_eq!(
            outcome.failure().unwrap().primary.code,
            OperationCode::ArtifactLockFailed
        );
        assert!(!output.exists());
        assert!(!report.exists());
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn warning_policy_has_typed_failure_and_no_output_publication() {
        let directory = artifact_test_directory("typed_warning_policy");
        let ir = directory.join("legacy.json");
        let output = directory.join("bindings.rs");
        let report = directory.join("report.json");
        let mut legacy: serde_json::Value =
            serde_json::from_slice(include_bytes!("../tests/fixtures/minimal_ir.json")).unwrap();
        legacy
            .as_object_mut()
            .unwrap()
            .insert("legacy_unknown".to_owned(), serde_json::Value::Bool(true));
        std::fs::write(&ir, serde_json::to_vec(&legacy).unwrap()).unwrap();
        let args = CliArgs::try_parse_from(vec![
            "pyo3_bindgen".to_owned(),
            "--ir".to_owned(),
            ir.display().to_string(),
            "--output".to_owned(),
            output.display().to_string(),
            "--report".to_owned(),
            report.display().to_string(),
            "--deny-warnings".to_owned(),
        ])
        .unwrap();

        let outcome = run_cli_detailed(args);

        assert_eq!(outcome.exit_class(), expected_failure_report_exit_class());
        assert_eq!(
            outcome.failure().unwrap().primary.code,
            OperationCode::WarningPolicyFailed
        );
        assert!(matches!(
            outcome.state().output,
            DeliveryState::NotPublished { .. }
        ));
        assert!(!output.exists());
        assert!(report.exists());
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn output_failure_with_recovered_report_is_an_ordinary_typed_failure() {
        let directory = artifact_test_directory("typed_output_failure");
        let output = directory.join("bindings.rs");
        let report_path = directory.join("report.json");
        inject_output_write_once(InjectedOutputWrite::NotPublished);

        let outcome = run_cli_detailed(external_ir_cli_args(&output, &report_path));

        assert_eq!(outcome.exit_class(), expected_failure_report_exit_class());
        assert_eq!(
            outcome.failure().unwrap().primary.code,
            OperationCode::OutputWriteFailed
        );
        assert!(matches!(
            outcome.state().output,
            DeliveryState::NotPublished {
                kind: DeliveryKind::OutputFile,
                ..
            }
        ));
        assert!(matches!(
            outcome.state().report,
            DeliveryState::Published(DeliveryReceipt {
                kind: DeliveryKind::Report,
                ..
            })
        ));
        assert!(!output.exists());
        let report: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&report_path).unwrap()).unwrap();
        assert_eq!(report["schema_version"], 1);
        assert_eq!(report["outputs"], serde_json::json!([]));
        assert!(report["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|diagnostic| diagnostic["code"] == "output_write_failed"));
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn output_success_then_report_failure_is_typed_partial_success() {
        let directory = artifact_test_directory("typed_report_failure");
        let output = directory.join("bindings.rs");
        let report = directory.join("report.json");
        inject_report_write_failure_once();

        let outcome = run_cli_detailed(external_ir_cli_args(&output, &report));

        assert_eq!(outcome.exit_class(), ExitClass::PartialPublication);
        let failure = outcome.failure().unwrap();
        if cfg!(unix) {
            assert_eq!(failure.primary.code, OperationCode::ReportWriteFailed);
            assert!(failure.related.is_empty());
        } else {
            assert_eq!(failure.primary.code, OperationCode::OutputDurabilityFailed);
            assert_eq!(failure.related.len(), 1);
            assert_eq!(failure.related[0].code, OperationCode::ReportWriteFailed);
        }
        let DeliveryState::Published(receipt) = &outcome.state().output else {
            panic!("published output identity must survive report failure")
        };
        let output_bytes = std::fs::read(&output).unwrap();
        let expected_output_path = std::fs::canonicalize(&output).unwrap();
        assert_eq!(receipt.kind, DeliveryKind::OutputFile);
        assert_eq!(
            receipt.path.as_deref(),
            Some(expected_output_path.as_path())
        );
        assert_eq!(receipt.bytes, output_bytes.len());
        assert_eq!(receipt.hash, pyo3_bindgen::stable_hash(&output_bytes));
        assert_eq!(
            receipt.durability,
            if cfg!(unix) {
                DeliveryDurability::DirectorySynchronized
            } else {
                DeliveryDurability::PlatformUnsupported
            }
        );
        assert!(matches!(
            outcome.state().report,
            DeliveryState::NotPublished {
                kind: DeliveryKind::Report,
                ..
            }
        ));
        assert!(!report.exists());
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn visible_output_with_directory_sync_failure_is_typed_partial_success() {
        let directory = artifact_test_directory("typed_output_durability");
        let output = directory.join("bindings.rs");
        let report_path = directory.join("report.json");
        inject_output_write_once(InjectedOutputWrite::DirectorySyncFailed);

        let outcome = run_cli_detailed(external_ir_cli_args(&output, &report_path));

        assert_eq!(outcome.exit_class(), ExitClass::PartialPublication);
        assert_eq!(
            outcome.failure().unwrap().primary.code,
            OperationCode::OutputDurabilityFailed
        );
        assert!(matches!(
            outcome.state().output,
            DeliveryState::Published(DeliveryReceipt {
                durability: DeliveryDurability::DirectorySyncFailed { .. },
                ..
            })
        ));
        assert!(output.exists());
        assert!(matches!(
            outcome.state().report,
            DeliveryState::Published(DeliveryReceipt {
                kind: DeliveryKind::Report,
                ..
            })
        ));
        let report: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&report_path).unwrap()).unwrap();
        assert_eq!(report["schema_version"], 1);
        assert_eq!(report["outputs"].as_array().unwrap().len(), 1);
        assert!(report["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|diagnostic| diagnostic["code"] == "output_durability_failed"));
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn visible_report_with_directory_sync_failure_is_typed_partial_success() {
        let directory = artifact_test_directory("typed_report_durability");
        let output = directory.join("bindings.rs");
        let report = directory.join("report.json");
        inject_report_directory_sync_failure_once();

        let outcome = run_cli_detailed(external_ir_cli_args(&output, &report));

        assert_eq!(outcome.exit_class(), ExitClass::PartialPublication);
        let failure = outcome.failure().unwrap();
        if cfg!(unix) {
            assert_eq!(failure.primary.code, OperationCode::ReportDurabilityFailed);
            assert!(failure.related.is_empty());
        } else {
            assert_eq!(failure.primary.code, OperationCode::OutputDurabilityFailed);
            assert_eq!(failure.related.len(), 1);
            assert_eq!(
                failure.related[0].code,
                OperationCode::ReportDurabilityFailed
            );
        }
        assert!(matches!(
            outcome.state().output,
            DeliveryState::Published(DeliveryReceipt {
                kind: DeliveryKind::OutputFile,
                ..
            })
        ));
        let DeliveryState::Published(output_receipt) = &outcome.state().output else {
            panic!("published output identity must survive report durability failure")
        };
        assert_eq!(
            output_receipt.durability,
            if cfg!(unix) {
                DeliveryDurability::DirectorySynchronized
            } else {
                DeliveryDurability::PlatformUnsupported
            }
        );
        assert!(matches!(
            outcome.state().report,
            DeliveryState::Published(DeliveryReceipt {
                kind: DeliveryKind::Report,
                durability: DeliveryDurability::DirectorySyncFailed { .. },
                ..
            })
        ));
        assert!(output.exists());
        assert!(report.exists());
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn visible_output_with_platform_unsupported_is_typed_partial_success() {
        let directory = artifact_test_directory("typed_output_platform_unsupported");
        let output = directory.join("bindings.rs");
        let report = directory.join("report.json");
        inject_output_write_once(InjectedOutputWrite::PlatformUnsupported);

        let outcome = run_cli_detailed(external_ir_cli_args(&output, &report));

        assert_eq!(outcome.exit_class(), ExitClass::PartialPublication);
        assert_eq!(
            outcome.failure().unwrap().primary.code,
            OperationCode::OutputDurabilityFailed
        );
        let output_bytes = std::fs::read(&output).unwrap();
        let DeliveryState::Published(receipt) = &outcome.state().output else {
            panic!("published output identity must survive unsupported durability")
        };
        assert_eq!(receipt.kind, DeliveryKind::OutputFile);
        assert_eq!(receipt.path.as_deref(), Some(output.as_path()));
        assert_eq!(receipt.hash, pyo3_bindgen::stable_hash(&output_bytes));
        assert_eq!(receipt.bytes, output_bytes.len());
        assert_eq!(receipt.durability, DeliveryDurability::PlatformUnsupported);
        assert!(report.exists());
        let report_json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&report).unwrap()).unwrap();
        assert!(report_json["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|diagnostic| diagnostic["code"] == "output_durability_failed"));
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn visible_report_with_platform_unsupported_is_typed_partial_success() {
        let directory = artifact_test_directory("typed_report_platform_unsupported");
        let report = directory.join("report.json");
        inject_report_platform_unsupported_once();

        let outcome = run_cli_detailed(external_ir_report_only_cli_args(&report));

        assert_eq!(outcome.exit_class(), ExitClass::PartialPublication);
        assert_eq!(
            outcome.failure().unwrap().primary.code,
            OperationCode::ReportDurabilityFailed
        );
        assert!(matches!(
            outcome.state().output,
            DeliveryState::Published(DeliveryReceipt {
                kind: DeliveryKind::StandardOutput,
                ..
            })
        ));
        let DeliveryState::Published(receipt) = &outcome.state().report else {
            panic!("published report identity must survive unsupported durability")
        };
        assert_eq!(receipt.kind, DeliveryKind::Report);
        assert_eq!(receipt.path.as_deref(), Some(report.as_path()));
        assert_eq!(receipt.durability, DeliveryDurability::PlatformUnsupported);
        assert!(report.exists());
        let report_bytes = std::fs::read(&report).unwrap();
        assert_eq!(receipt.hash, pyo3_bindgen::stable_hash(&report_bytes));
        assert_eq!(receipt.bytes, report_bytes.len());
        let report_json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&report).unwrap()).unwrap();
        assert_eq!(report_json["outputs"][0]["kind"], "stdout");
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn check_mode_report_only_platform_unsupported_is_typed_partial_success() {
        let directory = artifact_test_directory("typed_check_report_platform_unsupported");
        let output = directory.join("bindings.rs");
        let initial_report = directory.join("initial-report.json");
        let initial = run_cli_detailed(external_ir_cli_args(&output, &initial_report));
        assert!(output.exists());
        drop(initial);

        let report = directory.join("check-report.json");
        inject_report_platform_unsupported_once();
        let outcome = run_cli_detailed(external_ir_check_cli_args(&output, &report));

        assert_eq!(outcome.exit_class(), ExitClass::PartialPublication);
        assert_eq!(
            outcome.failure().unwrap().primary.code,
            OperationCode::ReportDurabilityFailed
        );
        assert!(matches!(
            outcome.state().output,
            DeliveryState::Verified(DeliveryReceipt {
                kind: DeliveryKind::OutputFile,
                durability: DeliveryDurability::NotApplicable,
                ..
            })
        ));
        let DeliveryState::Published(receipt) = &outcome.state().report else {
            panic!("published report identity must survive unsupported durability")
        };
        assert_eq!(receipt.kind, DeliveryKind::Report);
        assert_eq!(receipt.path.as_deref(), Some(report.as_path()));
        assert_eq!(receipt.durability, DeliveryDurability::PlatformUnsupported);
        assert!(output.exists());
        assert!(report.exists());
        let report_bytes = std::fs::read(&report).unwrap();
        assert_eq!(receipt.hash, pyo3_bindgen::stable_hash(&report_bytes));
        assert_eq!(receipt.bytes, report_bytes.len());
        let _ = std::fs::remove_dir_all(directory);
    }

    #[cfg(not(windows))]
    fn replace_existing_test_file(staged: &Path, destination: &Path) {
        std::fs::rename(staged, destination).unwrap();
    }

    #[cfg(windows)]
    fn replace_existing_test_file(staged: &Path, destination: &Path) {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::ReplaceFileW;

        let wide_path = |path: &Path| {
            path.as_os_str()
                .encode_wide()
                .chain(std::iter::once(0))
                .collect::<Vec<_>>()
        };
        let staged = wide_path(staged);
        let destination = wide_path(destination);
        // SAFETY: both paths are NUL-terminated and live for the duration of the call.
        assert_ne!(
            unsafe {
                ReplaceFileW(
                    destination.as_ptr(),
                    staged.as_ptr(),
                    std::ptr::null(),
                    0,
                    std::ptr::null(),
                    std::ptr::null(),
                )
            },
            0,
            "failed to atomically replace test file: {}",
            io::Error::last_os_error()
        );
    }

    #[test]
    fn ir_snapshot_rejects_atomic_same_path_file_replacement() {
        let directory = artifact_test_directory("ir_atomic_replacement");
        let ir_path = directory.join("schema.json");
        let replacement = directory.join("replacement.json");
        std::fs::write(&ir_path, "original").unwrap();
        std::fs::write(&replacement, "replacement").unwrap();
        let snapshot = ArtifactPath::snapshot(ArtifactPathKind::Ir, &ir_path, &directory).unwrap();

        replace_existing_test_file(&replacement, &ir_path);
        assert_eq!(std::fs::read_to_string(&ir_path).unwrap(), "replacement");

        let error = snapshot
            .revalidate()
            .expect_err("revalidation must reject a replacement inode at the same pathname");
        assert!(
            error.contains("changed identity during generation"),
            "unexpected error: {error}"
        );
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn ir_open_rejects_atomic_replacement_after_path_revalidation_and_writes_report() {
        let directory = artifact_test_directory("ir_replacement_after_revalidation");
        let ir_path = directory.join("schema.json");
        let replacement = directory.join("replacement.json");
        let report_path = directory.join("report.json");
        std::fs::write(&ir_path, "original").unwrap();
        std::fs::write(&replacement, "replacement").unwrap();
        let mut publication = Request::new(
            Some(ir_path.clone()),
            None,
            Some(report_path.clone()),
            false,
        )
        .start()
        .unwrap();
        let ir_target = publication.ir_target().unwrap();
        publication.revalidate().unwrap();

        replace_existing_test_file(&replacement, ir_target.requested_path());
        let open_error = publication
            .open_ir()
            .expect_err("the opened replacement must not pass the snapshot identity check");
        assert!(matches!(&open_error, IrArtifactOpenError::Revalidation(_)));
        assert!(
            open_error
                .to_string()
                .contains("changed identity during generation"),
            "unexpected error: {open_error}"
        );

        let failure = external_ir_open_failure(&mut publication, open_error);
        assert_eq!(
            failure.operation.code,
            OperationCode::ArtifactRevalidationFailed
        );
        let report: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&report_path).unwrap()).unwrap();
        assert!(
            report["diagnostics"]
                .as_array()
                .unwrap()
                .iter()
                .any(|diagnostic| diagnostic["code"] == "artifact_revalidation_failed"),
            "unexpected report: {report}"
        );
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn ir_snapshot_rejects_file_created_after_missing_snapshot() {
        let directory = artifact_test_directory("ir_created_after_snapshot");
        let ir_path = directory.join("schema.json");
        let snapshot = ArtifactPath::snapshot(ArtifactPathKind::Ir, &ir_path, &directory).unwrap();
        assert!(!snapshot.was_present_at_snapshot());

        std::fs::write(&ir_path, "{}").unwrap();
        let error = snapshot
            .revalidate()
            .expect_err("an IR missing at snapshot must not be accepted if it appears later");
        assert!(
            error.contains("changed identity during generation"),
            "unexpected error: {error}"
        );
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn artifact_revalidation_uses_exact_path_not_collision_key() {
        let lower = PathBuf::from("parent").join("target").join("bindings.rs");
        let upper = PathBuf::from("parent").join("TARGET").join("bindings.rs");

        assert!(
            !artifact_revalidation_identity_matches(&lower, &upper),
            "case-distinct resolved paths must not revalidate as the frozen destination",
        );
        #[cfg(any(unix, windows))]
        assert_eq!(
            artifact_identity_key(&lower).unwrap(),
            artifact_identity_key(&upper).unwrap(),
            "the broader collision/lock key must exercise the distinction",
        );
    }

    #[test]
    fn windows_artifact_component_rejects_forbidden_characters() {
        for component in ["bad<name.rs", "bad/name.rs"] {
            let component = component.encode_utf16().collect::<Vec<_>>();

            assert_eq!(
                validate_windows_artifact_component(&component),
                Err("contains a Win32-forbidden character")
            );
        }
    }

    #[test]
    fn windows_artifact_component_rejects_control_characters() {
        for control in [0, 1, 0x1f] {
            let component = [b'o' as u16, b'k' as u16, control];

            assert_eq!(
                validate_windows_artifact_component(&component),
                Err("contains a control character")
            );
        }
    }

    #[test]
    fn windows_artifact_component_rejects_trailing_dot_or_space() {
        for component in ["bindings.rs.", "bindings.rs "] {
            let component = component.encode_utf16().collect::<Vec<_>>();

            assert_eq!(
                validate_windows_artifact_component(&component),
                Err("ends in a dot or space")
            );
        }
    }

    #[test]
    fn windows_artifact_component_rejects_leading_space() {
        for component in [" bindings.rs", " NUL.txt"] {
            let component = component.encode_utf16().collect::<Vec<_>>();

            assert_eq!(
                validate_windows_artifact_component(&component),
                Err("starts with a space")
            );
        }
    }

    #[test]
    fn windows_artifact_component_rejects_reserved_device_names() {
        for component in [
            "CON",
            "prn.txt",
            "Aux.tar.gz",
            "NUL.json",
            "NUL .txt",
            "COM1.rs",
            "COM1 .rs",
            "com9",
            "COM¹.rs",
            "com²",
            "COM³.log",
            "LPT1.log",
            "lpt9",
            "LPT¹.rs",
            "LPT¹ .json",
            "lpt²",
            "LPT³.log",
            "CONIN$",
            "conout$.txt",
        ] {
            let component = component.encode_utf16().collect::<Vec<_>>();

            assert_eq!(
                validate_windows_artifact_component(&component),
                Err("uses a reserved DOS device name")
            );
        }

        for component in ["console.rs", "COM0.rs", "COM10.rs", "LPT0", "LPT10"] {
            let component = component.encode_utf16().collect::<Vec<_>>();
            assert_eq!(validate_windows_artifact_component(&component), Ok(()));
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_artifact_path_validates_missing_parent_components() {
        assert!(validate_windows_artifact_path(Path::new(r"valid\invalid.\bindings.rs")).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn windows_artifact_path_rejects_non_filesystem_namespaces() {
        for path in [
            r"\\.\NUL",
            r"\\.\pipe\bindings.rs",
            r"\\?\GLOBALROOT\Device\HarddiskVolume1\bindings.rs",
        ] {
            let error = validate_windows_artifact_path(Path::new(path)).unwrap_err();
            assert!(
                error.contains("unsupported Windows path namespace"),
                "unexpected error for {path}: {error}"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn ir_snapshot_rejects_non_filesystem_namespace_before_lookup() {
        let requested = Path::new(r"\\.\NUL");
        let invocation_directory = std::env::current_dir().unwrap();
        let error = ArtifactPath::snapshot(ArtifactPathKind::Ir, requested, &invocation_directory)
            .err()
            .expect("the device namespace must be rejected");

        assert!(
            error.contains("unsupported Windows path namespace"),
            "unexpected error: {error}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_artifact_path_allows_absolute_disk_and_unc_namespaces() {
        for path in [
            r"C:\generated\bindings.rs",
            r"\\?\C:\generated\bindings.rs",
            r"\\server\share\generated\bindings.rs",
            r"\\?\UNC\server\share\generated\bindings.rs",
        ] {
            assert_eq!(
                validate_windows_artifact_path(Path::new(path)),
                Ok(()),
                "unexpected rejection for {path}"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_verbatim_paths_reject_relative_components() {
        for path in [
            r"\\?\C:\generated\.\bindings.rs",
            r"\\?\C:\generated\..\bindings.rs",
            r"\\?\UNC\server\share\generated\.\bindings.rs",
            r"\\?\UNC\server\share\generated\..\bindings.rs",
        ] {
            let error = validate_windows_artifact_path(Path::new(path)).unwrap_err();
            assert!(
                error.contains("relative component") && error.contains("verbatim"),
                "unexpected error for {path}: {error}"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_ordinary_paths_keep_relative_component_and_separator_semantics() {
        for path in [
            r"C:\generated\.\bindings.rs",
            r"C:\generated\..\bindings.rs",
            r"\\server\share\generated\.\bindings.rs",
            r"\\server\share\generated\..\bindings.rs",
            r"C:\generated/bindings.rs",
            r"\\server\share/generated/bindings.rs",
        ] {
            assert_eq!(
                validate_windows_artifact_path(Path::new(path)),
                Ok(()),
                "unexpected rejection for {path}"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_verbatim_paths_reject_forward_slashes() {
        for path in [
            r"\\?\C:\generated/bindings.rs",
            r"\\?\UNC\server\share\generated/bindings.rs",
        ] {
            let error = validate_windows_artifact_path(Path::new(path)).unwrap_err();
            assert!(
                error.contains("Win32-forbidden character"),
                "unexpected error for {path}: {error}"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_artifact_path_rejects_drive_relative_names() {
        let error =
            validate_windows_artifact_path(Path::new(r"C:generated\bindings.rs")).unwrap_err();
        assert!(
            error.contains("drive-relative"),
            "unexpected error: {error}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_artifact_path_validates_unc_authority_components() {
        for path in [
            r"\\server.\share\bindings.rs",
            r"\\server\share.\bindings.rs",
        ] {
            let error = validate_windows_artifact_path(Path::new(path)).unwrap_err();
            assert!(
                error.contains("UNC") && error.contains("ends in a dot or space"),
                "unexpected error for {path}: {error}"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_artifact_identity_uses_ordinal_upcase_without_loss() {
        use std::ffi::OsString;
        use std::os::windows::ffi::OsStringExt;

        let sigma = Path::new(r"C:\generated\Σ.rs");
        let final_sigma = Path::new(r"C:\generated\ς.rs");
        assert_eq!(
            artifact_identity_key(sigma).unwrap(),
            artifact_identity_key(final_sigma).unwrap()
        );

        let dotted_capital_i = Path::new("C:\\generated\\İ.rs");
        let decomposed_lower_i = Path::new("C:\\generated\\i\u{307}.rs");
        assert_ne!(
            artifact_identity_key(dotted_capital_i).unwrap(),
            artifact_identity_key(decomposed_lower_i).unwrap()
        );

        let first = PathBuf::from(OsString::from_wide(&[
            b'C' as u16,
            b':' as u16,
            b'\\' as u16,
            0xd800,
            b'.' as u16,
            b'r' as u16,
            b's' as u16,
        ]));
        let second = PathBuf::from(OsString::from_wide(&[
            b'C' as u16,
            b':' as u16,
            b'\\' as u16,
            0xd801,
            b'.' as u16,
            b'r' as u16,
            b's' as u16,
        ]));
        assert_ne!(
            artifact_identity_key(&first).unwrap(),
            artifact_identity_key(&second).unwrap()
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_file_identity_preserves_the_full_128_bit_file_id() {
        let lower_half = 0x0123_4567_89ab_cdef_u64.to_le_bytes();
        let mut first = [0_u8; 16];
        first[..8].copy_from_slice(&lower_half);
        let mut second = first;
        second[15] = 1;

        assert_ne!(
            windows_file_id_identity(7, &first),
            windows_file_id_identity(7, &second),
            "ReFS file IDs that share their legacy 64-bit prefix must remain distinct"
        );
        assert_ne!(
            windows_file_id_identity(7, &first),
            windows_file_id_identity(8, &first),
            "the full 64-bit volume serial is part of the identity"
        );
        assert!(windows_legacy_file_identity_is_sound(
            &"NTFS".encode_utf16().collect::<Vec<_>>()
        ));
        assert!(
            !windows_legacy_file_identity_is_sound(&"ReFS".encode_utf16().collect::<Vec<_>>()),
            "the legacy 64-bit fallback must never be used for ReFS"
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_full_path_lexically_collapses_parent_components() {
        let directory = artifact_test_directory("windows_full_path_parent");
        let nested = directory.join("nested");
        std::fs::create_dir_all(&nested).unwrap();

        let normalized =
            windows_lexically_normalized_absolute_path(&nested.join("..").join("bindings.rs"))
                .unwrap();
        assert_eq!(normalized, directory.join("bindings.rs"));

        let _ = std::fs::remove_dir_all(directory);
    }

    #[cfg(windows)]
    #[test]
    fn windows_artifact_identity_folds_parent_before_following_reparse_points() {
        use std::os::windows::fs::symlink_dir;

        let directory = artifact_test_directory("windows_reparse_parent_identity");
        let work = directory.join("work");
        let deep = directory.join("target").join("deep");
        std::fs::create_dir_all(&work).unwrap();
        std::fs::create_dir_all(&deep).unwrap();
        let link = work.join("link");
        if let Err(error) = symlink_dir(&deep, &link) {
            let _ = std::fs::remove_dir_all(directory);
            if error.kind() == io::ErrorKind::PermissionDenied {
                return;
            }
            panic!("failed to create Windows directory symlink fixture: {error}");
        }

        let through_reparse = artifact_identity_path(&link.join("..").join("bindings.rs")).unwrap();
        let lexical_target = artifact_identity_path(&work.join("bindings.rs")).unwrap();
        assert_eq!(
            through_reparse, lexical_target,
            "ordinary Win32 `..` must be consumed before the reparse point is traversed"
        );

        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn test_parser_all() {
        // Arrange
        let input = ["", "-m", "os", "--output", "bindings.rs"];

        // Act
        let args = Args::parse_from(input);

        // Assert
        assert_eq!(args.module_names, ["os"]);
        assert_eq!(args.output, Some("bindings.rs".into()));
    }

    #[test]
    fn test_parser_short() {
        // Arrange
        let input = ["", "-m", "sys"];

        // Act
        let args = Args::parse_from(input);

        // Assert
        assert_eq!(args.module_names, ["sys"]);
    }

    #[test]
    fn test_parser_long() {
        // Arrange
        let input = ["", "--module-name", "io"];

        // Act
        let args = Args::parse_from(input);

        // Assert
        assert_eq!(args.module_names, ["io"]);
    }

    #[test]
    fn test_parser_multiple() {
        // Arrange
        let input = ["", "-m", "os", "sys", "--module-name", "io"];

        // Act
        let args = Args::parse_from(input);

        // Assert
        assert_eq!(args.module_names, ["os", "sys", "io"]);
    }

    #[test]
    fn test_parser_ir_only() {
        // Arrange
        let input = ["", "--ir", "schema.json", "--output", "bindings.rs"];

        // Act
        let args = Args::parse_from(input);

        // Assert
        assert_eq!(args.ir, Some("schema.json".into()));
        assert!(args.module_names.is_empty());
    }

    #[test]
    fn test_parser_ir_and_module_conflict() {
        // Arrange
        let input = ["", "--ir", "schema.json", "-m", "os"];

        // Act
        let result = Args::try_parse_from(input);

        // Assert (`--ir` and `-m` are mutually exclusive)
        assert!(result.is_err());
    }

    #[test]
    fn test_parser_neither_ir_nor_module_errors() {
        // Arrange
        let input = ["", "--output", "bindings.rs"];

        // Act
        let result = Args::try_parse_from(input);

        // Assert (one of `--ir` / `-m` is required)
        assert!(result.is_err());
    }

    #[test]
    fn test_parser_no_inherent_forwarders_and_lenient_ir() {
        // Arrange
        let input = [
            "",
            "--ir",
            "schema.json",
            "--no-inherent-forwarders",
            "--lenient-ir",
        ];

        // Act
        let args = Args::parse_from(input);

        // Assert
        assert!(args.no_inherent_forwarders);
        assert!(args.lenient_ir);
    }

    #[test]
    fn test_parser_lenient_ir_conflicts_with_live_modules() {
        // `--lenient-ir` is external-IR-only, so it cannot combine with live `-m` modules
        // (and without `-m` the `--ir`-less form is rejected by `required_unless_present`).
        let input = ["", "-m", "os", "--lenient-ir"];

        // Act
        let result = Args::try_parse_from(input);

        // Assert
        assert!(result.is_err());
    }

    #[test]
    fn test_parser_enum_class_fallback() {
        // Arrange
        let input = ["", "--ir", "schema.json", "--enum-class-fallback"];

        // Act
        let args = Args::parse_from(input);

        // Assert
        assert!(args.enum_class_fallback);
    }

    #[test]
    fn test_parser_enum_class_fallback_conflicts_with_live_modules() {
        // `--enum-class-fallback` is external-IR-only, so it cannot combine with live `-m`
        // modules (the live path already falls back rather than aborting).
        let input = ["", "-m", "os", "--enum-class-fallback"];

        // Act
        let result = Args::try_parse_from(input);

        // Assert
        assert!(result.is_err());
    }

    #[test]
    fn test_parser_enum_string_backed() {
        // Arrange
        let input = ["", "--ir", "schema.json", "--enum-string-backed"];

        // Act
        let args = Args::parse_from(input);

        // Assert
        assert!(args.enum_string_backed);
    }

    #[test]
    fn test_parser_enum_string_backed_conflicts_with_live_modules() {
        // `--enum-string-backed` is external-IR-only (RNA enum properties are strings, and
        // the live path introspects real Python enum classes), so it cannot combine with
        // live `-m` modules.
        let input = ["", "-m", "os", "--enum-string-backed"];

        // Act
        let result = Args::try_parse_from(input);

        // Assert
        assert!(result.is_err());
    }

    #[test]
    fn ir_snapshot_rejects_directories_and_revalidation_rechecks_file_type() {
        let directory = artifact_test_directory("ir_regular_file_test");
        let ir_path = directory.join("schema.json");

        let directory_error = ArtifactPath::snapshot(ArtifactPathKind::Ir, &directory, &directory)
            .err()
            .expect("a directory must not be accepted as IR");
        assert!(
            directory_error.contains("does not resolve to a regular file"),
            "unexpected error: {directory_error}"
        );

        std::fs::write(&ir_path, "{}").unwrap();
        let snapshot = ArtifactPath::snapshot(ArtifactPathKind::Ir, &ir_path, &directory).unwrap();
        std::fs::remove_file(&ir_path).unwrap();
        std::fs::create_dir(&ir_path).unwrap();

        let revalidation_error = snapshot
            .revalidate()
            .expect_err("revalidation must reject a non-file replacement");
        assert!(
            revalidation_error.contains("does not resolve to a regular file"),
            "unexpected error: {revalidation_error}"
        );
        let _ = std::fs::remove_dir_all(directory);
    }

    #[cfg(unix)]
    #[test]
    fn ir_snapshot_allows_file_symlinks_but_rejects_fifos_without_opening() {
        use std::{
            ffi::CString,
            os::unix::{ffi::OsStrExt, fs::symlink},
        };

        let directory = artifact_test_directory("ir_input_kind_test");
        let target = directory.join("schema.json");
        let alias = directory.join("schema-alias.json");
        let fifo = directory.join("schema.fifo");
        std::fs::write(&target, "{}").unwrap();
        symlink(&target, &alias).unwrap();

        let snapshot = ArtifactPath::snapshot(ArtifactPathKind::Ir, &alias, &directory).unwrap();
        assert_eq!(snapshot.resolved(), target.canonicalize().unwrap());
        snapshot.revalidate().unwrap();

        let fifo_c = CString::new(fifo.as_os_str().as_bytes()).unwrap();
        // SAFETY: `fifo_c` is a NUL-terminated path and the mode is valid.
        assert_eq!(unsafe { libc::mkfifo(fifo_c.as_ptr(), 0o600) }, 0);
        let error = ArtifactPath::snapshot(ArtifactPathKind::Ir, &fifo, &directory)
            .err()
            .expect("a FIFO must not be accepted as IR");
        assert!(
            error.contains("does not resolve to a regular file"),
            "unexpected error: {error}"
        );
        let _ = std::fs::remove_dir_all(directory);
    }

    #[cfg(unix)]
    #[test]
    fn ir_revalidation_rejects_invalid_raw_traversal_with_unchanged_canonical_identity() {
        use std::os::unix::fs::symlink;

        let directory = artifact_test_directory("ir_raw_traversal_revalidation");
        let request = directory.join("request");
        let target = directory.join("target");
        let deep = target.join("deep");
        let link = request.join("link");
        std::fs::create_dir_all(&request).unwrap();
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(target.join("file"), "not a directory").unwrap();
        std::fs::write(target.join("schema.json"), "{}").unwrap();
        symlink(&deep, &link).unwrap();

        let requested_ir = link.join("..").join("schema.json");
        let snapshot =
            ArtifactPath::snapshot(ArtifactPathKind::Ir, &requested_ir, &directory).unwrap();
        let original_identity = snapshot.resolved().to_owned();

        std::fs::remove_file(&link).unwrap();
        symlink(target.join("file"), &link).unwrap();
        assert_eq!(
            artifact_identity_path(&requested_ir).unwrap(),
            original_identity,
            "canonical identity alone cannot expose the invalid raw traversal"
        );

        let error = snapshot
            .revalidate()
            .expect_err("raw traversal through a regular file must be rejected");
        assert!(
            error.contains("Failed to inspect --ir path"),
            "unexpected error: {error}"
        );
        let _ = std::fs::remove_dir_all(directory);
    }

    #[cfg(unix)]
    #[test]
    fn post_open_ir_revalidation_failure_writes_requested_report() {
        use std::os::unix::fs::symlink;

        let directory = artifact_test_directory("ir_post_open_revalidation_report");
        let request = directory.join("request");
        let target = directory.join("target");
        let deep = target.join("deep");
        let link = request.join("link");
        let report = directory.join("report.json");
        std::fs::create_dir_all(&request).unwrap();
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(target.join("file"), "not a directory").unwrap();
        std::fs::write(target.join("schema.json"), "{}").unwrap();
        symlink(&deep, &link).unwrap();

        let requested_ir = link.join("..").join("schema.json");
        let publication = PublicationPlan::snapshot(
            PublicationPolicy::cli(),
            Some(&requested_ir),
            None,
            Some(&report),
        )
        .unwrap()
        .acquire()
        .unwrap();
        let _opened = publication.open_ir().unwrap();

        // Model the path changing after the raw file was opened but before the
        // whole artifact set is revalidated. Canonical identity remains unchanged,
        // while actual OS traversal through a regular file now fails with ENOTDIR.
        std::fs::remove_file(&link).unwrap();
        symlink(target.join("file"), &link).unwrap();
        let error = revalidate_ir_artifacts_for_read(&publication)
            .expect_err("post-open raw-path failure must abort the IR read");
        assert!(
            error.contains("Failed to read IR file") && error.contains("Failed to inspect --ir"),
            "unexpected error: {error}"
        );
        let report = std::fs::read_to_string(&report)
            .expect("the requested IR failure report must be written");
        assert!(
            report.contains("\"code\": \"artifact_revalidation_failed\"")
                && report.contains("Failed to read IR file"),
            "unexpected failure report: {report}"
        );
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn artifact_lock_filename_is_portable() {
        let path = artifact_lock_path(b"artifact identity").unwrap();
        let file_name = path.file_name().unwrap().to_string_lossy();
        assert!(!file_name.contains(':'));
        assert!(file_name.ends_with(".lock"));
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn artifact_lock_keys_share_existing_file_identity_across_hardlinks() {
        let directory = std::env::temp_dir().join(format!(
            "pyo3_bindgen_hardlink_lock_test_{}_{}",
            std::process::id(),
            LOCK_OWNER_COUNTER.fetch_add(1, Ordering::Relaxed),
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let first = directory.join("first.rs");
        let second = directory.join("second.rs");
        std::fs::write(&first, "same inode").unwrap();
        std::fs::hard_link(&first, &second).unwrap();
        let first = ArtifactPath::snapshot(ArtifactPathKind::Output, &first, &directory).unwrap();
        let second = ArtifactPath::snapshot(ArtifactPathKind::Output, &second, &directory).unwrap();

        let first_keys = artifact_lock_keys(&first).unwrap();
        let second_keys = artifact_lock_keys(&second).unwrap();
        assert!(
            first_keys
                .iter()
                .any(|first| second_keys.iter().any(|second| first == second)),
            "hardlink aliases must share at least one cross-process lock key"
        );
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn unix_artifact_identity_conservatively_folds_case_and_unicode_normalization() {
        assert_eq!(
            conservative_casefold_identity_key(Path::new("/generated/Bindings.rs")),
            conservative_casefold_identity_key(Path::new("/generated/bindings.rs"))
        );
        assert_eq!(
            conservative_casefold_identity_key(Path::new("/generated/\u{e9}.rs")),
            conservative_casefold_identity_key(Path::new("/generated/e\u{301}.rs"))
        );
        assert_eq!(
            conservative_casefold_identity_key(Path::new("/generated/\u{3a3}.rs")),
            conservative_casefold_identity_key(Path::new("/generated/\u{3c2}.rs"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn artifact_lock_refuses_symlink_files() {
        use std::os::unix::fs::symlink;

        let directory = std::env::temp_dir().join(format!(
            "pyo3_bindgen_lock_symlink_test_{}_{}",
            std::process::id(),
            LOCK_OWNER_COUNTER.fetch_add(1, Ordering::Relaxed),
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let target = directory.join("target");
        let lock = directory.join("lock");
        std::fs::write(&target, "must stay unchanged").unwrap();
        symlink(&target, &lock).unwrap();

        assert!(open_artifact_lock(&lock).is_err());
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "must stay unchanged",
        );
        let _ = std::fs::remove_dir_all(directory);
    }

    #[cfg(windows)]
    #[test]
    fn artifact_lock_refuses_windows_reparse_points() {
        use std::os::windows::fs::symlink_file;

        let directory = std::env::temp_dir().join(format!(
            "pyo3_bindgen_lock_symlink_test_{}_{}",
            std::process::id(),
            LOCK_OWNER_COUNTER.fetch_add(1, Ordering::Relaxed),
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let target = directory.join("target");
        let lock = directory.join("lock");
        std::fs::write(&target, "must stay unchanged").unwrap();
        if let Err(error) = symlink_file(&target, &lock) {
            let _ = std::fs::remove_dir_all(directory);
            if error.kind() == io::ErrorKind::PermissionDenied {
                return;
            }
            panic!("failed to create Windows symlink fixture: {error}");
        }

        assert!(open_artifact_lock(&lock).is_err());
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "must stay unchanged",
        );
        let _ = std::fs::remove_dir_all(directory);
    }
}
