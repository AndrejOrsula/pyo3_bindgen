use std::{fs, io, io::Write, path::PathBuf};

use pyo3_bindgen::{
    publication::Request, ArtifactStatus, DeliveryDurability, DeliveryKind, DeliveryState,
    ExitClass, GenerationReport, OperationCode, OperationFailure, RunOutcome, RustArtifact,
};

#[path = "test_support/bounded_consumer.rs"]
#[allow(dead_code)]
mod bounded_consumer;

fn artifact(source: &str) -> RustArtifact {
    RustArtifact::from_tokens(source.parse().expect("test source should tokenize"))
        .expect("test source should format")
}

fn request(output: Option<PathBuf>, check: bool) -> Request {
    Request::new(None, output, None, check)
}

#[test]
fn finish_rejects_unfinished_requested_deliveries() {
    let outcome = request(None, false).start().unwrap().finish(Vec::new());
    assert_eq!(
        outcome.failure().unwrap().primary.code,
        OperationCode::InvalidArguments
    );
    let workspace = bounded_consumer::ConsumerWorkspace::new("unfinished-deliveries");
    let output = workspace.path().join("bindings.rs");
    let expected = artifact("pub fn expected() {}");
    fs::write(&output, "different").unwrap();
    let mut session = request(Some(output), true).start().unwrap();
    assert_eq!(
        session.check_output(&expected, true).unwrap(),
        ArtifactStatus::Drifted
    );
    assert_eq!(
        session.finish(Vec::new()).failure().unwrap().primary.code,
        OperationCode::InvalidArguments
    );
    let mut session = Request::new(
        None,
        None,
        Some(workspace.path().join("report.json")),
        false,
    )
    .start()
    .unwrap();
    session.write_stdout(&expected, &mut Vec::new()).unwrap();
    let outcome = session.finish(Vec::new());
    assert_eq!(outcome.exit_class(), ExitClass::PartialPublication);
    assert_eq!(
        outcome.failure().unwrap().primary.code,
        OperationCode::InvalidArguments
    );
}

#[test]
fn check_errors_preserve_guard_and_io_codes() {
    let workspace = bounded_consumer::ConsumerWorkspace::new("typed-check-errors");
    let output = workspace.path().join("bindings.rs");
    let expected = artifact("pub fn checked() {}");
    let mut wrong_mode = request(None, false).start().unwrap();
    assert_eq!(
        wrong_mode.check_output(&expected, true).unwrap_err().code,
        OperationCode::InvalidArguments
    );
    let mut missing = request(Some(output.clone()), true).start().unwrap();
    let failure = missing.check_output(&expected, true).unwrap_err();
    assert_eq!(failure.code, OperationCode::OutputCheckFailed);
    assert_eq!(
        missing.check_output(&expected, true).unwrap_err().code,
        OperationCode::InvalidArguments
    );
    assert_eq!(
        missing
            .finish(vec![failure])
            .failure()
            .unwrap()
            .primary
            .code,
        OperationCode::OutputCheckFailed
    );
    fs::write(&output, expected.bytes()).unwrap();
    let mut current = request(Some(output), true).start().unwrap();
    assert_eq!(
        current.check_output(&expected, true).unwrap(),
        ArtifactStatus::Current
    );
    let repeat = current.check_output(&expected, true).unwrap_err();
    assert_eq!(repeat.code, OperationCode::InvalidArguments);
    let outcome: RunOutcome = current.finish(vec![repeat]).into();
    assert!(matches!(outcome.state().output, DeliveryState::Verified(_)));
}

#[test]
fn report_publication_adopts_and_preserves_correlation() {
    let mut report = GenerationReport::empty();
    report.operation_id = Some("op-v1:automatic-correlation".to_owned());
    let mut session = request(None, false).start().unwrap();
    session
        .write_stdout(&artifact("pub fn correlated() {}"), &mut Vec::new())
        .unwrap();
    assert!(session.publish_report(&report).is_none());
    let mut uncorrelated = GenerationReport::empty();
    uncorrelated.operation_id = None;
    session.observe_report(&uncorrelated);
    assert!(session.publish_report_recovery(&uncorrelated).is_none());
    assert_eq!(
        session.finish(Vec::new()).operation_id(),
        "op-v1:automatic-correlation"
    );
}

