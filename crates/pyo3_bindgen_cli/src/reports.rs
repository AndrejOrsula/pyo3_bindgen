use crate::args::Args;
use pyo3_bindgen::publication::Session;
use pyo3_bindgen::{
    ArtifactStatus, DiagnosticSeverity, GenerationDiagnostic, GenerationReport, RustArtifact,
};
#[cfg(test)]
use pyo3_bindgen::{PublicationPlan, PublicationPolicy, PublicationSession};

pub(crate) fn attach_external_ir_provenance(
    report: &mut GenerationReport,
    raw_bytes: usize,
    sha256: &str,
    byte_limit: usize,
) {
    report.diagnostics.insert(
        0,
        GenerationDiagnostic {
            severity: DiagnosticSeverity::Info,
            code: "external_ir_input".to_owned(),
            path: Some(sha256.to_owned()),
            message: format!(
                "captured {raw_bytes} bytes of authoritative external IR input under a {byte_limit}-byte acquisition limit"
            ),
        },
    );
}

pub(crate) fn attach_external_ir_warnings(report: &mut GenerationReport, paths: &[String]) {
    report.diagnostics.extend(paths.iter().map(|path| {
        GenerationDiagnostic {
            severity: DiagnosticSeverity::Warning,
            code: "ir_legacy_unknown_field".to_owned(),
            path: Some(path.clone()),
            message:
                "legacy external IR contains an unknown field; it was ignored for v1 compatibility"
                    .to_owned(),
        }
    }));
}

pub(crate) fn apply_warning_policy(args: &Args, report: &mut GenerationReport, failed: &mut bool) {
    let warning_count = report.warning_count();
    if args.deny_warnings && warning_count > 0 {
        tracing::warn!(
            warning_count,
            policy = "deny_warnings",
            "generation warning policy rejected output"
        );
        report.push_error(
            "warning_policy",
            None,
            format!("generation produced {warning_count} warning(s), but --deny-warnings is set"),
        );
        *failed = true;
    }
    if let Some(warning_budget) = args.warning_budget {
        if warning_count > warning_budget {
            tracing::warn!(
                warning_count,
                warning_budget,
                policy = "warning_budget",
                "generation warning policy rejected output"
            );
            report.push_error(
                "warning_budget_exceeded",
                None,
                format!(
                    "generation produced {warning_count} warning(s), exceeding warning budget {warning_budget}"
                ),
            );
            *failed = true;
        }
    }
}

#[cfg(test)]
pub(crate) fn inject_report_write_failure_once() {
    pyo3_bindgen::inject_report_publication_failure_once(
        pyo3_bindgen::TestPublicationFailure::NotPublished,
    );
}

#[cfg(test)]
pub(crate) fn inject_report_directory_sync_failure_once() {
    pyo3_bindgen::inject_report_publication_failure_once(
        pyo3_bindgen::TestPublicationFailure::DirectorySyncFailed,
    );
}

#[cfg(test)]
pub(crate) fn inject_report_platform_unsupported_once() {
    pyo3_bindgen::inject_report_publication_failure_once(
        pyo3_bindgen::TestPublicationFailure::PlatformUnsupported,
    );
}

#[cfg(test)]
pub(crate) fn failure_with_ir_read_report(
    publication: &PublicationSession,
    message: String,
) -> String {
    let mut report = GenerationReport::empty();
    report.push_error("ir_read_failed", None, message.clone());
    match write_report_recovery(publication, &report) {
        Ok(()) => message,
        Err(report_error) => format!("{message}\nAdditionally, {report_error}"),
    }
}

#[cfg(all(test, unix))]
pub(crate) fn revalidate_ir_artifacts_for_read(
    publication: &PublicationSession,
) -> Result<(), String> {
    publication.revalidate().map_err(|err| {
        let ir_path = publication
            .targets()
            .ir()
            .expect("IR revalidation test requires an IR target");
        let message = format!(
            "Failed to read IR file {}: {err}",
            ir_path.requested_path().display()
        );
        let mut report = GenerationReport::empty();
        report.push_error("artifact_revalidation_failed", None, message.clone());
        match write_report_recovery(publication, &report) {
            Ok(()) => message,
            Err(report_error) => format!("{message}\nAdditionally, {report_error}"),
        }
    })
}

pub(crate) fn check_output(
    publication: &mut Session,
    artifact: &RustArtifact,
    report: &mut GenerationReport,
    failed: &mut bool,
) {
    let output = publication.output_path();
    let output_display = output.map_or_else(
        || "<unspecified>".to_owned(),
        |path| path.display().to_string(),
    );
    match publication.check_output(artifact, !*failed) {
        Ok(ArtifactStatus::Current) => {
            tracing::info!(path = %output_display, "generated output is current");
        }
        Ok(ArtifactStatus::Drifted) => {
            tracing::warn!(path = %output_display, "generated output drift detected");
            report.push_error(
                "output_drift",
                Some(output_display.clone()),
                format!("generated output drift detected for {output_display}"),
            );
            *failed = true;
        }
        Err(err) => {
            let err = err.message;
            tracing::error!(
                path = %output_display,
                error = %err,
                "generated output check failed"
            );
            report.push_error(
                "output_missing",
                Some(output_display.clone()),
                format!("failed to read existing generated output {output_display}: {err}"),
            );
            *failed = true;
        }
    }
}

#[cfg(test)]
pub(crate) fn write_report_recovery(
    publication: &PublicationSession,
    report: &GenerationReport,
) -> Result<(), String> {
    let attempt = publication.publish_report_recovery(report);
    attempt
        .failure
        .map_or(Ok(()), |failure| Err(failure.message))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    static TEST_DIRECTORY_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn test_directory(name: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "pyo3_bindgen_report_recovery_{name}_{}_{}",
            std::process::id(),
            TEST_DIRECTORY_COUNTER.fetch_add(1, Ordering::Relaxed),
        ));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        directory
    }

    #[test]
    fn recovery_without_report_path_is_a_noop() {
        let directory = test_directory("no_report");
        let ir = directory.join("schema.json");
        std::fs::write(&ir, "schema").unwrap();
        let publication =
            PublicationPlan::snapshot(PublicationPolicy::cli(), Some(&ir), None, None)
                .unwrap()
                .acquire()
                .unwrap();

        write_report_recovery(&publication, &GenerationReport::empty()).unwrap();
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn recovery_failure_report_keeps_v1_shape_and_empty_outputs() {
        let directory = test_directory("v1_shape");
        let ir = directory.join("schema.json");
        let report_path = directory.join("report.json");
        std::fs::write(&ir, "schema").unwrap();
        let publication = PublicationPlan::snapshot(
            PublicationPolicy::cli(),
            Some(&ir),
            None,
            Some(&report_path),
        )
        .unwrap()
        .acquire()
        .unwrap();

        let resolved_report_path = publication
            .targets()
            .report()
            .unwrap()
            .path()
            .display()
            .to_string();
        let message = failure_with_ir_read_report(&publication, "failed to read schema".to_owned());
        if cfg!(unix) {
            assert_eq!(message, "failed to read schema");
        } else {
            assert_eq!(
                message,
                format!(
                    "failed to read schema\nAdditionally, Report {} was published, but parent directory synchronization is unsupported on this platform",
                    resolved_report_path
                )
            );
        }
        let report: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&report_path).unwrap()).unwrap();
        assert_eq!(report["schema_version"], 1);
        assert_eq!(report["outputs"], serde_json::json!([]));
        assert_eq!(report["diagnostics"][0]["code"], "ir_read_failed");
        let _ = std::fs::remove_dir_all(directory);
    }
}
