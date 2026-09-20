use pyo3::types::PyAnyMethods;
use pyo3_bindgen_engine::{Codegen, Config, DiagnosticSeverity};
use std::{
    cell::Cell,
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

static TEMP_PACKAGE_COUNTER: AtomicU64 = AtomicU64::new(0);

struct TempPackage {
    name: String,
    root: PathBuf,
    path_registered: Cell<bool>,
}

impl TempPackage {
    fn import_marker() -> Self {
        Self::with_files(
            "import_marker",
            &[
                ("__init__.py", ""),
                (
                    "test.py",
                    "with open(__file__[:-3] + '.marker', 'w', encoding='utf-8') as marker:\n    marker.write('imported')\n\nvalue = 42\n",
                ),
            ],
        )
    }

    fn import_marker_path(&self) -> PathBuf {
        self.root.join(&self.name).join("test.marker")
    }

    fn incomplete() -> Self {
        Self::with_files(
            "incomplete",
            &[
                ("__init__.py", ""),
                (
                    "good.py",
                    "def ready(value: int) -> int:\n    return value\n",
                ),
                (
                    "broken.py",
                    "raise RuntimeError('intentional discovery failure')\n",
                ),
                ("test.py", "def excluded():\n    return True\n"),
            ],
        )
    }

    fn shadowed() -> Self {
        Self::with_files(
            "shadowed",
            &[
                (
                    "__init__.py",
                    "from .shadowed import Widget as shadowed\nfrom .failed_source import Widget as failed\n",
                ),
                (
                    "shadowed.py",
                    "class Widget:\n    pass\n\nclass Other:\n    pass\n",
                ),
                (
                    "failed.py",
                    "raise RuntimeError('intentional shadowed recovery failure')\n",
                ),
                ("failed_source.py", "class Widget:\n    pass\n"),
            ],
        )
    }

    fn with_files(label: &str, files: &[(&str, &str)]) -> Self {
        let fixture = loop {
            let id = TEMP_PACKAGE_COUNTER.fetch_add(1, Ordering::Relaxed);
            let name = format!("pyo3_bindgen_discovery_fixture_{label}_{id}");
            let root = std::env::temp_dir().join(format!("{name}_{}", std::process::id()));
            match fs::create_dir(&root) {
                Ok(()) => {
                    break Self {
                        name,
                        root,
                        path_registered: Cell::new(false),
                    };
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("cannot allocate exclusive Python fixture: {error}"),
            }
        };
        let package = fixture.root.join(&fixture.name);
        fs::create_dir(&package).unwrap();
        for (file_name, contents) in files {
            fs::write(package.join(file_name), contents).unwrap();
        }

        fixture
    }

    fn add_to_sys_path(&self) {
        pyo3::Python::initialize();
        pyo3::Python::attach(|py| -> pyo3::PyResult<()> {
            let sys = py.import("sys")?;
            sys.getattr("path")?
                .call_method1("insert", (0, self.root.to_string_lossy().into_owned()))?;
            Ok(())
        })
        .unwrap();
        self.path_registered.set(true);
    }
}

impl Drop for TempPackage {
    fn drop(&mut self) {
        if self.path_registered.get() {
            let result = pyo3::Python::attach(|py| -> pyo3::PyResult<()> {
                let sys = py.import("sys")?;
                sys.getattr("path")?
                    .call_method1("remove", (self.root.to_string_lossy().into_owned(),))?;
                let modules = sys.getattr("modules")?;
                let prefix = format!("{}.", self.name);
                for key in modules
                    .call_method0("copy")?
                    .call_method0("keys")?
                    .try_iter()?
                {
                    let key = key?;
                    if let Ok(name) = key.extract::<String>() {
                        if name == self.name || name.starts_with(&prefix) {
                            modules.del_item(key)?;
                        }
                    }
                }
                Ok(())
            });
            if let Err(error) = result {
                eprintln!(
                    "Python fixture import cleanup failed for {}: {error}",
                    self.name
                );
            }
        }
        if let Err(error) = fs::remove_dir_all(&self.root) {
            if error.kind() != std::io::ErrorKind::NotFound {
                eprintln!("Python fixture directory cleanup failed: {error}");
            }
        }
    }
}

#[test]
fn discovered_good_and_broken_submodules_are_not_silent() {
    let package = TempPackage::incomplete();
    package.add_to_sys_path();

    let generated = Codegen::default()
        .module_name(&package.name)
        .unwrap()
        .generate_with_report()
        .unwrap();

    let good_path = format!("{}.good", package.name);
    let broken_path = format!("{}.broken", package.name);
    let skipped_path = format!("{}.test", package.name);

    assert!(
        generated
            .report
            .modules
            .iter()
            .any(|module| module.name == good_path),
        "the importable discovered child must still be emitted"
    );
    assert!(!generated
        .report
        .modules
        .iter()
        .any(|module| module.name == broken_path));

    let attempted_paths = generated
        .report
        .diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic.code == "discovery_submodule_attempted"
                && diagnostic.severity == DiagnosticSeverity::Info
        })
        .map(|diagnostic| diagnostic.path.as_deref().unwrap().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        attempted_paths,
        vec![broken_path.clone(), good_path.clone(), skipped_path.clone()],
        "every sorted discovered origin needs an observable attempt"
    );

    let outcomes = generated
        .report
        .diagnostics
        .iter()
        .filter(|diagnostic| {
            matches!(
                diagnostic.code.as_str(),
                "discovery_submodule_emitted"
                    | "discovery_submodule_skipped"
                    | "discovery_submodule_failed"
                    | "discovery_submodule_truncated"
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
                broken_path.clone(),
                "discovery_submodule_failed",
                DiagnosticSeverity::Warning,
            ),
            (
                good_path,
                "discovery_submodule_emitted",
                DiagnosticSeverity::Info,
            ),
            (
                skipped_path,
                "discovery_submodule_skipped",
                DiagnosticSeverity::Warning,
            ),
        ],
        "each attempted origin needs exactly one final outcome: {outcomes:?}"
    );

    let broken_diagnostic = generated
        .report
        .diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic.code == "discovery_submodule_failed"
                && diagnostic.path.as_deref() == Some(broken_path.as_str())
        })
        .unwrap();
    assert!(
        broken_diagnostic
            .message
            .contains("intentional discovery failure"),
        "the failure must retain the import exception details: {broken_diagnostic:?}"
    );

    let summary = generated
        .report
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.code == "discovery_submodule_summary")
        .expect("submodule discovery must publish deterministic totals");
    assert_eq!(
        summary.message,
        "attempted=3;emitted=1;skipped=2;failed=1;truncated=0"
    );
}

