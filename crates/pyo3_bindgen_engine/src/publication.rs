//! Engine-owned artifact publication.
//!
//! This module is the single coordination boundary for filesystem artifacts. Callers choose
//! a policy and provide paths; identity, locks, revalidation, replacement, durability, report
//! delivery, and typed outcomes stay here.

use crate::{
    artifact::{
        ArtifactDurability, ArtifactStatus, ArtifactWriteFailure, RustArtifact, TextArtifact,
    },
    report::GenerationReport,
};
use std::{
    fmt,
    fs::File,
    io::{self, Seek, SeekFrom, Write},
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// Stable process exit classes used by the command-facing publication boundary.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ExitClass {
    Success = 0,
    Failure = 1,
    Usage = 2,
    PartialPublication = 3,
}

impl ExitClass {
    #[doc(hidden)]
    pub const fn code(self) -> u8 {
        self as u8
    }
}

/// Stable machine-readable category for a publication or command operation.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationCode {
    InvalidArguments,
    UnsafeArtifactPath,
    ArtifactLockFailed,
    InputReadFailed,
    InputLimitExceeded,
    InputParseFailed,
    InputContractFailed,
    ModuleParseFailed,
    GenerationFailed,
    GeneratedRustParseFailed,
    StrictCompletenessFailed,
    WarningPolicyFailed,
    OutputCheckFailed,
    ArtifactRevalidationFailed,
    OutputWriteFailed,
    OutputPublicationUncertain,
    OutputDurabilityFailed,
    ReportSerializationFailed,
    ReportWriteFailed,
    ReportPublicationUncertain,
    ReportDurabilityFailed,
}

impl OperationCode {
    #[doc(hidden)]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidArguments => "invalid_arguments",
            Self::UnsafeArtifactPath => "unsafe_artifact_path",
            Self::ArtifactLockFailed => "artifact_lock_failed",
            Self::InputReadFailed => "input_read_failed",
            Self::InputLimitExceeded => "input_limit_exceeded",
            Self::InputParseFailed => "input_parse_failed",
            Self::InputContractFailed => "input_contract_failed",
            Self::ModuleParseFailed => "module_parse_failed",
            Self::GenerationFailed => "generation_failed",
            Self::GeneratedRustParseFailed => "generated_rust_parse_failed",
            Self::StrictCompletenessFailed => "strict_completeness_failed",
            Self::WarningPolicyFailed => "warning_policy_failed",
            Self::OutputCheckFailed => "output_check_failed",
            Self::ArtifactRevalidationFailed => "artifact_revalidation_failed",
            Self::OutputWriteFailed => "output_write_failed",
            Self::OutputPublicationUncertain => "output_publication_uncertain",
            Self::OutputDurabilityFailed => "output_durability_failed",
            Self::ReportSerializationFailed => "report_serialization_failed",
            Self::ReportWriteFailed => "report_write_failed",
            Self::ReportPublicationUncertain => "report_publication_uncertain",
            Self::ReportDurabilityFailed => "report_durability_failed",
        }
    }
}

/// One typed operation failure.
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationFailure {
    pub code: OperationCode,
    pub message: String,
}

impl OperationFailure {
    #[doc(hidden)]
    pub fn new(code: OperationCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

/// Primary failure plus report/recovery failures.
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunFailure {
    pub primary: OperationFailure,
    pub related: Vec<OperationFailure>,
}

impl RunFailure {
    #[doc(hidden)]
    pub fn new(primary: OperationFailure) -> Self {
        Self {
            primary,
            related: Vec::new(),
        }
    }

    #[doc(hidden)]
    pub fn with_related(mut self, related: Option<OperationFailure>) -> Self {
        self.related.extend(related);
        self
    }

    #[doc(hidden)]
    pub fn message(&self) -> String {
        let mut message = self.primary.message.clone();
        for related in &self.related {
            message.push_str("\nAdditionally, ");
            message.push_str(&related.message);
        }
        message
    }
}

/// Kind of externally visible delivery.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryKind {
    OutputFile,
    StandardOutput,
    Report,
}

/// Durability guarantee associated with a visible delivery.
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeliveryDurability {
    NotApplicable,
    DirectorySynchronized,
    PlatformUnsupported,
    DirectorySyncFailed { message: String },
}

impl DeliveryDurability {
    const fn as_str(&self) -> &'static str {
        match self {
            Self::NotApplicable => "not_applicable",
            Self::DirectorySynchronized => "directory_synchronized",
            Self::PlatformUnsupported => "platform_unsupported",
            Self::DirectorySyncFailed { .. } => "directory_sync_failed",
        }
    }
}

/// Content identity and durability of a verified or visible delivery.
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryReceipt {
    pub kind: DeliveryKind,
    pub path: Option<PathBuf>,
    pub hash: String,
    pub bytes: usize,
    pub durability: DeliveryDurability,
}

/// Complete known state of one requested delivery.
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeliveryState {
    NotRequested,
    NotPublished {
        kind: DeliveryKind,
        path: Option<PathBuf>,
    },
    Verified(DeliveryReceipt),
    Published(DeliveryReceipt),
    PossiblyPublished {
        kind: DeliveryKind,
        path: Option<PathBuf>,
        staged_path: Option<PathBuf>,
        message: String,
    },
}

impl DeliveryState {
    #[doc(hidden)]
    pub fn pending(kind: DeliveryKind, path: Option<PathBuf>) -> Self {
        Self::NotPublished { kind, path }
    }

    fn creates_partial_failure(&self, primary_output: bool) -> bool {
        match self {
            Self::PossiblyPublished { .. } => true,
            Self::Published(receipt) => {
                primary_output
                    || matches!(
                        receipt.durability,
                        DeliveryDurability::DirectorySyncFailed { .. }
                            | DeliveryDurability::PlatformUnsupported
                    )
            }
            Self::NotRequested | Self::NotPublished { .. } | Self::Verified(_) => false,
        }
    }
}

/// Typed output/report state retained on a command outcome.
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunState {
    pub output: DeliveryState,
    pub report: DeliveryState,
    operation_id: String,
}

impl RunState {
    #[doc(hidden)]
    pub fn new(output: DeliveryState, report: DeliveryState) -> Self {
        Self {
            output,
            report,
            operation_id: "unavailable".to_owned(),
        }
    }

    #[doc(hidden)]
    pub fn set_operation_id(&mut self, operation_id: Option<&str>) {
        self.operation_id = operation_id.unwrap_or("unavailable").to_owned();
    }

    #[doc(hidden)]
    pub fn operation_id(&self) -> &str {
        &self.operation_id
    }

    fn creates_partial_failure(&self) -> bool {
        self.output.creates_partial_failure(true) || self.report.creates_partial_failure(false)
    }
}

/// Total typed result of a command publication.
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunOutcome {
    Success {
        state: RunState,
    },
    Failure {
        failure: RunFailure,
        state: RunState,
    },
    PartialSuccess {
        failure: RunFailure,
        state: RunState,
    },
}

impl RunOutcome {
    #[doc(hidden)]
    pub fn success(state: RunState) -> Self {
        Self::Success { state }
    }

    #[doc(hidden)]
    pub fn failed(failure: RunFailure, state: RunState) -> Self {
        if state.creates_partial_failure() {
            Self::PartialSuccess { failure, state }
        } else {
            Self::Failure { failure, state }
        }
    }

    #[doc(hidden)]
    pub const fn exit_class(&self) -> ExitClass {
        match self {
            Self::Success { .. } => ExitClass::Success,
            Self::Failure { .. } => ExitClass::Failure,
            Self::PartialSuccess { .. } => ExitClass::PartialPublication,
        }
    }

    #[doc(hidden)]
    pub const fn state(&self) -> &RunState {
        match self {
            Self::Success { state }
            | Self::Failure { state, .. }
            | Self::PartialSuccess { state, .. } => state,
        }
    }

    #[doc(hidden)]
    pub fn operation_id(&self) -> &str {
        self.state().operation_id()
    }

    #[doc(hidden)]
    pub const fn failure(&self) -> Option<&RunFailure> {
        match self {
            Self::Success { .. } => None,
            Self::Failure { failure, .. } | Self::PartialSuccess { failure, .. } => Some(failure),
        }
    }

    #[doc(hidden)]
    pub fn into_legacy_result(self) -> Result<(), String> {
        match self {
            Self::Success { .. } => Ok(()),
            Self::Failure { failure, .. } | Self::PartialSuccess { failure, .. } => {
                Err(failure.message())
            }
        }
    }
}

/// Explicit caller policy for the shared publication interface.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicationPolicy {
    Cli,
    Codegen,
}

impl PublicationPolicy {
    #[doc(hidden)]
    pub const fn cli() -> Self {
        Self::Cli
    }

    #[doc(hidden)]
    pub const fn codegen() -> Self {
        Self::Codegen
    }
}

/// A planned, identity-frozen publication request.
#[doc(hidden)]
pub struct PublicationPlan {
    policy: PublicationPolicy,
    paths: ArtifactPaths,
}

impl PublicationPlan {
    #[doc(hidden)]
    pub fn snapshot(
        policy: PublicationPolicy,
        ir: Option<&Path>,
        output: Option<&Path>,
        report: Option<&Path>,
    ) -> Result<Self, String> {
        if matches!(policy, PublicationPolicy::Codegen)
            && (ir.is_some() || output.is_none() || report.is_some())
        {
            return Err(
                "Codegen publication requires exactly one output path and no IR or report path"
                    .to_owned(),
            );
        }
        Ok(Self {
            policy,
            paths: ArtifactPaths::snapshot(ir, output, report)?,
        })
    }

    #[doc(hidden)]
    pub fn targets(&self) -> PublicationTargets<'_> {
        PublicationTargets::from_paths(&self.paths)
    }

    #[doc(hidden)]
    pub fn acquire(self) -> Result<PublicationSession, String> {
        let locks = ArtifactLockSet::acquire(&self.paths)?;
        Ok(PublicationSession {
            policy: self.policy,
            paths: self.paths,
            operation_id: None,
            _locks: locks,
        })
    }
}

/// An identity-frozen publication request with its ordered locks held.
#[doc(hidden)]
pub struct PublicationSession {
    policy: PublicationPolicy,
    paths: ArtifactPaths,
    operation_id: Option<String>,
    _locks: ArtifactLockSet,
}

impl PublicationSession {
    #[doc(hidden)]
    pub fn set_operation_id(&mut self, operation_id: Option<&str>) {
        self.operation_id = operation_id.map(str::to_owned);
    }

    #[doc(hidden)]
    pub const fn policy(&self) -> PublicationPolicy {
        self.policy
    }

