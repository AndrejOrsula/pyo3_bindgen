//! Matrix compile-oracle: every target in the table below must GENERATE and COMPILE.
//!
//! The rest of the suite compiles exactly one real library (`gymnasium.envs.registration`,
//! see `tests/gymnasium_dogfood.rs`) plus synthetic IR (`tests/ir_hierarchy_compile.rs`).
//! This sweep encodes the much broader generate-and-compile matrix that was previously only
//! ever run by hand: ~90 standard-library modules flat, a traversed slice, the two emission
//! toggles that have no CLI flag (`Config::generate_docs`,
//! `Config::generate_inherent_forwarders`) and the heavy third-party targets.
//!
//! The matrix tests below are `#[ignore]`d so that `cargo test --workspace --all-targets` keeps
//! its current cost; the bounded consumer ownership regressions run normally. Run the sweep
//! explicitly:
//!
//! ```text
//! scripts/compile-sweep.sh                 # or, equivalently:
//! cargo test -p pyo3_bindgen_engine --test compile_sweep -- --ignored --test-threads=1
//! ```
//!
//! `--test-threads=1` is mandatory: every batch spawns a `cargo check` against the same
//! shared target directory, so concurrent batches would serialize on the cargo lock while
//! stacking peak rustc memory.

use pyo3_bindgen_engine::{Codegen, Config};
use std::{fs, process::Command};

#[path = "../test_support/bounded_consumer.rs"]
mod bounded_consumer;

/// One row of the sweep matrix: a Python module plus the emission toggles it exercises.
#[derive(Clone, Copy)]
struct Target {
    module: &'static str,
    traverse: bool,
    docs: bool,
    forwarders: bool,
}

/// Default row: flat (no submodule traversal), docs on, inherent forwarders on.
const fn target(module: &'static str) -> Target {
    Target {
        module,
        traverse: false,
        docs: true,
        forwarders: true,
    }
}

/// Standard-library targets that are generated without submodule traversal.
///
/// Empirically verified to generate AND compile on CPython 3.10. `builtins` is included: it
/// emits a `pub struct str` newtype that shadows the primitive, and the module-scoped
/// provenance consts now qualify their type as `&::core::primitive::str` to compile through it.
const STDLIB_FLAT: &[&str] = &[
    "abc",
    "argparse",
    "array",
    "ast",
    "base64",
    "binascii",
    "bisect",
    "builtins",
    "calendar",
    "cmath",
    "codecs",
    "colorsys",
    "configparser",
    "contextlib",
    "copy",
    "csv",
    "datetime",
    "decimal",
    "difflib",
    "enum",
    "errno",
    "filecmp",
    "fnmatch",
    "fractions",
    "functools",
    "gc",
    "getpass",
    "glob",
    "gzip",
    "hashlib",
    "heapq",
    "hmac",
    "html",
    "http",
    "imaplib",
    "inspect",
    "io",
    "ipaddress",
    "itertools",
    "keyword",
    "linecache",
    "locale",
    "logging",
    "lzma",
    "marshal",
    "math",
    "mimetypes",
    "numbers",
    "operator",
    "os",
    "pathlib",
    "pickle",
    "pkgutil",
    "platform",
    "pprint",
    "queue",
    "quopri",
    "random",
    "re",
    "reprlib",
    "secrets",
    "select",
    "shlex",
    "shutil",
    "signal",
    "socket",
    "sqlite3",
    "stat",
    "statistics",
    "string",
    "stringprep",
    "struct",
    "subprocess",
    "symtable",
    "sys",
    "tarfile",
    "tempfile",
    "textwrap",
    "threading",
    "time",
    "timeit",
    "token",
    "tokenize",
    "traceback",
    "types",
    "typing",
    "unicodedata",
    "uuid",
    "warnings",
    "wave",
    "weakref",
    "zipfile",
    "zlib",
];

/// Traversal changes the object model (submodule nesting, cross-module base classes), so it
/// needs its own targets rather than a flag on the flat ones.
const STDLIB_TRAVERSED: &[&str] = &["json", "collections", "urllib", "email"];

/// Heavy third-party targets. Only meaningful traversed: their top level is lazy, so a flat
/// generation yields a near-empty module.
const HEAVY_TRAVERSED: &[&str] = &["numpy", "pandas", "scipy", "yaml", "requests", "gymnasium"];

/// When set, a heavy target that is not importable is a hard failure instead of a skip.
/// Mirrors `PYO3_BINDGEN_REQUIRE_GYMNASIUM` in `tests/gymnasium_dogfood.rs`.
const REQUIRE_HEAVY_ENV: &str = "PYO3_BINDGEN_REQUIRE_HEAVY";

