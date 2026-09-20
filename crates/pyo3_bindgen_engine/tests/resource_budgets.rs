use pyo3_bindgen_engine::{
    ClassIr, Codegen, FunctionIr, FunctionKindIr, GenerationBudget, ModuleIr, PyBindgenError,
    ResourceKind, ResourceLimitExceeded, ResourcePhase, SchemaIr,
};
use std::{path::PathBuf, time::Duration};

fn schema() -> SchemaIr {
    SchemaIr {
        modules: vec![ModuleIr {
            name: "budgeted".to_owned(),
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

fn limit_error(budget: GenerationBudget) -> ResourceLimitExceeded {
    let error = Codegen::default()
        .with_budget(budget)
        .modules_from_ir(schema())
        .expect("registration remains structural")
        .generate()
        .expect_err("budget must fail");
    let PyBindgenError::ResourceLimitExceeded(error) = error else {
        panic!("expected typed resource limit, got {error}");
    };
    error
}

fn temp_output(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "pyo3_bindgen_resource_{name}_{}_{}.rs",
        std::process::id(),
        std::thread::current().name().unwrap_or("unnamed")
    ))
}

#[test]
fn structural_limits_fail_before_emission_with_typed_evidence() {
    for (budget, kind) in [
        (
            GenerationBudget {
                max_definitions: 0,
                ..GenerationBudget::default()
            },
            ResourceKind::Definitions,
        ),
        (
            GenerationBudget {
                max_recursion_depth: 1,
                ..GenerationBudget::default()
            },
            ResourceKind::RecursionDepth,
        ),
        (
            GenerationBudget {
                max_model_nodes: 1,
                ..GenerationBudget::default()
            },
            ResourceKind::ModelNodes,
        ),
    ] {
        let error = limit_error(budget);
        assert_eq!(error.kind, kind);
        assert_eq!(error.phase, ResourcePhase::Validation);
        assert!(error.observed > error.limit);
    }
}

#[test]
fn output_limit_is_typed_and_build_publishes_nothing() {
    let output = temp_output("no_publish");
    let _ = std::fs::remove_file(&output);
    let error = Codegen::default()
        .with_budget(GenerationBudget {
            max_output_bytes: 1,
            ..GenerationBudget::default()
        })
        .modules_from_ir(schema())
        .unwrap()
        .build(&output)
        .expect_err("canonical output must exceed one byte");

    let PyBindgenError::ResourceLimitExceeded(error) = error else {
        panic!("expected typed resource limit");
    };
    assert_eq!(error.kind, ResourceKind::OutputBytes);
    assert_eq!(error.phase, ResourcePhase::Emission);
    assert!(!output.exists(), "failed generation must publish no output");
}

#[test]
fn zero_wall_budget_fails_cooperatively() {
    let error = limit_error(GenerationBudget {
        max_wall_time: Duration::ZERO,
        ..GenerationBudget::default()
    });
    assert_eq!(error.kind, ResourceKind::WallTime);
    assert!(error.observed > 0);
    assert_eq!(error.limit, 0);
}

#[test]
fn deep_inheritance_is_measured_iteratively_before_validation() {
    let mut input = schema();
    input.modules[0].classes = (0..1_000)
        .map(|index| ClassIr {
            name: format!("budgeted.Class{index}"),
            base_classes: (index > 0)
                .then(|| format!("budgeted.Class{}", index - 1))
                .into_iter()
                .collect(),
            docstring: None,
            methods: vec![],
            properties: vec![],
        })
        .collect();

    let error = Codegen::default()
        .modules_from_ir(input)
        .unwrap()
        .generate()
        .expect_err("the default depth budget must reject a deep flat hierarchy");
    let PyBindgenError::ResourceLimitExceeded(error) = error else {
        panic!("expected typed depth evidence");
    };
    assert_eq!(error.kind, ResourceKind::RecursionDepth);
    assert_eq!(error.phase, ResourcePhase::Validation);
    assert_eq!(error.observed, 1_000);
    assert_eq!(error.limit, 32);
}

#[test]
fn deeply_nested_annotation_is_bounded_before_recursive_parsing() {
    let mut input = schema();
    input.modules[0].functions.push(FunctionIr {
        name: "budgeted.deep_type".to_owned(),
        kind: FunctionKindIr::Function,
        params: vec![],
        return_type: format!("{}int{}", "list[".repeat(1_000), "]".repeat(1_000)),
        docstring: None,
    });

    let error = Codegen::default()
        .modules_from_ir(input)
        .unwrap()
        .generate()
        .expect_err("the structural budget must run before recursive type parsing");
    let PyBindgenError::ResourceLimitExceeded(error) = error else {
        panic!("expected typed depth evidence");
    };
    assert_eq!(error.kind, ResourceKind::RecursionDepth);
    assert_eq!(error.phase, ResourcePhase::Validation);
    assert!(error.observed >= 1_000);
    assert_eq!(error.limit, 32);
}

#[test]
fn deeply_dotted_module_name_is_bounded_before_recursive_canonicalization() {
    let mut input = schema();
    input.modules[0].name = (0..64)
        .map(|index| format!("segment_{index}"))
        .collect::<Vec<_>>()
        .join(".");

    let error = Codegen::default()
        .with_budget(GenerationBudget {
            max_recursion_depth: 8,
            ..GenerationBudget::default()
        })
        .modules_from_ir(input)
        .unwrap()
        .generate()
        .expect_err("the dotted path budget must run before recursive canonicalization");
    let PyBindgenError::ResourceLimitExceeded(error) = error else {
        panic!("expected typed depth evidence");
    };
    assert_eq!(error.kind, ResourceKind::RecursionDepth);
    assert_eq!(error.phase, ResourcePhase::Validation);
    assert!(error.observed >= 64);
    assert_eq!(error.limit, 8);
}

#[test]
fn combined_dotted_module_and_annotation_depth_is_bounded_before_lowering() {
    let mut input = schema();
    let module_name = (0..6)
        .map(|index| format!("segment_{index}"))
        .collect::<Vec<_>>()
        .join(".");
    input.modules[0].name.clone_from(&module_name);
    input.modules[0].functions.push(FunctionIr {
        name: format!("{module_name}.deep_type"),
        kind: FunctionKindIr::Function,
        params: vec![],
        return_type: "list[list[int]]".to_owned(),
        docstring: None,
    });

    let error = Codegen::default()
        .with_budget(GenerationBudget {
            max_recursion_depth: 8,
            ..GenerationBudget::default()
        })
        .modules_from_ir(input)
        .unwrap()
        .generate()
        .expect_err("combined dotted and annotation depth must fail before lowering");
    let PyBindgenError::ResourceLimitExceeded(error) = error else {
        panic!("expected typed depth evidence");
    };
    assert_eq!(error.kind, ResourceKind::RecursionDepth);
    assert_eq!(error.phase, ResourcePhase::Validation);
    assert_eq!(error.observed, 9);
    assert_eq!(error.limit, 8);
}

#[test]
fn adaptive_output_budget_may_discard_only_the_oversized_candidate() {
    let methods = (0..20)
        .map(|index| {
            format!(
                "    def method_{index}(self, value: int, text: str) -> int:\n\
                 \x20       \"\"\"A deliberately substantial inherited method.\"\"\"\n\
                 \x20       return value\n"
            )
        })
        .collect::<String>();
    let source = format!(
        "class Base:\n{methods}\n\
         class Derived(Base):\n\
         \x20   pass\n"
    );

    let error = Codegen::default()
        .with_budget(GenerationBudget {
            max_output_bytes: 1,
            ..GenerationBudget::default()
        })
        .module_from_str(&source, "adaptive_budget")
        .unwrap()
        .generate()
        .expect_err("two oversized adaptive candidates must fail");
    let PyBindgenError::ResourceLimitExceeded(error) = error else {
        panic!("expected typed output limit when both candidates exceed it");
    };
    assert_eq!(error.kind, ResourceKind::OutputBytes);
    assert_eq!(error.phase, ResourcePhase::Emission);

    let selected = Codegen::default()
        .module_from_str(&source, "adaptive_budget")
        .unwrap()
        .generate()
        .unwrap();
    let selected_bytes = pyo3_bindgen_engine::RustArtifact::from_tokens(selected)
        .unwrap()
        .len();

    let generated = Codegen::default()
        .with_budget(GenerationBudget {
            max_output_bytes: selected_bytes,
            ..GenerationBudget::default()
        })
        .module_from_str(&source, "adaptive_budget")
        .unwrap()
        .generate()
        .expect("the exact selected candidate fits even when its fallback does not");
    assert_eq!(
        pyo3_bindgen_engine::RustArtifact::from_tokens(generated)
            .unwrap()
            .len(),
        selected_bytes
    );
}

#[cfg(any(
    target_os = "android",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "linux",
    target_os = "macos",
    target_os = "netbsd",
    target_os = "openbsd",
    windows,
))]
#[test]
fn finite_peak_rss_budget_fails_end_to_end_with_typed_error() {
    const CHILD_ENV: &str = "PYO3_BINDGEN_RSS_BUDGET_CHILD";
    const CHILD_ROLE: &str = "pyo3-bindgen-rss-budget-child-v1";
    const TEST_NAME: &str = "finite_peak_rss_budget_fails_end_to_end_with_typed_error";
    const CHILD_SUCCESS_MARKER: &str = "PYO3_BINDGEN_RSS_BUDGET_CHILD_SUCCESS";

    if std::env::var(CHILD_ENV).as_deref() == Ok(CHILD_ROLE) {
        let mut input = schema();
        input.modules[0].functions = (0..5_000)
            .map(|index| FunctionIr {
                name: format!("budgeted.function_{index}"),
                kind: FunctionKindIr::Function,
                params: vec![],
                return_type: "int".to_owned(),
                docstring: Some("RSS budget workload".repeat(8)),
            })
            .collect();
        let error = Codegen::default()
            .with_budget(GenerationBudget {
                max_peak_rss_delta_bytes: Some(0),
                ..GenerationBudget::default()
            })
            .modules_from_ir(input)
            .unwrap()
            .generate()
            .expect_err("a fresh child must grow peak RSS while generating a wide graph");
        let PyBindgenError::ResourceLimitExceeded(error) = error else {
            panic!("expected typed RSS limit, got {error}");
        };
        assert_eq!(error.kind, ResourceKind::PeakRssDeltaBytes);
        assert!(error.observed > 0);
        assert_eq!(error.limit, 0);
        println!("{CHILD_SUCCESS_MARKER}");
        return;
    }

    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", TEST_NAME, "--nocapture"])
        .env(CHILD_ENV, CHILD_ROLE)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let success_markers = stdout
        .split_whitespace()
        .filter(|token| *token == CHILD_SUCCESS_MARKER)
        .count();
    assert!(
        output.status.success() && success_markers == 1,
        "RSS-budget child failed\nstdout:\n{}\nstderr:\n{}",
        stdout,
        String::from_utf8_lossy(&output.stderr),
    );
}
