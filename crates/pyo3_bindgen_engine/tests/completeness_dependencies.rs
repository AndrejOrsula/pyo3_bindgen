use pyo3::{
    exceptions::PyModuleNotFoundError,
    types::{PyAnyMethods, PyDict, PyDictMethods, PyModule, PyModuleMethods},
};
use pyo3_bindgen_engine::{Codegen, Config, DiagnosticSeverity};
use std::{
    fs,
    io::{self, Write},
    panic::{catch_unwind, AssertUnwindSafe},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
};

static OUTPUT_COUNTER: AtomicU64 = AtomicU64::new(0);
static PACKAGE_COUNTER: AtomicU64 = AtomicU64::new(0);
static DEPENDENCY_TEST_LOCK: Mutex<()> = Mutex::new(());

fn report_fixture_cleanup_errors(fixture: &str, errors: impl IntoIterator<Item = String>) {
    let mut stderr = io::stderr().lock();
    for error in errors {
        let _ = writeln!(stderr, "{fixture} cleanup failed: {error}");
    }
}

fn cleanup_dependency_imports(
    py: pyo3::Python<'_>,
    root: &Path,
    package_name: &str,
) -> Vec<String> {
    let mut errors = Vec::new();
    let root = root.to_string_lossy().into_owned();
    let sys = match py.import("sys") {
        Ok(sys) => sys,
        Err(error) => {
            errors.push(format!("importing sys: {error}"));
            return errors;
        }
    };

    match sys.getattr("path") {
        Ok(path) => {
            if let Err(error) = path.call_method1("remove", (root.as_str(),)) {
                errors.push(format!("removing sys.path entry {root:?}: {error}"));
            }
        }
        Err(error) => errors.push(format!("reading sys.path: {error}")),
    }

    let modules = match sys.getattr("modules") {
        Ok(modules) => modules,
        Err(error) => {
            errors.push(format!("reading sys.modules: {error}"));
            return errors;
        }
    };
    let prefix = format!("{package_name}.");
    let keys = modules
        .call_method0("copy")
        .and_then(|copy| copy.call_method0("keys"))
        .and_then(|keys| keys.try_iter());
    match keys {
        Ok(keys) => {
            for key in keys {
                let key = match key {
                    Ok(key) => key,
                    Err(error) => {
                        errors.push(format!("reading sys.modules key: {error}"));
                        continue;
                    }
                };
                let Ok(module_name) = key.extract::<String>() else {
                    continue;
                };
                if module_name == package_name || module_name.starts_with(&prefix) {
                    if let Err(error) = modules.del_item(&key) {
                        errors.push(format!("removing sys.modules[{module_name:?}]: {error}"));
                    }
                }
            }
        }
        Err(error) => errors.push(format!("enumerating sys.modules: {error}")),
    }
    errors
}

fn cleanup_dependency_imports_safely(root: &Path, package_name: &str) -> Vec<String> {
    match catch_unwind(AssertUnwindSafe(|| {
        pyo3::Python::attach(|py| cleanup_dependency_imports(py, root, package_name))
    })) {
        Ok(errors) => errors,
        Err(_) => vec!["Python import cleanup panicked".to_owned()],
    }
}

struct TempDependencyPackage {
    name: String,
    root: PathBuf,
}

impl TempDependencyPackage {
    fn new() -> Self {
        let (name, root) = loop {
            let id = PACKAGE_COUNTER.fetch_add(1, Ordering::Relaxed);
            let name = format!("pyo3_bindgen_duplicate_dependency_{id}");
            let root = std::env::temp_dir().join(format!("{name}_{}", std::process::id()));
            match fs::create_dir(&root) {
                Ok(()) => break (name, root),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("failed to allocate temporary dependency root: {error}"),
            }
        };
        let package = root.join(&name);
        fs::create_dir(&package).unwrap();
        fs::write(package.join("__init__.py"), "").unwrap();
        fs::write(package.join("child.py"), "VALUE = 1\n").unwrap();

        pyo3::Python::initialize();
        pyo3::Python::attach(|py| -> pyo3::PyResult<()> {
            py.import("sys")?
                .getattr("path")?
                .call_method1("insert", (0, root.to_string_lossy().into_owned()))?;
            Ok(())
        })
        .unwrap();

        Self { name, root }
    }
}

impl Drop for TempDependencyPackage {
    fn drop(&mut self) {
        let mut errors = cleanup_dependency_imports_safely(&self.root, &self.name);
        if let Err(error) = fs::remove_dir_all(&self.root) {
            if error.kind() != std::io::ErrorKind::NotFound {
                errors.push(format!(
                    "removing directory {}: {error}",
                    self.root.display()
                ));
            }
        }
        report_fixture_cleanup_errors("temporary dependency package", errors);
    }
}

struct PrivateAliasDependencyPackage {
    name: String,
    root: PathBuf,
}

impl PrivateAliasDependencyPackage {
    fn new() -> Self {
        Self::new_inner(false)
    }

    fn new_malformed_name() -> Self {
        Self::new_inner(true)
    }

