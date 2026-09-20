use assert_cmd::cargo::cargo_bin_cmd;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::PathBuf,
    process::Output,
    sync::atomic::{AtomicU64, Ordering},
};

const MATRIX: &str = include_str!("../../../docs/CONFIG_CLI_POLICY_MATRIX.md");
const CONFIG_SOURCE: &str = include_str!("../../pyo3_bindgen_engine/src/config.rs");
const FACADE_MANIFEST: &str = include_str!("../../pyo3_bindgen/Cargo.toml");
const ENGINE_MANIFEST: &str = include_str!("../../pyo3_bindgen_engine/Cargo.toml");
const CLI_MANIFEST: &str = include_str!("../Cargo.toml");
const MACRO_SOURCE: &str = include_str!("../../pyo3_bindgen_macros/src/lib.rs");
const SYNTAX_CLASS_SOURCE: &str = include_str!("../../pyo3_bindgen_engine/src/syntax/class.rs");
const TYPE_EMISSION_SOURCE: &str = include_str!("../../pyo3_bindgen_engine/src/typing/into_rs.rs");
const REQUIRED_COLUMNS: [&str; 10] = [
    "Index",
    "Kind",
    "Control",
    "Aliases",
    "Behavior",
    "Owner",
    "Default",
    "Source-mode applicability",
    "Conflicts",
    "Generated consumer dependency",
];

#[derive(Debug)]
struct PolicyRow {
    cells: BTreeMap<String, String>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum SourceMode {
    Live,
    ExternalIr,
}

impl SourceMode {
    fn label(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::ExternalIr => "external-ir",
        }
    }
}

struct PolicyTempDir {
    path: PathBuf,
    ir: PathBuf,
}

impl PolicyTempDir {
    fn new() -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "pyo3_bindgen_policy_matrix_{}_{}",
            std::process::id(),
            id
        ));
        fs::create_dir_all(&path).expect("policy test temporary directory should be writable");
        let ir = path.join("schema.json");
        fs::write(&ir, minimal_ir_json()).expect("policy test IR fixture should be writable");
        Self { path, ir }
    }

    fn artifact(&self, control: &str, mode: SourceMode, suffix: &str) -> PathBuf {
        let control = control.trim_start_matches('-').replace('-', "_");
        self.path
            .join(format!("{control}_{}_{suffix}", mode.label()))
    }
}

impl Drop for PolicyTempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

impl PolicyRow {
    fn get(&self, column: &str) -> &str {
        self.cells
            .get(column)
            .unwrap_or_else(|| panic!("matrix row is missing required column {column:?}"))
    }
}

fn split_table_row(line: &str) -> Vec<String> {
    let line = line.trim();
    assert!(
        line.starts_with('|') && line.ends_with('|'),
        "matrix table row must start and end with |: {line}"
    );
    line[1..line.len() - 1]
        .split('|')
        .map(|cell| cell.trim().to_owned())
        .collect()
}

fn parse_matrix() -> Vec<PolicyRow> {
    let mut lines = MATRIX.lines();
    let header_line = lines
        .find(|line| line.trim_start().starts_with("| Index |"))
        .expect("policy matrix must contain its indexed table header");
    let headers = split_table_row(header_line);
    assert_eq!(
        headers,
        REQUIRED_COLUMNS
            .iter()
            .map(|column| (*column).to_owned())
            .collect::<Vec<_>>(),
        "matrix columns changed"
    );

    let separator = lines
        .next()
        .expect("matrix header must have a separator row");
    let separator_cells = split_table_row(separator);
    assert_eq!(separator_cells.len(), REQUIRED_COLUMNS.len());
    assert!(
        separator_cells
            .iter()
            .all(|cell| cell.chars().all(|character| matches!(character, '-' | ':'))),
        "matrix separator row must contain only Markdown separator characters"
    );

    let rows = lines
        .take_while(|line| line.trim_start().starts_with('|'))
        .map(|line| {
            let cells = split_table_row(line);
            assert_eq!(
                cells.len(),
                REQUIRED_COLUMNS.len(),
                "matrix row has the wrong number of cells: {line}"
            );
            PolicyRow {
                cells: headers.iter().cloned().zip(cells).collect(),
            }
        })
        .collect::<Vec<_>>();
    assert!(!rows.is_empty(), "policy matrix must contain policy rows");
    rows
}

