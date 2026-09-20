//! External wire-input acquisition.
//!
//! Publication owns path identity and revalidation; this module owns the JSON transport
//! edge and returns the engine's transport-neutral `SchemaIr` value.

use crate::outcome::{OperationCode, OperationFailure};
use pyo3_bindgen::{publication, GenerationReport, IrArtifactOpenError};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fmt,
    io::{self, Read},
};

const CURRENT_SCHEMA_VERSION: u64 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExternalIrSnapshot {
    pub(crate) schema: pyo3_bindgen::SchemaIr,
    pub(crate) raw_bytes: Vec<u8>,
    pub(crate) sha256: String,
    pub(crate) byte_limit: usize,
    pub(crate) compatibility_warnings: Vec<String>,
}

impl ExternalIrSnapshot {
    pub(crate) fn warnings(&self) -> &[String] {
        &self.compatibility_warnings
    }
}

#[derive(Debug)]
pub(crate) enum ExternalIrInputError {
    Read(io::Error),
    TooLarge { limit: usize },
    Parse(serde_json::Error),
    UnknownField { path: String },
    UnsupportedVersion { version: u64 },
    InvalidVersionType { actual: &'static str },
    InvalidEnvelope { reason: String },
}

impl ExternalIrInputError {
    pub(crate) const fn code(&self) -> &'static str {
        match self {
            Self::Read(_) => "ir_read_failed",
            Self::TooLarge { .. } => "ir_input_too_large",
            Self::Parse(_) => "ir_parse_failed",
            Self::UnknownField { .. } => "ir_unknown_field",
            Self::UnsupportedVersion { .. } => "ir_unsupported_schema_version",
            Self::InvalidVersionType { .. } => "ir_schema_version_type",
            Self::InvalidEnvelope { .. } => "ir_invalid_envelope",
        }
    }

    fn report_path(&self) -> Option<String> {
        match self {
            Self::UnknownField { path } => Some(path.clone()),
            Self::UnsupportedVersion { .. } | Self::InvalidVersionType { .. } => {
                Some("schema_version".to_owned())
            }
            Self::Read(_)
            | Self::TooLarge { .. }
            | Self::Parse(_)
            | Self::InvalidEnvelope { .. } => None,
        }
    }
}

impl fmt::Display for ExternalIrInputError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read(error) => write!(formatter, "failed to read external IR: {error}"),
            Self::TooLarge { limit } => {
                write!(
                    formatter,
                    "external IR exceeds the configured limit of {limit} bytes"
                )
            }
            Self::Parse(error) => write!(formatter, "failed to parse external IR: {error}"),
            Self::UnknownField { path } => {
                write!(formatter, "unknown field in current external IR at {path}")
            }
            Self::UnsupportedVersion { version } => write!(
                formatter,
                "unsupported external IR schema version {version}; supported version is {CURRENT_SCHEMA_VERSION}"
            ),
            Self::InvalidVersionType { actual } => write!(
                formatter,
                "external IR schema_version must be an unsigned integer, got {actual}"
            ),
            Self::InvalidEnvelope { reason } => {
                write!(formatter, "invalid external IR envelope: {reason}")
            }
        }
    }
}

impl std::error::Error for ExternalIrInputError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read(error) => Some(error),
            Self::TooLarge { .. } => None,
            Self::Parse(error) => Some(error),
            Self::UnknownField { .. }
            | Self::UnsupportedVersion { .. }
            | Self::InvalidVersionType { .. }
            | Self::InvalidEnvelope { .. } => None,
        }
    }
}

pub(crate) fn read_external_ir_snapshot<R: Read>(
    reader: R,
    limit: usize,
) -> Result<ExternalIrSnapshot, ExternalIrInputError> {
    let mut raw_bytes = Vec::new();
    reader
        .take((limit as u64).saturating_add(1))
        .read_to_end(&mut raw_bytes)
        .map_err(ExternalIrInputError::Read)?;
    if raw_bytes.len() > limit {
        return Err(ExternalIrInputError::TooLarge { limit });
    }

    let (schema, compatibility_warnings) = decode_wire_document(&raw_bytes)?;
    let digest = Sha256::digest(&raw_bytes);
    let mut sha256 = String::with_capacity("sha256:".len() + digest.len() * 2);
    sha256.push_str("sha256:");
    for byte in digest {
        use fmt::Write as _;
        write!(sha256, "{byte:02x}").expect("writing to a String cannot fail");
    }

    Ok(ExternalIrSnapshot {
        schema,
        raw_bytes,
        sha256,
        byte_limit: limit,
        compatibility_warnings,
    })
}