    fn new_inner(malformed_name: bool) -> Self {
        let (name, root) = loop {
            let id = PACKAGE_COUNTER.fetch_add(1, Ordering::Relaxed);
            let name = format!("pyo3_bindgen_private_alias_dependency_{id}");
            let root = std::env::temp_dir().join(format!("{name}_{}", std::process::id()));
            match fs::create_dir(&root) {
                Ok(()) => break (name, root),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("failed to allocate temporary dependency root: {error}"),
            }
        };
        let package = root.join(&name);
        fs::create_dir(&package).unwrap();
        let module_name_override = if malformed_name {
            format!(
                "_private.__name__ = {malformed_name:?}\n",
                malformed_name = format!("{name}::malformed")
            )
        } else {
            String::new()
        };
        fs::write(
            package.join("__init__.py"),
            format!("from . import _private\n{module_name_override}public = _private\n"),
        )
        .unwrap();
        fs::write(
            package.join("_private.py"),
            format!(
                "class Imported:\n    value = 1\nImported.__module__ = {public_name:?}\n",
                public_name = format!("{name}.public"),
            ),
        )
        .unwrap();

        pyo3::Python::initialize();
        pyo3::Python::attach(|py| -> pyo3::PyResult<()> {
            py.import("sys")?
                .getattr("path")?
                .call_method1("insert", (0, root.to_string_lossy().into_owned()))?;
            Ok(())
        })
        .unwrap();

        Self { name, root }
    }
}

impl Drop for PrivateAliasDependencyPackage {
    fn drop(&mut self) {
        let mut errors = cleanup_dependency_imports_safely(&self.root, &self.name);
        if let Err(error) = fs::remove_dir_all(&self.root) {
            if error.kind() != std::io::ErrorKind::NotFound {
                errors.push(format!(
                    "removing directory {}: {error}",
                    self.root.display()
                ));
            }
        }
        report_fixture_cleanup_errors("private alias dependency package", errors);
    }
}

struct HostileDependencyPackage {
    name: String,
    root: PathBuf,
    original_sys_class: pyo3::Py<pyo3::types::PyAny>,
    original_stdout: pyo3::Py<pyo3::types::PyAny>,
    original_stderr: pyo3::Py<pyo3::types::PyAny>,
    protected_stdlib_snapshots: Vec<ProtectedStdlibSnapshot>,
}

impl HostileDependencyPackage {
    fn new() -> Self {
        Self::new_inner(false, false)
    }

    fn new_parse_failure() -> Self {
        Self::new_inner(true, false)
    }

    fn new_cleanup_failure() -> Self {
        Self::new_inner(false, true)
    }

    fn new_inner(parse_failure: bool, cleanup_failure: bool) -> Self {
        let (name, root) = loop {
            let id = PACKAGE_COUNTER.fetch_add(1, Ordering::Relaxed);
            let name = format!("pyo3_bindgen_hostile_dependency_{id}");
            let root = std::env::temp_dir().join(format!("{name}_{}", std::process::id()));
            match fs::create_dir(&root) {
                Ok(()) => break (name, root),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("failed to allocate temporary dependency root: {error}"),
            }
        };
        let package = root.join(&name);
        fs::create_dir(&package).unwrap();
        let parse_failure_suffix = if parse_failure {
            "\ndef __dir__():\n    return [\"\"]\n"
        } else {
            ""
        };
        let cleanup_failure_suffix = if cleanup_failure {
            r#"
import sys as _sys

class _OneShotHostileSys(type(_sys)):
    armed = False
    remaining_failures = ["stdout"]

    def __setattr__(self, name, value):
        if type(self).armed and name in type(self).remaining_failures:
            type(self).remaining_failures.remove(name)
            type(self).armed = False
            raise RuntimeError("hostile sys.__setattr__ failed for " + name)
        return super().__setattr__(name, value)

_sys.__class__ = _OneShotHostileSys
_OneShotHostileSys.armed = True
"#
        } else {
            ""
        };
        fs::write(
            package.join("__init__.py"),
            format!(
                r#"import inspect as _inspect
import functools as _functools
import _functools as _c_functools
import enum as _enum
import typing as _typing
import dataclasses as _dataclasses
import types as _types
import pkgutil as _pkgutil
import builtins as _builtins

try:
    import annotationlib as _annotationlib
except ModuleNotFoundError as error:
    if error.name != "annotationlib":
        raise
    _annotationlib = None

def _replace(module, key, marker):
    module.__dict__[key] = object()
    module.__dict__[marker] = object()

_replace(_inspect, "signature", "_pyo3_bindgen_hostile_inspect")
_replace(_functools, "partial", "_pyo3_bindgen_hostile_functools")
_replace(_c_functools, "partial", "_pyo3_bindgen_hostile_c_functools")
_replace(_enum, "Enum", "_pyo3_bindgen_hostile_enum")
_replace(_typing, "TypeVar", "_pyo3_bindgen_hostile_typing")
_replace(_dataclasses, "fields", "_pyo3_bindgen_hostile_dataclasses")
_replace(_types, "__name__", "_pyo3_bindgen_hostile_types")
_replace(_pkgutil, "iter_modules", "_pyo3_bindgen_hostile_pkgutil")
_replace(_builtins, "property", "_pyo3_bindgen_hostile_builtins")
if _annotationlib is not None:
    _replace(_annotationlib, "get_annotations", "_pyo3_bindgen_hostile_annotationlib")
{parse_failure_suffix}{cleanup_failure_suffix}"#,
                parse_failure_suffix = parse_failure_suffix,
                cleanup_failure_suffix = cleanup_failure_suffix,
            ),
        )
        .unwrap();

        pyo3::Python::initialize();
        let (original_sys_class, original_stdout, original_stderr, protected_stdlib_snapshots) =
            pyo3::Python::attach(|py| -> Result<_, pyo3_bindgen_engine::PyBindgenError> {
                Codegen::prime_live_runtime(py)?;
                let sys = py.import("sys")?;
                Ok((
                    sys.getattr("__class__")?.unbind(),
                    sys.getattr("stdout")?.unbind(),
                    sys.getattr("stderr")?.unbind(),
                    capture_protected_stdlib_snapshots(py)?,
                ))
            })
            .unwrap();
        pyo3::Python::attach(|py| -> pyo3::PyResult<()> {
            py.import("sys")?
                .getattr("path")?
                .call_method1("insert", (0, root.to_string_lossy().into_owned()))?;
            Ok(())
        })
        .unwrap();

        Self {
            name,
            root,
            original_sys_class,
            original_stdout,
            original_stderr,
            protected_stdlib_snapshots,
        }
    }
}

