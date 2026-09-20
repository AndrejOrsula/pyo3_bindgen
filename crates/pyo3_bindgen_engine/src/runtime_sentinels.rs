use pyo3::{
    exceptions::{PyAttributeError, PyModuleNotFoundError, PyRuntimeError},
    prelude::*,
    sync::PyOnceLock,
    types::{PyDict, PyDictMethods, PyModule, PyModuleMethods},
};
use std::cell::{Cell, RefCell};

#[derive(Clone, Copy)]
pub(crate) enum StdlibModule {
    Inspect,
    Functools,
    CFunctools,
    Enum,
    Typing,
    Dataclasses,
    Types,
    Pkgutil,
    Annotationlib,
}

struct ModuleSnapshot {
    module: Py<PyModule>,
    dictionary: Py<PyDict>,
}

impl ModuleSnapshot {
    fn capture(module: Bound<'_, PyModule>, eager_attributes: &[&str]) -> PyResult<Self> {
        // Some supported runtimes expose stdlib compatibility attributes lazily. In
        // CPython 3.14t, for example, `typing.ForwardRef` is absent from both
        // `typing.__dict__` and `dir(typing)` until the first attribute access. Force every
        // sentinel consumed by the engine before copying the dictionary; missing
        // version-specific attributes remain legitimately absent.
        for attribute in eager_attributes {
            match module.getattr(*attribute) {
                Ok(_) => {}
                Err(error) if error.is_instance_of::<PyAttributeError>(module.py()) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(Self {
            dictionary: module.dict().copy()?.unbind(),
            module: module.unbind(),
        })
    }

    fn capture_validated(
        module: Bound<'_, PyModule>,
        expected_module: &str,
        eager_attributes: &[&str],
        anchors: &[(&str, &str, &str)],
    ) -> PyResult<Self> {
        let snapshot = Self::capture(module.clone(), eager_attributes)?;
        validate_pristine_module(&module, expected_module, anchors)?;
        Ok(snapshot)
    }
}

/// Reject a first snapshot whose stable stdlib anchors were already replaced.
///
/// Live introspection is trusted, not a security sandbox; this check targets the accidental
/// preloaded-module footgun. A protected module and each anchor must still advertise the
/// canonical stdlib module/name pair before their identities become the process baseline.
fn validate_pristine_module(
    module: &Bound<'_, PyModule>,
    expected_module: &str,
    anchors: &[(&str, &str, &str)],
) -> PyResult<()> {
    let actual_module = module.name()?.to_string();
    if actual_module != expected_module {
        return Err(PyRuntimeError::new_err(format!(
            "refusing to prime live runtime: expected stdlib module '{expected_module}', found '{actual_module}'"
        )));
    }
    for &(attribute, expected_origin, expected_name) in anchors {
        let value = module.getattr(attribute).map_err(|error| {
            PyRuntimeError::new_err(format!(
                "refusing to prime live runtime: stdlib anchor '{expected_module}.{attribute}' is unavailable: {error}"
            ))
        })?;
        let origin = value
            .getattr(pyo3::intern!(module.py(), "__module__"))
            .and_then(|origin| origin.extract::<String>())
            .map_err(|error| {
                PyRuntimeError::new_err(format!(
                    "refusing to prime live runtime: stdlib anchor '{expected_module}.{attribute}' has no canonical origin: {error}"
                ))
            })?;
        let name = value
            .getattr(pyo3::intern!(module.py(), "__name__"))
            .and_then(|name| name.extract::<String>())
            .map_err(|error| {
                PyRuntimeError::new_err(format!(
                    "refusing to prime live runtime: stdlib anchor '{expected_module}.{attribute}' has no canonical name: {error}"
                ))
            })?;
        if origin != expected_origin || name != expected_name {
            return Err(PyRuntimeError::new_err(format!(
                "refusing to prime live runtime: stdlib anchor '{expected_module}.{attribute}' is already replaced (found '{origin}.{name}')"
            )));
        }
    }
    Ok(())
}

struct RuntimeStdlib {
    inspect: ModuleSnapshot,
    functools: ModuleSnapshot,
    c_functools: ModuleSnapshot,
    enum_module: ModuleSnapshot,
    typing: ModuleSnapshot,
    dataclasses: ModuleSnapshot,
    types: ModuleSnapshot,
    pkgutil: ModuleSnapshot,
    builtins: ModuleSnapshot,
    annotationlib: Option<ModuleSnapshot>,
}

static RUNTIME_STDLIB: PyOnceLock<RuntimeStdlib> = PyOnceLock::new();

thread_local! {
    static STDLIB_INIT_DEPTH: Cell<usize> = const { Cell::new(0) };
    static RESTORE_DEPTH: Cell<usize> = const { Cell::new(0) };
    static INTEGRITY_FAILURE: RefCell<Option<PyErr>> = const { RefCell::new(None) };
}

struct StdlibInitGuard;

impl StdlibInitGuard {
    fn enter() -> PyResult<Self> {
        let reentrant = STDLIB_INIT_DEPTH.with(|depth| {
            if depth.get() > 0 {
                true
            } else {
                depth.set(1);
                false
            }
        });
        if reentrant {
            tracing::error!(
                observability_schema = 1,
                operation_id = "unavailable",
                component = "stdlib_sentinel_initialization",
                outcome = "reentrant_rejected",
                "reentrant runtime sentinel initialization rejected before PyOnceLock entry"
            );
            Err(PyRuntimeError::new_err(
                "reentrant runtime sentinel initialization is unsupported; prime the live runtime before installing import hooks that invoke pyo3_bindgen",
            ))
        } else {
            Ok(Self)
        }
    }
}

impl Drop for StdlibInitGuard {
    fn drop(&mut self) {
        STDLIB_INIT_DEPTH.with(|depth| {
            debug_assert_eq!(depth.get(), 1);
            depth.set(0);
        });
    }
}

struct RestoreDepthGuard {
    outermost: bool,
}

impl RestoreDepthGuard {
    fn enter() -> Self {
        let outermost = RESTORE_DEPTH.with(|depth| {
            let outermost = depth.get() == 0;
            depth.set(depth.get() + 1);
            outermost
        });
        if outermost {
            INTEGRITY_FAILURE.with(|failure| {
                failure.borrow_mut().take();
            });
        }
        Self { outermost }
    }
}

impl Drop for RestoreDepthGuard {
    fn drop(&mut self) {
        RESTORE_DEPTH.with(|depth| {
            debug_assert!(depth.get() > 0);
            depth.set(depth.get() - 1);
        });
        if self.outermost {
            INTEGRITY_FAILURE.with(|failure| {
                failure.borrow_mut().take();
            });
        }
    }
}

struct RestoreState {
    actual: Py<PyDict>,
    previous: Py<PyDict>,
}

struct RestoreGuard<'py> {
    py: Python<'py>,
    states: Option<Vec<RestoreState>>,
}

impl<'py> RestoreGuard<'py> {
    fn new(py: Python<'py>, capacity: usize) -> Self {
        Self {
            py,
            states: Some(Vec::with_capacity(capacity)),
        }
    }

    fn push(&mut self, state: RestoreState) {
        self.states
            .as_mut()
            .expect("restore states remain available until finish")
            .push(state);
    }

    fn restore(&self) -> PyResult<()> {
        restore_states(
            self.py,
            self.states
                .as_deref()
                .expect("restore states remain available until finish"),
        )
    }

