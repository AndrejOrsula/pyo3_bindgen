use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const PACKAGES: &[&str] = &[
    "pyo3_bindgen",
    "pyo3_bindgen_cli",
    "pyo3_bindgen_engine",
    "pyo3_bindgen_macros",
];

const INTERFACE_PACKAGES: &[&str] = &[
    "pyo3_bindgen_engine",
    "pyo3_bindgen",
    "pyo3_bindgen_macros",
    "pyo3_bindgen_cli",
];
const FACADE_PACKAGE_PATCHES: &[&str] = &["pyo3_bindgen_engine", "pyo3_bindgen_macros"];
const MACRO_PACKAGE_PATCHES: &[&str] = &["pyo3_bindgen_engine"];
const CLI_PACKAGE_PATCHES: &[&str] =
    &["pyo3_bindgen", "pyo3_bindgen_engine", "pyo3_bindgen_macros"];
// These bounds exist to catch a hung child, not to assert build performance. Each nested `cargo`
// call compiles PyO3 and the packaged crate from scratch, so a busy machine or a fresh toolchain
// can legitimately take several minutes: a tight bound turns that into a spurious failure, which
// is exactly what a 180-second version did three times on a loaded workstation while passing when
// run alone. Keep them generous enough that only a real hang trips them.
const CARGO_TIMEOUT: Duration = Duration::from_secs(900);
const CONSUMER_TIMEOUT: Duration = Duration::from_secs(900);
const CLI_PROCESS_TIMEOUT: Duration = Duration::from_secs(120);
const DIRECT_CHILD_REAP_TIMEOUT: Duration = Duration::from_secs(5);
const CHILD_POLL_INTERVAL: Duration = Duration::from_millis(20);
const TIMEOUT_CHILD_ENV: &str = "PYO3_BINDGEN_PACKAGE_ARCHIVES_TIMEOUT_CHILD";
const TIMEOUT_CHILD_ROLE: &str = "pyo3-bindgen-package-archives-timeout-child-v1";
const CHILD_START_MARKER_ENV: &str = "PYO3_BINDGEN_PACKAGE_ARCHIVES_CHILD_START_MARKER";
const CHILD_START_MARKER: &[u8] = b"pyo3-bindgen-package-archives-child-started-v1\n";
const CLI_LONG_FLAGS: &[&str] = &[
    "--blocklist-name",
    "--check",
    "--deny-warnings",
    "--enum-class-fallback",
    "--enum-string-backed",
    "--help",
    "--include-private",
    "--ir",
    "--lenient-ir",
    "--max-ir-bytes",
    "--module-name",
    "--no-deduplicate-inherited-members",
    "--no-inherent-forwarders",
    "--no-traverse-submodules",
    "--output",
    "--output-dir",
    "--report",
    "--strict-completeness",
    "--version",
    "--warning-budget",
];
// Keep these fixtures inside the CLI package: this integration test is shipped in the CLI
// archive and must not reach back into a sibling source package when that archive is unpacked.
const MACRO_PASS_SOURCE: &str = r#"
use pyo3_bindgen::import_python;

import_python!("math");

pub fn python_pi(py: ::pyo3::Python<'_>) -> ::pyo3::PyResult<f64> {
    math::pi(py)
}
"#;
const MACRO_COMPILE_FAIL_SOURCE: &str = r#"
use pyo3_bindgen::import_python;

import_python!(42);
"#;

struct PackagedArchive {
    name: &'static str,
    root: PathBuf,
}

fn workspace_root() -> Option<PathBuf> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find(|directory| {
            std::fs::read_to_string(directory.join("Cargo.toml"))
                .is_ok_and(|manifest| manifest.contains("[workspace]"))
        })
        .map(Path::to_path_buf)
}

fn source_workspace() -> Option<(PathBuf, String)> {
    let root = workspace_root()?;
    let manifest = std::fs::read_to_string(root.join("Cargo.toml")).ok()?;
    manifest.contains("[workspace]").then_some((root, manifest))
}

struct TempDir {
    path: PathBuf,
    remove_on_drop: bool,
}

impl TempDir {
    fn create(name: &str) -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after the Unix epoch")
            .as_nanos();

        for attempt in 0..128_u32 {
            let sequence = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "pyo3_bindgen_{name}_{}_{}_{}_{}",
                std::process::id(),
                timestamp,
                sequence,
                attempt
            ));
            match fs::create_dir(&path) {
                Ok(()) => {
                    return Self {
                        path,
                        remove_on_drop: true,
                    };
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!(
                    "temporary package directory {} should be created: {error}",
                    path.display()
                ),
            }
        }

        panic!("could not allocate an exclusive temporary package directory for `{name}`");
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn retain(&mut self) {
        self.remove_on_drop = false;
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        if self.remove_on_drop {
            if let Err(error) = fs::remove_dir_all(&self.path) {
                if error.kind() != io::ErrorKind::NotFound {
                    eprintln!(
                        "failed to remove owned temporary package directory {}: {error}",
                        self.path.display()
                    );
                }
            }
        } else {
            eprintln!(
                "retaining owned temporary package directory after child timeout or uncertain termination: {}",
                self.path.display()
            );
        }
    }
}

