use assert_cmd::cargo::cargo_bin_cmd;
use predicates::prelude::*;

const MAX_NAMES: usize = 100_000;
const MAX_REQUEST_BYTES: usize = 8 * 1024 * 1024;

fn request_with_names(count: usize, name: &str) -> Vec<u8> {
    let mut request = String::from(r#"{"protocol_version":1,"python_names":["#);
    for index in 0..count {
        if index > 0 {
            request.push(',');
        }
        request.push('"');
        request.push_str(name);
        request.push('"');
    }
    request.push_str("]}");
    request.into_bytes()
}

fn request_with_size(size: usize) -> Vec<u8> {
    const PREFIX: &[u8] = b"{\"protocol_version\":1,\"python_names\":[\"";
    const SUFFIX: &[u8] = b"\"]}";
    assert!(size >= PREFIX.len() + SUFFIX.len());

    let mut request = Vec::with_capacity(size);
    request.extend_from_slice(PREFIX);
    request.resize(size - SUFFIX.len(), b'!');
    request.extend_from_slice(SUFFIX);
    request
}

fn assert_invalid_request(request: Vec<u8>, signal: &str) {
    let mut command = cargo_bin_cmd!("pyo3_bindgen");
    command
        .arg("name-plan")
        .write_stdin(request)
        .assert()
        .code(2)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("ERROR[invalid_arguments]"))
        .stderr(predicate::str::contains(signal));
}

fn assert_exact_invalid_request(request: Vec<u8>, stderr: &str) {
    let mut command = cargo_bin_cmd!("pyo3_bindgen");
    command
        .arg("name-plan")
        .write_stdin(request)
        .assert()
        .code(2)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::eq(stderr));
}

fn assert_empty_variants(request: Vec<u8>) {
    let mut command = cargo_bin_cmd!("pyo3_bindgen");
    command
        .arg("name-plan")
        .write_stdin(request)
        .assert()
        .success()
        .stdout(predicate::eq("{\"protocol_version\":1,\"variants\":[]}\n"))
        .stderr(predicate::str::is_empty());
}

#[test]
fn batch_name_plan_is_ordered_drop_empty_and_first_collision_wins() {
    let mut command = cargo_bin_cmd!("pyo3_bindgen");
    command
        .arg("name-plan")
        .write_stdin(
            r#"{"protocol_version":1,"python_names":["AL_PHA","al_pha","3GAMMA","!!!","self","café"]}"#,
        )
        .assert()
        .success()
        .stdout(predicate::eq(
            "{\"protocol_version\":1,\"variants\":[{\"python_name\":\"AL_PHA\",\"rust_name\":\"AlPha\"},{\"python_name\":\"3GAMMA\",\"rust_name\":\"V3gamma\"},{\"python_name\":\"self\",\"rust_name\":\"VSelf\"},{\"python_name\":\"café\",\"rust_name\":\"Café\"}]}\n",
        ));
}

#[test]
fn name_plan_rejects_invalid_requests_without_partial_output() {
    let mut invalid_utf8 = b"{\"protocol_version\":1,\"python_names\":[\"".to_vec();
    invalid_utf8.push(0xff);
    invalid_utf8.extend_from_slice(b"\"]}");

    let cases = [
        ("empty", Vec::new(), "invalid name-plan JSON request"),
        (
            "malformed",
            br#"{"#.to_vec(),
            "invalid name-plan JSON request",
        ),
        ("invalid UTF-8", invalid_utf8, "invalid unicode code point"),
        (
            "trailing JSON",
            br#"{"protocol_version":1,"python_names":[]} trailing"#.to_vec(),
            "trailing characters",
        ),
        (
            "missing protocol version",
            br#"{"python_names":[]}"#.to_vec(),
            "missing field",
        ),
        (
            "missing names",
            br#"{"protocol_version":1}"#.to_vec(),
            "missing field",
        ),
        (
            "wrong root type",
            b"[]".to_vec(),
            "invalid name-plan JSON request",
        ),
        (
            "wrong names type",
            br#"{"protocol_version":1,"python_names":"x"}"#.to_vec(),
            "invalid type",
        ),
        (
            "wrong element type",
            br#"{"protocol_version":1,"python_names":[1]}"#.to_vec(),
            "invalid type",
        ),
        (
            "unknown field",
            br#"{"protocol_version":1,"python_names":[],"extra":true}"#.to_vec(),
            "unknown field",
        ),
        (
            "protocol version zero",
            br#"{"protocol_version":0,"python_names":[]}"#.to_vec(),
            "unsupported name-plan protocol version 0",
        ),
        (
            "protocol version two",
            br#"{"protocol_version":2,"python_names":[]}"#.to_vec(),
            "unsupported name-plan protocol version 2",
        ),
        (
            "too many names",
            request_with_names(MAX_NAMES + 1, "!"),
            "name-plan request exceeds 100000 names",
        ),
        (
            "request too large",
            request_with_size(MAX_REQUEST_BYTES + 1),
            "name-plan request exceeds 8388608 bytes",
        ),
    ];

    for (_case, request, signal) in cases {
        assert_invalid_request(request, signal);
    }
}

#[test]
fn name_plan_reports_deterministic_diagnostics() {
    assert_exact_invalid_request(
        br#"{"#.to_vec(),
        "ERROR[invalid_arguments]: invalid name-plan JSON request: EOF while parsing an object at line 1 column 1\n",
    );
    assert_exact_invalid_request(
        br#"{"protocol_version":1,"python_names":[],"extra":true}"#.to_vec(),
        "ERROR[invalid_arguments]: invalid name-plan JSON request: unknown field `extra`, expected `protocol_version` or `python_names` at line 1 column 47\n",
    );
    assert_exact_invalid_request(
        br#"{"protocol_version":2,"python_names":[]}"#.to_vec(),
        "ERROR[invalid_arguments]: unsupported name-plan protocol version 2; expected 1\n",
    );
}

#[test]
fn name_plan_accepts_empty_and_limit_boundaries() {
    assert_empty_variants(request_with_names(0, "!"));
    assert_empty_variants(request_with_names(MAX_NAMES, "!"));
    assert_empty_variants(request_with_size(MAX_REQUEST_BYTES));
}

#[test]
fn name_plan_rejects_extra_arguments_without_partial_output() {
    let mut command = cargo_bin_cmd!("pyo3_bindgen");
    command
        .args(["name-plan", "unexpected"])
        .write_stdin(br#"{"protocol_version":1,"python_names":[]}"#)
        .assert()
        .code(2)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::eq(
            "ERROR[invalid_arguments]: name-plan accepts no command-line arguments\n",
        ));
}