    #[doc(hidden)]
    pub fn targets(&self) -> PublicationTargets<'_> {
        PublicationTargets::from_paths(&self.paths)
    }

    #[doc(hidden)]
    pub fn revalidate(&self) -> Result<(), String> {
        let result = self.paths.revalidate();
        let targets = self.targets();
        let target_count = usize::from(targets.ir().is_some())
            + usize::from(targets.output().is_some())
            + usize::from(targets.report().is_some());
        tracing::debug!(
            observability_schema = 1,
            operation_id = self.operation_id.as_deref().unwrap_or("unavailable"),
            target_count,
            result = if result.is_ok() { "ok" } else { "failed" },
            "artifact targets revalidated"
        );
        result
    }

    #[doc(hidden)]
    pub fn open_ir(&self) -> Result<File, IrArtifactOpenError> {
        let Some(path) = self.paths.ir.as_ref() else {
            return Err(IrArtifactOpenError::NoPath);
        };
        open_ir_artifact_file_inner(path)
    }

    #[doc(hidden)]
    pub fn check_output(&self, artifact: &RustArtifact) -> Result<ArtifactStatus, String> {
        self.revalidate()?;
        let output = self
            .paths
            .output
            .as_ref()
            .ok_or_else(|| "publication request has no output path".to_owned())?;
        artifact
            .check(output.resolved())
            .map_err(|error| error.to_string())
    }

    #[doc(hidden)]
    pub fn publish_output(&self, artifact: &RustArtifact) -> PublicationAttempt {
        let span = tracing::info_span!(
            "pyo3_bindgen.publication.output",
            observability_schema = 1,
            operation_id = self.operation_id.as_deref().unwrap_or("unavailable"),
            target_kind = "output_file",
        );
        let _entered = span.enter();
        let Some(output) = self.paths.output.as_ref() else {
            return PublicationAttempt::failure(
                DeliveryKind::OutputFile,
                None,
                OperationCode::OutputWriteFailed,
                "publication request has no output path",
            );
        };
        if let Err(error) = self.revalidate() {
            return PublicationAttempt {
                delivery: DeliveryState::pending(
                    DeliveryKind::OutputFile,
                    Some(output.resolved().to_owned()),
                ),
                failure: Some(OperationFailure::new(
                    OperationCode::ArtifactRevalidationFailed,
                    error,
                )),
            };
        }
        let write_result = write_output_artifact(artifact, output.resolved());
        let attempt = output_attempt(
            self.policy,
            output.resolved(),
            artifact,
            write_result,
            OperationCode::OutputWriteFailed,
            OperationCode::OutputPublicationUncertain,
            OperationCode::OutputDurabilityFailed,
        );
        if let DeliveryState::Published(receipt) = &attempt.delivery {
            tracing::info!(
                observability_schema = 1,
                operation_id = self.operation_id.as_deref().unwrap_or("unavailable"),
                target_kind = "output_file",
                outcome = if attempt.failure.is_none() {
                    "published"
                } else {
                    "partially_published"
                },
                bytes = receipt.bytes,
                hash = receipt.hash.as_str(),
                durability = receipt.durability.as_str(),
                "output publication completed"
            );
        }
        attempt
    }

    #[doc(hidden)]
    pub fn publish_report(&self, report: &GenerationReport) -> ReportWriteAttempt {
        self.publish_report_with_recovery(report, false)
    }

    #[doc(hidden)]
    pub fn publish_report_recovery(&self, report: &GenerationReport) -> ReportWriteAttempt {
        self.publish_report_with_recovery(report, true)
    }

    fn publish_report_with_recovery(
        &self,
        report: &GenerationReport,
        recovery: bool,
    ) -> ReportWriteAttempt {
        let attempt = publish_report_inner(self.policy, &self.paths, report, recovery);
        tracing::info!(
            observability_schema = 1,
            operation_id = self.operation_id.as_deref().unwrap_or("unavailable"),
            target_kind = "report",
            recovery,
            result = if attempt.failure.is_none() {
                "published"
            } else {
                "failed"
            },
            "report delivery completed"
        );
        attempt
    }
}

/// Read-only descriptors for the exact targets captured by a publication plan or session.
///
/// Descriptors expose paths for diagnostics and delivery metadata, but do not expose the
/// identity snapshot used by publication operations and cannot be supplied to those operations.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublicationTarget<'a> {
    kind: ArtifactPathKind,
    absolute: &'a Path,
    resolved: &'a Path,
    was_present_at_snapshot: bool,
}

impl<'a> PublicationTarget<'a> {
    #[doc(hidden)]
    pub const fn kind(self) -> ArtifactPathKind {
        self.kind
    }

    /// The absolute spelling captured from the caller's request.
    #[doc(hidden)]
    pub const fn requested_path(&self) -> &'a Path {
        self.absolute
    }

    /// The identity-resolved path used for delivery metadata.
    #[doc(hidden)]
    pub const fn path(&self) -> &'a Path {
        self.resolved
    }

    #[doc(hidden)]
    pub const fn was_present_at_snapshot(&self) -> bool {
        self.was_present_at_snapshot
    }
}

/// The exact, typed target set owned by one publication request.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublicationTargets<'a> {
    ir: Option<PublicationTarget<'a>>,
    output: Option<PublicationTarget<'a>>,
    report: Option<PublicationTarget<'a>>,
}

impl<'a> PublicationTargets<'a> {
    fn from_paths(paths: &'a ArtifactPaths) -> Self {
        Self {
            ir: paths.ir.as_ref().map(PublicationTarget::from_path),
            output: paths.output.as_ref().map(PublicationTarget::from_path),
            report: paths.report.as_ref().map(PublicationTarget::from_path),
        }
    }

    #[doc(hidden)]
    pub const fn ir(self) -> Option<PublicationTarget<'a>> {
        self.ir
    }

    #[doc(hidden)]
    pub const fn output(self) -> Option<PublicationTarget<'a>> {
        self.output
    }

    #[doc(hidden)]
    pub const fn report(self) -> Option<PublicationTarget<'a>> {
        self.report
    }
}

impl<'a> PublicationTarget<'a> {
    fn from_path(path: &'a ArtifactPath) -> Self {
        Self {
            kind: path.kind,
            absolute: &path.absolute,
            resolved: &path.resolved,
            was_present_at_snapshot: path.snapshot_file_identity.is_some(),
        }
    }
}

/// Result of one output or report publication attempt.
#[doc(hidden)]
#[derive(Debug)]
pub struct PublicationAttempt {
    pub delivery: DeliveryState,
    pub failure: Option<OperationFailure>,
}

impl PublicationAttempt {
    fn failure(
        kind: DeliveryKind,
        path: Option<PathBuf>,
        code: OperationCode,
        message: impl Into<String>,
    ) -> Self {
        Self {
            delivery: DeliveryState::pending(kind, path),
            failure: Some(OperationFailure::new(code, message)),
        }
    }
}

/// Detailed report-delivery result retained by the CLI and build adapters.
#[doc(hidden)]
pub type ReportWriteAttempt = PublicationAttempt;

/// Input-file open failure that distinguishes transport errors from identity races.
#[doc(hidden)]
#[derive(Debug)]
pub enum IrArtifactOpenError {
    NoPath,
    Read(io::Error),
    Revalidation(String),
}

impl fmt::Display for IrArtifactOpenError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoPath => formatter.write_str("publication request has no IR path"),
            Self::Read(error) => error.fmt(formatter),
            Self::Revalidation(error) => formatter.write_str(error),
        }
    }
}

impl std::error::Error for IrArtifactOpenError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::NoPath | Self::Revalidation(_) => None,
            Self::Read(error) => Some(error),
        }
    }
}

/// Kind of a planned filesystem artifact.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactPathKind {
    Ir,
    Output,
    Report,
}

impl ArtifactPathKind {
    fn argument(self) -> &'static str {
        match self {
            Self::Ir => "--ir",
            Self::Output => "--output/--output-dir",
            Self::Report => "--report",
        }
    }

    fn is_write(self) -> bool {
        matches!(self, Self::Output | Self::Report)
    }

    fn is_ir(self) -> bool {
        matches!(self, Self::Ir)
    }
}

/// Compatibility-only identity snapshot for one requested artifact path.
///
/// Production publication operations accept only their owning plan/session; the snapshot fields
/// remain private so callers cannot forge or retarget an operation identity.
#[doc(hidden)]
pub struct ArtifactPath {
    kind: ArtifactPathKind,
    absolute: PathBuf,
    resolved: PathBuf,
    identity_key: Vec<u8>,
    snapshot_file_identity: Option<Vec<u8>>,
}

/// Current identity used by independent failure-report recovery.
#[doc(hidden)]
#[derive(Debug, PartialEq, Eq)]
pub struct CurrentArtifactIdentity {
    pub resolved: PathBuf,
    pub identity_key: Vec<u8>,
    pub existing_file_identity: Option<Vec<u8>>,
}

impl CurrentArtifactIdentity {
    #[doc(hidden)]
    pub fn from_absolute(absolute: &Path) -> Result<Self, String> {
        let resolved = artifact_identity_path(absolute)?;
        let identity_key = artifact_identity_key(&resolved)?;
        let existing_file_identity = current_file_identity(&resolved)?;
        Ok(Self {
            resolved,
            identity_key,
            existing_file_identity,
        })
    }
}

impl ArtifactPath {
    #[doc(hidden)]
    pub fn snapshot(
        kind: ArtifactPathKind,
        requested: &Path,
        invocation_directory: &Path,
    ) -> Result<Self, String> {
        let absolute = if requested.is_absolute() {
            requested.to_owned()
        } else {
            invocation_directory.join(requested)
        };
        if !absolute.is_absolute() {
            return Err(format!(
                "{} path {} could not be resolved to an absolute path",
                kind.argument(),
                requested.display()
            ));
        }
        validate_artifact_path_syntax(kind, &absolute)?;
        if kind.is_ir() {
            validate_ir_artifact_file(&absolute, true)?;
        }
        let resolved = artifact_identity_path(&absolute)?;
        if kind.is_ir() {
            validate_ir_artifact_file(&resolved, true)?;
        }
        let identity_key = artifact_identity_key(&resolved)?;
        let snapshot_file_identity = if kind.is_ir() {
            existing_file_identity(&resolved)?
        } else {
            None
        };
        Ok(Self {
            kind,
            absolute,
            resolved,
            identity_key,
            snapshot_file_identity,
        })
    }

    #[doc(hidden)]
    pub fn resolved(&self) -> &Path {
        &self.resolved
    }

    #[doc(hidden)]
    pub fn absolute(&self) -> &Path {
        &self.absolute
    }

    #[doc(hidden)]
    pub const fn was_present_at_snapshot(&self) -> bool {
        self.snapshot_file_identity.is_some()
    }

    #[doc(hidden)]
    pub fn current_identity(&self) -> Result<CurrentArtifactIdentity, String> {
        validate_artifact_path_syntax(self.kind, &self.absolute)?;
        CurrentArtifactIdentity::from_absolute(&self.absolute)
    }

    /// Revalidate raw spelling, canonical path identity, and IR file identity.
    #[doc(hidden)]
    pub fn revalidate(&self) -> Result<(), String> {
        validate_artifact_path_syntax(self.kind, &self.absolute)?;
        if self.kind.is_ir() {
            validate_ir_artifact_file(&self.absolute, false)?;
        }
        let current_identity = artifact_identity_path(&self.absolute)?;
        if self.kind.is_ir() {
            validate_ir_artifact_file(&current_identity, false)?;
        }
        if !artifact_revalidation_identity_matches(&self.resolved, &current_identity) {
            return Err(format!(
                "{} path {} changed identity during generation (was {}, now {})",
                self.kind.argument(),
                self.absolute.display(),
                self.resolved.display(),
                current_identity.display()
            ));
        }
        self.revalidate_file_identity(&current_identity)
    }

