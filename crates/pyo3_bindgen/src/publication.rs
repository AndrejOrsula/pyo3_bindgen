//! Command publication with facade-owned request, delivery state, and outcomes.
//!
//! The engine retains path identity, ordered locks, revalidation, atomic replacement, and
//! durability policy. Callers retain generation, diagnostics, and command policy. This interface
//! uses CLI durability semantics; it does not change direct artifact writers or `Codegen::build`.

use pyo3_bindgen_engine as engine;
use std::{fs::File, io::Write, path::PathBuf};

use engine::{ArtifactStatus, GenerationReport, RustArtifact};
pub use engine::{ExitClass, IrArtifactOpenError, OperationCode, OperationFailure, RunFailure};

/// An owned command request, before path validation or lock acquisition.
pub struct Request {
    ir: Option<PathBuf>,
    output: Option<PathBuf>,
    report: Option<PathBuf>,
    check: bool,
}

impl Request {
    /// Construct a request without filesystem effects. A missing output means standard output
    /// unless `check` is selected; check mode requires an output file.
    pub fn new(
        ir: Option<PathBuf>,
        output: Option<PathBuf>,
        report: Option<PathBuf>,
        check: bool,
    ) -> Self {
        Self {
            ir,
            output,
            report,
            check,
        }
    }

    /// Validate the request, freeze target identities, and acquire the engine's ordered locks.
    pub fn start(self) -> Result<Session, Outcome> {
        if self.check && self.output.is_none() {
            return Err(self.reject(OperationFailure::new(
                OperationCode::InvalidArguments,
                "--check requires --output or --output-dir",
            )));
        }
        let state = self.provisional_state();
        let plan = self.snapshot().map_err(|message| {
            Outcome::failed(
                OperationFailure::new(OperationCode::UnsafeArtifactPath, message),
                state,
            )
        })?;
        let state = frozen_state(&plan, self.check);
        match plan.acquire() {
            Ok(inner) => Ok(Session::new(inner, state, self.check)),
            Err(message) => Err(Outcome::failed(
                OperationFailure::new(OperationCode::ArtifactLockFailed, message),
                state,
            )),
        }
    }

    /// Reject a request without filesystem effects, preserving provisional target spellings.
    pub fn reject(self, primary: OperationFailure) -> Outcome {
        Outcome::failed(primary, self.provisional_state())
    }

    /// Reject a command and attempt a caller-authored recovery report when its targets are safe.
    ///
    /// An unsafe snapshot preserves the original failure without writing. A lock failure is
    /// related to the original failure. This recovery path intentionally permits check mode
    /// without an output, so invalid commands can still report their diagnostics.
    pub fn reject_with_report(
        self,
        primary: OperationFailure,
        report: GenerationReport,
    ) -> Outcome {
        let mut state = self.provisional_state();
        state.set_operation_id(report.operation_id.as_deref());
        if self.report.is_none() {
            return Outcome::failed(primary, state);
        }
        let Ok(plan) = self.snapshot() else {
            return Outcome::failed(primary, state);
        };
        let mut state = frozen_state(&plan, self.check);
        state.set_operation_id(report.operation_id.as_deref());
        match plan.acquire() {
            Ok(inner) => Session::new(inner, state, self.check).finish_failure(primary, report),
            Err(message) => Outcome {
                inner: Box::new(engine::RunOutcome::failed(
                    RunFailure::new(primary).with_related(Some(OperationFailure::new(
                        OperationCode::ArtifactLockFailed,
                        message,
                    ))),
                    state,
                )),
            },
        }
    }

    fn snapshot(&self) -> Result<engine::PublicationPlan, String> {
        engine::PublicationPlan::snapshot(
            engine::PublicationPolicy::cli(),
            self.ir.as_deref(),
            self.output.as_deref(),
            self.report.as_deref(),
        )
    }

    fn provisional_state(&self) -> engine::RunState {
        pending_state(self.output.clone(), self.report.clone(), self.check)
    }
}

fn pending_state(
    output: Option<PathBuf>,
    report: Option<PathBuf>,
    check: bool,
) -> engine::RunState {
    let output = match output {
        Some(path) => engine::DeliveryState::pending(engine::DeliveryKind::OutputFile, Some(path)),
        None if check => engine::DeliveryState::NotRequested,
        None => engine::DeliveryState::pending(engine::DeliveryKind::StandardOutput, None),
    };
    let report = report.map_or(engine::DeliveryState::NotRequested, |path| {
        engine::DeliveryState::pending(engine::DeliveryKind::Report, Some(path))
    });
    engine::RunState::new(output, report)
}

