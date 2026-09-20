use std::fs;
use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find(|directory| {
            fs::read_to_string(directory.join("Cargo.toml"))
                .is_ok_and(|manifest| manifest.contains("[workspace]"))
        })
        .expect("CLI crate should be nested below the workspace root")
        .to_path_buf()
}

/// A source-responsibility gate, not a restriction on public direct-writer capabilities.
#[test]
fn artifact_values_and_production_delivery_keep_separate_responsibilities() {
    let root = workspace_root();
    let values = fs::read_to_string(root.join("crates/pyo3_bindgen_engine/src/artifact/value.rs"))
        .expect("the private artifact value module must exist");
    let compact_values: String = values.chars().filter(|c| !c.is_whitespace()).collect();
    for forbidden in [
        "std::fs",
        "std::path",
        "std::io",
        "write_atomically",
        "write_detailed",
        "PublicationSession",
        "PublicationPlan",
    ] {
        assert!(
            !compact_values.contains(forbidden),
            "pure artifact methods gained publication mechanics: {forbidden}"
        );
    }

    for relative in [
        "crates/pyo3_bindgen_engine/src/codegen.rs",
        "crates/pyo3_bindgen_cli/src/lib.rs",
    ] {
        let source = fs::read_to_string(root.join(relative)).unwrap();
        let (production, _) = source
            .split_once("#[cfg(test)]\nmod tests")
            .expect("the checked production prefix must end at the explicit test module");
        let compact: String = production.chars().filter(|c| !c.is_whitespace()).collect();
        for forbidden in [
            "write_atomically",
            "write_detailed",
            "fs::write(",
            "artifact.write(",
        ] {
            assert!(
                !compact.contains(forbidden),
                "{relative} regained direct artifact publication: {forbidden}"
            );
        }
        assert!(
            compact.contains(".publish_output("),
            "{relative} must delegate file output through the publication session"
        );
    }
}

#[test]
fn production_cli_leaves_delivery_state_inside_the_facade_session() {
    let root = workspace_root();
    let source = fs::read_to_string(root.join("crates/pyo3_bindgen_cli/src/lib.rs")).unwrap();
    // Compatibility root reexports remain public. Check actual command orchestration instead.
    let operations = source.split_once("pub fn run(").unwrap().1;
    let production = operations.split_once("#[cfg(test)]\nmod tests").unwrap().0;
    for forbidden in [
        "PublicationPlan",
        "PublicationPolicy",
        "RunState",
        "DeliveryState",
        "DeliveryReceipt",
        "ReportWriteAttempt",
        ".delivery",
        "set_operation_id",
        "output_delivery_receipt",
    ] {
        assert!(
            !production.contains(forbidden),
            "CLI regained delivery ownership: {forbidden}"
        );
    }
    for required in [
        "Request::new",
        "publication.finish(",
        "publication.write_stdout(",
    ] {
        assert!(
            production.contains(required),
            "CLI lost facade delegation: {required}"
        );
    }
}

fn pre_commit_ci_version(workflow: &str) -> &str {
    workflow
        .lines()
        .map(str::trim)
        .find_map(|line| {
            line.strip_prefix("run: python -m pip install \"pre-commit==")
                .and_then(|version| version.strip_suffix('\"'))
        })
        .expect("pre-commit workflow should install a pinned version")
}

fn workflow_job<'a>(workflow: &'a str, name: &str) -> &'a str {
    let marker = format!("  {name}:\n");
    let start = workflow
        .find(&marker)
        .unwrap_or_else(|| panic!("Rust workflow should define the {name} job"));
    let body = &workflow[start + marker.len()..];
    body.match_indices("\n  ")
        .find(|(index, _)| body.as_bytes().get(index + 3) != Some(&b' '))
        .map_or(body, |(end, _)| &body[..end])
}

const DUAL_LINUX_RUNNER: &str = "${{ github.repository == 'SpaceRL/pyo3_bindgen_private' && github.event.repository.private == true && (github.event_name != 'pull_request' || github.event.pull_request.head.repo.full_name == github.repository) && fromJSON('[\"self-hosted\", \"Linux\", \"X64\"]') || 'ubuntu-24.04' }}";
const PRIVATE_FORK_ADMISSION: &str = "${{ github.repository != 'SpaceRL/pyo3_bindgen_private' || github.event.repository.private != true || github.event_name != 'pull_request' || github.event.pull_request.head.repo.full_name == github.repository }}";
const DENY_ADMISSION: &str = "${{ always() && (github.repository != 'SpaceRL/pyo3_bindgen_private' || github.event.repository.private != true || github.event_name != 'pull_request' || github.event.pull_request.head.repo.full_name == github.repository) }}";

