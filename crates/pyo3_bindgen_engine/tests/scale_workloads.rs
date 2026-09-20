//! Reproducible P2 scale/resource matrix.
//!
//! Run with:
//! `cargo test -p pyo3_bindgen_engine --test scale_workloads -- --ignored --exact isolated_scale_resource_matrix --nocapture`

use pyo3::{types::PyAnyMethods, Python};
use pyo3_bindgen_engine::{
    stable_hash, ClassIr, Codegen, FunctionIr, FunctionKindIr, GeneratedArtifact, GenerationReport,
    GenerationWork, ImportIr, ModuleIr, SchemaIr,
};
use std::{
    fmt::{self, Debug, Display},
    process::Command,
    str::FromStr,
    time::{Duration, Instant},
};

#[path = "../test_support/bounded_consumer.rs"]
mod bounded_consumer;

#[path = "../test_support/combined_scale.rs"]
mod combined_scale;

const CHILD_ENV: &str = "PYO3_BINDGEN_SCALE_CHILD_CASE";
const CHILD_ROLE_ENV: &str = "PYO3_BINDGEN_SCALE_CHILD_ROLE";
const CHILD_ROLE: &str = "pyo3-bindgen-scale-child-v1";
const CHILD_WORK_MARKER: &str = "PYO3_BINDGEN_SCALE_CHILD_WORK_STARTED";
const CHILD_SUCCESS_MARKER: &str = "PYO3_BINDGEN_SCALE_CHILD_SUCCESS";
const CHILD_TIMEOUT: Duration = Duration::from_secs(120);
const NUMPY_ORACLE_VERSION: &str = "2.2.4";
const COMBINED_FUNCTIONAL_SCALE: usize = 3;

fn marker_count(stdout: &str, marker: &str) -> usize {
    stdout
        .split_whitespace()
        .filter(|token| *token == marker)
        .count()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScaleCase {
    Wide,
    Deep,
    Dense,
    Incremental,
    Adaptive,
    Numpy,
}

#[derive(Debug, PartialEq, Eq)]
struct ScaleDeterministicEvidence {
    output_bytes: usize,
    hash: String,
    validation_lowering_passes: usize,
    normalization_passes: usize,
    module_tree_clones: usize,
    canonical_renders: usize,
}

#[derive(Debug, PartialEq, Eq)]
struct ScaleEvidence {
    case: ScaleCase,
    elapsed_ms: u128,
    peak_rss_delta_bytes: u64,
    deterministic: ScaleDeterministicEvidence,
}

impl ScaleCase {
    const ALL: [Self; 6] = [
        Self::Wide,
        Self::Deep,
        Self::Dense,
        Self::Incremental,
        Self::Adaptive,
        Self::Numpy,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Wide => "wide",
            Self::Deep => "deep",
            Self::Dense => "dense",
            Self::Incremental => "incremental",
            Self::Adaptive => "adaptive",
            Self::Numpy => "numpy",
        }
    }

    fn codegen(self) -> Codegen {
        match self {
            Self::Wide => Codegen::default()
                .modules_from_ir(wide_schema(1_000))
                .unwrap(),
            Self::Deep => Codegen::default().modules_from_ir(deep_schema(24)).unwrap(),
            Self::Dense => Codegen::default().modules_from_ir(dense_schema(6)).unwrap(),
            Self::Incremental => incremental(1_000),
            Self::Adaptive => Codegen::default()
                .module_from_str(&adaptive_source(40), "scale_adaptive")
                .unwrap(),
            Self::Numpy => {
                let version = installed_numpy_version();
                validate_numpy_oracle_version(&version).unwrap_or_else(|error| panic!("{error}"));
                Codegen::default().module_name("numpy").unwrap()
            }
        }
    }

    fn deterministic_oracle(self) -> ScaleDeterministicEvidence {
        match self {
            Self::Wide => ScaleDeterministicEvidence {
                output_bytes: 419_214,
                hash: "fnv1a64:6e885e896bc3a5dc".to_owned(),
                validation_lowering_passes: 1,
                normalization_passes: 1,
                module_tree_clones: 0,
                canonical_renders: 1,
            },
            Self::Deep => ScaleDeterministicEvidence {
                output_bytes: 247_648,
                hash: "fnv1a64:7e9967fbf5cb3d59".to_owned(),
                validation_lowering_passes: 1,
                normalization_passes: 1,
                module_tree_clones: 0,
                canonical_renders: 1,
            },
            Self::Dense => ScaleDeterministicEvidence {
                output_bytes: 3_674,
                hash: "fnv1a64:78a8e6e5de4fc149".to_owned(),
                validation_lowering_passes: 1,
                normalization_passes: 1,
                module_tree_clones: 0,
                canonical_renders: 1,
            },
            Self::Incremental => ScaleDeterministicEvidence {
                output_bytes: 321_216,
                hash: "fnv1a64:ffa1f40131fa2927".to_owned(),
                validation_lowering_passes: 1,
                normalization_passes: 1,
                module_tree_clones: 0,
                canonical_renders: 1,
            },
            Self::Adaptive => ScaleDeterministicEvidence {
                output_bytes: 69_716,
                hash: "fnv1a64:2075a802804f04bd".to_owned(),
                validation_lowering_passes: 0,
                normalization_passes: 1,
                module_tree_clones: 1,
                canonical_renders: 2,
            },
            Self::Numpy => ScaleDeterministicEvidence {
                output_bytes: 13_114_958,
                hash: "fnv1a64:1287bfa540376e3c".to_owned(),
                validation_lowering_passes: 0,
                normalization_passes: 1,
                module_tree_clones: 1,
                canonical_renders: 2,
            },
        }
    }
}

