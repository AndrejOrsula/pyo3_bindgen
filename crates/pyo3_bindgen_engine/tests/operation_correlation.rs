use pyo3_bindgen_engine::{
    Codegen, Config, GenerationBudget, ModuleIr, PyBindgenError, ResourceKind, SchemaIr,
};

fn schema() -> SchemaIr {
    SchemaIr {
        modules: vec![ModuleIr {
            name: "correlated".to_owned(),
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
    }
}

fn generate(config: Config) -> pyo3_bindgen_engine::GeneratedBindings {
    Codegen::new(config)
        .modules_from_ir(schema())
        .unwrap()
        .generate_with_report()
        .unwrap()
}

#[test]
fn identical_source_and_config_share_one_deterministic_operation_id() {
    let first = generate(Config::default());
    let second = generate(Config::default());
    let first_id = first.report.operation_id.as_deref().unwrap();

    assert_eq!(Some(first_id), second.report.operation_id.as_deref());
    assert!(first_id.starts_with("op-v1:fnv1a64:"));
    assert_eq!(first.tokens.to_string(), second.tokens.to_string());

    let json = serde_json::to_value(&first.report).unwrap();
    assert_eq!(json["operation_id"], first_id);
}

#[test]
fn output_affecting_config_changes_operation_id() {
    let default = generate(Config::default());
    let without_docs = generate(Config {
        generate_docs: false,
        ..Config::default()
    });
    assert_ne!(
        default.report.operation_id,
        without_docs.report.operation_id
    );
}

#[test]
fn empty_pre_identity_report_uses_the_stable_unavailable_identity() {
    let report = pyo3_bindgen_engine::GenerationReport::empty();
    assert_eq!(report.operation_id.as_deref(), Some("unavailable"));
    assert_eq!(
        serde_json::to_value(report).unwrap()["operation_id"],
        "unavailable"
    );
}

#[test]
fn post_normalization_failure_carries_the_operation_id() {
    let failure = Codegen::default()
        .with_budget(GenerationBudget {
            max_output_bytes: 1,
            ..GenerationBudget::default()
        })
        .modules_from_ir(SchemaIr {
            modules: vec![ModuleIr {
                name: "correlated_failure".to_owned(),
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
        .generate_artifact_with_report_detailed()
        .expect_err("one output byte must be insufficient");

    assert!(matches!(
        failure.error(),
        PyBindgenError::ResourceLimitExceeded(error) if error.kind == ResourceKind::OutputBytes
    ));
    let operation_id = failure
        .report()
        .operation_id
        .as_deref()
        .expect("failure report must carry correlation identity");
    assert!(operation_id.starts_with("op-v1:fnv1a64:"));
}