impl Drop for HostileDependencyPackage {
    fn drop(&mut self) {
        let cleanup = catch_unwind(AssertUnwindSafe(|| {
            pyo3::Python::attach(|py| {
                let mut errors = cleanup_dependency_imports(py, &self.root, &self.name);
                match py.import("sys") {
                    Ok(sys) => {
                        if let Err(error) =
                            sys.setattr("__class__", self.original_sys_class.bind(py))
                        {
                            errors.push(format!("restoring sys.__class__: {error}"));
                        }
                        if let Err(error) = sys.setattr("stdout", self.original_stdout.bind(py)) {
                            errors.push(format!("restoring sys.stdout: {error}"));
                        }
                        if let Err(error) = sys.setattr("stderr", self.original_stderr.bind(py)) {
                            errors.push(format!("restoring sys.stderr: {error}"));
                        }
                    }
                    Err(error) => errors.push(format!("importing sys for restoration: {error}")),
                }
                errors.extend(restore_protected_stdlib_snapshots(
                    py,
                    &self.protected_stdlib_snapshots,
                ));
                errors
            })
        }));
        let mut errors = match cleanup {
            Ok(errors) => errors,
            Err(_) => vec!["Python hostile-fixture cleanup panicked".to_owned()],
        };
        if let Err(error) = fs::remove_dir_all(&self.root) {
            if error.kind() != std::io::ErrorKind::NotFound {
                errors.push(format!(
                    "removing directory {}: {error}",
                    self.root.display()
                ));
            }
        }
        report_fixture_cleanup_errors("hostile dependency package", errors);
    }
}

const TARGET_SOURCE: &str = r#"
class MissingRoot:
    __module__ = "pyo3_bindgen_missing_dependency_root"

class Truncated:
    __module__ = "json.decoder.not_a_module"

class FinalItem:
    __module__ = "json.decoder"
"#;

fn dependency_config(strict_completeness: bool) -> Config {
    Config::builder()
        .generate_dependencies(true)
        .strict_completeness(strict_completeness)
        .build()
}

fn generated(strict_completeness: bool) -> Result<Codegen, pyo3_bindgen_engine::PyBindgenError> {
    Codegen::new(dependency_config(strict_completeness))
        .module_from_str(TARGET_SOURCE, "pyo3_bindgen_dependency_target")
}

struct OwnedOutputPath {
    root: PathBuf,
    path: PathBuf,
}

impl OwnedOutputPath {
    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for OwnedOutputPath {
    fn drop(&mut self) {
        let mut errors = Vec::new();
        if let Err(error) = fs::remove_dir_all(&self.root) {
            if error.kind() != std::io::ErrorKind::NotFound {
                errors.push(format!(
                    "removing directory {}: {error}",
                    self.root.display()
                ));
            }
        }
        report_fixture_cleanup_errors("strict completeness output", errors);
    }
}

fn fresh_output_path() -> OwnedOutputPath {
    let (root, path) = loop {
        let id = OUTPUT_COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "pyo3_bindgen_dependency_strict_{}_{}",
            std::process::id(),
            id
        ));
        match fs::create_dir(&root) {
            Ok(()) => break (root.clone(), root.join("output.rs")),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => panic!("failed to allocate strict completeness output root: {error}"),
        }
    };
    OwnedOutputPath { root, path }
}