    fn disarm(mut self) {
        self.states.take();
    }

    fn finish(mut self) -> PyResult<()> {
        let result = self.restore();
        self.states.take();
        result
    }
}

impl Drop for RestoreGuard<'_> {
    fn drop(&mut self) {
        if let Some(states) = self.states.take() {
            // A panic is already in flight, so restoration is necessarily best-effort.
            // Report a failed cleanup through Python's unraisable hook instead of
            // silently hiding possible process-global corruption.
            if let Err(error) = restore_states(self.py, &states) {
                error.write_unraisable(self.py, None);
            }
        }
    }
}

fn restore_states(py: Python<'_>, states: &[RestoreState]) -> PyResult<()> {
    let mut first_error = None;
    // Replacing an operation-installed object can synchronously execute its Python
    // finalizer. Keep every displaced value alive until the whole snapshot has been
    // restored, then reassert after those finalizers run. A reentrant finalizer can keep
    // installing new finalizable values forever, so fail closed instead of looping
    // without bound.
    //
    // This only covers destruction caused synchronously by releasing these references.
    // It deliberately does not force a process-wide garbage collection: cyclic objects
    // and runtimes with deferred finalization may run target callbacks later. Live
    // introspection executes trusted in-process code and is not a transactional sandbox.
    const MAX_RESTORE_PASSES: usize = 16;
    for pass in 1..=MAX_RESTORE_PASSES {
        let mut displaced = Vec::new();
        reassert_states(py, states, &mut displaced, &mut first_error);
        drop(displaced);

        if states_match(py, states) {
            if pass > 1 {
                tracing::debug!(
                    observability_schema = 1,
                    operation_id = "unavailable",
                    component = "stdlib_snapshot_restoration",
                    outcome = "stabilized",
                    restore_passes = pass,
                    "stdlib snapshot restoration stabilized"
                );
            }
            if let Some(error) = first_error {
                latch_integrity_failure(py, &error);
                return Err(error);
            }
            return Ok(());
        }
    }

    record_first_error(
        &mut first_error,
        PyRuntimeError::new_err(format!(
            "stdlib restoration did not stabilize after {MAX_RESTORE_PASSES} passes"
        )),
    );
    let error = first_error.expect("restoration instability always records an error");
    tracing::error!(
        observability_schema = 1,
        operation_id = "unavailable",
        component = "stdlib_snapshot_restoration",
        outcome = "failed",
        restore_passes = MAX_RESTORE_PASSES,
        "stdlib snapshot restoration failed to stabilize"
    );
    tracing::debug!(error = %error, "stdlib snapshot restoration failure detail");
    latch_integrity_failure(py, &error);
    Err(error)
}

fn reassert_states(
    py: Python<'_>,
    states: &[RestoreState],
    displaced: &mut Vec<Py<PyAny>>,
    first_error: &mut Option<PyErr>,
) {
    for state in states {
        reassert_dictionary(
            state.actual.bind(py),
            state.previous.bind(py),
            displaced,
            first_error,
        );
    }
}

fn states_match(py: Python<'_>, states: &[RestoreState]) -> bool {
    for state in states {
        if !dictionary_matches(state.actual.bind(py), state.previous.bind(py)) {
            return false;
        }
    }
    true
}

fn reassert_dictionary(
    actual: &Bound<'_, PyDict>,
    desired: &Bound<'_, PyDict>,
    displaced: &mut Vec<Py<PyAny>>,
    first_error: &mut Option<PyErr>,
) {
    if dictionary_matches(actual, desired) {
        return;
    }

    // Replacing only desired sentinel names is insufficient: arbitrary target callbacks
    // can add module globals that shadow builtins used by otherwise-canonical stdlib
    // functions. Rebuild the whole exact dictionary so absent keys are removed and the
    // desired key identities/insertion order are restored too.
    //
    // Retain every displaced key and value until all modules have been rebuilt. Clearing
    // the dictionary therefore cannot run their finalizers mid-pass, and no Python
    // hash/equality callback runs while a Rust dictionary iterator is live.
    let current_entries = actual
        .iter()
        .map(|(key, value)| (key.unbind(), value.unbind()))
        .collect::<Vec<_>>();
    actual.clear();
    if let Err(error) = actual.update(desired.as_mapping()) {
        record_first_error(first_error, error);
    }
    for (key, value) in current_entries {
        displaced.push(key);
        displaced.push(value);
    }
}

fn dictionary_matches(actual: &Bound<'_, PyDict>, desired: &Bound<'_, PyDict>) -> bool {
    if actual.len() != desired.len() {
        return false;
    }
    actual.iter().zip(desired.iter()).all(
        |((actual_key, actual_value), (desired_key, desired_value))| {
            actual_key.is(&desired_key) && actual_value.is(&desired_value)
        },
    )
}

fn record_first_error(first_error: &mut Option<PyErr>, error: PyErr) {
    if first_error.is_none() {
        *first_error = Some(error);
    }
}

fn latch_integrity_failure(py: Python<'_>, error: &PyErr) {
    INTEGRITY_FAILURE.with(|failure| {
        let mut failure = failure.borrow_mut();
        if failure.is_none() {
            *failure = Some(error.clone_ref(py));
        }
    });
}

fn latched_integrity_failure(py: Python<'_>) -> Option<PyErr> {
    INTEGRITY_FAILURE.with(|failure| failure.borrow().as_ref().map(|error| error.clone_ref(py)))
}

fn optional_module_snapshot(
    py: Python<'_>,
    module_name: &str,
    eager_attributes: &[&str],
    anchors: &[(&str, &str, &str)],
) -> PyResult<Option<ModuleSnapshot>> {
    match py.import(module_name) {
        Ok(module) => {
            ModuleSnapshot::capture_validated(module, module_name, eager_attributes, anchors)
                .map(Some)
        }
        Err(error) if is_exact_missing_module(py, &error, module_name) => Ok(None),
        Err(error) => Err(error),
    }
}

fn is_exact_missing_module(py: Python<'_>, error: &PyErr, module_name: &str) -> bool {
    error.is_instance_of::<PyModuleNotFoundError>(py)
        && error
            .value(py)
            .getattr("name")
            .and_then(|name| name.extract::<String>())
            .is_ok_and(|name| name == module_name)
}

#[cfg(test)]
pub(crate) struct TestAttributeRestore {
    module: Py<PyModule>,
    name: String,
    original: Option<Py<PyAny>>,
}

#[cfg(test)]
impl TestAttributeRestore {
    pub(crate) fn new(py: Python<'_>, module: &Bound<'_, PyModule>, name: &str) -> PyResult<Self> {
        let original = match module.getattr(name) {
            Ok(value) => Some(value.unbind()),
            Err(error) if error.is_instance_of::<PyAttributeError>(py) => None,
            Err(error) => return Err(error),
        };
        Ok(Self {
            module: module.clone().unbind(),
            name: name.to_owned(),
            original,
        })
    }
}

#[cfg(test)]
impl Drop for TestAttributeRestore {
    fn drop(&mut self) {
        Python::attach(|py| {
            let Ok(_runtime_permit) = crate::io_utils::acquire_python_runtime(py) else {
                return;
            };
            let module = self.module.bind(py);
            match &self.original {
                Some(original) => {
                    let _ = module.setattr(self.name.as_str(), original.bind(py));
                }
                None => {
                    if let Err(error) = module.delattr(self.name.as_str()) {
                        if !error.is_instance_of::<PyAttributeError>(py) {
                            error.write_unraisable(py, Some(module));
                        }
                    }
                }
            }
        });
    }
}