/// Generate `target` in-process and pretty-print it.
///
/// Generation goes through `Codegen::generate()` (not `generate_with_report()`): the sweep is
/// a compile oracle and must not depend on the report surface.
fn generate(target: Target) -> String {
    let cfg = Config::builder()
        .traverse_submodules(target.traverse)
        .generate_docs(target.docs)
        .generate_inherent_forwarders(target.forwarders)
        .build();
    let tokens = Codegen::new(cfg)
        .module_name(target.module)
        .unwrap_or_else(|error| panic!("{}: introspection failed: {error}", target.module))
        .generate()
        .unwrap_or_else(|error| panic!("{}: generation failed: {error}", target.module));
    let file = syn::parse2(tokens).unwrap_or_else(|error| {
        panic!(
            "{}: generated tokens do not parse as Rust: {error}",
            target.module
        )
    });
    prettyplease::unparse(&file)
}

/// Whether the Python interpreter can import `module` at all.
fn importable(module: &str) -> bool {
    pyo3::Python::initialize();
    pyo3::Python::attach(|py| py.import(module).is_ok())
}

/// A module path is not a valid Rust file stem; `email.mime` becomes `email_mime`.
fn sanitize(module: &str) -> String {
    module.replace('.', "_")
}

/// Target directory shared by every throwaway crate of a run, so the pinned pyo3 is built
/// exactly once instead of once per batch. Overridable so CI can place it on the runner's
/// temp volume and `scripts/compile-sweep.sh` can keep it warm between runs.
fn sweep_target_dir() -> std::path::PathBuf {
    std::env::var_os("PYO3_BINDGEN_SWEEP_TARGET_DIR")
        .map(Into::into)
        .unwrap_or_else(|| std::env::temp_dir().join("pyo3_bindgen_compile_sweep_target"))
}

/// Write `units` as sibling files plus a `lib.rs` that `#[path]`-mounts each one, then run
/// `cargo check` over the whole batch.
///
/// One file per unit keeps each target's prelude and `PyObjectBase` in its own namespace, so
/// N targets cost ONE rustc invocation. `units` is `(module file stem, generated source)`.
fn assert_batch_compiles(batch: &str, units: &[(String, String)]) {
    let mut crate_guard =
        bounded_consumer::OwnedTempDir::allocate(&format!("pyo3_bindgen_sweep_{batch}"))
            .unwrap_or_else(|error| {
                panic!("temporary compile-sweep crate must be allocated: {error}")
            });
    let crate_dir = crate_guard.path().to_owned();
    fs::create_dir_all(crate_dir.join("src")).unwrap();

    // Minimal consumer manifest. The pyo3 requirement is taken from the workspace pin that
    // `build.rs` exposes (never hardcoded) so the consumer can never drift to a different
    // pyo3 than the one the emitted code targets; the feature set matches for the same
    // reason, and so the shared target directory is reused across every batch.
    let pyo3_version = env!("PYO3_BINDGEN_PYO3_VERSION");
    fs::write(
        crate_dir.join("Cargo.toml"),
        format!(
            r#"[package]
name = "pyo3_bindgen_compile_sweep_{batch}"
version = "0.0.0"
edition = "2021"
publish = false

[workspace]

[dependencies]
pyo3 = {{ version = "={pyo3_version}", default-features = false, features = ["py-clone"] }}
"#
        ),
    )
    .unwrap();

    let mut lib = String::new();
    for (name, source) in units {
        let file = format!("{name}.rs");
        fs::write(crate_dir.join("src").join(&file), source).unwrap();
        // NOTE: never build this list by globbing `src/*.rs` -- that self-includes `lib.rs`
        // and yields `error: circular modules`.
        lib.push_str(&format!("#[path = \"{file}\"] pub mod unit_{name};\n"));
    }
    fs::write(crate_dir.join("src/lib.rs"), lib).unwrap();

    // Deliberately NOT `--offline`: the shared target directory may be cold on a developer
    // machine, and a cold sweep must not fail on a registry that was simply never warmed.
    // The shared target directory is deliberately not task-owned and is never cleaned here.
    let mut command = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned()));
    command
        .arg("check")
        .arg("--quiet")
        .current_dir(&crate_dir)
        .env("CARGO_TARGET_DIR", sweep_target_dir());
    let output = match crate_guard.run(&mut command, bounded_consumer::CARGO_COMMAND_TIMEOUT) {
        Ok(output) => output,
        Err(error) => panic!(
            "compile sweep batch `{batch}` could not complete: {error}\nstdout:\n{}\nstderr:\n{}",
            error.stdout(),
            error.stderr(),
        ),
    };

    assert!(
        output.status.success(),
        "compile sweep batch `{batch}` failed ({} units: {})\nstatus: {}\nstdout:\n{}\nstderr:\n{}",
        units.len(),
        units
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>()
            .join(", "),
        output.status,
        output.stdout,
        output.stderr,
    );
}