#[test]
fn rejected_report_adopts_correlation_even_without_safe_delivery() {
    let workspace = bounded_consumer::ConsumerWorkspace::new("rejected-report-correlation");
    for path in [
        None,
        Some(PathBuf::from("\0unsafe")),
        Some(workspace.path().join("report.json")),
    ] {
        let mut report = GenerationReport::empty();
        report.operation_id = Some("op-v1:rejection-correlation".to_owned());
        let outcome = Request::new(None, None, path, false).reject_with_report(
            OperationFailure::new(OperationCode::GenerationFailed, "original failure"),
            report,
        );
        assert_eq!(outcome.operation_id(), "op-v1:rejection-correlation");
        assert_eq!(
            outcome.failure().unwrap().primary.code,
            OperationCode::GenerationFailed
        );
    }
}

#[test]
fn start_rejects_invalid_check_before_unsafe_path_io() {
    let unsafe_report = PathBuf::from("\0report.json");
    let outcome: RunOutcome =
        match Request::new(None, None, Some(unsafe_report.clone()), true).start() {
            Ok(_) => panic!("invalid check arguments must win before path validation"),
            Err(outcome) => outcome.into(),
        };
    assert_eq!(outcome.exit_class(), ExitClass::Failure);
    assert_eq!(
        outcome
            .failure()
            .expect("invalid check must be retained")
            .primary
            .code,
        OperationCode::InvalidArguments
    );
    assert!(matches!(
        &outcome.state().output,
        DeliveryState::NotRequested
    ));
    assert!(matches!(
        &outcome.state().report,
        DeliveryState::NotPublished {
            kind: DeliveryKind::Report,
            path: Some(path),
        } if path == &unsafe_report
    ));
}

#[test]
fn reject_preserves_provisional_state_without_io() {
    let output = PathBuf::from("\0output.rs");
    let report = PathBuf::from("\0report.json");
    let primary = OperationFailure::new(OperationCode::GenerationFailed, "caller rejection");

    let outcome: RunOutcome = Request::new(None, Some(output.clone()), Some(report.clone()), false)
        .reject(primary)
        .into();

    assert_eq!(outcome.exit_class(), ExitClass::Failure);
    assert_eq!(
        outcome
            .failure()
            .expect("rejection must be retained")
            .primary
            .code,
        OperationCode::GenerationFailed
    );
    assert!(matches!(
        &outcome.state().output,
        DeliveryState::NotPublished {
            kind: DeliveryKind::OutputFile,
            path: Some(path),
        } if path == &output
    ));
    assert!(matches!(
        &outcome.state().report,
        DeliveryState::NotPublished {
            kind: DeliveryKind::Report,
            path: Some(path),
        } if path == &report
    ));
}

#[test]
fn reject_with_report_safe_path_delivers_report_but_keeps_caller_primary() {
    let workspace = bounded_consumer::ConsumerWorkspace::new(
        "reject_with_report_safe_path_delivers_report_but_keeps_caller_primary",
    );
    let report_path = workspace.path().join("failure.json");
    let primary = OperationFailure::new(OperationCode::GenerationFailed, "caller primary");

    let outcome: RunOutcome = Request::new(None, None, Some(report_path.clone()), false)
        .reject_with_report(primary, GenerationReport::empty())
        .into();

    assert_eq!(
        outcome
            .failure()
            .expect("caller failure must be retained")
            .primary
            .code,
        OperationCode::GenerationFailed
    );
    assert_eq!(outcome.failure().unwrap().primary.message, "caller primary");
    assert_eq!(
        outcome.exit_class(),
        if cfg!(unix) {
            ExitClass::Failure
        } else {
            ExitClass::PartialPublication
        }
    );
    assert!(matches!(
        &outcome.state().report,
        DeliveryState::Published(receipt)
            if receipt.kind == DeliveryKind::Report
                && receipt.path.as_deref() == Some(report_path.as_path())
    ));
    assert!(report_path.is_file());
    assert!(matches!(
        &outcome.state().output,
        DeliveryState::NotPublished {
            kind: DeliveryKind::StandardOutput,
            path: None,
        }
    ));
}

