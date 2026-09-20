use assert_cmd::cargo::cargo_bin_cmd;
use std::{
    fs,
    time::{SystemTime, UNIX_EPOCH},
};

fn temp_dir() -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "pyo3_bindgen_cli_annotation_stability_{}_{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&path).unwrap();
    path
}

fn expected_file_publication_exit_code() -> i32 {
    if cfg!(unix) {
        0
    } else {
        3
    }
}

#[test]
fn opaque_annotations_are_reproducible_across_processes() {
    let temp_dir = temp_dir();
    let python_dir = temp_dir.join("python");
    fs::create_dir_all(&python_dir).unwrap();
    fs::write(
        python_dir.join("opaque_annotations.py"),
        r#"
_opaque = object()

def transform(value: _opaque) -> _opaque:
    return value
"#,
    )
    .unwrap();

    let generate = |suffix: &str| {
        let output = temp_dir.join(format!("bindings-{suffix}.rs"));
        let report = temp_dir.join(format!("report-{suffix}.json"));
        let mut cmd = cargo_bin_cmd!("pyo3_bindgen");
        cmd.env("PYTHONPATH", &python_dir)
            .args(["-m", "opaque_annotations", "--output"])
            .arg(&output)
            .args(["--report"])
            .arg(&report)
            .assert()
            .code(expected_file_publication_exit_code());

        let report_json: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(report).unwrap()).unwrap();
        let module = report_json["modules"]
            .as_array()
            .unwrap()
            .iter()
            .find(|module| module["name"] == "opaque_annotations")
            .unwrap();
        (
            fs::read(output).unwrap(),
            module["input_hash"].as_str().unwrap().to_owned(),
        )
    };

    let first = generate("first");
    let second = generate("second");

    assert_eq!(first.0, second.0);
    assert_eq!(first.1, second.1);
    assert!(!String::from_utf8(first.0).unwrap().contains("object at 0x"));
}
