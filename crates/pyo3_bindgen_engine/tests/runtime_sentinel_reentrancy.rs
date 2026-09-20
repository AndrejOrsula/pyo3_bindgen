use pyo3::{
    exceptions::PyRuntimeError,
    prelude::*,
    types::{PyAnyMethods, PyCFunction, PyDict, PyDictMethods, PyList, PyListMethods, PyModule},
};
use pyo3_bindgen_engine::Codegen;
use std::{
    env, fs,
    fs::File,
    io::{self, Write},
    path::PathBuf,
    process::{Child, Command, ExitStatus, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

#[path = "../test_support/bounded_consumer.rs"]
mod bounded_consumer;
use bounded_consumer::OwnedTempDir as ChildOutputDir;

const CHILD_ENV: &str = "PYO3_BINDGEN_RUNTIME_SENTINEL_REENTRANCY_CHILD";
const CHILD_ROLE: &str = "pyo3-bindgen-runtime-sentinel-reentrancy-child-v1";
const HOOK_MARKER_ENV: &str = "PYO3_BINDGEN_RUNTIME_SENTINEL_REENTRANCY_HOOK_MARKER";
const CHILD_ERROR_EXIT: i32 = 73;
const CHILD_TIMEOUT: Duration = Duration::from_secs(10);
const REAP_TIMEOUT: Duration = Duration::from_secs(3);

const FINDER_SOURCE: &str = r#"
class ReentrantFinder:
    def __init__(self, callback):
        self.callback = callback
        self.triggered = False
        self.nested_result = None

    def find_spec(self, fullname, path=None, target=None):
        if fullname == "inspect" and not self.triggered:
            self.triggered = True
            self.nested_result = self.callback()
            raise RuntimeError(self.nested_result)
        return None
"#;

#[derive(Default)]
struct ChildCleanup {
    status: Option<ExitStatus>,
    kill_error: Option<String>,
    reap_error: Option<String>,
}

fn record_child_error(slot: &mut Option<String>, error: impl Into<String>) {
    let error = error.into();
    if let Some(existing) = slot {
        existing.push_str("; ");
        existing.push_str(&error);
    } else {
        *slot = Some(error);
    }
}

struct ChildMonitor {
    child: Option<Child>,
}

impl ChildMonitor {
    fn new(child: Child) -> Self {
        Self { child: Some(child) }
    }

    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        match self.child.as_mut() {
            Some(child) => child.try_wait(),
            None => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "child ownership was already released",
            )),
        }
    }

    fn release_reaped(&mut self) {
        let _ = self.child.take();
    }

    fn force_kill_and_reap(&mut self) -> ChildCleanup {
        let mut cleanup = ChildCleanup::default();
        if self.child.is_none() {
            record_child_error(
                &mut cleanup.reap_error,
                "child ownership was already released before cleanup",
            );
            return cleanup;
        }

        if let Some(error) = self.child.as_mut().and_then(|child| child.kill().err()) {
            record_child_error(&mut cleanup.kill_error, error.to_string());
        }

        let reap_deadline = Instant::now() + REAP_TIMEOUT;
        cleanup.status = loop {
            let wait_result = match self.child.as_mut() {
                Some(child) => child.try_wait(),
                None => {
                    record_child_error(
                        &mut cleanup.reap_error,
                        "child ownership disappeared during cleanup",
                    );
                    break None;
                }
            };

            match wait_result {
                Ok(Some(status)) => break Some(status),
                Ok(None) if Instant::now() < reap_deadline => {
                    thread::sleep(Duration::from_millis(10));
                }
                Ok(None) => {
                    record_child_error(
                        &mut cleanup.reap_error,
                        format!(
                            "child was not reaped within {} seconds after kill",
                            REAP_TIMEOUT.as_secs()
                        ),
                    );
                    break None;
                }
                Err(error) => {
                    record_child_error(
                        &mut cleanup.reap_error,
                        format!("child status check failed while reaping: {error}"),
                    );
                    if let Some(kill_error) =
                        self.child.as_mut().and_then(|child| child.kill().err())
                    {
                        record_child_error(&mut cleanup.kill_error, kill_error.to_string());
                    }
                    if Instant::now() < reap_deadline {
                        thread::sleep(Duration::from_millis(10));
                    } else {
                        record_child_error(
                            &mut cleanup.reap_error,
                            format!(
                                "child was not reaped within {} seconds after kill",
                                REAP_TIMEOUT.as_secs()
                            ),
                        );
                        break None;
                    }
                }
            }
        };

        if cleanup.status.is_some() {
            let _ = self.child.take();
        } else if let Some(mut child) = self.child.take() {
            // Transfer ownership to a hard reaper. The monitor remains bounded,
            // while a slow OS wait cannot make a live Child reach Drop or be forgotten.
            let (sender, receiver) = mpsc::sync_channel(1);
            let reaper = thread::spawn(move || {
                let result = child.wait().map_err(|error| error.to_string());
                let _ = sender.send(result);
            });
            match receiver.recv_timeout(REAP_TIMEOUT) {
                Ok(Ok(status)) => {
                    cleanup.status = Some(status);
                    let _ = reaper.join();
                }
                Ok(Err(error)) => {
                    record_child_error(
                        &mut cleanup.reap_error,
                        format!("child hard wait failed after kill: {error}"),
                    );
                    let _ = reaper.join();
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    record_child_error(
                        &mut cleanup.reap_error,
                        format!(
                            "child hard reaper did not finish within {} seconds",
                            REAP_TIMEOUT.as_secs()
                        ),
                    );
                    drop(reaper);
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    record_child_error(&mut cleanup.reap_error, "child hard reaper disconnected");
                    let _ = reaper.join();
                }
            }
        }
        cleanup
    }
}

