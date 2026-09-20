//! Private ownership boundary for live-Python operations.
//!
//! A [`LiveSession`] is the only production context that performs engine-owned Python
//! acquisition outside the syntax parsers. It owns attachment and initialization, runtime
//! priming, imports, source execution, dynamic dependency access, parent construction, and
//! emission-time host facts. [`LiveSession::finish`] is the explicit completion boundary: normal
//! returns finish both process-global stream restoration and stdlib-sentinel restoration, while
//! the lower-level guards retain their unwind fallback.
//!
//! Session contract: same-thread output coordination is reentrant, while same-thread first-use
//! sentinel initialization is rejected before entering `PyOnceLock`; first use also validates
//! stable stdlib anchor provenance before accepting a restoration baseline. A different thread waits for the process-wide coordination permit
//! for at most the guard's 300-second bound, then receives an error. The session and its permit
//! are thread-owned (`!Send`), so Python access and cleanup stay on the acquiring thread. Normal
//! completion explicitly restores streams and sentinel dictionaries; callback, setup, and
//! cleanup errors propagate (cleanup errors take precedence or are aggregated), and `Drop` is
//! retained only as the best-effort partial-setup or panic/unwind fallback.

use crate::{
    planning::{ImportPrefixes, Path},
    report::{
        DiagnosticCode, DiscoveryOutcome, DiscoverySubject, DomainDiagnostic, PythonMetadata,
    },
    syntax::{Ident, Module},
    Config, PyBindgenError, Result,
};
use pyo3::prelude::*;
use rustc_hash::FxHashMap as HashMap;

#[derive(Debug, Clone, Copy)]
pub(crate) struct LivePolicy {
    suppress_stdout: bool,
    suppress_stderr: bool,
}

impl LivePolicy {
    pub(crate) const fn from_config(config: &Config) -> Self {
        Self {
            suppress_stdout: config.suppress_python_stdout,
            suppress_stderr: config.suppress_python_stderr,
        }
    }

    const fn unsuppressed() -> Self {
        Self {
            suppress_stdout: false,
            suppress_stderr: false,
        }
    }
}

/// Pure domain values acquired for the remaining live generation phases.
pub(crate) struct LiveGenerationContext {
    parent_modules: Vec<Module>,
    import_prefixes: ImportPrefixes,
}

impl LiveGenerationContext {
    pub(crate) fn into_parts(self) -> (Vec<Module>, ImportPrefixes) {
        (self.parent_modules, self.import_prefixes)
    }
}

/// Parsed dependency-domain values and typed discovery facts. No Python object can escape it.
pub(crate) struct LiveDependencyOutput {
    pub(crate) modules: Vec<Module>,
    pub(crate) diagnostics: Vec<DomainDiagnostic>,
}

enum DependencyResolution<'py> {
    Resolved(ResolvedDependency<'py>),
    Failed(String),
    Skipped(String),
}

struct ResolvedDependency<'py> {
    module: Bound<'py, pyo3::types::PyModule>,
    module_name: String,
    truncation: Option<String>,
}

fn dependency_outcome(
    origin: &str,
    outcome: DiscoveryOutcome,
    message: impl Into<String>,
) -> DomainDiagnostic {
    let code = DiagnosticCode::discovery(DiscoverySubject::Dependency, outcome);
    if outcome == DiscoveryOutcome::Emitted {
        DomainDiagnostic::info(code, origin, message)
    } else {
        DomainDiagnostic::warning(code, origin, message)
    }
}

/// Typed private context for one attached, primed, and cleanup-scoped live operation.
pub(crate) struct LiveSession<'py> {
    py: Python<'py>,
    policy: LivePolicy,
    _thread_bound: std::marker::PhantomData<std::rc::Rc<()>>,
}

impl<'py> LiveSession<'py> {
    fn new(py: Python<'py>, policy: LivePolicy) -> Self {
        Self {
            py,
            policy,
            _thread_bound: std::marker::PhantomData,
        }
    }