#[test]
fn reject_with_report_unsafe_path_keeps_primary_and_does_not_write() {
    let workspace = bounded_consumer::ConsumerWorkspace::new(
        "reject_with_report_unsafe_path_keeps_primary_and_does_not_write",
    );
    let collision = workspace.path().join("same-artifact");
    let primary = OperationFailure::new(OperationCode::GenerationFailed, "caller primary");

    let outcome: RunOutcome = Request::new(
        None,
        Some(collision.clone()),
        Some(collision.clone()),
        false,
    )
    .reject_with_report(primary, GenerationReport::empty())
    .into();

    let failure = outcome.failure().expect("caller failure must be retained");
    assert_eq!(failure.primary.code, OperationCode::GenerationFailed);
    assert!(
        failure.related.is_empty(),
        "unsafe recovery must not add a report failure"
    );
    assert_eq!(outcome.exit_class(), ExitClass::Failure);
    assert!(matches!(
        &outcome.state().output,
        DeliveryState::NotPublished {
            kind: DeliveryKind::OutputFile,
            path: Some(path),
        } if path == &collision
    ));
    assert!(matches!(
        &outcome.state().report,
        DeliveryState::NotPublished {
            kind: DeliveryKind::Report,
            path: Some(path),
        } if path == &collision
    ));
    assert!(!collision.exists());
}

#[test]
fn rejected_check_keeps_primary_and_frozen_paths_when_report_lock_is_held() {
    let workspace = bounded_consumer::ConsumerWorkspace::new("rejected-check-report-lock");
    let normal = workspace.path().join("report.json");
    fs::create_dir(workspace.path().join("parent")).unwrap();
    let requested = workspace.path().join("parent/../report.json");
    let _owner = Request::new(None, None, Some(normal.clone()), false)
        .start()
        .unwrap();
    let outcome: RunOutcome = Request::new(None, None, Some(requested), true)
        .reject_with_report(
            OperationFailure::new(OperationCode::InvalidArguments, "invalid check"),
            GenerationReport::empty(),
        )
        .into();
    let failure = outcome.failure().unwrap();
    assert_eq!(failure.primary.code, OperationCode::InvalidArguments);
    assert_eq!(failure.related.len(), 1);
    assert_eq!(failure.related[0].code, OperationCode::ArtifactLockFailed);
    assert_eq!(outcome.exit_class(), ExitClass::Failure);
    assert!(matches!(
        outcome.state().output,
        DeliveryState::NotRequested
    ));
    assert!(matches!(&outcome.state().report,
        DeliveryState::NotPublished { path: Some(path), .. } if path == &normal
    ));
    assert!(!normal.exists());
}

#[test]
fn output_file_checks_track_current_drift_and_acceptance() {
    let workspace = bounded_consumer::ConsumerWorkspace::new(
        "output_file_checks_track_current_drift_and_acceptance",
    );
    let output = workspace.path().join("bindings.rs");
    let expected = artifact("pub fn checked() -> bool { true }");
    let drifted = artifact("pub fn checked() -> bool { false }");
    fs::write(&output, expected.bytes()).expect("current artifact fixture should be writable");

    let mut current = request(Some(output.clone()), true)
        .start()
        .expect("current check request should start");
    assert_eq!(
        current
            .check_output(&expected, true)
            .expect("current output check should succeed"),
        ArtifactStatus::Current
    );
    let current: RunOutcome = current.finish(Vec::new()).into();
    assert_eq!(current.exit_class(), ExitClass::Success);
    assert!(matches!(
        &current.state().output,
        DeliveryState::Verified(receipt)
            if receipt.kind == DeliveryKind::OutputFile
                && receipt.path.as_deref() == Some(output.as_path())
                && receipt.hash == expected.hash()
                && receipt.bytes == expected.len()
                && receipt.durability == DeliveryDurability::NotApplicable
    ));

    let mut accepted_false = request(Some(output.clone()), true)
        .start()
        .expect("accepted=false check request should start");
    assert_eq!(
        accepted_false
            .check_output(&expected, false)
            .expect("accepted=false current check should succeed"),
        ArtifactStatus::Current
    );
    let accepted_false: RunOutcome = accepted_false
        .finish(vec![OperationFailure::new(
            OperationCode::WarningPolicyFailed,
            "caller policy rejected verification",
        )])
        .into();
    assert_eq!(accepted_false.exit_class(), ExitClass::Failure);
    assert!(matches!(
        &accepted_false.state().output,
        DeliveryState::NotPublished {
            kind: DeliveryKind::OutputFile,
            path: Some(path),
        } if path == &output
    ));

    fs::write(&output, drifted.bytes()).expect("drift fixture should be writable");
    let mut drift = request(Some(output.clone()), true)
        .start()
        .expect("drift check request should start");
    assert_eq!(
        drift
            .check_output(&expected, true)
            .expect("drift output check should succeed"),
        ArtifactStatus::Drifted
    );
    let drift: RunOutcome = drift
        .finish(vec![OperationFailure::new(
            OperationCode::OutputCheckFailed,
            "generated output drifted",
        )])
        .into();
    assert_eq!(drift.exit_class(), ExitClass::Failure);
    assert_eq!(
        drift.failure().unwrap().primary.code,
        OperationCode::OutputCheckFailed
    );
    assert!(matches!(
        &drift.state().output,
        DeliveryState::NotPublished {
            kind: DeliveryKind::OutputFile,
            path: Some(path),
        } if path == &output
    ));
}