fn dependency_test_guard() -> std::sync::MutexGuard<'static, ()> {
    DEPENDENCY_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

const PROTECTED_STDLIB_SNAPSHOT_SPECS: &[(&str, &str)] = &[
    ("inspect", "signature"),
    ("functools", "partial"),
    ("_functools", "partial"),
    ("enum", "Enum"),
    ("typing", "TypeVar"),
    ("dataclasses", "fields"),
    ("types", "__name__"),
    ("pkgutil", "iter_modules"),
    ("builtins", "property"),
];

struct ProtectedStdlibSnapshot {
    module_name: &'static str,
    anchor_name: &'static str,
    module: pyo3::Py<PyModule>,
    dictionary: pyo3::Py<PyDict>,
    anchor: pyo3::Py<pyo3::types::PyAny>,
}

fn protected_stdlib_snapshots_match(
    py: pyo3::Python<'_>,
    snapshots: &[ProtectedStdlibSnapshot],
) -> bool {
    snapshots.iter().all(|snapshot| {
        let actual = snapshot.module.bind(py).dict();
        let expected = snapshot.dictionary.bind(py);
        actual.len() == expected.len()
            && actual.iter().zip(expected.iter()).all(
                |((actual_key, actual_value), (expected_key, expected_value))| {
                    actual_key.is(&expected_key) && actual_value.is(&expected_value)
                },
            )
    })
}

fn restore_protected_stdlib_snapshots(
    py: pyo3::Python<'_>,
    snapshots: &[ProtectedStdlibSnapshot],
) -> Vec<String> {
    const MAX_RESTORE_PASSES: usize = 16;
    let mut errors = Vec::new();
    for _ in 0..MAX_RESTORE_PASSES {
        let mut displaced = Vec::new();
        for snapshot in snapshots {
            let actual = snapshot.module.bind(py).dict();
            let current_entries = actual
                .iter()
                .map(|(key, value)| (key.unbind(), value.unbind()))
                .collect::<Vec<_>>();
            actual.clear();
            if let Err(error) = actual.update(snapshot.dictionary.bind(py).as_mapping()) {
                errors.push(format!(
                    "restoring {} dictionary: {error}",
                    snapshot.module_name
                ));
            }
            for (key, value) in current_entries {
                displaced.push(key);
                displaced.push(value);
            }
        }
        drop(displaced);
        if protected_stdlib_snapshots_match(py, snapshots) {
            return errors;
        }
    }
    errors.push(format!(
        "protected stdlib snapshot restoration did not stabilize after {MAX_RESTORE_PASSES} passes"
    ));
    errors
}

fn capture_protected_stdlib_snapshots(
    py: pyo3::Python<'_>,
) -> pyo3::PyResult<Vec<ProtectedStdlibSnapshot>> {
    let mut snapshots = Vec::new();
    for &(module_name, anchor_name) in PROTECTED_STDLIB_SNAPSHOT_SPECS {
        let module = py.import(module_name)?;
        snapshots.push(ProtectedStdlibSnapshot {
            module_name,
            anchor_name,
            dictionary: module.dict().copy()?.unbind(),
            anchor: module.getattr(anchor_name)?.unbind(),
            module: module.unbind(),
        });
    }

    let annotationlib = match py.import("annotationlib") {
        Ok(module) => Some(module),
        Err(error) => {
            let missing = error.is_instance_of::<PyModuleNotFoundError>(py)
                && error
                    .value(py)
                    .getattr("name")
                    .and_then(|name| name.extract::<String>())
                    .is_ok_and(|name| name == "annotationlib");
            if missing {
                None
            } else {
                return Err(error);
            }
        }
    };
    if let Some(module) = annotationlib {
        snapshots.push(ProtectedStdlibSnapshot {
            module_name: "annotationlib",
            anchor_name: "get_annotations",
            dictionary: module.dict().copy()?.unbind(),
            anchor: module.getattr("get_annotations")?.unbind(),
            module: module.unbind(),
        });
    }
    Ok(snapshots)
}

fn assert_same_dict_identities(
    actual: &pyo3::Bound<'_, PyDict>,
    baseline: &pyo3::Bound<'_, PyDict>,
    module_name: &str,
) {
    assert_eq!(
        actual.len(),
        baseline.len(),
        "protected module dictionary key set changed: {module_name}"
    );
    for ((actual_key, actual_value), (expected_key, expected_value)) in
        actual.iter().zip(baseline.iter())
    {
        assert!(
            actual_key.is(&expected_key),
            "protected module dictionary key identity changed: {module_name}.{expected_key}"
        );
        assert!(
            actual_value.is(&expected_value),
            "protected module dictionary value identity changed for {module_name}.{expected_key}"
        );
    }
}