fn frozen_state(plan: &engine::PublicationPlan, check: bool) -> engine::RunState {
    pending_state(
        plan.targets()
            .output()
            .map(|target| target.path().to_owned()),
        plan.targets()
            .report()
            .map(|target| target.path().to_owned()),
        check,
    )
}

/// Owned diagnostic information about the input captured by a request.
pub struct IrTarget {
    requested: PathBuf,
    was_present: bool,
}

impl IrTarget {
    /// The absolute input spelling captured at snapshot time.
    pub fn requested_path(&self) -> &std::path::Path {
        &self.requested
    }
    /// Whether the input existed when its identity was captured.
    pub fn was_present_at_snapshot(&self) -> bool {
        self.was_present
    }
}

/// A command's locked publication session and private delivery state.
///
/// Perform one primary output/check attempt in the requested mode. A repeated or wrong-mode
/// operation fails without replacing the retained delivery evidence. Report recovery may repeat.
pub struct Session {
    inner: engine::PublicationSession,
    state: engine::RunState,
    report_failure: Option<OperationFailure>,
    check: bool,
    output_attempted: bool,
}

impl Session {
    fn new(inner: engine::PublicationSession, state: engine::RunState, check: bool) -> Self {
        Self {
            inner,
            state,
            report_failure: None,
            check,
            output_attempted: false,
        }
    }

    /// Copy input information for diagnostics without lending the publication identity.
    pub fn ir_target(&self) -> Option<IrTarget> {
        self.inner.targets().ir().map(|target| IrTarget {
            requested: target.requested_path().to_owned(),
            was_present: target.was_present_at_snapshot(),
        })
    }

    /// Copy the frozen output path, or return `None` for standard output.
    pub fn output_path(&self) -> Option<PathBuf> {
        self.inner
            .targets()
            .output()
            .map(|target| target.path().to_owned())
    }

    /// Adopt generation correlation for both engine events and the eventual outcome.
    /// A report without an identifier leaves established correlation unchanged.
    pub fn observe_report(&mut self, report: &GenerationReport) {
        if let Some(operation_id) = report.operation_id.as_deref() {
            self.state.set_operation_id(Some(operation_id));
            self.inner.set_operation_id(Some(operation_id));
        }
    }

    /// Revalidate every captured artifact identity.
    pub fn revalidate(&self) -> Result<(), String> {
        self.inner.revalidate()
    }

    /// Open the captured input with the engine's identity checks.
    pub fn open_ir(&self) -> Result<File, IrArtifactOpenError> {
        self.inner.open_ir()
    }

    /// Check output bytes. Record verification only when the caller's other policies accept it.
    pub fn check_output(
        &mut self,
        artifact: &RustArtifact,
        accepted: bool,
    ) -> Result<ArtifactStatus, OperationFailure> {
        self.begin_output(true, true)?;
        let status = self
            .inner
            .check_output(artifact)
            .map_err(|message| OperationFailure::new(OperationCode::OutputCheckFailed, message))?;
        if accepted && status == ArtifactStatus::Current {
            self.state.output = engine::DeliveryState::Verified(receipt(
                artifact,
                engine::DeliveryKind::OutputFile,
                self.output_path(),
            ));
        }
        Ok(status)
    }

    /// Publish the output file and retain its receipt, including uncertain/durability failures.
    pub fn publish_output(&mut self, artifact: &RustArtifact) -> Result<(), OperationFailure> {
        self.begin_output(false, true)?;
        let attempt = self.inner.publish_output(artifact);
        self.state.output = attempt.delivery;
        attempt.failure.map_or(Ok(()), Err)
    }

    /// Whether file or standard-output bytes were positively delivered, even if not durable.
    pub fn output_published(&self) -> bool {
        matches!(self.state.output, engine::DeliveryState::Published(_))
    }

    /// Write to the caller-selected standard-output writer, preserving uncertain-prefix errors.
    pub fn write_stdout<W: Write>(
        &mut self,
        artifact: &RustArtifact,
        writer: &mut W,
    ) -> Result<(), OperationFailure> {
        self.begin_output(false, false)?;
        self.inner.revalidate().map_err(|error| {
            OperationFailure::new(
                OperationCode::ArtifactRevalidationFailed,
                format!("Artifact revalidation failed before output publication: {error}"),
            )
        })?;
        if let Err(error) = writer.write_all(artifact.bytes()) {
            let message = format!(
                "Failed to write generated bindings to stdout; a prefix may have been delivered: {error}"
            );
            self.state.output = engine::DeliveryState::PossiblyPublished {
                kind: engine::DeliveryKind::StandardOutput,
                path: None,
                staged_path: None,
                message: message.clone(),
            };
            return Err(OperationFailure::new(
                OperationCode::OutputPublicationUncertain,
                message,
            ));
        }
        self.state.output = engine::DeliveryState::Published(receipt(
            artifact,
            engine::DeliveryKind::StandardOutput,
            None,
        ));
        Ok(())
    }

