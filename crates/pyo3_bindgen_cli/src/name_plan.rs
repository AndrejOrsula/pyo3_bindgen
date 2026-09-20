use serde::{Deserialize, Serialize};
use std::io::{Read, Write};

const PROTOCOL_VERSION: u32 = 1;
const MAX_REQUEST_BYTES: u64 = 8 * 1024 * 1024;
const MAX_NAMES: usize = 100_000;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NamePlanRequest {
    protocol_version: u32,
    python_names: Vec<String>,
}

#[derive(Serialize)]
struct NamePlanResponse {
    protocol_version: u32,
    variants: Vec<pyo3_bindgen::EnumVariantPlan>,
}

/// Execute the versioned batch enum-name planning protocol over stdin/stdout.
#[doc(hidden)]
pub fn run_name_plan() -> Result<(), String> {
    let mut bytes = Vec::new();
    std::io::stdin()
        .take(MAX_REQUEST_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("failed to read name-plan request: {error}"))?;
    if bytes.len() as u64 > MAX_REQUEST_BYTES {
        return Err(format!(
            "name-plan request exceeds {MAX_REQUEST_BYTES} bytes"
        ));
    }
    let request: NamePlanRequest = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid name-plan JSON request: {error}"))?;
    if request.protocol_version != PROTOCOL_VERSION {
        return Err(format!(
            "unsupported name-plan protocol version {}; expected {PROTOCOL_VERSION}",
            request.protocol_version
        ));
    }
    if request.python_names.len() > MAX_NAMES {
        return Err(format!("name-plan request exceeds {MAX_NAMES} names"));
    }

    let response = NamePlanResponse {
        protocol_version: PROTOCOL_VERSION,
        variants: pyo3_bindgen::plan_enum_variants(&request.python_names),
    };
    serde_json::to_writer(std::io::stdout().lock(), &response)
        .map_err(|error| format!("failed to write name-plan response: {error}"))?;
    std::io::stdout()
        .write_all(b"\n")
        .map_err(|error| format!("failed to finish name-plan response: {error}"))
}