    #[cfg(test)]
    fn python(&self) -> Python<'py> {
        self.py
    }

    /// Complete one live operation under output suppression and stdlib restoration.
    ///
    /// The callback is intentionally the only way callers receive a session. The callback's
    /// Python objects cannot escape this owned completion boundary unless the caller explicitly
    /// stores them in a result type tied to the session lifetime, which the production adapters
    /// do not do.
    fn finish<T>(
        &self,
        prepare: impl FnOnce(&Self) -> Result<()>,
        operation: impl FnOnce(&Self) -> Result<T>,
    ) -> Result<T> {
        crate::io_utils::with_suppressed_python_output(
            self.py,
            self.policy.suppress_stdout,
            self.policy.suppress_stderr,
            || {
                prepare(self)?;
                crate::runtime_sentinels::with_restored(self.py, || operation(self))
            },
        )
    }

    fn prime_runtime(&self) -> Result<()> {
        crate::runtime_sentinels::prime(self.py).map_err(Into::into)
    }

    fn require_primed(&self) -> Result<()> {
        if crate::runtime_sentinels::is_primed(self.py) {
            Ok(())
        } else {
            Err(PyBindgenError::CodegenError(
                "preloaded live modules require Codegen::prime_live_runtime(py) before target import or execution"
                    .to_owned(),
            ))
        }
    }

    fn import_module_value(&self, path: &Path) -> Result<Bound<'py, pyo3::types::PyModule>> {
        Ok(self.py.import(path.to_py().as_str())?)
    }

    /// Import and parse a module without exposing the live Python object to the caller.
    pub(crate) fn import_module(&self, config: &Config, path: &Path) -> Result<Module> {
        let module = self.import_module_value(path)?;
        self.inspect_module(config, &module, None)
    }

    /// Execute and parse source as a module without exposing the live Python object.
    pub(crate) fn execute_module(
        &self,
        config: &Config,
        source_code: &str,
        module_path: &Path,
    ) -> Result<Module> {
        let canonical_name = module_path.to_py();
        let source_code = python_c_string(source_code, "source")?;
        let module_name = python_c_string(canonical_name.as_str(), "module name")?;
        let file_name =
            python_c_string(format!("{canonical_name}/__init__.py"), "module filename")?;
        let module = pyo3::types::PyModule::from_code(
            self.py,
            source_code.as_c_str(),
            file_name.as_c_str(),
            module_name.as_c_str(),
        )?;
        self.inspect_module(config, &module, Some(module_path))
    }

    /// Parse a caller-created module into a Python-free domain value.
    pub(crate) fn inspect_module(
        &self,
        config: &Config,
        module: &Bound<'py, pyo3::types::PyModule>,
        expected_path: Option<&Path>,
    ) -> Result<Module> {
        let actual_path = Path::from_live_module(&module.name()?.to_string())?;
        if let Some(expected_path) = expected_path {
            if &actual_path != expected_path {
                return Err(PyBindgenError::CodegenError(format!(
                    "live module resolved to '{actual_path}', expected '{expected_path}'"
                )));
            }
        }
        Module::parse_named(config, module, actual_path)
    }

    /// Construct the live synthetic parents requested by normalization.
    pub(crate) fn make_parent_modules(&self, paths: &[Path]) -> Result<Vec<Module>> {
        paths
            .iter()
            .cloned()
            .map(|path| Module::empty(self.py, path))
            .collect()
    }

    /// Resolve one dotted dependency origin, including segment access and policy filtering.
    fn resolve_dependency(&self, config: &Config, origin: &Path) -> DependencyResolution<'py> {
        let Some(root) = origin.root() else {
            return DependencyResolution::Skipped(
                "Dependency origin is empty and cannot be imported.".to_owned(),
            );
        };

        let mut last_module = match self.import_module_value(&root) {
            Ok(module) => module,
            Err(error) => {
                return DependencyResolution::Failed(format!(
                    "Could not import dependency root '{}': {error}",
                    root
                ));
            }
        };

        let mut truncation = None;
        for (index, path) in origin.iter().enumerate().skip(1) {
            let is_final = index + 1 == origin.len();
            match last_module.getattr(path.as_py()) {
                Ok(attr) => match attr.cast_into::<pyo3::types::PyModule>() {
                    Ok(module) => last_module = module,
                    Err(error) if !is_final => {
                        truncation = Some(format!(
                            "Intermediate dependency segment '{}' under '{}' is not a module: {error}",
                            path.as_py(),
                            last_module
                                .name()
                                .map(|name| name.to_string())
                                .unwrap_or_else(|_| "<unknown>".to_owned()),
                        ));
                        break;
                    }
                    Err(_) => {
                        // A final class/function/value is a normal dependency origin; the
                        // containing module is what can be emitted.
                    }
                },
                Err(error) if !is_final => {
                    truncation = Some(format!(
                        "Could not resolve intermediate dependency segment '{}' under '{}': {error}",
                        path.as_py(),
                        last_module
                            .name()
                            .map(|name| name.to_string())
                            .unwrap_or_else(|_| "<unknown>".to_owned()),
                    ));
                    break;
                }
                Err(_) => {
                    // A missing final item does not make its module prefix incomplete; only
                    // failed intermediate segments truncate.
                }
            }
        }

        let module_name = match last_module.name() {
            Ok(name) => {
                let name = name.to_string();
                if !name.is_empty() {
                    name
                } else {
                    return DependencyResolution::Skipped(
                        "Resolved dependency has no module name.".to_owned(),
                    );
                }
            }
            Err(error) => {
                return DependencyResolution::Skipped(format!(
                    "Could not inspect resolved dependency name: {error}"
                ));
            }
        };

        // Dependency policy follows the resolved module's dotted `__name__`. A module object
        // normally has no `__module__`, and a class reachable through a public alias can spoof
        // that field anyway; neither is a reliable identity for the module we are about to
        // parse. Segment-wise filtering of `module_name` preserves the ordinary name/blocklist
        // check while rejecting private module segments unless explicitly enabled.
        let module_origin = match Path::from_live_module(&module_name) {
            Ok(path) => path,
            Err(error) => {
                return DependencyResolution::Skipped(format!(
                    "Resolved dependency module '{module_name}' has invalid module name: {error}"
                ));
            }
        };
        if !config.is_attr_allowed(
            &Ident::from_py(&module_name),
            &module_origin,
            &self.py.get_type::<pyo3::types::PyModule>(),
        ) {
            return DependencyResolution::Skipped(format!(
                "Resolved dependency module '{module_name}' was excluded by configuration policy."
            ));
        }

        DependencyResolution::Resolved(ResolvedDependency {
            module: last_module,
            module_name,
            truncation,
        })
    }

    /// Resolve and parse every external import while this session owns all Python access.
    pub(crate) fn discover_dependencies(
        &self,
        config: &Config,
        existing_modules: &[Module],
    ) -> Result<LiveDependencyOutput> {
        fn collect_origins(modules: &[Module], origins: &mut Vec<String>) {
            for module in modules {
                for import in &module.imports {
                    if import.is_external() {
                        origins.push(import.origin.to_py());
                    }
                }
                collect_origins(&module.submodules, origins);
            }
        }

        let mut external_imports = Vec::new();
        collect_origins(existing_modules, &mut external_imports);
        external_imports.sort_unstable();
        external_imports.dedup();
        if external_imports.iter().any(String::is_empty) {
            return Err(PyBindgenError::CodegenError(
                "cannot resolve an empty external import origin".to_owned(),
            ));
        }

        tracing::debug!(
            requested_dependencies = external_imports.len(),
            "discovering live Python dependencies"
        );
        let mut modules = Vec::new();
        let mut diagnostics = Vec::new();
        // Cache successes and failures: repeated origins must not make completeness counts
        // depend on origin multiplicity.
        let mut resolved_modules: HashMap<String, Option<String>> = existing_modules
            .iter()
            .map(|module| (module.name.to_py(), None))
            .collect();

        for origin in &external_imports {
            diagnostics.push(DomainDiagnostic::info(
                DiagnosticCode::discovery(
                    DiscoverySubject::Dependency,
                    DiscoveryOutcome::Attempted,
                ),
                origin,
                format!("Attempting to discover dependency '{origin}'."),
            ));

            let origin_path = Path::from_py(origin);
            let resolved = match self.resolve_dependency(config, &origin_path) {
                DependencyResolution::Failed(detail) => {
                    diagnostics.push(dependency_outcome(origin, DiscoveryOutcome::Failed, detail));
                    continue;
                }
                DependencyResolution::Skipped(detail) => {
                    diagnostics.push(dependency_outcome(
                        origin,
                        DiscoveryOutcome::Skipped,
                        detail,
                    ));
                    continue;
                }
                DependencyResolution::Resolved(resolved) => resolved,
            };

            let module_name = resolved.module_name;
            if let Some(previous_failure) = resolved_modules.get(&module_name) {
                match previous_failure {
                    Some(detail) => diagnostics.push(dependency_outcome(
                        origin,
                        DiscoveryOutcome::Failed,
                        format!(
                            "Dependency module '{module_name}' previously failed to parse: {detail}"
                        ),
                    )),
                    None => match resolved.truncation {
                        Some(detail) => diagnostics.push(dependency_outcome(
                            origin,
                            DiscoveryOutcome::Truncated,
                            detail,
                        )),
                        None => diagnostics.push(dependency_outcome(
                            origin,
                            DiscoveryOutcome::Emitted,
                            format!(
                                "Dependency origin '{origin}' uses already-emitted module '{module_name}'."
                            ),
                        )),
                    },
                }
                continue;
            }

            match Module::parse(config, &resolved.module) {
                Ok(module) => {
                    modules.push(module);
                    resolved_modules.insert(module_name.clone(), None);
                    match resolved.truncation {
                        Some(detail) => diagnostics.push(dependency_outcome(
                            origin,
                            DiscoveryOutcome::Truncated,
                            detail,
                        )),
                        None => diagnostics.push(dependency_outcome(
                            origin,
                            DiscoveryOutcome::Emitted,
                            format!(
                                "Dependency origin '{origin}' was emitted from module '{module_name}'."
                            ),
                        )),
                    }
                }
                Err(error) => {
                    let detail =
                        format!("Could not parse dependency module '{module_name}': {error}");
                    resolved_modules.insert(module_name, Some(detail.clone()));
                    diagnostics.push(dependency_outcome(origin, DiscoveryOutcome::Failed, detail));
                }
            }
        }

        tracing::debug!(
            requested_dependencies = external_imports.len(),
            registered_dependencies = modules.len(),
            "live Python dependency discovery completed"
        );
        Ok(LiveDependencyOutput {
            modules,
            diagnostics,
        })
    }

    /// Acquire every live-only value needed by pure normalization and planning.
    pub(crate) fn generation_context(
        &self,
        config: &Config,
        modules: &[Module],
    ) -> Result<LiveGenerationContext> {
        let parent_paths = crate::normalized::required_live_parent_paths(modules);
        let parent_modules = self.make_parent_modules(&parent_paths)?;
        let paths = crate::normalized::required_import_paths(modules, config);
        let entries = paths.iter().map(|path| {
            let mut package = path.root().unwrap_or_else(|| unreachable!());
            for index in (1..path.len()).rev() {
                let candidate = Path::from(&path[..index]);
                if self.import_module_value(&candidate).is_ok() {
                    package = candidate;
                    break;
                }
            }
            (path.clone(), package)
        });
        Ok(LiveGenerationContext {
            parent_modules,
            import_prefixes: ImportPrefixes::new(entries)?,
        })
    }
}