#[derive(Debug)]
enum BoundedCommandErrorKind {
    OutputFile {
        stream: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    OutputRead {
        stream: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    Spawn {
        source: io::Error,
    },
    StatusCheck {
        source: io::Error,
        kill: io::Result<()>,
        reap: io::Result<ExitStatus>,
    },
    Timeout {
        timeout: Duration,
        kill: io::Result<()>,
        reap: io::Result<ExitStatus>,
    },
}

#[derive(Debug)]
struct BoundedCommandError {
    kind: BoundedCommandErrorKind,
    stdout_path: PathBuf,
    stderr_path: PathBuf,
    working_directory: PathBuf,
}

impl std::fmt::Display for BoundedCommandError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.kind {
            BoundedCommandErrorKind::OutputFile {
                stream,
                path,
                source,
            } => write!(
                formatter,
                "could not open {stream} log {}: {source}",
                path.display()
            )?,
            BoundedCommandErrorKind::OutputRead {
                stream,
                path,
                source,
            } => write!(
                formatter,
                "could not read {stream} log {}: {source}",
                path.display()
            )?,
            BoundedCommandErrorKind::Spawn { source } => {
                write!(formatter, "could not spawn direct child: {source}")?;
            }
            BoundedCommandErrorKind::StatusCheck { source, kill, reap } => {
                write!(formatter, "direct child status check failed: {source}; ")?;
                write_termination_results(formatter, kill, reap)?;
            }
            BoundedCommandErrorKind::Timeout {
                timeout,
                kill,
                reap,
            } => {
                write!(formatter, "direct child timed out after {timeout:?}; ")?;
                write_termination_results(formatter, kill, reap)?;
            }
        }
        write!(
            formatter,
            "; stdout log: {}; stderr log: {}; working directory: {}; only the direct child is owned",
            self.stdout_path.display(),
            self.stderr_path.display(),
            self.working_directory.display(),
        )
    }
}

fn write_termination_results(
    formatter: &mut std::fmt::Formatter<'_>,
    kill: &io::Result<()>,
    reap: &io::Result<ExitStatus>,
) -> std::fmt::Result {
    write!(
        formatter,
        "direct-child kill={kill:?}; direct-child reap={reap:?}"
    )
}

fn bounded_command_error(
    kind: BoundedCommandErrorKind,
    stdout_path: &Path,
    stderr_path: &Path,
    working_directory: &TempDir,
) -> BoundedCommandError {
    BoundedCommandError {
        kind,
        stdout_path: stdout_path.to_owned(),
        stderr_path: stderr_path.to_owned(),
        working_directory: working_directory.path().to_owned(),
    }
}

/// Kill and reap only the direct child, with a finite reap window. A child process may have
/// descendants that inherited its log handles; this helper makes no process-tree isolation claim.
fn terminate_direct_child(child: &mut Child) -> (io::Result<()>, io::Result<ExitStatus>) {
    let kill = child.kill();
    let deadline = Instant::now() + DIRECT_CHILD_REAP_TIMEOUT;
    let reap = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if Instant::now() < deadline => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                thread::sleep(remaining.min(CHILD_POLL_INTERVAL));
            }
            Ok(None) => {
                break Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "direct child did not exit during the bounded reap window",
                ));
            }
            Err(error) => break Err(error),
        }
    };
    (kill, reap)
}

/// Run a direct child with file-backed, complete stdout/stderr and a finite deadline.
///
/// The returned `Output` reads the complete logs because cargo/package protocol output is part of
/// these assertions. Only timeout and status-check paths retain the caller-owned directory.
fn run_bounded(
    command: &mut Command,
    timeout: Duration,
    working_directory: &mut TempDir,
) -> Result<Output, Box<BoundedCommandError>> {
    static NEXT_LOG_ID: AtomicU64 = AtomicU64::new(0);
    let log_id = NEXT_LOG_ID.fetch_add(1, Ordering::Relaxed);
    let stdout_path = working_directory
        .path()
        .join(format!("bounded-command-{log_id}.stdout"));
    let stderr_path = working_directory
        .path()
        .join(format!("bounded-command-{log_id}.stderr"));
    let stdout = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&stdout_path)
        .map_err(|source| {
            Box::new(bounded_command_error(
                BoundedCommandErrorKind::OutputFile {
                    stream: "stdout",
                    path: stdout_path.clone(),
                    source,
                },
                &stdout_path,
                &stderr_path,
                working_directory,
            ))
        })?;
    let stderr = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&stderr_path)
        .map_err(|source| {
            Box::new(bounded_command_error(
                BoundedCommandErrorKind::OutputFile {
                    stream: "stderr",
                    path: stderr_path.clone(),
                    source,
                },
                &stdout_path,
                &stderr_path,
                working_directory,
            ))
        })?;
    let mut child = command
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()
        .map_err(|source| {
            Box::new(bounded_command_error(
                BoundedCommandErrorKind::Spawn { source },
                &stdout_path,
                &stderr_path,
                working_directory,
            ))
        })?;
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                thread::sleep(remaining.min(CHILD_POLL_INTERVAL));
            }
            Ok(None) => {
                working_directory.retain();
                let (kill, reap) = terminate_direct_child(&mut child);
                return Err(Box::new(bounded_command_error(
                    BoundedCommandErrorKind::Timeout {
                        timeout,
                        kill,
                        reap,
                    },
                    &stdout_path,
                    &stderr_path,
                    working_directory,
                )));
            }
            Err(error) => {
                working_directory.retain();
                let (kill, reap) = terminate_direct_child(&mut child);
                return Err(Box::new(bounded_command_error(
                    BoundedCommandErrorKind::StatusCheck {
                        source: error,
                        kill,
                        reap,
                    },
                    &stdout_path,
                    &stderr_path,
                    working_directory,
                )));
            }
        }
    };

    let stdout = fs::read(&stdout_path).map_err(|source| {
        Box::new(bounded_command_error(
            BoundedCommandErrorKind::OutputRead {
                stream: "stdout",
                path: stdout_path.clone(),
                source,
            },
            &stdout_path,
            &stderr_path,
            working_directory,
        ))
    })?;
    let stderr = fs::read(&stderr_path).map_err(|source| {
        Box::new(bounded_command_error(
            BoundedCommandErrorKind::OutputRead {
                stream: "stderr",
                path: stderr_path.clone(),
                source,
            },
            &stdout_path,
            &stderr_path,
            working_directory,
        ))
    })?;
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

