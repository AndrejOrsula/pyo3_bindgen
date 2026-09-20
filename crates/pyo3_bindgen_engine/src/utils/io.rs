use crate::Result;
use pyo3::prelude::*;
use std::{
    marker::PhantomData,
    rc::Rc,
    sync::{Condvar, Mutex},
    time::{Duration, Instant},
};

/// Upper bound on how long `acquire` waits for a lock held by a *different*
/// thread before giving up. This is a last-resort safety net against a
/// genuine deadlock, not a mechanism for detecting reentrant cycles: legitimate
/// concurrent introspection of independent, realistically large modules on
/// separate threads (e.g. multiple `import_python!` expansions, or a build
/// script parallelizing codegen across target modules) must be able to wait
/// this long without spuriously failing. Same-thread reentrancy never waits
/// at all (see `acquire`), so it is unaffected by this timeout.
const PYTHON_OUTPUT_CROSS_THREAD_TIMEOUT: Duration = Duration::from_secs(300);

static PYTHON_OUTPUT_LOCK: PythonOutputLock = PythonOutputLock {
    state: Mutex::new(PythonOutputLockState {
        owner: None,
        depth: 0,
    }),
    available: Condvar::new(),
};

struct PythonOutputLock {
    state: Mutex<PythonOutputLockState>,
    available: Condvar,
}

struct PythonOutputLockState {
    owner: Option<std::thread::ThreadId>,
    depth: usize,
}

impl PythonOutputLock {
    fn acquire(&self) -> Result<std::thread::ThreadId> {
        let owner = std::thread::current().id();
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // A free lock or same-thread reentrancy proceeds immediately: there is
        // nothing to wait for, and a thread re-entering a lock it already
        // holds could never resolve by waiting anyway. This is the genuine
        // reentrant-cycle fast path and must stay instantaneous.
        if state
            .owner
            .as_ref()
            .is_some_and(|current| current != &owner)
        {
            // Held by a genuinely different thread: ordinary cross-thread
            // contention, not a cycle. Wait generously (bounded only by a
            // last-resort safety-net timeout) rather than failing quickly,
            // since the other thread may simply be making slow-but-legitimate
            // progress on an unrelated, realistically large module.
            let wait_started = Instant::now();
            let deadline = wait_started + PYTHON_OUTPUT_CROSS_THREAD_TIMEOUT;
            tracing::debug!(
                observability_schema = 1,
                operation_id = "unavailable",
                component = "python_runtime_coordination",
                outcome = "contended",
                waited_milliseconds = 0,
                timeout_milliseconds = PYTHON_OUTPUT_CROSS_THREAD_TIMEOUT.as_millis(),
                "waiting for process-wide Python runtime coordination"
            );
            while state
                .owner
                .as_ref()
                .is_some_and(|current| current != &owner)
            {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    tracing::error!(
                        observability_schema = 1,
                        operation_id = "unavailable",
                        component = "python_runtime_coordination",
                        outcome = "timeout",
                        waited_milliseconds = wait_started.elapsed().as_millis(),
                        "Python runtime coordination timed out"
                    );
                    return Err(crate::PyBindgenError::CodegenError(
                        "Python output coordination is already active on another thread".to_owned(),
                    ));
                }
                (state, _) = self
                    .available
                    .wait_timeout(state, remaining)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }
            tracing::debug!(
                observability_schema = 1,
                operation_id = "unavailable",
                component = "python_runtime_coordination",
                outcome = "acquired",
                waited_milliseconds = wait_started.elapsed().as_millis(),
                "acquired process-wide Python runtime coordination"
            );
        }
        state.owner = Some(owner);
        state.depth += 1;
        Ok(owner)
    }

    fn release(&self, owner: &std::thread::ThreadId) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        debug_assert_eq!(state.owner.as_ref(), Some(owner));
        state.depth -= 1;
        if state.depth == 0 {
            state.owner = None;
            self.available.notify_one();
        }
    }
}

pub(crate) struct PythonOutputPermit {
    owner: std::thread::ThreadId,
    _thread_bound: PhantomData<Rc<()>>,
}

impl PythonOutputPermit {
    fn new(owner: std::thread::ThreadId) -> Self {
        Self {
            owner,
            _thread_bound: PhantomData,
        }
    }
}