impl Display for ScaleCase {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label())
    }
}

impl FromStr for ScaleCase {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|case| case.label() == value)
            .ok_or_else(|| format!("unknown scale case: {value}"))
    }
}

fn validate_numpy_oracle_version(version: &str) -> Result<(), String> {
    if version == NUMPY_ORACLE_VERSION {
        Ok(())
    } else {
        Err(format!(
            "NumPy scale oracle requires numpy=={NUMPY_ORACLE_VERSION}, found {version}"
        ))
    }
}

fn installed_numpy_version() -> String {
    Python::initialize();
    Python::attach(|python| {
        python
            .import("numpy")?
            .getattr("__version__")?
            .extract::<String>()
    })
    .expect("NumPy scale oracle requires an importable numpy package")
}

#[test]
fn numpy_oracle_rejects_an_unpinned_dependency_version() {
    assert!(validate_numpy_oracle_version("2.2.4").is_ok());
    assert!(validate_numpy_oracle_version("2.2.5").is_err());
}

fn parse_field<T>(key: &str, value: &str) -> T
where
    T: FromStr,
    T::Err: Debug,
{
    value
        .parse()
        .unwrap_or_else(|error| panic!("invalid scale evidence {key}={value}: {error:?}"))
}

fn next_scale_field<'a, I>(fields: &mut I, key: &str) -> &'a str
where
    I: Iterator<Item = &'a str>,
{
    let field = fields
        .next()
        .unwrap_or_else(|| panic!("scale evidence is missing {key}"));
    let (actual_key, value) = field
        .split_once('=')
        .unwrap_or_else(|| panic!("malformed scale evidence field: {field}"));
    assert_eq!(actual_key, key, "unexpected scale evidence field: {field}");
    value
}

fn parse_scale_case(value: &str) -> ScaleCase {
    value.parse().unwrap_or_else(|error| panic!("{error}"))
}

fn parse_scale_evidence(stdout: &str) -> ScaleEvidence {
    let evidence_lines = stdout
        .lines()
        .filter(|line| line.starts_with("scale case="))
        .collect::<Vec<_>>();
    assert_eq!(
        evidence_lines.len(),
        1,
        "scale child must print exactly one evidence line: {stdout}"
    );
    let line = evidence_lines[0];
    let mut fields = line.split_whitespace();
    assert_eq!(fields.next(), Some("scale"));

    let case = parse_scale_case(next_scale_field(&mut fields, "case"));
    let elapsed_ms = parse_field("elapsed_ms", next_scale_field(&mut fields, "elapsed_ms"));
    let peak_rss_delta_bytes = parse_field(
        "peak_rss_delta_bytes",
        next_scale_field(&mut fields, "peak_rss_delta_bytes"),
    );
    let deterministic = ScaleDeterministicEvidence {
        output_bytes: parse_field(
            "output_bytes",
            next_scale_field(&mut fields, "output_bytes"),
        ),
        hash: next_scale_field(&mut fields, "hash").to_owned(),
        validation_lowering_passes: parse_field(
            "validation_lowering_passes",
            next_scale_field(&mut fields, "validation_lowering_passes"),
        ),
        normalization_passes: parse_field(
            "normalization_passes",
            next_scale_field(&mut fields, "normalization_passes"),
        ),
        module_tree_clones: parse_field(
            "module_tree_clones",
            next_scale_field(&mut fields, "module_tree_clones"),
        ),
        canonical_renders: parse_field(
            "canonical_renders",
            next_scale_field(&mut fields, "canonical_renders"),
        ),
    };

    if let Some(field) = fields.next() {
        panic!("unexpected scale evidence field: {field}");
    }

    ScaleEvidence {
        case,
        elapsed_ms,
        peak_rss_delta_bytes,
        deterministic,
    }
}

