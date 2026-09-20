//! R-13 experiment: keep report metadata acquisition and process-global stream
//! suppression/restoration live at the same time on a free-threaded interpreter.

use pyo3::types::{PyAnyMethods, PyDict, PyDictMethods, PyTypeMethods};
use pyo3_bindgen_engine::{Codegen, Config, GenerationReport};
use std::{
    env, fs,
    fs::File,
    io::{self, Write},
    path::Path,
    process::{Child, Command, ExitStatus, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant},
};

#[path = "../test_support/bounded_consumer.rs"]
mod bounded_consumer;
use bounded_consumer::OwnedTempDir as ChildOutputDir;

const CHILD_ENV: &str = "PYO3_BINDGEN_R13_FREE_THREADED_CHILD";
const CHILD_ROLE: &str = "pyo3-bindgen-r13-free-threaded-child-v1";
const CHILD_TIMEOUT: Duration = Duration::from_secs(45);
const REAP_TIMEOUT: Duration = Duration::from_secs(3);
const ITERATIONS: usize = 24;
const WORKER_TIMEOUT: Duration = Duration::from_secs(8);
const EVENT_TIMEOUT: Duration = Duration::from_secs(8);

const ORIGINAL_STDOUT: &str = "_pyo3_bindgen_r13_original_stdout";
const ORIGINAL_STDERR: &str = "_pyo3_bindgen_r13_original_stderr";
const CAPTURE_STDOUT: &str = "_pyo3_bindgen_r13_capture_stdout";
const CAPTURE_STDERR: &str = "_pyo3_bindgen_r13_capture_stderr";

struct ChildObservation {
    status: Option<ExitStatus>,
    timed_out: bool,
    kill_error: Option<String>,
    reap_error: Option<String>,
    stdout: String,
    stderr: String,
}

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

struct TestModulesGuard(Vec<String>);

impl Drop for TestModulesGuard {
    fn drop(&mut self) {
        pyo3::Python::attach(|py| {
            let Ok(sys) = py.import("sys") else {
                return;
            };
            let Ok(modules) = sys
                .getattr("modules")
                .and_then(|modules| modules.cast_into::<PyDict>().map_err(Into::into))
            else {
                return;
            };
            for module_name in &self.0 {
                if modules.get_item(module_name).ok().flatten().is_some() {
                    let _ = modules.del_item(module_name);
                }
            }
        });
    }
}

#[derive(Debug)]
struct RuntimeInfo {
    implementation: String,
    version: String,
    executable: Option<String>,
    gil_enabled: Option<bool>,
    gil_disabled_config: Option<String>,
}

#[derive(Debug)]
struct ExpectedMetadata {
    implementation: String,
    version: String,
    executable: Option<String>,
}

#[derive(Debug)]
struct EventNames {
    control_module: String,
    metadata_started: String,
    metadata_release: String,
    metadata_released: String,
    metadata_finish: String,
    suppression_started: String,
    suppression_ready: String,
    suppression_release: String,
}

impl EventNames {
    fn new(iteration: usize) -> Self {
        Self {
            control_module: format!("_pyo3_bindgen_r13_control_{iteration}"),
            metadata_started: format!("_pyo3_bindgen_r13_metadata_started_{iteration}"),
            metadata_release: format!("_pyo3_bindgen_r13_metadata_release_{iteration}"),
            metadata_released: format!("_pyo3_bindgen_r13_metadata_released_{iteration}"),
            metadata_finish: format!("_pyo3_bindgen_r13_metadata_finish_{iteration}"),
            suppression_started: format!("_pyo3_bindgen_r13_suppression_started_{iteration}"),
            suppression_ready: format!("_pyo3_bindgen_r13_suppression_ready_{iteration}"),
            suppression_release: format!("_pyo3_bindgen_r13_suppression_release_{iteration}"),
        }
    }