    #[doc(hidden)]
    pub fn revalidate_file_identity(&self, current_path: &Path) -> Result<(), String> {
        if !self.kind.is_ir() {
            return Ok(());
        }
        let current_file_identity = existing_file_identity(current_path)?;
        if self.snapshot_file_identity != current_file_identity {
            return Err(format!(
                "{} path {} changed identity during generation",
                self.kind.argument(),
                self.absolute.display()
            ));
        }
        Ok(())
    }
}

/// Revalidation uses exact canonical spelling; collision and lock identities use the
/// conservative filesystem key.
#[doc(hidden)]
pub fn artifact_revalidation_identity_matches(snapshot: &Path, current: &Path) -> bool {
    snapshot == current
}

/// Compatibility-only path set whose identities were frozen together.
///
/// `PublicationPlan` and `PublicationSession` are the production owners of this value.
#[doc(hidden)]
pub struct ArtifactPaths {
    ir: Option<ArtifactPath>,
    output: Option<ArtifactPath>,
    report: Option<ArtifactPath>,
}

impl ArtifactPaths {
    pub(crate) fn snapshot(
        ir: Option<&Path>,
        output: Option<&Path>,
        report: Option<&Path>,
    ) -> Result<Self, String> {
        let invocation_directory = std::env::current_dir()
            .map_err(|err| format!("Failed to resolve artifact paths: {err}"))?;
        let paths = Self {
            ir: ir
                .map(|path| {
                    ArtifactPath::snapshot(ArtifactPathKind::Ir, path, &invocation_directory)
                })
                .transpose()?,
            output: output
                .map(|path| {
                    ArtifactPath::snapshot(ArtifactPathKind::Output, path, &invocation_directory)
                })
                .transpose()?,
            report: report
                .map(|path| {
                    ArtifactPath::snapshot(ArtifactPathKind::Report, path, &invocation_directory)
                })
                .transpose()?,
        };
        paths.validate_distinct()?;
        Ok(paths)
    }

    pub(crate) fn validate_distinct(&self) -> Result<(), String> {
        if let (Some(output), Some(report)) = (&self.output, &self.report) {
            reject_artifact_collision(output, report)?;
        }
        if let Some(ir) = &self.ir {
            if let Some(output) = &self.output {
                reject_artifact_collision(ir, output)?;
            }
            if let Some(report) = &self.report {
                reject_artifact_collision(ir, report)?;
            }
        }
        Ok(())
    }

    /// Re-resolve every path immediately before an externally visible read or delivery.
    pub(crate) fn revalidate(&self) -> Result<(), String> {
        for path in [self.ir.as_ref(), self.output.as_ref(), self.report.as_ref()]
            .into_iter()
            .flatten()
        {
            path.revalidate()?;
        }
        self.validate_distinct()
    }

    /// Revalidate only a distinct, frozen report destination for failure recovery.
    pub(crate) fn revalidate_report_for_recovery(&self) -> Result<(), String> {
        let Some(report) = self.report.as_ref() else {
            return Ok(());
        };
        report.revalidate()?;
        let ir = self
            .ir
            .as_ref()
            .map(ArtifactPath::current_identity)
            .transpose()?;
        let output = self
            .output
            .as_ref()
            .map(ArtifactPath::current_identity)
            .transpose()?;
        let report_current = report.current_identity()?;
        if let (Some(ir), Some(ir_path)) = (ir.as_ref(), self.ir.as_ref()) {
            reject_current_artifact_collision(ir, &report_current, ir_path, report)?;
        }
        if let (Some(output), Some(output_path)) = (output.as_ref(), self.output.as_ref()) {
            reject_current_artifact_collision(output, &report_current, output_path, report)?;
        }
        Ok(())
    }
}

fn reject_artifact_collision(left: &ArtifactPath, right: &ArtifactPath) -> Result<(), String> {
    let same_existing_file = match (
        existing_file_identity(left.resolved())?,
        existing_file_identity(right.resolved())?,
    ) {
        (Some(left), Some(right)) => left == right,
        _ => false,
    };
    if left.identity_key == right.identity_key || same_existing_file {
        return Err(format!(
            "{} and {} must refer to different files (both resolve to {})",
            left.kind.argument(),
            right.kind.argument(),
            left.resolved.display()
        ));
    }
    Ok(())
}

fn reject_current_artifact_collision(
    left: &CurrentArtifactIdentity,
    right: &CurrentArtifactIdentity,
    left_path: &ArtifactPath,
    right_path: &ArtifactPath,
) -> Result<(), String> {
    let same_existing_file = matches!(
        (
            left.existing_file_identity.as_ref(),
            right.existing_file_identity.as_ref(),
        ),
        (Some(left), Some(right)) if left == right
    );
    if left.identity_key == right.identity_key || same_existing_file {
        return Err(format!(
            "{} and {} must refer to different files (both currently resolve to {})",
            left_path.kind.argument(),
            right_path.kind.argument(),
            right.resolved.display()
        ));
    }
    Ok(())
}

fn validate_artifact_path_syntax(kind: ArtifactPathKind, path: &Path) -> Result<(), String> {
    if kind.is_write() {
        return validate_write_artifact_path(path);
    }
    #[cfg(windows)]
    validate_windows_artifact_path(path)?;
    Ok(())
}

fn validate_ir_artifact_file(path: &Path, allow_missing: bool) -> Result<(), String> {
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => Ok(()),
        Ok(_) => Err(format!(
            "--ir path {} does not resolve to a regular file",
            path.display()
        )),
        Err(err)
            if allow_missing
                && matches!(
                    err.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                ) =>
        {
            Ok(())
        }
        Err(err) => Err(format!(
            "Failed to inspect --ir path {}: {err}",
            path.display()
        )),
    }
}

/// Open the IR path and prove that the opened regular file is the snapshotted file.
#[doc(hidden)]
fn open_ir_artifact_file_inner(path: &ArtifactPath) -> Result<File, IrArtifactOpenError> {
    let file = File::open(path.absolute()).map_err(|error| {
        if path.snapshot_file_identity.is_some()
            && matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            )
        {
            IrArtifactOpenError::Revalidation(format!(
                "{} path {} changed identity during generation: {error}",
                path.kind.argument(),
                path.absolute().display()
            ))
        } else {
            IrArtifactOpenError::Read(error)
        }
    })?;
    let metadata = file.metadata().map_err(|error| {
        IrArtifactOpenError::Revalidation(format!(
            "Failed to inspect opened --ir artifact {}: {error}",
            path.absolute().display()
        ))
    })?;
    if metadata.is_file() {
        let opened_file_identity = opened_file_identity(&file, path.absolute())
            .map_err(IrArtifactOpenError::Revalidation)?;
        if path.snapshot_file_identity.as_ref() == Some(&opened_file_identity) {
            Ok(file)
        } else {
            Err(IrArtifactOpenError::Revalidation(format!(
                "{} path {} changed identity during generation",
                path.kind.argument(),
                path.absolute().display()
            )))
        }
    } else {
        Err(IrArtifactOpenError::Revalidation(
            "the opened --ir artifact is not a regular file".to_owned(),
        ))
    }
}

/// Compatibility shim for the pre-session hidden facade surface.
#[doc(hidden)]
pub fn open_ir_artifact_file(
    publication: &PublicationSession,
) -> Result<File, IrArtifactOpenError> {
    publication.open_ir()
}

#[cfg(any(unix, windows))]
fn current_file_identity(path: &Path) -> Result<Option<Vec<u8>>, String> {
    existing_file_identity(path)
}

#[cfg(not(any(unix, windows)))]
fn current_file_identity(path: &Path) -> Result<Option<Vec<u8>>, String> {
    Err(format!(
        "Failed to inspect artifact identity {}: stable file identity is unsupported on this platform",
        path.display()
    ))
}

#[cfg(unix)]
#[doc(hidden)]
pub fn existing_file_identity(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match std::fs::metadata(path) {
        Ok(metadata) => Ok(Some(unix_file_identity(&metadata))),
        Err(err)
            if matches!(
                err.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            Ok(None)
        }
        Err(err) => Err(format!(
            "Failed to inspect artifact identity {}: {err}",
            path.display()
        )),
    }
}

#[cfg(unix)]
fn opened_file_identity(file: &File, path: &Path) -> Result<Vec<u8>, String> {
    file.metadata()
        .map(|metadata| unix_file_identity(&metadata))
        .map_err(|err| {
            format!(
                "Failed to inspect opened artifact identity {}: {err}",
                path.display()
            )
        })
}

#[cfg(unix)]
fn unix_file_identity(metadata: &std::fs::Metadata) -> Vec<u8> {
    use std::os::unix::fs::MetadataExt;

    let mut identity = b"unix-file\0".to_vec();
    identity.extend_from_slice(&metadata.dev().to_le_bytes());
    identity.extend_from_slice(&metadata.ino().to_le_bytes());
    identity
}

#[cfg(windows)]
#[doc(hidden)]
pub fn existing_file_identity(path: &Path) -> Result<Option<Vec<u8>>, String> {
    use std::fs::OpenOptions;
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    let file = match OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
    {
        Ok(file) => file,
        Err(err)
            if matches!(
                err.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            return Ok(None);
        }
        Err(err) => {
            return Err(format!(
                "Failed to open artifact identity {}: {err}",
                path.display()
            ));
        }
    };
    windows_opened_file_identity(&file, path).map(Some)
}

#[cfg(windows)]
fn opened_file_identity(file: &File, path: &Path) -> Result<Vec<u8>, String> {
    windows_opened_file_identity(file, path)
}