fn config_fields_and_defaults() -> BTreeMap<String, String> {
    let mut fields = BTreeMap::new();
    let mut in_config = false;
    let mut default_marker = None;

    for line in CONFIG_SOURCE.lines() {
        let trimmed = line.trim();
        if trimmed == "pub struct Config {" {
            in_config = true;
            continue;
        }
        if !in_config {
            continue;
        }
        if trimmed == "}" {
            break;
        }
        if trimmed.starts_with("#[builder(default = true)]") {
            default_marker = Some("true".to_owned());
        } else if trimmed.starts_with("#[builder(default = false)]") {
            default_marker = Some("false".to_owned());
        } else if trimmed.starts_with("#[builder(default)]") {
            default_marker = Some("empty".to_owned());
        } else if trimmed.starts_with("#[builder(default =") {
            default_marker = Some("custom".to_owned());
        }

        let Some(field) = trimmed.strip_prefix("pub ") else {
            continue;
        };
        let Some((name, _type)) = field.split_once(':') else {
            continue;
        };
        let name = name.trim();
        assert!(
            name.chars()
                .all(|character| character == '_' || character.is_ascii_alphanumeric()),
            "unexpected public Config member syntax: {trimmed}"
        );
        let default = default_marker
            .take()
            .unwrap_or_else(|| panic!("Config field {name} has no builder default"));
        assert!(
            fields.insert(name.to_owned(), default).is_none(),
            "Config field {name} appears more than once in the public source"
        );
    }

    assert!(
        !fields.is_empty(),
        "Config source parser found no public fields"
    );
    fields
}

fn rows_by_kind<'a>(rows: &'a [PolicyRow], kind: &str) -> BTreeMap<String, &'a PolicyRow> {
    rows.iter()
        .filter(|row| row.get("Kind") == kind)
        .map(|row| (markdown_code(row.get("Control")).to_owned(), row))
        .collect()
}

fn markdown_code(value: &str) -> &str {
    value
        .strip_prefix('`')
        .and_then(|value| value.strip_suffix('`'))
        .unwrap_or(value)
}

fn run_cli(args: &[String]) -> Output {
    cargo_bin_cmd!("pyo3_bindgen")
        .args(args)
        .output()
        .expect("policy-matrix CLI invocation should execute")
}