    fn all(&self) -> [&str; 7] {
        [
            &self.metadata_started,
            &self.metadata_release,
            &self.metadata_released,
            &self.metadata_finish,
            &self.suppression_started,
            &self.suppression_ready,
            &self.suppression_release,
        ]
    }
}

#[derive(Debug)]
struct StreamState {
    stdout_ptr: usize,
    stderr_ptr: usize,
    stdout_seed: String,
    stderr_seed: String,
}

#[derive(Debug)]
struct StreamSnapshot {
    current_stdout_ptr: usize,
    current_stderr_ptr: usize,
    current_stdout_type: String,
    current_stderr_type: String,
    captured_stdout: String,
    captured_stderr: String,
}

fn runtime_info() -> RuntimeInfo {
    pyo3::Python::attach(|py| {
        let sys = py.import("sys").unwrap();
        let platform = py.import("platform").unwrap();
        let sysconfig = py.import("sysconfig").unwrap();
        let gil_disabled_config = sysconfig
            .call_method1("get_config_var", ("Py_GIL_DISABLED",))
            .ok()
            .and_then(|value| {
                if value.is_none() {
                    None
                } else {
                    value
                        .extract::<i64>()
                        .map(|number| number.to_string())
                        .or_else(|_| value.extract::<String>())
                        .ok()
                }
            });
        let gil_enabled = sys
            .getattr("_is_gil_enabled")
            .ok()
            .and_then(|callable| callable.call0().ok())
            .and_then(|value| value.extract::<bool>().ok());

        RuntimeInfo {
            implementation: platform
                .call_method0("python_implementation")
                .unwrap()
                .extract()
                .unwrap(),
            version: sys.getattr("version").unwrap().extract().unwrap(),
            executable: sys
                .getattr("executable")
                .ok()
                .and_then(|value| value.extract().ok()),
            gil_enabled,
            gil_disabled_config,
        }
    })
}

fn expected_metadata() -> ExpectedMetadata {
    pyo3::Python::attach(|py| {
        let sys = py.import("sys").unwrap();
        let platform = py.import("platform").unwrap();
        ExpectedMetadata {
            implementation: platform
                .call_method0("python_implementation")
                .unwrap()
                .extract()
                .unwrap(),
            version: sys.getattr("version").unwrap().extract().unwrap(),
            executable: sys
                .getattr("executable")
                .ok()
                .and_then(|value| value.extract().ok()),
        }
    })
}

fn event_is_set(module_name: &str, name: &str) -> bool {
    pyo3::Python::attach(|py| {
        py.import(module_name)
            .unwrap()
            .getattr(name)
            .unwrap()
            .call_method0("is_set")
            .unwrap()
            .extract()
            .unwrap()
    })
}

fn wait_for_event(module_name: &str, name: &str) -> bool {
    let deadline = Instant::now() + EVENT_TIMEOUT;
    while Instant::now() < deadline {
        if event_is_set(module_name, name) {
            return true;
        }
        thread::sleep(Duration::from_millis(2));
    }
    event_is_set(module_name, name)
}

fn set_event(module_name: &str, name: &str) {
    pyo3::Python::attach(|py| {
        py.import(module_name)
            .unwrap()
            .getattr(name)
            .unwrap()
            .call_method0("set")
            .unwrap();
    });
}

fn set_all_events(events: &EventNames) {
    for name in events.all() {
        set_event(&events.control_module, name);
    }
}

fn save_original_streams() {
    pyo3::Python::attach(|py| {
        let sys = py.import("sys").unwrap();
        let stdout = sys.getattr("stdout").unwrap();
        let stderr = sys.getattr("stderr").unwrap();
        sys.setattr(ORIGINAL_STDOUT, &stdout).unwrap();
        sys.setattr(ORIGINAL_STDERR, &stderr).unwrap();
    });
}