fn expected_linux_runner(
    repository: &str,
    repository_private: bool,
    event_name: &str,
    pull_request_head_repository: Option<&str>,
) -> &'static str {
    if repository == "SpaceRL/pyo3_bindgen_private"
        && repository_private
        && (event_name != "pull_request" || pull_request_head_repository == Some(repository))
    {
        "self-hosted Linux X64"
    } else {
        "ubuntu-24.04"
    }
}

#[test]
fn linux_ci_runner_selection_is_private_exact_and_fork_safe() {
    assert_eq!(
        expected_linux_runner("SpaceRL/pyo3_bindgen_private", true, "push", None,),
        "self-hosted Linux X64"
    );
    assert_eq!(
        expected_linux_runner(
            "SpaceRL/pyo3_bindgen_private",
            true,
            "pull_request",
            Some("SpaceRL/pyo3_bindgen_private"),
        ),
        "self-hosted Linux X64"
    );
    for (repository, repository_private, head_repository) in [
        ("SpaceRL/pyo3_bindgen_private", true, None),
        (
            "SpaceRL/pyo3_bindgen_private",
            true,
            Some("contributor/pyo3_bindgen_private"),
        ),
        (
            "AndrejOrsula/pyo3_bindgen",
            false,
            Some("contributor/pyo3_bindgen"),
        ),
        (
            "SpaceRL/pyo3_bindgen_private",
            false,
            Some("SpaceRL/pyo3_bindgen_private"),
        ),
        (
            "SpaceRL/pyo3_bindgen_private_evil",
            true,
            Some("SpaceRL/pyo3_bindgen_private_evil"),
        ),
        ("AndrejOrsula/pyo3_bindgen", false, None),
    ] {
        assert_eq!(
            expected_linux_runner(
                repository,
                repository_private,
                "pull_request",
                head_repository,
            ),
            "ubuntu-24.04",
            "non-owned, public, or fork pull requests must use hosted Linux"
        );
    }
}

#[test]
fn all_linux_ci_jobs_use_the_guarded_dual_runner() {
    let root = workspace_root();
    let rust_workflow = fs::read_to_string(root.join(".github/workflows/rust.yml"))
        .expect("Rust workflow should exist");
    let pre_commit_workflow = fs::read_to_string(root.join(".github/workflows/pre-commit.yml"))
        .expect("pre-commit workflow should exist");
    let cargo_deny_workflow = fs::read_to_string(root.join(".github/workflows/cargo-deny.yml"))
        .expect("cargo-deny workflow should exist");
    let rust_jobs = [
        "rustfmt",
        "cargo",
        "platform-runtime",
        "compile-sweep",
        "native-platforms",
        "windows-msrv-cross-check",
        "windows-msvc-cross-check",
        "apple-cross-check",
        "python-compatibility",
        "abi3",
        "pypy-external-ir",
        "graalpy-cross-check",
        "deny",
    ];
    for job_name in rust_jobs {
        let job = workflow_job(&rust_workflow, job_name);
        if job_name == "deny" {
            assert!(
                job.contains("runs-on: "),
                "deny aggregate should execute its success assertion"
            );
        }
        assert!(
            job.contains(&format!("runs-on: {DUAL_LINUX_RUNNER}")),
            "Rust {job_name} must use the fail-closed dual Linux runner"
        );
    }
    let dependency_policy = workflow_job(&rust_workflow, "dependency-policy");
    assert!(
        dependency_policy.contains("uses: ./.github/workflows/cargo-deny.yml")
            && dependency_policy.contains(&format!("if: {PRIVATE_FORK_ADMISSION}")),
        "dependency-policy must remain the reusable cargo-deny caller"
    );
    assert!(
        !dependency_policy.contains("runs-on: "),
        "reusable dependency-policy caller must not claim a runner"
    );

    let pre_commit_job = workflow_job(&pre_commit_workflow, "pre-commit");
    assert!(
        pre_commit_job.contains(&format!("runs-on: {DUAL_LINUX_RUNNER}")),
        "pre-commit must use the fail-closed dual Linux runner"
    );
    let cargo_deny_job = workflow_job(&cargo_deny_workflow, "cargo-deny");
    assert!(
        cargo_deny_job.contains(&format!("if: {PRIVATE_FORK_ADMISSION}"))
            && cargo_deny_job.contains(&format!("runs-on: {DUAL_LINUX_RUNNER}")),
        "reusable cargo-deny must use the fail-closed dual Linux runner"
    );
    assert!(
        workflow_job(&rust_workflow, "rustfmt").contains(&format!("if: {PRIVATE_FORK_ADMISSION}"))
            && pre_commit_job.contains(&format!("if: {PRIVATE_FORK_ADMISSION}")),
        "private fork PRs must be excluded before persistent job scheduling"
    );
    let aggregate = workflow_job(&rust_workflow, "deny");
    assert!(
        aggregate.contains(&format!("if: {DENY_ADMISSION}"))
            && aggregate.contains("timeout-minutes: 5"),
        "deny aggregate must retain the private-fork admission guard and bounded timeout"
    );
    for job in [
        workflow_job(&rust_workflow, "cargo"),
        dependency_policy,
        aggregate,
        cargo_deny_job,
    ] {
        assert!(
            !job.contains("continue-on-error"),
            "CI matrix, reusable caller, and deny aggregate must not mask failures"
        );
    }
    for workflow in [&rust_workflow, &pre_commit_workflow, &cargo_deny_workflow] {
        assert!(
            !workflow.contains("runs-on: [self-hosted, Linux, X64]"),
            "persistent runner labels must not bypass the repository and fork guards"
        );
        assert!(
            workflow.contains("github.event.repository.private == true")
                && workflow.contains("github.event.pull_request.head.repo.full_name == github.repository")
                && workflow.contains("'ubuntu-24.04'"),
            "Linux runner selection must require private repository identity and same-repository PRs"
        );
    }
}