#[cfg(windows)]
fn windows_opened_file_identity(file: &File, path: &Path) -> Result<Vec<u8>, String> {
    use std::os::windows::io::AsRawHandle;
    use std::ptr;
    use windows_sys::Win32::Foundation::{
        ERROR_CALL_NOT_IMPLEMENTED, ERROR_INVALID_FUNCTION, ERROR_INVALID_LEVEL,
        ERROR_INVALID_PARAMETER, ERROR_NOT_SUPPORTED, HANDLE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        FileIdInfo, GetFileInformationByHandle, GetFileInformationByHandleEx,
        GetVolumeInformationByHandleW, BY_HANDLE_FILE_INFORMATION, FILE_ID_INFO,
    };

    let handle = file.as_raw_handle() as HANDLE;
    let mut information = FILE_ID_INFO::default();
    if unsafe {
        // SAFETY: `handle` is borrowed from the live `File`, so it remains a
        // valid non-owning handle for this synchronous call. `information` is
        // a properly aligned writable `FILE_ID_INFO` buffer and its exact size
        // is supplied for `FileIdInfo`. The BOOL result is checked before any
        // output fields are read; the call does not transfer handle ownership.
        GetFileInformationByHandleEx(
            handle,
            FileIdInfo,
            &mut information as *mut FILE_ID_INFO as _,
            std::mem::size_of::<FILE_ID_INFO>() as u32,
        )
    } != 0
    {
        return Ok(windows_file_id_identity(
            information.VolumeSerialNumber,
            &information.FileId.Identifier,
        ));
    }

    let file_id_error = io::Error::last_os_error();
    let file_id_unsupported = file_id_error.raw_os_error().is_some_and(|code| {
        matches!(
            code as u32,
            ERROR_CALL_NOT_IMPLEMENTED
                | ERROR_INVALID_FUNCTION
                | ERROR_INVALID_LEVEL
                | ERROR_INVALID_PARAMETER
                | ERROR_NOT_SUPPORTED
        )
    });
    if !file_id_unsupported {
        return Err(format!(
            "Failed to inspect artifact identity {}: {}",
            path.display(),
            file_id_error
        ));
    }

    let mut file_system_name = [0_u16; 261];
    if unsafe {
        // SAFETY: `handle` remains borrowed from the live `File`. The
        // 261-element array is an initialized, aligned writable WCHAR buffer,
        // and its element count is passed as the buffer size. The other output
        // pointers are null with zero sizes, which this API permits. The BOOL
        // result is checked before the array is used.
        GetVolumeInformationByHandleW(
            handle,
            ptr::null_mut(),
            0,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            file_system_name.as_mut_ptr(),
            file_system_name.len() as u32,
        )
    } == 0
    {
        return Err(format!(
            "Failed to inspect 128-bit artifact identity {} ({file_id_error}); refusing an unsafe \
             64-bit fallback because its file system could not be identified: {}",
            path.display(),
            io::Error::last_os_error()
        ));
    }
    let file_system_name = &file_system_name[..file_system_name
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(file_system_name.len())];
    if !windows_legacy_file_identity_is_sound(file_system_name) {
        return Err(format!(
            "Failed to inspect 128-bit artifact identity {} ({file_id_error}); refusing an unsafe \
             64-bit fallback on file system {}",
            path.display(),
            String::from_utf16_lossy(file_system_name)
        ));
    }

    let mut information = unsafe {
        // SAFETY: `BY_HANDLE_FILE_INFORMATION` is a C-compatible structure of
        // integer and `FILETIME` fields, for which the all-zero bit pattern is
        // valid. It is used as a writable output buffer and is not read until
        // the API below reports success.
        std::mem::zeroed::<BY_HANDLE_FILE_INFORMATION>()
    };
    if unsafe {
        // SAFETY: `handle` remains valid through the borrowed `File`, and
        // `information` is an aligned, writable output structure. The BOOL
        // result is checked before its fields are consumed; this call borrows
        // rather than closes or otherwise takes ownership of the handle.
        GetFileInformationByHandle(handle, &mut information)
    } == 0
    {
        return Err(format!(
            "Failed to inspect legacy NTFS artifact identity {} after 128-bit identity was \
             unavailable ({file_id_error}): {}",
            path.display(),
            io::Error::last_os_error()
        ));
    }
    let mut identity = b"windows-file-id-ntfs-64\0".to_vec();
    identity.extend_from_slice(&information.dwVolumeSerialNumber.to_le_bytes());
    identity.extend_from_slice(&information.nFileIndexHigh.to_le_bytes());
    identity.extend_from_slice(&information.nFileIndexLow.to_le_bytes());
    Ok(identity)
}

#[cfg(windows)]
#[doc(hidden)]
pub fn windows_legacy_file_identity_is_sound(file_system_name: &[u16]) -> bool {
    wide_eq_ignore_ascii_case(file_system_name, b"NTFS")
}

#[cfg(windows)]
#[doc(hidden)]
pub fn windows_file_id_identity(volume_serial: u64, file_id: &[u8; 16]) -> Vec<u8> {
    let mut identity = b"windows-file-id-128\0".to_vec();
    identity.extend_from_slice(&volume_serial.to_le_bytes());
    identity.extend_from_slice(file_id);
    identity
}

#[cfg(not(any(unix, windows)))]
#[doc(hidden)]
pub fn existing_file_identity(_path: &Path) -> Result<Option<Vec<u8>>, String> {
    Ok(None)
}

#[cfg(not(any(unix, windows)))]
fn opened_file_identity(_file: &File, path: &Path) -> Result<Vec<u8>, String> {
    Err(format!(
        "Failed to inspect opened artifact identity {}: stable file identity is supported only on Unix and Windows",
        path.display()
    ))
}

fn validate_write_artifact_path(path: &Path) -> Result<(), String> {
    if !has_normal_final_file_spelling(path) {
        return Err(format!(
            "artifact path {} must end in a normal file name, not a separator, dot or dot-dot",
            path.display()
        ));
    }
    #[cfg(windows)]
    validate_windows_artifact_path(path)?;
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(format!(
            "artifact path {} must not be a symbolic link destination",
            path.display()
        )),
        Ok(metadata) if !metadata.file_type().is_file() => Err(format!(
            "artifact path {} exists but is not a regular file",
            path.display()
        )),
        Ok(_) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(format!(
            "Failed to inspect artifact path {}: {err}",
            path.display()
        )),
    }
}

fn has_normal_final_file_spelling(path: &Path) -> bool {
    matches!(path.components().next_back(), Some(Component::Normal(_)))
        && raw_final_component_is_normal(path)
}

#[cfg(unix)]
fn raw_final_component_is_normal(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;

    let final_component = path
        .as_os_str()
        .as_bytes()
        .rsplit(|byte| *byte == b'/')
        .next()
        .unwrap_or_default();
    !matches!(final_component, b"" | b"." | b"..")
}

#[cfg(windows)]
fn raw_final_component_is_normal(path: &Path) -> bool {
    use std::os::windows::ffi::OsStrExt;

    let encoded = path.as_os_str().encode_wide().collect::<Vec<_>>();
    let final_component = encoded
        .rsplit(|unit| matches!(*unit, 0x2f | 0x5c))
        .next()
        .unwrap_or_default();
    !matches!(final_component, [] | [0x2e] | [0x2e, 0x2e])
}

#[cfg(not(any(unix, windows)))]
fn raw_final_component_is_normal(path: &Path) -> bool {
    let path = path.to_string_lossy();
    let final_component = path.rsplit(['/', '\\']).next().unwrap_or_default();
    !matches!(final_component, "" | "." | "..")
}

/// Conservative case-folded identity used for collision and lock keys.
#[doc(hidden)]
pub fn conservative_casefold_identity_key(path: &Path) -> Vec<u8> {
    use unicode_normalization::UnicodeNormalization;

    let normalized = path
        .as_os_str()
        .to_string_lossy()
        .nfd()
        .flat_map(char::to_uppercase)
        .collect::<String>()
        .nfd()
        .collect::<String>();
    normalized.into_bytes()
}

#[cfg(unix)]
#[doc(hidden)]
pub fn artifact_identity_key(path: &Path) -> Result<Vec<u8>, String> {
    Ok(conservative_casefold_identity_key(path))
}

#[cfg(windows)]
#[doc(hidden)]
pub fn artifact_identity_key(path: &Path) -> Result<Vec<u8>, String> {
    use std::os::windows::ffi::OsStrExt;

    let encoded = path.as_os_str().encode_wide().collect::<Vec<_>>();
    Ok(utf16_uppercase_fold(&encoded)
        .into_iter()
        .flat_map(u16::to_le_bytes)
        .collect())
}

/// Ordinal uppercase fold over UTF-16 code units, used for the Windows identity key.
///
/// This is deliberately not a locale mapping and not a normalization. Windows file systems compare
/// names by upcasing each code point through a frozen table, so this applies Unicode uppercase to
/// each scalar value and leaves everything else exactly as it was found. Two properties the callers
/// depend on follow from that, and both are pinned by the tests below:
///
/// - Greek capital sigma and Greek final sigma fold together, because a case-insensitive volume
///   treats them as one name and a collision between them must be refused rather than missed.
/// - `İ` stays distinct from a decomposed `i` followed by a combining dot above, because no
///   normalization runs here. The Unix key in [`conservative_casefold_identity_key`] normalizes and
///   therefore merges that pair, which is why the two platforms cannot share one key.
///
/// Unpaired surrogates are preserved verbatim rather than replaced, so two paths that differ only
/// in an unpaired surrogate keep distinct keys instead of colliding on a replacement character.
///
/// The fold is conservative in one direction only. Unicode full uppercase can expand one scalar
/// into several, so `ß` folds to `SS` where a file system's simple upcase table leaves it alone.
/// That can refuse a pair of names the volume would have accepted as distinct; it cannot miss a
/// collision, which is the direction that matters for a key guarding artifact writes.
///
/// It stays compiled on every target rather than behind `cfg(windows)` so that the tests which pin
/// those properties run in the ordinary local suite. The previous implementation asked the
/// operating system, which meant no local gate could observe its behavior at all.
#[cfg_attr(not(windows), allow(dead_code))]
fn utf16_uppercase_fold(input: &[u16]) -> Vec<u16> {
    let mut output = Vec::with_capacity(input.len());
    let mut buffer = [0_u16; 2];
    for decoded in char::decode_utf16(input.iter().copied()) {
        match decoded {
            Ok(scalar) => {
                for upper in scalar.to_uppercase() {
                    output.extend_from_slice(upper.encode_utf16(&mut buffer));
                }
            }
            Err(unpaired) => output.push(unpaired.unpaired_surrogate()),
        }
    }
    output
}

#[cfg(not(any(unix, windows)))]
#[doc(hidden)]
pub fn artifact_identity_key(path: &Path) -> Result<Vec<u8>, String> {
    Ok(path.to_string_lossy().into_owned().into_bytes())
}

#[cfg(windows)]
#[doc(hidden)]
pub fn windows_lexically_normalized_absolute_path(path: &Path) -> Result<PathBuf, String> {
    use std::ffi::OsString;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::ptr;
    use windows_sys::Win32::Storage::FileSystem::GetFullPathNameW;

    let mut input = path.as_os_str().encode_wide().collect::<Vec<_>>();
    if input.contains(&0) {
        return Err(format!(
            "Failed to normalize Windows artifact path {}: path contains a NUL character",
            path.display()
        ));
    }
    input.push(0);
    let required = unsafe {
        // SAFETY: `input` is a live, NUL-terminated UTF-16 buffer. This
        // zero-length call supplies the documented null output pointers to
        // obtain the required capacity, so no output buffer is accessed. The
        // returned size is checked before a writable buffer is allocated.
        GetFullPathNameW(input.as_ptr(), 0, ptr::null_mut(), ptr::null_mut())
    };
    if required == 0 {
        return Err(format!(
            "Failed to normalize Windows artifact path {}: {}",
            path.display(),
            io::Error::last_os_error()
        ));
    }
    let mut capacity = required;
    loop {
        let mut normalized = vec![0_u16; capacity as usize];
        let written = unsafe {
            // SAFETY: `input` remains NUL-terminated and alive, while
            // `normalized` owns `capacity` initialized WCHAR slots and stays
            // mutable for the synchronous call. `lpFilePart` is the API's
            // optional null output. The return value is checked before the
            // buffer is truncated or converted.
            GetFullPathNameW(
                input.as_ptr(),
                capacity,
                normalized.as_mut_ptr(),
                ptr::null_mut(),
            )
        };
        if written == 0 {
            return Err(format!(
                "Failed to normalize Windows artifact path {}: {}",
                path.display(),
                io::Error::last_os_error()
            ));
        }
        if written < capacity {
            normalized.truncate(written as usize);
            return Ok(PathBuf::from(OsString::from_wide(&normalized)));
        }
        capacity = written;
    }
}