fn install_capture_streams(iteration: usize) -> StreamState {
    pyo3::Python::attach(|py| {
        let sys = py.import("sys").unwrap();
        let io = py.import("io").unwrap();
        let stdout = io.call_method0("StringIO").unwrap();
        let stderr = io.call_method0("StringIO").unwrap();
        let stdout_seed = format!("seed-stdout-{iteration}\n");
        let stderr_seed = format!("seed-stderr-{iteration}\n");
        stdout.call_method1("write", (&stdout_seed,)).unwrap();
        stderr.call_method1("write", (&stderr_seed,)).unwrap();
        sys.setattr(CAPTURE_STDOUT, &stdout).unwrap();
        sys.setattr(CAPTURE_STDERR, &stderr).unwrap();
        sys.setattr("stdout", &stdout).unwrap();
        sys.setattr("stderr", &stderr).unwrap();
        StreamState {
            stdout_ptr: stdout.as_ptr() as usize,
            stderr_ptr: stderr.as_ptr() as usize,
            stdout_seed,
            stderr_seed,
        }
    })
}

fn inspect_streams() -> StreamSnapshot {
    pyo3::Python::attach(|py| {
        let sys = py.import("sys").unwrap();
        let current_stdout = sys.getattr("stdout").unwrap();
        let current_stderr = sys.getattr("stderr").unwrap();
        let captured_stdout = sys
            .getattr(CAPTURE_STDOUT)
            .unwrap()
            .call_method0("getvalue")
            .unwrap()
            .extract()
            .unwrap();
        let captured_stderr = sys
            .getattr(CAPTURE_STDERR)
            .unwrap()
            .call_method0("getvalue")
            .unwrap()
            .extract()
            .unwrap();
        StreamSnapshot {
            current_stdout_ptr: current_stdout.as_ptr() as usize,
            current_stderr_ptr: current_stderr.as_ptr() as usize,
            current_stdout_type: current_stdout.get_type().name().unwrap().to_string(),
            current_stderr_type: current_stderr.get_type().name().unwrap().to_string(),
            captured_stdout,
            captured_stderr,
        }
    })
}

fn write_visible_output(iteration: usize) {
    pyo3::Python::attach(|py| {
        let sys = py.import("sys").unwrap();
        let stdout = format!("visible-stdout-{iteration}\n");
        let stderr = format!("visible-stderr-{iteration}\n");
        sys.getattr("stdout")
            .unwrap()
            .call_method1("write", (&stdout,))
            .unwrap();
        sys.getattr("stderr")
            .unwrap()
            .call_method1("write", (&stderr,))
            .unwrap();
    });
}

fn restore_saved_streams() {
    pyo3::Python::attach(|py| {
        let sys = py.import("sys").unwrap();
        let stdout = sys.getattr(ORIGINAL_STDOUT).unwrap();
        let stderr = sys.getattr(ORIGINAL_STDERR).unwrap();
        sys.setattr("stdout", &stdout).unwrap();
        sys.setattr("stderr", &stderr).unwrap();
        sys.delattr(CAPTURE_STDOUT).unwrap();
        sys.delattr(CAPTURE_STDERR).unwrap();
    });
}

fn restore_original_streams_and_cleanup() {
    pyo3::Python::attach(|py| {
        let sys = py.import("sys").unwrap();
        let stdout = sys.getattr(ORIGINAL_STDOUT).unwrap();
        let stderr = sys.getattr(ORIGINAL_STDERR).unwrap();
        sys.setattr("stdout", &stdout).unwrap();
        sys.setattr("stderr", &stderr).unwrap();
        sys.delattr(ORIGINAL_STDOUT).unwrap();
        sys.delattr(ORIGINAL_STDERR).unwrap();
    });
}

