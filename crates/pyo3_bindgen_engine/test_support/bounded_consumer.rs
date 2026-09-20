#![allow(dead_code)]

use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

pub const CARGO_COMMAND_TIMEOUT: Duration = Duration::from_secs(900);
const DIRECT_CHILD_REAP_TIMEOUT: Duration = Duration::from_secs(30);
const CHILD_POLL_INTERVAL: Duration = Duration::from_millis(10);
const MAX_DIAGNOSTIC_BYTES: usize = 128 * 1024;
const TIMEOUT_CHILD_ROLE_ENV: &str = "PYO3_BINDGEN_BOUNDED_CONSUMER_TIMEOUT_CHILD";
const TIMEOUT_CHILD_ROLE: &str = "pyo3-bindgen-bounded-consumer-timeout-child-v1";
const TIMEOUT_CHILD_MARKER_ENV: &str = "PYO3_BINDGEN_BOUNDED_CONSUMER_START_MARKER";
const TIMEOUT_CHILD_START_MARKER: &[u8] = b"pyo3-bindgen-bounded-consumer-child-started-v1\n";
const TIMEOUT_CHILD_SLEEP: Duration = Duration::from_secs(60);
const TIMEOUT_CHILD_DEADLINE: Duration = Duration::from_secs(5);

static NEXT_TEMP_DIR: AtomicU64 = AtomicU64::new(0);
static NEXT_LOG_FILE: AtomicU64 = AtomicU64::new(0);