fn assert_command_succeeds(name: &str, output: std::process::Output) {
    assert!(
        output.status.success(),
        "{name} failed with {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn diagnostics(output: &Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

// A successful cargo status does not prove that a libtest filter matched anything. The
// per-test result line is the positive execution marker; avoid timing and locale-sensitive
// summary parsing while requiring the expected test exactly once.
fn parse_libtest_passed_tests(output: &str) -> Vec<&str> {
    output
        .lines()
        .filter_map(|line| line.trim().strip_prefix("test ")?.strip_suffix(" ... ok"))
        .collect()
}

fn validate_libtest_execution(output: &str, expected_test_name: &str) -> Result<(), String> {
    let passed_tests = parse_libtest_passed_tests(output);
    let expected_count = passed_tests
        .iter()
        .filter(|&&test_name| test_name == expected_test_name)
        .count();
    if expected_count != 1 {
        return Err(format!(
            "expected exactly one passing `{expected_test_name}` test, found {expected_count}; passing tests: {passed_tests:?}"
        ));
    }
    Ok(())
}

fn assert_libtest_execution(name: &str, output: &Output, expected_test_name: &str) {
    let text = diagnostics(output);
    if let Err(error) = validate_libtest_execution(&text, expected_test_name) {
        panic!(
            "{name} did not prove execution of `{expected_test_name}`: {error}\noutput:\n{text}"
        );
    }
}

#[cfg(test)]
mod libtest_execution_tests {
    use super::validate_libtest_execution;

    const PASSING_TEST: &str = "expected_test";

    #[test]
    fn accepts_one_named_passing_test() {
        let output = format!("running 1 test\ntest {PASSING_TEST} ... ok");
        assert!(validate_libtest_execution(&output, PASSING_TEST).is_ok());
    }

    #[test]
    fn rejects_successful_zero_test_child() {
        let output = "running 0 tests\n\ntest result: ok. 0 passed; 0 failed";
        let error = validate_libtest_execution(output, PASSING_TEST).unwrap_err();
        assert!(error.contains("found 0"));
    }

    #[test]
    fn rejects_missing_or_duplicate_expected_test_line() {
        let missing = "running 1 test\ntest other_test ... ok";
        let missing_error = validate_libtest_execution(missing, PASSING_TEST).unwrap_err();
        assert!(missing_error.contains("found 0"));

        let duplicate =
            format!("running 2 tests\ntest {PASSING_TEST} ... ok\ntest {PASSING_TEST} ... ok");
        let duplicate_error = validate_libtest_execution(&duplicate, PASSING_TEST).unwrap_err();
        assert!(duplicate_error.contains("found 2"));
    }
}

#[test]
fn bounded_consumer_timeout_reaps_direct_child_and_retains_directory() {
    if std::env::var(TIMEOUT_CHILD_ENV).as_deref() == Ok(TIMEOUT_CHILD_ROLE) {
        let marker_path = std::env::var_os(CHILD_START_MARKER_ENV)
            .expect("timeout child must receive its start-marker path");
        fs::write(marker_path, CHILD_START_MARKER).expect("timeout child must write start marker");
        loop {
            thread::sleep(Duration::from_secs(60));
        }
    }
    assert!(
        std::env::var_os(TIMEOUT_CHILD_ENV).is_none(),
        "timeout regression child role must be exact"
    );

    let mut directory = TempDir::create("package_archives_timeout");
    let root = directory.path().to_owned();
    let marker_path = root.join("child.started");
    let mut command = Command::new(std::env::current_exe().expect("test executable must exist"));
    command
        .args([
            "--exact",
            "bounded_consumer_timeout_reaps_direct_child_and_retains_directory",
            "--nocapture",
        ])
        .current_dir(&root)
        .env(TIMEOUT_CHILD_ENV, TIMEOUT_CHILD_ROLE)
        .env(CHILD_START_MARKER_ENV, &marker_path);

    let error = run_bounded(&mut command, Duration::from_secs(5), &mut directory)
        .expect_err("the deliberately sleeping direct child must hit the deadline");
    assert_eq!(
        fs::read(&marker_path).expect("timeout evidence requires a started child"),
        CHILD_START_MARKER,
        "the exact child-start marker proves that the child test executed"
    );
    match &error.kind {
        BoundedCommandErrorKind::Timeout { kill, reap, .. } => {
            assert!(kill.is_ok(), "direct-child kill error was lost: {kill:?}");
            assert!(reap.is_ok(), "direct-child reap error was lost: {reap:?}");
        }
        other => panic!("expected a bounded timeout, got {other:?}"),
    }
    let stdout_path = error.stdout_path.clone();
    let stderr_path = error.stderr_path.clone();
    assert!(stdout_path.is_file(), "timeout stdout log must be retained");
    assert!(stderr_path.is_file(), "timeout stderr log must be retained");

    // This fixture creates no descendants. The direct child was reaped above, so cleanup of this
    // exact test-owned directory is safe after proving that Drop retains it on timeout.
    drop(directory);
    assert!(
        root.is_dir(),
        "timeout must retain the owned directory and file-backed logs"
    );
    assert!(
        stdout_path.is_file(),
        "retained timeout stdout log disappeared"
    );
    assert!(
        stderr_path.is_file(),
        "retained timeout stderr log disappeared"
    );
    fs::remove_dir_all(&root).expect("test-owned retained directory must be removable");
}

fn run_or_panic(
    name: &str,
    command: &mut Command,
    timeout: Duration,
    working_directory: &mut TempDir,
) -> Output {
    run_bounded(command, timeout, working_directory)
        .unwrap_or_else(|error| panic!("{name} should run within {timeout:?}: {error}"))
}

fn host_target(working_directory: &mut TempDir) -> String {
    let rustc = std::env::var_os("RUSTC").map_or_else(
        || {
            Path::new(env!("CARGO"))
                .parent()
                .expect("Cargo executable should have a parent directory")
                .join(if cfg!(windows) { "rustc.exe" } else { "rustc" })
        },
        PathBuf::from,
    );
    let mut command = Command::new(rustc);
    command.arg("-vV");
    let output = run_or_panic(
        "matching rustc host target query",
        &mut command,
        CARGO_TIMEOUT,
        working_directory,
    );
    assert!(
        output.status.success(),
        "matching rustc host target query failed with {}\n{}",
        output.status,
        diagnostics(&output)
    );
    String::from_utf8(output.stdout)
        .expect("matching rustc host target query should return UTF-8")
        .lines()
        .find_map(|line| line.strip_prefix("host: ").map(str::to_owned))
        .filter(|target| !target.is_empty())
        .expect("matching rustc host target query should report a non-empty host target")
}

fn package_and_extract(
    root: &Path,
    version: &str,
    temp: &mut TempDir,
    name: &'static str,
) -> PackagedArchive {
    let package_target = temp.path().join(format!("{name}-package-target"));
    let mut package = Command::new(env!("CARGO"));
    package
        .args([
            "package",
            "--offline",
            "--locked",
            "--allow-dirty",
            "--no-verify",
            "-p",
            name,
            "--target-dir",
        ])
        .arg(&package_target)
        .current_dir(root);
    for &dependency in package_patch_names(name) {
        let dependency_path = root.join("crates").join(dependency);
        // Release versions may not exist in the local index yet. Cargo still normalizes the
        // archived manifest to registry dependencies; this patch only lets the source workspace
        // prepare that archive without publishing sibling packages first.
        append_patch_config(&mut package, dependency, &dependency_path);
    }
    let package = run_or_panic("cargo package", &mut package, CARGO_TIMEOUT, temp);
    assert_command_succeeds(&format!("cargo package for {name}"), package);

    let archive = package_target
        .join("package")
        .join(format!("{name}-{version}.crate"));
    assert!(
        archive.is_file(),
        "cargo package should create {}",
        archive.display()
    );
    let extracted_root = temp.path().join("archives");
    std::fs::create_dir_all(&extracted_root)
        .expect("archive extraction directory should be created");
    let mut extract = Command::new("tar");
    extract
        .args(["-xzf"])
        .arg(&archive)
        .arg("-C")
        .arg(&extracted_root);
    let extract = run_or_panic(
        &format!("extract packaged {name}"),
        &mut extract,
        CARGO_TIMEOUT,
        temp,
    );
    assert_command_succeeds(&format!("extract packaged {name}"), extract);

    let extracted_package = extracted_root.join(format!("{name}-{version}"));
    assert!(
        extracted_package.join("Cargo.toml").is_file(),
        "extracted {name} archive should contain Cargo.toml"
    );
    PackagedArchive {
        name,
        root: extracted_package,
    }
}

fn package_patch_names(name: &str) -> &'static [&'static str] {
    match name {
        "pyo3_bindgen_engine" => &[],
        "pyo3_bindgen" => FACADE_PACKAGE_PATCHES,
        "pyo3_bindgen_macros" => MACRO_PACKAGE_PATCHES,
        "pyo3_bindgen_cli" => CLI_PACKAGE_PATCHES,
        other => panic!("unsupported package archive {other}"),
    }
}

fn package_interfaces(root: &Path, manifest: &str, temp: &mut TempDir) -> Vec<PackagedArchive> {
    let version = manifest
        .lines()
        .find_map(|line| line.strip_prefix("version = \"")?.strip_suffix('"'))
        .expect("workspace package version should be present");
    let archives = INTERFACE_PACKAGES
        .iter()
        .map(|&name| package_and_extract(root, version, temp, name))
        .collect::<Vec<_>>();
    assert_eq!(
        archives.len(),
        INTERFACE_PACKAGES.len(),
        "every public package interface must be extracted"
    );
    archives
}

fn cargo_patch_section(archives: &[PackagedArchive]) -> String {
    let mut section = String::from("[patch.crates-io]\n");
    for archive in archives {
        section.push_str(&format!(
            "{} = {{ path = {:?} }}\n",
            archive.name,
            archive.root.to_string_lossy().to_string()
        ));
    }
    section
}

fn append_patch_config(command: &mut Command, name: &str, path: &Path) {
    command.arg("--config").arg(format!(
        "patch.crates-io.{name}.path={:?}",
        path.to_string_lossy().to_string()
    ));
}

fn feature_list(features: &[&str]) -> String {
    features
        .iter()
        .map(|feature| format!("\"{feature}\""))
        .collect::<Vec<_>>()
        .join(", ")
}

fn dependency_spec(archive: &PackagedArchive, features: &[&str]) -> String {
    format!(
        "{} = {{ path = {:?}, features = [{}] }}",
        archive.name,
        archive.root.to_string_lossy().to_string(),
        feature_list(features)
    )
}

fn write_consumer(
    temp: &TempDir,
    name: &str,
    dependency: &PackagedArchive,
    dependency_features: &[&str],
    extra_dependencies: &str,
    source: &str,
    archives: &[PackagedArchive],
) {
    std::fs::create_dir_all(temp.path().join("src")).expect("consumer source directory writable");
    let manifest = format!(
        "[package]\nname = \"{name}\"\nversion = \"0.0.0\"\nedition = \"2021\"\npublish = false\n\n[workspace]\n\n[dependencies]\n{}\n{}\n{}",
        dependency_spec(dependency, dependency_features),
        extra_dependencies,
        cargo_patch_section(archives),
    );
    std::fs::write(temp.path().join("Cargo.toml"), manifest)
        .expect("isolated consumer manifest should be writable");
    std::fs::write(temp.path().join("src/lib.rs"), source)
        .expect("isolated consumer source should be writable");
}

fn cargo_consumer_check(name: &str, temp: &mut TempDir) -> Output {
    let target = temp.path().join("target");
    let mut lockfile = Command::new(env!("CARGO"));
    lockfile
        .args(["generate-lockfile", "--offline"])
        .current_dir(temp.path())
        .env("CARGO_TARGET_DIR", &target);
    let lockfile = run_or_panic(
        "cargo generate-lockfile",
        &mut lockfile,
        CONSUMER_TIMEOUT,
        temp,
    );
    assert_command_succeeds(&format!("{name} consumer lockfile"), lockfile);

    let mut check = Command::new(env!("CARGO"));
    check
        .args([
            "check",
            "--offline",
            "--locked",
            "--all-features",
            "--quiet",
        ])
        .current_dir(temp.path())
        .env("CARGO_TARGET_DIR", target);
    run_or_panic("cargo check", &mut check, CONSUMER_TIMEOUT, temp)
}

fn assert_library_consumer(archives: &[PackagedArchive], package: &str, source: &str) {
    let archive = archives
        .iter()
        .find(|archive| archive.name == package)
        .unwrap_or_else(|| panic!("packaged {package} archive must be available"));
    let mut consumer = TempDir::create(&format!("{package}_consumer"));
    let extra_dependencies = if matches!(package, "pyo3_bindgen" | "pyo3_bindgen_macros") {
        "pyo3 = { version = \"=0.29.2\", default-features = false, features = [\"py-clone\", \"num-complex\"] }\n"
    } else {
        ""
    };
    let features = match package {
        "pyo3_bindgen_engine" => ["numpy", "num-complex"].as_slice(),
        "pyo3_bindgen" => ["macros", "numpy", "num-complex"].as_slice(),
        "pyo3_bindgen_macros" => ["numpy", "num-complex"].as_slice(),
        other => panic!("unsupported library consumer {other}"),
    };
    write_consumer(
        &consumer,
        &format!("{package}_archive_consumer"),
        archive,
        features,
        extra_dependencies,
        source,
        archives,
    );
    let output = cargo_consumer_check(&format!("packaged {package} consumer"), &mut consumer);
    assert_command_succeeds(&format!("packaged {package} isolated consumer"), output);
}

fn engine_consumer_source() -> &'static str {
    r#"
use pyo3_bindgen_engine::{Codegen, Config, SchemaIr};

pub fn public_probe() {
    let _ = Codegen::default();
    let _ = Config::default();
    let _ = SchemaIr { modules: Vec::new() };
}
"#
}

