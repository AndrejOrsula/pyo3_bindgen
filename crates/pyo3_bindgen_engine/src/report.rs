use pyo3::types::PyAnyMethods;
use serde::ser::Error as _;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, hash::Hasher, path::Path};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenerationReport {
    pub schema_version: u32,
    /// Deterministic correlation fingerprint for this source/config operation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
    /// Whether source discovery produced a complete graph.
    #[serde(default = "report_complete_default")]
    pub complete: bool,
    pub generator: GeneratorMetadata,
    pub python: Option<PythonMetadata>,
    pub modules: Vec<ModuleReport>,
    pub diagnostics: Vec<GenerationDiagnostic>,
    pub outputs: Vec<OutputMetadata>,
}

const fn report_complete_default() -> bool {
    true
}

/// A generation report together with forward-compatible root-level extensions.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct GenerationReportDocument {
    #[serde(flatten)]
    pub report: GenerationReport,
    #[serde(flatten)]
    pub extensions: BTreeMap<String, serde_json::Value>,
}

impl Serialize for GenerationReportDocument {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut report = serde_json::to_value(&self.report).map_err(S::Error::custom)?;
        let fields = report
            .as_object_mut()
            .ok_or_else(|| S::Error::custom("generation report must serialize as a JSON object"))?;

        // `operation_id` is part of the report schema even when serde omits it from this
        // instance's output. Populate it only in the probe so collision checks follow serde's
        // field names without maintaining a duplicate list.
        let mut reserved_report = self.report.clone();
        if reserved_report.operation_id.is_none() {
            reserved_report.operation_id = Some(String::new());
        }
        let reserved_report = serde_json::to_value(reserved_report).map_err(S::Error::custom)?;
        let reserved_fields = reserved_report
            .as_object()
            .ok_or_else(|| S::Error::custom("generation report must serialize as a JSON object"))?;

        for (key, value) in &self.extensions {
            if reserved_fields.contains_key(key) {
                return Err(S::Error::custom(format!(
                    "report extension key '{key}' is reserved by GenerationReport"
                )));
            }
            fields.insert(key.clone(), value.clone());
        }
        report.serialize(serializer)
    }
}