impl Drop for ChildMonitor {
    fn drop(&mut self) {
        if self.child.is_none() {
            return;
        }

        let cleanup = self.force_kill_and_reap();
        if cleanup.status.is_none() || cleanup.kill_error.is_some() || cleanup.reap_error.is_some()
        {
            let _ = writeln!(
                io::stderr(),
                "child cleanup during unwinding: status={:?}; kill_error={:?}; reap_error={:?}",
                cleanup.status,
                cleanup.kill_error,
                cleanup.reap_error
            );
        }
    }
}

struct ChildObservation {
    status: Option<ExitStatus>,
    timed_out: bool,
    hook_triggered: bool,
    kill_error: Option<String>,
    reap_error: Option<String>,
    stdout: String,
    stderr: String,
}

#[derive(Debug, PartialEq, Eq)]
enum ChildOutcome {
    Completed,
    Panic,
    Error,
    DeadlockTimeout,
}

type ChildRunResult = (
    Result<(), String>,
    Result<(), String>,
    Result<(), String>,
    bool,
    String,
);

#[test]
fn runtime_sentinel_reentrancy() {
    if env::var(CHILD_ENV).as_deref() == Ok(CHILD_ROLE) {
        run_child_case();
    } else {
        run_parent_case();
    }
}

#[test]
fn child_output_directory_is_owned_before_use() {
    let directory = unique_output_dir();
    let path = directory.path().to_owned();
    assert!(
        path.is_dir(),
        "child directory must already be allocated exclusively"
    );
    let other = unique_output_dir();
    assert_ne!(path, other.path());
    drop(directory);
    assert!(
        !path.exists(),
        "owned child directory must be cleaned after use"
    );
}