#[allow(dead_code)]
pub enum ConsumerManifest<'a> {
    Fixture(&'a str),
    Inline(&'a str),
}

/// A directory allocated by this test task with an atomic create operation.
///
/// The runner owns and reaps only its direct child. It provides no process-tree isolation, so a
/// timeout or uncertain status retains this directory and its file-backed reports.
pub struct OwnedTempDir {
    path: PathBuf,
    cleanup_on_drop: bool,
    retained: bool,
}

impl OwnedTempDir {
    pub fn allocate(tag: &str) -> io::Result<Self> {
        let tag = sanitize_component(tag);
        let process_id = std::process::id();
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();

        for attempt in 0..128_u32 {
            let sequence = NEXT_TEMP_DIR.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "pyo3_bindgen_bounded_consumer_{tag}_{process_id}_{timestamp}_{sequence}_{attempt}"
            ));
            match fs::create_dir(&path) {
                Ok(()) => {
                    return Ok(Self {
                        path,
                        cleanup_on_drop: true,
                        retained: false,
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }

        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("could not allocate an exclusive consumer directory for `{tag}`"),
        ))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn retain(&mut self) {
        self.cleanup_on_drop = false;
        self.retained = true;
    }

    pub fn cleanup_now(&mut self) -> io::Result<()> {
        match fs::remove_dir_all(&self.path) {
            Ok(()) => {
                self.cleanup_on_drop = false;
                self.retained = false;
                Ok(())
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                self.cleanup_on_drop = false;
                self.retained = false;
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    /// Run one direct child with file-backed stdout/stderr and a finite deadline.
    ///
    /// The caller controls the command's current directory and environment, including any
    /// `CARGO_TARGET_DIR`. The directory's target is never inferred or removed by this method;
    /// only this exact owned directory is cleaned on an ordinary drop.
    pub fn run(
        &mut self,
        command: &mut Command,
        timeout: Duration,
    ) -> Result<BoundedCommandOutput, Box<BoundedCommandError>> {
        let log_id = NEXT_LOG_FILE.fetch_add(1, Ordering::Relaxed);
        let stdout_path = self.path.join(format!("bounded-command-{log_id}.stdout"));
        let stderr_path = self.path.join(format!("bounded-command-{log_id}.stderr"));
        let label = format!(
            "bounded command `{}`",
            command.get_program().to_string_lossy()
        );
        run_bounded_command(label, command, timeout, self, &stdout_path, &stderr_path)
    }
}

impl Drop for OwnedTempDir {
    fn drop(&mut self) {
        if self.cleanup_on_drop {
            if let Err(error) = fs::remove_dir_all(&self.path) {
                if error.kind() != io::ErrorKind::NotFound {
                    eprintln!(
                        "failed to remove owned consumer directory {}: {error}",
                        self.path.display()
                    );
                }
            }
        } else if self.retained {
            eprintln!(
                "retaining owned consumer directory after uncertain child termination: {}",
                self.path.display()
            );
        }
    }
}

fn sanitize_component(value: &str) -> String {
    let component: String = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect();
    if component.is_empty() {
        "consumer".to_owned()
    } else {
        component
    }
}

#[derive(Debug)]
pub struct BoundedCommandOutput {
    pub status: ExitStatus,
    pub stdout: String,
    pub stderr: String,
    pub stdout_path: PathBuf,
    pub stderr_path: PathBuf,
}

#[derive(Debug)]
enum BoundedCommandErrorKind {
    OutputFile {
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
pub struct BoundedCommandError {
    label: String,
    kind: BoundedCommandErrorKind,
    stdout: String,
    stderr: String,
    stdout_path: PathBuf,
    stderr_path: PathBuf,
    working_directory: PathBuf,
    retained_working_directory: bool,
}

impl BoundedCommandError {
    pub fn is_timeout(&self) -> bool {
        matches!(&self.kind, BoundedCommandErrorKind::Timeout { .. })
    }

    pub fn termination_succeeded(&self) -> bool {
        match &self.kind {
            BoundedCommandErrorKind::StatusCheck { kill, reap, .. }
            | BoundedCommandErrorKind::Timeout { kill, reap, .. } => kill.is_ok() && reap.is_ok(),
            BoundedCommandErrorKind::OutputFile { .. } | BoundedCommandErrorKind::Spawn { .. } => {
                false
            }
        }
    }

    pub fn retained_working_directory(&self) -> bool {
        self.retained_working_directory
    }

    pub fn stdout(&self) -> &str {
        &self.stdout
    }

    pub fn stderr(&self) -> &str {
        &self.stderr
    }

    pub fn stdout_path(&self) -> &Path {
        &self.stdout_path
    }

    pub fn stderr_path(&self) -> &Path {
        &self.stderr_path
    }

    pub fn working_directory(&self) -> &Path {
        &self.working_directory
    }
}

impl std::fmt::Display for BoundedCommandError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: ", self.label)?;
        match &self.kind {
            BoundedCommandErrorKind::OutputFile {
                stream,
                path,
                source,
            } => write!(
                formatter,
                "could not open {stream} report {}: {source}",
                path.display()
            )?,
            BoundedCommandErrorKind::Spawn { source } => {
                write!(formatter, "could not spawn direct child: {source}")?;
            }
            BoundedCommandErrorKind::StatusCheck { source, kill, reap } => {
                write!(formatter, "could not read direct child status: {source}; ")?;
                write_termination_results(formatter, kill, reap)?;
            }
            BoundedCommandErrorKind::Timeout {
                timeout,
                kill,
                reap,
            } => {
                write!(formatter, "timed out after {timeout:?}; ")?;
                write_termination_results(formatter, kill, reap)?;
            }
        }

        write!(
            formatter,
            "; stdout report: {}; stderr report: {}",
            self.stdout_path.display(),
            self.stderr_path.display()
        )?;
        if self.retained_working_directory {
            write!(
                formatter,
                "; working directory retained at {}",
                self.working_directory.display()
            )?;
        }
        Ok(())
    }
}

fn write_termination_results(
    formatter: &mut std::fmt::Formatter<'_>,
    kill: &io::Result<()>,
    reap: &io::Result<ExitStatus>,
) -> std::fmt::Result {
    match kill {
        Ok(()) => write!(formatter, "direct child kill succeeded; ")?,
        Err(error) => write!(formatter, "direct child kill failed: {error}; ")?,
    }
    match reap {
        Ok(status) => write!(
            formatter,
            "direct child reap succeeded with status {status}"
        ),
        Err(error) => write!(formatter, "direct child reap failed: {error}"),
    }
}

fn read_report(path: &Path) -> String {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) => return format!("<could not read {}: {error}>", path.display()),
    };
    let mut bytes = Vec::with_capacity(MAX_DIAGNOSTIC_BYTES + 1);
    if let Err(error) = Read::by_ref(&mut file)
        .take((MAX_DIAGNOSTIC_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
    {
        return format!("<could not read {}: {error}>", path.display());
    }
    let truncated = bytes.len() > MAX_DIAGNOSTIC_BYTES;
    if truncated {
        bytes.truncate(MAX_DIAGNOSTIC_BYTES);
    }
    let mut text = String::from_utf8_lossy(&bytes).into_owned();
    if truncated {
        text.push_str(&format!(
            "\n[diagnostic truncated to {MAX_DIAGNOSTIC_BYTES} bytes; full log: {}]",
            path.display()
        ));
    }
    text
}

fn make_error(
    label: String,
    kind: BoundedCommandErrorKind,
    stdout_path: &Path,
    stderr_path: &Path,
    working_directory: &OwnedTempDir,
    retained_working_directory: bool,
) -> BoundedCommandError {
    BoundedCommandError {
        label,
        kind,
        stdout: read_report(stdout_path),
        stderr: read_report(stderr_path),
        stdout_path: stdout_path.to_owned(),
        stderr_path: stderr_path.to_owned(),
        working_directory: working_directory.path().to_owned(),
        retained_working_directory,
    }
}

/// Kill and reap only the direct child. A failed kill is followed by a non-blocking reap check;
/// a direct child that does not exit after a successful kill is also reported as uncertain.
fn terminate_direct_child(mut child: Child) -> (io::Result<()>, io::Result<ExitStatus>) {
    let kill = child.kill();
    let reap = match &kill {
        Err(_) => match child.try_wait() {
            Ok(Some(status)) => Ok(status),
            Ok(None) => Err(io::Error::other(
                "direct child remained running after kill failed",
            )),
            Err(error) => Err(error),
        },
        Ok(()) => {
            let deadline = Instant::now() + DIRECT_CHILD_REAP_TIMEOUT;
            loop {
                match child.try_wait() {
                    Ok(Some(status)) => break Ok(status),
                    Ok(None) if Instant::now() < deadline => {
                        thread::sleep(CHILD_POLL_INTERVAL);
                    }
                    Ok(None) => {
                        break Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            format!(
                                "direct child did not reap within {DIRECT_CHILD_REAP_TIMEOUT:?}"
                            ),
                        ))
                    }
                    Err(error) => break Err(error),
                }
            }
        }
    };
    (kill, reap)
}

/// Run one command with file-backed stdout/stderr and a finite deadline.
///
/// Only the direct child is owned and terminated. This is not a process-tree isolation primitive.
fn run_bounded_command(
    label: String,
    command: &mut Command,
    timeout: Duration,
    working_directory: &mut OwnedTempDir,
    stdout_path: &Path,
    stderr_path: &Path,
) -> Result<BoundedCommandOutput, Box<BoundedCommandError>> {
    let stdout = File::create(stdout_path).map_err(|source| {
        Box::new(make_error(
            label.clone(),
            BoundedCommandErrorKind::OutputFile {
                stream: "stdout",
                path: stdout_path.to_owned(),
                source,
            },
            stdout_path,
            stderr_path,
            working_directory,
            false,
        ))
    })?;
    let stderr = File::create(stderr_path).map_err(|source| {
        Box::new(make_error(
            label.clone(),
            BoundedCommandErrorKind::OutputFile {
                stream: "stderr",
                path: stderr_path.to_owned(),
                source,
            },
            stdout_path,
            stderr_path,
            working_directory,
            false,
        ))
    })?;
    let mut child = command
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()
        .map_err(|source| {
            Box::new(make_error(
                label.clone(),
                BoundedCommandErrorKind::Spawn { source },
                stdout_path,
                stderr_path,
                working_directory,
                false,
            ))
        })?;

    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return Ok(BoundedCommandOutput {
                    status,
                    stdout: read_report(stdout_path),
                    stderr: read_report(stderr_path),
                    stdout_path: stdout_path.to_owned(),
                    stderr_path: stderr_path.to_owned(),
                });
            }
            Ok(None) if Instant::now() < deadline => {
                thread::sleep(CHILD_POLL_INTERVAL);
            }
            Ok(None) => {
                working_directory.retain();
                let (kill, reap) = terminate_direct_child(child);
                return Err(Box::new(make_error(
                    label,
                    BoundedCommandErrorKind::Timeout {
                        timeout,
                        kill,
                        reap,
                    },
                    stdout_path,
                    stderr_path,
                    working_directory,
                    true,
                )));
            }
            Err(source) => {
                working_directory.retain();
                let (kill, reap) = terminate_direct_child(child);
                return Err(Box::new(make_error(
                    label,
                    BoundedCommandErrorKind::StatusCheck { source, kill, reap },
                    stdout_path,
                    stderr_path,
                    working_directory,
                    true,
                )));
            }
        }
    }
}

