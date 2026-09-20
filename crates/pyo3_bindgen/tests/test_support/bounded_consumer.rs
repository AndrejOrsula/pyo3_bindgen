use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const CARGO_CHECK_TIMEOUT: Duration = Duration::from_secs(900);
const DIRECT_CHILD_REAP_TIMEOUT: Duration = Duration::from_secs(30);
const CHILD_POLL_INTERVAL: Duration = Duration::from_millis(20);
const MAX_DIAGNOSTIC_BYTES: usize = 128 * 1024;
const REQUIRE_WORKSPACE_ENV: &str = "PYO3_BINDGEN_REQUIRE_WORKSPACE";
const TIMEOUT_CHILD_ROLE_ENV: &str = "PYO3_BINDGEN_BOUNDED_CONSUMER_TIMEOUT_CHILD";
const TIMEOUT_CHILD_ROLE: &str = "pyo3-bindgen-facade-timeout-child-v1";
const TIMEOUT_CHILD_MARKER_ENV: &str = "PYO3_BINDGEN_BOUNDED_CONSUMER_START_MARKER";
const TIMEOUT_CHILD_START_MARKER: &[u8] = b"pyo3-bindgen-facade-timeout-child-started-v1\n";
const TIMEOUT_CHILD_SLEEP: Duration = Duration::from_secs(60);
const TIMEOUT_CHILD_DEADLINE: Duration = Duration::from_secs(5);

static NEXT_TEMP_DIR: AtomicU64 = AtomicU64::new(0);

/// One test task's exclusive workspace. It retains its directory after an uncertain child
/// termination because this runner owns only the direct child and makes no process-tree claim.
pub struct ConsumerWorkspace {
    path: PathBuf,
    cleanup_on_drop: bool,
    retained_after_uncertain_termination: bool,
}

impl ConsumerWorkspace {
    pub fn new(tag: &str) -> Self {
        let workspace = Self {
            path: allocate_directory(tag),
            cleanup_on_drop: true,
            retained_after_uncertain_termination: false,
        };
        fs::create_dir(workspace.path.join("src")).unwrap_or_else(|error| {
            panic!(
                "failed to create facade consumer source directory {}: {error}",
                workspace.path.join("src").display()
            )
        });
        workspace
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn retain(&mut self) {
        self.cleanup_on_drop = false;
        self.retained_after_uncertain_termination = true;
    }

    pub fn cleanup_now(&mut self) -> io::Result<()> {
        match fs::remove_dir_all(&self.path) {
            Ok(()) => {
                self.cleanup_on_drop = false;
                self.retained_after_uncertain_termination = false;
                Ok(())
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                self.cleanup_on_drop = false;
                self.retained_after_uncertain_termination = false;
                Ok(())
            }
            Err(error) => Err(error),
        }
    }
}

impl Drop for ConsumerWorkspace {
    fn drop(&mut self) {
        if self.cleanup_on_drop {
            if let Err(error) = fs::remove_dir_all(&self.path) {
                if error.kind() != io::ErrorKind::NotFound {
                    eprintln!(
                        "failed to remove owned facade consumer directory {}: {error}",
                        self.path.display()
                    );
                }
            }
        } else if self.retained_after_uncertain_termination {
            eprintln!(
                "retaining owned facade consumer directory after uncertain child termination: {}",
                self.path.display()
            );
        }
    }
}

fn allocate_directory(tag: &str) -> PathBuf {
    let tag = sanitize_component(tag);
    let process_id = std::process::id();
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();

    for attempt in 0..128_u32 {
        let sequence = NEXT_TEMP_DIR.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "pyo3_bindgen_facade_consumer_{tag}_{process_id}_{timestamp}_{sequence}_{attempt}"
        ));
        match fs::create_dir(&path) {
            Ok(()) => return path,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => panic!(
                "failed to allocate exclusive facade consumer directory {}: {error}",
                path.display()
            ),
        }
    }

    panic!("could not allocate an exclusive facade consumer directory for `{tag}`");
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

/// Find the source workspace, visibly skipping only when this test is running from a packaged
/// crate archive. The source-tree shape and the opt-in flag make a missing workspace fail rather
/// than silently turning a real repository run green.
pub fn workspace_root(test_name: &str) -> Option<PathBuf> {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let source_tree_shape = manifest_dir
        .file_name()
        .is_some_and(|name| name == "pyo3_bindgen")
        && manifest_dir
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|name| name == "crates");
    if source_tree_shape {
        let root = manifest_dir
            .parent()
            .and_then(Path::parent)
            .expect("source-tree facade manifest must have a workspace parent");
        let manifest = fs::read_to_string(root.join("Cargo.toml")).unwrap_or_else(|error| {
            panic!(
                "{test_name} requires the source workspace manifest {}: {error}",
                root.join("Cargo.toml").display()
            )
        });
        assert!(
            manifest.contains("[workspace]"),
            "{test_name} requires a [workspace] manifest at {}",
            root.join("Cargo.toml").display()
        );
        return Some(root.to_path_buf());
    }

    let root = manifest_dir.ancestors().find(|directory| {
        fs::read_to_string(directory.join("Cargo.toml"))
            .is_ok_and(|manifest| manifest.contains("[workspace]"))
    });
    if let Some(root) = root {
        return Some(root.to_path_buf());
    }

    if std::env::var_os(REQUIRE_WORKSPACE_ENV).is_some() {
        panic!(
            "{test_name} requires the source workspace, but no workspace manifest was found; packaged-archive runs may omit the workspace only when {REQUIRE_WORKSPACE_ENV} is unset"
        );
    }

    eprintln!(
        "SKIP {test_name}: source workspace unavailable (expected for an installed package archive)"
    );
    None
}