fn run_parent_case() {
    let mut output_dir = unique_output_dir();
    let stdout_path = output_dir.path().join("stdout");
    let stderr_path = output_dir.path().join("stderr");
    let stdout = File::create(&stdout_path).expect("child stdout file must be creatable");
    let stderr = File::create(&stderr_path).expect("child stderr file must be creatable");

    let mut child = ChildMonitor::new(
        Command::new(env::current_exe().expect("current test executable must exist"))
            .args(["--exact", "runtime_sentinel_reentrancy", "--nocapture"])
            .env(CHILD_ENV, CHILD_ROLE)
            .env(
                HOOK_MARKER_ENV,
                output_dir.path().join("hook-triggered").as_os_str(),
            )
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .spawn()
            .expect("runtime sentinel child process must spawn"),
    );

    let deadline = Instant::now() + CHILD_TIMEOUT;
    let (status, timed_out, kill_error, reap_error) = loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                child.release_reaped();
                break (Some(status), false, None, None);
            }
            Ok(None) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(10));
            }
            Ok(None) => {
                let cleanup = child.force_kill_and_reap();
                break (cleanup.status, true, cleanup.kill_error, cleanup.reap_error);
            }
            Err(error) => {
                let mut cleanup = child.force_kill_and_reap();
                record_child_error(
                    &mut cleanup.reap_error,
                    format!("runtime sentinel child status check failed: {error}"),
                );
                break (
                    cleanup.status,
                    false,
                    cleanup.kill_error,
                    cleanup.reap_error,
                );
            }
        }
    };

    if timed_out || status.is_none() || kill_error.is_some() || reap_error.is_some() {
        output_dir.retain();
    }

    let stdout = fs::read_to_string(&stdout_path).unwrap_or_default();
    let stderr = fs::read_to_string(&stderr_path).unwrap_or_default();
    let observation = ChildObservation {
        status,
        timed_out,
        hook_triggered: output_dir.path().join("hook-triggered").exists(),
        kill_error,
        reap_error,
        stdout,
        stderr,
    };

    match classify(&observation) {
        ChildOutcome::Completed => {
            assert!(
                observation.stdout.contains("R14_CHILD_RESULT=success"),
                "child completed without a success marker\n{}",
                format_observation(&observation)
            );
            assert!(
                observation.stdout.contains("hook_triggered=true"),
                "child did not prove that the hostile import hook ran\n{}",
                format_observation(&observation)
            );
            assert!(
                observation.stdout.contains("nested_rejected=true"),
                "child did not prove deterministic reentrant-call rejection\n{}",
                format_observation(&observation)
            );
            assert!(
                observation.stdout.contains("retry_succeeded=true"),
                "child did not prove retry after failed first initialization\n{}",
                format_observation(&observation)
            );
        }
        ChildOutcome::Panic => panic!(
            "R-14 reproduced as a panic during first runtime-sentinel initialization\n{}",
            format_observation(&observation)
        ),
        ChildOutcome::Error => panic!(
            "R-14 reproduced as an initialization/generation error\n{}",
            format_observation(&observation)
        ),
        ChildOutcome::DeadlockTimeout => panic!(
            "R-14 reproduced as a deadlock/timeout after {} seconds\n{}",
            CHILD_TIMEOUT.as_secs(),
            format_observation(&observation)
        ),
    }
}

fn run_child_case() {
    pyo3::Python::initialize();

    let result = pyo3::Python::attach(|py| -> PyResult<ChildRunResult> {
        let finder = install_reentrant_import_hook(py)?;
        println!("R14_CHILD_ENV=python={}", pyo3::Python::version_str());

        let first_initialization =
            Codegen::prime_live_runtime(py).map_err(|error| error.to_string());
        let hook_triggered = finder.getattr("triggered")?.extract::<bool>()?;
        let nested_result = finder.getattr("nested_result")?.extract::<String>()?;
        let retry = Codegen::prime_live_runtime(py).map_err(|error| error.to_string());
        let post_retry_generation = Codegen::default()
            .module_from_str("value = 1\n", "pyo3_bindgen_after_reentrant_retry")
            .and_then(Codegen::generate)
            .map(|_| ())
            .map_err(|error| error.to_string());
        Ok((
            first_initialization,
            retry,
            post_retry_generation,
            hook_triggered,
            nested_result,
        ))
    });

    match result {
        Ok((Err(first_error), Ok(()), Ok(()), true, nested_result))
            if nested_result
                .contains("reentrant runtime sentinel initialization is unsupported")
                && first_error
                    .contains("reentrant runtime sentinel initialization is unsupported") =>
        {
            println!(
                "R14_CHILD_RESULT=success; hook_triggered=true; nested_rejected=true; retry_succeeded=true; nested_result={nested_result}"
            );
        }
        Ok((first, retry, post_retry, hook_triggered, nested_result)) => {
            eprintln!(
                "R14_CHILD_RESULT=error; hook_triggered={hook_triggered}; nested_result={nested_result}; first={first:?}; retry={retry:?}; post_retry={post_retry:?}"
            );
            std::process::exit(CHILD_ERROR_EXIT);
        }
        Err(error) => {
            eprintln!("R14_CHILD_RESULT=error; child setup failed; error={error}");
            std::process::exit(CHILD_ERROR_EXIT);
        }
    }
}