    fn begin_output(&mut self, check: bool, file: bool) -> Result<(), OperationFailure> {
        if self.check != check || self.inner.targets().output().is_some() != file {
            return Err(OperationFailure::new(
                OperationCode::InvalidArguments,
                "publication session does not permit this output operation",
            ));
        }
        if self.output_attempted {
            return Err(OperationFailure::new(
                OperationCode::InvalidArguments,
                "publication session already attempted its output operation",
            ));
        }
        self.output_attempted = true;
        Ok(())
    }

    /// Publish the caller's report, replacing the retained report-delivery attempt.
    pub fn publish_report(&mut self, report: &GenerationReport) -> Option<OperationFailure> {
        self.observe_report(report);
        let attempt = self.inner.publish_report(report);
        self.state.report = attempt.delivery;
        self.report_failure = attempt.failure;
        self.report_failure.clone()
    }

    /// Publish a recovery report under the engine's independently-safe report-target policy.
    pub fn publish_report_recovery(
        &mut self,
        report: &GenerationReport,
    ) -> Option<OperationFailure> {
        self.observe_report(report);
        let attempt = self.inner.publish_report_recovery(report);
        self.state.report = attempt.delivery;
        self.report_failure = attempt.failure;
        self.report_failure.clone()
    }

    /// Finalize ordered caller failures, followed by the most recent report failure.
    /// Callers must include failures returned by output/check operations and their own policies.
    /// Empty failures cannot certify incomplete requested deliveries as successful.
    pub fn finish(self, mut failures: Vec<OperationFailure>) -> Outcome {
        failures.extend(self.report_failure);
        if failures.is_empty()
            && (!matches!(
                self.state.output,
                engine::DeliveryState::Published(_) | engine::DeliveryState::Verified(_)
            ) || !matches!(
                self.state.report,
                engine::DeliveryState::NotRequested | engine::DeliveryState::Published(_)
            ))
        {
            failures.push(OperationFailure::new(
                OperationCode::InvalidArguments,
                "publication session finished before requested delivery completed",
            ));
        }
        let inner = if failures.is_empty() {
            engine::RunOutcome::success(self.state)
        } else {
            let primary = failures.remove(0);
            engine::RunOutcome::failed(
                RunFailure {
                    primary,
                    related: failures,
                },
                self.state,
            )
        };
        Outcome {
            inner: Box::new(inner),
        }
    }

    /// Attempt a caller-authored failure report and finalize the original failure first.
    pub fn finish_failure(
        mut self,
        primary: OperationFailure,
        report: GenerationReport,
    ) -> Outcome {
        self.publish_report_recovery(&report);
        self.finish(vec![primary])
    }
}

fn receipt(
    artifact: &RustArtifact,
    kind: engine::DeliveryKind,
    path: Option<PathBuf>,
) -> engine::DeliveryReceipt {
    engine::DeliveryReceipt {
        kind,
        path,
        hash: artifact.hash(),
        bytes: artifact.len(),
        durability: engine::DeliveryDurability::NotApplicable,
    }
}

/// Read-only command outcome with private delivery state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    inner: Box<engine::RunOutcome>,
}

impl Outcome {
    fn failed(primary: OperationFailure, state: engine::RunState) -> Self {
        Self {
            inner: Box::new(engine::RunOutcome::failed(RunFailure::new(primary), state)),
        }
    }
    /// Stable command exit classification.
    pub fn exit_class(&self) -> ExitClass {
        self.inner.exit_class()
    }
    /// Ordered primary and related failures, if any.
    pub fn failure(&self) -> Option<&RunFailure> {
        self.inner.failure()
    }
    /// Generation correlation, or `unavailable` before generation establishes it.
    pub fn operation_id(&self) -> &str {
        self.inner.operation_id()
    }
    /// Preserve the traditional string-error command interface.
    pub fn into_legacy_result(self) -> Result<(), String> {
        self.inner.into_legacy_result()
    }
}

/// Compatibility bridge for the existing detailed CLI return identity.
impl From<Outcome> for engine::RunOutcome {
    fn from(outcome: Outcome) -> Self {
        *outcome.inner
    }
}
