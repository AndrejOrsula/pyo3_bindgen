use pyo3_bindgen_engine::{
    Codegen, GenerationReport, GenerationReportDocument, ModuleIr, SchemaIr,
};
use serde_json::Value;
use std::collections::BTreeSet;

fn keys(value: &serde_json::Value) -> BTreeSet<&str> {
    value
        .as_object()
        .expect("schema object")
        .keys()
        .map(String::as_str)
        .collect()
}

fn checked_report_schema() -> Value {
    serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../schemas/generation_report.v1.json"
    )))
    .expect("checked report schema is JSON")
}

#[test]
fn checked_report_schema_tracks_the_serialized_v1_dto() {
    let schema = checked_report_schema();
    assert_eq!(
        schema["$schema"],
        "https://json-schema.org/draft/2020-12/schema"
    );

    let generated = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![ModuleIr {
                name: "report_schema".to_owned(),
                docstring: None,
                is_package: false,
                classes: vec![],
                enums: vec![],
                functions: vec![],
                properties: vec![],
                submodules: vec![],
                prelude: vec![],
                imports: vec![],
                type_vars: vec![],
            }],
        })
        .unwrap()
        .generate_with_report()
        .unwrap();
    let report = serde_json::to_value(generated.report).unwrap();

    assert_eq!(keys(&report), keys(&schema["properties"]));
    assert_eq!(
        report["schema_version"],
        schema["properties"]["schema_version"]["const"]
    );
    assert_eq!(report["complete"], true);
    let operation_id = report["operation_id"].as_str().unwrap();
    assert!(operation_id.starts_with("op-v1:fnv1a64:"));
    assert_eq!(operation_id.len(), "op-v1:fnv1a64:".len() + 16);

    let required = schema["required"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item.as_str().unwrap())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        required,
        BTreeSet::from([
            "diagnostics",
            "generator",
            "modules",
            "outputs",
            "python",
            "schema_version",
        ])
    );
    assert!(schema["properties"]["operation_id"]["pattern"]
        .as_str()
        .unwrap()
        .contains("unavailable"));
    assert_eq!(
        serde_json::to_value(pyo3_bindgen_engine::GenerationReport::empty()).unwrap()["complete"],
        false
    );
}

#[test]
fn serialized_report_document_extensions_validate_with_draft_2020_12() {
    let mut document = GenerationReportDocument::new(GenerationReport::empty());
    document.extensions.insert(
        "future_field".to_owned(),
        serde_json::json!({"producer": "fixture", "revision": 2}),
    );
    let serialized = serde_json::to_value(document).expect("report document should serialize");
    let validator = jsonschema::draft202012::new(&checked_report_schema())
        .expect("checked report schema should compile independently");

    assert!(
        validator.is_valid(&serialized),
        "serialized report extension must validate: {serialized}"
    );
}

#[test]
fn hand_authored_report_corpus_matches_the_checked_schema() {
    let validator = jsonschema::draft202012::new(&checked_report_schema())
        .expect("checked report schema should compile independently");
    let cases = [
        (
            "valid_root_extension",
            include_str!("fixtures/report_schema_v1/valid_root_extension.json"),
            true,
        ),
        (
            "invalid_nested_generator_extension",
            include_str!("fixtures/report_schema_v1/invalid_nested_generator_extension.json"),
            false,
        ),
        (
            "invalid_nested_module_extension",
            include_str!("fixtures/report_schema_v1/invalid_nested_module_extension.json"),
            false,
        ),
    ];

    for (name, raw, expected_valid) in cases {
        let instance: Value = serde_json::from_str(raw).expect("report fixture is JSON");
        assert_eq!(
            validator.is_valid(&instance),
            expected_valid,
            "report corpus case {name} has an unexpected schema result"
        );
    }
}
