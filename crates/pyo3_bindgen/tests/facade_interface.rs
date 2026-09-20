use std::{any::TypeId, path::Path};

use pyo3_bindgen::{Codegen, FunctionIr, FunctionKindIr, ModuleIr, ParameterIr, SchemaIr};

#[path = "test_support/bounded_consumer.rs"]
mod bounded_consumer;

macro_rules! assert_engine_reexport {
    ($facade:ty, $engine:ty) => {
        assert_eq!(
            TypeId::of::<$facade>(),
            TypeId::of::<$engine>(),
            "facade reexport changed identity: {}",
            stringify!($facade),
        );
    };
}

#[test]
fn facade_reexports_preserve_public_type_identities() {
    assert_engine_reexport!(
        pyo3_bindgen::ArtifactDurability,
        pyo3_bindgen_engine::ArtifactDurability
    );
    assert_engine_reexport!(
        pyo3_bindgen::ArtifactStatus,
        pyo3_bindgen_engine::ArtifactStatus
    );
    assert_engine_reexport!(
        pyo3_bindgen::ArtifactWriteFailure,
        pyo3_bindgen_engine::ArtifactWriteFailure
    );
    assert_engine_reexport!(
        pyo3_bindgen::ArtifactWriteReceipt,
        pyo3_bindgen_engine::ArtifactWriteReceipt
    );
    assert_engine_reexport!(pyo3_bindgen::ClassIr, pyo3_bindgen_engine::ClassIr);
    assert_engine_reexport!(pyo3_bindgen::Codegen, pyo3_bindgen_engine::Codegen);
    assert_engine_reexport!(pyo3_bindgen::Config, pyo3_bindgen_engine::Config);
    assert_engine_reexport!(
        pyo3_bindgen::ExternalIrSourceContext,
        pyo3_bindgen_engine::ExternalIrSourceContext
    );
    assert_engine_reexport!(
        pyo3_bindgen::ArtifactPaths,
        pyo3_bindgen_engine::ArtifactPaths
    );
    assert_engine_reexport!(
        pyo3_bindgen::DiagnosticSeverity,
        pyo3_bindgen_engine::DiagnosticSeverity
    );
    assert_engine_reexport!(pyo3_bindgen::EnumAliasIr, pyo3_bindgen_engine::EnumAliasIr);
    assert_engine_reexport!(
        pyo3_bindgen::EnumAliasReport,
        pyo3_bindgen_engine::EnumAliasReport
    );
    assert_engine_reexport!(pyo3_bindgen::EnumIr, pyo3_bindgen_engine::EnumIr);
    assert_engine_reexport!(pyo3_bindgen::EnumKindIr, pyo3_bindgen_engine::EnumKindIr);
    assert_engine_reexport!(pyo3_bindgen::EnumReport, pyo3_bindgen_engine::EnumReport);
    assert_engine_reexport!(
        pyo3_bindgen::EnumVariantIr,
        pyo3_bindgen_engine::EnumVariantIr
    );
    assert_engine_reexport!(
        pyo3_bindgen::EnumVariantPlan,
        pyo3_bindgen_engine::EnumVariantPlan
    );
    assert_engine_reexport!(
        pyo3_bindgen::EnumVariantReport,
        pyo3_bindgen_engine::EnumVariantReport
    );
    assert_engine_reexport!(pyo3_bindgen::FunctionIr, pyo3_bindgen_engine::FunctionIr);
    assert_engine_reexport!(
        pyo3_bindgen::FunctionKindIr,
        pyo3_bindgen_engine::FunctionKindIr
    );
    assert_engine_reexport!(
        pyo3_bindgen::GeneratedBindings,
        pyo3_bindgen_engine::GeneratedBindings
    );
    assert_engine_reexport!(
        pyo3_bindgen::GeneratedArtifactFailure,
        pyo3_bindgen_engine::GeneratedArtifactFailure
    );
    assert_engine_reexport!(
        pyo3_bindgen::GeneratedCounts,
        pyo3_bindgen_engine::GeneratedCounts
    );
    assert_engine_reexport!(
        pyo3_bindgen::GenerationDiagnostic,
        pyo3_bindgen_engine::GenerationDiagnostic
    );
    assert_engine_reexport!(
        pyo3_bindgen::GenerationReport,
        pyo3_bindgen_engine::GenerationReport
    );
    assert_engine_reexport!(
        pyo3_bindgen::GenerationReportDocument,
        pyo3_bindgen_engine::GenerationReportDocument
    );
    assert_engine_reexport!(
        pyo3_bindgen::GenerationWork,
        pyo3_bindgen_engine::GenerationWork
    );
    assert_engine_reexport!(
        pyo3_bindgen::GeneratorMetadata,
        pyo3_bindgen_engine::GeneratorMetadata
    );
    assert_engine_reexport!(pyo3_bindgen::ImportIr, pyo3_bindgen_engine::ImportIr);
    assert_engine_reexport!(pyo3_bindgen::LiveModules, pyo3_bindgen_engine::LiveModules);
    assert_engine_reexport!(
        pyo3_bindgen::MethodKindIr,
        pyo3_bindgen_engine::MethodKindIr
    );
    assert_engine_reexport!(pyo3_bindgen::ModuleIr, pyo3_bindgen_engine::ModuleIr);
    assert_engine_reexport!(
        pyo3_bindgen::ModuleReport,
        pyo3_bindgen_engine::ModuleReport
    );
    assert_engine_reexport!(
        pyo3_bindgen::OutputMetadata,
        pyo3_bindgen_engine::OutputMetadata
    );
    assert_engine_reexport!(pyo3_bindgen::ParameterIr, pyo3_bindgen_engine::ParameterIr);
    assert_engine_reexport!(
        pyo3_bindgen::ParameterKindIr,
        pyo3_bindgen_engine::ParameterKindIr
    );
    assert_engine_reexport!(pyo3_bindgen::PropertyIr, pyo3_bindgen_engine::PropertyIr);
    assert_engine_reexport!(
        pyo3_bindgen::PublicationPlan,
        pyo3_bindgen_engine::PublicationPlan
    );
    assert_engine_reexport!(
        pyo3_bindgen::PublicationPolicy,
        pyo3_bindgen_engine::PublicationPolicy
    );
    assert_engine_reexport!(
        pyo3_bindgen::PublicationSession,
        pyo3_bindgen_engine::PublicationSession
    );
    assert_engine_reexport!(
        pyo3_bindgen::PyBindgenError,
        pyo3_bindgen_engine::PyBindgenError
    );
    assert_engine_reexport!(
        pyo3_bindgen::PyBindgenResult<()>,
        pyo3_bindgen_engine::PyBindgenResult<()>
    );
    assert_engine_reexport!(
        pyo3_bindgen::PythonMetadata,
        pyo3_bindgen_engine::PythonMetadata
    );
    assert_engine_reexport!(
        pyo3_bindgen::RustArtifact,
        pyo3_bindgen_engine::RustArtifact
    );
    assert_engine_reexport!(pyo3_bindgen::SchemaIr, pyo3_bindgen_engine::SchemaIr);
    assert_engine_reexport!(pyo3_bindgen::RunOutcome, pyo3_bindgen_engine::RunOutcome);
    assert_engine_reexport!(
        pyo3_bindgen::SchemaSource,
        pyo3_bindgen_engine::SchemaSource
    );
    assert_engine_reexport!(
        pyo3_bindgen::TextArtifact,
        pyo3_bindgen_engine::TextArtifact
    );
    assert_engine_reexport!(pyo3_bindgen::pyo3::PyErr, pyo3::PyErr);

    let facade_hash = pyo3_bindgen::stable_hash("facade");
    let engine_hash = pyo3_bindgen_engine::stable_hash("facade");
    assert_eq!(facade_hash, engine_hash);
    assert_eq!(
        pyo3_bindgen::enum_variant_ident("café"),
        Some("Café".to_owned())
    );
    assert_eq!(
        pyo3_bindgen::plan_enum_variants(&[
            "AL_PHA".to_owned(),
            "al_pha".to_owned(),
            "!!!".to_owned(),
            "self".to_owned(),
        ]),
        vec![
            pyo3_bindgen::EnumVariantPlan {
                python_name: "AL_PHA".to_owned(),
                rust_name: "AlPha".to_owned(),
            },
            pyo3_bindgen::EnumVariantPlan {
                python_name: "self".to_owned(),
                rust_name: "VSelf".to_owned(),
            },
        ]
    );

    fn accepts_facade_source<T: pyo3_bindgen::BindingSource>(
        source: T,
    ) -> pyo3_bindgen_engine::PyBindgenResult<pyo3_bindgen_engine::SchemaSource> {
        pyo3_bindgen::BindingSource::load(source)
    }

    let source = accepts_facade_source(pyo3_bindgen::SchemaIr { modules: vec![] })
        .expect("SchemaIr should implement the facade BindingSource identity");
    assert!(matches!(source, pyo3_bindgen_engine::SchemaSource::Ir(_)));
}