fn install_metadata_pause(events: &EventNames, original_name: &str) {
    pyo3::Python::attach(|py| {
        let sys = py.import("sys").unwrap();
        let types = py.import("types").unwrap();
        let threading = py.import("threading").unwrap();
        let control = types
            .call_method1("ModuleType", (&events.control_module,))
            .unwrap();
        for name in events.all() {
            let event = threading.call_method0("Event").unwrap();
            control.setattr(name, &event).unwrap();
        }
        sys.getattr("modules")
            .unwrap()
            .call_method1("__setitem__", (&events.control_module, &control))
            .unwrap();

        let platform = py.import("platform").unwrap();
        let original = platform.getattr("python_implementation").unwrap();
        platform.setattr(original_name, &original).unwrap();

        let globals = PyDict::new(py);
        globals
            .set_item("original_python_implementation", &original)
            .unwrap();
        globals
            .set_item(
                "metadata_started",
                control.getattr(&events.metadata_started).unwrap(),
            )
            .unwrap();
        globals
            .set_item(
                "metadata_release",
                control.getattr(&events.metadata_release).unwrap(),
            )
            .unwrap();
        globals
            .set_item(
                "metadata_released",
                control.getattr(&events.metadata_released).unwrap(),
            )
            .unwrap();
        globals
            .set_item(
                "metadata_finish",
                control.getattr(&events.metadata_finish).unwrap(),
            )
            .unwrap();
        py.run(
            pyo3::ffi::c_str!(
                r#"def r13_wrapped_python_implementation():
    metadata_started.set()
    if not metadata_release.wait(5.0):
        raise RuntimeError('R-13 metadata release timed out')
    metadata_released.set()
    if not metadata_finish.wait(5.0):
        raise RuntimeError('R-13 metadata finish timed out')
    return original_python_implementation()
"#
            ),
            Some(&globals),
            None,
        )
        .unwrap();
        let wrapped = globals
            .get_item("r13_wrapped_python_implementation")
            .unwrap()
            .unwrap();
        platform.setattr("python_implementation", &wrapped).unwrap();
    });
}

fn cleanup_metadata_pause(events: &EventNames, original_name: &str) {
    pyo3::Python::attach(|py| {
        let sys = py.import("sys").unwrap();
        let platform = py.import("platform").unwrap();
        let original = platform.getattr(original_name).unwrap();
        platform
            .setattr("python_implementation", &original)
            .unwrap();
        platform.delattr(original_name).unwrap();
        sys.getattr("modules")
            .unwrap()
            .call_method1("__delitem__", (&events.control_module,))
            .unwrap();
    });
}

fn helper_namespace_is_clean() -> Result<(), String> {
    pyo3::Python::attach(|py| {
        let sys = py.import("sys").map_err(|error| error.to_string())?;
        for attribute in [
            ORIGINAL_STDOUT,
            ORIGINAL_STDERR,
            CAPTURE_STDOUT,
            CAPTURE_STDERR,
        ] {
            if sys.hasattr(attribute).map_err(|error| error.to_string())? {
                return Err(format!("test helper left sys.{attribute} installed"));
            }
        }
        let modules = sys
            .getattr("modules")
            .map_err(|error| error.to_string())?
            .cast_into::<PyDict>()
            .map_err(|error| error.to_string())?;
        let platform = py.import("platform").map_err(|error| error.to_string())?;
        for iteration in 0..ITERATIONS {
            let events = EventNames::new(iteration);
            for module_name in [
                events.control_module,
                format!("r13_metadata_module_{iteration}"),
                format!("r13_suppression_module_{iteration}"),
            ] {
                if modules
                    .get_item(module_name.as_str())
                    .map_err(|error| error.to_string())?
                    .is_some()
                {
                    return Err(format!(
                        "test helper left sys.modules[{module_name:?}] installed"
                    ));
                }
            }
            let original_name =
                format!("_pyo3_bindgen_r13_original_python_implementation_{iteration}");
            if platform
                .hasattr(original_name.as_str())
                .map_err(|error| error.to_string())?
            {
                return Err(format!(
                    "test helper left platform.{original_name} installed"
                ));
            }
        }
        Ok(())
    })
}