impl GenerationReportDocument {
    /// Wrap a report in a document with no extension fields.
    #[must_use]
    pub fn new(report: GenerationReport) -> Self {
        Self {
            report,
            extensions: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GeneratorMetadata {
    pub name: String,
    pub version: String,
    pub pyo3_version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PythonMetadata {
    pub implementation: String,
    pub version: String,
    pub executable: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleReport {
    pub name: String,
    pub input_kind: String,
    pub input_hash: String,
    pub source_hash: Option<String>,
    /// Enum details for mirrors actually emitted into the Rust artifact.
    pub enums: Vec<EnumReport>,
    /// Items actually emitted after configuration, validation, renaming, and skips.
    pub counts: GeneratedCounts,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnumReport {
    pub name: String,
    pub kind: String,
    pub variants: Vec<EnumVariantReport>,
    pub aliases: Vec<EnumAliasReport>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnumVariantReport {
    pub python_name: String,
    pub rust_name: String,
    pub value_repr: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnumAliasReport {
    pub python_name: String,
    pub rust_name: String,
    pub target_python_name: String,
    pub target_rust_name: String,
    pub value_repr: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GeneratedCounts {
    pub imports: usize,
    pub submodules: usize,
    pub enums: usize,
    pub classes: usize,
    pub type_vars: usize,
    pub functions: usize,
    pub properties: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GenerationDiagnostic {
    pub severity: DiagnosticSeverity,
    pub code: String,
    pub path: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticSeverity {
    Info,
    Warning,
    Error,
}

macro_rules! known_diagnostic_codes {
    ($($variant:ident => $value:literal),+ $(,)?) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub(crate) enum KnownDiagnosticCode {
            $($variant),+
        }

        impl KnownDiagnosticCode {
            pub(crate) const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $value),+
                }
            }

            fn parse(value: &str) -> Option<Self> {
                match value {
                    $($value => Some(Self::$variant),)+
                    _ => None,
                }
            }
        }
    };
}

known_diagnostic_codes! {
    AsyncPropertySetterSkipped => "async_property_setter_skipped",
    AttributeAccessFailed => "attribute_access_failed",
    BaseClassOutOfScope => "base_class_out_of_scope",
    ClassAmbiguousWithoutForwarders => "class_ambiguous_without_forwarders",
    ClassAnnotationNameInvalid => "class_annotation_name_invalid",
    ClassAnnotationParseFallback => "class_annotation_parse_fallback",
    ClassAnnotationUnreadable => "class_annotation_unreadable",
    ClassAnnotationsUnreadable => "class_annotations_unreadable",
    ClassAttributeInaccessible => "class_attribute_inaccessible",
    ClassClosureSkipped => "class_closure_skipped",
    ClassDiamondWinnerUnresolved => "class_diamond_winner_unresolved",
    ClassForwarderIdentConflict => "class_forwarder_ident_conflict",
    ClassForwarderWinnerNotCallable => "class_forwarder_winner_not_callable",
    ClassImportSkipped => "class_import_skipped",
    ClassInvalidIdentRenamed => "class_invalid_ident_renamed",
    ClassInvalidIdentSkipped => "class_invalid_ident_skipped",
    ClassSubclassSkipped => "class_subclass_skipped",
    ClassSubmoduleSkipped => "class_submodule_skipped",
    ClassTypeVarSkipped => "class_type_var_skipped",
    EnumConfiguredRawFallback => "enum_configured_raw_fallback",
    EnumFlagFallback => "enum_flag_fallback",
    EnumIntenumOrdDropped => "enum_intenum_ord_dropped",
    EnumInvalidIdentRenamed => "enum_invalid_ident_renamed",
    EnumInvalidIdentSkipped => "enum_invalid_ident_skipped",
    EnumIrMirrorFallback => "enum_ir_mirror_fallback",
    EnumMirrorFallback => "enum_mirror_fallback",
    FunctionGeneratedNameRenamed => "function_generated_name_renamed",
    FunctionInvalidIdentRenamed => "function_invalid_ident_renamed",
    FunctionInvalidIdentSkipped => "function_invalid_ident_skipped",
    FunctionParameterBindingCollision => "function_parameter_binding_collision",
    FunctionSelfParameterRenamed => "function_self_parameter_renamed",
    FunctionSelfParametersReplaced => "function_self_parameters_replaced",
    ImportOriginUnresolved => "import_origin_unresolved",
    LiveAnnotationParseFallback => "live_annotation_parse_fallback",
    MethodInherentNameRenamed => "method_inherent_name_renamed",
    MethodSelfParametersReplaced => "method_self_parameters_replaced",
    MethodStaticFallback => "method_static_fallback",
    MethodUnknownTypeSkipped => "method_unknown_type_skipped",
    ModuleInvalidIdentRenamed => "module_invalid_ident_renamed",
    ModuleInvalidIdentSkipped => "module_invalid_ident_skipped",
    ProtocolNonRuntimeCheckableFallback => "protocol_non_runtime_checkable_fallback",
    SignatureAnnotationSourceUntrusted => "signature_annotation_source_untrusted",
    SubmoduleShadowedByImport => "submodule_shadowed_by_import",
    TypeVarInvalidIdentRenamed => "type_var_invalid_ident_renamed",
    TypeVarInvalidIdentSkipped => "type_var_invalid_ident_skipped",
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum DiscoverySubject {
    Submodule,
    Dependency,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum DiscoveryOutcome {
    Attempted,
    Emitted,
    Skipped,
    Failed,
    Truncated,
    Summary,
}

/// Typed internal view of the stable report-v1 string code.
///
/// Unknown strings remain round-trippable so a newer producer can be consumed without losing its
/// machine-readable code. The public serialized `GenerationDiagnostic::code` field stays unchanged.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum DiagnosticCode {
    Known(KnownDiagnosticCode),
    Discovery {
        subject: DiscoverySubject,
        outcome: DiscoveryOutcome,
    },
    Unknown(String),
}

impl DiagnosticCode {
    pub(crate) const fn discovery(subject: DiscoverySubject, outcome: DiscoveryOutcome) -> Self {
        Self::Discovery { subject, outcome }
    }

    pub(crate) fn as_str(&self) -> &str {
        match self {
            Self::Known(code) => code.as_str(),
            Self::Discovery { subject, outcome } => match (subject, outcome) {
                (DiscoverySubject::Submodule, DiscoveryOutcome::Attempted) => {
                    "discovery_submodule_attempted"
                }
                (DiscoverySubject::Submodule, DiscoveryOutcome::Emitted) => {
                    "discovery_submodule_emitted"
                }
                (DiscoverySubject::Submodule, DiscoveryOutcome::Skipped) => {
                    "discovery_submodule_skipped"
                }
                (DiscoverySubject::Submodule, DiscoveryOutcome::Failed) => {
                    "discovery_submodule_failed"
                }
                (DiscoverySubject::Submodule, DiscoveryOutcome::Truncated) => {
                    "discovery_submodule_truncated"
                }
                (DiscoverySubject::Submodule, DiscoveryOutcome::Summary) => {
                    "discovery_submodule_summary"
                }
                (DiscoverySubject::Dependency, DiscoveryOutcome::Attempted) => {
                    "discovery_dependency_attempted"
                }
                (DiscoverySubject::Dependency, DiscoveryOutcome::Emitted) => {
                    "discovery_dependency_emitted"
                }
                (DiscoverySubject::Dependency, DiscoveryOutcome::Skipped) => {
                    "discovery_dependency_skipped"
                }
                (DiscoverySubject::Dependency, DiscoveryOutcome::Failed) => {
                    "discovery_dependency_failed"
                }
                (DiscoverySubject::Dependency, DiscoveryOutcome::Truncated) => {
                    "discovery_dependency_truncated"
                }
                (DiscoverySubject::Dependency, DiscoveryOutcome::Summary) => {
                    "discovery_dependency_summary"
                }
            },
            Self::Unknown(value) => value,
        }
    }

    pub(crate) fn into_string(self) -> String {
        match self {
            Self::Unknown(value) => value,
            known => known.as_str().to_owned(),
        }
    }
}

impl From<KnownDiagnosticCode> for DiagnosticCode {
    fn from(value: KnownDiagnosticCode) -> Self {
        Self::Known(value)
    }
}

impl From<&str> for DiagnosticCode {
    fn from(value: &str) -> Self {
        if let Some(code) = KnownDiagnosticCode::parse(value) {
            return Self::Known(code);
        }
        match value {
            "discovery_submodule_attempted" => {
                Self::discovery(DiscoverySubject::Submodule, DiscoveryOutcome::Attempted)
            }
            "discovery_submodule_emitted" => {
                Self::discovery(DiscoverySubject::Submodule, DiscoveryOutcome::Emitted)
            }
            "discovery_submodule_skipped" => {
                Self::discovery(DiscoverySubject::Submodule, DiscoveryOutcome::Skipped)
            }
            "discovery_submodule_failed" => {
                Self::discovery(DiscoverySubject::Submodule, DiscoveryOutcome::Failed)
            }
            "discovery_submodule_truncated" => {
                Self::discovery(DiscoverySubject::Submodule, DiscoveryOutcome::Truncated)
            }
            "discovery_submodule_summary" => {
                Self::discovery(DiscoverySubject::Submodule, DiscoveryOutcome::Summary)
            }
            "discovery_dependency_attempted" => {
                Self::discovery(DiscoverySubject::Dependency, DiscoveryOutcome::Attempted)
            }
            "discovery_dependency_emitted" => {
                Self::discovery(DiscoverySubject::Dependency, DiscoveryOutcome::Emitted)
            }
            "discovery_dependency_skipped" => {
                Self::discovery(DiscoverySubject::Dependency, DiscoveryOutcome::Skipped)
            }
            "discovery_dependency_failed" => {
                Self::discovery(DiscoverySubject::Dependency, DiscoveryOutcome::Failed)
            }
            "discovery_dependency_truncated" => {
                Self::discovery(DiscoverySubject::Dependency, DiscoveryOutcome::Truncated)
            }
            "discovery_dependency_summary" => {
                Self::discovery(DiscoverySubject::Dependency, DiscoveryOutcome::Summary)
            }
            _ => Self::Unknown(value.to_owned()),
        }
    }
}

impl From<String> for DiagnosticCode {
    fn from(value: String) -> Self {
        match Self::from(value.as_str()) {
            Self::Unknown(_) => Self::Unknown(value),
            known => known,
        }
    }
}

/// Typed diagnostic used by acquisition, normalization, planning, and emission.
///
/// Serialization is deliberately unavailable here. Conversion to the stable public report DTO
/// happens only when report assembly crosses the engine boundary.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct DomainDiagnostic {
    pub(crate) severity: DomainSeverity,
    pub(crate) code: DiagnosticCode,
    pub(crate) path: Option<String>,
    pub(crate) message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum DomainSeverity {
    Info,
    Warning,
}

impl From<DomainSeverity> for DiagnosticSeverity {
    fn from(value: DomainSeverity) -> Self {
        match value {
            DomainSeverity::Info => Self::Info,
            DomainSeverity::Warning => Self::Warning,
        }
    }
}

impl DomainDiagnostic {
    pub(crate) fn typed_code(&self) -> DiagnosticCode {
        self.code.clone()
    }

    pub(crate) fn info(
        code: impl Into<DiagnosticCode>,
        path: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            severity: DomainSeverity::Info,
            code: code.into(),
            path: Some(path.into()),
            message: message.into(),
        }
    }

    pub(crate) fn warning(
        code: impl Into<DiagnosticCode>,
        path: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            severity: DomainSeverity::Warning,
            code: code.into(),
            path: Some(path.into()),
            message: message.into(),
        }
    }
}

impl From<DomainDiagnostic> for GenerationDiagnostic {
    fn from(value: DomainDiagnostic) -> Self {
        Self {
            severity: value.severity.into(),
            code: value.code.into_string(),
            path: value.path,
            message: value.message,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputMetadata {
    pub kind: String,
    /// Human-readable OS path. This is display-only and may be lossy when the
    /// underlying path is not valid Unicode.
    pub path: Option<String>,
    pub hash: String,
    pub bytes: usize,
}

impl OutputMetadata {
    /// Describe an output from the exact bytes delivered to its consumer.
    #[must_use]
    pub fn from_bytes(kind: impl Into<String>, path: Option<String>, bytes: &[u8]) -> Self {
        Self {
            kind: kind.into(),
            path,
            hash: stable_hash(bytes),
            bytes: bytes.len(),
        }
    }

    /// Describe an output from a [`std::fmt::Display`] value without ever materializing
    /// its rendering.
    ///
    /// Exactly equivalent to `Self::from_bytes(kind, path, value.to_string().as_bytes())`
    /// -- same hash, same byte count -- but it streams the rendering through the hasher
    /// instead of buffering it. On multi-megabyte token streams that removes the single
    /// largest transient allocation in the generation pipeline.
    #[must_use]
    pub(crate) fn from_display(
        kind: impl Into<String>,
        path: Option<String>,
        value: &impl std::fmt::Display,
    ) -> Self {
        let (hash, bytes) = stable_hash_display(value);
        Self {
            kind: kind.into(),
            path,
            hash,
            bytes,
        }
    }

    /// Describe a path-backed output without using its display form for I/O.
    #[must_use]
    pub fn from_path_bytes(kind: impl Into<String>, path: Option<&Path>, bytes: &[u8]) -> Self {
        Self::from_bytes(kind, path.map(|path| path.display().to_string()), bytes)
    }
}

impl GenerationReport {
    /// An report describing no generation at all, used to carry a standalone failure.
    ///
    /// Its `python` field is always `None`: nothing was introspected, so there is no
    /// interpreter to describe. This also keeps the constructor callable before (or
    /// entirely without) interpreter initialization, which the external-IR failure
    /// paths rely on.
    #[must_use]
    pub fn empty() -> Self {
        let mut report = Self::new_with_python(Vec::new(), Vec::new(), Vec::new(), None);
        report.operation_id = Some("unavailable".to_owned());
        report.complete = false;
        report
    }

    /// Construct a report with metadata captured by the source-acquisition boundary.
    pub(crate) fn new_with_python(
        modules: Vec<ModuleReport>,
        diagnostics: Vec<GenerationDiagnostic>,
        outputs: Vec<OutputMetadata>,
        python: Option<PythonMetadata>,
    ) -> Self {
        Self {
            schema_version: 1,
            operation_id: None,
            complete: true,
            generator: GeneratorMetadata {
                name: "pyo3_bindgen".to_owned(),
                version: env!("CARGO_PKG_VERSION").to_owned(),
                pyo3_version: option_env!("PYO3_BINDGEN_PYO3_VERSION")
                    .map(std::borrow::ToOwned::to_owned),
            },
            python,
            modules,
            diagnostics,
            outputs,
        }
    }

    #[must_use]
    pub fn warning_count(&self) -> usize {
        self.diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.severity == DiagnosticSeverity::Warning)
            .count()
    }

    #[must_use]
    pub fn error_count(&self) -> usize {
        self.diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.severity == DiagnosticSeverity::Error)
            .count()
    }

    pub fn push_error(
        &mut self,
        code: impl Into<String>,
        path: Option<String>,
        message: impl Into<String>,
    ) {
        self.diagnostics.push(GenerationDiagnostic {
            severity: DiagnosticSeverity::Error,
            code: code.into(),
            path,
            message: message.into(),
        });
    }
}

impl GenerationDiagnostic {
    pub(crate) fn typed_code(&self) -> DiagnosticCode {
        DiagnosticCode::from(self.code.clone())
    }

    pub(crate) fn info(
        code: impl Into<DiagnosticCode>,
        path: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            severity: DiagnosticSeverity::Info,
            code: code.into().into_string(),
            path: Some(path.into()),
            message: message.into(),
        }
    }

    #[cfg(test)]
    pub(crate) fn warning(
        code: impl Into<DiagnosticCode>,
        path: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            severity: DiagnosticSeverity::Warning,
            code: code.into().into_string(),
            path: Some(path.into()),
            message: message.into(),
        }
    }
}

pub(crate) fn warn(
    diagnostics: &mut Vec<DomainDiagnostic>,
    code: impl Into<DiagnosticCode>,
    path: impl Into<String>,
    message: impl Into<String>,
) {
    diagnostics.push(DomainDiagnostic::warning(code, path, message));
}

/// Emit only the warnings belonging to the generation result selected for delivery.
///
/// Live inheritance generation can speculatively build both deduplicated and flat
/// candidates. Printing while either candidate is built duplicates shared warnings and
/// leaks diagnostics from a candidate that is later discarded, so collection and emission
/// are deliberately separate.
pub(crate) fn emit_warnings(report: &GenerationReport) {
    for diagnostic in &report.diagnostics {
        if diagnostic.severity == DiagnosticSeverity::Warning {
            tracing::warn!(
                code = %diagnostic.code,
                path = ?diagnostic.path,
                message = %diagnostic.message,
                "generation diagnostic"
            );
        }
    }
}

#[must_use]
pub fn stable_hash(input: impl AsRef<[u8]>) -> String {
    let mut hasher = StableFnv1a64::default();
    hasher.write(input.as_ref());
    format!("fnv1a64:{:016x}", hasher.finish())
}

/// Stream a [`std::fmt::Display`] rendering into the stable hash without materializing it,
/// returning `(hash, rendered byte length)`.
///
/// Bit-for-bit equivalent to the pair
/// `(stable_hash(value.to_string().as_bytes()), value.to_string().len())`: [`ToString`] is
/// itself defined as `write!(buf, "{value}")`, so the sink below observes the exact same
/// `&str` fragments, in the same order, that would have been concatenated into the
/// intermediate `String`. Fragment *boundaries* are irrelevant because FNV-1a is a
/// byte-at-a-time state machine, so `hasher.write` is associative over any splitting of
/// the same byte sequence.
///
/// `format_args!("{value}")` carries no width, precision or fill, exactly as `to_string()`
/// does not, so even a `Display` impl that consulted [`std::fmt::Formatter`] options would
/// see the same (empty) ones on both paths. `proc_macro2`'s `Display for TokenStream`
/// reads none of them regardless -- it writes tokens straight through. The equivalence is
/// pinned against a silent upstream rendering change by
/// `token_stream_output_hash_matches_materialized_rendering` in `tests/bindgen.rs`.
fn stable_hash_display(value: &impl std::fmt::Display) -> (String, usize) {
    /// A `fmt::Write` sink that hashes and counts, but never stores.
    struct Sink {
        hasher: StableFnv1a64,
        len: usize,
    }

    impl std::fmt::Write for Sink {
        fn write_str(&mut self, s: &str) -> std::fmt::Result {
            self.hasher.write(s.as_bytes());
            self.len += s.len();
            Ok(())
        }
    }

    let mut sink = Sink {
        hasher: StableFnv1a64::default(),
        len: 0,
    };
    // `Sink::write_str` is infallible, so the only way this can fail is a `Display` impl
    // that returns `Err` on its own -- in which case the caller would have observed a
    // panic from `to_string()` instead. Nothing in this crate's hashed values does so.
    let _ = std::fmt::Write::write_fmt(&mut sink, format_args!("{value}"));
    (format!("fnv1a64:{:016x}", sink.hasher.finish()), sink.len)
}

struct StableFnv1a64(u64);

impl Default for StableFnv1a64 {
    fn default() -> Self {
        Self(0xcbf29ce484222325)
    }
}

impl Hasher for StableFnv1a64 {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(0x100000001b3);
        }
    }

    fn write_u8(&mut self, value: u8) {
        self.write(&value.to_le_bytes());
    }

    fn write_u16(&mut self, value: u16) {
        self.write(&value.to_le_bytes());
    }

    fn write_u32(&mut self, value: u32) {
        self.write(&value.to_le_bytes());
    }

    fn write_u64(&mut self, value: u64) {
        self.write(&value.to_le_bytes());
    }

    fn write_u128(&mut self, value: u128) {
        self.write(&value.to_le_bytes());
    }

    fn write_usize(&mut self, value: usize) {
        self.write_u64(value as u64);
    }

    fn write_i8(&mut self, value: i8) {
        self.write(&value.to_le_bytes());
    }

    fn write_i16(&mut self, value: i16) {
        self.write(&value.to_le_bytes());
    }

    fn write_i32(&mut self, value: i32) {
        self.write(&value.to_le_bytes());
    }

    fn write_i64(&mut self, value: i64) {
        self.write(&value.to_le_bytes());
    }

    fn write_i128(&mut self, value: i128) {
        self.write(&value.to_le_bytes());
    }

    fn write_isize(&mut self, value: isize) {
        self.write_i64(value as i64);
    }
}

pub(crate) fn python_metadata(py: pyo3::Python<'_>) -> Option<PythonMetadata> {
    let sys = py.import(pyo3::intern!(py, "sys")).ok()?;
    let platform = py.import(pyo3::intern!(py, "platform")).ok()?;
    Some(PythonMetadata {
        implementation: platform
            .call_method0(pyo3::intern!(py, "python_implementation"))
            .ok()?
            .extract()
            .ok()?,
        version: sys
            .getattr(pyo3::intern!(py, "version"))
            .ok()?
            .extract()
            .ok()?,
        executable: sys
            .getattr(pyo3::intern!(py, "executable"))
            .ok()
            .and_then(|value| value.extract::<String>().ok()),
    })
}

#[cfg(test)]
mod tests {
    use super::{
        stable_hash, stable_hash_display, DiagnosticCode, DiscoveryOutcome, DiscoverySubject,
        GenerationDiagnostic, GenerationReport, GenerationReportDocument, KnownDiagnosticCode,
        OutputMetadata,
    };
    use serde_json::json;

    #[test]
    fn typed_diagnostic_codes_preserve_stable_and_unknown_strings() {
        let known = DiagnosticCode::from("attribute_access_failed");
        assert_eq!(
            known,
            DiagnosticCode::Known(KnownDiagnosticCode::AttributeAccessFailed)
        );
        assert_eq!(known.as_str(), "attribute_access_failed");

        let discovery =
            DiagnosticCode::discovery(DiscoverySubject::Dependency, DiscoveryOutcome::Truncated);
        let diagnostic = GenerationDiagnostic::warning(
            discovery.clone(),
            "pkg.member",
            "bounded discovery stopped",
        );
        assert_eq!(diagnostic.code, "discovery_dependency_truncated");
        assert_eq!(diagnostic.typed_code(), discovery);

        let unknown = DiagnosticCode::from("future_diagnostic_code".to_owned());
        assert_eq!(unknown.as_str(), "future_diagnostic_code");
        assert_eq!(unknown.into_string(), "future_diagnostic_code");
    }

    #[test]
    fn report_document_new_flattens_the_report() {
        let report = GenerationReport::empty();
        let document = GenerationReportDocument::new(report.clone());

        assert_eq!(
            serde_json::to_value(&document).unwrap(),
            serde_json::to_value(report).unwrap()
        );
        assert!(document.extensions.is_empty());
    }

    #[test]
    fn report_document_round_trips_root_extensions() {
        let mut report = GenerationReport::empty();
        report.operation_id = Some("op-v1:test".to_owned());
        let mut document = GenerationReportDocument::new(report.clone());
        document.extensions.insert(
            "future_metadata".to_owned(),
            json!({"enabled": true, "version": 2}),
        );

        let encoded = serde_json::to_value(&document).unwrap();
        assert_eq!(encoded["operation_id"], "op-v1:test");
        assert_eq!(
            encoded["future_metadata"],
            json!({"enabled": true, "version": 2})
        );

        let decoded: GenerationReportDocument = serde_json::from_value(encoded).unwrap();
        assert_eq!(decoded.report, report);
        assert_eq!(decoded.extensions, document.extensions);
    }

    #[test]
    fn report_document_rejects_reserved_extension_keys() {
        let mut report = GenerationReport::empty();
        report.operation_id = None;
        let mut document = GenerationReportDocument::new(report);
        document.extensions.insert(
            "operation_id".to_owned(),
            json!("op-v1:fnv1a64:0000000000000000"),
        );

        let error = serde_json::to_value(document)
            .expect_err("extensions must not overwrite base report fields");
        assert!(
            error.to_string().contains("operation_id"),
            "collision error must identify the reserved field: {error}"
        );
    }

    /// A token stream exercising the rendering features that could plausibly make the
    /// streamed and materialized paths disagree: joint punctuation (which suppresses the
    /// separating space), nested groups of every delimiter, raw identifiers, non-ASCII
    /// and escaped string literals, doc comments, and lifetimes.
    fn adversarial_tokens() -> proc_macro2::TokenStream {
        quote::quote! {
            #[doc = " A docstring with \"quotes\", a backslash \\ and non-ASCII: αβγ 🦀."]
            pub struct r#struct<'a, T: ?Sized + 'a> {
                pub r#type: &'a T,
                pub tuple: (u8, [i64; 4], { }),
            }

            impl<'a, T: ?Sized + 'a> r#struct<'a, T> {
                pub fn shifts(x: i32) -> i32 {
                    let y = x << 2 >> 1;
                    let z = if y >= 0 && y != 1 { y..=3 } else { 0..=0 };
                    let _ = ::core::option::Option::<u8>::None;
                    z.start() + y - -1
                }
            }
        }
    }

    #[test]
    fn stable_hash_display_matches_materialized_rendering() {
        let tokens = adversarial_tokens();
        let rendered = tokens.to_string();
        // A trivial stream would not distinguish the two paths.
        assert!(rendered.len() > 200);

        let (hash, bytes) = stable_hash_display(&tokens);
        assert_eq!(hash, stable_hash(rendered.as_bytes()));
        assert_eq!(bytes, rendered.len());
    }

    #[test]
    fn from_display_matches_from_bytes() {
        let tokens = adversarial_tokens();
        let rendered = tokens.to_string();

        assert_eq!(
            OutputMetadata::from_display("token_stream", None, &tokens),
            OutputMetadata::from_bytes("token_stream", None, rendered.as_bytes())
        );
    }

    #[test]
    fn stable_hash_display_matches_on_empty_and_single_token_streams() {
        for tokens in [
            proc_macro2::TokenStream::new(),
            quote::quote!(x),
            quote::quote!(""),
        ] {
            let rendered = tokens.to_string();
            assert_eq!(
                stable_hash_display(&tokens),
                (stable_hash(rendered.as_bytes()), rendered.len()),
                "streamed hash diverged for `{rendered}`"
            );
        }
    }
}