fn python_c_string(value: impl Into<Vec<u8>>, description: &str) -> Result<std::ffi::CString> {
    std::ffi::CString::new(value).map_err(|error| {
        PyBindgenError::CodegenError(format!(
            "Python {description} contains a NUL byte at offset {}",
            error.nul_position()
        ))
    })
}

/// Prime the process-local live runtime for a caller-created module.
pub(crate) fn prime(py: Python<'_>) -> Result<()> {
    LiveSession::new(py, LivePolicy::unsuppressed()).prime_runtime()
}

/// Initialize and attach to Python, prime sentinels before target work, then finish cleanup.
pub(crate) fn with_session<T>(
    policy: LivePolicy,
    operation: impl for<'py> FnOnce(&LiveSession<'py>) -> Result<T>,
) -> Result<T> {
    #[cfg(not(any(PyPy, GraalPy)))]
    Python::initialize();
    Python::attach(|py| {
        LiveSession::new(py, policy).finish(|session| session.prime_runtime(), operation)
    })
}

/// Run a generation acquisition session and sample report metadata after coordinated cleanup.
///
/// Metadata reads intentionally occur after the output/sentinel permit is released. They only
/// inspect `platform` and `sys`, so they do not need sentinel restoration, and allowing them to
/// overlap another thread's stream suppression preserves the report contract on free-threaded
/// Python without making an unsuppressed live operation observe mutable target state.
pub(crate) fn with_generation_session<T>(
    policy: LivePolicy,
    operation: impl for<'py> FnOnce(&LiveSession<'py>) -> Result<T>,
) -> Result<(T, Option<PythonMetadata>)> {
    #[cfg(not(any(PyPy, GraalPy)))]
    Python::initialize();
    Python::attach(|py| {
        let session = LiveSession::new(py, policy);
        let value = session.finish(|session| session.prime_runtime(), operation)?;
        Ok((value, crate::report::python_metadata(py)))
    })
}