impl Drop for PythonOutputPermit {
    fn drop(&mut self) {
        PYTHON_OUTPUT_LOCK.release(&self.owner);
    }
}

/// Acquire the process-wide live-Python coordination lock for stdlib snapshot access.
///
/// Runtime sentinel readers use the same lock as output redirection so snapshot capture,
/// temporary stdlib restoration, and process-global stream replacement cannot overlap on
/// different threads. The lock is reentrant on the current thread.
pub(crate) fn acquire_python_runtime(py: pyo3::Python<'_>) -> PyResult<PythonOutputPermit> {
    let owner = py
        .detach(|| PYTHON_OUTPUT_LOCK.acquire())
        .map_err(|error| pyo3::exceptions::PyRuntimeError::new_err(error.to_string()))?;
    Ok(PythonOutputPermit::new(owner))
}

pub fn with_suppressed_python_output<T>(
    py: pyo3::Python,
    suppress_stdout: bool,
    suppress_stderr: bool,
    f: impl FnOnce() -> Result<T>,
) -> Result<T> {
    // Every call must participate because an unsuppressed callback would otherwise
    // run while another thread has redirected the process-global streams.
    // Same-thread nesting is reentrant and returns immediately. Calls from a
    // different thread wait for the lock to free up -- bounded only by a
    // generous last-resort timeout, not a short one -- since legitimate
    // concurrent introspection of independent modules can take a while;
    // only a thread re-entering its own held lock is a genuine cycle.
    let owner = py.detach(|| PYTHON_OUTPUT_LOCK.acquire())?;
    let _output_permit = PythonOutputPermit::new(owner);

    if !suppress_stdout && !suppress_stderr {
        return f();
    }

    let sys = py.import(pyo3::intern!(py, "sys"))?;
    let stdout_ident = pyo3::intern!(py, "stdout");
    let stderr_ident = pyo3::intern!(py, "stderr");

    // Prepare the dummy object that silently discards all output
    let suppressed_output = py.eval(
        pyo3::ffi::c_str!(
            r"type('SuppressedOutput', (), {'write': lambda self, x: None, 'flush': lambda self: None})()"
        ),
        None,
        None,
    )?;

    // Record the original streams and suppress the requested output. Normal
    // returns use explicit fallible cleanup; Drop remains the best-effort path
    // for setup failures and panics.
    let mut guard = StreamRestoreGuard {
        sys,
        stdout: None,
        stderr: None,
    };
    if suppress_stdout {
        let original = guard.sys.getattr(stdout_ident)?;
        guard.sys.setattr(stdout_ident, &suppressed_output)?;
        guard.stdout = Some((stdout_ident, original));
    }
    if suppress_stderr {
        let original = guard.sys.getattr(stderr_ident)?;
        guard.sys.setattr(stderr_ident, &suppressed_output)?;
        guard.stderr = Some((stderr_ident, original));
    }

    // Run the function, then explicitly finish the guard to restore the streams
    // before returning the result. A callback failure and a cleanup failure are
    // both actionable, so retain both messages when they occur together.
    let callback_result = f();
    let cleanup_result = guard.finish();
    match (callback_result, cleanup_result) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) => Err(error),
        (Ok(value), Err(error)) => {
            drop(value);
            Err(error.into())
        }
        (Err(callback_error), Err(cleanup_error)) => Err(crate::PyBindgenError::CodegenError(
            format!(
                "Python output callback failed: {callback_error}; stream restoration failed: {cleanup_error}"
            ),
        )),
    }
}

/// Guard for restoring `sys.stdout`/`sys.stderr` to their original objects.
/// Explicit `finish` reports cleanup errors on normal returns; Drop provides
/// best-effort cleanup for setup failures and unwinding.
struct StreamRestoreGuard<'py> {
    sys: pyo3::Bound<'py, pyo3::types::PyModule>,
    stdout: Option<(
        &'py pyo3::Bound<'py, pyo3::types::PyString>,
        pyo3::Bound<'py, pyo3::types::PyAny>,
    )>,
    stderr: Option<(
        &'py pyo3::Bound<'py, pyo3::types::PyString>,
        pyo3::Bound<'py, pyo3::types::PyAny>,
    )>,
}