fn overlap_config(suppress_stdout: bool, suppress_stderr: bool) -> Config {
    Config {
        traverse_submodules: false,
        generate_preludes: false,
        generate_imports: false,
        generate_classes: false,
        generate_enums: false,
        generate_type_vars: false,
        generate_functions: false,
        generate_properties: false,
        generate_docs: false,
        generate_provenance: false,
        deduplicate_inherited_members: false,
        generate_dependencies: false,
        suppress_python_stdout: suppress_stdout,
        suppress_python_stderr: suppress_stderr,
        ..Config::default()
    }
}

fn receive_report(
    receiver: &Receiver<Result<GenerationReport, String>>,
) -> Option<Result<GenerationReport, String>> {
    receiver.recv_timeout(WORKER_TIMEOUT).ok()
}

fn check_suppressed_streams(snapshot: &StreamSnapshot, state: &StreamState) -> Result<(), String> {
    if snapshot.current_stdout_type != "SuppressedOutput"
        || snapshot.current_stderr_type != "SuppressedOutput"
    {
        return Err(format!(
            "suppressed stream types were stdout={} stderr={}",
            snapshot.current_stdout_type, snapshot.current_stderr_type
        ));
    }
    if snapshot.current_stdout_ptr != snapshot.current_stderr_ptr {
        return Err("stdout and stderr did not share the suppression object".to_owned());
    }
    if snapshot.current_stdout_ptr == state.stdout_ptr
        || snapshot.current_stderr_ptr == state.stderr_ptr
    {
        return Err("suppression replaced neither original stream".to_owned());
    }
    if snapshot.captured_stdout != state.stdout_seed
        || snapshot.captured_stderr != state.stderr_seed
    {
        return Err(format!(
            "suppressed output reached capture streams: stdout={:?} stderr={:?}",
            snapshot.captured_stdout, snapshot.captured_stderr
        ));
    }
    Ok(())
}

fn check_restored_streams(
    snapshot: &StreamSnapshot,
    state: &StreamState,
    iteration: usize,
) -> Result<(), String> {
    if snapshot.current_stdout_ptr != state.stdout_ptr
        || snapshot.current_stderr_ptr != state.stderr_ptr
    {
        return Err("stream identity was not restored to the original objects".to_owned());
    }
    let expected_stdout = format!("{}visible-stdout-{iteration}\n", state.stdout_seed);
    let expected_stderr = format!("{}visible-stderr-{iteration}\n", state.stderr_seed);
    if snapshot.captured_stdout != expected_stdout || snapshot.captured_stderr != expected_stderr {
        return Err(format!(
            "restored stream content mismatch: stdout={:?} stderr={:?}",
            snapshot.captured_stdout, snapshot.captured_stderr
        ));
    }
    Ok(())
}

fn check_report(
    report: &GenerationReport,
    expected: &ExpectedMetadata,
    module_name: &str,
) -> Result<(), String> {
    let Some(metadata) = report.python.as_ref() else {
        return Err("report.python was None for live generation".to_owned());
    };
    if metadata.implementation != expected.implementation
        || metadata.version != expected.version
        || metadata.executable != expected.executable
    {
        return Err(format!(
            "report metadata mismatch: got implementation={:?} version={:?} executable={:?}; expected implementation={:?} version={:?} executable={:?}",
            metadata.implementation,
            metadata.version,
            metadata.executable,
            expected.implementation,
            expected.version,
            expected.executable,
        ));
    }
    if report.schema_version != 1
        || !report
            .modules
            .iter()
            .any(|module| module.name == module_name)
    {
        return Err(format!(
            "report shape mismatch: schema_version={} modules={:?}",
            report.schema_version,
            report
                .modules
                .iter()
                .map(|module| module.name.as_str())
                .collect::<Vec<_>>(),
        ));
    }
    Ok(())
}