/// Compatibility harness for the existing emission unit fixture.
///
/// Production Codegen paths use [`with_session`] so raw Python values do not cross the live
/// operation boundary. The fixture is intentionally kept source-compatible until its parser
/// callback is migrated with the rest of the test helpers.
#[cfg(test)]
pub(crate) fn with_attached<T>(
    policy: LivePolicy,
    operation: impl for<'py> FnOnce(Python<'py>) -> Result<T>,
) -> Result<T> {
    with_session(policy, |session| operation(session.python()))
}

/// Run against a caller-created module only after the caller explicitly primed the runtime.
pub(crate) fn with_preloaded<'py, T>(
    module: &Bound<'py, pyo3::types::PyModule>,
    policy: LivePolicy,
    operation: impl FnOnce(&LiveSession<'py>) -> Result<T>,
) -> Result<T> {
    let session = LiveSession::new(module.py(), policy);
    session.require_primed()?;
    session.finish(|_| Ok(()), operation)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preloaded_operation_requires_explicit_priming() {
        #[cfg(not(any(PyPy, GraalPy)))]
        Python::initialize();

        // The assertion is meaningful in a fresh test process. If another test has already
        // primed the process-global snapshot, the preloaded contract is already satisfied and
        // this order-independent unit test has nothing further to reject.
        Python::attach(|py| {
            if !crate::runtime_sentinels::is_primed(py) {
                let error = with_preloaded(
                    &pyo3::types::PyModule::new(py, "unprimed_target").unwrap(),
                    LivePolicy::from_config(&Config::default()),
                    |_| Ok(()),
                )
                .unwrap_err();
                assert!(error.to_string().contains("prime_live_runtime"));
            }
        });
    }
}