fn decode_wire_document(
    raw_bytes: &[u8],
) -> Result<(pyo3_bindgen::SchemaIr, Vec<String>), ExternalIrInputError> {
    let document =
        serde_json::from_slice::<Value>(raw_bytes).map_err(ExternalIrInputError::Parse)?;
    let Some(object) = document.as_object() else {
        return decode_legacy_schema(raw_bytes);
    };

    // Preserve exact root-value lexemes for the version discriminator. A normal
    // `serde_json::Value` may round a long fractional JSON number through `f64`, which
    // could turn a non-integer near 1 into the accepted version. `RawValue` keeps the
    // source spelling while the ordinary `Value` remains convenient for strict shape
    // validation below.
    let raw_object: BTreeMap<String, Box<serde_json::value::RawValue>> =
        serde_json::from_slice(raw_bytes).map_err(ExternalIrInputError::Parse)?;

    // A legacy `SchemaIr` accepted and ignored arbitrary root fields before the
    // envelope existed. Require both envelope-defining keys so a legacy producer's
    // unrelated `schema_version` metadata remains a warned legacy field instead of
    // silently opting into different decoding rules.
    let (Some(version_value), Some(version_raw), Some(schema)) = (
        object.get("schema_version"),
        raw_object.get("schema_version"),
        object.get("schema"),
    ) else {
        return decode_legacy_schema(raw_bytes);
    };
    let version = json_unsigned_integer(version_raw.get()).ok_or(
        ExternalIrInputError::InvalidVersionType {
            actual: json_type_name(version_value),
        },
    )?;
    if version != CURRENT_SCHEMA_VERSION {
        return Err(ExternalIrInputError::UnsupportedVersion { version });
    }

    reject_unknown_fields(object, "", &["schema_version", "schema", "extensions"])?;

    if object
        .get("extensions")
        .is_some_and(|extensions| !extensions.is_object())
    {
        return Err(ExternalIrInputError::InvalidEnvelope {
            reason: "`extensions` must be an object".to_owned(),
        });
    }

    validate_current_schema(schema, "schema")?;
    let schema = serde_json::from_value(schema.clone()).map_err(ExternalIrInputError::Parse)?;
    Ok((schema, Vec::new()))
}

/// Interpret a JSON number using JSON Schema's mathematical `integer` semantics.
///
/// `1`, `1.0`, and `1e0` are the same integer in JSON Schema. `serde_json::Number::as_u64`
/// deliberately preserves lexical representation and therefore rejects the latter two;
/// normalize the decimal/exponent form exactly instead of rounding through `f64`.
fn json_unsigned_integer(raw: &str) -> Option<u64> {
    if raw.starts_with('-') {
        return None;
    }

    let (mantissa, exponent) = match raw.split_once(['e', 'E']) {
        Some((mantissa, exponent)) => (mantissa, exponent.parse::<i64>().ok()?),
        None => (raw, 0_i64),
    };
    let (integer, fraction) = mantissa
        .split_once('.')
        .map_or((mantissa, ""), |(integer, fraction)| (integer, fraction));
    let mut digits = String::with_capacity(integer.len() + fraction.len());
    digits.push_str(integer);
    digits.push_str(fraction);

    if digits.bytes().all(|byte| byte == b'0') {
        return Some(0);
    }

    let shift = exponent.checked_sub(i64::try_from(fraction.len()).ok()?)?;
    if shift >= 0 {
        let shift = u32::try_from(shift).ok()?;
        let value = digits.parse::<u64>().ok()?;
        value.checked_mul(10_u64.checked_pow(shift)?)
    } else {
        let discarded = usize::try_from(shift.unsigned_abs()).ok()?;
        if discarded > digits.len() {
            return None;
        }
        let retained = digits.len() - discarded;
        if !digits.as_bytes()[retained..]
            .iter()
            .all(|byte| *byte == b'0')
        {
            return None;
        }
        let significant = &digits[..retained];
        if significant.is_empty() {
            Some(0)
        } else {
            significant.parse::<u64>().ok()
        }
    }
}

fn decode_legacy_schema(
    raw_bytes: &[u8],
) -> Result<(pyo3_bindgen::SchemaIr, Vec<String>), ExternalIrInputError> {
    let mut deserializer = serde_json::Deserializer::from_slice(raw_bytes);
    let mut warnings = Vec::new();
    let schema = serde_ignored::deserialize(&mut deserializer, |path| {
        warnings.push(path.to_string());
    })
    .map_err(ExternalIrInputError::Parse)?;
    deserializer.end().map_err(ExternalIrInputError::Parse)?;
    Ok((schema, warnings))
}

fn json_type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn reject_unknown_fields(
    object: &serde_json::Map<String, Value>,
    parent_path: &str,
    allowed: &[&str],
) -> Result<(), ExternalIrInputError> {
    let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
    keys.sort_unstable();
    if let Some(key) = keys.into_iter().find(|key| !allowed.contains(key)) {
        return Err(ExternalIrInputError::UnknownField {
            path: qualified_path(parent_path, key),
        });
    }
    Ok(())
}