fn write_manifest(crate_dir: &Path, manifest: ConsumerManifest<'_>) -> io::Result<()> {
    match manifest {
        ConsumerManifest::Inline(contents) => fs::write(crate_dir.join("Cargo.toml"), contents),
        ConsumerManifest::Fixture(fixture) => {
            let fixture_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures")
                .join(fixture);
            for (template, output) in [
                ("Cargo.toml.fixture", "Cargo.toml"),
                ("Cargo.lock.fixture", "Cargo.lock"),
            ] {
                fs::copy(fixture_dir.join(template), crate_dir.join(output))?;
            }
            Ok(())
        }
    }
}

/// Write a generated consumer crate and run its `cargo check` with a private target directory.
pub fn assert_consumer_compiles(
    source: &str,
    tag: &str,
    manifest: ConsumerManifest<'_>,
    cargo_args: &[&str],
) {
    let mut working_directory = OwnedTempDir::allocate(tag).unwrap_or_else(|error| {
        panic!("failed to allocate consumer directory for `{tag}`: {error}")
    });
    let crate_dir = working_directory.path().to_owned();
    fs::create_dir(crate_dir.join("src")).unwrap_or_else(|error| {
        panic!(
            "failed to create consumer source directory {}: {error}",
            crate_dir.display()
        )
    });
    write_manifest(&crate_dir, manifest).unwrap_or_else(|error| {
        panic!(
            "failed to write consumer manifest in {}: {error}",
            crate_dir.display()
        )
    });
    fs::write(crate_dir.join("src/lib.rs"), source).unwrap_or_else(|error| {
        panic!(
            "failed to write generated consumer source in {}: {error}",
            crate_dir.display()
        )
    });

    let mut command = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned()));
    command
        .arg("check")
        .args(cargo_args)
        .current_dir(&crate_dir)
        .env("CARGO_TARGET_DIR", crate_dir.join("target"));

    match working_directory.run(&mut command, CARGO_COMMAND_TIMEOUT) {
        Ok(output) => assert!(
            output.status.success(),
            "generated consumer failed to compile\nstatus: {}\nstdout log: {}\nstderr log: {}\nstdout:\n{}\nstderr:\n{}\n\n--- generated lib.rs ---\n{source}",
            output.status,
            output.stdout_path.display(),
            output.stderr_path.display(),
            output.stdout,
            output.stderr,
        ),
        Err(error) => panic!(
            "bounded generated consumer check failed: {error}\nworking directory: {}\nstdout:\n{}\nstderr:\n{}\n\n--- generated lib.rs ---\n{source}",
            error.working_directory().display(),
            error.stdout(),
            error.stderr(),
        ),
    }
}