fn assert_protected_stdlib_restored(
    py: pyo3::Python<'_>,
    snapshots: &[ProtectedStdlibSnapshot],
) -> pyo3::PyResult<()> {
    for snapshot in snapshots {
        let module = snapshot.module.bind(py);
        let dictionary = module.dict();
        assert_same_dict_identities(
            &dictionary,
            snapshot.dictionary.bind(py),
            snapshot.module_name,
        );
        let actual_anchor = dictionary
            .get_item(snapshot.anchor_name)?
            .unwrap_or_else(|| {
                panic!(
                    "protected module anchor disappeared: {}.{}",
                    snapshot.module_name, snapshot.anchor_name
                )
            });
        assert!(
            actual_anchor.is(snapshot.anchor.bind(py)),
            "protected module anchor identity changed: {}.{}",
            snapshot.module_name,
            snapshot.anchor_name
        );
    }
    Ok(())
}

#[test]
fn best_effort_dependency_discovery_reports_each_origin_and_summary() {
    let _test_guard = dependency_test_guard();
    let generated = generated(false)
        .unwrap()
        .generate_with_report()
        .expect("best-effort dependency discovery should succeed");

    let attempted = generated
        .report
        .diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic.code == "discovery_dependency_attempted"
                && diagnostic.severity == DiagnosticSeverity::Info
        })
        .map(|diagnostic| diagnostic.path.as_deref().unwrap().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        attempted,
        vec![
            "json.decoder.FinalItem",
            "json.decoder.not_a_module.Truncated",
            "pyo3_bindgen_missing_dependency_root.MissingRoot",
        ],
        "requested dependency origins must be sorted and unique"
    );

    let outcomes = generated
        .report
        .diagnostics
        .iter()
        .filter(|diagnostic| {
            matches!(
                diagnostic.code.as_str(),
                "discovery_dependency_emitted"
                    | "discovery_dependency_skipped"
                    | "discovery_dependency_failed"
                    | "discovery_dependency_truncated"
            )
        })
        .map(|diagnostic| {
            (
                diagnostic.path.as_deref().unwrap().to_owned(),
                diagnostic.code.as_str(),
                diagnostic.severity,
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        outcomes,
        vec![
            (
                "json.decoder.FinalItem".to_owned(),
                "discovery_dependency_emitted",
                DiagnosticSeverity::Info,
            ),
            (
                "json.decoder.not_a_module.Truncated".to_owned(),
                "discovery_dependency_truncated",
                DiagnosticSeverity::Warning,
            ),
            (
                "pyo3_bindgen_missing_dependency_root.MissingRoot".to_owned(),
                "discovery_dependency_failed",
                DiagnosticSeverity::Warning,
            ),
        ],
        "each dependency origin needs one final outcome"
    );

    let dependency_summary = generated
        .report
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.code == "discovery_dependency_summary")
        .expect("dependency summary must be present");
    assert_eq!(
        dependency_summary.message,
        "attempted=3;emitted=1;skipped=2;failed=1;truncated=1"
    );
}

#[test]
fn strict_completeness_fails_before_writing_output() {
    let _test_guard = dependency_test_guard();
    let output = fresh_output_path();
    assert!(
        !output.path().exists(),
        "the owned strict-completeness output child must start absent"
    );

    let error = generated(true)
        .unwrap()
        .build(output.path())
        .expect_err("strict completeness must reject incomplete dependency discovery");

    match error {
        pyo3_bindgen_engine::PyBindgenError::CodegenError(detail) => assert!(
            detail.starts_with("strict completeness failed:"),
            "unexpected strict completeness error: {detail}"
        ),
        error => panic!("unexpected strict completeness error: {error}"),
    }
    assert!(
        !output.path().exists(),
        "strict completeness must fail before the public build writes output"
    );
}

#[test]
fn duplicate_origins_parse_their_containing_module_once() {
    let _test_guard = dependency_test_guard();
    let package = TempDependencyPackage::new();
    let source = format!(
        "class One:\n    pass\nOne.__module__ = {name:?}\n\nclass Two:\n    pass\nTwo.__module__ = {name:?}\n",
        name = package.name,
    );
    let generated = Codegen::new(dependency_config(false))
        .module_from_str(&source, "pyo3_bindgen_duplicate_dependency_target")
        .unwrap()
        .generate_with_report()
        .unwrap();

    let child = format!("{}.child", package.name);
    for code in [
        "discovery_submodule_attempted",
        "discovery_submodule_emitted",
    ] {
        assert_eq!(
            generated
                .report
                .diagnostics
                .iter()
                .filter(|diagnostic| {
                    diagnostic.code == code && diagnostic.path.as_deref() == Some(child.as_str())
                })
                .count(),
            1,
            "duplicate dependency origins must not duplicate {code} for {child}"
        );
    }
    assert_eq!(
        generated
            .report
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.code == "discovery_dependency_emitted")
            .count(),
        2,
        "each original dependency origin still needs its own final outcome"
    );
}