fn run_iteration(iteration: usize, expected: &ExpectedMetadata) -> Result<(), String> {
    let state = install_capture_streams(iteration);
    let events = EventNames::new(iteration);
    let original_name = format!("_pyo3_bindgen_r13_original_python_implementation_{iteration}");
    let module_name = format!("r13_metadata_module_{iteration}");
    let suppression_module_name = format!("r13_suppression_module_{iteration}");
    let _module_cleanup = TestModulesGuard(vec![
        events.control_module.clone(),
        module_name.clone(),
        suppression_module_name.clone(),
    ]);
    install_metadata_pause(&events, &original_name);

    let metadata_codegen = Codegen::new(overlap_config(false, false))
        .module_from_str("value = 1", &module_name)
        .map_err(|error| format!("metadata setup failed: {error}"))?;
    let metadata_source = format!(
        r#"import {} as _r13_control
import sys as _r13_sys
_r13_control.{}.set()
print('suppressed-stdout-{}')
print('suppressed-stderr-{}', file=_r13_sys.stderr)
_r13_control.{}.set()
if not _r13_control.{}.wait(5.0):
    raise RuntimeError('R-13 suppression release timed out')
value = 1
"#,
        events.control_module,
        events.suppression_started,
        iteration,
        iteration,
        events.suppression_ready,
        events.suppression_release,
    );

    let (metadata_tx, metadata_rx) = mpsc::channel();
    let metadata_thread = thread::spawn(move || {
        let result = metadata_codegen
            .generate_with_report()
            .map(|generated| generated.report)
            .map_err(|error| error.to_string());
        metadata_tx.send(result).unwrap();
    });

    let mut failures = Vec::new();
    if !wait_for_event(&events.control_module, &events.metadata_started) {
        failures.push("metadata callback did not start".to_owned());
    }

    let mut suppression_thread = None;
    let mut suppression_rx = None;
    if failures.is_empty() {
        let (suppression_tx, receiver) = mpsc::channel();
        let suppression_module_name = suppression_module_name.clone();
        let suppression_source = metadata_source;
        let suppression_thread_config = overlap_config(true, true);
        suppression_thread = Some(thread::spawn(move || {
            let result = Codegen::new(suppression_thread_config)
                .module_from_str(&suppression_source, &suppression_module_name)
                .map(|_| ())
                .map_err(|error| error.to_string());
            suppression_tx.send(result).unwrap();
        }));
        suppression_rx = Some(receiver);
        if !wait_for_event(&events.control_module, &events.suppression_ready) {
            failures.push("suppression callback did not reach its held-stream phase".to_owned());
        }
    }

    let mut metadata_result = None;
    let mut suppression_result = None;
    if failures.is_empty() {
        let suppressed_snapshot = inspect_streams();
        if let Err(error) = check_suppressed_streams(&suppressed_snapshot, &state) {
            failures.push(format!("iteration {iteration}: {error}"));
        }

        set_event(&events.control_module, &events.metadata_release);
        if !wait_for_event(&events.control_module, &events.metadata_released) {
            failures.push("metadata callback did not remain live after first release".to_owned());
        } else if iteration % 2 == 0 {
            // Even iterations finish metadata while suppression is still active. The
            // report thread must therefore read sys.version/sys.executable with both
            // process-global streams redirected.
            set_event(&events.control_module, &events.metadata_finish);
            metadata_result = receive_report(&metadata_rx);
            if metadata_result.is_none() {
                failures.push("metadata report did not complete during suppression".to_owned());
            }
            let still_suppressed = inspect_streams();
            if let Err(error) = check_suppressed_streams(&still_suppressed, &state) {
                failures.push(format!("iteration {iteration}: {error}"));
            }
            set_event(&events.control_module, &events.suppression_release);
            if let Some(receiver) = suppression_rx.as_ref() {
                suppression_result = receiver.recv_timeout(WORKER_TIMEOUT).ok();
            }
            if suppression_result.is_none() {
                failures.push("suppression thread did not restore streams".to_owned());
            }
            let restored = inspect_streams();
            write_visible_output(iteration);
            let restored_with_output = inspect_streams();
            if let Err(error) = check_restored_streams(&restored_with_output, &state, iteration) {
                failures.push(format!("iteration {iteration}: {error}"));
            }
            if restored.current_stdout_ptr != state.stdout_ptr
                || restored.current_stderr_ptr != state.stderr_ptr
            {
                failures.push(format!(
                    "iteration {iteration}: streams changed identity before visible output"
                ));
            }
        } else {
            // Odd iterations restore both streams while metadata acquisition is still
            // blocked inside the Python callback, then let the report complete.
            set_event(&events.control_module, &events.suppression_release);
            if let Some(receiver) = suppression_rx.as_ref() {
                suppression_result = receiver.recv_timeout(WORKER_TIMEOUT).ok();
            }
            if suppression_result.is_none() {
                failures.push("suppression thread did not restore streams".to_owned());
            }
            let restored = inspect_streams();
            write_visible_output(iteration);
            let restored_with_output = inspect_streams();
            if let Err(error) = check_restored_streams(&restored_with_output, &state, iteration) {
                failures.push(format!("iteration {iteration}: {error}"));
            }
            if restored.current_stdout_ptr != state.stdout_ptr
                || restored.current_stderr_ptr != state.stderr_ptr
            {
                failures.push(format!(
                    "iteration {iteration}: streams changed identity before visible output"
                ));
            }
            set_event(&events.control_module, &events.metadata_finish);
            metadata_result = receive_report(&metadata_rx);
            if metadata_result.is_none() {
                failures.push("metadata report did not complete after restoration".to_owned());
            }
        }
    }

    set_all_events(&events);
    if metadata_result.is_none() {
        metadata_result = receive_report(&metadata_rx);
    }
    if suppression_result.is_none() {
        if let Some(receiver) = suppression_rx.as_ref() {
            suppression_result = receiver.recv_timeout(WORKER_TIMEOUT).ok();
        }
    }

    if metadata_thread.join().is_err() {
        failures.push("metadata thread panicked".to_owned());
    }
    if let Some(thread) = suppression_thread {
        if thread.join().is_err() {
            failures.push("suppression thread panicked".to_owned());
        }
    }

    if let Some(Ok(report)) = metadata_result {
        if let Err(error) = check_report(&report, expected, &module_name) {
            failures.push(format!("iteration {iteration}: {error}"));
        }
    } else if let Some(Err(error)) = metadata_result {
        failures.push(format!("metadata generation failed: {error}"));
    } else {
        failures.push("metadata generation result was unavailable".to_owned());
    }

    if let Some(Err(error)) = suppression_result {
        failures.push(format!("suppressed generation failed: {error}"));
    } else if suppression_rx.is_some() && suppression_result.is_none() {
        failures.push("suppressed generation result was unavailable".to_owned());
    }

    cleanup_metadata_pause(&events, &original_name);
    restore_saved_streams();

    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

struct OriginalStreamsGuard;

impl Drop for OriginalStreamsGuard {
    fn drop(&mut self) {
        restore_original_streams_and_cleanup();
    }
}

fn runtime_is_supported(runtime: &RuntimeInfo) -> bool {
    runtime.implementation == "CPython"
        && runtime.version.starts_with("3.14.")
        && runtime.gil_enabled == Some(false)
        && runtime.gil_disabled_config.as_deref() == Some("1")
}

fn unique_child_output_dir() -> ChildOutputDir {
    ChildOutputDir::allocate("r13-child")
        .expect("child output directory must be allocated exclusively")
}

#[test]
fn metadata_child_directory_is_owned_before_use() {
    let directory = unique_child_output_dir();
    let path = directory.path().to_owned();
    assert!(
        path.is_dir(),
        "child directory must already be allocated exclusively"
    );
    let other = unique_child_output_dir();
    assert_ne!(path, other.path());
    drop(directory);
    assert!(
        !path.exists(),
        "owned child directory must be cleaned after use"
    );
}

fn wait_for_child(mut child: ChildMonitor, output_dir: &Path) -> ChildObservation {
    let deadline = Instant::now() + CHILD_TIMEOUT;
    let mut timed_out = false;
    let mut kill_error = None;
    let mut reap_error = None;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                child.release_reaped();
                break Some(status);
            }
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                timed_out = true;
                let cleanup = child.force_kill_and_reap();
                kill_error = cleanup.kill_error;
                reap_error = cleanup.reap_error;
                break cleanup.status;
            }
            Err(error) => {
                let mut cleanup = child.force_kill_and_reap();
                record_child_error(
                    &mut cleanup.reap_error,
                    format!("R-13 child status check failed: {error}"),
                );
                kill_error = cleanup.kill_error;
                reap_error = cleanup.reap_error;
                break cleanup.status;
            }
        }
    };
    ChildObservation {
        status,
        timed_out,
        kill_error,
        reap_error,
        stdout: fs::read_to_string(output_dir.join("stdout")).unwrap_or_default(),
        stderr: fs::read_to_string(output_dir.join("stderr")).unwrap_or_default(),
    }
}