impl StreamRestoreGuard<'_> {
    fn finish(mut self) -> PyResult<()> {
        // Take every entry before attempting restoration. This guarantees that
        // Drop cannot retry an entry after its explicit fallible attempt, while
        // still allowing the second selected stream to be attempted if the first
        // one fails.
        let stdout = self.stdout.take();
        let stderr = self.stderr.take();
        let mut errors = Vec::new();

        if let Some((ident, original)) = stdout {
            if let Err(error) = self.sys.setattr(ident, &original) {
                tracing::error!(
                    observability_schema = 1,
                    operation_id = "unavailable",
                    component = "python_stream_restoration",
                    stream = "stdout",
                    outcome = "failed",
                    restore_passes = 1,
                    "failed to restore Python stream"
                );
                tracing::debug!(error = %error, "Python stream restoration failure detail");
                errors.push(("stdout", error));
            }
        }
        if let Some((ident, original)) = stderr {
            if let Err(error) = self.sys.setattr(ident, &original) {
                tracing::error!(
                    observability_schema = 1,
                    operation_id = "unavailable",
                    component = "python_stream_restoration",
                    stream = "stderr",
                    outcome = "failed",
                    restore_passes = 1,
                    "failed to restore Python stream"
                );
                tracing::debug!(error = %error, "Python stream restoration failure detail");
                errors.push(("stderr", error));
            }
        }

        match errors.len() {
            0 => Ok(()),
            1 => Err(errors.pop().expect("one restoration error exists").1),
            _ => {
                let details = errors
                    .into_iter()
                    .map(|(stream, error)| format!("{stream}: {error}"))
                    .collect::<Vec<_>>()
                    .join("; ");
                Err(pyo3::exceptions::PyRuntimeError::new_err(format!(
                    "failed to restore Python streams: {details}"
                )))
            }
        }
    }
}