#[test]
fn dependency_fixture_cleanup_is_scoped_to_its_path_and_module_prefix() {
    let _test_guard = dependency_test_guard();
    let package = TempDependencyPackage::new();
    let package_name = package.name.clone();
    let child_name = format!("{package_name}.child");
    let unowned_name = format!("{package_name}_sibling");
    let root = package.root.clone();
    pyo3::Python::attach(|py| -> pyo3::PyResult<()> {
        let sys = py.import("sys")?;
        py.import(package_name.as_str())?;
        py.import(child_name.as_str())?;
        sys.getattr("modules")?
            .set_item(unowned_name.as_str(), py.None())?;
        Ok(())
    })
    .unwrap();

    drop(package);

    let state = pyo3::Python::attach(|py| -> pyo3::PyResult<_> {
        let sys = py.import("sys")?;
        let root = root.to_string_lossy().into_owned();
        let path_count = sys
            .getattr("path")?
            .call_method1("count", (root.as_str(),))?
            .extract::<usize>()?;
        let modules = sys.getattr("modules")?;
        let package_present = modules
            .call_method1("__contains__", (package_name.as_str(),))?
            .extract::<bool>()?;
        let child_present = modules
            .call_method1("__contains__", (child_name.as_str(),))?
            .extract::<bool>()?;
        let unowned_present = modules
            .call_method1("__contains__", (unowned_name.as_str(),))?
            .extract::<bool>()?;
        modules.del_item(unowned_name.as_str())?;
        Ok((path_count, package_present, child_present, unowned_present))
    })
    .unwrap();

    assert_eq!(
        state.0, 0,
        "the fixture must remove exactly its sys.path entry"
    );
    assert!(!state.1, "the fixture package must leave sys.modules");
    assert!(!state.2, "the fixture child must leave sys.modules");
    assert!(
        state.3,
        "a similarly named unowned sys.modules entry must remain"
    );
    assert!(!root.exists(), "the owned fixture root must be removed");
}

#[test]
fn private_alias_to_private_module_is_skipped_by_default() {
    let _test_guard = dependency_test_guard();
    let package = PrivateAliasDependencyPackage::new();
    let public_name = format!("{}.public", package.name);
    let private_name = format!("{}._private", package.name);
    let source = format!(
        "class Imported:\n    __module__ = {public_name:?}\n",
        public_name = public_name,
    );
    let generated = Codegen::new(Config::builder().generate_dependencies(true).build())
        .module_from_str(&source, &format!("{}_target", package.name))
        .unwrap()
        .generate_with_report()
        .expect("best-effort dependency discovery should skip private aliases");

    assert!(
        generated.report.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "discovery_dependency_skipped"
                && diagnostic.path.as_deref() == Some(format!("{public_name}.Imported").as_str())
        }),
        "diagnostics: {:?}",
        generated.report.diagnostics
    );
    assert!(
        !generated
            .report
            .modules
            .iter()
            .any(|module| module.name == private_name),
        "private dependency module must not be emitted: {private_name}"
    );
}

#[test]
fn private_alias_to_private_module_is_allowed_with_include_private() {
    let _test_guard = dependency_test_guard();
    let package = PrivateAliasDependencyPackage::new();
    let public_name = format!("{}.public", package.name);
    let private_name = format!("{}._private", package.name);
    let source = format!(
        "class Imported:\n    __module__ = {public_name:?}\n",
        public_name = public_name,
    );
    let generated = Codegen::new(
        Config::builder()
            .generate_dependencies(true)
            .include_private(true)
            .build(),
    )
    .module_from_str(&source, &format!("{}_target", package.name))
    .unwrap()
    .generate_with_report()
    .expect("include_private should allow private dependency aliases");

    assert!(generated.report.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "discovery_dependency_emitted"
            && diagnostic.path.as_deref() == Some(format!("{public_name}.Imported").as_str())
    }));
    assert!(
        generated
            .report
            .modules
            .iter()
            .any(|module| module.name == private_name),
        "include_private should emit the resolved private module: {private_name}"
    );
}

#[test]
fn malformed_resolved_module_name_is_skipped_without_panicking() {
    let _test_guard = dependency_test_guard();
    let package = PrivateAliasDependencyPackage::new_malformed_name();
    let public_name = format!("{}.public", package.name);
    let malformed_name = format!("{}::malformed", package.name);
    let source = format!(
        "class Imported:\n    __module__ = {public_name:?}\n",
        public_name = public_name,
    );
    let generated = Codegen::new(Config::builder().generate_dependencies(true).build())
        .module_from_str(&source, &format!("{}_target", package.name))
        .unwrap()
        .generate_with_report()
        .expect("malformed resolved module names should be skipped best-effort");

    let diagnostic = generated
        .report
        .diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic.code == "discovery_dependency_skipped"
                && diagnostic.path.as_deref() == Some(format!("{public_name}.Imported").as_str())
        })
        .expect("malformed resolved module must produce a skipped diagnostic");
    assert!(
        diagnostic.message.contains("invalid module name"),
        "unexpected malformed-name diagnostic: {}",
        diagnostic.message
    );
    assert!(
        !generated
            .report
            .modules
            .iter()
            .any(|module| module.name == malformed_name),
        "malformed resolved module must not be emitted: {malformed_name}"
    );
}