fn stdlib(py: Python<'_>) -> PyResult<&RuntimeStdlib> {
    if let Some(stdlib) = RUNTIME_STDLIB.get(py) {
        return Ok(stdlib);
    }
    // `PyOnceLock` deliberately has undefined reentrant-initialization behavior and may
    // deadlock. Python imports execute arbitrary meta-path hooks synchronously, so reject
    // same-thread recursion before entering the cell. Ordinary concurrent callers retain
    // PyOnceLock's detach-and-wait behavior; calls after initialization take the fast path.
    let _init_guard = StdlibInitGuard::enter()?;
    RUNTIME_STDLIB.get_or_try_init(py, || {
        Ok(RuntimeStdlib {
            inspect: ModuleSnapshot::capture_validated(
                py.import(pyo3::intern!(py, "inspect"))?,
                "inspect",
                &[
                    "signature",
                    "getattr_static",
                    "isfunction",
                    "ismethod",
                    "iscoroutinefunction",
                    "isasyncgenfunction",
                    "isroutine",
                    "Signature",
                ],
                &[("signature", "inspect", "signature")],
            )?,
            functools: ModuleSnapshot::capture_validated(
                py.import(pyo3::intern!(py, "functools"))?,
                "functools",
                &["partial", "partialmethod"],
                &[("partial", "functools", "partial")],
            )?,
            c_functools: ModuleSnapshot::capture_validated(
                py.import(pyo3::intern!(py, "_functools"))?,
                "_functools",
                &["partial"],
                &[("partial", "functools", "partial")],
            )?,
            enum_module: ModuleSnapshot::capture_validated(
                py.import(pyo3::intern!(py, "enum"))?,
                "enum",
                &["Enum", "Flag", "IntFlag", "IntEnum", "StrEnum"],
                &[("Enum", "enum", "Enum")],
            )?,
            typing: ModuleSnapshot::capture_validated(
                py.import(pyo3::intern!(py, "typing"))?,
                "typing",
                &[
                    "ClassVar",
                    "ForwardRef",
                    "TypeVar",
                    "_AnyMeta",
                    "_CallableType",
                    "_GenericAlias",
                    "_LiteralGenericAlias",
                    "_SpecialForm",
                    "_SpecialGenericAlias",
                    "_TupleType",
                    "_UnionGenericAlias",
                ],
                &[("TypeVar", "typing", "TypeVar")],
            )?,
            dataclasses: ModuleSnapshot::capture_validated(
                py.import(pyo3::intern!(py, "dataclasses"))?,
                "dataclasses",
                &["InitVar", "KW_ONLY", "fields"],
                &[("fields", "dataclasses", "fields")],
            )?,
            types: ModuleSnapshot::capture_validated(
                py.import(pyo3::intern!(py, "types"))?,
                "types",
                &["GenericAlias", "UnionType"],
                // `types.GenericAlias` only exists from 3.9 on. Anchoring it unconditionally
                // conflates "absent because the interpreter predates it" with "replaced by
                // something else", which is the only thing this check exists to catch, and made
                // priming refuse outright on 3.8. Where the attribute exists it is still anchored,
                // so a deleted or substituted one is still rejected. `UnionType` is deliberately
                // eager-only: it arrives in 3.10 and both consumers already degrade to `false`
                // when it is missing.
                if py.version_info() >= (3, 9) {
                    &[("GenericAlias", "types", "GenericAlias")]
                } else {
                    &[]
                },
            )?,
            pkgutil: ModuleSnapshot::capture_validated(
                py.import(pyo3::intern!(py, "pkgutil"))?,
                "pkgutil",
                &["iter_modules"],
                &[("iter_modules", "pkgutil", "iter_modules")],
            )?,
            builtins: ModuleSnapshot::capture_validated(
                py.import(pyo3::intern!(py, "builtins"))?,
                "builtins",
                &["classmethod", "property", "staticmethod"],
                &[("property", "builtins", "property")],
            )?,
            annotationlib: optional_module_snapshot(
                py,
                "annotationlib",
                &["Format", "ForwardRef", "get_annotations"],
                &[("get_annotations", "annotationlib", "get_annotations")],
            )?,
        })
    })
}

pub(crate) fn prime(py: Python<'_>) -> PyResult<()> {
    let _runtime_permit = crate::io_utils::acquire_python_runtime(py)?;
    stdlib(py).map(|_| ())
}

pub(crate) fn is_primed(py: Python<'_>) -> bool {
    RUNTIME_STDLIB.get(py).is_some()
}

fn module_snapshot(stdlib: &RuntimeStdlib, module: StdlibModule) -> Option<&ModuleSnapshot> {
    match module {
        StdlibModule::Inspect => Some(&stdlib.inspect),
        StdlibModule::Functools => Some(&stdlib.functools),
        StdlibModule::CFunctools => Some(&stdlib.c_functools),
        StdlibModule::Enum => Some(&stdlib.enum_module),
        StdlibModule::Typing => Some(&stdlib.typing),
        StdlibModule::Dataclasses => Some(&stdlib.dataclasses),
        StdlibModule::Types => Some(&stdlib.types),
        StdlibModule::Pkgutil => Some(&stdlib.pkgutil),
        StdlibModule::Annotationlib => stdlib.annotationlib.as_ref(),
    }
}

fn module_snapshots(stdlib: &RuntimeStdlib) -> [Option<&ModuleSnapshot>; 10] {
    [
        Some(&stdlib.inspect),
        Some(&stdlib.functools),
        Some(&stdlib.c_functools),
        Some(&stdlib.enum_module),
        Some(&stdlib.typing),
        Some(&stdlib.dataclasses),
        Some(&stdlib.types),
        Some(&stdlib.pkgutil),
        Some(&stdlib.builtins),
        stdlib.annotationlib.as_ref(),
    ]
}

/// Reassert every canonical module dictionary immediately before returning a helper.
///
/// Live `__dir__`, descriptor, and annotation callbacks can mutate stdlib globals after
/// [`with_restored`] first installs the snapshots. All consumers call these accessors
/// directly before invoking the returned helper. Replacing canonical entries is not
/// enough: an added module global can shadow a builtin used by the canonical function.
/// Restore the exact key set and value identities, then reassert after synchronous
/// finalizers run. The outer guard remains responsible for restoring the target-visible
/// dictionaries after parsing.
fn reassert_all(py: Python<'_>, stdlib: &RuntimeStdlib) -> PyResult<()> {
    // Outside `with_restored`, return snapshotted objects without mutating the public
    // stdlib dictionaries. Production helper calls run inside that guard; direct type
    // classification remains side-effect free.
    if !RESTORE_DEPTH.with(|depth| depth.get() > 0) {
        return Ok(());
    }

    const MAX_CANONICAL_PASSES: usize = 16;
    let mut first_error = None;
    for _ in 0..MAX_CANONICAL_PASSES {
        let mut displaced = Vec::new();
        for snapshot in module_snapshots(stdlib).into_iter().flatten() {
            reassert_dictionary(
                &snapshot.module.bind(py).dict(),
                snapshot.dictionary.bind(py),
                &mut displaced,
                &mut first_error,
            );
        }
        drop(displaced);

        let matches = module_snapshots(stdlib)
            .into_iter()
            .flatten()
            .all(|snapshot| {
                dictionary_matches(
                    &snapshot.module.bind(py).dict(),
                    snapshot.dictionary.bind(py),
                )
            });
        if matches {
            if let Some(error) = first_error {
                latch_integrity_failure(py, &error);
                return Err(error);
            }
            return Ok(());
        }
    }

    record_first_error(
        &mut first_error,
        PyRuntimeError::new_err(format!(
            "stdlib canonicalization did not stabilize after {MAX_CANONICAL_PASSES} passes"
        )),
    );
    let error = first_error.expect("canonicalization instability always records an error");
    latch_integrity_failure(py, &error);
    Err(error)
}