impl Drop for StreamRestoreGuard<'_> {
    fn drop(&mut self) {
        // Drop is only the best-effort path for setup failures and unwinding;
        // finish() takes completed entries so they are never retried here.
        if let Some((ident, original)) = self.stdout.take() {
            if let Err(error) = self.sys.setattr(ident, &original) {
                tracing::error!(
                    observability_schema = 1,
                    operation_id = "unavailable",
                    component = "python_stream_restoration",
                    stream = "stdout",
                    outcome = "failed",
                    restore_passes = 1,
                    "failed to restore Python stream"
                );
                tracing::debug!(error = %error, "Python stream restoration failure detail");
            }
        }
        if let Some((ident, original)) = self.stderr.take() {
            if let Err(error) = self.sys.setattr(ident, &original) {
                tracing::error!(
                    observability_schema = 1,
                    operation_id = "unavailable",
                    component = "python_stream_restoration",
                    stream = "stderr",
                    outcome = "failed",
                    restore_passes = 1,
                    "failed to restore Python stream"
                );
                tracing::debug!(error = %error, "Python stream restoration failure detail");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pyo3::types::{PyAnyMethods, PyDict, PyDictMethods, PyModule};
    use std::{
        sync::{
            atomic::{AtomicBool, Ordering},
            mpsc, Arc, Mutex,
        },
        time::Duration,
    };

    static PYTHON_OUTPUT_TEST_LOCK: Mutex<()> = Mutex::new(());

    struct SysStateGuard<'py> {
        sys: Bound<'py, PyModule>,
        original_class: Bound<'py, PyAny>,
        original_stdout: Bound<'py, PyAny>,
        original_stderr: Bound<'py, PyAny>,
    }

    impl<'py> SysStateGuard<'py> {
        fn new(sys: Bound<'py, PyModule>) -> Self {
            Self {
                original_class: sys.getattr("__class__").unwrap(),
                original_stdout: sys.getattr("stdout").unwrap(),
                original_stderr: sys.getattr("stderr").unwrap(),
                sys,
            }
        }

        fn install_one_shot_failure(&self, fail_name: &str) -> Bound<'py, PyAny> {
            self.install_one_shot_failures(&[fail_name])
        }

        fn install_one_shot_failures(&self, fail_names: &[&str]) -> Bound<'py, PyAny> {
            let py = self.sys.py();
            let globals = PyDict::new(py);
            globals.set_item("fail_names", fail_names).unwrap();
            globals.set_item("sys", &self.sys).unwrap();
            py.run(
                pyo3::ffi::c_str!(
                    r#"
class OneShotHostileSys(type(sys)):
    armed = False
    remaining_failures = set(fail_names)

    def __setattr__(self, name, value):
        if type(self).armed and name in type(self).remaining_failures:
            type(self).remaining_failures.remove(name)
            if not type(self).remaining_failures:
                type(self).armed = False
            raise RuntimeError("hostile sys.__setattr__ failed for " + name)
        return super().__setattr__(name, value)
"#
                ),
                Some(&globals),
                None,
            )
            .unwrap();
            let hostile_class = globals
                .get_item("OneShotHostileSys")
                .unwrap()
                .expect("hostile sys class exists");
            self.sys.setattr("__class__", &hostile_class).unwrap();
            hostile_class
        }

        fn stream_is_original(&self, name: &str, original: &Bound<'py, PyAny>) -> bool {
            self.sys.getattr(name).unwrap().is(original)
        }
    }

    impl Drop for SysStateGuard<'_> {
        fn drop(&mut self) {
            // Restore the module type first so even a still-armed hostile
            // __setattr__ cannot prevent the stream cleanup below. This is a
            // test-only last line of defense for assertion/unwind paths.
            let _ = self.sys.setattr("__class__", &self.original_class);
            let _ = self.sys.setattr("stdout", &self.original_stdout);
            let _ = self.sys.setattr("stderr", &self.original_stderr);
        }
    }

    #[test]
    fn cleanup_failure_is_propagated_and_other_stream_is_restored() {
        let _test_guard = PYTHON_OUTPUT_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        pyo3::Python::initialize();
        pyo3::Python::attach(|py| {
            let owner = py.detach(|| PYTHON_OUTPUT_LOCK.acquire()).unwrap();
            let _permit = PythonOutputPermit::new(owner);
            let state = SysStateGuard::new(py.import("sys").unwrap());
            let hostile = state.install_one_shot_failure("stdout");

            let result: crate::Result<()> = with_suppressed_python_output(py, true, true, || {
                hostile.setattr("armed", true)?;
                Ok(())
            });

            let error = result.expect_err("stream cleanup failure must be returned");
            let error = error.to_string();
            assert!(
                error.contains("hostile sys.__setattr__ failed for stdout"),
                "unexpected cleanup error: {error}"
            );
            assert!(
                !state.stream_is_original("stdout", &state.original_stdout),
                "Drop must not retry a stream already attempted by finish"
            );
            assert!(
                state.stream_is_original("stderr", &state.original_stderr),
                "finish must attempt the other selected stream after stdout fails"
            );
        });
    }

    #[test]
    fn both_cleanup_failures_are_aggregated_without_drop_retry() {
        let _test_guard = PYTHON_OUTPUT_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        pyo3::Python::initialize();
        pyo3::Python::attach(|py| {
            let owner = py.detach(|| PYTHON_OUTPUT_LOCK.acquire()).unwrap();
            let _permit = PythonOutputPermit::new(owner);
            let state = SysStateGuard::new(py.import("sys").unwrap());
            let hostile = state.install_one_shot_failures(&["stdout", "stderr"]);

            let result: crate::Result<()> = with_suppressed_python_output(py, true, true, || {
                hostile.setattr("armed", true)?;
                Ok(())
            });

            let error = result
                .expect_err("both stream cleanup failures must be returned")
                .to_string();
            assert!(
                error.contains("stdout")
                    && error.contains("hostile sys.__setattr__ failed for stdout"),
                "stdout cleanup error was lost: {error}"
            );
            assert!(
                error.contains("stderr")
                    && error.contains("hostile sys.__setattr__ failed for stderr"),
                "stderr cleanup error was lost: {error}"
            );
            assert!(
                !state.stream_is_original("stdout", &state.original_stdout),
                "Drop must not retry completed stdout cleanup"
            );
            assert!(
                !state.stream_is_original("stderr", &state.original_stderr),
                "Drop must not retry completed stderr cleanup"
            );
        });
    }

    #[test]
    fn callback_and_all_cleanup_failures_preserve_every_error_message() {
        let _test_guard = PYTHON_OUTPUT_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        pyo3::Python::initialize();
        pyo3::Python::attach(|py| {
            let owner = py.detach(|| PYTHON_OUTPUT_LOCK.acquire()).unwrap();
            let _permit = PythonOutputPermit::new(owner);
            let state = SysStateGuard::new(py.import("sys").unwrap());
            let hostile = state.install_one_shot_failures(&["stdout", "stderr"]);

            let result: crate::Result<()> = with_suppressed_python_output(py, true, true, || {
                hostile.setattr("armed", true)?;
                Err(crate::PyBindgenError::CodegenError(
                    "callback failure sentinel".to_owned(),
                ))
            });

            let error = result.expect_err("callback failure must be returned");
            let error = error.to_string();
            assert!(
                error.contains("callback failure sentinel"),
                "callback error was lost: {error}"
            );
            assert!(
                error.contains("hostile sys.__setattr__ failed for stdout"),
                "stdout cleanup error was lost: {error}"
            );
            assert!(
                error.contains("hostile sys.__setattr__ failed for stderr"),
                "stderr cleanup error was lost: {error}"
            );
            assert!(
                !state.stream_is_original("stdout", &state.original_stdout),
                "Drop must not retry a stream already attempted by finish"
            );
            assert!(
                !state.stream_is_original("stderr", &state.original_stderr),
                "Drop must not retry completed stderr cleanup"
            );
        });
    }

    #[test]
    fn callback_and_one_cleanup_failure_preserve_public_error_precedence() {
        let _test_guard = PYTHON_OUTPUT_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        pyo3::Python::initialize();
        pyo3::Python::attach(|py| {
            let owner = py.detach(|| PYTHON_OUTPUT_LOCK.acquire()).unwrap();
            let _permit = PythonOutputPermit::new(owner);
            let state = SysStateGuard::new(py.import("sys").unwrap());
            let hostile = state.install_one_shot_failure("stdout");

            let result: crate::Result<()> = with_suppressed_python_output(py, true, true, || {
                hostile.setattr("armed", true)?;
                Err(crate::PyBindgenError::CodegenError(
                    "callback failure precedence sentinel".to_owned(),
                ))
            });

            let error = match result {
                Err(crate::PyBindgenError::CodegenError(error)) => error,
                Err(error) => panic!("unexpected public error variant: {error}"),
                Ok(()) => panic!("callback failure must be returned"),
            };
            let callback_position = error
                .find("callback failure precedence sentinel")
                .expect("callback error must be retained");
            let cleanup_position = error
                .find("hostile sys.__setattr__ failed for stdout")
                .expect("stream cleanup error must be retained");
            assert!(
                callback_position < cleanup_position,
                "public error precedence changed: {error}"
            );
            assert!(
                error.starts_with(
                    "Python output callback failed: Failed to generate Rust code: \
                     callback failure precedence sentinel; \
                     stream restoration failed:"
                ),
                "unexpected combined public error: {error}"
            );
            assert!(
                !state.stream_is_original("stdout", &state.original_stdout),
                "Drop must not retry completed stdout cleanup"
            );
            assert!(
                state.stream_is_original("stderr", &state.original_stderr),
                "the other selected stream must still be restored"
            );
        });
    }

    #[test]
    fn partial_setup_failure_restores_an_already_replaced_stream() {
        let _test_guard = PYTHON_OUTPUT_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        pyo3::Python::initialize();
        pyo3::Python::attach(|py| {
            let owner = py.detach(|| PYTHON_OUTPUT_LOCK.acquire()).unwrap();
            let _permit = PythonOutputPermit::new(owner);
            let state = SysStateGuard::new(py.import("sys").unwrap());
            let hostile = state.install_one_shot_failure("stderr");
            hostile.setattr("armed", true).unwrap();

            let result: crate::Result<()> =
                with_suppressed_python_output(py, true, true, || Ok(()));

            let error = result.expect_err("partial setup failure must be returned");
            let error = error.to_string();
            assert!(
                error.contains("hostile sys.__setattr__ failed for stderr"),
                "unexpected setup error: {error}"
            );
            assert!(
                state.stream_is_original("stdout", &state.original_stdout),
                "setup Drop must restore stdout after stderr setup fails"
            );
            assert!(
                state.stream_is_original("stderr", &state.original_stderr),
                "stderr must remain unchanged when its replacement fails"
            );
        });
    }

    #[test]
    fn embedded_source_execution_honors_stdout_suppression() {
        let _test_guard = PYTHON_OUTPUT_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        pyo3::Python::initialize();
        pyo3::Python::attach(|py| {
            // Keep the global ordering invariant used by production callers: never
            // wait for the runtime lock while retaining the interpreter lock, or a
            // current owner that detached before acquiring could be unable to reattach
            // and release it.
            let owner = py.detach(|| PYTHON_OUTPUT_LOCK.acquire()).unwrap();
            let _permit = PythonOutputPermit::new(owner);
            let sys = py.import("sys").unwrap();
            let original_stdout = sys.getattr("stdout").unwrap();

            let suppressed_capture = py.import("io").unwrap().call_method0("StringIO").unwrap();
            sys.setattr("stdout", &suppressed_capture).unwrap();
            crate::Codegen::default()
                .module_from_str(
                    "print('embedded source output')\nvalue = 1",
                    "suppressed_source_output",
                )
                .unwrap();
            let suppressed = suppressed_capture
                .call_method0("getvalue")
                .unwrap()
                .extract::<String>()
                .unwrap();

            let visible_capture = py.import("io").unwrap().call_method0("StringIO").unwrap();
            sys.setattr("stdout", &visible_capture).unwrap();
            crate::Codegen::new(crate::Config {
                suppress_python_stdout: false,
                suppress_python_stderr: false,
                ..crate::Config::default()
            })
            .module_from_str(
                "print('embedded source output')\nvalue = 1",
                "visible_source_output",
            )
            .unwrap();
            let visible = visible_capture
                .call_method0("getvalue")
                .unwrap()
                .extract::<String>()
                .unwrap();

            sys.setattr("stdout", original_stdout).unwrap();
            assert_eq!(suppressed, "");
            assert_eq!(visible, "embedded source output\n");
        });
    }

    #[test]
    fn concurrent_call_on_different_thread_waits_instead_of_failing() {
        // Cross-thread contention on the lock is ordinary concurrent usage
        // (e.g. two independent modules being introspected in parallel), not
        // a reentrant cycle, so a contending call from a different thread
        // must wait for the lock rather than failing quickly.
        let _test_guard = PYTHON_OUTPUT_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        pyo3::Python::initialize();

        let (redirected_tx, redirected_rx) = mpsc::channel();
        let (restore_tx, restore_rx) = mpsc::channel();
        let suppressing_thread = std::thread::spawn(move || {
            pyo3::Python::attach(|py| {
                with_suppressed_python_output(py, true, false, || {
                    let stdout_type = py
                        .import("sys")?
                        .getattr("stdout")?
                        .get_type()
                        .name()?
                        .to_string();
                    py.detach(move || {
                        redirected_tx.send(stdout_type).unwrap();
                        restore_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                    });
                    Ok(())
                })
                .unwrap();
            });
        });

        assert_eq!(
            redirected_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            "SuppressedOutput"
        );

        let ran = Arc::new(AtomicBool::new(false));
        let contender_ran = Arc::clone(&ran);
        let (result_tx, result_rx) = mpsc::channel();
        let contending_thread = std::thread::spawn(move || {
            let result = pyo3::Python::attach(|py| {
                with_suppressed_python_output(py, false, false, || {
                    contender_ran.store(true, Ordering::SeqCst);
                    Ok(())
                })
            });
            result_tx
                .send(result.map_err(|error| error.to_string()))
                .unwrap();
        });

        // While the other thread still holds the lock, the contending call
        // must still be waiting, not have already failed.
        assert!(
            result_rx.recv_timeout(Duration::from_millis(300)).is_err(),
            "contending call on a different thread returned before the lock was released; \
             it should wait, not fail promptly"
        );
        assert!(!ran.load(Ordering::SeqCst));

        // Release the lock and let the previously-waiting contender proceed.
        restore_tx.send(()).unwrap();
        suppressing_thread.join().unwrap();

        let result = result_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("contending call did not complete after the lock was released");
        contending_thread.join().unwrap();

        result.expect("contending call should succeed once the lock becomes free");
        assert!(ran.load(Ordering::SeqCst));
    }
}