fn empty_module(name: impl Into<String>) -> ModuleIr {
    ModuleIr {
        name: name.into(),
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
    }
}

fn wide_schema(width: usize) -> SchemaIr {
    let mut module = empty_module("scale_wide");
    module.functions = (0..width)
        .map(|index| FunctionIr {
            name: format!("scale_wide.function_{index}"),
            kind: FunctionKindIr::Function,
            params: vec![],
            return_type: "int".to_owned(),
            docstring: None,
        })
        .collect();
    SchemaIr {
        modules: vec![module],
    }
}

fn deep_schema(depth: usize) -> SchemaIr {
    let mut module = empty_module("scale_deep");
    module.classes = (0..depth)
        .map(|index| ClassIr {
            name: format!("scale_deep.Class{index}"),
            base_classes: (index > 0)
                .then(|| format!("scale_deep.Class{}", index - 1))
                .into_iter()
                .collect(),
            docstring: None,
            methods: vec![],
            properties: vec![],
        })
        .collect();
    SchemaIr {
        modules: vec![module],
    }
}

fn dense_schema(width: usize) -> SchemaIr {
    SchemaIr {
        modules: (0..width)
            .map(|index| {
                let mut module = empty_module(format!("scale_imports.module_{index}"));
                module.imports = (0..width)
                    .filter(|target| *target != index)
                    .map(|target| ImportIr {
                        origin: format!("scale_imports.module_{target}"),
                        target: format!("scale_imports.module_{index}.alias_{target}"),
                    })
                    .collect();
                module
            })
            .collect(),
    }
}

fn combined_report_projection(report: &GenerationReport) -> serde_json::Value {
    // Artifact generation has no publication path. Keep the deterministic source/generation
    // report fields in the projection while comparing the operation identity separately.
    serde_json::json!({
        "schema_version": report.schema_version,
        "complete": report.complete,
        "generator": &report.generator,
        "python": &report.python,
        "modules": &report.modules,
        "diagnostics": &report.diagnostics,
        "outputs": &report.outputs,
    })
}

fn combined_external_batched(schema: SchemaIr) -> GeneratedArtifact {
    Codegen::default()
        .modules_from_ir(schema)
        .unwrap()
        .generate_artifact_with_report()
        .unwrap()
}

fn combined_external_incremental(schema: SchemaIr) -> GeneratedArtifact {
    let codegen = schema
        .modules
        .into_iter()
        .fold(Codegen::default(), |codegen, module| {
            codegen.module_from_ir(module).unwrap()
        });
    codegen.generate_artifact_with_report().unwrap()
}

fn assert_combined_artifacts_equal(left: &GeneratedArtifact, right: &GeneratedArtifact) {
    assert_eq!(left.artifact.bytes(), right.artifact.bytes());
    assert_eq!(left.artifact.hash(), stable_hash(left.artifact.bytes()));
    assert_eq!(right.artifact.hash(), stable_hash(right.artifact.bytes()));
    assert_eq!(left.artifact.hash(), right.artifact.hash());
    assert_eq!(
        combined_report_projection(&left.report),
        combined_report_projection(&right.report)
    );
    assert_eq!(left.report.operation_id, right.report.operation_id);
    assert_eq!(left.work, right.work);
}

#[test]
fn combined_external_ir_is_deterministic_and_registration_equivalent() {
    let schema = combined_scale::combined_external_schema(COMBINED_FUNCTIONAL_SCALE);
    let first = combined_external_batched(schema.clone());
    let second = combined_external_batched(schema.clone());
    let incremental = combined_external_incremental(schema);

    assert_combined_artifacts_equal(&first, &second);
    assert_combined_artifacts_equal(&first, &incremental);
    assert_eq!(
        first.work,
        GenerationWork {
            validation_lowering_passes: 1,
            normalization_passes: 1,
            module_tree_clones: 0,
            canonical_renders: 1,
        }
    );
    assert!(first.report.modules.iter().any(|module| {
        module.name == "scale_combined.model"
            && module.counts.classes == COMBINED_FUNCTIONAL_SCALE + 3
    }));
    assert!(first
        .report
        .modules
        .iter()
        .any(|module| { module.name == "scale_combined.entry" && module.counts.imports > 0 }));
    // The imported alias must remain a typed dictionary value, not degrade to `PyDict`.
    assert!(first
        .artifact
        .contents()
        .contains("::std::collections::HashMap<::std::string::String, Vec<TypeAlias0>>"));
}