struct PrefixThenFailWriter {
    bytes: Vec<u8>,
    prefix_len: usize,
    failed: bool,
}

impl Write for PrefixThenFailWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.failed {
            return Err(io::Error::other("injected stdout writer failure"));
        }
        self.failed = true;
        let written = self.prefix_len.min(bytes.len());
        self.bytes.extend_from_slice(&bytes[..written]);
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn stdout_success_and_prefix_failure_have_typed_delivery_outcomes() {
    let generated = artifact("pub fn stdout_value() -> bool { true }");

    let mut success_session = request(None, false)
        .start()
        .expect("stdout request should start");
    let mut stdout = Vec::new();
    success_session
        .write_stdout(&generated, &mut stdout)
        .expect("stdout writer should receive the complete artifact");
    assert_eq!(stdout, generated.bytes());
    let success: RunOutcome = success_session.finish(Vec::new()).into();
    assert_eq!(success.exit_class(), ExitClass::Success);
    assert!(matches!(
        &success.state().output,
        DeliveryState::Published(receipt)
            if receipt.kind == DeliveryKind::StandardOutput
                && receipt.path.is_none()
                && receipt.hash == generated.hash()
                && receipt.bytes == generated.len()
    ));

    let mut failure_session = request(None, false)
        .start()
        .expect("stdout failure request should start");
    let mut writer = PrefixThenFailWriter {
        bytes: Vec::new(),
        prefix_len: 4,
        failed: false,
    };
    let failure = failure_session
        .write_stdout(&generated, &mut writer)
        .expect_err("writer failure after a prefix must be reported");
    assert_eq!(failure.code, OperationCode::OutputPublicationUncertain);
    assert_eq!(writer.bytes, &generated.bytes()[..4]);

    let uncertain: RunOutcome = failure_session.finish(vec![failure]).into();
    assert_eq!(uncertain.exit_class(), ExitClass::PartialPublication);
    assert_eq!(
        uncertain.failure().unwrap().primary.code,
        OperationCode::OutputPublicationUncertain
    );
    assert!(matches!(
        &uncertain.state().output,
        DeliveryState::PossiblyPublished {
            kind: DeliveryKind::StandardOutput,
            path: None,
            staged_path: None,
            message,
        } if message.contains("prefix may have been delivered")
    ));
}

#[test]
fn stdout_revalidation_failure_happens_before_writer() {
    let workspace = bounded_consumer::ConsumerWorkspace::new(
        "stdout_revalidation_failure_happens_before_writer",
    );
    let report_path = workspace.path().join("report.json");
    let mut session = Request::new(None, None, Some(report_path.clone()), false)
        .start()
        .expect("stdout request should start before the target changes");
    fs::create_dir(&report_path).expect("report target mutation should be possible");
    let mut writer = Vec::new();

    let failure = session
        .write_stdout(
            &artifact("pub fn revalidated() -> bool { true }"),
            &mut writer,
        )
        .expect_err("stdout must revalidate captured targets before writing");
    assert_eq!(failure.code, OperationCode::ArtifactRevalidationFailed);
    assert!(
        writer.is_empty(),
        "revalidation failure must precede writer I/O"
    );

    let outcome: RunOutcome = session.finish(vec![failure]).into();
    assert_eq!(outcome.exit_class(), ExitClass::Failure);
    assert_eq!(
        outcome.failure().unwrap().primary.code,
        OperationCode::ArtifactRevalidationFailed
    );
    assert!(matches!(
        &outcome.state().output,
        DeliveryState::NotPublished {
            kind: DeliveryKind::StandardOutput,
            path: None,
        }
    ));
}