fn run_parent_case() {
    let mut output_dir = unique_child_output_dir();
    let stdout = File::create(output_dir.path().join("stdout")).expect("child stdout file");
    let stderr = File::create(output_dir.path().join("stderr")).expect("child stderr file");
    let child = ChildMonitor::new(
        Command::new(env::current_exe().expect("current test executable must exist"))
            .args([
                "--exact",
                "free_threaded_metadata_overlap_preserves_streams_and_report",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CHILD_ENV, CHILD_ROLE)
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .spawn()
            .expect("R-13 child process must spawn"),
    );
    let observation = wait_for_child(child, output_dir.path());
    if observation.timed_out
        || observation.status.is_none()
        || observation.kill_error.is_some()
        || observation.reap_error.is_some()
    {
        // A hard reaper may still own the child. Preserve its files and diagnostics.
        output_dir.retain();
    }
    assert!(
        !observation.timed_out
            && observation.kill_error.is_none()
            && observation.reap_error.is_none()
            && observation.status.is_some_and(|status| status.success())
            && observation.stdout.matches("R13_CHILD_RESULT=success").count() == 1,
        "R-13 child failed: status={:?}; timed_out={}; kill_error={:?}; reap_error={:?}; stdout:\n{}\nstderr:\n{}",
        observation.status,
        observation.timed_out,
        observation.kill_error,
        observation.reap_error,
        observation.stdout,
        observation.stderr,
    );
}

fn run_child_case(runtime: &RuntimeInfo) {
    helper_namespace_is_clean().expect("R-13 helper namespace must begin clean");
    let expected = expected_metadata();
    save_original_streams();
    let original_streams_guard = OriginalStreamsGuard;
    for iteration in 0..ITERATIONS {
        run_iteration(iteration, &expected)
            .unwrap_or_else(|error| panic!("R-13 iteration {iteration} failed: {error}"));
    }
    drop(original_streams_guard);
    helper_namespace_is_clean().expect("R-13 helper namespace must be restored exactly");
    println!(
        "R13_CHILD_RESULT=success; iterations={}; implementation={}; version={:?}; Py_GIL_DISABLED={:?}",
        ITERATIONS, runtime.implementation, runtime.version, runtime.gil_disabled_config,
    );
}

#[test]
fn free_threaded_metadata_overlap_preserves_streams_and_report() {
    pyo3::Python::initialize();
    let runtime = runtime_info();
    if !runtime_is_supported(&runtime) {
        eprintln!(
            "skipping R-13 free-threaded metadata overlap: requires CPython 3.14t with _is_gil_enabled() == false and Py_GIL_DISABLED == 1; got implementation={:?}, version={:?}, executable={:?}, _is_gil_enabled={:?}, Py_GIL_DISABLED={:?}",
            runtime.implementation,
            runtime.version,
            runtime.executable,
            runtime.gil_enabled,
            runtime.gil_disabled_config,
        );
        return;
    }
    if env::var(CHILD_ENV).as_deref() == Ok(CHILD_ROLE) {
        run_child_case(&runtime);
    } else {
        run_parent_case();
    }
}