fn run_scale_child(
    command: &mut Command,
    tag: &str,
) -> (
    bounded_consumer::OwnedTempDir,
    bounded_consumer::BoundedCommandOutput,
) {
    let mut working_directory =
        bounded_consumer::OwnedTempDir::allocate(tag).unwrap_or_else(|error| {
            panic!("scale child directory must be allocated exclusively: {error}")
        });
    let output = match working_directory.run(command, CHILD_TIMEOUT) {
        Ok(output) => output,
        Err(error) => panic!(
            "scale child command failed before completion for {tag}: {error}\n\
             working directory: {}\n\
             timeout: {}\n\
             termination_succeeded: {}\n\
             stdout report: {}\n\
             stderr report: {}\n\
             stdout:\n{}\n\
             stderr:\n{}",
            working_directory.path().display(),
            error.is_timeout(),
            error.termination_succeeded(),
            error.stdout_path().display(),
            error.stderr_path().display(),
            error.stdout(),
            error.stderr(),
        ),
    };
    (working_directory, output)
}

#[test]
fn isolated_scale_child_rejects_invalid_invocation_without_work() {
    for (role, case, expected_error) in [
        (None, Some("wide"), CHILD_ROLE_ENV),
        (Some("wrong-role"), Some("wide"), CHILD_ROLE_ENV),
        (Some(CHILD_ROLE), None, CHILD_ENV),
        (
            Some(CHILD_ROLE),
            Some("not-a-scale-case"),
            "unknown scale case",
        ),
    ] {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--ignored",
                "--exact",
                "isolated_scale_child",
                "--nocapture",
            ])
            .env_remove(CHILD_ROLE_ENV)
            .env_remove(CHILD_ENV);
        if let Some(role) = role {
            command.env(CHILD_ROLE_ENV, role);
        }
        if let Some(case) = case {
            command.env(CHILD_ENV, case);
        }
        let (working_directory, output) = run_scale_child(&mut command, "scale-invalid-invocation");
        let stdout = &output.stdout;
        let stderr = &output.stderr;
        assert!(
            !output.status.success(),
            "invalid scale child invocation did not fail cleanly for {expected_error}:\nstatus: {}\nworking directory: {}\nstdout report: {}\nstderr report: {}\nstdout:\n{stdout}\nstderr:\n{stderr}",
            output.status,
            working_directory.path().display(),
            output.stdout_path.display(),
            output.stderr_path.display(),
        );
        assert_eq!(
            marker_count(stdout, CHILD_WORK_MARKER),
            0,
            "invalid scale child invocation reached scale/RSS work for {expected_error}:\nworking directory: {}\nstdout report: {}\nstderr report: {}\nstdout:\n{stdout}\nstderr:\n{stderr}",
            working_directory.path().display(),
            output.stdout_path.display(),
            output.stderr_path.display(),
        );
        assert_eq!(
            marker_count(stdout, CHILD_SUCCESS_MARKER),
            0,
            "invalid scale child invocation reported success for {expected_error}:\nworking directory: {}\nstdout report: {}\nstderr report: {}\nstdout:\n{stdout}\nstderr:\n{stderr}",
            working_directory.path().display(),
            output.stdout_path.display(),
            output.stderr_path.display(),
        );
        assert!(
            !stdout.contains("scale case="),
            "invalid scale child invocation emitted scale evidence for {expected_error}:\nworking directory: {}\nstdout report: {}\nstderr report: {}\nstdout:\n{stdout}\nstderr:\n{stderr}",
            working_directory.path().display(),
            output.stdout_path.display(),
            output.stderr_path.display(),
        );
        assert!(
            stderr.contains(expected_error),
            "invalid scale child invocation did not report {expected_error}:\nworking directory: {}\nstdout report: {}\nstderr report: {}\nstdout:\n{stdout}\nstderr:\n{stderr}",
            working_directory.path().display(),
            output.stdout_path.display(),
            output.stderr_path.display(),
        );
    }
}

fn adaptive_source(methods: usize) -> String {
    let methods = (0..methods)
        .map(|index| {
            format!(
                "    def method_{index}(self, value: int, text: str) -> int:\n\
                 \x20       \"\"\"Substantial inherited method for adaptive sizing.\"\"\"\n\
                 \x20       return value\n"
            )
        })
        .collect::<String>();
    format!("class Base:\n{methods}\nclass Derived(Base):\n    pass\n")
}