#[test]
fn facade_artifact_values_keep_construction_and_access_without_delivery() {
    let text = pyo3_bindgen::TextArtifact::new("héllo");
    assert_eq!(text.contents(), "héllo");
    assert_eq!(text.bytes(), "héllo".as_bytes());
    assert_eq!(text.len(), 6);
    assert!(!text.is_empty());
    assert_eq!(AsRef::<str>::as_ref(&text), text.contents());
    assert_eq!(text.hash(), pyo3_bindgen::stable_hash(text.bytes()));

    let rust = pyo3_bindgen::RustArtifact::from_tokens("pub fn answer()->i32{42}".parse().unwrap())
        .unwrap();
    assert_eq!(rust.contents(), "pub fn answer() -> i32 {\n    42\n}\n");
    assert_eq!(rust.bytes(), rust.contents().as_bytes());
    assert_eq!(rust.len(), rust.contents().len());
    assert!(!rust.is_empty());
    assert_eq!(AsRef::<str>::as_ref(&rust), rust.contents());
    assert_eq!(rust.hash(), pyo3_bindgen::stable_hash(rust.bytes()));
    assert!(pyo3_bindgen::TextArtifact::new("").is_empty());
    assert!(pyo3_bindgen::RustArtifact::from_tokens("".parse().unwrap())
        .unwrap()
        .is_empty());
}