fn facade_consumer_source() -> &'static str {
    r#"
use pyo3_bindgen::{import_python, Codegen, Config, SchemaIr};
use pyo3_bindgen::publication::{ExitClass, OperationCode, OperationFailure, Request};

import_python!("math");

pub fn public_probe(py: ::pyo3::Python<'_>) -> ::pyo3::PyResult<f64> {
    let _ = Codegen::default();
    let _ = Config::default();
    let _ = SchemaIr { modules: Vec::new() };
    let outcome = Request::new(None, None, None, false).reject(
        OperationFailure::new(OperationCode::InvalidArguments, "consumer probe")
    );
    assert_eq!(outcome.exit_class(), ExitClass::Failure);
    math::pi(py)
}
"#
}

fn macro_consumer_source(fixture: &str) -> String {
    fixture.replace(
        "use pyo3_bindgen::import_python;",
        "use pyo3_bindgen_macros::import_python;",
    )
}

fn assert_packaged_macro_contract(archives: &[PackagedArchive]) {
    let macro_archive = archives
        .iter()
        .find(|archive| archive.name == "pyo3_bindgen_macros")
        .expect("packaged macro archive must be available");
    let extra_dependencies =
        "pyo3 = { version = \"=0.29.2\", default-features = false, features = [\"py-clone\", \"num-complex\"] }\n";
    let mut pass = TempDir::create("packaged_macro_pass");
    write_consumer(
        &pass,
        "packaged_macro_pass_consumer",
        macro_archive,
        &["numpy", "num-complex"],
        extra_dependencies,
        &macro_consumer_source(MACRO_PASS_SOURCE),
        archives,
    );
    let pass_output = cargo_consumer_check("packaged macro pass consumer", &mut pass);
    assert_command_succeeds("packaged macro pass consumer", pass_output);

    let mut compile_fail = TempDir::create("packaged_macro_compile_fail");
    write_consumer(
        &compile_fail,
        "packaged_macro_compile_fail_consumer",
        macro_archive,
        &["numpy", "num-complex"],
        extra_dependencies,
        &macro_consumer_source(MACRO_COMPILE_FAIL_SOURCE),
        archives,
    );
    let fail_output =
        cargo_consumer_check("packaged macro compile-fail consumer", &mut compile_fail);
    assert!(
        !fail_output.status.success(),
        "packaged macro compile-fail consumer should fail\n{}",
        diagnostics(&fail_output)
    );
    assert!(
        diagnostics(&fail_output).contains("expected string literal"),
        "packaged macro compile-fail diagnostic changed:\n{}",
        diagnostics(&fail_output)
    );
}