fn qualified_path(parent_path: &str, child: impl fmt::Display) -> String {
    if parent_path.is_empty() {
        child.to_string()
    } else {
        format!("{parent_path}.{child}")
    }
}

fn validate_current_schema(value: &Value, path: &str) -> Result<(), ExternalIrInputError> {
    let Some(object) = value.as_object() else {
        return Ok(());
    };
    reject_unknown_fields(object, path, &["modules"])?;
    if let Some(modules) = object.get("modules").and_then(Value::as_array) {
        for (index, module) in modules.iter().enumerate() {
            validate_module(module, &qualified_path(path, format!("modules.{index}")))?;
        }
    }
    Ok(())
}

fn validate_module(value: &Value, path: &str) -> Result<(), ExternalIrInputError> {
    let Some(object) = value.as_object() else {
        return Ok(());
    };
    reject_unknown_fields(
        object,
        path,
        &[
            "name",
            "docstring",
            "is_package",
            "classes",
            "enums",
            "functions",
            "properties",
            "submodules",
            "prelude",
            "imports",
            "type_vars",
        ],
    )?;
    if let Some(classes) = object.get("classes").and_then(Value::as_array) {
        for (index, class) in classes.iter().enumerate() {
            validate_class(class, &qualified_path(path, format!("classes.{index}")))?;
        }
    }
    if let Some(enums) = object.get("enums").and_then(Value::as_array) {
        for (index, enum_value) in enums.iter().enumerate() {
            validate_enum(enum_value, &qualified_path(path, format!("enums.{index}")))?;
        }
    }
    if let Some(functions) = object.get("functions").and_then(Value::as_array) {
        for (index, function) in functions.iter().enumerate() {
            validate_function(
                function,
                &qualified_path(path, format!("functions.{index}")),
            )?;
        }
    }
    if let Some(properties) = object.get("properties").and_then(Value::as_array) {
        for (index, property) in properties.iter().enumerate() {
            validate_property(
                property,
                &qualified_path(path, format!("properties.{index}")),
            )?;
        }
    }
    if let Some(submodules) = object.get("submodules").and_then(Value::as_array) {
        for (index, submodule) in submodules.iter().enumerate() {
            validate_module(
                submodule,
                &qualified_path(path, format!("submodules.{index}")),
            )?;
        }
    }
    if let Some(imports) = object.get("imports").and_then(Value::as_array) {
        for (index, import) in imports.iter().enumerate() {
            validate_import(import, &qualified_path(path, format!("imports.{index}")))?;
        }
    }
    Ok(())
}

fn validate_class(value: &Value, path: &str) -> Result<(), ExternalIrInputError> {
    let Some(object) = value.as_object() else {
        return Ok(());
    };
    reject_unknown_fields(
        object,
        path,
        &["name", "base_classes", "docstring", "methods", "properties"],
    )?;
    if let Some(methods) = object.get("methods").and_then(Value::as_array) {
        for (index, method) in methods.iter().enumerate() {
            validate_function(method, &qualified_path(path, format!("methods.{index}")))?;
        }
    }
    if let Some(properties) = object.get("properties").and_then(Value::as_array) {
        for (index, property) in properties.iter().enumerate() {
            validate_property(
                property,
                &qualified_path(path, format!("properties.{index}")),
            )?;
        }
    }
    Ok(())
}

fn validate_function(value: &Value, path: &str) -> Result<(), ExternalIrInputError> {
    let Some(object) = value.as_object() else {
        return Ok(());
    };
    reject_unknown_fields(
        object,
        path,
        &["name", "kind", "params", "return_type", "docstring"],
    )?;
    if let Some(kind) = object.get("kind") {
        validate_function_kind(kind, &qualified_path(path, "kind"))?;
    }
    if let Some(params) = object.get("params").and_then(Value::as_array) {
        for (index, parameter) in params.iter().enumerate() {
            validate_parameter(parameter, &qualified_path(path, format!("params.{index}")))?;
        }
    }
    Ok(())
}

fn validate_function_kind(value: &Value, path: &str) -> Result<(), ExternalIrInputError> {
    let Some(object) = value.as_object() else {
        return Ok(());
    };
    reject_unknown_fields(object, path, &["Method"])?;
    if let Some(method) = object.get("Method") {
        if let Some(method_object) = method.as_object() {
            reject_unknown_fields(
                method_object,
                &qualified_path(path, "Method"),
                &["method_type"],
            )?;
        }
    }
    Ok(())
}