fn output_text(output: &Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn expected_file_publication_exit_code() -> i32 {
    if cfg!(unix) {
        0
    } else {
        3
    }
}

fn expected_applicability_exit_code(control: &str) -> i32 {
    match control {
        "--output" | "--output-dir" | "--report" => expected_file_publication_exit_code(),
        _ => 0,
    }
}

fn flag_args(control: &str, temp: &PolicyTempDir, mode: SourceMode) -> Vec<String> {
    let artifact = |suffix| {
        temp.artifact(control, mode, suffix)
            .to_string_lossy()
            .into_owned()
    };

    match control {
        "--module-name" => vec!["--module-name".to_owned(), "sys".to_owned()],
        "--ir" => vec!["--ir".to_owned(), temp.ir.to_string_lossy().into_owned()],
        "--output" => vec!["--output".to_owned(), artifact("bindings.rs")],
        "--output-dir" => {
            let output_dir = temp.artifact(control, mode, "directory");
            fs::create_dir_all(&output_dir).expect("policy output directory should be writable");
            vec![
                "--output-dir".to_owned(),
                output_dir.to_string_lossy().into_owned(),
            ]
        }
        "--report" => vec!["--report".to_owned(), artifact("report.json")],
        "--check" => vec!["--check".to_owned()],
        "--deny-warnings" => vec!["--deny-warnings".to_owned()],
        "--warning-budget" => vec!["--warning-budget".to_owned(), "1000000".to_owned()],
        "--include-private" => vec!["--include-private".to_owned()],
        "--no-traverse-submodules" => vec!["--no-traverse-submodules".to_owned()],
        "--blocklist-name" => vec![
            "--blocklist-name".to_owned(),
            "policy_matrix_name_that_is_not_present".to_owned(),
        ],
        "--no-deduplicate-inherited-members" => {
            vec!["--no-deduplicate-inherited-members".to_owned()]
        }
        "--no-inherent-forwarders" => vec!["--no-inherent-forwarders".to_owned()],
        "--lenient-ir" => vec!["--lenient-ir".to_owned()],
        "--enum-class-fallback" => vec!["--enum-class-fallback".to_owned()],
        "--enum-string-backed" => vec!["--enum-string-backed".to_owned()],
        "--max-ir-bytes" => vec![
            "--max-ir-bytes".to_owned(),
            match mode {
                SourceMode::Live => "1".to_owned(),
                SourceMode::ExternalIr => "1000000".to_owned(),
            },
        ],
        "--strict-completeness" => vec!["--strict-completeness".to_owned()],
        other => panic!("no public CLI exercise is defined for {other}"),
    }
}

fn source_args(mode: SourceMode, temp: &PolicyTempDir) -> Vec<String> {
    match mode {
        SourceMode::Live => flag_args("--module-name", temp, mode),
        SourceMode::ExternalIr => flag_args("--ir", temp, mode),
    }
}

fn run_applicability_case(control: &str, mode: SourceMode, temp: &PolicyTempDir) -> Output {
    let mut args = source_args(mode, temp);

    if control == "--module-name" || control == "--ir" {
        return run_cli(&args);
    }

    if control == "--check" {
        let output_path = temp.artifact(control, mode, "checked.rs");
        let mut baseline = args.clone();
        baseline.extend([
            "--output".to_owned(),
            output_path.to_string_lossy().into_owned(),
        ]);
        let baseline_output = run_cli(&baseline);
        assert_eq!(
            baseline_output.status.code(),
            Some(expected_file_publication_exit_code()),
            "--check baseline should generate for {} mode:\n{}",
            mode.label(),
            output_text(&baseline_output)
        );
        args.extend([
            "--output".to_owned(),
            output_path.to_string_lossy().into_owned(),
            "--check".to_owned(),
        ]);
        return run_cli(&args);
    }

    args.extend(flag_args(control, temp, mode));
    run_cli(&args)
}

fn backtick_contents(value: &str) -> Vec<String> {
    value
        .split('`')
        .enumerate()
        .filter_map(|(index, part)| (index % 2 == 1).then_some(part.to_owned()))
        .collect()
}

fn direct_conflict_flags(row: &PolicyRow) -> Vec<String> {
    let conflicts = row.get("Conflicts").trim();
    if conflicts == "none"
        || conflicts.starts_with("none;")
        || conflicts.starts_with("semantic requirement")
    {
        return Vec::new();
    }
    backtick_contents(conflicts)
        .into_iter()
        .filter(|value| value.starts_with("--"))
        .collect()
}

fn documented_conflict_pairs(rows: &[PolicyRow]) -> BTreeSet<(String, String)> {
    let cli_rows = rows_by_kind(rows, "cli-flag");
    let cli_controls = cli_rows.keys().cloned().collect::<BTreeSet<_>>();
    let mut pairs = BTreeSet::new();

    for row in cli_rows.values() {
        let control = markdown_code(row.get("Control")).to_owned();
        for conflict in direct_conflict_flags(row) {
            assert!(
                cli_controls.contains(&conflict),
                "{control} documents unknown CLI conflict {conflict}"
            );
            let pair = if control < conflict {
                (control.clone(), conflict)
            } else {
                (conflict, control.clone())
            };
            pairs.insert(pair);
        }
    }
    pairs
}

fn conflict_invocation(left: &str, right: &str, temp: &PolicyTempDir) -> Vec<String> {
    let mode = if left == "--ir" || right == "--ir" {
        SourceMode::ExternalIr
    } else {
        SourceMode::Live
    };
    let mut args = flag_args(left, temp, mode);
    args.extend(flag_args(right, temp, mode));
    if left != "--module-name" && right != "--module-name" && left != "--ir" && right != "--ir" {
        args.splice(0..0, flag_args("--module-name", temp, SourceMode::Live));
    }
    args
}

fn config_default_markers() -> BTreeMap<String, String> {
    let config = pyo3_bindgen::Config::default();
    let expected_blocklist = [
        "builtins".to_owned(),
        "testing".to_owned(),
        "tests".to_owned(),
        "test".to_owned(),
    ];
    let blocklist_marker = if config.blocklist_names == expected_blocklist {
        "custom"
    } else {
        "unexpected"
    };
    let markers = BTreeMap::from([
        (
            "Config::traverse_submodules".to_owned(),
            config.traverse_submodules.to_string(),
        ),
        (
            "Config::generate_preludes".to_owned(),
            config.generate_preludes.to_string(),
        ),
        (
            "Config::generate_imports".to_owned(),
            config.generate_imports.to_string(),
        ),
        (
            "Config::generate_classes".to_owned(),
            config.generate_classes.to_string(),
        ),
        (
            "Config::generate_enums".to_owned(),
            config.generate_enums.to_string(),
        ),
        (
            "Config::enum_raw_fallback_paths".to_owned(),
            if config.enum_raw_fallback_paths.is_empty() {
                "empty".to_owned()
            } else {
                "unexpected".to_owned()
            },
        ),
        (
            "Config::enum_ir_class_fallback".to_owned(),
            config.enum_ir_class_fallback.to_string(),
        ),
        (
            "Config::enum_ir_string_backed".to_owned(),
            config.enum_ir_string_backed.to_string(),
        ),
        (
            "Config::generate_type_vars".to_owned(),
            config.generate_type_vars.to_string(),
        ),
        (
            "Config::generate_functions".to_owned(),
            config.generate_functions.to_string(),
        ),
        (
            "Config::generate_properties".to_owned(),
            config.generate_properties.to_string(),
        ),
        (
            "Config::generate_docs".to_owned(),
            config.generate_docs.to_string(),
        ),
        (
            "Config::generate_provenance".to_owned(),
            config.generate_provenance.to_string(),
        ),
        (
            "Config::blocklist_names".to_owned(),
            blocklist_marker.to_owned(),
        ),
        (
            "Config::include_private".to_owned(),
            config.include_private.to_string(),
        ),
        (
            "Config::deduplicate_inherited_members".to_owned(),
            config.deduplicate_inherited_members.to_string(),
        ),
        (
            "Config::generate_inherent_forwarders".to_owned(),
            config.generate_inherent_forwarders.to_string(),
        ),
        (
            "Config::lenient_external_ir".to_owned(),
            config.lenient_external_ir.to_string(),
        ),
        (
            "Config::generate_dependencies".to_owned(),
            config.generate_dependencies.to_string(),
        ),
        (
            "Config::strict_completeness".to_owned(),
            config.strict_completeness.to_string(),
        ),
        (
            "Config::suppress_python_stdout".to_owned(),
            config.suppress_python_stdout.to_string(),
        ),
        (
            "Config::suppress_python_stderr".to_owned(),
            config.suppress_python_stderr.to_string(),
        ),
    ]);
    assert_eq!(
        markers.len(),
        22,
        "public Config default table must stay complete"
    );
    markers
}

fn assert_complete_row_schema(rows: &[PolicyRow]) {
    let mut indexes = BTreeSet::new();
    let mut controls = BTreeSet::new();
    let allowed_kinds = [
        "config-field",
        "cli-flag",
        "macro-policy",
        "consumer-dependency",
        "workspace-internal",
    ];
    let allowed_applicability = ["live", "external-ir", "both", "n/a"];

    for row in rows {
        let index = row.get("Index");
        assert!(!index.is_empty(), "every matrix row needs an index");
        assert!(indexes.insert(index), "matrix index is not unique: {index}");

        let kind = row.get("Kind");
        assert!(
            allowed_kinds.contains(&kind),
            "unsupported matrix row kind {kind:?} at {index}"
        );
        let control = row.get("Control");
        assert!(!control.is_empty(), "row {index} needs a control");
        assert!(
            controls.insert(control.to_owned()),
            "control is duplicated in the indexed matrix: {control} (kind {kind})"
        );

        for column in [
            "Aliases",
            "Behavior",
            "Owner",
            "Default",
            "Source-mode applicability",
            "Conflicts",
            "Generated consumer dependency",
        ] {
            let value = row.get(column);
            assert!(
                !value.is_empty() && !matches!(value, "TBD" | "unknown" | "unspecified"),
                "row {index} has no usable {column} entry"
            );
        }
        assert!(
            allowed_applicability.contains(&row.get("Source-mode applicability")),
            "row {index} has invalid source-mode applicability"
        );
    }
}

fn help_long_flags() -> (BTreeSet<String>, String) {
    let output = cargo_bin_cmd!("pyo3_bindgen")
        .arg("--help")
        .output()
        .expect("CLI --help should execute");
    assert!(output.status.success(), "CLI --help failed: {output:?}");
    let help = String::from_utf8(output.stdout).expect("CLI help must be UTF-8");
    let flags = help
        .lines()
        .map(str::trim_start)
        .filter(|line| line.starts_with('-'))
        .flat_map(|line| line.split_whitespace())
        .map(|token| token.trim_end_matches(','))
        .filter(|token| token.starts_with("--"))
        .map(str::to_owned)
        .collect();
    (flags, help)
}

#[test]
fn policy_matrix_covers_public_config_and_cli_help() {
    let rows = parse_matrix();
    assert_complete_row_schema(&rows);

    let config_rows = rows_by_kind(&rows, "config-field");
    let config_fields = config_fields_and_defaults();
    assert_eq!(
        config_rows.len(),
        config_fields.len(),
        "matrix Config row count must match the public Config field count"
    );
    for (field, default_marker) in config_fields {
        let control = format!("Config::{field}");
        let row = config_rows
            .get(&control)
            .unwrap_or_else(|| panic!("public Config field {field} is missing from the matrix"));
        let documented_default = markdown_code(row.get("Default"));
        match default_marker.as_str() {
            "true" | "false" => assert_eq!(documented_default, default_marker.as_str()),
            "empty" => assert!(
                documented_default.contains("empty"),
                "{control} must document its empty default"
            ),
            "custom" => assert_eq!(
                documented_default, "[`builtins`, `testing`, `tests`, `test`]",
                "{control} must document the source-defined default list"
            ),
            marker => panic!("unknown Config default marker {marker}"),
        }
    }

    let mut public_defaults = config_default_markers();
    assert_eq!(
        public_defaults.keys().cloned().collect::<BTreeSet<_>>(),
        config_rows.keys().cloned().collect::<BTreeSet<_>>(),
        "every public Config default must have exactly one indexed row"
    );
    for (control, row) in &config_rows {
        let observed = public_defaults
            .remove(control)
            .expect("Config default was checked exactly once");
        match observed.as_str() {
            "true" | "false" => assert_eq!(markdown_code(row.get("Default")), observed),
            "empty" => assert!(
                row.get("Default").contains("empty"),
                "{control} must document its empty public default"
            ),
            "custom" => assert_eq!(
                markdown_code(row.get("Default")),
                "[`builtins`, `testing`, `tests`, `test`]",
                "{control} must document the public source-defined default list"
            ),
            marker => panic!("unexpected public Config default marker {marker}"),
        }
    }
    assert!(
        public_defaults.is_empty(),
        "all public Config defaults must be consumed by indexed rows"
    );

    let cli_rows = rows_by_kind(&rows, "cli-flag");
    let (help_flags, help) = help_long_flags();
    let matrix_flags = cli_rows.keys().cloned().collect::<BTreeSet<_>>();
    assert_eq!(
        matrix_flags, help_flags,
        "CLI long flags in help and the policy matrix must match"
    );
    for row in cli_rows.values() {
        let aliases = row.get("Aliases");
        if aliases != "none" {
            for alias in aliases.split_whitespace() {
                let alias = markdown_code(alias);
                assert!(
                    help.contains(alias),
                    "CLI alias {alias} for {} is absent from --help",
                    row.get("Control")
                );
            }
        }
    }
    assert!(
        help.contains("[default: STDOUT]"),
        "CLI help must retain the documented stdout default"
    );
    assert!(
        help.contains("[default: 67108864]"),
        "CLI help must retain the documented external-IR byte default"
    );

    let version = cargo_bin_cmd!("pyo3_bindgen")
        .arg("--version")
        .output()
        .expect("CLI --version should execute");
    assert!(
        version.status.success(),
        "CLI --version failed: {version:?}"
    );
    let version_text = String::from_utf8(version.stdout).expect("CLI version must be UTF-8");
    assert!(
        version_text.contains(env!("CARGO_PKG_VERSION")),
        "CLI --version must print the package version"
    );

    let macro_rows = rows_by_kind(&rows, "macro-policy");
    assert_eq!(
        macro_rows.keys().cloned().collect::<BTreeSet<_>>(),
        ["feature:macros", "import_python!(\"module\")"]
            .into_iter()
            .map(str::to_owned)
            .collect()
    );
    assert!(FACADE_MANIFEST.contains("macros = [\"dep:pyo3_bindgen_macros\"]"));
    assert!(
        FACADE_MANIFEST
            .lines()
            .any(|line| line.trim() == "default = []"),
        "the macro feature must remain disabled by default"
    );
    assert!(
        MACRO_SOURCE.contains("syn::parse_macro_input!(input as parser::Args)"),
        "import_python! must keep its one-parser entry point"
    );
    assert!(
        MACRO_SOURCE.contains("pyo3_bindgen_engine::Codegen::default()")
            && MACRO_SOURCE.contains("syn::Error::into_compile_error"),
        "import_python! must use the documented default live policy and call-site errors"
    );

    let dependency_rows = rows_by_kind(&rows, "consumer-dependency");
    assert_eq!(
        dependency_rows.keys().cloned().collect::<BTreeSet<_>>(),
        [
            "generated ::pyo3",
            "feature:num-complex -> ::num_complex::Complex64",
            "feature:numpy -> ::numpy::PyUntypedArray",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect()
    );
    for feature in ["numpy", "num-complex"] {
        assert!(
            FACADE_MANIFEST.contains(&format!("{feature} = [")),
            "facade feature {feature} disappeared"
        );
        assert!(
            ENGINE_MANIFEST.contains(&format!("{feature} =")),
            "engine feature {feature} disappeared"
        );
    }
    assert!(
        FACADE_MANIFEST.contains("pyo3 = { workspace = true }"),
        "the facade must retain a direct PyO3 dependency for generated consumers"
    );
    assert!(
        SYNTAX_CLASS_SOURCE.contains("::pyo3"),
        "generated bindings must keep direct ::pyo3 paths"
    );
    assert!(
        TYPE_EMISSION_SOURCE.contains("#[cfg(feature = \"numpy\")]\n            \"numpy.ndarray\"")
            && TYPE_EMISSION_SOURCE.contains("#[cfg(feature = \"num-complex\")]"),
        "optional generated type mappings must remain feature-gated"
    );
    assert!(TYPE_EMISSION_SOURCE.contains("::numpy::PyUntypedArray"));
    assert!(TYPE_EMISSION_SOURCE.contains("::num_complex::Complex64"));

    let internal_rows = rows_by_kind(&rows, "workspace-internal");
    assert_eq!(
        internal_rows.keys().cloned().collect::<BTreeSet<_>>(),
        ["feature:internal-test-hooks"]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        "every workspace-internal control needs exactly one matrix row"
    );
    assert!(
        ENGINE_MANIFEST
            .lines()
            .any(|line| line.trim() == "internal-test-hooks = []")
            && FACADE_MANIFEST.lines().any(|line| {
                line.trim() == "internal-test-hooks = [\"pyo3_bindgen_engine/internal-test-hooks\"]"
            }),
        "the internal test hooks must stay a declared, non-default feature of engine and facade"
    );
    assert!(
        !FACADE_MANIFEST.contains("default = [\"internal-test-hooks\"")
            && !ENGINE_MANIFEST.contains("default = [\"internal-test-hooks\""),
        "publication fault injection must never reach a default build"
    );
    assert!(
        CLI_MANIFEST.contains("[dev-dependencies]")
            && CLI_MANIFEST
                .split("[dev-dependencies]")
                .nth(1)
                .is_some_and(|section| section.contains("features = [\"internal-test-hooks\"]")),
        "the CLI must request the internal test hooks as a dev-dependency only"
    );
}

fn minimal_ir_json() -> String {
    serde_json::json!({
        "modules": [{
            "name": "policy_fixture",
            "is_package": false,
            "classes": [],
            "functions": [],
            "properties": [],
            "imports": [],
            "submodules": [],
            "prelude": []
        }]
    })
    .to_string()
}

#[test]
fn indexed_cli_conflicts_are_exercised_at_the_public_parser() {
    let rows = parse_matrix();
    assert_complete_row_schema(&rows);
    let pairs = documented_conflict_pairs(&rows);
    assert!(
        !pairs.is_empty(),
        "the matrix must contain direct CLI conflicts"
    );
    let temp = PolicyTempDir::new();

    for (left, right) in &pairs {
        let output = run_cli(&conflict_invocation(left, right, &temp));
        let diagnostics = output_text(&output);
        assert!(
            !output.status.success(),
            "documented conflict {left} + {right} unexpectedly parsed:\n{diagnostics}"
        );
        assert!(
            diagnostics.contains("cannot be used with"),
            "documented conflict {left} + {right} did not reach Clap's conflict rule:\n{diagnostics}"
        );
    }

    for row in rows.iter().filter(|row| row.get("Kind") == "cli-flag") {
        let control = markdown_code(row.get("Control"));
        let applicability = row.get("Source-mode applicability");
        let conflicts = direct_conflict_flags(row);
        match applicability {
            "live" => assert!(
                conflicts.contains(&"--ir".to_owned()) || control == "--max-ir-bytes",
                "live-only CLI row {control} must document its external-IR conflict"
            ),
            "external-ir" => assert!(
                conflicts.contains(&"--module-name".to_owned()) || control == "--max-ir-bytes",
                "external-IR-only CLI row {control} must document its live-source conflict"
            ),
            "both" | "n/a" => {}
            other => panic!("unsupported CLI applicability {other:?} for {control}"),
        }
    }
}

#[test]
fn indexed_cli_source_applicability_is_exercised_at_the_public_interface() {
    let rows = parse_matrix();
    assert_complete_row_schema(&rows);
    let cli_rows = rows_by_kind(&rows, "cli-flag");
    let temp = PolicyTempDir::new();
    let mut exercised = BTreeSet::new();

    for row in cli_rows.values() {
        let control = markdown_code(row.get("Control"));
        exercised.insert(control.to_owned());

        if matches!(control, "--help" | "--version") {
            let output = run_cli(&[control.to_owned()]);
            assert!(
                output.status.success(),
                "{control} should be a successful display action:\n{}",
                output_text(&output)
            );
            let alias = if control == "--help" { "-h" } else { "-V" };
            let alias_output = run_cli(&[alias.to_owned()]);
            assert!(
                alias_output.status.success(),
                "{alias} should be the documented alias for {control}:\n{}",
                output_text(&alias_output)
            );
            continue;
        }

        let mut modes = match row.get("Source-mode applicability") {
            "live" => vec![SourceMode::Live],
            "external-ir" => vec![SourceMode::ExternalIr],
            "both" => vec![SourceMode::Live, SourceMode::ExternalIr],
            "n/a" => panic!("CLI row {control} cannot have n/a applicability"),
            other => panic!("unsupported CLI applicability {other:?} for {control}"),
        };
        if control == "--max-ir-bytes" {
            // The matrix deliberately documents the parser's accepted-but-unused live value.
            modes.push(SourceMode::Live);
        }
        modes.sort_unstable();
        modes.dedup();

        for mode in modes {
            let output = run_applicability_case(control, mode, &temp);
            assert_eq!(
                output.status.code(),
                Some(expected_applicability_exit_code(control)),
                "{control} should be executable in {} mode:\n{}",
                mode.label(),
                output_text(&output)
            );
        }
    }

    assert_eq!(
        exercised,
        cli_rows.keys().cloned().collect::<BTreeSet<_>>(),
        "every CLI flag must have exactly one indexed public-interface exercise"
    );
}

#[test]
fn indexed_check_requirement_is_exercised_for_both_sources() {
    let rows = parse_matrix();
    assert_complete_row_schema(&rows);
    let cli_rows = rows_by_kind(&rows, "cli-flag");
    let check = cli_rows
        .get("--check")
        .expect("--check must have one indexed row");
    assert_eq!(
        check.get("Conflicts"),
        r#"semantic requirement after parsing: `--output` or `--output-dir`"#
    );
    let temp = PolicyTempDir::new();

    for mode in [SourceMode::Live, SourceMode::ExternalIr] {
        let mut args = source_args(mode, &temp);
        args.push("--check".to_owned());
        let output = run_cli(&args);
        let diagnostics = output_text(&output);
        assert!(
            !output.status.success(),
            "--check without a target should fail in {} mode:\n{diagnostics}",
            mode.label()
        );
        assert!(
            diagnostics.contains("--check requires --output or --output-dir"),
            "--check's semantic requirement changed in {} mode:\n{diagnostics}",
            mode.label()
        );
    }
}