fn cli_executable_name() -> &'static str {
    if cfg!(windows) {
        "pyo3_bindgen.exe"
    } else {
        "pyo3_bindgen"
    }
}

fn assert_packaged_cli_contract(root: &mut TempDir, archives: &[PackagedArchive]) {
    let cli_archive = archives
        .iter()
        .find(|archive| archive.name == "pyo3_bindgen_cli")
        .expect("packaged CLI archive must be available");

    let build_target = root.path().join("packaged-cli-target");
    let manifest = cli_archive.root.join("Cargo.toml");
    let mut lockfile = Command::new(env!("CARGO"));
    lockfile
        .args(["generate-lockfile", "--offline", "--manifest-path"])
        .arg(&manifest)
        .current_dir(root.path())
        .env("CARGO_TARGET_DIR", &build_target);
    for archive in archives {
        append_patch_config(&mut lockfile, archive.name, &archive.root);
    }
    let lockfile = run_or_panic(
        "packaged CLI lockfile generation",
        &mut lockfile,
        CONSUMER_TIMEOUT,
        root,
    );
    assert_command_succeeds("packaged CLI lockfile", lockfile);

    let mut build = Command::new(env!("CARGO"));
    build
        .args([
            "build",
            "--offline",
            "--locked",
            "--all-features",
            "--quiet",
            "--manifest-path",
        ])
        .arg(&manifest)
        .arg("--bin")
        .arg("pyo3_bindgen")
        .current_dir(root.path())
        .env("CARGO_TARGET_DIR", &build_target);
    for archive in archives {
        append_patch_config(&mut build, archive.name, &archive.root);
    }
    let build = run_or_panic("packaged CLI build", &mut build, CONSUMER_TIMEOUT, root);
    assert_command_succeeds("packaged CLI build", build);

    let executable = build_target.join("debug").join(cli_executable_name());
    assert!(
        executable.is_file(),
        "packaged CLI build should create {}",
        executable.display()
    );
    let mut help = Command::new(&executable);
    help.arg("--help").current_dir(root.path());
    let help = run_or_panic("packaged CLI --help", &mut help, CLI_PROCESS_TIMEOUT, root);
    let help_text = String::from_utf8_lossy(&help.stdout).into_owned();
    assert_command_succeeds("packaged CLI --help", help);
    assert!(help_text.contains("Usage: pyo3_bindgen"));
    let actual_long_flags = help_text
        .lines()
        .map(str::trim_start)
        .filter(|line| line.starts_with('-'))
        .flat_map(|line| line.split_whitespace())
        .map(|token| token.trim_end_matches(','))
        .filter(|token| token.starts_with("--"))
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        actual_long_flags,
        CLI_LONG_FLAGS.iter().copied().collect(),
        "packaged CLI help must expose the complete public long-flag contract"
    );

    let mut version = Command::new(&executable);
    version.arg("--version").current_dir(root.path());
    let version = run_or_panic(
        "packaged CLI --version",
        &mut version,
        CLI_PROCESS_TIMEOUT,
        root,
    );
    let version_text = String::from_utf8_lossy(&version.stdout).into_owned();
    assert_command_succeeds("packaged CLI --version", version);
    assert!(
        version_text.contains(env!("CARGO_PKG_VERSION")),
        "packaged CLI --version should print the package version: {version_text}"
    );

    let ir = root.path().join("minimal_ir.json");
    std::fs::write(&ir, include_str!("fixtures/minimal_ir.json").as_bytes())
        .expect("packaged CLI IR fixture should be writable");
    let output_path = root.path().join("generated.rs");
    let report_path = root.path().join("generation-report.json");
    let mut generate = Command::new(&executable);
    generate
        .args(["--ir"])
        .arg(&ir)
        .args(["--output"])
        .arg(&output_path)
        .args(["--report"])
        .arg(&report_path)
        .current_dir(root.path());
    let generate = run_or_panic(
        "packaged CLI external-IR generation",
        &mut generate,
        CLI_PROCESS_TIMEOUT,
        root,
    );
    assert_command_succeeds("packaged CLI external-IR generation", generate);
    assert!(
        std::fs::metadata(&output_path)
            .expect("packaged CLI should publish generated output")
            .len()
            > 0
    );
    assert!(
        std::fs::metadata(&report_path).is_ok(),
        "packaged CLI should publish a generation report"
    );

    let check_report_path = root.path().join("check-report.json");
    let mut check = Command::new(&executable);
    check
        .args(["--ir"])
        .arg(&ir)
        .args(["--output"])
        .arg(&output_path)
        .arg("--check")
        .args(["--report"])
        .arg(&check_report_path)
        .current_dir(root.path());
    let check = run_or_panic(
        "packaged CLI external-IR check",
        &mut check,
        CLI_PROCESS_TIMEOUT,
        root,
    );
    assert_command_succeeds("packaged CLI external-IR check", check);
}