#[test]
fn repeated_stdout_attempt_preserves_uncertain_prefix() {
    let generated = artifact("pub fn uncertain_stdout() -> bool { true }");
    let mut session = request(None, false).start().unwrap();
    let mut first_writer = PrefixThenFailWriter {
        bytes: Vec::new(),
        prefix_len: 4,
        failed: false,
    };
    let initial = session
        .write_stdout(&generated, &mut first_writer)
        .unwrap_err();
    let mut second_writer = Vec::new();
    let repeat = session
        .write_stdout(&generated, &mut second_writer)
        .unwrap_err();
    assert_eq!(repeat.code, OperationCode::InvalidArguments);
    assert!(second_writer.is_empty());
    assert_eq!(first_writer.bytes, generated.bytes()[..4]);
    let outcome: RunOutcome = session.finish(vec![initial, repeat]).into();
    assert_eq!(outcome.exit_class(), ExitClass::PartialPublication);
    assert_eq!(
        outcome.failure().unwrap().primary.code,
        OperationCode::OutputPublicationUncertain
    );
    assert!(matches!(
        &outcome.state().output,
        DeliveryState::PossiblyPublished {
            kind: DeliveryKind::StandardOutput,
            ..
        }
    ));
}

#[test]
fn report_failure_is_related_after_caller_primary_failure() {
    let workspace = bounded_consumer::ConsumerWorkspace::new(
        "report_failure_is_related_after_caller_primary_failure",
    );
    let report_path = workspace.path().join("report.json");
    let mut session = Request::new(None, None, Some(report_path.clone()), false)
        .start()
        .expect("report request should start before the target changes");
    fs::create_dir(&report_path).expect("report target mutation should be possible");

    let report_failure = session
        .publish_report(&GenerationReport::empty())
        .expect("report revalidation failure should be returned");
    assert_eq!(
        report_failure.code,
        OperationCode::ArtifactRevalidationFailed
    );
    let primary = OperationFailure::new(OperationCode::GenerationFailed, "caller primary");
    let outcome: RunOutcome = session.finish(vec![primary]).into();

    let failure = outcome.failure().expect("caller failure must be retained");
    assert_eq!(failure.primary.code, OperationCode::GenerationFailed);
    assert_eq!(failure.primary.message, "caller primary");
    assert_eq!(failure.related.len(), 1);
    assert_eq!(failure.related[0], report_failure);
    assert_eq!(outcome.exit_class(), ExitClass::Failure);
    assert!(matches!(
        &outcome.state().report,
        DeliveryState::NotPublished {
            kind: DeliveryKind::Report,
            path: Some(path),
        } if path == &report_path
    ));
}

#[test]
fn observe_report_preserves_operation_id_in_final_outcome() {
    let mut session = request(None, false)
        .start()
        .expect("operation correlation request should start");
    let mut report = GenerationReport::empty();
    report.operation_id = Some("op-v1:facade-publication-test".to_owned());
    session.observe_report(&report);
    session
        .write_stdout(&artifact("pub fn observed() {}"), &mut Vec::new())
        .unwrap();

    let outcome: RunOutcome = session.finish(Vec::new()).into();
    assert_eq!(outcome.exit_class(), ExitClass::Success);
    assert_eq!(outcome.operation_id(), "op-v1:facade-publication-test");
}