#[derive(Debug)]
pub struct BoundedCommandOutput {
    pub status: ExitStatus,
    pub stdout: String,
    pub stderr: String,
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
    pub fn stdout(&self) -> &str {
        &self.stdout
    }

    pub fn stderr(&self) -> &str {
        &self.stderr
    }

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
    working_directory: &ConsumerWorkspace,
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
/// a direct child that does not exit after a successful kill is reported as uncertain.
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

/// Run `cargo check --offline --quiet` with file-backed stdout/stderr and a finite deadline.
///
/// The runner owns and reaps only the direct child. It is not process-tree isolation; if status
/// is uncertain, the task directory remains available for diagnosis.
pub fn cargo_check(
    working_directory: &mut ConsumerWorkspace,
) -> Result<BoundedCommandOutput, Box<BoundedCommandError>> {
    let crate_directory = working_directory.path().to_owned();
    let stdout_path = crate_directory.join("cargo-check.stdout");
    let stderr_path = crate_directory.join("cargo-check.stderr");
    let mut command = Command::new(env!("CARGO"));
    command
        .args(["check", "--offline", "--quiet"])
        .current_dir(&crate_directory)
        .env("CARGO_TARGET_DIR", crate_directory.join("target"));

    run_bounded_command(
        "facade consumer cargo check",
        command,
        CARGO_CHECK_TIMEOUT,
        working_directory,
        &stdout_path,
        &stderr_path,
    )
}

fn run_bounded_command(
    label: &str,
    mut command: Command,
    timeout: Duration,
    working_directory: &mut ConsumerWorkspace,
    stdout_path: &Path,
    stderr_path: &Path,
) -> Result<BoundedCommandOutput, Box<BoundedCommandError>> {
    let stdout = File::create(stdout_path).map_err(|source| {
        Box::new(make_error(
            label.to_owned(),
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
            label.to_owned(),
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
                label.to_owned(),
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
                });
            }
            Ok(None) if Instant::now() < deadline => thread::sleep(CHILD_POLL_INTERVAL),
            Ok(None) => {
                working_directory.retain();
                let (kill, reap) = terminate_direct_child(child);
                return Err(Box::new(make_error(
                    label.to_owned(),
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
                    label.to_owned(),
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
    eprintln!("facade timeout child started");
    thread::sleep(TIMEOUT_CHILD_SLEEP);
    panic!("facade timeout child escaped its deadline");
}

/// Prove exact-child selection, child start, timeout, direct-child kill/reap, and retained logs.
pub fn assert_timeout_regression(test_name: &str) {
    if std::env::var_os(TIMEOUT_CHILD_ROLE_ENV).is_some() {
        run_timeout_child();
        unreachable!("timeout child must sleep until the parent terminates it");
    }

    let mut working_directory = ConsumerWorkspace::new("timeout");
    let stdout_path = working_directory.path().join("timeout-child.stdout");
    let stderr_path = working_directory.path().join("timeout-child.stderr");
    let marker_path = working_directory.path().join("timeout-child.started");
    assert!(
        !marker_path.exists(),
        "timeout child marker must start absent"
    );

    let current_exe = std::env::current_exe().expect("test executable must resolve");
    let mut command = Command::new(current_exe);
    command
        .args(["--exact", test_name, "--nocapture"])
        .current_dir(working_directory.path())
        .env(TIMEOUT_CHILD_ROLE_ENV, TIMEOUT_CHILD_ROLE)
        .env(TIMEOUT_CHILD_MARKER_ENV, &marker_path);

    let error = match run_bounded_command(
        "exact facade timeout child",
        command,
        TIMEOUT_CHILD_DEADLINE,
        &mut working_directory,
        &stdout_path,
        &stderr_path,
    ) {
        Ok(output) => panic!(
            "exact timeout child exited before its deadline with status {}\nstdout:\n{}\nstderr:\n{}",
            output.status, output.stdout, output.stderr
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
        "timeout must retain the task directory and file-backed reports: {error}"
    );
    assert_eq!(
        fs::read(&marker_path).expect("exact timeout child start marker must exist"),
        TIMEOUT_CHILD_START_MARKER,
        "the marker proves the exact child branch executed"
    );
    assert!(working_directory.path().is_dir());
    assert!(stdout_path.is_file());
    assert!(stderr_path.is_file());

    // This exact child has no descendants. Cleanup is safe only after its direct reap succeeds;
    // the runner itself never claims process-tree isolation.
    working_directory
        .cleanup_now()
        .expect("owned timeout regression directory must clean up after direct-child reap");
}