#[cfg(unix)]
#[test]
fn deny_aggregate_preserves_required_context_and_rejects_non_success_results() {
    use std::process::Command;

    let root = workspace_root();
    let workflow = fs::read_to_string(root.join(".github/workflows/rust.yml"))
        .expect("Rust workflow should exist");
    let dependency_policy = workflow_job(&workflow, "dependency-policy");
    let aggregate = workflow_job(&workflow, "deny");

    assert!(
        dependency_policy.contains("uses: ./.github/workflows/cargo-deny.yml")
            && aggregate.contains("name: deny")
            && aggregate.contains("needs: dependency-policy")
            && aggregate.contains(&format!("if: {DENY_ADMISSION}")),
        "deny must remain a public aggregate over the reusable dependency-policy job"
    );
    assert!(
        aggregate.contains("DEPENDENCY_POLICY_RESULT: ${{ needs['dependency-policy'].result }}"),
        "deny aggregate must pass the reusable result through an environment variable"
    );
    let run_command = aggregate
        .lines()
        .find_map(|line| line.trim().strip_prefix("run:").map(str::trim))
        .expect("deny aggregate should contain a run assertion");
    assert_eq!(run_command, r#"test "$DEPENDENCY_POLICY_RESULT" = success"#);
    for result in ["success", "failure", "skipped", "cancelled", ""] {
        let output = Command::new("sh")
            .args(["-c", run_command])
            .env("DEPENDENCY_POLICY_RESULT", result)
            .output()
            .expect("deny aggregate assertion shell should start");
        assert_eq!(
            output.status.success(),
            result == "success",
            "deny aggregate must reject dependency-policy result {result:?}"
        );
    }
}

#[test]
fn local_pre_commit_version_matches_ci_and_hook_path_is_portable() {
    let root = workspace_root();
    let installer = fs::read_to_string(root.join(".git_hooks/install.bash"))
        .expect("hook installer should exist");
    let config = fs::read_to_string(root.join(".pre-commit-config.yaml"))
        .expect("pre-commit configuration should exist");
    let workflow = fs::read_to_string(root.join(".github/workflows/pre-commit.yml"))
        .expect("pre-commit workflow should exist");
    let version = pre_commit_ci_version(&workflow);

    assert_eq!(version, "4.6.0");
    assert!(
        config.contains(&format!("minimum_pre_commit_version: \"{version}\"")),
        "local pre-commit configuration should require the CI version"
    );
    assert!(
        installer.contains(&format!("PRE_COMMIT_VERSION={version}")),
        "hook installer should select the CI pre-commit version"
    );
    assert!(
        installer.contains("-m pip install --upgrade \"pre-commit==${PRE_COMMIT_VERSION}\""),
        "hook installer should enforce the pinned version without a user-site assumption"
    );
    assert!(
        installer.contains("\"${PRE_COMMIT_PYTHON}\" -m pre_commit install --install-hooks --config"),
        "hook installer should invoke the installed pre-commit module through its selected environment"
    );
    assert!(
        !installer.contains("command -v pre-commit") && !installer.contains("\npre-commit install"),
        "hook installer must not accept or invoke an unrelated pre-commit executable"
    );
    assert!(
        !installer.contains("pip install --user"),
        "hook installer must work in virtualenv and externally managed Python environments"
    );
    assert!(
        !installer.contains("readlink -f"),
        "hook installer must not depend on GNU readlink -f"
    );
    assert!(
        installer.contains("dirname \"${BASH_SOURCE[0]}\"") && installer.contains("pwd -P"),
        "hook installer should resolve its directory with portable shell primitives"
    );
}

#[test]
fn ci_uses_the_fail_closed_compile_sweep_wrapper() {
    let root = workspace_root();
    let workflow = fs::read_to_string(root.join(".github/workflows/rust.yml"))
        .expect("Rust workflow should exist");
    assert!(
        workflow.contains("scripts/compile-sweep.sh --tier all --require-heavy"),
        "CI must use the same explicit heavy-target contract as local runs"
    );
    assert!(
        !workflow.contains("--test compile_sweep --locked --verbose"),
        "CI must not bypass the wrapper's zero-match guard"
    );
}

#[test]
fn pypy_ci_uses_only_the_supported_external_ir_lane() {
    let root = workspace_root();
    let workflow = fs::read_to_string(root.join(".github/workflows/rust.yml"))
        .expect("Rust workflow should exist");
    let platform_runtime = workflow_job(&workflow, "platform-runtime");
    let pypy_external_ir = workflow_job(&workflow, "pypy-external-ir");

    assert!(
        platform_runtime.contains(&format!("runs-on: {DUAL_LINUX_RUNNER}"))
            && platform_runtime.contains("label: linux-free-threaded")
            && !platform_runtime.contains("label: windows-cpython")
            && !platform_runtime.contains("label: macos-cpython"),
        "platform/runtime should describe the available self-hosted Linux runtime lane"
    );
    for job_name in [
        "windows-msrv-cross-check",
        "windows-msvc-cross-check",
        "apple-cross-check",
    ] {
        let cross_check = workflow_job(&workflow, job_name);
        assert!(
            cross_check.contains(&format!("runs-on: {DUAL_LINUX_RUNNER}"))
                && cross_check.contains("PYO3_CROSS: \"1\"")
                && cross_check.contains("cargo check ")
                && !cross_check.contains("cargo test ")
                && !cross_check.contains("continue-on-error"),
            "{job_name} should retain required compile-only target coverage, not claim native execution"
        );
    }
    assert!(
        !platform_runtime.contains("pypy"),
        "platform/runtime must not claim unsupported full-workspace PyPy coverage"
    );
    assert!(
        pypy_external_ir.contains("python-version: \"pypy3.11\"")
            && pypy_external_ir.contains("crates/pyo3_bindgen_cli/tests/fixtures/minimal_ir.json")
            && pypy_external_ir.contains("cargo run --locked -p pyo3_bindgen_cli --"),
        "PyPy coverage should remain the scoped Python-free external-IR lane"
    );
    assert!(
        !pypy_external_ir.contains("cargo test --workspace"),
        "the PyPy external-IR lane must not run embedded/full workspace tests"
    );
}

#[cfg(unix)]
mod unix {
    use super::workspace_root;
    use std::env;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    struct TempDir(PathBuf);

    impl TempDir {
        fn create() -> Self {
            let unique = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock should be after the Unix epoch")
                .as_nanos();
            let path = env::temp_dir().join(format!(
                "pyo3_bindgen_tooling_{}_{}",
                std::process::id(),
                unique
            ));
            fs::create_dir_all(&path).expect("temporary tooling directory should be created");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn executable(path: &Path, body: &str) {
        fs::write(path, body).expect("test executable should be written");
        let mut permissions = fs::metadata(path)
            .expect("test executable metadata should be readable")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).expect("test executable should be executable");
    }

    #[test]
    fn installer_works_without_readlink_from_an_unrelated_working_directory() {
        let temp = TempDir::create();
        let bin = temp.path().join("bin");
        fs::create_dir(&bin).expect("test bin directory should be created");
        let log = temp.path().join("pre-commit.log");

        executable(&bin.join("readlink"), "#!/bin/sh\nexit 97\n");
        executable(
            &bin.join("python"),
            "#!/bin/sh\nif [ \"$1\" = \"-c\" ]; then\n  exit 0\nfi\nif [ \"$1\" = \"-m\" ] && [ \"$2\" = \"pre_commit\" ]; then\n  printf '%s\\n' \"$PWD\" > \"$TOOL_LOG\"\n  shift 2\n  printf '%s\\n' \"$@\" >> \"$TOOL_LOG\"\n  exit 0\nfi\nexit 98\n",
        );
        executable(
            &bin.join("pre-commit"),
            "#!/bin/sh\nprintf '%s\\n' \"$PWD\" > \"$TOOL_LOG\"\nprintf '%s\\n' \"$@\" >> \"$TOOL_LOG\"\n",
        );

        let path = env::var_os("PATH").expect("PATH should be available");
        let path = format!("{}:{}", bin.display(), path.to_string_lossy());
        let root = workspace_root();
        let output = Command::new("bash")
            .arg(root.join(".git_hooks/install.bash"))
            .current_dir(temp.path())
            .env("PATH", path)
            .env("TOOL_LOG", &log)
            .output()
            .expect("portable hook installer should start");

        assert!(
            output.status.success(),
            "portable hook installer failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let invocation = fs::read_to_string(log).expect("fake pre-commit should be invoked");
        let mut lines = invocation.lines();
        assert_eq!(
            Path::new(lines.next().expect("fake pre-commit should log its cwd")),
            root
        );
        assert_eq!(lines.next(), Some("install"));
        assert_eq!(lines.next(), Some("--install-hooks"));
        assert_eq!(lines.next(), Some("--config"));
        let config = root.join(".pre-commit-config.yaml");
        assert_eq!(lines.next().map(Path::new), Some(config.as_path()));
    }

    #[test]
    fn installer_enforces_the_pin_and_uses_python_module_over_wrong_path_executable() {
        let temp = TempDir::create();
        let bin = temp.path().join("bin");
        fs::create_dir(&bin).expect("test bin directory should be created");
        let python_log = temp.path().join("python.log");
        let bare_log = temp.path().join("bare-pre-commit.log");

        executable(
            &bin.join("python"),
            "#!/bin/sh\nprintf 'python:%s\\n' \"$*\" >> \"$PYTHON_LOG\"\nif [ \"$1\" = \"-c\" ]; then\n  case \"$2\" in\n    *metadata.version*) test -f \"$PIN_MARKER\"; exit $? ;;\n    *sys.prefix*) exit 0 ;;\n  esac\nfi\nif [ \"$1\" = \"-m\" ] && [ \"$2\" = \"pip\" ]; then\n  touch \"$PIN_MARKER\"\n  exit 0\nfi\nif [ \"$1\" = \"-m\" ] && [ \"$2\" = \"pre_commit\" ]; then\n  exit 0\nfi\nexit 98\n",
        );
        executable(
            &bin.join("pre-commit"),
            "#!/bin/sh\nprintf '%s\\n' called > \"$BARE_LOG\"\nexit 97\n",
        );

        let path = env::var_os("PATH").expect("PATH should be available");
        let path = format!("{}:{}", bin.display(), path.to_string_lossy());
        let root = workspace_root();
        let output = Command::new("bash")
            .arg(root.join(".git_hooks/install.bash"))
            .current_dir(temp.path())
            .env("PATH", path)
            .env("PYTHON_LOG", &python_log)
            .env("BARE_LOG", &bare_log)
            .env("PIN_MARKER", temp.path().join("pin-installed"))
            .env("VIRTUAL_ENV", temp.path().join("venv"))
            .output()
            .expect("hook installer should start");

        assert!(
            output.status.success(),
            "hook installer failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let invocations = fs::read_to_string(python_log).expect("selected Python should be run");
        assert!(
            invocations.contains("python:-m pip install --upgrade pre-commit==4.6.0"),
            "installer should install the exact pinned package: {invocations}"
        );
        assert!(
            !invocations.contains("--user"),
            "virtualenv installation must not use the user site: {invocations}"
        );
        assert!(
            invocations.contains("python:-m pre_commit install --install-hooks --config"),
            "installer should invoke pre_commit as a module: {invocations}"
        );
        assert!(
            !bare_log.exists(),
            "installer must not invoke a pre-commit executable already found on PATH"
        );
    }

    #[test]
    fn installer_uses_a_git_local_venv_when_environment_marker_does_not_match_python() {
        let temp = TempDir::create();
        let bin = temp.path().join("bin");
        let git_dir = temp.path().join("git-dir");
        fs::create_dir(&bin).expect("test bin directory should be created");
        fs::create_dir(&git_dir).expect("fake git directory should be created");
        let python_log = temp.path().join("python.log");
        let pin_marker = temp.path().join("pin-installed");

        executable(
            &bin.join("python"),
            "#!/bin/sh\nprintf 'python:%s\\n' \"$*\" >> \"$PYTHON_LOG\"\nif [ \"$1\" = \"-c\" ]; then\n  case \"$2\" in\n    *metadata.version*) test -f \"$PIN_MARKER\"; exit $? ;;\n    *sys.prefix*) exit 1 ;;\n  esac\nfi\nif [ \"$1\" = \"-m\" ] && [ \"$2\" = \"venv\" ]; then\n  mkdir -p \"$3/bin\"\n  cp \"$0\" \"$3/bin/python\"\n  chmod +x \"$3/bin/python\"\n  exit 0\nfi\nif [ \"$1\" = \"-m\" ] && [ \"$2\" = \"pip\" ]; then\n  touch \"$PIN_MARKER\"\n  exit 0\nfi\nif [ \"$1\" = \"-m\" ] && [ \"$2\" = \"pre_commit\" ]; then\n  exit 0\nfi\nexit 98\n",
        );
        executable(
            &bin.join("git"),
            "#!/bin/sh\nprintf '%s\\n' \"$FAKE_GIT_DIR\"\n",
        );

        let path = env::var_os("PATH").expect("PATH should be available");
        let path = format!("{}:{}", bin.display(), path.to_string_lossy());
        let root = workspace_root();
        let output = Command::new("bash")
            .arg(root.join(".git_hooks/install.bash"))
            .current_dir(temp.path())
            .env("PATH", path)
            .env("PYTHON_LOG", &python_log)
            .env("PIN_MARKER", &pin_marker)
            .env("FAKE_GIT_DIR", &git_dir)
            .env("VIRTUAL_ENV", temp.path().join("stale-venv-marker"))
            .output()
            .expect("hook installer should start");

        assert!(
            output.status.success(),
            "system-Python fallback failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let environment = git_dir.join("pyo3_bindgen-pre-commit-4.6.0");
        assert!(environment.join("bin/python").is_file());
        let invocations = fs::read_to_string(python_log).expect("selected Python should be run");
        assert!(invocations.contains(&format!("python:-m venv {}", environment.display())));
        assert!(invocations.contains("python:-m pip install --upgrade pre-commit==4.6.0"));
        assert!(!invocations.contains("--user"));
    }

    #[test]
    fn compile_sweep_rejects_unknown_and_zero_match_tiers() {
        let temp = TempDir::create();
        let root = workspace_root();
        let script = root.join("scripts/compile-sweep.sh");

        let invalid = Command::new("bash")
            .arg(&script)
            .args(["--tier", "typo"])
            .env("PYO3_BINDGEN_SWEEP_TARGET_DIR", temp.path().join("invalid"))
            .output()
            .expect("invalid-tier probe should run");
        assert!(!invalid.status.success());
        assert!(String::from_utf8_lossy(&invalid.stderr).contains("unknown tier: typo"));

        let fake_cargo = temp.path().join("cargo-no-match");
        executable(
            &fake_cargo,
            "#!/bin/sh\nprintf '%s\\n' 'test result: ok. 0 passed; 0 failed; 0 ignored'\n",
        );
        let no_match = Command::new("bash")
            .arg(script)
            .args(["--tier", "heavy"])
            .env("CARGO_BIN", fake_cargo)
            .env(
                "PYO3_BINDGEN_SWEEP_TARGET_DIR",
                temp.path().join("no-match"),
            )
            .output()
            .expect("zero-match probe should run");
        assert!(!no_match.status.success());
        assert!(
            String::from_utf8_lossy(&no_match.stderr).contains("matched no tests"),
            "zero-match sweep must fail closed: {}",
            String::from_utf8_lossy(&no_match.stderr)
        );
    }
}

/// Every `--exclude <package>` in CI, release scripts, and the release checklist must name a real
/// workspace member. A silently stale name is not a cargo error: the exclusion simply stops
/// applying, so a package the release gate believes it skipped is built, tested, and packaged.
#[test]
fn excluded_package_names_exist_in_the_workspace() {
    let root = workspace_root();
    let manifest =
        fs::read_to_string(root.join("Cargo.toml")).expect("workspace manifest should exist");
    // `members` and `default-members` repeat the same paths, so collect the declared package names
    // through a set rather than trusting either list's length.
    let mut members = manifest
        .lines()
        .filter_map(|line| {
            let path = line.trim().trim_end_matches(',').trim_matches('"');
            (path.starts_with("crates/") || path == "examples").then_some(path)
        })
        .map(|path| {
            let member = fs::read_to_string(root.join(path).join("Cargo.toml"))
                .unwrap_or_else(|_| panic!("workspace member {path} should have a manifest"));
            member
                .lines()
                .find_map(|line| line.trim().strip_prefix("name = "))
                .map(|name| name.trim().trim_matches('"').to_owned())
                .unwrap_or_else(|| panic!("workspace member {path} should declare a name"))
        })
        .collect::<Vec<_>>();
    members.sort();
    members.dedup();
    assert_eq!(
        members,
        [
            "pyo3_bindgen",
            "pyo3_bindgen_cli",
            "pyo3_bindgen_engine",
            "pyo3_bindgen_examples",
            "pyo3_bindgen_macros",
        ],
        "workspace membership changed; update every `--exclude` site with it"
    );

    for relative in [
        ".github/workflows/rust.yml",
        "scripts/release-preflight.sh",
        "docs/release_checklist.md",
    ] {
        let text = fs::read_to_string(root.join(relative))
            .unwrap_or_else(|_| panic!("{relative} should exist"));
        for (offset, _) in text.match_indices("--exclude ") {
            let excluded = text[offset + "--exclude ".len()..]
                .split_whitespace()
                .next()
                .unwrap_or_default();
            assert!(
                members.iter().any(|member| member == excluded),
                "{relative} excludes `{excluded}`, which is not a workspace member: {members:?}"
            );
        }
    }
}

/// Collect the names re-exported by `#[doc(hidden)] pub use` blocks in one crate root, skipping
/// blocks gated behind `internal-test-hooks`. Platform-gated source blocks remain in the inventory.
fn non_test_hidden_exports(source: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut rest = source;
    while let Some(offset) = rest.find("#[doc(hidden)]") {
        let preceding = &rest[..offset];
        let block_is_test_gated = preceding
            .lines()
            .rev()
            .take_while(|line| line.trim_start().starts_with("#[") || line.trim().is_empty())
            .any(|line| line.contains("internal-test-hooks"));
        let after = &rest[offset..];
        let Some(end) = after.find(';') else { break };
        let item = &after[..end];
        rest = &after[end..];
        if block_is_test_gated || !item.contains("pub use") {
            continue;
        }
        let list = item.split_once('{').map_or_else(
            || item.rsplit("::").next().unwrap_or_default(),
            |(_, tail)| tail,
        );
        for name in list.split(',') {
            let name = name.trim().trim_end_matches('}').trim();
            if !name.is_empty() && !name.contains(char::is_whitespace) {
                names.push(name.to_owned());
            }
        }
    }
    names.sort();
    names.dedup();
    names
}

/// The `#[doc(hidden)]` re-exports are a real semver surface on two published crates, so their
/// membership must change deliberately. Publication fault injection and the lock-owner counter
/// must never appear here: they are reachable only behind `internal-test-hooks`, which no
/// published consumer enables.
#[test]
fn hidden_public_surface_is_pinned_and_excludes_test_hooks() {
    const ENGINE_EXPECTED: [&str; 39] = [
        "ArtifactLockSet",
        "ArtifactPath",
        "ArtifactPathKind",
        "ArtifactPaths",
        "CurrentArtifactIdentity",
        "DeliveryDurability",
        "DeliveryKind",
        "DeliveryReceipt",
        "DeliveryState",
        "ExitClass",
        "ExternalIrSourceContext",
        "IrArtifactOpenError",
        "OperationCode",
        "OperationFailure",
        "PublicationAttempt",
        "PublicationPlan",
        "PublicationPolicy",
        "PublicationSession",
        "PublicationTarget",
        "PublicationTargets",
        "ReportWriteAttempt",
        "RunFailure",
        "RunOutcome",
        "RunState",
        "artifact_identity_key",
        "artifact_identity_path",
        "artifact_lock_keys",
        "artifact_lock_path",
        "artifact_revalidation_identity_matches",
        "conservative_casefold_identity_key",
        "existing_file_identity",
        "open_artifact_lock",
        "open_ir_artifact_file",
        "publish_report",
        "validate_windows_artifact_component",
        "validate_windows_artifact_path",
        "windows_file_id_identity",
        "windows_legacy_file_identity_is_sound",
        "windows_lexically_normalized_absolute_path",
    ];
    const FACADE_EXPECTED: [&str; 37] = [
        "ArtifactLockSet",
        "ArtifactPath",
        "ArtifactPathKind",
        "ArtifactPaths",
        "CurrentArtifactIdentity",
        "DeliveryDurability",
        "DeliveryKind",
        "DeliveryReceipt",
        "DeliveryState",
        "ExitClass",
        "ExternalIrSourceContext",
        "IrArtifactOpenError",
        "OperationCode",
        "OperationFailure",
        "PublicationAttempt",
        "PublicationPlan",
        "PublicationPolicy",
        "PublicationSession",
        "ReportWriteAttempt",
        "RunFailure",
        "RunOutcome",
        "RunState",
        "artifact_identity_key",
        "artifact_identity_path",
        "artifact_lock_keys",
        "artifact_lock_path",
        "artifact_revalidation_identity_matches",
        "conservative_casefold_identity_key",
        "existing_file_identity",
        "open_artifact_lock",
        "open_ir_artifact_file",
        "publish_report",
        "validate_windows_artifact_component",
        "validate_windows_artifact_path",
        "windows_file_id_identity",
        "windows_legacy_file_identity_is_sound",
        "windows_lexically_normalized_absolute_path",
    ];
    const TEST_ONLY: [&str; 4] = [
        "LOCK_OWNER_COUNTER",
        "TestPublicationFailure",
        "inject_output_publication_failure_once",
        "inject_report_publication_failure_once",
    ];
    let root = workspace_root();
    let engine = fs::read_to_string(root.join("crates/pyo3_bindgen_engine/src/lib.rs"))
        .expect("engine crate root should exist");
    let facade = fs::read_to_string(root.join("crates/pyo3_bindgen/src/lib.rs"))
        .expect("facade crate root should exist");

    let engine_hidden = non_test_hidden_exports(&engine);
    let facade_hidden = non_test_hidden_exports(&facade);
    assert_eq!(
        engine_hidden, ENGINE_EXPECTED,
        "engine hidden surface changed; review it as a public-API change: {engine_hidden:?}"
    );
    assert_eq!(
        facade_hidden, FACADE_EXPECTED,
        "facade hidden surface changed; review it as a public-API change: {facade_hidden:?}"
    );

    for name in TEST_ONLY {
        assert!(
            !engine_hidden.contains(&name.to_owned()),
            "{name} must stay behind the `internal-test-hooks` feature in the engine"
        );
        assert!(
            !facade_hidden.contains(&name.to_owned()),
            "{name} must stay behind the `internal-test-hooks` feature in the facade"
        );
        assert!(
            engine.contains(name) && facade.contains(name),
            "{name} should still be re-exported behind the feature"
        );
    }

    // The facade omits exactly the engine-only publication target types.
    let facade_only: Vec<_> = facade_hidden
        .iter()
        .filter(|name| !engine_hidden.contains(name))
        .collect();
    assert!(
        facade_only.is_empty(),
        "facade must not widen the hidden surface beyond the engine: {facade_only:?}"
    );
}
