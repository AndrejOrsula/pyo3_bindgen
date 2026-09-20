#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static COUNTER: AtomicU64 = AtomicU64::new(0);

struct TestDirectory(PathBuf);

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn workspace_root() -> Option<PathBuf> {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find(|directory| {
            std::fs::read_to_string(directory.join("Cargo.toml"))
                .is_ok_and(|manifest| manifest.contains("[workspace]"))
        })
        .map(Path::to_path_buf)
}

fn write_executable(path: &Path, source: &str) {
    std::fs::write(path, source).expect("release fixture should be writable");
    let mut permissions = std::fs::metadata(path)
        .expect("release fixture metadata should be readable")
        .permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(path, permissions).expect("release fixture should be executable");
}

fn assert_output_success(name: &str, output: &Output) {
    assert!(
        output.status.success(),
        "{name} should succeed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn helper_output(root: &Path, repository_url: &str) -> Output {
    Command::new("bash")
        .args([
            "-p",
            "-c",
            r#"
set -Eeuo pipefail
source "$RELEASE_COMMON"
die() { printf '%s\n' "$*" >&2; exit 1; }
normalized="$(release_common_validate_public_repository_url "$RELEASE_REPOSITORY_URL")"
printf '%s\n' "$normalized"
"#,
        ])
        .env("RELEASE_COMMON", root.join("scripts/release-common.sh"))
        .env("RELEASE_REPOSITORY_URL", repository_url)
        .env_remove("BASH_ENV")
        .output()
        .expect("repository helper subprocess should run")
}

fn metadata_for(repository_url: &str, target_directory: &Path) -> String {
    let packages = [
        "pyo3_bindgen_engine",
        "pyo3_bindgen_macros",
        "pyo3_bindgen",
        "pyo3_bindgen_cli",
    ]
    .into_iter()
    .map(|name| {
        serde_json::json!({
            "name": name,
            "version": "0.6.0",
            "repository": repository_url,
            "publish": ["crates-io"],
        })
    })
    .collect::<Vec<_>>();
    serde_json::json!({"packages": packages, "target_directory": target_directory}).to_string()
}

struct ScriptFixture<'a> {
    cargo: &'a Path,
    python: &'a Path,
    python_version: &'a str,
    git: &'a Path,
    rustup: &'a Path,
    toolchain_root: &'a Path,
    metadata: &'a Path,
    git_log: &'a Path,
    curl_log: &'a Path,
}

impl ScriptFixture<'_> {
    fn command(&self, script: &Path, curl: Option<&Path>) -> Command {
        let mut command = Command::new(script);
        command
            .arg("--check")
            .env("CARGO_BIN", self.cargo)
            .env("PYTHON_BIN", self.python)
            .env("GIT_BIN", self.git)
            .env("RUSTUP_BIN", self.rustup)
            .env("RELEASE_RUST_TOOLCHAIN", "1.2.3")
            .env("RELEASE_PYTHON_VERSION", self.python_version)
            .env("RELEASE_TEST_TOOLCHAIN_ROOT", self.toolchain_root)
            .env("RELEASE_TEST_METADATA", self.metadata)
            .env("RELEASE_TEST_GIT_LOG", self.git_log)
            .env("RELEASE_TEST_CURL_LOG", self.curl_log)
            .env_remove("BASH_ENV")
            .env_remove("CARGO_REGISTRY_TOKEN")
            .env_remove("RELEASE_REGISTRY_TOKEN")
            .env_remove("RELEASE_ENV_SANITIZED");
        if let Some(curl) = curl {
            command.env("CURL_BIN", curl);
        } else {
            command.env_remove("CURL_BIN");
        }
        for variable in [
            "PYO3_BUILD_EXTENSION_MODULE",
            "PYO3_CONFIG_FILE",
            "PYO3_CROSS",
            "PYO3_CROSS_LIB_DIR",
            "PYO3_CROSS_PYTHON_VERSION",
            "PYO3_CROSS_PYTHON_IMPLEMENTATION",
            "PYO3_NO_PYTHON",
            "PYO3_PRINT_CONFIG",
            "PYO3_USE_ABI3_FORWARD_COMPATIBILITY",
            "PYO3_USE_STABLE_ABI_FORWARD_COMPATIBILITY",
            "UNSAFE_PYO3_SKIP_VERSION_CHECK",
        ] {
            command.env_remove(variable);
        }
        command
    }
}