#[test]
fn excluded_discovered_submodule_code_is_not_imported() {
    let package = TempPackage::import_marker();
    let marker = package.import_marker_path();
    package.add_to_sys_path();
    assert!(
        !marker.exists(),
        "the fixture must not execute during setup"
    );

    let codegen = Codegen::default()
        .module_name(&package.name)
        .expect("the package should remain importable");
    assert!(
        !marker.exists(),
        "generator-initiated discovery must filter an excluded child before importing it"
    );

    let generated = codegen
        .generate_with_report()
        .expect("excluding the child should not fail generation");
    assert!(
        !marker.exists(),
        "later generation phases must not import the excluded child"
    );

    let child_path = format!("{}.test", package.name);
    let skipped = generated
        .report
        .diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic.code == "discovery_submodule_skipped"
                && diagnostic.path.as_deref() == Some(child_path.as_str())
        })
        .expect("the excluded child must have a path-qualified skip diagnostic");
    assert_eq!(skipped.severity, DiagnosticSeverity::Warning);
    assert!(
        !generated
            .report
            .modules
            .iter()
            .any(|module| module.name == child_path),
        "the excluded child must not appear in the generated module report"
    );
}

#[test]
fn explicitly_included_discovered_submodule_code_is_imported() {
    let package = TempPackage::import_marker();
    let marker = package.import_marker_path();
    package.add_to_sys_path();

    let codegen = Codegen::new(
        Config::builder()
            .blocklist_names(Vec::<String>::new())
            .build(),
    )
    .module_name(&package.name)
    .expect("the package should remain importable");
    assert!(
        marker.exists(),
        "explicitly removing the blocklist must allow generator-initiated import"
    );

    let generated = codegen
        .generate_with_report()
        .expect("the explicitly included child should generate");
    let child_path = format!("{}.test", package.name);
    assert!(generated
        .report
        .modules
        .iter()
        .any(|module| module.name == child_path));
    assert!(generated.report.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "discovery_submodule_emitted"
            && diagnostic.path.as_deref() == Some(child_path.as_str())
    }));
    assert!(!generated.report.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "discovery_submodule_skipped"
            && diagnostic.path.as_deref() == Some(child_path.as_str())
    }));
}

#[test]
fn shadowed_submodule_recovery_reports_truncation_or_failure() {
    let package = TempPackage::shadowed();
    package.add_to_sys_path();

    let generated = Codegen::default()
        .module_name(&package.name)
        .unwrap()
        .generate_with_report()
        .unwrap();

    let partial_path = format!("{}.shadowed", package.name);
    let failed_path = format!("{}.failed", package.name);
    let diagnostics = &generated.report.diagnostics;

    for path in [&partial_path, &failed_path] {
        assert_eq!(
            diagnostics
                .iter()
                .filter(|diagnostic| {
                    diagnostic.code == "discovery_submodule_attempted"
                        && diagnostic.path.as_deref() == Some(path.as_str())
                })
                .count(),
            1,
            "shadowed origin must have one attempt: {path}"
        );
    }
    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "submodule_shadowed_by_import"
            && diagnostic.path.as_deref() == Some(partial_path.as_str())
    }));
    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "submodule_shadowed_by_import"
            && diagnostic.path.as_deref() == Some(failed_path.as_str())
    }));

    let partial_outcomes = diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic.path.as_deref() == Some(partial_path.as_str())
                && diagnostic.code.starts_with("discovery_submodule_")
                && diagnostic.code != "discovery_submodule_attempted"
        })
        .collect::<Vec<_>>();
    assert_eq!(partial_outcomes.len(), 1);
    assert_eq!(partial_outcomes[0].code, "discovery_submodule_truncated");
    assert_eq!(partial_outcomes[0].severity, DiagnosticSeverity::Warning);

    let failed_outcomes = diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic.path.as_deref() == Some(failed_path.as_str())
                && diagnostic.code.starts_with("discovery_submodule_")
                && diagnostic.code != "discovery_submodule_attempted"
        })
        .collect::<Vec<_>>();
    assert_eq!(failed_outcomes.len(), 1);
    assert_eq!(failed_outcomes[0].code, "discovery_submodule_failed");
    assert_eq!(failed_outcomes[0].severity, DiagnosticSeverity::Warning);
    assert!(failed_outcomes[0]
        .message
        .contains("intentional shadowed recovery failure"));
}