#[cfg(windows)]
#[doc(hidden)]
pub fn validate_windows_artifact_path(path: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use std::path::Prefix;

    let verbatim = matches!(
        path.components().next(),
        Some(Component::Prefix(prefix)) if prefix.kind().is_verbatim()
    );
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => match prefix.kind() {
                Prefix::Disk(_) | Prefix::VerbatimDisk(_) if path.has_root() => {}
                Prefix::Disk(_) | Prefix::VerbatimDisk(_) => {
                    return Err(format!(
                        "artifact path prefix {:?} is drive-relative; use an absolute drive path",
                        prefix.as_os_str()
                    ));
                }
                Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share) => {
                    for (kind, value) in [("server", server), ("share", share)] {
                        let value_units = value.encode_wide().collect::<Vec<_>>();
                        validate_windows_artifact_component(&value_units).map_err(|reason| {
                            format!(
                                "UNC {kind} component {value:?} is not valid on Windows: {reason}"
                            )
                        })?;
                    }
                }
                Prefix::DeviceNS(_) | Prefix::Verbatim(_) => {
                    return Err(format!(
                        "artifact path prefix {:?} uses an unsupported Windows path namespace",
                        prefix.as_os_str()
                    ));
                }
            },
            Component::Normal(component) => {
                let component_units = component.encode_wide().collect::<Vec<_>>();
                validate_windows_artifact_component(&component_units).map_err(|reason| {
                    format!(
                        "artifact path component {component:?} is not valid on Windows: {reason}"
                    )
                })?;
            }
            component @ (Component::CurDir | Component::ParentDir) if verbatim => {
                return Err(format!(
                    "artifact path {} uses relative component {:?} in a verbatim Windows namespace",
                    path.display(),
                    component.as_os_str()
                ));
            }
            Component::RootDir | Component::CurDir | Component::ParentDir => {}
        }
    }
    Ok(())
}

#[doc(hidden)]
pub fn validate_windows_artifact_component(component: &[u16]) -> Result<(), &'static str> {
    validate_windows_component_syntax(component)?;
    let stem = component
        .split(|unit| *unit == 0x2e)
        .next()
        .unwrap_or(component);
    let stem = stem
        .iter()
        .rposition(|unit| *unit != 0x20)
        .map_or(&[][..], |last| &stem[..=last]);
    if windows_reserved_device_name(stem) {
        return Err("uses a reserved DOS device name");
    }
    Ok(())
}

fn validate_windows_component_syntax(component: &[u16]) -> Result<(), &'static str> {
    if component.is_empty() {
        return Err("is empty");
    }
    if component.first() == Some(&0x20) {
        return Err("starts with a space");
    }
    if component.iter().any(|unit| *unit <= 0x1f) {
        return Err("contains a control character");
    }
    if component
        .iter()
        .any(|unit| matches!(*unit, 0x2f | 0x3c | 0x3e | 0x3a | 0x22 | 0x7c | 0x3f | 0x2a))
    {
        return Err("contains a Win32-forbidden character");
    }
    if matches!(component.last(), Some(0x2e | 0x20)) {
        return Err("ends in a dot or space");
    }
    Ok(())
}

fn windows_reserved_device_name(stem: &[u16]) -> bool {
    [
        &b"CON"[..],
        &b"PRN"[..],
        &b"AUX"[..],
        &b"NUL"[..],
        &b"CONIN$"[..],
        &b"CONOUT$"[..],
    ]
    .iter()
    .any(|reserved| wide_eq_ignore_ascii_case(stem, reserved))
        || (stem.len() == 4
            && (wide_eq_ignore_ascii_case(&stem[..3], b"COM")
                || wide_eq_ignore_ascii_case(&stem[..3], b"LPT"))
            && matches!(stem[3], 0x31..=0x39 | 0x00b9 | 0x00b2 | 0x00b3))
}

fn wide_eq_ignore_ascii_case(value: &[u16], expected: &[u8]) -> bool {
    value.len() == expected.len()
        && value.iter().zip(expected).all(|(left, right)| {
            let left = if matches!(*left, 0x61..=0x7a) {
                *left - 0x20
            } else {
                *left
            };
            left == u16::from(*right)
        })
}

/// Resolve existing path prefixes one component at a time and retain safe lexical suffixes.
#[doc(hidden)]
pub fn artifact_identity_path(path: &Path) -> Result<PathBuf, String> {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()
            .map_err(|err| format!("Failed to resolve artifact paths: {err}"))?
            .join(path)
    };
    #[cfg(windows)]
    let absolute = windows_lexically_normalized_absolute_path(&absolute)?;

    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(name) => {
                let candidate = normalized.join(name);
                match std::fs::canonicalize(&candidate) {
                    Ok(canonical) => normalized = canonical,
                    Err(err)
                        if matches!(
                            err.kind(),
                            io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                        ) =>
                    {
                        normalized = candidate;
                    }
                    Err(err) => {
                        return Err(format!(
                            "Failed to resolve artifact path {}: {err}",
                            candidate.display()
                        ));
                    }
                }
            }
            component @ (Component::Prefix(_) | Component::RootDir) => {
                normalized.push(component.as_os_str());
            }
        }
    }
    Ok(normalized)
}

#[doc(hidden)]
pub static LOCK_OWNER_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Ordered cross-process locks for a complete output/report write set.
#[doc(hidden)]
pub struct ArtifactLockSet {
    _locks: Vec<ArtifactLock>,
}

impl ArtifactLockSet {
    fn acquire(paths: &ArtifactPaths) -> Result<Self, String> {
        let requested_targets =
            usize::from(paths.output.is_some()) + usize::from(paths.report.is_some());
        let span = tracing::debug_span!(
            "pyo3_bindgen.publication.artifact_locks",
            observability_schema = 1,
            requested_targets,
        );
        let _entered = span.enter();
        let mut identities = Vec::new();
        for path in [paths.output.as_ref(), paths.report.as_ref()]
            .into_iter()
            .flatten()
        {
            identities.extend(
                artifact_lock_keys_inner(path)?
                    .into_iter()
                    .map(|key| (key, path.resolved().to_owned())),
            );
        }
        identities.sort_by(|(left, _), (right, _)| left.cmp(right));
        identities.dedup_by(|(left, _), (right, _)| left == right);

        let mut locks = Vec::with_capacity(identities.len());
        for (key, identity) in identities {
            locks.push(ArtifactLock::acquire(&identity, &key)?);
        }
        tracing::debug!(
            observability_schema = 1,
            requested_targets,
            lock_count = locks.len(),
            "artifact lock set acquired"
        );
        Ok(Self { _locks: locks })
    }
}

#[doc(hidden)]
fn artifact_lock_keys_inner(path: &ArtifactPath) -> Result<Vec<Vec<u8>>, String> {
    fn tagged_key(tag: &[u8], parts: &[&[u8]]) -> Vec<u8> {
        let mut key = tag.to_vec();
        for part in parts {
            key.extend_from_slice(&(part.len() as u64).to_le_bytes());
            key.extend_from_slice(part);
        }
        key
    }

    let mut keys = vec![tagged_key(b"path\0", &[&path.identity_key])];
    if let Some(file_identity) = existing_file_identity(path.resolved())? {
        keys.push(tagged_key(b"file\0", &[&file_identity]));
    }
    if let (Some(parent), Some(file_name)) = (path.resolved().parent(), path.resolved().file_name())
    {
        if let Some(parent_identity) = existing_file_identity(parent)? {
            let final_component_key = artifact_identity_key(Path::new(file_name))?;
            keys.push(tagged_key(
                b"directory-entry\0",
                &[&parent_identity, &final_component_key],
            ));
        }
    }
    Ok(keys)
}

/// Compatibility shim for the pre-session hidden facade surface.
#[doc(hidden)]
pub fn artifact_lock_keys(path: &ArtifactPath) -> Result<Vec<Vec<u8>>, String> {
    artifact_lock_keys_inner(path)
}

struct ArtifactLock {
    _file: File,
}

impl ArtifactLock {
    fn acquire(identity: &Path, identity_key: &[u8]) -> Result<Self, String> {
        let started = std::time::Instant::now();
        let span = tracing::debug_span!("pyo3_bindgen.publication.artifact_lock");
        let _entered = span.enter();
        let path = artifact_lock_path_inner(identity_key)?;
        let owner = format!(
            "pid={}\ncreated_unix_ms={}\ntoken={}\nidentity={}\n",
            std::process::id(),
            unix_time_millis(),
            LOCK_OWNER_COUNTER.fetch_add(1, Ordering::Relaxed),
            identity.display()
        );

        let mut file = open_artifact_lock_inner(&path).map_err(|err| {
            if lock_is_contended(&err) {
                let process = lock_owner_pid(&path)
                    .map(|pid| pid.to_string())
                    .unwrap_or_else(|| "with unknown owner".to_owned());
                let message = format!(
                    "artifact {} is locked by active process {process} ({})",
                    identity.display(),
                    path.display()
                );
                tracing::warn!(
                    observability_schema = 1,
                    path_hash = %crate::stable_hash(path.as_os_str().as_encoded_bytes()),
                    contended = true,
                    acquisition_ms = started.elapsed().as_millis(),
                    result = "contended",
                    "artifact lock contention"
                );
                message
            } else {
                let message = format!("Failed to acquire artifact lock {}: {err}", path.display());
                tracing::error!(
                    observability_schema = 1,
                    path_hash = %crate::stable_hash(path.as_os_str().as_encoded_bytes()),
                    contended = false,
                    acquisition_ms = started.elapsed().as_millis(),
                    result = "failed",
                    "artifact lock acquisition failed"
                );
                message
            }
        })?;
        file.set_len(0)
            .and_then(|()| file.seek(SeekFrom::Start(0)).map(|_| ()))
            .and_then(|()| file.write_all(owner.as_bytes()))
            .and_then(|()| file.sync_all())
            .map_err(|err| {
                format!(
                    "Failed to initialize artifact lock {}: {err}",
                    path.display()
                )
            })?;
        tracing::debug!(
            observability_schema = 1,
            path_hash = %crate::stable_hash(path.as_os_str().as_encoded_bytes()),
            contended = false,
            acquisition_ms = started.elapsed().as_millis(),
            result = "acquired",
            "artifact lock acquired"
        );
        Ok(Self { _file: file })
    }
}