#[test]
fn bounded_consumer_timeout_reaps_direct_child_and_retains_directory() {
    bounded_consumer::assert_timeout_regression(
        "bounded_consumer_timeout_reaps_direct_child_and_retains_directory",
    );
}

#[test]
#[ignore = "compile sweep; run with --ignored (see scripts/compile-sweep.sh)"]
fn stdlib_flat_targets_generate_and_compile() {
    let units: Vec<_> = STDLIB_FLAT
        .iter()
        .map(|module| (sanitize(module), generate(target(module))))
        .collect();
    assert_batch_compiles("stdlib_flat", &units);
}

#[test]
#[ignore = "compile sweep; run with --ignored (see scripts/compile-sweep.sh)"]
fn stdlib_traversed_targets_generate_and_compile() {
    let units: Vec<_> = STDLIB_TRAVERSED
        .iter()
        .map(|module| {
            (
                sanitize(module),
                generate(Target {
                    traverse: true,
                    ..target(module)
                }),
            )
        })
        .collect();
    assert_batch_compiles("stdlib_traversed", &units);
}

/// Docs-off is a distinct emission path (`Config::generate_docs`) with no CLI flag, and the
/// live inheritance-deduplication size model reads it, so a docs-off regression is invisible
/// to every other test. A small representative slice keeps it cheap.
#[test]
#[ignore = "compile sweep; run with --ignored (see scripts/compile-sweep.sh)"]
fn docs_off_targets_generate_and_compile() {
    let units: Vec<_> = ["os", "typing", "logging", "argparse", "socket"]
        .iter()
        .map(|module| {
            (
                sanitize(module),
                generate(Target {
                    docs: false,
                    ..target(module)
                }),
            )
        })
        .collect();
    assert_batch_compiles("docs_off", &units);
}

/// The forwarder-free emission model (`--no-inherent-forwarders`, as consumed downstream)
/// over REAL hierarchies, complementing the synthetic-IR-only coverage of
/// `tests/ir_hierarchy_compile.rs`.
#[test]
#[ignore = "compile sweep; run with --ignored (see scripts/compile-sweep.sh)"]
fn no_inherent_forwarder_targets_generate_and_compile() {
    let units: Vec<_> = ["pathlib", "argparse", "logging", "http"]
        .iter()
        .map(|module| {
            (
                sanitize(module),
                generate(Target {
                    forwarders: false,
                    ..target(module)
                }),
            )
        })
        .collect();
    assert_batch_compiles("no_forwarders", &units);
}

/// Heavy third-party targets, compiled ONE CRATE PER TARGET to bound peak rustc memory
/// (pandas alone is >10 MB of generated Rust). Do not "optimize" this into a single batch.
#[test]
#[ignore = "compile sweep; run with --ignored (see scripts/compile-sweep.sh)"]
fn heavy_traversed_targets_generate_and_compile() {
    for module in HEAVY_TRAVERSED {
        if !importable(module) {
            assert!(
                std::env::var(REQUIRE_HEAVY_ENV).is_err(),
                "{REQUIRE_HEAVY_ENV} is set but the Python `{module}` package is not importable; \
                 install the pinned heavy set (see scripts/compile-sweep.sh) before running this \
                 test"
            );
            eprintln!("SKIPPED heavy target `{module}` (not importable)");
            continue;
        }
        let source = generate(Target {
            traverse: true,
            ..target(module)
        });
        assert_batch_compiles(
            &format!("heavy_{}", sanitize(module)),
            &[(sanitize(module), source)],
        );
    }
}

/// Two independent generations of the same target must be byte-identical. Guards the class of
/// bug already fixed once (`__all__` set-ordering). No compile, so it is nearly free.
#[test]
#[ignore = "compile sweep; run with --ignored (see scripts/compile-sweep.sh)"]
fn sweep_generation_is_byte_deterministic() {
    for module in ["os", "typing", "email", "logging"] {
        let first = generate(Target {
            traverse: true,
            ..target(module)
        });
        let second = generate(Target {
            traverse: true,
            ..target(module)
        });
        assert_eq!(
            first, second,
            "generation for `{module}` is not deterministic"
        );
    }
}