#[test]
fn write_stdout_rejects_file_output_without_mutating_writer_or_state() {
    let workspace = bounded_consumer::ConsumerWorkspace::new(
        "write_stdout_rejects_file_output_without_mutating_writer_or_state",
    );
    let output = workspace.path().join("bindings.rs");
    let mut session = request(Some(output.clone()), false)
        .start()
        .expect("file publication request should start");
    let mut writer = Vec::new();

    let failure = session
        .write_stdout(
            &artifact("pub fn generated() -> bool { true }"),
            &mut writer,
        )
        .expect_err("file output must reject stdout delivery");

    assert_eq!(failure.code, OperationCode::InvalidArguments);
    assert!(
        writer.is_empty(),
        "wrong-mode write must not touch the writer"
    );

    let outcome: RunOutcome = session.finish(vec![failure]).into();
    assert_eq!(outcome.exit_class(), ExitClass::Failure);
    assert_eq!(
        outcome
            .failure()
            .expect("rejection must be retained")
            .primary
            .code,
        OperationCode::InvalidArguments
    );
    assert!(matches!(
        &outcome.state().output,
        DeliveryState::NotPublished {
            kind: DeliveryKind::OutputFile,
            path: Some(path),
        } if path == &output
    ));
    assert!(!output.exists());
}

#[test]
fn repeated_output_attempt_preserves_initial_receipt() {
    let workspace = bounded_consumer::ConsumerWorkspace::new(
        "repeated_output_attempt_preserves_initial_receipt",
    );
    let output = workspace.path().join("bindings.rs");
    let first = artifact("pub fn first() -> bool { true }");
    let second = artifact("pub fn second() -> bool { false }");
    let mut session = request(Some(output.clone()), false)
        .start()
        .expect("file publication request should start");

    let initial_failure = session.publish_output(&first).err();
    let repeat = session
        .publish_output(&second)
        .expect_err("a second primary output attempt must be rejected");

    assert_eq!(repeat.code, OperationCode::InvalidArguments);
    assert_eq!(
        fs::read(&output).expect("initial output should remain"),
        first.bytes()
    );

    let mut failures = initial_failure.into_iter().collect::<Vec<_>>();
    failures.push(repeat);
    let outcome: RunOutcome = session.finish(failures).into();
    assert_eq!(outcome.exit_class(), ExitClass::PartialPublication);
    assert!(matches!(
        &outcome.state().output,
        DeliveryState::Published(receipt)
            if receipt.kind == DeliveryKind::OutputFile
                && receipt.path.as_deref() == Some(output.as_path())
                && receipt.hash == first.hash()
                && receipt.bytes == first.len()
    ));
}

#[cfg(feature = "internal-test-hooks")]
#[test]
fn repeated_output_attempt_preserves_initial_uncertain_outcome() {
    let workspace = bounded_consumer::ConsumerWorkspace::new(
        "repeated_output_attempt_preserves_initial_uncertain_outcome",
    );
    let output = workspace.path().join("bindings.rs");
    let first = artifact("pub fn uncertain_first() -> bool { true }");
    let second = artifact("pub fn uncertain_second() -> bool { false }");
    let mut session = request(Some(output.clone()), false)
        .start()
        .expect("file publication request should start");

    pyo3_bindgen::inject_output_publication_failure_once(
        pyo3_bindgen::TestPublicationFailure::DirectorySyncFailed,
    );
    let initial = session
        .publish_output(&first)
        .expect_err("injected durability failure should be returned");
    assert_eq!(initial.code, OperationCode::OutputDurabilityFailed);
    let repeat = session
        .publish_output(&second)
        .expect_err("a repeated output attempt must be rejected");
    assert_eq!(repeat.code, OperationCode::InvalidArguments);
    assert_eq!(
        fs::read(&output).expect("initial output should remain"),
        first.bytes()
    );

    let outcome: RunOutcome = session.finish(vec![initial, repeat]).into();
    assert_eq!(outcome.exit_class(), ExitClass::PartialPublication);
    assert!(matches!(
        &outcome.state().output,
        DeliveryState::Published(receipt)
            if receipt.kind == DeliveryKind::OutputFile
                && receipt.path.as_deref() == Some(output.as_path())
                && receipt.hash == first.hash()
                && receipt.bytes == first.len()
                && matches!(
                    &receipt.durability,
                    DeliveryDurability::DirectorySyncFailed { .. }
                )
    ));
    assert_eq!(
        outcome.failure().unwrap().primary.code,
        OperationCode::OutputDurabilityFailed
    );
    assert_eq!(
        outcome.failure().unwrap().related[0].code,
        OperationCode::InvalidArguments
    );
}