#[doc(hidden)]
fn artifact_lock_path_inner(identity_key: &[u8]) -> Result<PathBuf, String> {
    let directory = artifact_lock_directory()?;
    std::fs::create_dir_all(&directory).map_err(|err| {
        format!(
            "Failed to create artifact lock directory {}: {err}",
            directory.display()
        )
    })?;
    if std::fs::symlink_metadata(&directory)
        .map_err(|err| {
            format!(
                "Failed to inspect artifact lock directory {}: {err}",
                directory.display()
            )
        })?
        .file_type()
        .is_symlink()
    {
        return Err(format!(
            "artifact lock directory {} must not be a symbolic link",
            directory.display()
        ));
    }
    secure_lock_directory(&directory)?;

    let hash = crate::stable_hash(identity_key);
    let file_hash = hash
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    Ok(directory.join(format!("{file_hash}.lock")))
}

/// Compatibility shim for the pre-session hidden facade surface.
#[doc(hidden)]
pub fn artifact_lock_path(identity_key: &[u8]) -> Result<PathBuf, String> {
    artifact_lock_path_inner(identity_key)
}

#[cfg(unix)]
fn artifact_lock_directory() -> Result<PathBuf, String> {
    Ok(PathBuf::from(format!(
        "/tmp/pyo3-bindgen-artifact-locks-{}",
        unsafe {
            // SAFETY: `geteuid` has no pointer, ownership, or lifetime
            // preconditions, and this libc binding matches the Unix ABI.
            libc::geteuid()
        }
    )))
}

#[cfg(not(any(unix, windows)))]
fn artifact_lock_directory() -> Result<PathBuf, String> {
    Err("artifact locking is supported only on Unix and Windows".to_owned())
}

#[cfg(unix)]
fn secure_lock_directory(directory: &Path) -> Result<(), String> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let effective_uid = unsafe {
        // SAFETY: `geteuid` has no pointer, ownership, or lifetime
        // preconditions, and this libc binding matches the Unix ABI.
        libc::geteuid()
    };
    let metadata = std::fs::symlink_metadata(directory).map_err(|err| {
        format!(
            "Failed to inspect artifact lock directory {}: {err}",
            directory.display()
        )
    })?;
    if metadata.uid() != effective_uid {
        return Err(format!(
            "artifact lock directory {} is not owned by the current user",
            directory.display()
        ));
    }
    if metadata.permissions().mode() & 0o077 != 0 {
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700)).map_err(
            |err| {
                format!(
                    "Failed to secure artifact lock directory {}: {err}",
                    directory.display()
                )
            },
        )?;
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn secure_lock_directory(_directory: &Path) -> Result<(), String> {
    Ok(())
}

fn lock_owner_pid(path: &Path) -> Option<u32> {
    std::fs::read_to_string(path)
        .ok()?
        .lines()
        .find_map(|line| line.strip_prefix("pid="))?
        .parse()
        .ok()
}

#[cfg(windows)]
fn artifact_lock_directory() -> Result<PathBuf, String> {
    use std::{ffi::OsString, os::windows::ffi::OsStringExt, ptr, slice};
    use windows_sys::Win32::{
        Foundation::{RPC_E_CHANGED_MODE, S_FALSE, S_OK},
        System::Com::{CoInitializeEx, CoTaskMemFree, CoUninitialize, COINIT_APARTMENTTHREADED},
        UI::Shell::{FOLDERID_LocalAppData, SHGetKnownFolderPath},
    };

    struct ComInitialization {
        must_uninitialize: bool,
    }

    impl ComInitialization {
        fn initialize() -> Result<Self, String> {
            let status = unsafe {
                // SAFETY: the reserved pointer is null as required, and
                // `COINIT_APARTMENTTHREADED` is a valid single concurrency
                // model. The HRESULT is matched below so only successful
                // initializations are later balanced.
                CoInitializeEx(ptr::null(), COINIT_APARTMENTTHREADED as u32)
            };
            match status {
                S_OK | S_FALSE => Ok(Self {
                    must_uninitialize: true,
                }),
                RPC_E_CHANGED_MODE => Ok(Self {
                    must_uninitialize: false,
                }),
                _ => Err(format!(
                    "Failed to initialize COM for LocalAppData lookup (HRESULT {:#010x})",
                    status as u32
                )),
            }
        }
    }

    impl Drop for ComInitialization {
        fn drop(&mut self) {
            if self.must_uninitialize {
                unsafe {
                    // SAFETY: `must_uninitialize` is set only after this
                    // thread's `CoInitializeEx` returned S_OK or S_FALSE, so
                    // this synchronously dropped guard balances that success
                    // on the same thread.
                    CoUninitialize();
                };
            }
        }
    }

    let _com = ComInitialization::initialize()?;
    let mut raw_path = ptr::null_mut();
    let status = unsafe {
        // SAFETY: the static `FOLDERID_LocalAppData` reference is valid, flags
        // are zero, and the null token requests the current user. `raw_path`
        // is a valid writable out-pointer initialized to null. The API's
        // HRESULT is checked before dereferencing the result, and its task
        // memory is freed below before normal control flow returns.
        SHGetKnownFolderPath(&FOLDERID_LocalAppData, 0, ptr::null_mut(), &mut raw_path)
    };
    let result = if status != S_OK {
        Err(format!(
            "Failed to resolve the per-user LocalAppData lock root (HRESULT {:#010x})",
            status as u32
        ))
    } else if raw_path.is_null() {
        Err("Windows returned an empty LocalAppData lock root".to_owned())
    } else {
        let len = unsafe {
            // SAFETY: successful `SHGetKnownFolderPath` returns a non-null,
            // null-terminated UTF-16 string in one task-memory allocation.
            // Each `add` stays within that allocation until its documented
            // terminator, and the raw pointer does not escape this scan.
            let mut len = 0usize;
            while *raw_path.add(len) != 0 {
                len += 1;
            }
            len
        };
        let path = unsafe {
            // SAFETY: `raw_path` is non-null and points to `len` initialized,
            // aligned UTF-16 code units in the single allocation returned by
            // the API; `len` excludes the terminator. `from_wide` copies the
            // slice before `CoTaskMemFree` below invalidates that allocation.
            slice::from_raw_parts(raw_path, len)
        };
        Ok(PathBuf::from(OsString::from_wide(path))
            .join("pyo3-bindgen")
            .join("artifact-locks"))
    };
    unsafe {
        // SAFETY: `raw_path` is either still null or the task-memory pointer
        // supplied by `SHGetKnownFolderPath`, whose contract requires
        // `CoTaskMemFree` even when that call fails. The borrowed path was
        // copied before this sole cleanup call.
        CoTaskMemFree(raw_path.cast());
    };
    result
}

#[cfg(windows)]
fn secure_lock_directory(directory: &Path) -> Result<(), String> {
    use std::os::windows::fs::MetadataExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

    let metadata = std::fs::symlink_metadata(directory).map_err(|err| {
        format!(
            "Failed to inspect artifact lock directory {}: {err}",
            directory.display()
        )
    })?;
    if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(format!(
            "artifact lock directory {} must not be a Windows reparse point",
            directory.display()
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn open_artifact_lock_inner(path: &Path) -> io::Result<File> {
    use std::os::{fd::AsRawFd, unix::fs::OpenOptionsExt};

    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)?;
    if unsafe {
        // SAFETY: `file` is an open, live `File`; its raw descriptor remains
        // valid for this non-owning call. The operation combines the valid
        // `LOCK_EX` and `LOCK_NB` flags, and the return value is checked before
        // reporting `last_os_error` below.
        libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB)
    } == 0
    {
        Ok(file)
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(windows)]
fn open_artifact_lock_inner(path: &Path) -> io::Result<File> {
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_OPEN_REPARSE_POINT,
    };

    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    if file.metadata()?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "artifact lock files must not be Windows reparse points",
        ));
    }
    Ok(file)
}

#[cfg(not(any(unix, windows)))]
fn open_artifact_lock_inner(_path: &Path) -> io::Result<File> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "artifact locking is supported only on Unix and Windows",
    ))
}

/// Compatibility shim for the pre-session hidden facade surface.
#[doc(hidden)]
pub fn open_artifact_lock(path: &Path) -> io::Result<File> {
    open_artifact_lock_inner(path)
}

#[cfg(unix)]
fn lock_is_contended(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::WouldBlock
}

#[cfg(windows)]
fn lock_is_contended(error: &io::Error) -> bool {
    matches!(error.raw_os_error(), Some(32 | 33))
}

#[cfg(not(any(unix, windows)))]
fn lock_is_contended(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::AlreadyExists
}

fn unix_time_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_millis()
}

fn delivery_receipt(
    kind: DeliveryKind,
    path: Option<&Path>,
    hash: String,
    bytes: usize,
    durability: DeliveryDurability,
) -> DeliveryReceipt {
    DeliveryReceipt {
        kind,
        path: path.map(Path::to_owned),
        hash,
        bytes,
        durability,
    }
}

fn unsupported_durability_failure(
    policy: PublicationPolicy,
    kind: DeliveryKind,
    path: &Path,
) -> Option<OperationFailure> {
    if !matches!(policy, PublicationPolicy::Cli) {
        return None;
    }
    let (code, label) = match kind {
        DeliveryKind::OutputFile => (OperationCode::OutputDurabilityFailed, "Output"),
        DeliveryKind::Report => (OperationCode::ReportDurabilityFailed, "Report"),
        DeliveryKind::StandardOutput => return None,
    };
    Some(OperationFailure::new(
        code,
        format!(
            "{label} {} was published, but parent directory synchronization is unsupported on this platform",
            path.display()
        ),
    ))
}

fn output_attempt(
    policy: PublicationPolicy,
    path: &Path,
    artifact: &RustArtifact,
    result: Result<crate::artifact::ArtifactWriteReceipt, ArtifactWriteFailure>,
    write_code: OperationCode,
    uncertain_code: OperationCode,
    durability_code: OperationCode,
) -> PublicationAttempt {
    let pending = || DeliveryState::pending(DeliveryKind::OutputFile, Some(path.to_owned()));
    match result {
        Ok(receipt) => match receipt.durability {
            ArtifactDurability::DirectorySynchronized => PublicationAttempt {
                delivery: DeliveryState::Published(delivery_receipt(
                    DeliveryKind::OutputFile,
                    Some(path),
                    artifact.hash(),
                    artifact.len(),
                    DeliveryDurability::DirectorySynchronized,
                )),
                failure: None,
            },
            ArtifactDurability::PlatformUnsupported => PublicationAttempt {
                delivery: DeliveryState::Published(delivery_receipt(
                    DeliveryKind::OutputFile,
                    Some(path),
                    artifact.hash(),
                    artifact.len(),
                    DeliveryDurability::PlatformUnsupported,
                )),
                failure: unsupported_durability_failure(policy, DeliveryKind::OutputFile, path),
            },
            ArtifactDurability::DirectorySyncFailed(error) => {
                let message = format!(
                    "Output {} was published, but parent directory synchronization failed: {error}",
                    path.display()
                );
                PublicationAttempt {
                    delivery: DeliveryState::Published(delivery_receipt(
                        DeliveryKind::OutputFile,
                        Some(path),
                        artifact.hash(),
                        artifact.len(),
                        DeliveryDurability::DirectorySyncFailed {
                            message: error.to_string(),
                        },
                    )),
                    failure: Some(OperationFailure::new(durability_code, message)),
                }
            }
        },
        Err(ArtifactWriteFailure::NotPublished(error)) => PublicationAttempt {
            delivery: pending(),
            failure: Some(OperationFailure::new(
                write_code,
                format!("Failed to write to file {}: {error}", path.display()),
            )),
        },
        Err(ArtifactWriteFailure::PossiblyPublished { error, staged_path }) => {
            let message = format!("Output {} may have been published: {error}", path.display());
            PublicationAttempt {
                delivery: DeliveryState::PossiblyPublished {
                    kind: DeliveryKind::OutputFile,
                    path: Some(path.to_owned()),
                    staged_path: Some(staged_path),
                    message: message.clone(),
                },
                failure: Some(OperationFailure::new(uncertain_code, message)),
            }
        }
    }
}