fn install_reentrant_import_hook(py: Python<'_>) -> PyResult<Bound<'_, PyAny>> {
    let sys = PyModule::import(py, "sys")?;
    let modules = sys.getattr("modules")?.cast_into::<PyDict>()?;
    if modules.get_item("inspect")?.is_some() {
        modules.del_item("inspect")?;
    }

    let hook_marker = env::var_os(HOOK_MARKER_ENV).map(PathBuf::from);
    let nested_generation =
        PyCFunction::new_closure(py, None, None, move |_args, _kwargs| -> PyResult<String> {
            if let Some(path) = &hook_marker {
                fs::write(path, b"triggered").map_err(|error| {
                    PyRuntimeError::new_err(format!("hook marker write failed: {error}"))
                })?;
            }
            let nested = Codegen::default()
                .module_from_str("value = 1\n", "pyo3_bindgen_reentrant_target")
                .and_then(Codegen::generate);
            Ok(match nested {
                Ok(_) => "unexpected-success".to_owned(),
                Err(error) => format!("rejected:{error}"),
            })
        })?;

    let source = std::ffi::CString::new(FINDER_SOURCE).expect("finder source has no NUL bytes");
    let hook_module = PyModule::from_code(
        py,
        source.as_c_str(),
        pyo3::ffi::c_str!("runtime_sentinel_reentrancy.py"),
        pyo3::ffi::c_str!("runtime_sentinel_reentrancy_hook"),
    )?;
    let finder_type = hook_module.getattr("ReentrantFinder")?;
    let finder = finder_type.call1((nested_generation,))?;
    sys.getattr("meta_path")?
        .cast_into::<PyList>()?
        .insert(0, &finder)?;
    Ok(finder)
}

fn classify(observation: &ChildObservation) -> ChildOutcome {
    if observation.timed_out {
        return if observation.hook_triggered {
            ChildOutcome::DeadlockTimeout
        } else {
            ChildOutcome::Error
        };
    }
    if observation.status.is_some_and(|status| status.success())
        && observation.kill_error.is_none()
        && observation.reap_error.is_none()
        && observation
            .stdout
            .matches("R14_CHILD_RESULT=success")
            .count()
            == 1
        && observation.hook_triggered
    {
        return ChildOutcome::Completed;
    }

    let combined = format!("{}\n{}", observation.stdout, observation.stderr);
    if combined.contains("panicked at")
        || combined.contains("PanicException")
        || combined.contains("panic during first runtime-sentinel")
    {
        ChildOutcome::Panic
    } else {
        ChildOutcome::Error
    }
}

fn format_observation(observation: &ChildObservation) -> String {
    format!(
        "status={:?}; timed_out={}; hook_triggered={}; kill_error={:?}; reap_error={:?}; stdout:\n{}\nstderr:\n{}",
        observation.status,
        observation.timed_out,
        observation.hook_triggered,
        observation.kill_error,
        observation.reap_error,
        observation.stdout,
        observation.stderr
    )
}

fn unique_output_dir() -> ChildOutputDir {
    ChildOutputDir::allocate("r14-child")
        .expect("child output directory must be allocated exclusively")
}