#[test]
fn dependency_discovery_restores_hostile_stdlib_mutations() {
    let _test_guard = dependency_test_guard();
    let package = HostileDependencyPackage::new();
    let snapshots = pyo3::Python::attach(|py| -> Result<_, pyo3_bindgen_engine::PyBindgenError> {
        Codegen::prime_live_runtime(py)?;
        Ok(capture_protected_stdlib_snapshots(py)?)
    })
    .unwrap();

    let source = format!(
        "class Imported:\n    __module__ = {name:?}\n",
        name = package.name,
    );
    let generated = Codegen::new(dependency_config(false))
        .module_from_str(&source, "pyo3_bindgen_hostile_dependency_target")
        .unwrap()
        .generate_with_report()
        .expect("hostile dependency discovery should still generate");
    assert!(generated.report.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "discovery_dependency_emitted"
            && diagnostic.path.as_deref() == Some(format!("{}.Imported", package.name).as_str())
    }));

    pyo3::Python::attach(|py| assert_protected_stdlib_restored(py, &snapshots)).unwrap();
}

#[test]
fn dependency_discovery_restores_hostile_stdlib_mutations_after_parse_failure() {
    let _test_guard = dependency_test_guard();
    let package = HostileDependencyPackage::new_parse_failure();
    let snapshots = pyo3::Python::attach(|py| -> Result<_, pyo3_bindgen_engine::PyBindgenError> {
        Codegen::prime_live_runtime(py)?;
        Ok(capture_protected_stdlib_snapshots(py)?)
    })
    .unwrap();

    let source = format!(
        "class Imported:\n    __module__ = {name:?}\n",
        name = package.name,
    );
    let generated = Codegen::new(dependency_config(false))
        .module_from_str(
            &source,
            "pyo3_bindgen_hostile_dependency_parse_failure_target",
        )
        .unwrap()
        .generate_with_report()
        .expect("dependency parse failure should remain best-effort");
    let failure_path = format!("{}.Imported", package.name);
    let diagnostic = generated
        .report
        .diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic.code == "discovery_dependency_failed"
                && diagnostic.path.as_deref() == Some(failure_path.as_str())
        })
        .expect("dependency parse failure must be reported");
    assert!(
        diagnostic
            .message
            .contains("Could not parse dependency module"),
        "unexpected dependency parse diagnostic: {}",
        diagnostic.message
    );

    pyo3::Python::attach(|py| assert_protected_stdlib_restored(py, &snapshots)).unwrap();
}

#[test]
fn dependency_discovery_reports_cleanup_failure_after_restoring_hostile_stdlib_state() {
    let _test_guard = dependency_test_guard();
    pyo3::Python::initialize();
    let (original_sys_class, original_stdout, original_stderr) =
        pyo3::Python::attach(|py| -> pyo3::PyResult<_> {
            let sys = py.import("sys")?;
            Ok((
                sys.getattr("__class__")?.unbind(),
                sys.getattr("stdout")?.unbind(),
                sys.getattr("stderr")?.unbind(),
            ))
        })
        .unwrap();
    let package = HostileDependencyPackage::new_cleanup_failure();
    let snapshots = pyo3::Python::attach(|py| -> Result<_, pyo3_bindgen_engine::PyBindgenError> {
        Codegen::prime_live_runtime(py)?;
        Ok(capture_protected_stdlib_snapshots(py)?)
    })
    .unwrap();

    let source = format!(
        "class Imported:\n    __module__ = {name:?}\n",
        name = package.name,
    );
    let error = Codegen::new(dependency_config(false))
        .module_from_str(
            &source,
            "pyo3_bindgen_hostile_dependency_cleanup_failure_target",
        )
        .unwrap()
        .generate_with_report()
        .expect_err("dependency cleanup failure must reach the public Codegen result");
    match &error {
        pyo3_bindgen_engine::PyBindgenError::PyError(error) => assert!(
            error
                .to_string()
                .contains("hostile sys.__setattr__ failed for stdout"),
            "unexpected public cleanup error: {error}"
        ),
        error => panic!("unexpected public cleanup error variant: {error}"),
    }

    pyo3::Python::attach(|py| -> pyo3::PyResult<()> {
        assert_protected_stdlib_restored(py, &snapshots)?;
        let sys = py.import("sys")?;
        assert!(
            !sys.getattr("__class__")?.is(original_sys_class.bind(py)),
            "the hostile sys class must still expose the failed cleanup before test RAII"
        );
        assert!(
            !sys.getattr("stdout")?.is(original_stdout.bind(py)),
            "stdout must remain unrecovered until the test RAII guard runs"
        );
        assert!(
            sys.getattr("stderr")?.is(original_stderr.bind(py)),
            "the other stream must be restored after stdout cleanup fails"
        );
        Ok(())
    })
    .unwrap();

    drop(package);
    pyo3::Python::attach(|py| -> pyo3::PyResult<()> {
        let sys = py.import("sys")?;
        assert!(
            sys.getattr("__class__")?.is(original_sys_class.bind(py)),
            "test RAII must restore sys.__class__"
        );
        assert!(
            sys.getattr("stdout")?.is(original_stdout.bind(py)),
            "test RAII must restore sys.stdout"
        );
        assert!(
            sys.getattr("stderr")?.is(original_stderr.bind(py)),
            "test RAII must restore sys.stderr"
        );
        Ok(())
    })
    .unwrap();
}