#[doc(hidden)]
fn publish_report_inner(
    policy: PublicationPolicy,
    artifact_paths: &ArtifactPaths,
    report: &GenerationReport,
    recovery: bool,
) -> ReportWriteAttempt {
    let Some(path) = artifact_paths.report.as_ref() else {
        return PublicationAttempt {
            delivery: DeliveryState::NotRequested,
            failure: None,
        };
    };
    let pending = || DeliveryState::pending(DeliveryKind::Report, Some(path.resolved().to_owned()));
    let report_json = match serde_json::to_string_pretty(report) {
        Ok(report_json) => report_json,
        Err(error) => {
            return PublicationAttempt {
                delivery: pending(),
                failure: Some(OperationFailure::new(
                    OperationCode::ReportSerializationFailed,
                    format!("Failed to serialize generation report: {error}"),
                )),
            };
        }
    };
    let revalidation = if recovery {
        artifact_paths.revalidate_report_for_recovery()
    } else {
        artifact_paths.revalidate()
    };
    if let Err(error) = revalidation {
        return PublicationAttempt {
            delivery: pending(),
            failure: Some(OperationFailure::new(
                OperationCode::ArtifactRevalidationFailed,
                error,
            )),
        };
    }

    tracing::debug!(
        path = %path.resolved().display(),
        diagnostics = report.diagnostics.len(),
        outputs = report.outputs.len(),
        recovery,
        "writing generation report"
    );
    let artifact = TextArtifact::new(report_json);
    let write_result = write_report_artifact(&artifact, path.resolved());
    let identity = |durability| {
        delivery_receipt(
            DeliveryKind::Report,
            Some(path.resolved()),
            artifact.hash(),
            artifact.len(),
            durability,
        )
    };
    match write_result {
        Ok(receipt) => match receipt.durability {
            ArtifactDurability::DirectorySynchronized => PublicationAttempt {
                delivery: DeliveryState::Published(identity(
                    DeliveryDurability::DirectorySynchronized,
                )),
                failure: None,
            },
            ArtifactDurability::PlatformUnsupported => PublicationAttempt {
                delivery: DeliveryState::Published(identity(
                    DeliveryDurability::PlatformUnsupported,
                )),
                failure: unsupported_durability_failure(
                    policy,
                    DeliveryKind::Report,
                    path.resolved(),
                ),
            },
            ArtifactDurability::DirectorySyncFailed(error) => {
                let message = format!(
                    "Report {} was published, but parent directory synchronization failed: {error}",
                    path.resolved().display()
                );
                PublicationAttempt {
                    delivery: DeliveryState::Published(identity(
                        DeliveryDurability::DirectorySyncFailed {
                            message: error.to_string(),
                        },
                    )),
                    failure: Some(OperationFailure::new(
                        OperationCode::ReportDurabilityFailed,
                        message,
                    )),
                }
            }
        },
        Err(ArtifactWriteFailure::NotPublished(error)) => PublicationAttempt {
            delivery: pending(),
            failure: Some(OperationFailure::new(
                OperationCode::ReportWriteFailed,
                format!(
                    "Failed to write report {}: {error}",
                    path.resolved().display()
                ),
            )),
        },
        Err(ArtifactWriteFailure::PossiblyPublished { error, staged_path }) => {
            let message = format!(
                "Report {} may have been published: {error}",
                path.resolved().display()
            );
            PublicationAttempt {
                delivery: DeliveryState::PossiblyPublished {
                    kind: DeliveryKind::Report,
                    path: Some(path.resolved().to_owned()),
                    staged_path: Some(staged_path),
                    message: message.clone(),
                },
                failure: Some(OperationFailure::new(
                    OperationCode::ReportPublicationUncertain,
                    message,
                )),
            }
        }
    }
}

/// Compatibility shim for the pre-session hidden facade surface.
#[doc(hidden)]
pub fn publish_report(
    publication: &PublicationSession,
    report: &GenerationReport,
    recovery: bool,
) -> ReportWriteAttempt {
    publish_report_inner(publication.policy, &publication.paths, report, recovery)
}

#[cfg(test)]
mod identity_fold_tests {
    use super::utf16_uppercase_fold;

    fn fold(text: &str) -> Vec<u16> {
        utf16_uppercase_fold(&text.encode_utf16().collect::<Vec<_>>())
    }

    #[test]
    fn greek_final_sigma_folds_onto_capital_sigma() {
        assert_eq!(fold("\u{3a3}.rs"), fold("\u{3c2}.rs"));
        assert_eq!(fold("\u{3c3}.rs"), fold("\u{3c2}.rs"));
    }

    #[test]
    fn dotted_capital_i_stays_distinct_from_a_decomposed_lowercase_i() {
        assert_ne!(fold("\u{130}.rs"), fold("i\u{307}.rs"));
    }

    #[test]
    fn ascii_and_non_basic_plane_case_pairs_fold_together() {
        assert_eq!(fold("bindings.rs"), fold("BINDINGS.RS"));
        // Deseret small letter long I and its capital, which need surrogate pairs.
        assert_eq!(fold("\u{10428}.rs"), fold("\u{10400}.rs"));
    }

    #[test]
    fn unpaired_surrogates_survive_verbatim_and_stay_distinct() {
        let leading = [0xd800, u16::from(b'.'), u16::from(b'r'), u16::from(b's')];
        let other_leading = [0xd801, u16::from(b'.'), u16::from(b'r'), u16::from(b's')];
        let trailing = [0xdc00, u16::from(b'.'), u16::from(b'r'), u16::from(b's')];
        let folded_suffix = [u16::from(b'.'), u16::from(b'R'), u16::from(b'S')];

        assert_eq!(
            utf16_uppercase_fold(&leading),
            [&[0xd800][..], &folded_suffix[..]].concat(),
            "an unpaired leading surrogate must pass through while the rest still folds"
        );
        assert_eq!(
            utf16_uppercase_fold(&trailing),
            [&[0xdc00][..], &folded_suffix[..]].concat()
        );
        assert_ne!(
            utf16_uppercase_fold(&leading),
            utf16_uppercase_fold(&other_leading)
        );
    }

    #[test]
    fn a_surrogate_pair_is_decoded_rather_than_folded_as_two_halves() {
        // U+10428 is a lowercase letter whose two code units are meaningless on their own.
        let pair = "\u{10428}".encode_utf16().collect::<Vec<_>>();
        assert_eq!(pair.len(), 2);
        assert_eq!(
            utf16_uppercase_fold(&pair),
            "\u{10400}".encode_utf16().collect::<Vec<_>>()
        );
        // Folding each half alone must not produce the same units, or the pair was never decoded.
        let halves = [
            utf16_uppercase_fold(&pair[..1]),
            utf16_uppercase_fold(&pair[1..]),
        ]
        .concat();
        assert_eq!(halves, pair);
    }

    #[test]
    fn folding_is_idempotent_and_preserves_the_empty_input() {
        assert_eq!(utf16_uppercase_fold(&[]), Vec::<u16>::new());
        let once = fold("Straße/Σ/i\u{307}.rs");
        assert_eq!(utf16_uppercase_fold(&once), once);
    }
}