fn validate_parameter(value: &Value, path: &str) -> Result<(), ExternalIrInputError> {
    let Some(object) = value.as_object() else {
        return Ok(());
    };
    reject_unknown_fields(object, path, &["name", "kind", "annotation", "has_default"])
}

fn validate_property(value: &Value, path: &str) -> Result<(), ExternalIrInputError> {
    let Some(object) = value.as_object() else {
        return Ok(());
    };
    reject_unknown_fields(
        object,
        path,
        &[
            "name",
            "py_type",
            "setter_type",
            "readonly",
            "docstring",
            "setter_docstring",
        ],
    )
}

fn validate_enum(value: &Value, path: &str) -> Result<(), ExternalIrInputError> {
    let Some(object) = value.as_object() else {
        return Ok(());
    };
    reject_unknown_fields(
        object,
        path,
        &[
            "name",
            "kind",
            "variants",
            "aliases",
            "docstring",
            "is_flag",
            "is_open",
            "string_backed",
            "plain_mirror",
        ],
    )?;
    if let Some(variants) = object.get("variants").and_then(Value::as_array) {
        for (index, variant) in variants.iter().enumerate() {
            validate_enum_variant(variant, &qualified_path(path, format!("variants.{index}")))?;
        }
    }
    if let Some(aliases) = object.get("aliases").and_then(Value::as_array) {
        for (index, alias) in aliases.iter().enumerate() {
            validate_enum_alias(alias, &qualified_path(path, format!("aliases.{index}")))?;
        }
    }
    Ok(())
}

fn validate_enum_variant(value: &Value, path: &str) -> Result<(), ExternalIrInputError> {
    let Some(object) = value.as_object() else {
        return Ok(());
    };
    reject_unknown_fields(object, path, &["py_name", "value_repr"])
}

fn validate_enum_alias(value: &Value, path: &str) -> Result<(), ExternalIrInputError> {
    let Some(object) = value.as_object() else {
        return Ok(());
    };
    reject_unknown_fields(object, path, &["py_name", "target_py_name", "value_repr"])
}

fn validate_import(value: &Value, path: &str) -> Result<(), ExternalIrInputError> {
    let Some(object) = value.as_object() else {
        return Ok(());
    };
    reject_unknown_fields(object, path, &["origin", "target"])
}

pub(crate) struct ExternalIrLoadFailure {
    pub(crate) operation: OperationFailure,
}

fn external_ir_failure(
    publication: &mut publication::Session,
    operation_code: OperationCode,
    diagnostic_code: &'static str,
    path: Option<String>,
    message: String,
) -> ExternalIrLoadFailure {
    let mut report = GenerationReport::empty();
    report.push_error(diagnostic_code, path, message.clone());
    let _ = publication.publish_report_recovery(&report);
    ExternalIrLoadFailure {
        operation: OperationFailure::new(operation_code, message),
    }
}

pub(crate) fn external_ir_open_failure(
    publication: &mut publication::Session,
    error: IrArtifactOpenError,
) -> ExternalIrLoadFailure {
    let (operation_code, diagnostic_code) = match &error {
        IrArtifactOpenError::NoPath => (OperationCode::InputReadFailed, "ir_read_failed"),
        IrArtifactOpenError::Read(_) => (OperationCode::InputReadFailed, "ir_read_failed"),
        IrArtifactOpenError::Revalidation(_) => (
            OperationCode::ArtifactRevalidationFailed,
            "artifact_revalidation_failed",
        ),
    };
    external_ir_failure(
        publication,
        operation_code,
        diagnostic_code,
        None,
        format!(
            "Failed to read IR file {}: {error}",
            publication
                .ir_target()
                .map(|target| target.requested_path().display().to_string())
                .unwrap_or_else(|| "<unspecified>".to_owned())
        ),
    )
}