#[test]
fn hostile_dependency_fixture_restores_snapshots_during_unwind() {
    let _test_guard = dependency_test_guard();
    pyo3::Python::initialize();
    let (original_sys_class, original_stdout, original_stderr, snapshots) =
        pyo3::Python::attach(|py| -> Result<_, pyo3_bindgen_engine::PyBindgenError> {
            Codegen::prime_live_runtime(py)?;
            let sys = py.import("sys")?;
            Ok((
                sys.getattr("__class__")?.unbind(),
                sys.getattr("stdout")?.unbind(),
                sys.getattr("stderr")?.unbind(),
                capture_protected_stdlib_snapshots(py)?,
            ))
        })
        .unwrap();
    let mut package_name = None;
    let mut package_root = None;

    let unwind = catch_unwind(AssertUnwindSafe(|| {
        let package = HostileDependencyPackage::new_cleanup_failure();
        package_name = Some(package.name.clone());
        package_root = Some(package.root.clone());
        let unowned_name = format!("{}_sibling", package.name);
        let source = format!(
            "class Imported:\n    __module__ = {name:?}\n",
            name = package.name,
        );
        let error = Codegen::new(dependency_config(false))
            .module_from_str(&source, "pyo3_bindgen_hostile_dependency_unwind_target")
            .unwrap()
            .generate_with_report()
            .expect_err("dependency cleanup failure must reach the public Codegen result");
        match error {
            pyo3_bindgen_engine::PyBindgenError::PyError(error) => assert!(
                error
                    .to_string()
                    .contains("hostile sys.__setattr__ failed for stdout"),
                "unexpected public cleanup error: {error}"
            ),
            error => panic!("unexpected public cleanup error variant: {error}"),
        }

        // These assertions must observe production restoration before the fixture's Drop
        // fallback runs during the intentional unwind below.
        pyo3::Python::attach(|py| -> pyo3::PyResult<()> {
            assert_protected_stdlib_restored(py, &snapshots)?;
            let sys = py.import("sys")?;
            assert!(
                !sys.getattr("__class__")?.is(original_sys_class.bind(py)),
                "the hostile sys class must still expose the failed cleanup before fallback"
            );
            assert!(
                !sys.getattr("stdout")?.is(original_stdout.bind(py)),
                "stdout must remain unrecovered before fallback"
            );
            assert!(
                sys.getattr("stderr")?.is(original_stderr.bind(py)),
                "stderr must be restored after stdout cleanup fails"
            );
            sys.getattr("modules")?
                .set_item(unowned_name.as_str(), py.None())?;
            let inspect = py.import("inspect")?;
            inspect.setattr("signature", py.None())?;
            inspect.setattr("_pyo3_bindgen_unwind_marker", py.None())?;
            Ok(())
        })
        .unwrap();
        panic!("intentional fixture cleanup unwind");
    }));
    assert!(unwind.is_err(), "the regression operation must panic");

    let package_name = package_name.expect("fixture must be allocated before the unwind");
    let package_root = package_root.expect("fixture root must be recorded before the unwind");
    let unowned_name = format!("{package_name}_sibling");
    let state = pyo3::Python::attach(|py| -> pyo3::PyResult<_> {
        let sys = py.import("sys")?;
        let package_root = package_root.to_string_lossy().into_owned();
        let path_count = sys
            .getattr("path")?
            .call_method1("count", (package_root.as_str(),))?
            .extract::<usize>()?;
        let modules = sys.getattr("modules")?;
        let package_present = modules
            .call_method1("__contains__", (package_name.as_str(),))?
            .extract::<bool>()?;
        let unowned_present = modules
            .call_method1("__contains__", (unowned_name.as_str(),))?
            .extract::<bool>()?;
        modules.del_item(unowned_name.as_str())?;
        Ok((
            path_count,
            package_present,
            unowned_present,
            sys.getattr("__class__")?.is(original_sys_class.bind(py)),
            sys.getattr("stdout")?.is(original_stdout.bind(py)),
            sys.getattr("stderr")?.is(original_stderr.bind(py)),
        ))
    })
    .unwrap();
    pyo3::Python::attach(|py| assert_protected_stdlib_restored(py, &snapshots)).unwrap();

    assert_eq!(
        state.0, 0,
        "unwind cleanup must remove the owned sys.path entry"
    );
    assert!(
        !state.1,
        "unwind cleanup must remove the owned package module"
    );
    assert!(state.2, "unwind cleanup must preserve an unowned module");
    assert!(state.3, "unwind cleanup must restore sys.__class__");
    assert!(state.4, "unwind cleanup must restore sys.stdout");
    assert!(state.5, "unwind cleanup must restore sys.stderr");
    assert!(
        !package_root.exists(),
        "unwind cleanup must remove the owned fixture root"
    );
}