fn incremental(count: usize) -> Codegen {
    (0..count).fold(Codegen::default(), |codegen, index| {
        codegen
            .modules_from_ir(SchemaIr {
                modules: vec![empty_module(format!("scale_calls.module_{index}"))],
            })
            .unwrap()
    })
}

#[cfg(unix)]
fn peak_rss_bytes() -> u64 {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
    assert_eq!(
        unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) },
        0
    );
    let raw = unsafe { usage.assume_init() }.ru_maxrss as u64;
    if cfg!(target_os = "macos") {
        raw
    } else {
        raw * 1024
    }
}

#[cfg(windows)]
fn peak_rss_bytes() -> u64 {
    use windows_sys::Win32::System::{
        ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS},
        Threading::GetCurrentProcess,
    };
    let mut counters = PROCESS_MEMORY_COUNTERS {
        cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        ..PROCESS_MEMORY_COUNTERS::default()
    };
    assert_ne!(
        unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.cb) },
        0
    );
    counters.PeakWorkingSetSize as u64
}

#[cfg(not(any(unix, windows)))]
fn peak_rss_bytes() -> u64 {
    0
}

#[test]
#[ignore = "internal child; invoked by isolated_scale_resource_matrix"]
fn isolated_scale_child() {
    let role = std::env::var(CHILD_ROLE_ENV)
        .unwrap_or_else(|_| panic!("internal child requires {CHILD_ROLE_ENV}={CHILD_ROLE}"));
    assert_eq!(
        role, CHILD_ROLE,
        "internal child received an unexpected {CHILD_ROLE_ENV} value"
    );
    let case_name = std::env::var(CHILD_ENV)
        .unwrap_or_else(|_| panic!("internal child requires {CHILD_ENV}=<scale case>"));
    let case = parse_scale_case(&case_name);
    println!("{CHILD_WORK_MARKER}");
    let baseline_rss = peak_rss_bytes();
    let started = Instant::now();
    let generated = case.codegen().generate_artifact_with_report().unwrap();
    let elapsed = started.elapsed();
    let peak_rss_delta = peak_rss_bytes().saturating_sub(baseline_rss);
    let source = generated.artifact.bytes();
    println!(
        "scale case={case} elapsed_ms={} peak_rss_delta_bytes={peak_rss_delta} output_bytes={} hash={} validation_lowering_passes={} normalization_passes={} module_tree_clones={} canonical_renders={}",
        elapsed.as_millis(),
        source.len(),
        stable_hash(source),
        generated.work.validation_lowering_passes,
        generated.work.normalization_passes,
        generated.work.module_tree_clones,
        generated.work.canonical_renders,
    );
    println!("{CHILD_SUCCESS_MARKER}");
}

#[test]
#[ignore = "isolated scale/RSS matrix; run explicitly for P2 evidence"]
fn isolated_scale_resource_matrix() {
    const CHILD_TEST: &str = "isolated_scale_child";
    for case in ScaleCase::ALL {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--ignored", "--exact", CHILD_TEST, "--nocapture"])
            .env(CHILD_ROLE_ENV, CHILD_ROLE)
            .env(CHILD_ENV, case.to_string());
        let (working_directory, output) = run_scale_child(&mut command, "scale-resource-matrix");
        let stdout = &output.stdout;
        assert!(
            output.status.success(),
            "scale case {case} failed\nstatus: {}\nworking directory: {}\nstdout report: {}\nstderr report: {}\nstdout:\n{stdout}\nstderr:\n{}",
            output.status,
            working_directory.path().display(),
            output.stdout_path.display(),
            output.stderr_path.display(),
            output.stderr,
        );
        assert_eq!(
            marker_count(stdout, CHILD_WORK_MARKER),
            1,
            "scale case {case} must prove exactly one work start marker:\n{stdout}"
        );
        assert_eq!(
            marker_count(stdout, CHILD_SUCCESS_MARKER),
            1,
            "scale case {case} must prove exactly one success marker:\n{stdout}"
        );
        let evidence = parse_scale_evidence(stdout);
        print!("{stdout}");
        assert_eq!(evidence.case, case);
        assert_eq!(
            evidence.deterministic,
            case.deterministic_oracle(),
            "deterministic scale oracle changed for {case}; elapsed_ms={} peak_rss_delta_bytes={}",
            evidence.elapsed_ms,
            evidence.peak_rss_delta_bytes,
        );
    }
}