pub(crate) fn attr<'py>(
    py: Python<'py>,
    module: StdlibModule,
    name: &str,
) -> PyResult<Bound<'py, PyAny>> {
    let _runtime_permit = crate::io_utils::acquire_python_runtime(py)?;
    let stdlib = stdlib(py)?;
    reassert_all(py, stdlib)?;
    module_snapshot(stdlib, module)
        .and_then(|snapshot| snapshot.dictionary.bind(py).get_item(name).ok().flatten())
        .ok_or_else(|| {
            PyAttributeError::new_err(format!(
                "snapshotted stdlib module has no attribute {name:?}"
            ))
        })
}

pub(crate) fn optional_attr<'py>(
    py: Python<'py>,
    module: StdlibModule,
    name: &str,
) -> PyResult<Option<Bound<'py, PyAny>>> {
    let _runtime_permit = crate::io_utils::acquire_python_runtime(py)?;
    let stdlib = stdlib(py)?;
    reassert_all(py, stdlib)?;
    match module_snapshot(stdlib, module) {
        Some(snapshot) => snapshot.dictionary.bind(py).get_item(name),
        None => Ok(None),
    }
}

pub(crate) fn inspect_module(py: Python<'_>) -> PyResult<Bound<'_, PyModule>> {
    let _runtime_permit = crate::io_utils::acquire_python_runtime(py)?;
    let stdlib = stdlib(py)?;
    reassert_all(py, stdlib)?;
    Ok(stdlib.inspect.module.bind(py).clone())
}

/// Temporarily expose the pre-target stdlib dictionaries while live parsing runs.
///
/// Snapshotted function objects retain their original module globals. Restoring the
/// module entries as well as holding the function objects therefore protects internal
/// dependencies such as `inspect.iscoroutinefunction -> functools.partial`. Every
/// target-visible mutation is restored before this function returns, including on
/// parsing failure.
pub(crate) fn with_restored<T, E>(
    py: Python<'_>,
    operation: impl FnOnce() -> std::result::Result<T, E>,
) -> std::result::Result<T, E>
where
    E: From<PyErr>,
{
    let _runtime_permit = crate::io_utils::acquire_python_runtime(py).map_err(E::from)?;
    let _restore_depth = RestoreDepthGuard::enter();
    let stdlib = stdlib(py).map_err(E::from)?;
    let snapshots = module_snapshots(stdlib);
    let mut restore_guard = RestoreGuard::new(py, snapshots.len());
    let setup_result = (|| {
        // Capture every target-visible dictionary before changing any of them. A
        // displaced object's finalizer can mutate another stdlib module, so interleaving
        // capture and canonicalization would record a finalizer-created state as that
        // later module's entry state.
        for snapshot in snapshots.into_iter().flatten() {
            let actual = snapshot.module.bind(py).dict();
            let previous = actual.copy().map_err(E::from)?;
            restore_guard.push(RestoreState {
                actual: actual.clone().unbind(),
                previous: previous.unbind(),
            });
        }
        reassert_all(py, stdlib).map_err(E::from)
    })();
    if let Err(error) = setup_result {
        return resolve_with_restoration(Err(error), restore_guard.finish());
    }

    let mut result = Some(operation());
    let mut integrity_failure = latched_integrity_failure(py);
    if integrity_failure.is_some() {
        // Returning an integrity error discards the operation result. Drop it while the
        // entry-state snapshot is still available: owned Python values can run finalizers
        // which mutate stdlib dictionaries and must therefore precede exact cleanup.
        drop(result.take());
    }

    if let Err(error) = restore_guard.restore() {
        restore_guard.disarm();
        // Cleanup failure takes precedence because target-visible process-global state
        // may remain corrupted even if an earlier canonicalization failure was latched.
        return Err(E::from(error));
    }

    if integrity_failure.is_none() {
        integrity_failure = latched_integrity_failure(py);
        if integrity_failure.is_some() {
            // A reentrant finalizer can first latch an integrity failure during cleanup.
            // Discard the now-unsuccessful result, then restore the retained entry snapshot
            // once more so that result finalizers cannot poison the returned process state.
            drop(result.take());
            if let Err(error) = restore_guard.finish() {
                return Err(E::from(error));
            }
        } else {
            restore_guard.disarm();
        }
    } else {
        restore_guard.disarm();
    }

    if let Some(error) = integrity_failure {
        // Sentinel consumers may deliberately turn ordinary target-introspection errors
        // into `false`/`None`. Exact stdlib canonicalization failures are different:
        // they mean process-global integrity was not established. Latch the first such
        // failure for the whole outer operation so even a fail-soft consumer cannot
        // accidentally convert it into successful generation.
        return Err(E::from(error));
    }
    result.expect("a successful operation retains its result")
}

fn resolve_with_restoration<T, E>(
    operation: std::result::Result<T, E>,
    restoration: PyResult<()>,
) -> std::result::Result<T, E>
where
    E: From<PyErr>,
{
    match (operation, restoration) {
        // A cleanup failure means process-global sentinels may remain corrupted. It
        // therefore takes precedence over the ordinary operation error.
        (Err(_), Err(cleanup_error)) => Err(E::from(cleanup_error)),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(error)) => Err(E::from(error)),
        (Ok(value), Ok(())) => Ok(value),
    }
}