fn assert_packaged_engine_test(
    manifest: &Path,
    target_dir: &Path,
    name: &str,
    expected_test_name: &str,
    test_args: &[&str],
    working_directory: &mut TempDir,
) {
    let mut command = Command::new(env!("CARGO"));
    command
        .arg("test")
        .arg("--manifest-path")
        .arg(manifest)
        .args(["--all-features", "--locked"])
        .args(test_args)
        .env("CARGO_TARGET_DIR", target_dir);
    let output = run_or_panic(name, &mut command, CARGO_TIMEOUT, working_directory);
    if output.status.success() {
        assert_libtest_execution(name, &output, expected_test_name);
    } else {
        assert_command_succeeds(name, output);
    }
}

#[test]
fn publishable_archives_contain_canonical_licenses_and_portable_readme() {
    // `cargo package` verifies the CLI from an isolated single-crate workspace.
    // Avoid recursively packaging from that extracted archive; this regression
    // belongs to the source workspace release gate.
    let Some((root, manifest)) = source_workspace() else {
        return;
    };

    let canonical_mit =
        std::fs::read(root.join("LICENSE-MIT")).expect("workspace MIT license should be readable");
    let canonical_apache = std::fs::read(root.join("LICENSE-APACHE"))
        .expect("workspace Apache license should be readable");
    let readme = std::fs::read_to_string(root.join("README.md"))
        .expect("workspace README should be readable");
    let mut command_logs = TempDir::create("package_list_command_logs");

    for package in PACKAGES {
        let package_dir = root.join("crates").join(package);
        assert_eq!(
            std::fs::read(package_dir.join("LICENSE-MIT"))
                .expect("package MIT license should be readable"),
            canonical_mit,
            "{package} must ship the canonical MIT license"
        );
        assert_eq!(
            std::fs::read(package_dir.join("LICENSE-APACHE"))
                .expect("package Apache license should be readable"),
            canonical_apache,
            "{package} must ship the canonical Apache license"
        );

        let mut command = Command::new(env!("CARGO"));
        command
            .args([
                "package",
                "--list",
                "--locked",
                "--allow-dirty",
                "-p",
                package,
            ])
            .current_dir(&root);
        let output = run_or_panic(
            "cargo package --list",
            &mut command,
            CARGO_TIMEOUT,
            &mut command_logs,
        );
        assert!(
            output.status.success(),
            "cargo package --list failed for {package}:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let files =
            String::from_utf8(output.stdout).expect("cargo package --list output should be UTF-8");
        for required in ["LICENSE-MIT", "LICENSE-APACHE", "README.md"] {
            assert!(
                files.lines().any(|file| file == required),
                "{package} archive is missing {required}:\n{files}"
            );
        }
        if *package == "pyo3_bindgen_engine" {
            for fixture in ["num_complex_consumer", "numpy_consumer"] {
                for template in ["Cargo.toml.fixture", "Cargo.lock.fixture"] {
                    let required = format!("tests/fixtures/{fixture}/{template}");
                    assert!(
                        files.lines().any(|file| file == required.as_str()),
                        "{package} archive is missing {required}:\n{files}"
                    );
                }
            }
        }
    }

    let version = manifest
        .lines()
        .find_map(|line| line.strip_prefix("version = \"")?.strip_suffix('"'))
        .expect("workspace package version should be present");
    let release_root = format!("https://github.com/AndrejOrsula/pyo3_bindgen/blob/v{version}/");
    assert!(readme.contains(&format!("{release_root}LICENSE-MIT")));
    assert!(readme.contains(&format!("{release_root}LICENSE-APACHE")));
    assert!(!readme.contains("](LICENSE-MIT)"));
    assert!(!readme.contains("](LICENSE-APACHE)"));
    assert!(!readme.contains("](./crates/pyo3_bindgen_engine/"));
}

#[test]
fn packaged_engine_feature_consumers_are_self_contained() {
    let Some((root, manifest)) = source_workspace() else {
        return;
    };
    let version = manifest
        .lines()
        .find_map(|line| line.strip_prefix("version = \"")?.strip_suffix('"'))
        .expect("workspace package version should be present");
    let mut temp = TempDir::create("packaged_engine_test");
    let package_target = temp.path().join("package-target");

    let mut package_command = Command::new(env!("CARGO"));
    package_command
        .args([
            "package",
            "--locked",
            "--allow-dirty",
            "--no-verify",
            "-p",
            "pyo3_bindgen_engine",
            "--target-dir",
        ])
        .arg(&package_target)
        .current_dir(&root);
    let package = run_or_panic(
        "cargo package for extracted engine test",
        &mut package_command,
        CARGO_TIMEOUT,
        &mut temp,
    );
    assert_command_succeeds("cargo package for extracted engine test", package);

    let archive = package_target
        .join("package")
        .join(format!("pyo3_bindgen_engine-{version}.crate"));
    let extracted_root = temp.path().join("extracted");
    std::fs::create_dir_all(&extracted_root).expect("archive extraction directory should exist");
    let mut extract_command = Command::new("tar");
    extract_command
        .arg("-xzf")
        .arg(&archive)
        .arg("-C")
        .arg(&extracted_root);
    let extract = run_or_panic(
        "extract packaged engine",
        &mut extract_command,
        CARGO_TIMEOUT,
        &mut temp,
    );
    assert_command_succeeds("extract packaged engine", extract);

    let extracted_package = extracted_root.join(format!("pyo3_bindgen_engine-{version}"));
    let extracted_manifest = extracted_package.join("Cargo.toml");
    let extracted_lock = extracted_package.join("Cargo.lock");
    if !extracted_lock.exists() {
        // Cargo 1.83 does not include a lockfile in library crate archives. Seed
        // its standalone graph from the checked-in workspace lock, then let the
        // same Cargo prune unrelated workspace packages before the locked test. The standalone
        // package has no workspace resolver metadata, so this preparatory pass necessarily
        // rewrites the seed. Restrict it to the actual host target; otherwise Cargo loads unused
        // target-only manifests such as `wit-bindgen` 0.57.1, which Cargo 1.83 cannot parse.
        std::fs::copy(root.join("Cargo.lock"), &extracted_lock)
            .expect("workspace lockfile should seed legacy Cargo package tests");
        let target = host_target(&mut temp);
        let mut prepare_lock = Command::new(env!("CARGO"));
        prepare_lock
            .args(["metadata", "--manifest-path"])
            .arg(&extracted_manifest)
            .args([
                "--format-version",
                "1",
                "--all-features",
                "--filter-platform",
            ])
            .arg(target)
            .env("CARGO_TARGET_DIR", temp.path().join("metadata-target"));
        let prepare_lock = run_or_panic(
            "prepare standalone engine lockfile",
            &mut prepare_lock,
            CARGO_TIMEOUT,
            &mut temp,
        );
        assert_command_succeeds("prepare standalone engine lockfile", prepare_lock);
    }
    let test_target = temp.path().join("test-target");
    assert_packaged_engine_test(
        &extracted_manifest,
        &test_target,
        "packaged engine build metadata test",
        "build_script_resolves_manifest_directory_at_execution_time",
        &["--test", "build_metadata"],
        &mut temp,
    );
    assert_packaged_engine_test(
        &extracted_manifest,
        &test_target,
        "packaged engine report provenance test",
        "bindgen_report_tracks_modules_outputs_and_warnings",
        &[
            "--test",
            "bindgen",
            "bindgen_report_tracks_modules_outputs_and_warnings",
            "--",
            "--exact",
        ],
        &mut temp,
    );
    assert_packaged_engine_test(
        &extracted_manifest,
        &test_target,
        "packaged engine feature test",
        "exact_import_alias_type_references_compile_for_consumers",
        &["--test", "feature_bindings"],
        &mut temp,
    );
}

#[test]
fn packaged_public_interfaces_are_exercised_without_recursive_packaging() {
    let Some((root, manifest)) = source_workspace() else {
        eprintln!("SKIP packaged public interfaces: workspace root is unavailable");
        return;
    };
    let mut archive_temp = TempDir::create("public_interface_archives");
    let archives = package_interfaces(&root, &manifest, &mut archive_temp);

    // These consumers are separate workspaces and depend only on extracted archive roots.
    // No cargo package/test command is ever run from an extracted archive, so this test cannot
    // recursively package itself through the archived test sources.
    assert_library_consumer(&archives, "pyo3_bindgen_engine", engine_consumer_source());
    assert_library_consumer(&archives, "pyo3_bindgen", facade_consumer_source());
    assert_packaged_macro_contract(&archives);
    assert_packaged_cli_contract(&mut archive_temp, &archives);
}
