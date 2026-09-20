//! Engine for automatic generation of Rust FFI bindings to Python modules.

mod artifact;
mod codegen;
mod config;
mod emission;
mod external_ir;
mod live_operation;
mod normalized;
mod operation;
mod planning;
mod publication;
mod report;
mod resource;
mod runtime_sentinels;
mod schema;
mod schema_validation;
mod source;
mod syntax;
mod typing;
mod utils;

// Internal re-exports for convenience
use utils::io as io_utils;
use utils::result::Result;

// Public API re-exports
pub use artifact::{
    ArtifactDurability, ArtifactStatus, ArtifactWriteFailure, ArtifactWriteReceipt, RustArtifact,
    TextArtifact,
};
pub use codegen::{
    Codegen, GeneratedArtifact, GeneratedArtifactFailure, GeneratedBindings, GenerationWork,
};
pub use config::Config;

pub use syntax::naming::EnumVariantPlan;

/// Convert one Python enum member name to its owned Rust variant spelling.
#[must_use]
pub fn enum_variant_ident(python_name: &str) -> Option<String> {
    syntax::naming::enum_variant_ident_string(python_name)
}

/// Plan representable enum variants in input order, keeping the first Rust-name occurrence.
#[must_use]
pub fn plan_enum_variants(python_names: &[String]) -> Vec<EnumVariantPlan> {
    syntax::naming::plan_enum_variants(python_names)
}

#[doc(hidden)]
pub use publication::{
    artifact_identity_key, artifact_identity_path, artifact_lock_keys, artifact_lock_path,
    artifact_revalidation_identity_matches, conservative_casefold_identity_key,
    existing_file_identity, open_artifact_lock, open_ir_artifact_file, publish_report,
    validate_windows_artifact_component, ArtifactLockSet, ArtifactPath, ArtifactPathKind,
    ArtifactPaths, CurrentArtifactIdentity, DeliveryDurability, DeliveryKind, DeliveryReceipt,
    DeliveryState, ExitClass, IrArtifactOpenError, OperationCode, OperationFailure,
    PublicationAttempt, PublicationPlan, PublicationPolicy, PublicationSession, PublicationTarget,
    PublicationTargets, ReportWriteAttempt, RunFailure, RunOutcome, RunState,
};

/// Publication fault injection and lock-owner instrumentation for this workspace's own tests.
///
/// These items are deliberately absent from a default build: exporting them would let any
/// downstream caller fail a real publication. The CLI enables `internal-test-hooks` as a
/// dev-dependency feature, so they exist only while this workspace's tests are built.
#[cfg(feature = "internal-test-hooks")]
#[doc(hidden)]
pub use publication::{
    inject_output_publication_failure_once, inject_report_publication_failure_once,
    TestPublicationFailure, LOCK_OWNER_COUNTER,
};
#[cfg(windows)]
#[doc(hidden)]
pub use publication::{
    validate_windows_artifact_path, windows_file_id_identity,
    windows_legacy_file_identity_is_sound, windows_lexically_normalized_absolute_path,
};
pub use report::{
    stable_hash, DiagnosticSeverity, EnumAliasReport, EnumReport, EnumVariantReport,
    GeneratedCounts, GenerationDiagnostic, GenerationReport, GenerationReportDocument,
    GeneratorMetadata, ModuleReport, OutputMetadata, PythonMetadata,
};
pub use resource::{
    GenerationBudget, ResourceKind, ResourceLimitExceeded, ResourceMeasurementUnavailable,
    ResourcePhase,
};
pub use schema::{
    ClassIr, EnumAliasIr, EnumIr, EnumKindIr, EnumVariantIr, FunctionIr, FunctionKindIr, ImportIr,
    MethodKindIr, ModuleIr, ParameterIr, ParameterKindIr, PropertyIr, SchemaIr,
};
#[doc(hidden)]
pub use source::ExternalIrSourceContext;
pub use source::{BindingSource, LiveModules, SchemaSource};
pub use utils::{error::PyBindgenError, result::PyBindgenResult};