pub(crate) fn functools_partial(py: Python<'_>) -> PyResult<Bound<'_, PyAny>> {
    attr(py, StdlibModule::CFunctools, "partial")
}

pub(crate) fn functools_partialmethod(py: Python<'_>) -> PyResult<Bound<'_, PyAny>> {
    attr(py, StdlibModule::Functools, "partialmethod")
}

pub(crate) fn inspect_signature(py: Python<'_>) -> PyResult<Bound<'_, PyAny>> {
    attr(py, StdlibModule::Inspect, "signature")
}

pub(crate) fn inspect_getattr_static(py: Python<'_>) -> PyResult<Bound<'_, PyAny>> {
    attr(py, StdlibModule::Inspect, "getattr_static")
}

pub(crate) fn inspect_isfunction(py: Python<'_>) -> PyResult<Bound<'_, PyAny>> {
    attr(py, StdlibModule::Inspect, "isfunction")
}

pub(crate) fn inspect_ismethod(py: Python<'_>) -> PyResult<Bound<'_, PyAny>> {
    attr(py, StdlibModule::Inspect, "ismethod")
}

pub(crate) fn inspect_iscoroutinefunction(py: Python<'_>) -> PyResult<Bound<'_, PyAny>> {
    attr(py, StdlibModule::Inspect, "iscoroutinefunction")
}

pub(crate) fn inspect_isasyncgenfunction(py: Python<'_>) -> PyResult<Bound<'_, PyAny>> {
    attr(py, StdlibModule::Inspect, "isasyncgenfunction")
}

pub(crate) fn inspect_isroutine(py: Python<'_>) -> PyResult<Bound<'_, PyAny>> {
    attr(py, StdlibModule::Inspect, "isroutine")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::panic::{catch_unwind, AssertUnwindSafe};

    #[cfg(not(any(PyPy, GraalPy)))]
    fn unstable_finalizer_globals<'py>(
        py: Python<'py>,
        inspect: &Bound<'py, PyModule>,
        builtins: &Bound<'py, PyModule>,
        marker: &Bound<'py, PyAny>,
    ) -> PyResult<Bound<'py, PyDict>> {
        let globals = PyDict::new(py);
        globals.set_item("inspect", inspect)?;
        globals.set_item("builtins", builtins)?;
        globals.set_item("marker", marker)?;
        py.run(
            pyo3::ffi::c_str!(
                r#"
remaining = 64

class Bomb:
    def __del__(self):
        global remaining
        inspect.signature = marker
        if remaining:
            remaining -= 1
            builtins.property = Bomb()
"#
            ),
            Some(&globals),
            None,
        )?;
        Ok(globals)
    }

    #[cfg(not(any(PyPy, GraalPy)))]
    fn disarm_finalizer_chain(
        globals: &Bound<'_, PyDict>,
        inspect: &Bound<'_, PyModule>,
        builtins: &Bound<'_, PyModule>,
        original_property: &Bound<'_, PyAny>,
        target_signature: &Bound<'_, PyAny>,
    ) -> PyResult<()> {
        globals.set_item("remaining", 0)?;
        // Replacing the last Bomb runs one finalizer, which may touch inspect.
        builtins.setattr("property", original_property)?;
        inspect.setattr("signature", target_signature)?;
        Ok(())
    }

    #[cfg(not(any(PyPy, GraalPy)))]
    #[test]
    fn type_classification_propagates_canonicalization_failure() {
        pyo3::Python::initialize();
        pyo3::Python::attach(|py| {
            crate::io_utils::with_suppressed_python_output(py, false, false, || {
                prime(py)?;
                let inspect = py.import(pyo3::intern!(py, "inspect"))?;
                let builtins = py.import(pyo3::intern!(py, "builtins"))?;
                let _inspect_restore = TestAttributeRestore::new(py, &inspect, "signature")?;
                let _builtins_restore = TestAttributeRestore::new(py, &builtins, "property")?;

                let original_property = builtins.getattr("property")?;
                let canonical_signature = inspect.getattr("signature")?;
                let marker = py.eval(pyo3::ffi::c_str!("object()"), None, None)?;
                let alias = py.eval(
                    pyo3::ffi::c_str!("__import__('typing').List[int]"),
                    None,
                    None,
                )?;
                let globals = unstable_finalizer_globals(py, &inspect, &builtins, &marker)?;
                // Sixteen finalizers each mutate a module already visited in the pass.
                // The seventeenth canonicalization pass would succeed, so a consumer
                // which swallows the first accessor error can continue with corrupted
                // process-global state and still let outer cleanup succeed.
                globals.set_item("remaining", 15)?;

                let result: crate::Result<crate::typing::Type> = with_restored(py, || {
                    py.run(
                        pyo3::ffi::c_str!("builtins.property = Bomb()"),
                        Some(&globals),
                        None,
                    )?;
                    crate::typing::Type::try_from(alias.clone())
                });
                disarm_finalizer_chain(
                    &globals,
                    &inspect,
                    &builtins,
                    &original_property,
                    &canonical_signature,
                )?;

                let error =
                    result.expect_err("canonicalization failure must abort type classification");
                let crate::PyBindgenError::PyError(error) = error else {
                    panic!("unexpected classification error: {error}");
                };
                assert!(
                    error.is_instance_of::<PyRuntimeError>(py)
                        && error
                            .to_string()
                            .contains("stdlib canonicalization did not stabilize"),
                    "unexpected canonicalization failure: {error}",
                );

                let recovered: crate::Result<crate::typing::Type> =
                    with_restored(py, || crate::typing::Type::try_from(alias.clone()));
                assert_eq!(
                    recovered?,
                    crate::typing::Type::PyList(Box::new(crate::typing::Type::PyInt)),
                    "a completed outer guard must clear its integrity latch",
                );
                Ok(())
            })
            .unwrap();
        });
    }

    #[cfg(not(any(PyPy, GraalPy)))]
    #[test]
    fn discarded_success_value_is_dropped_before_integrity_cleanup() {
        pyo3::Python::initialize();
        pyo3::Python::attach(|py| {
            crate::io_utils::with_suppressed_python_output(py, false, false, || {
                prime(py)?;
                let inspect = py.import(pyo3::intern!(py, "inspect"))?;
                let builtins = py.import(pyo3::intern!(py, "builtins"))?;
                let _inspect_restore = TestAttributeRestore::new(py, &inspect, "signature")?;
                let _builtins_restore = TestAttributeRestore::new(py, &builtins, "property")?;

                let original_property = builtins.getattr("property")?;
                let canonical_signature = inspect.getattr("signature")?;
                let marker = py.eval(pyo3::ffi::c_str!("object()"), None, None)?;
                let result_marker = py.eval(pyo3::ffi::c_str!("object()"), None, None)?;
                let alias = py.eval(
                    pyo3::ffi::c_str!("__import__('typing').List[int]"),
                    None,
                    None,
                )?;
                let globals = unstable_finalizer_globals(py, &inspect, &builtins, &marker)?;
                globals.set_item("remaining", 15)?;
                globals.set_item("result_marker", &result_marker)?;
                py.run(
                    pyo3::ffi::c_str!(
                        r#"
class ResultBomb:
    def __del__(self):
        inspect.signature = result_marker
"#
                    ),
                    Some(&globals),
                    None,
                )?;

                let result: crate::Result<Py<PyAny>> = with_restored(py, || {
                    py.run(
                        pyo3::ffi::c_str!("builtins.property = Bomb()"),
                        Some(&globals),
                        None,
                    )?;
                    let _ = crate::typing::Type::try_from(alias.clone())?;
                    Ok(py
                        .eval(pyo3::ffi::c_str!("ResultBomb()"), Some(&globals), None)?
                        .unbind())
                });
                let signature_was_restored = inspect.getattr("signature")?.is(&canonical_signature);
                disarm_finalizer_chain(
                    &globals,
                    &inspect,
                    &builtins,
                    &original_property,
                    &canonical_signature,
                )?;

                let error = result.expect_err("latched integrity failure must discard success");
                assert!(
                    matches!(
                        error,
                        crate::PyBindgenError::PyError(ref error)
                            if error.is_instance_of::<PyRuntimeError>(py)
                                && error
                                    .to_string()
                                    .contains("stdlib canonicalization did not stabilize")
                    ),
                    "unexpected canonicalization failure: {error}",
                );
                assert!(
                    signature_was_restored,
                    "discarded success value finalized after exact stdlib cleanup",
                );
                Ok(())
            })
            .unwrap();
        });
    }

    #[cfg(not(any(PyPy, GraalPy)))]
    #[test]
    fn nested_guard_cannot_hide_a_latched_integrity_failure() {
        pyo3::Python::initialize();
        pyo3::Python::attach(|py| {
            crate::io_utils::with_suppressed_python_output(py, false, false, || {
                prime(py)?;
                let inspect = py.import(pyo3::intern!(py, "inspect"))?;
                let builtins = py.import(pyo3::intern!(py, "builtins"))?;
                let _inspect_restore = TestAttributeRestore::new(py, &inspect, "signature")?;
                let _builtins_restore = TestAttributeRestore::new(py, &builtins, "property")?;

                let original_property = builtins.getattr("property")?;
                let canonical_signature = inspect.getattr("signature")?;
                let marker = py.eval(pyo3::ffi::c_str!("object()"), None, None)?;
                let alias = py.eval(
                    pyo3::ffi::c_str!("__import__('typing').List[int]"),
                    None,
                    None,
                )?;
                let globals = unstable_finalizer_globals(py, &inspect, &builtins, &marker)?;
                globals.set_item("remaining", 15)?;

                let result: crate::Result<()> = with_restored(py, || {
                    let _swallowed: crate::Result<crate::typing::Type> = with_restored(py, || {
                        py.run(
                            pyo3::ffi::c_str!("builtins.property = Bomb()"),
                            Some(&globals),
                            None,
                        )?;
                        crate::typing::Type::try_from(alias.clone())
                    });
                    Ok(())
                });
                disarm_finalizer_chain(
                    &globals,
                    &inspect,
                    &builtins,
                    &original_property,
                    &canonical_signature,
                )?;

                let error =
                    result.expect_err("an outer guard must retain a nested integrity failure");
                assert!(
                    matches!(
                        error,
                        crate::PyBindgenError::PyError(ref error)
                            if error.is_instance_of::<PyRuntimeError>(py)
                                && error
                                    .to_string()
                                    .contains("stdlib canonicalization did not stabilize")
                    ),
                    "unexpected nested canonicalization failure: {error}",
                );
                Ok(())
            })
            .unwrap();
        });
    }

    #[cfg(not(any(PyPy, GraalPy)))]
    #[test]
    fn outer_guard_propagates_a_swallowed_nested_cleanup_failure() {
        pyo3::Python::initialize();
        pyo3::Python::attach(|py| {
            crate::io_utils::with_suppressed_python_output(py, false, false, || {
                prime(py)?;
                let inspect = py.import(pyo3::intern!(py, "inspect"))?;
                let builtins = py.import(pyo3::intern!(py, "builtins"))?;
                let _inspect_restore = TestAttributeRestore::new(py, &inspect, "signature")?;
                let _builtins_restore = TestAttributeRestore::new(py, &builtins, "property")?;

                let original_property = builtins.getattr("property")?;
                let canonical_signature = inspect.getattr("signature")?;
                let marker = py.eval(pyo3::ffi::c_str!("object()"), None, None)?;
                let globals = unstable_finalizer_globals(py, &inspect, &builtins, &marker)?;
                globals.set_item("remaining", 15)?;

                let result: crate::Result<()> = with_restored(py, || {
                    let _swallowed: crate::Result<()> = with_restored(py, || {
                        py.run(
                            pyo3::ffi::c_str!("builtins.property = Bomb()"),
                            Some(&globals),
                            None,
                        )?;
                        Ok(())
                    });
                    Ok(())
                });
                disarm_finalizer_chain(
                    &globals,
                    &inspect,
                    &builtins,
                    &original_property,
                    &canonical_signature,
                )?;

                let error =
                    result.expect_err("an outer guard must retain a nested cleanup failure");
                assert!(
                    matches!(
                        error,
                        crate::PyBindgenError::PyError(ref error)
                            if error.is_instance_of::<PyRuntimeError>(py)
                                && error
                                    .to_string()
                                    .contains("stdlib restoration did not stabilize")
                    ),
                    "unexpected nested cleanup failure: {error}",
                );
                Ok(())
            })
            .unwrap();
        });
    }

    #[test]
    fn optional_module_suppresses_only_exact_absence() {
        pyo3::Python::initialize();
        pyo3::Python::attach(|py| {
            let exact = PyModuleNotFoundError::new_err("annotationlib is absent");
            exact.value(py).setattr("name", "annotationlib").unwrap();
            assert!(is_exact_missing_module(py, &exact, "annotationlib"));

            let dependency = PyModuleNotFoundError::new_err("dependency is absent");
            dependency
                .value(py)
                .setattr("name", "annotationlib_dependency")
                .unwrap();
            assert!(
                !is_exact_missing_module(py, &dependency, "annotationlib"),
                "a missing transitive dependency must propagate",
            );

            let runtime = pyo3::exceptions::PyRuntimeError::new_err("broken import hook");
            assert!(
                !is_exact_missing_module(py, &runtime, "annotationlib"),
                "unexpected import failures must propagate",
            );
        });
    }

    #[test]
    fn restoration_is_unwind_safe() {
        pyo3::Python::initialize();
        pyo3::Python::attach(|py| {
            crate::io_utils::with_suppressed_python_output(py, false, false, || {
                prime(py)?;
                let inspect = py.import(pyo3::intern!(py, "inspect"))?;
                let _restore = TestAttributeRestore::new(py, &inspect, "signature")?;
                let added_name = "__pyo3_bindgen_added_during_unwind";
                let _added_restore = TestAttributeRestore::new(py, &inspect, added_name)?;
                if inspect.hasattr(added_name)? {
                    inspect.delattr(added_name)?;
                }
                let poison = py.eval(pyo3::ffi::c_str!("lambda value: value"), None, None)?;
                inspect.setattr(pyo3::intern!(py, "signature"), &poison)?;

                let unwind = catch_unwind(AssertUnwindSafe(|| {
                    let _: PyResult<()> = with_restored(py, || -> PyResult<()> {
                        inspect.setattr(added_name, &poison)?;
                        panic!("intentional restoration regression panic")
                    });
                }));
                assert!(unwind.is_err(), "the regression operation must panic");
                assert!(
                    inspect.getattr(pyo3::intern!(py, "signature"))?.is(&poison),
                    "panic cleanup did not restore the target-visible inspect.signature",
                );
                assert!(
                    !inspect.hasattr(added_name)?,
                    "panic cleanup retained an attribute added by the operation",
                );
                Ok(())
            })
            .unwrap();
        });
    }

    #[test]
    fn cleanup_restores_the_exact_entry_dictionary() {
        pyo3::Python::initialize();
        pyo3::Python::attach(|py| {
            crate::io_utils::with_suppressed_python_output(py, false, false, || {
                prime(py)?;
                let inspect = py.import(pyo3::intern!(py, "inspect"))?;
                let preserved_name = "__pyo3_bindgen_preexisting_noncanonical";
                let added_name = "__pyo3_bindgen_added_during_operation";
                let _preserved_restore = TestAttributeRestore::new(py, &inspect, preserved_name)?;
                let _added_restore = TestAttributeRestore::new(py, &inspect, added_name)?;

                let original = py.eval(pyo3::ffi::c_str!("object()"), None, None)?;
                let replacement = py.eval(pyo3::ffi::c_str!("object()"), None, None)?;
                inspect.setattr(preserved_name, &original)?;
                if inspect.hasattr(added_name)? {
                    inspect.delattr(added_name)?;
                }

                let result: PyResult<()> = with_restored(py, || {
                    inspect.setattr(preserved_name, &replacement)?;
                    inspect.setattr(added_name, &replacement)?;
                    Ok(())
                });
                result?;

                assert!(
                    inspect.getattr(preserved_name)?.is(&original),
                    "cleanup did not restore a noncanonical attribute present on entry",
                );
                assert!(
                    !inspect.hasattr(added_name)?,
                    "cleanup retained an attribute added by the operation",
                );
                Ok(())
            })
            .unwrap();
        });
    }

    #[test]
    fn canonicalization_does_not_invoke_a_non_string_key() {
        pyo3::Python::initialize();
        pyo3::Python::attach(|py| {
            crate::io_utils::with_suppressed_python_output(py, false, false, || {
                prime(py)?;
                let inspect = py.import(pyo3::intern!(py, "inspect"))?;
                let mutation_name = "__pyo3_bindgen_mutating_key_callback";
                let _mutation_restore = TestAttributeRestore::new(py, &inspect, mutation_name)?;
                if inspect.hasattr(mutation_name)? {
                    inspect.delattr(mutation_name)?;
                }

                let globals = PyDict::new(py);
                globals.set_item("inspect", &inspect)?;
                globals.set_item(
                    "marker",
                    py.eval(pyo3::ffi::c_str!("object()"), None, None)?,
                )?;
                py.run(
                    pyo3::ffi::c_str!(
                        r#"
class MutatingKey:
    armed = False

    def __hash__(self):
        if self.armed:
            self.armed = False
            inspect.__dict__["__pyo3_bindgen_mutating_key_callback"] = marker
        return 8675309

key = MutatingKey()
inspect.__dict__[key] = marker
key.armed = True
"#
                    ),
                    Some(&globals),
                    None,
                )?;
                let key = globals
                    .get_item("key")?
                    .expect("the custom dictionary key exists");

                let observation: PyResult<(bool, bool)> = with_restored(py, || {
                    Ok((
                        !inspect
                            .dict()
                            .iter()
                            .any(|(candidate, _)| candidate.is(&key)),
                        !inspect.hasattr(mutation_name)?,
                    ))
                });

                // Gather the postconditions and remove the deliberately noncanonical test
                // key before asserting, keeping this process-global regression hermetic.
                let key_was_restored = inspect
                    .dict()
                    .iter()
                    .any(|(candidate, _)| candidate.is(&key));
                let callback_entry_was_removed = !inspect.hasattr(mutation_name)?;
                let callback_never_ran = key.getattr("armed")?.extract::<bool>()?;
                key.setattr("armed", false)?;
                inspect.dict().del_item(&key)?;
                let (key_was_hidden, callback_entry_was_hidden) = observation?;

                assert!(
                    key_was_hidden && callback_entry_was_hidden && callback_never_ran,
                    "the guarded operation did not see the exact canonical dictionary",
                );
                assert!(
                    key_was_restored && callback_entry_was_removed,
                    "cleanup did not restore the exact entry dictionary after a mutating key",
                );
                Ok(())
            })
            .unwrap();
        });
    }

    #[cfg(not(any(PyPy, GraalPy)))]
    #[test]
    fn canonicalization_reasserts_after_displaced_value_finalizers() {
        pyo3::Python::initialize();
        pyo3::Python::attach(|py| {
            crate::io_utils::with_suppressed_python_output(py, false, false, || {
                prime(py)?;
                let inspect = py.import(pyo3::intern!(py, "inspect"))?;
                let builtins = py.import(pyo3::intern!(py, "builtins"))?;
                let added_name = "__pyo3_bindgen_added_during_canonicalization";
                // Drop in reverse order: removing the Bomb runs its finalizer, then the
                // inspect and added-entry guards repair those deliberate test mutations.
                let _added_restore = TestAttributeRestore::new(py, &inspect, added_name)?;
                let _inspect_restore = TestAttributeRestore::new(py, &inspect, "signature")?;
                let _builtins_restore = TestAttributeRestore::new(py, &builtins, "property")?;
                if inspect.hasattr(added_name)? {
                    inspect.delattr(added_name)?;
                }

                let canonical_signature = inspect.getattr("signature")?;
                let poison = py.eval(pyo3::ffi::c_str!("lambda value: value"), None, None)?;
                let marker = py.eval(pyo3::ffi::c_str!("object()"), None, None)?;
                inspect.setattr("signature", &poison)?;

                let globals = PyDict::new(py);
                globals.set_item("inspect", &inspect)?;
                globals.set_item("builtins", &builtins)?;
                globals.set_item("marker", &marker)?;
                globals.set_item("finalized", false)?;
                py.run(
                    pyo3::ffi::c_str!(
                        r#"
class Bomb:
    def __del__(self):
        global finalized
        finalized = True
        inspect.signature = marker
        inspect.__pyo3_bindgen_added_during_canonicalization = marker
"#
                    ),
                    Some(&globals),
                    None,
                )?;

                let canonical_observation: PyResult<(bool, bool, bool)> = with_restored(py, || {
                    py.run(
                        pyo3::ffi::c_str!("builtins.property = Bomb()"),
                        Some(&globals),
                        None,
                    )?;
                    // Replacing the operation-created Bomb releases its last reference.
                    // Its finalizer mutates inspect, so this accessor must run a second
                    // exact-canonicalization pass before returning.
                    let _ = inspect_signature(py)?;
                    Ok((
                        globals
                            .get_item("finalized")?
                            .expect("finalized sentinel exists")
                            .extract::<bool>()?,
                        inspect.getattr("signature")?.is(&canonical_signature),
                        !inspect.hasattr(added_name)?,
                    ))
                });
                let (finalizer_ran, signature_was_canonical, added_entry_was_absent) =
                    canonical_observation?;

                assert!(
                    finalizer_ran && signature_was_canonical && added_entry_was_absent,
                    "a displaced-value finalizer corrupted the canonical operation view",
                );
                assert!(
                    inspect.getattr("signature")?.is(&poison) && !inspect.hasattr(added_name)?,
                    "cleanup did not restore the target-visible entry state",
                );
                Ok(())
            })
            .unwrap();
        });
    }

    #[test]
    fn cleanup_tolerates_already_missing_keys_and_restores_later_modules() {
        pyo3::Python::initialize();
        pyo3::Python::attach(|py| {
            crate::io_utils::with_suppressed_python_output(py, false, false, || {
                prime(py)?;
                let inspect = py.import(pyo3::intern!(py, "inspect"))?;
                let builtins = py.import(pyo3::intern!(py, "builtins"))?;
                let _inspect_restore = TestAttributeRestore::new(py, &inspect, "signature")?;
                let _builtins_restore = TestAttributeRestore::new(py, &builtins, "property")?;

                inspect.delattr(pyo3::intern!(py, "signature"))?;
                let poison = py.get_type::<PyAny>();
                builtins.setattr(pyo3::intern!(py, "property"), &poison)?;

                let result: PyResult<()> = with_restored(py, || {
                    inspect.delattr(pyo3::intern!(py, "signature"))?;
                    Ok(())
                });
                result?;

                assert!(
                    !inspect.hasattr(pyo3::intern!(py, "signature"))?,
                    "an originally-missing key must stay missing",
                );
                assert!(
                    builtins
                        .getattr(pyo3::intern!(py, "property"))?
                        .is(poison.as_any()),
                    "an already-missing earlier key aborted restoration of a later module",
                );
                Ok(())
            })
            .unwrap();
        });
    }

    #[cfg(not(any(PyPy, GraalPy)))]
    #[test]
    fn cleanup_reasserts_after_displaced_value_finalizers() {
        pyo3::Python::initialize();
        pyo3::Python::attach(|py| {
            crate::io_utils::with_suppressed_python_output(py, false, false, || {
                prime(py)?;
                let inspect = py.import(pyo3::intern!(py, "inspect"))?;
                let builtins = py.import(pyo3::intern!(py, "builtins"))?;
                let _inspect_restore = TestAttributeRestore::new(py, &inspect, "signature")?;
                let _builtins_restore = TestAttributeRestore::new(py, &builtins, "property")?;
                let added_name = "__pyo3_bindgen_added_by_finalizer";
                let _added_restore = TestAttributeRestore::new(py, &inspect, added_name)?;
                if inspect.hasattr(added_name)? {
                    inspect.delattr(added_name)?;
                }

                let poison = py.eval(pyo3::ffi::c_str!("lambda value: value"), None, None)?;
                let marker = py.eval(pyo3::ffi::c_str!("object()"), None, None)?;
                inspect.setattr(pyo3::intern!(py, "signature"), &poison)?;

                let globals = PyDict::new(py);
                globals.set_item("inspect", &inspect)?;
                globals.set_item("builtins", &builtins)?;
                globals.set_item("marker", &marker)?;
                globals.set_item("finalized", false)?;

                let result: PyResult<()> = with_restored(py, || {
                    py.run(
                        pyo3::ffi::c_str!(
                            r#"
class Bomb:
    def __del__(self):
        global finalized
        finalized = True
        inspect.signature = marker
        inspect.__pyo3_bindgen_added_by_finalizer = marker

builtins.property = Bomb()
"#
                        ),
                        Some(&globals),
                        None,
                    )
                });
                result?;

                assert!(
                    globals
                        .get_item("finalized")?
                        .expect("finalized sentinel exists")
                        .extract::<bool>()?,
                    "the displaced value's finalizer did not exercise the regression",
                );
                assert!(
                    inspect.getattr(pyo3::intern!(py, "signature"))?.is(&poison),
                    "a displaced value finalizer mutated an already-restored module",
                );
                assert!(
                    !inspect.hasattr(added_name)?,
                    "a displaced value finalizer left a newly added module attribute",
                );
                Ok(())
            })
            .unwrap();
        });
    }

    #[cfg(not(any(PyPy, GraalPy)))]
    #[test]
    fn cleanup_failure_takes_precedence_over_operation_failure() {
        pyo3::Python::initialize();
        pyo3::Python::attach(|py| {
            crate::io_utils::with_suppressed_python_output(py, false, false, || {
                prime(py)?;
                let inspect = py.import(pyo3::intern!(py, "inspect"))?;
                let builtins = py.import(pyo3::intern!(py, "builtins"))?;
                let _inspect_restore = TestAttributeRestore::new(py, &inspect, "signature")?;
                let _builtins_restore = TestAttributeRestore::new(py, &builtins, "property")?;

                let original_property = builtins.getattr("property")?;
                let poison = py.eval(pyo3::ffi::c_str!("lambda value: value"), None, None)?;
                let marker = py.eval(pyo3::ffi::c_str!("object()"), None, None)?;
                inspect.setattr("signature", &poison)?;
                let globals = unstable_finalizer_globals(py, &inspect, &builtins, &marker)?;

                let result: PyResult<()> = with_restored(py, || -> PyResult<()> {
                    py.run(
                        pyo3::ffi::c_str!("builtins.property = Bomb()"),
                        Some(&globals),
                        None,
                    )?;
                    Err(pyo3::exceptions::PyValueError::new_err(
                        "ordinary operation failure",
                    ))
                });
                let cleanup_error = result.err();
                disarm_finalizer_chain(&globals, &inspect, &builtins, &original_property, &poison)?;

                let cleanup_error =
                    cleanup_error.expect("an unstable cleanup must fail the operation");
                assert!(
                    cleanup_error.is_instance_of::<PyRuntimeError>(py),
                    "cleanup failure was hidden by the ordinary operation error: {cleanup_error}",
                );
                assert!(
                    cleanup_error.to_string().contains("did not stabilize"),
                    "unexpected cleanup failure: {cleanup_error}",
                );
                Ok(())
            })
            .unwrap();
        });
    }

    #[cfg(not(any(PyPy, GraalPy)))]
    #[test]
    fn panic_cleanup_reports_non_stabilization_as_unraisable() {
        pyo3::Python::initialize();
        pyo3::Python::attach(|py| {
            crate::io_utils::with_suppressed_python_output(py, false, false, || {
                prime(py)?;
                let inspect = py.import(pyo3::intern!(py, "inspect"))?;
                let builtins = py.import(pyo3::intern!(py, "builtins"))?;
                let sys = py.import(pyo3::intern!(py, "sys"))?;
                let _inspect_restore = TestAttributeRestore::new(py, &inspect, "signature")?;
                let _builtins_restore = TestAttributeRestore::new(py, &builtins, "property")?;
                let _hook_restore = TestAttributeRestore::new(py, &sys, "unraisablehook")?;

                let original_property = builtins.getattr("property")?;
                let poison = py.eval(pyo3::ffi::c_str!("lambda value: value"), None, None)?;
                let marker = py.eval(pyo3::ffi::c_str!("object()"), None, None)?;
                inspect.setattr("signature", &poison)?;
                let globals = unstable_finalizer_globals(py, &inspect, &builtins, &marker)?;

                let events = pyo3::types::PyList::empty(py);
                let hook_globals = PyDict::new(py);
                hook_globals.set_item("events", &events)?;
                let hook = py.eval(
                    pyo3::ffi::c_str!("lambda args: events.append(args.exc_value)"),
                    Some(&hook_globals),
                    None,
                )?;
                sys.setattr("unraisablehook", hook)?;

                let unwind = catch_unwind(AssertUnwindSafe(|| {
                    let _: PyResult<()> = with_restored(py, || -> PyResult<()> {
                        py.run(
                            pyo3::ffi::c_str!("builtins.property = Bomb()"),
                            Some(&globals),
                            None,
                        )?;
                        panic!("intentional unstable-cleanup panic")
                    });
                }));
                disarm_finalizer_chain(&globals, &inspect, &builtins, &original_property, &poison)?;

                assert!(unwind.is_err(), "the regression operation must panic");
                assert_eq!(
                    events.len(),
                    1,
                    "panic cleanup must report exactly one unraisable error",
                );
                let cleanup_error = events.get_item(0)?;
                assert!(
                    cleanup_error.is_instance_of::<PyRuntimeError>(),
                    "unexpected unraisable cleanup error: {cleanup_error:?}",
                );
                assert!(
                    cleanup_error
                        .str()?
                        .to_string_lossy()
                        .contains("did not stabilize"),
                    "unexpected unraisable cleanup error: {cleanup_error:?}",
                );
                Ok(())
            })
            .unwrap();
        });
    }
}