#[test]
fn release_identity_is_pinned_and_checked_offline() {
    let Some(root) = workspace_root() else {
        return;
    };
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should follow the Unix epoch")
        .as_nanos();
    let directory_path = std::env::temp_dir().join(format!(
        "pyo3-bindgen-release-repository-{}-{nonce}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir(&directory_path).expect("release fixture directory should be exclusive");
    let directory = TestDirectory(directory_path);

    let toolchain_root = directory.0.join("toolchain");
    let toolchain_bin = toolchain_root.join("bin");
    std::fs::create_dir_all(&toolchain_bin).expect("toolchain fixture should be creatable");
    let cargo = directory.0.join("cargo");
    let rustup = directory.0.join("rustup");
    let git = directory.0.join("git");
    let curl = directory.0.join("curl");
    let toolchain_cargo = toolchain_bin.join("cargo");
    let toolchain_rustc = toolchain_bin.join("rustc");
    let metadata = directory.0.join("metadata.json");
    let git_log = directory.0.join("git.log");
    let curl_log = directory.0.join("curl.log");
    let target_directory = directory.0.join("target");

    let cargo_source = r#"#!/bin/bash
set -Eeuo pipefail
cat "${RELEASE_TEST_METADATA:?}"
"#;
    write_executable(&cargo, cargo_source);
    write_executable(&toolchain_cargo, "#!/bin/bash\nexit 0\n");
    write_executable(
        &toolchain_rustc,
        "#!/bin/bash\nprintf '%s\\n' 'rustc 1.2.3 (release test fixture)'\n",
    );
    write_executable(
        &rustup,
        r#"#!/bin/bash
set -Eeuo pipefail
[[ "$#" == 3 && "$1" == toolchain && "$2" == list && "$3" == --verbose ]]
printf '%s\n' "1.2.3-x86_64-unknown-linux-gnu ${RELEASE_TEST_TOOLCHAIN_ROOT:?}"
"#,
    );
    write_executable(
        &git,
        r#"#!/bin/bash
printf '%s\n' "$*" >> "${RELEASE_TEST_GIT_LOG:?}"
exit 0
"#,
    );
    write_executable(
        &curl,
        r#"#!/bin/bash
printf '%s\n' "$*" >> "${RELEASE_TEST_CURL_LOG:?}"
exit 0
"#,
    );

    let mut python_candidates = Vec::<PathBuf>::new();
    if let Some(configured) = std::env::var_os("PYO3_PYTHON") {
        python_candidates.push(PathBuf::from(configured));
    }
    python_candidates.push(PathBuf::from("python3"));
    let (python, python_version) = python_candidates
        .into_iter()
        .find_map(|candidate| {
            let output = Command::new(&candidate)
                .args([
                    "-c",
                    "import sys, sysconfig; print(sys.implementation.name); \
                     print('.'.join(map(str, sys.version_info[:3]))); \
                     print(int(sysconfig.get_config_var('Py_ENABLE_SHARED') == 1))",
                ])
                .output()
                .ok()?;
            if !output.status.success() {
                return None;
            }
            let facts = String::from_utf8(output.stdout).ok()?;
            let mut facts = facts.lines();
            let implementation = facts.next()?;
            let version = facts.next()?;
            let shared = facts.next()?;
            (implementation == "cpython" && shared == "1").then(|| (candidate, version.to_owned()))
        })
        .expect("release identity fixture needs a shared local CPython");
    let fixture = ScriptFixture {
        cargo: &cargo,
        python: &python,
        python_version: &python_version,
        git: &git,
        rustup: &rustup,
        toolchain_root: &toolchain_root,
        metadata: &metadata,
        git_log: &git_log,
        curl_log: &curl_log,
    };

    let accepted = [
        "https://github.com/AndrejOrsula/pyo3_bindgen",
        "https://github.com/AndrejOrsula/pyo3_bindgen/",
        "https://github.com/AndrejOrsula/pyo3_bindgen.git",
        "https://github.com/AndrejOrsula/pyo3_bindgen.git/",
    ];
    for repository_url in accepted {
        let output = helper_output(&root, repository_url);
        assert_output_success("approved repository URL helper", &output);
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "https://github.com/AndrejOrsula/pyo3_bindgen\n"
        );
    }

    let rejected = [
        ("wrong owner", "https://github.com/SpaceRL/pyo3_bindgen"),
        (
            "wrong repository",
            "https://github.com/AndrejOrsula/other_repository",
        ),
        (
            "private repository",
            "https://github.com/AndrejOrsula/pyo3_bindgen_private",
        ),
        (
            "temporary SpaceRL repository",
            "https://github.com/SpaceRL/pyo3_bindgen_private",
        ),
        (
            "wrong scheme",
            "http://github.com/AndrejOrsula/pyo3_bindgen",
        ),
        ("wrong host", "https://gitlab.com/AndrejOrsula/pyo3_bindgen"),
        (
            "extra path",
            "https://github.com/AndrejOrsula/pyo3_bindgen/releases",
        ),
        (
            "query string",
            "https://github.com/AndrejOrsula/pyo3_bindgen?tab=readme",
        ),
        ("empty URL", ""),
    ];
    for (name, repository_url) in rejected {
        let output = helper_output(&root, repository_url);
        assert!(
            !output.status.success(),
            "{name} should be rejected\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let preflight = root.join("scripts/release-preflight.sh");
    let publisher = root.join("scripts/publish-workspace.sh");
    for script in [&preflight, &publisher] {
        let source = std::fs::read_to_string(script).expect("release script should be readable");
        assert_eq!(
            source
                .matches("release_common_validate_public_repository_url")
                .count(),
            1,
            "{} should call the shared repository validator",
            script.display()
        );
    }

    for repository_url in accepted {
        std::fs::write(&metadata, metadata_for(repository_url, &target_directory))
            .expect("metadata fixture should be writable");
        let metadata_before =
            std::fs::read(&metadata).expect("metadata fixture should be readable");

        let output = fixture
            .command(&preflight, None)
            .output()
            .expect("release preflight should run");
        assert_output_success("release preflight identity check", &output);
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("check OK"),
            "preflight should report check success\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let output = fixture
            .command(&publisher, Some(&curl))
            .output()
            .expect("workspace publisher should run");
        assert_output_success("workspace publisher identity check", &output);
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("check OK"),
            "publisher should report check success\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        assert_eq!(
            std::fs::read(&metadata).expect("metadata fixture should remain readable"),
            metadata_before,
            "check mode must not mutate workspace metadata"
        );
        assert!(
            !git_log.exists() && !curl_log.exists(),
            "approved check mode must not read tags or access the network"
        );
    }

    for (name, repository_url) in rejected {
        std::fs::write(&metadata, metadata_for(repository_url, &target_directory))
            .expect("metadata fixture should be writable");
        let metadata_before =
            std::fs::read(&metadata).expect("metadata fixture should be readable");
        let output = fixture
            .command(&preflight, None)
            .output()
            .expect("rejected preflight should run");
        assert!(
            !output.status.success(),
            "{name} should fail preflight\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            !String::from_utf8_lossy(&output.stdout).contains("check OK"),
            "{name} must fail before check-mode success"
        );
        assert!(
            !git_log.exists() && !curl_log.exists(),
            "{name} must fail before tag or network access"
        );

        let output = fixture
            .command(&publisher, Some(&curl))
            .output()
            .expect("rejected publisher should run");
        assert!(
            !output.status.success(),
            "{name} should fail publisher preflight\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            !String::from_utf8_lossy(&output.stdout).contains("check OK"),
            "{name} publisher must fail before check-mode success"
        );
        assert!(
            !git_log.exists() && !curl_log.exists(),
            "{name} publisher must fail before tag or network access"
        );
        assert_eq!(
            std::fs::read(&metadata).expect("metadata fixture should remain readable"),
            metadata_before,
            "{name} checks must not mutate workspace metadata"
        );
    }

    assert!(
        !target_directory.exists(),
        "check mode must not create package output"
    );
}