fn run_timeout_child() {
    let role = std::env::var_os(TIMEOUT_CHILD_ROLE_ENV)
        .expect("timeout child role is required for the exact child invocation");
    assert_eq!(
        role,
        std::ffi::OsString::from(TIMEOUT_CHILD_ROLE),
        "timeout child role must be exact"
    );
    let marker_path = PathBuf::from(
        std::env::var_os(TIMEOUT_CHILD_MARKER_ENV)
            .expect("timeout child start-marker path is required"),
    );
    let mut marker = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&marker_path)
        .unwrap_or_else(|error| {
            panic!(
                "failed to create exact timeout child start marker {}: {error}",
                marker_path.display()
            )
        });
    marker
        .write_all(TIMEOUT_CHILD_START_MARKER)
        .expect("timeout child start marker write failed");
    marker
        .sync_all()
        .expect("timeout child start marker sync failed");
    drop(marker);
    eprintln!("bounded consumer timeout child started");
    thread::sleep(TIMEOUT_CHILD_SLEEP);
    panic!("bounded consumer timeout child escaped its deadline");
}

/// Prove the bounded runner selects an exact test-binary child, observes its start marker, and
/// reports a timeout while preserving direct-child termination and retained diagnostics.
pub fn assert_timeout_regression(test_name: &str) {
    if std::env::var_os(TIMEOUT_CHILD_ROLE_ENV).is_some() {
        run_timeout_child();
        unreachable!("timeout child must sleep until the parent terminates it");
    }

    let mut working_directory = OwnedTempDir::allocate("timeout")
        .expect("timeout regression workdir must be allocated exclusively");
    let crate_dir = working_directory.path().to_owned();
    let marker_path = crate_dir.join("timeout-child.started");
    assert!(
        !marker_path.exists(),
        "timeout child marker must start absent"
    );

    let current_exe = std::env::current_exe().expect("test executable must resolve");
    let mut command = Command::new(current_exe);
    command
        .args(["--exact", test_name, "--nocapture"])
        .env(TIMEOUT_CHILD_ROLE_ENV, TIMEOUT_CHILD_ROLE)
        .env(TIMEOUT_CHILD_MARKER_ENV, &marker_path);

    let error = match working_directory.run(&mut command, TIMEOUT_CHILD_DEADLINE) {
        Ok(output) => panic!(
            "exact timeout child exited before its deadline with status {}\nstdout log: {}\nstderr log: {}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            output.stdout_path.display(),
            output.stderr_path.display(),
            output.stdout,
            output.stderr,
        ),
        Err(error) => error,
    };

    assert!(
        error.is_timeout(),
        "exact child must hit the timeout: {error}"
    );
    assert!(
        error.termination_succeeded(),
        "direct-child kill/reap must be preserved and successful: {error}"
    );
    assert!(
        error.retained_working_directory(),
        "timeout must retain the workdir and file-backed reports: {error}"
    );
    assert_eq!(
        error.working_directory(),
        working_directory.path(),
        "timeout error must retain the owned workdir path"
    );
    assert_eq!(
        fs::read(&marker_path).expect("exact timeout child start marker must exist"),
        TIMEOUT_CHILD_START_MARKER,
        "the marker proves the exact child branch executed"
    );
    assert!(working_directory.path().is_dir());
    assert!(error.stdout_path().is_file());
    assert!(error.stderr_path().is_file());

    // This exact child has no descendants. Cleanup is safe after the direct child was reaped;
    // the runner itself never claims process-tree isolation.
    working_directory
        .cleanup_now()
        .expect("owned timeout regression directory must clean up after direct-child reap");
}