#[cfg(test)]
mod interface_tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static DIRECTORY_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn test_directory() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "pyo3_bindgen_publication_interface_{}_{}",
            std::process::id(),
            DIRECTORY_COUNTER.fetch_add(1, Ordering::Relaxed),
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn codegen_policy_rejects_non_output_artifacts() {
        let output = Path::new("bindings.rs");
        let report = Path::new("report.json");
        assert!(PublicationPlan::snapshot(PublicationPolicy::codegen(), None, None, None).is_err());
        assert!(PublicationPlan::snapshot(
            PublicationPolicy::codegen(),
            None,
            Some(output),
            Some(report),
        )
        .is_err());
    }

    #[test]
    fn session_owns_locked_output_and_report_delivery() {
        let directory = test_directory();
        let output = directory.join("bindings.rs");
        let report_path = directory.join("report.json");
        let session = PublicationPlan::snapshot(
            PublicationPolicy::cli(),
            None,
            Some(&output),
            Some(&report_path),
        )
        .unwrap()
        .acquire()
        .unwrap();
        assert_eq!(session.policy(), PublicationPolicy::Cli);

        let artifact = RustArtifact::from_tokens(quote::quote! {
            pub fn generated() -> bool { true }
        })
        .unwrap();
        let output_attempt = session.publish_output(&artifact);
        if cfg!(unix) {
            assert!(output_attempt.failure.is_none());
        } else {
            assert_eq!(
                output_attempt.failure.as_ref().map(|failure| failure.code),
                Some(OperationCode::OutputDurabilityFailed)
            );
        }
        let DeliveryState::Published(output_receipt) = &output_attempt.delivery else {
            panic!(
                "output must retain a published receipt: {:?}",
                output_attempt.delivery
            );
        };
        assert_eq!(output_receipt.kind, DeliveryKind::OutputFile);
        assert_eq!(output_receipt.hash, artifact.hash());
        assert_eq!(output_receipt.bytes, artifact.len());
        let expected_durability = if cfg!(unix) {
            DeliveryDurability::DirectorySynchronized
        } else {
            DeliveryDurability::PlatformUnsupported
        };
        assert_eq!(output_receipt.durability, expected_durability);

        let report = GenerationReport::empty();
        let report_attempt = session.publish_report(&report);
        if cfg!(unix) {
            assert!(report_attempt.failure.is_none());
        } else {
            assert_eq!(
                report_attempt.failure.as_ref().map(|failure| failure.code),
                Some(OperationCode::ReportDurabilityFailed)
            );
        }
        let DeliveryState::Published(report_receipt) = &report_attempt.delivery else {
            panic!(
                "report must retain a published receipt: {:?}",
                report_attempt.delivery
            );
        };
        assert_eq!(report_receipt.kind, DeliveryKind::Report);
        assert_eq!(report_receipt.durability, expected_durability);
        drop(session);

        assert_eq!(std::fs::read(&output).unwrap(), artifact.bytes());
        let report_bytes = std::fs::read(&report_path).unwrap();
        assert_eq!(report_receipt.bytes, report_bytes.len());
        assert_eq!(report_receipt.hash, crate::stable_hash(&report_bytes));
        let decoded: GenerationReport = serde_json::from_slice(&report_bytes).unwrap();
        assert_eq!(decoded, report);
        std::fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn artifact_lock_set_emits_versioned_aggregate_counts() {
        const CHILD_ENV: &str = "PYO3_BINDGEN_LOCK_SUMMARY_CHILD";
        const CHILD_ROLE: &str = "pyo3-bindgen-lock-summary-child-v1";
        const TEST_NAME: &str =
            "publication::interface_tests::artifact_lock_set_emits_versioned_aggregate_counts";

        if std::env::var(CHILD_ENV).as_deref() == Ok(CHILD_ROLE) {
            let subscriber = tracing_subscriber::fmt()
                .with_max_level(tracing::Level::DEBUG)
                .without_time()
                .with_target(false)
                // The parent asserts plain-text `key=value` substrings; ANSI styling would split
                // them, and the default depends on ambient `NO_COLOR`/`FORCE_COLOR`.
                .with_ansi(false)
                .with_writer(std::io::stderr)
                .finish();
            let _subscriber = tracing::subscriber::set_default(subscriber);
            let directory = test_directory();
            let output = directory.join("bindings.rs");
            let report = directory.join("report.json");
            let _session = PublicationPlan::snapshot(
                PublicationPolicy::cli(),
                None,
                Some(&output),
                Some(&report),
            )
            .unwrap()
            .acquire()
            .unwrap();
            return;
        }

        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", TEST_NAME, "--nocapture"])
            .env(CHILD_ENV, CHILD_ROLE)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "lock-summary child failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        let summaries = stderr
            .lines()
            .filter(|line| line.contains("artifact lock set acquired"))
            .collect::<Vec<_>>();
        assert_eq!(
            summaries.len(),
            1,
            "lock child must emit exactly one summary: {stderr}"
        );
        let summary = summaries[0];
        for field in [
            "observability_schema=1",
            "requested_targets=2",
            "lock_count=",
        ] {
            assert!(summary.contains(field), "missing {field:?}: {summary}");
        }
    }

    #[test]
    fn session_exposes_owned_target_descriptors_and_opens_owned_ir() {
        let directory = test_directory();
        let ir_path = directory.join("schema.json");
        let output_path = directory.join("bindings.rs");
        let report_path = directory.join("report.json");
        std::fs::write(&ir_path, "{\"modules\":[]}").unwrap();

        let plan = PublicationPlan::snapshot(
            PublicationPolicy::cli(),
            Some(&ir_path),
            Some(&output_path),
            Some(&report_path),
        )
        .unwrap();
        let plan_targets = plan.targets();
        assert_eq!(
            plan_targets.ir().unwrap().requested_path(),
            ir_path.as_path()
        );
        assert_eq!(plan_targets.output().unwrap().path(), output_path.as_path());
        assert_eq!(plan_targets.report().unwrap().path(), report_path.as_path());
        assert!(plan_targets.ir().unwrap().was_present_at_snapshot());

        let session = plan.acquire().unwrap();
        let mut opened = session.open_ir().unwrap();
        let mut contents = String::new();
        std::io::Read::read_to_string(&mut opened, &mut contents).unwrap();
        assert_eq!(contents, "{\"modules\":[]}");
        let session_targets = session.targets();
        assert_eq!(session_targets.ir().unwrap().requested_path(), ir_path);
        assert_eq!(session_targets.output().unwrap().path(), output_path);
        assert_eq!(session_targets.report().unwrap().path(), report_path);

        std::fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn cli_output_platform_unsupported_is_a_typed_durability_failure() {
        let artifact = RustArtifact::from_tokens(quote::quote! {
            pub fn generated() -> bool { true }
        })
        .unwrap();
        let attempt = output_attempt(
            PublicationPolicy::Cli,
            Path::new("bindings.rs"),
            &artifact,
            Ok(crate::artifact::ArtifactWriteReceipt {
                durability: ArtifactDurability::PlatformUnsupported,
            }),
            OperationCode::OutputWriteFailed,
            OperationCode::OutputPublicationUncertain,
            OperationCode::OutputDurabilityFailed,
        );

        assert!(matches!(
            attempt.delivery,
            DeliveryState::Published(DeliveryReceipt {
                durability: DeliveryDurability::PlatformUnsupported,
                ..
            })
        ));
        assert_eq!(
            attempt.failure.as_ref().map(|failure| failure.code),
            Some(OperationCode::OutputDurabilityFailed)
        );
    }

    #[test]
    fn unsupported_durability_mapping_is_cli_only_and_typed_by_delivery_kind() {
        let output_failure = unsupported_durability_failure(
            PublicationPolicy::Cli,
            DeliveryKind::OutputFile,
            Path::new("bindings.rs"),
        )
        .unwrap();
        assert_eq!(output_failure.code, OperationCode::OutputDurabilityFailed);

        let report_failure = unsupported_durability_failure(
            PublicationPolicy::Cli,
            DeliveryKind::Report,
            Path::new("report.json"),
        )
        .unwrap();
        assert_eq!(report_failure.code, OperationCode::ReportDurabilityFailed);

        assert!(unsupported_durability_failure(
            PublicationPolicy::Codegen,
            DeliveryKind::OutputFile,
            Path::new("bindings.rs"),
        )
        .is_none());
        assert!(unsupported_durability_failure(
            PublicationPolicy::Codegen,
            DeliveryKind::Report,
            Path::new("report.json"),
        )
        .is_none());
        assert!(unsupported_durability_failure(
            PublicationPolicy::Cli,
            DeliveryKind::StandardOutput,
            Path::new("stdout"),
        )
        .is_none());
    }

    #[test]
    fn unsupported_report_durability_classifies_as_partial_publication() {
        let state = RunState::new(
            DeliveryState::NotRequested,
            DeliveryState::Published(DeliveryReceipt {
                kind: DeliveryKind::Report,
                path: Some(PathBuf::from("report.json")),
                hash: "fnv1a64:0000000000000000".to_owned(),
                bytes: 0,
                durability: DeliveryDurability::PlatformUnsupported,
            }),
        );
        let outcome = RunOutcome::failed(
            RunFailure::new(OperationFailure::new(
                OperationCode::ReportDurabilityFailed,
                "report durability unsupported",
            )),
            state,
        );

        assert_eq!(outcome.exit_class(), ExitClass::PartialPublication);
        assert!(matches!(outcome, RunOutcome::PartialSuccess { .. }));
    }

    #[cfg(feature = "internal-test-hooks")]
    #[test]
    fn codegen_policy_keeps_platform_unsupported_as_success() {
        let directory = test_directory();
        let output = directory.join("bindings.rs");
        inject_output_publication_failure_once(TestPublicationFailure::PlatformUnsupported);
        let publication =
            PublicationPlan::snapshot(PublicationPolicy::Codegen, None, Some(&output), None)
                .unwrap()
                .acquire()
                .unwrap();
        let artifact = RustArtifact::from_tokens(quote::quote! {
            pub fn generated() -> bool { true }
        })
        .unwrap();

        let attempt = publication.publish_output(&artifact);

        assert!(attempt.failure.is_none());
        assert!(matches!(
            attempt.delivery,
            DeliveryState::Published(DeliveryReceipt {
                durability: DeliveryDurability::PlatformUnsupported,
                ..
            })
        ));
        assert!(output.exists());
        let _ = std::fs::remove_dir_all(directory);
    }
}

/// Write the generated artifact, honouring an injected failure when the test hooks are compiled in.
///
/// A default build has no injection state at all: this reduces to the real write, and the selector
/// type below does not exist. Exporting a way to fail a real publication is exactly what the
/// `internal-test-hooks` feature keeps out of published crates.
#[cfg(not(feature = "internal-test-hooks"))]
fn write_output_artifact(
    artifact: &RustArtifact,
    path: &Path,
) -> Result<crate::artifact::ArtifactWriteReceipt, ArtifactWriteFailure> {
    artifact.write_detailed(path)
}

#[cfg(not(feature = "internal-test-hooks"))]
fn write_report_artifact(
    artifact: &TextArtifact,
    path: &Path,
) -> Result<crate::artifact::ArtifactWriteReceipt, ArtifactWriteFailure> {
    artifact.write_detailed(path)
}

#[cfg(feature = "internal-test-hooks")]
mod test_hooks {
    use super::{ArtifactDurability, ArtifactWriteFailure, Path, RustArtifact, TextArtifact};
    use crate::artifact::ArtifactWriteReceipt;
    use std::io;

    /// Fault-injection selector used by the publication guarantee tests.
    #[doc(hidden)]
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum TestPublicationFailure {
        NotPublished,
        DirectorySyncFailed,
        PlatformUnsupported,
    }

    thread_local! {
        static INJECT_OUTPUT_FAILURE: std::cell::Cell<Option<TestPublicationFailure>> =
            const { std::cell::Cell::new(None) };
        static INJECT_REPORT_FAILURE: std::cell::Cell<Option<TestPublicationFailure>> =
            const { std::cell::Cell::new(None) };
    }

    #[doc(hidden)]
    pub fn inject_output_publication_failure_once(failure: TestPublicationFailure) {
        INJECT_OUTPUT_FAILURE.with(|slot| slot.set(Some(failure)));
    }

    #[doc(hidden)]
    pub fn inject_report_publication_failure_once(failure: TestPublicationFailure) {
        INJECT_REPORT_FAILURE.with(|slot| slot.set(Some(failure)));
    }

    fn injected(
        failure: TestPublicationFailure,
        write: impl Fn() -> Result<ArtifactWriteReceipt, ArtifactWriteFailure>,
        what: &'static str,
    ) -> Result<ArtifactWriteReceipt, ArtifactWriteFailure> {
        match failure {
            TestPublicationFailure::NotPublished => Err(ArtifactWriteFailure::NotPublished(
                io::Error::other(format!("injected {what} write failure")),
            )),
            TestPublicationFailure::DirectorySyncFailed => write().map(|_| ArtifactWriteReceipt {
                durability: ArtifactDurability::DirectorySyncFailed(io::Error::other(format!(
                    "injected {what} directory sync failure"
                ))),
            }),
            TestPublicationFailure::PlatformUnsupported => write().map(|_| ArtifactWriteReceipt {
                durability: ArtifactDurability::PlatformUnsupported,
            }),
        }
    }

    pub(super) fn write_output_artifact(
        artifact: &RustArtifact,
        path: &Path,
    ) -> Result<ArtifactWriteReceipt, ArtifactWriteFailure> {
        INJECT_OUTPUT_FAILURE
            .with(std::cell::Cell::take)
            .map_or_else(
                || artifact.write_detailed(path),
                |failure| injected(failure, || artifact.write_detailed(path), "output"),
            )
    }

    pub(super) fn write_report_artifact(
        artifact: &TextArtifact,
        path: &Path,
    ) -> Result<ArtifactWriteReceipt, ArtifactWriteFailure> {
        INJECT_REPORT_FAILURE
            .with(std::cell::Cell::take)
            .map_or_else(
                || artifact.write_detailed(path),
                |failure| injected(failure, || artifact.write_detailed(path), "report"),
            )
    }
}

#[cfg(feature = "internal-test-hooks")]
pub use test_hooks::{
    inject_output_publication_failure_once, inject_report_publication_failure_once,
    TestPublicationFailure,
};

#[cfg(feature = "internal-test-hooks")]
use test_hooks::{write_output_artifact, write_report_artifact};