pub(crate) fn load_external_schema_snapshot(
    publication: &mut publication::Session,
    limit: usize,
) -> Result<ExternalIrSnapshot, ExternalIrLoadFailure> {
    let Some(ir_target) = publication.ir_target() else {
        let message = "publication request has no IR path".to_owned();
        return Err(external_ir_failure(
            publication,
            OperationCode::InputReadFailed,
            "ir_read_failed",
            None,
            message,
        ));
    };
    let ir_path = ir_target.requested_path().to_owned();
    let was_present_at_snapshot = ir_target.was_present_at_snapshot();
    let span = tracing::info_span!(
        "pyo3_bindgen.cli.ir.read",
        path = %ir_path.display(),
    );
    let _entered = span.enter();

    if let Err(error) = publication.revalidate() {
        let operation_code = match std::fs::metadata(&ir_path) {
            Err(error)
                if !was_present_at_snapshot
                    && matches!(
                        error.kind(),
                        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                    ) =>
            {
                OperationCode::InputReadFailed
            }
            _ => OperationCode::ArtifactRevalidationFailed,
        };
        let diagnostic_code = if operation_code == OperationCode::InputReadFailed {
            "ir_read_failed"
        } else {
            "artifact_revalidation_failed"
        };
        let message = format!("Failed to read IR file {}: {error}", ir_path.display());
        return Err(external_ir_failure(
            publication,
            operation_code,
            diagnostic_code,
            None,
            message,
        ));
    }
    let ir_file = publication
        .open_ir()
        .map_err(|error| external_ir_open_failure(publication, error))?;
    if let Err(error) = publication.revalidate() {
        let message = format!("Failed to read IR file {}: {error}", ir_path.display());
        return Err(external_ir_failure(
            publication,
            OperationCode::ArtifactRevalidationFailed,
            "artifact_revalidation_failed",
            None,
            message,
        ));
    }

    let absolute_path = ir_path;
    let snapshot = read_external_ir_snapshot(ir_file, limit).map_err(|error| {
        let (operation_code, diagnostic_code, path, message) = match error {
            ExternalIrInputError::Read(error) => (
                OperationCode::InputReadFailed,
                "ir_read_failed",
                None,
                format!(
                    "Failed to read IR file {}: {error}",
                    absolute_path.display()
                ),
            ),
            ExternalIrInputError::TooLarge { limit } => (
                OperationCode::InputLimitExceeded,
                "ir_input_too_large",
                None,
                format!(
                    "IR file {} exceeds the configured limit of {limit} bytes",
                    absolute_path.display()
                ),
            ),
            ExternalIrInputError::Parse(error) => (
                OperationCode::InputParseFailed,
                "ir_parse_failed",
                None,
                format!(
                    "Failed to parse IR file {}: {error}",
                    absolute_path.display()
                ),
            ),
            error => {
                let code = error.code();
                let path = error.report_path();
                (
                    OperationCode::InputContractFailed,
                    code,
                    path,
                    format!(
                        "Failed to decode IR file {}: {error}",
                        absolute_path.display()
                    ),
                )
            }
        };
        external_ir_failure(publication, operation_code, diagnostic_code, path, message)
    })?;
    for path in snapshot.warnings() {
        tracing::warn!(
            code = "ir_legacy_unknown_field",
            path = %path,
            "legacy external IR contains an unknown field"
        );
    }
    tracing::debug!(
        bytes = snapshot.raw_bytes.len(),
        sha256 = %snapshot.sha256,
        "external IR read"
    );

    Ok(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    const VALID_IR: &[u8] = b"{\"modules\":[]}\n";

    fn current_envelope(schema: serde_json::Value) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "schema_version": 1,
            "schema": schema,
            "extensions": {},
        }))
        .unwrap()
    }

    #[test]
    fn legacy_decode_is_byte_equivalent_and_authoritative() {
        let raw_bytes = br#"{"modules":[]}
"#
        .to_vec();

        let snapshot = read_external_ir_snapshot(Cursor::new(raw_bytes.clone()), raw_bytes.len())
            .expect("legacy v1 input should decode");

        assert_eq!(snapshot.raw_bytes, raw_bytes);
        assert_eq!(snapshot.schema, serde_json::from_slice(&raw_bytes).unwrap());
        assert!(snapshot.warnings().is_empty());
    }

    #[test]
    fn legacy_unknown_nested_field_returns_path_qualified_warning() {
        let raw_bytes = br#"{
            "modules": [{
                "name": "mymod",
                "is_package": false,
                "classes": [{
                    "name": "mymod.MyClass",
                    "legacy_typo": true
                }]
            }]
        }"#;

        let snapshot = read_external_ir_snapshot(Cursor::new(raw_bytes), raw_bytes.len())
            .expect("legacy unknown fields remain compatible");

        assert_eq!(
            snapshot.warnings(),
            ["modules.0.classes.0.legacy_typo".to_owned()]
        );
    }

    #[test]
    fn legacy_schema_version_metadata_does_not_claim_the_envelope() {
        let raw_bytes = br#"{
            "modules": [],
            "schema_version": "producer-metadata"
        }"#;

        let snapshot = read_external_ir_snapshot(Cursor::new(raw_bytes), raw_bytes.len())
            .expect("a legacy unknown field named schema_version remains compatible");

        assert_eq!(snapshot.schema.modules.len(), 0);
        assert_eq!(snapshot.warnings(), ["schema_version".to_owned()]);
    }

    #[test]
    fn current_version_rejects_unknown_fields_inside_schema() {
        let schema = serde_json::json!({
            "modules": [{
                "name": "mymod",
                "is_package": false,
                "current_typo": true
            }]
        });
        let raw_bytes = current_envelope(schema);

        let error = read_external_ir_snapshot(Cursor::new(raw_bytes), usize::MAX)
            .expect_err("current version must reject unknown schema fields");

        assert_eq!(error.code(), "ir_unknown_field");
        assert!(matches!(
            error,
            ExternalIrInputError::UnknownField { path }
                if path == "schema.modules.0.current_typo"
        ));
    }

    #[test]
    fn current_version_accepts_arbitrary_extension_values() {
        let raw_bytes = serde_json::to_vec(&serde_json::json!({
            "schema_version": 1,
            "schema": {"modules": []},
            "extensions": {
                "producer": {
                    "nested": [null, true, {"unknown": "is allowed here"}]
                }
            }
        }))
        .unwrap();

        let snapshot = read_external_ir_snapshot(Cursor::new(raw_bytes), usize::MAX)
            .expect("extension values are intentionally unconstrained");

        assert_eq!(snapshot.schema.modules.len(), 0);
        assert!(snapshot.warnings().is_empty());
    }

    #[test]
    fn current_and_legacy_decode_to_equal_schemas() {
        let legacy = serde_json::json!({
            "modules": [{"name": "mymod", "is_package": false}]
        });
        let legacy_bytes = serde_json::to_vec(&legacy).unwrap();
        let current_bytes = current_envelope(legacy.clone());

        let legacy_snapshot =
            read_external_ir_snapshot(Cursor::new(legacy_bytes), usize::MAX).unwrap();
        let current_snapshot =
            read_external_ir_snapshot(Cursor::new(current_bytes), usize::MAX).unwrap();

        assert_eq!(legacy_snapshot.schema, current_snapshot.schema);
    }

    #[test]
    fn future_schema_version_is_rejected_before_schema_decode() {
        let raw_bytes = serde_json::to_vec(&serde_json::json!({
            "schema_version": 2,
            "schema": {"not_a_schema": true},
            "extensions": {}
        }))
        .unwrap();

        let error = read_external_ir_snapshot(Cursor::new(raw_bytes), usize::MAX)
            .expect_err("future versions must fail at the wire boundary");

        assert_eq!(error.code(), "ir_unsupported_schema_version");
        assert!(matches!(
            error,
            ExternalIrInputError::UnsupportedVersion { version: 2 }
        ));
    }

    #[test]
    fn wrong_type_schema_version_is_a_typed_contract_error() {
        let raw_bytes = serde_json::to_vec(&serde_json::json!({
            "schema_version": "1",
            "schema": {"modules": []},
            "extensions": {}
        }))
        .unwrap();

        let error = read_external_ir_snapshot(Cursor::new(raw_bytes), usize::MAX)
            .expect_err("schema_version type must be validated explicitly");

        assert_eq!(error.code(), "ir_schema_version_type");
        assert!(matches!(
            error,
            ExternalIrInputError::InvalidVersionType { .. }
        ));
    }

    #[test]
    fn mathematically_integral_current_version_spellings_are_accepted() {
        for version in ["1.0", "1e0"] {
            let raw_bytes = format!(
                r#"{{"schema_version":{version},"schema":{{"modules":[]}},"extensions":{{}}}}"#
            );

            let snapshot =
                read_external_ir_snapshot(Cursor::new(raw_bytes.as_bytes()), raw_bytes.len())
                    .unwrap_or_else(|error| {
                        panic!("version spelling {version} must decode: {error}")
                    });
            assert!(snapshot.schema.modules.is_empty());
        }
    }

    #[test]
    fn near_integral_fractional_version_is_rejected_without_rounding() {
        let raw_bytes = br#"{"schema_version":1.00000000000000001,"schema":{"modules":[]}}"#;

        let error = read_external_ir_snapshot(Cursor::new(raw_bytes), raw_bytes.len())
            .expect_err("a non-integral version must not round to the current integer");

        assert!(matches!(
            error,
            ExternalIrInputError::InvalidVersionType { .. }
        ));
    }

    #[derive(Debug, serde::Deserialize)]
    #[serde(rename_all = "snake_case")]
    enum GenerationExpectation {
        Accepted,
        TypedCodegenError,
        NotReached,
    }

    #[derive(Debug, serde::Deserialize)]
    struct ExternalIrConformanceCase {
        name: String,
        raw: String,
        schema_valid: bool,
        cli_accepts: bool,
        cli_error_code: Option<String>,
        dto_accepts: bool,
        schema_dto_accepts: bool,
        generation_expectation: GenerationExpectation,
        warning_paths: Vec<String>,
        version_lexeme: Option<String>,
    }

    #[test]
    fn external_ir_hand_authored_corpus_cross_checks_schema_cli_and_dto() {
        let checked_schema: Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../schemas/schema_ir.v1.json"
        )))
        .expect("checked external IR schema is JSON");
        let validator = jsonschema::draft202012::new(&checked_schema)
            .expect("checked external IR schema should compile independently");
        let cases: Vec<ExternalIrConformanceCase> = serde_json::from_str(include_str!(
            "../tests/fixtures/external_ir_conformance.json"
        ))
        .expect("external IR conformance fixture is JSON");
        assert!(
            cases.len() >= 8,
            "the conformance corpus should cover current, legacy, and DTO boundaries"
        );
        assert!(
            cases.iter().any(|case| {
                case.cli_accepts
                    && matches!(
                        &case.generation_expectation,
                        GenerationExpectation::TypedCodegenError
                    )
            }),
            "the conformance corpus must exercise a CLI-accepted semantic generation rejection"
        );

        for case in cases {
            let raw_value: Value = serde_json::from_str(&case.raw)
                .unwrap_or_else(|error| panic!("{} is not valid JSON: {error}", case.name));
            assert_eq!(
                validator.is_valid(&raw_value),
                case.schema_valid,
                "checked schema result differs for {}",
                case.name
            );

            let cli_result =
                read_external_ir_snapshot(Cursor::new(case.raw.as_bytes()), case.raw.len());
            let decoded_schema = match (case.cli_accepts, case.cli_error_code.as_deref()) {
                (true, None) => {
                    let snapshot = cli_result
                        .unwrap_or_else(|error| panic!("{} should decode: {error}", case.name));
                    assert_eq!(snapshot.raw_bytes, case.raw.as_bytes(), "{}", case.name);
                    assert_eq!(snapshot.warnings(), case.warning_paths, "{}", case.name);
                    Some(snapshot.schema)
                }
                (false, Some(expected_code)) => {
                    let error = cli_result
                        .expect_err("a rejected corpus case must fail in the CLI decoder");
                    assert_eq!(error.code(), expected_code, "{}", case.name);
                    assert!(case.warning_paths.is_empty(), "{}", case.name);
                    None
                }
                (true, Some(_)) | (false, None) => {
                    panic!("{} has an inconsistent CLI expectation", case.name)
                }
            };

            // Current documents are envelopes; legacy documents are bare SchemaIr values.
            // Keep this full-document DTO result distinct from the inner current payload below.
            let dto_result = serde_json::from_str::<pyo3_bindgen::SchemaIr>(&case.raw);
            assert_eq!(
                dto_result.is_ok(),
                case.dto_accepts,
                "public SchemaIr DTO result differs for {}",
                case.name
            );

            let schema_dto_result = raw_value.get("schema").map_or_else(
                || serde_json::from_str::<pyo3_bindgen::SchemaIr>(&case.raw),
                |schema| serde_json::from_value::<pyo3_bindgen::SchemaIr>(schema.clone()),
            );
            assert_eq!(
                schema_dto_result.is_ok(),
                case.schema_dto_accepts,
                "inner SchemaIr DTO result differs for {}",
                case.name
            );

            match (decoded_schema, case.generation_expectation) {
                (Some(schema), GenerationExpectation::Accepted) => {
                    let generation_result = pyo3_bindgen::Codegen::default()
                        .modules_from_ir(schema)
                        .expect("CLI-accepted SchemaIr must register with Codegen")
                        .generate();
                    assert!(
                        generation_result.is_ok(),
                        "CLI-accepted semantically valid SchemaIr must generate for {}: {:?}",
                        case.name,
                        generation_result
                    );
                }
                (Some(schema), GenerationExpectation::TypedCodegenError) => {
                    let error = pyo3_bindgen::Codegen::default()
                        .modules_from_ir(schema)
                        .expect("CLI-accepted SchemaIr must register with Codegen")
                        .generate()
                        .expect_err("the semantic-invalid case must fail at terminal generation");
                    assert!(
                        matches!(
                            &error,
                            pyo3_bindgen::PyBindgenError::CodegenError(message)
                                if message.contains("does not resolve to a class in external IR")
                        ),
                        "semantic-invalid case must preserve the typed CodegenError: {error:?}"
                    );
                }
                (None, GenerationExpectation::NotReached) => {}
                (Some(_), GenerationExpectation::NotReached)
                | (
                    None,
                    GenerationExpectation::Accepted | GenerationExpectation::TypedCodegenError,
                ) => {
                    panic!("{} has an inconsistent generation expectation", case.name)
                }
            }

            if let Some(version_lexeme) = case.version_lexeme {
                let raw_object: BTreeMap<String, Box<serde_json::value::RawValue>> =
                    serde_json::from_str(&case.raw)
                        .unwrap_or_else(|error| panic!("{} is not an object: {error}", case.name));
                assert_eq!(
                    raw_object
                        .get("schema_version")
                        .expect("version case has a schema_version")
                        .get(),
                    version_lexeme,
                    "CLI corpus must retain the source version lexeme for {}",
                    case.name
                );
            }
        }
    }

    #[test]
    fn external_ir_snapshot_accepts_input_at_exact_limit() {
        let snapshot = read_external_ir_snapshot(Cursor::new(VALID_IR), VALID_IR.len())
            .expect("input at the configured limit should succeed");

        assert_eq!(snapshot.raw_bytes, VALID_IR);
        assert_eq!(snapshot.byte_limit, VALID_IR.len());
    }

    #[test]
    fn external_ir_snapshot_rejects_input_over_limit_with_typed_error() {
        let limit = VALID_IR.len();
        let oversized = [VALID_IR, b" "].concat();

        let error = read_external_ir_snapshot(Cursor::new(oversized), limit)
            .expect_err("input over the configured limit should fail");

        assert!(matches!(
            error,
            ExternalIrInputError::TooLarge { limit: error_limit } if error_limit == limit
        ));
    }

    #[test]
    fn external_ir_snapshot_hash_is_exact_lowercase_sha256() {
        let snapshot = read_external_ir_snapshot(Cursor::new(VALID_IR), VALID_IR.len()).unwrap();

        assert_eq!(
            snapshot.sha256,
            "sha256:4489b472b977b07cbc2d63195abc8d4842e592bdeeac2aadb2ddf59805a19ce0"
        );
    }

    #[test]
    fn external_ir_snapshot_parses_and_hashes_the_retained_owned_bytes() {
        let raw_bytes = VALID_IR.to_vec();
        let snapshot =
            read_external_ir_snapshot(Cursor::new(raw_bytes.clone()), raw_bytes.len()).unwrap();

        assert_eq!(snapshot.raw_bytes, raw_bytes);
        assert_eq!(
            snapshot.schema,
            serde_json::from_slice(&snapshot.raw_bytes).unwrap()
        );
        assert_eq!(snapshot.sha256.len(), "sha256:".len() + 64);
        assert!(snapshot.sha256[7..]
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit()));
        assert_eq!(
            snapshot.sha256,
            "sha256:4489b472b977b07cbc2d63195abc8d4842e592bdeeac2aadb2ddf59805a19ce0"
        );
    }

    #[cfg(unix)]
    #[test]
    fn same_inode_rewrites_during_and_after_read_cannot_change_captured_bytes() {
        use std::{
            fs::{self, File, OpenOptions},
            io::Write,
            os::unix::fs::MetadataExt,
            path::PathBuf,
        };

        struct RewriteDuringRead {
            file: File,
            path: PathBuf,
            replacement: &'static [u8],
            first_chunk: usize,
            rewritten: bool,
        }

        impl Read for RewriteDuringRead {
            fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
                if !self.rewritten {
                    let count = self.first_chunk.min(buffer.len());
                    let read = self.file.read(&mut buffer[..count])?;
                    let mut replacement = OpenOptions::new()
                        .write(true)
                        .truncate(true)
                        .open(&self.path)?;
                    replacement.write_all(self.replacement)?;
                    replacement.sync_all()?;
                    self.rewritten = true;
                    return Ok(read);
                }
                self.file.read(buffer)
            }
        }

        let path = std::env::temp_dir().join(format!(
            "pyo3_bindgen_same_inode_snapshot_{}_{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("unnamed")
        ));
        let original = b"{\"modules\":[],\"padding\":\"old\"}\n";
        let during_rewrite = b"{\"modules\":[],\"padding\":\"new\"}\n";
        let after_rewrite = b"{\"modules\":[],\"padding\":\"end\"}\n";
        fs::write(&path, original).unwrap();
        let inode = fs::metadata(&path).unwrap().ino();
        let first_chunk = original
            .iter()
            .position(|byte| *byte == b'o')
            .expect("fixture contains the old value");

        let snapshot = read_external_ir_snapshot(
            RewriteDuringRead {
                file: File::open(&path).unwrap(),
                path: path.clone(),
                replacement: during_rewrite,
                first_chunk,
                rewritten: false,
            },
            original.len(),
        )
        .unwrap();

        assert_eq!(fs::metadata(&path).unwrap().ino(), inode);
        assert_eq!(snapshot.raw_bytes, during_rewrite);
        assert_eq!(
            snapshot.sha256,
            "sha256:795a690c6b0321f6c0bb75e4b23326eb248eab51110b128477792e4bed6bb971"
        );

        fs::write(&path, after_rewrite).unwrap();
        assert_eq!(fs::metadata(&path).unwrap().ino(), inode);
        assert_eq!(snapshot.raw_bytes, during_rewrite);
        assert_eq!(snapshot.schema.modules.len(), 0);
        let _ = fs::remove_file(path);
    }
}