#[test]
fn facade_generates_external_ir_for_a_direct_consumer() {
    let generated = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![ModuleIr {
                name: "facade_smoke".to_owned(),
                docstring: None,
                is_package: false,
                classes: vec![],
                enums: vec![],
                functions: vec![FunctionIr {
                    name: "facade_smoke.answer".to_owned(),
                    kind: FunctionKindIr::Function,
                    params: vec![ParameterIr {
                        name: "value".to_owned(),
                        kind: pyo3_bindgen::ParameterKindIr::PositionalOrKeyword,
                        annotation: "int".to_owned(),
                        has_default: false,
                    }],
                    return_type: "int".to_owned(),
                    docstring: None,
                }],
                properties: vec![],
                submodules: vec![],
                prelude: vec![],
                imports: vec![],
                type_vars: vec![],
            }],
        })
        .expect("facade should accept external IR")
        .generate()
        .expect("facade should generate external IR bindings")
        .to_string();

    assert!(generated.contains("facade_smoke"));
    assert!(generated.contains("answer"));

    let Some(root) =
        bounded_consumer::workspace_root("facade_generates_external_ir_for_a_direct_consumer")
    else {
        return;
    };
    let mut consumer = bounded_consumer::ConsumerWorkspace::new("facade_external_ir");
    std::fs::write(
        consumer.path().join("Cargo.toml"),
        consumer_manifest(&root, "facade_external_ir_consumer"),
    )
    .expect("consumer manifest should be writable");
    std::fs::write(
        consumer.path().join("src/lib.rs"),
        format!(
            "{generated}\n\
             pub fn probe(py: ::pyo3::Python<'_>) -> ::pyo3::PyResult<i64> {{\n\
                 facade_smoke::answer(py, 7)\n\
             }}\n"
        ),
    )
    .expect("consumer source should be writable");

    let output = bounded_consumer::cargo_check(&mut consumer).unwrap_or_else(|error| {
        panic!(
            "facade consumer cargo check did not finish cleanly: {error}\nstdout:\n{}\nstderr:\n{}",
            error.stdout(),
            error.stderr()
        )
    });
    assert!(
        output.status.success(),
        "generated facade consumer should compile\nstatus: {}\nstdout:\n{}\nstderr:\n{}\n\n--- generated source ---\n{generated}",
        output.status,
        output.stdout,
        output.stderr,
    );
}

fn consumer_manifest(root: &Path, name: &str) -> String {
    let facade_path = root.join("crates/pyo3_bindgen");
    format!(
        "[package]\nname = \"{name}\"\nversion = \"0.0.0\"\nedition = \"2021\"\npublish = false\n\n[workspace]\n\n[dependencies]\npyo3 = {{ version = \"=0.29.2\", default-features = false, features = [\"py-clone\"] }}\npyo3_bindgen = {{ path = {:?} }}\n",
        facade_path.to_string_lossy().to_string(),
    )
}

#[test]
fn bounded_consumer_timeout_is_proven_by_exact_child_marker() {
    bounded_consumer::assert_timeout_regression(
        "bounded_consumer_timeout_is_proven_by_exact_child_marker",
    );
}
