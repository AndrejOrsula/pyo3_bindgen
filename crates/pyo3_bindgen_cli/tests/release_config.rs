use std::path::{Path, PathBuf};

fn workspace_root() -> Option<PathBuf> {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find(|directory| {
            std::fs::read_to_string(directory.join("Cargo.toml"))
                .is_ok_and(|manifest| manifest.contains("[workspace]"))
        })
        .map(Path::to_path_buf)
}

fn normalize_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn bounded_section<'a>(document: &'a str, start: &str, end: &str, name: &str) -> &'a str {
    document
        .split_once(start)
        .and_then(|(_, remainder)| remainder.split_once(end))
        .map(|(section, _)| section)
        .unwrap_or_else(|| panic!("{name} should contain a bounded {start} section"))
}

fn workflow_job<'a>(workflow: &'a str, name: &str) -> &'a str {
    let marker = format!("\n  {name}:");
    let start = workflow
        .find(&marker)
        .map(|index| index + 1)
        .unwrap_or_else(|| panic!("workflow should contain a top-level {name} job"));
    let after_header = workflow[start..]
        .find('\n')
        .map(|index| start + index + 1)
        .unwrap_or(workflow.len());
    let remainder = &workflow[after_header..];
    let end = remainder
        .char_indices()
        .find_map(|(index, character)| {
            if character != '\n' {
                return None;
            }
            let next_line = &remainder[index + 1..];
            (next_line.starts_with("  ") && !next_line.starts_with("   ")).then_some(index)
        })
        .unwrap_or(remainder.len());
    &workflow[start..after_header + end]
}

const DUAL_LINUX_RUNNER: &str = "runs-on: ${{ github.repository == 'SpaceRL/pyo3_bindgen_private' && github.event.repository.private == true && (github.event_name != 'pull_request' || github.event.pull_request.head.repo.full_name == github.repository) && fromJSON('[\"self-hosted\", \"Linux\", \"X64\"]') || 'ubuntu-24.04' }}";
const PRIVATE_FORK_ADMISSION: &str = "${{ github.repository != 'SpaceRL/pyo3_bindgen_private' || github.event.repository.private != true || github.event_name != 'pull_request' || github.event.pull_request.head.repo.full_name == github.repository }}";
const DENY_ADMISSION: &str = "${{ always() && (github.repository != 'SpaceRL/pyo3_bindgen_private' || github.event.repository.private != true || github.event_name != 'pull_request' || github.event.pull_request.head.repo.full_name == github.repository) }}";

fn assert_workflow_runs_on_default_branch(name: &str, workflow: &str) {
    assert!(
        workflow.contains("push:") && workflow.contains("branches:"),
        "{name} workflow should define an explicit push branch gate"
    );
    assert!(
        workflow.lines().any(|line| line.trim() == "- dev"),
        "{name} workflow should run on the repository default branch `dev`"
    );
}

fn assert_actions_are_sha_pinned(name: &str, workflow: &str) {
    for line in workflow.lines().map(str::trim) {
        let line = line.strip_prefix('-').unwrap_or(line).trim_start();
        let Some(reference) = line.strip_prefix("uses:") else {
            continue;
        };
        if reference.trim().starts_with("./.github/workflows/")
            || reference.trim().starts_with("./.github/actions/")
        {
            continue;
        }
        let (_, revision) = reference
            .trim()
            .split_once('@')
            .unwrap_or_else(|| panic!("{name} action should include a revision: {line}"));
        let revision = revision
            .split_whitespace()
            .next()
            .expect("action revision should not be empty");
        assert!(
            revision.len() == 40 && revision.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "{name} action must be pinned to a full commit SHA: {line}"
        );
    }
}

fn assert_cache_follows_toolchain(name: &str, workflow_job: &str) {
    let toolchain_position = workflow_job
        .rfind("uses: ./.github/actions/setup-rust")
        .unwrap_or_else(|| panic!("{name} should provision a Rust toolchain"));
    let cache_position = workflow_job
        .find("uses: Swatinem/rust-cache@")
        .unwrap_or_else(|| panic!("{name} should use the shared Rust cache"));
    assert!(
        toolchain_position < cache_position,
        "{name} must initialize the Rust toolchain before the Rust cache"
    );
}

fn assert_checkouts_do_not_persist_credentials(name: &str, workflow: &str) {
    let lines = workflow.lines().collect::<Vec<_>>();
    let mut checkout_count = 0;
    for (index, line) in lines.iter().enumerate() {
        if !line.trim_start().starts_with("- uses: actions/checkout@") {
            continue;
        }
        checkout_count += 1;
        let step_indent = line.len() - line.trim_start().len();
        let checkout_step = lines[index + 1..]
            .iter()
            .take_while(|line| {
                line.trim().is_empty() || line.len() - line.trim_start().len() > step_indent
            })
            .copied()
            .collect::<Vec<_>>();
        let with_index = checkout_step
            .iter()
            .position(|line| {
                line.len() - line.trim_start().len() == step_indent + 2 && line.trim() == "with:"
            })
            .unwrap_or_else(|| panic!("{name} checkout step should contain a `with` block"));
        let with_indent = step_indent + 2;
        let persists_credentials = checkout_step[with_index + 1..]
            .iter()
            .take_while(|line| {
                line.trim().is_empty() || line.len() - line.trim_start().len() > with_indent
            })
            .any(|line| {
                line.len() - line.trim_start().len() == with_indent + 2
                    && line.trim() == "persist-credentials: false"
            });
        assert!(
            persists_credentials,
            "{name} checkout `with` block must disable persisted credentials"
        );
    }
    assert!(
        checkout_count > 0,
        "{name} workflow should check out source"
    );
}

fn assert_repository_permissions_are_read_only(name: &str, workflow: &str) {
    for raw_line in workflow.lines() {
        let line = raw_line.split('#').next().unwrap_or(raw_line).trim();
        assert!(
            !line.contains("write-all") && (!line.contains(": write") || line == "id-token: write"),
            "{name} workflow must not grant repository write permission: {line}"
        );
    }
}

fn assert_pre_commit_revisions_are_sha_pinned(config: &str) {
    for line in config.lines().map(str::trim) {
        let Some(revision) = line.strip_prefix("rev:") else {
            continue;
        };
        let revision = revision
            .split_whitespace()
            .next()
            .expect("pre-commit revision should not be empty");
        assert!(
            revision.len() == 40 && revision.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "pre-commit hooks must be pinned to full commit SHAs: {line}"
        );
    }
}

fn workspace_manifest(root: &Path) -> String {
    std::fs::read_to_string(root.join("Cargo.toml")).expect("workspace Cargo.toml should exist")
}

fn quoted_value_after<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let start = line.find(key)? + key.len();
    let end = line[start..].find('"')?;
    Some(&line[start..start + end])
}

fn workspace_package_version(root: &Path) -> String {
    let manifest = workspace_manifest(root);
    let mut in_workspace_package = false;

    for line in manifest.lines().map(str::trim) {
        if line.starts_with('[') {
            in_workspace_package = line == "[workspace.package]";
            continue;
        }

        if in_workspace_package && line.starts_with("version =") {
            if let Some(version) = quoted_value_after(line, "version = \"") {
                return version.to_owned();
            }
        }
    }

    panic!("workspace Cargo.toml should pin the workspace package version");
}

fn workspace_dependency_version(root: &Path, dependency: &str) -> String {
    let manifest = workspace_manifest(root);
    let mut in_workspace_dependencies = false;

    for line in manifest.lines().map(str::trim) {
        if line.starts_with('[') {
            in_workspace_dependencies = line == "[workspace.dependencies]";
            continue;
        }

        if in_workspace_dependencies && line.starts_with(&format!("{dependency} =")) {
            if let Some(version) = quoted_value_after(line, "version = \"") {
                return version.to_owned();
            }
        }
    }

    panic!("workspace Cargo.toml should pin {dependency}");
}

fn workspace_pyo3_version(root: &Path) -> String {
    workspace_dependency_version(root, "pyo3")
        .strip_prefix('=')
        .unwrap_or_else(|| panic!("workspace PyO3 dependency should use an exact =X.Y.Z pin"))
        .to_owned()
}

fn release_notes_path(root: &Path, release_minor_version: &str) -> PathBuf {
    root.join(format!("docs/release_notes_{release_minor_version}.md"))
}

fn release_notes(root: &Path, release_minor_version: &str) -> String {
    std::fs::read_to_string(release_notes_path(root, release_minor_version))
        .expect("release notes for workspace package version should exist")
}

fn release_minor_version(root: &Path) -> String {
    minor_version(&workspace_package_version(root))
}

fn internal_workspace_dependency_versions(root: &Path) -> [String; 3] {
    [
        workspace_dependency_version(root, "pyo3_bindgen"),
        workspace_dependency_version(root, "pyo3_bindgen_engine"),
        workspace_dependency_version(root, "pyo3_bindgen_macros"),
    ]
}

fn minor_version(version: &str) -> String {
    let mut parts = version.split('.');
    let major = parts.next().expect("version should have a major component");
    let minor = parts.next().expect("version should have a minor component");
    format!("{major}.{minor}")
}

#[test]
fn workspace_layout_separates_publishable_crates_from_examples() {
    let Some(root) = workspace_root() else {
        return;
    };
    let manifest =
        std::fs::read_to_string(root.join("Cargo.toml")).expect("workspace manifest should exist");
    let members = bounded_section(&manifest, "members = [", "]", "workspace manifest");
    let default_members =
        bounded_section(&manifest, "default-members = [", "]", "workspace manifest");

    for package in [
        "pyo3_bindgen",
        "pyo3_bindgen_cli",
        "pyo3_bindgen_engine",
        "pyo3_bindgen_macros",
    ] {
        let package_path = format!("crates/{package}");
        assert!(
            root.join(&package_path).join("Cargo.toml").is_file(),
            "{package} should live under crates/"
        );
        assert!(
            !root.join(package).exists(),
            "{package} should not remain at the workspace root"
        );
        assert!(
            members.contains(&format!("\"{package_path}\""))
                && default_members.contains(&format!("\"{package_path}\"")),
            "{package_path} should be an explicit workspace and default member"
        );
    }

    assert!(
        members.contains("\"examples\"") && !default_members.contains("\"examples\""),
        "examples should be a non-default workspace member"
    );
    let examples_manifest = std::fs::read_to_string(root.join("examples/Cargo.toml"))
        .expect("examples manifest should exist");
    assert!(
        examples_manifest.contains("publish = false"),
        "examples package must remain unpublished"
    );
    for dependency in ["pyo3_bindgen", "pyo3_bindgen_engine", "pyo3_bindgen_macros"] {
        assert!(
            manifest.contains(&format!(
                "{dependency} = {{ path = \"crates/{dependency}\","
            )),
            "{dependency} workspace dependency should use its crates/ path"
        );
    }
}

#[test]
fn release_version_examples_track_workspace_package_version() {
    let Some(root) = workspace_root() else {
        return;
    };
    let workspace_version = workspace_package_version(&root);
    let release_minor_version = minor_version(&workspace_version);

    assert_eq!(
        internal_workspace_dependency_versions(&root),
        [
            workspace_version.clone(),
            workspace_version.clone(),
            workspace_version.clone()
        ],
        "internal workspace dependency versions should track the workspace package version"
    );

    let readme = std::fs::read_to_string(root.join("README.md")).expect("README.md should exist");
    let checklist = std::fs::read_to_string(root.join("docs/release_checklist.md"))
        .expect("docs/release_checklist.md should exist");
    let public_docs = std::fs::read_to_string(root.join("crates/pyo3_bindgen/src/lib.rs"))
        .expect("pyo3_bindgen public docs should exist");
    let notes = release_notes(&root, &release_minor_version);

    let version_note = format!("Version `{release_minor_version}` of `pyo3_bindgen`");
    let dependency_snippet = format!("pyo3_bindgen = {{ version = \"{release_minor_version}\"");
    let release_notes_file = format!("docs/release_notes_{release_minor_version}.md");
    let release_notes_heading = format!("# pyo3_bindgen {release_minor_version} Release Notes");

    assert!(
        readme.contains(&version_note),
        "README API compatibility note should track workspace package minor version"
    );
    assert!(
        readme.matches(&dependency_snippet).count() >= 2,
        "README dependency examples should track workspace package minor version"
    );
    assert!(
        public_docs.matches(&dependency_snippet).count() >= 2,
        "public crate docs dependency examples should track workspace package minor version"
    );
    assert!(
        checklist.contains(&release_notes_file),
        "release checklist should point at the workspace package minor release notes"
    );
    assert!(
        notes.contains(&release_notes_heading),
        "release notes heading should track workspace package minor version"
    );
}

#[test]
fn feature_consumer_fixtures_track_workspace_pyo3() {
    let Some(root) = workspace_root() else {
        return;
    };
    let pyo3_version = workspace_pyo3_version(&root);
    let manifest_pin = format!(r#"pyo3 = {{ version = "={pyo3_version}""#);
    let lock_pin = format!("name = \"pyo3\"\nversion = \"{pyo3_version}\"");

    for fixture in ["num_complex_consumer", "numpy_consumer"] {
        let fixture = root
            .join("crates/pyo3_bindgen_engine/tests/fixtures")
            .join(fixture);
        let manifest = std::fs::read_to_string(fixture.join("Cargo.toml.fixture"))
            .expect("feature consumer manifest should exist");
        let lock = std::fs::read_to_string(fixture.join("Cargo.lock.fixture"))
            .expect("feature consumer lockfile should exist");

        assert!(
            manifest.contains(&manifest_pin),
            "feature consumer should pin the workspace PyO3 version"
        );
        assert!(
            lock.contains(&lock_pin),
            "feature consumer lockfile should resolve the workspace PyO3 version"
        );
    }

    for source in [
        "crates/pyo3_bindgen_engine/tests/bindgen.rs",
        "crates/pyo3_bindgen_engine/tests/gymnasium_dogfood.rs",
        "crates/pyo3_bindgen_engine/tests/ir_hierarchy_compile.rs",
        "crates/pyo3_bindgen_engine/tests/module_methods.rs",
        "crates/pyo3_bindgen_engine/tests/property_safety.rs",
        "crates/pyo3_bindgen_engine/tests/schema_ir.rs",
    ] {
        let source_text =
            std::fs::read_to_string(root.join(source)).expect("consumer test source should exist");
        assert!(
            source_text.contains(r#"env!("PYO3_BINDGEN_PYO3_VERSION")"#)
                && source_text.contains(r#"version = "={pyo3_version}""#)
                && !source_text.contains(&format!(r#"version = "{pyo3_version}""#)),
            "{source} temporary consumers must use the exact workspace PyO3 pin",
        );
    }
}

#[test]
fn release_checklist_covers_publish_decision_inputs() {
    let Some(root) = workspace_root() else {
        return;
    };
    let checklist = std::fs::read_to_string(root.join("docs/release_checklist.md"))
        .expect("docs/release_checklist.md should exist");

    for required in [
        "publish/no-publish",
        "PyO3",
        "Python",
        "CPython",
        "GraalPy",
        "generated API breakage",
        "wrapper semantics",
        "enum mirror behavior",
        "report format",
        "known fallbacks",
        "Gymnasium dogfood",
        "generic",
        "does not inherit Gymnasium-specific Python compatibility policy",
        "manual publish workflow",
        "trusted publishing",
        "advertised-public",
        "registry checksum",
        "cargo publish",
        "--locked",
        "pyo3_bindgen_engine",
        "pyo3_bindgen_macros",
        "pyo3_bindgen_cli",
        "CANDIDATE_SHA",
        "CANDIDATE_TAG",
        "PUBLIC_OWNER_REPO",
        "PUBLIC_OWNER_ID",
        "PUBLIC_REPOSITORY_ID",
        "PUBLIC_REMOTE",
        "## Current state and execution order",
        "## Retained follow-ups and conditional decisions",
        "22929099",
        "745882942",
        "public",
        "refs/tags/",
        "clean-tree",
        "head_sha",
        "run ID",
        "passed",
        "failed",
        "unavailable",
        "structural check-mode",
        "cannot verify",
        "actions/permissions",
        "tag ruleset",
        "rulesets",
        "environments/crates-io",
        "environments/crates-io/deployment-branch-policies",
        "PYO3_BINDGEN_CRATES_IO_POLICY_SHA",
        "prevent_self_review=false",
        "sole-maintainer",
        "required_approving_review_count=0",
        "require_last_push_approval=false",
        "can_admins_bypass=false",
        "workflow filename | `publish.yml`",
        "trusted-publisher",
        "use_default",
        "include_claim_keys",
        "use_immutable_subject",
        "private vulnerability reporting",
        "gh workflow run .github/workflows/publish.yml",
        "--ref \"$CANDIDATE_TAG\"",
        "-f tag=\"$CANDIDATE_TAG\"",
        "-f sha=\"$CANDIDATE_SHA\"",
        "exactly equal to `CANDIDATE_TAG`",
        "https://docs.github.com/en/actions/reference/workflows-and-actions/workflow-syntax",
        "https://docs.github.com/en/actions/reference/workflows-and-actions/events-that-trigger-workflows",
        "https://docs.github.com/en/actions/reference/workflows-and-actions/contexts",
        "https://docs.github.com/en/actions/reference/workflows-and-actions/deployments-and-environments",
        "https://docs.github.com/en/actions/how-tos/deploy/configure-and-manage-deployments/manage-environments",
        "https://docs.github.com/en/actions/reference/security/oidc",
        "https://docs.github.com/en/rest/actions/oidc",
        "ubuntu-24.04",
    ] {
        assert!(
            checklist.contains(required),
            "release checklist should cover {required}"
        );
    }
}

#[cfg(unix)]
#[test]
fn release_remote_tag_parser_accepts_only_lightweight_or_annotated_tags() {
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory(PathBuf);

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn write_executable(path: &Path, source: &str) {
        std::fs::write(path, source).expect("fake Git source should be writable");
        let mut permissions = std::fs::metadata(path)
            .expect("fake Git metadata should be readable")
            .permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(path, permissions).expect("fake Git should become executable");
    }

    let Some(root) = workspace_root() else {
        return;
    };
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should follow the Unix epoch")
        .as_nanos();
    let test_directory = TestDirectory(std::env::temp_dir().join(format!(
        "pyo3-bindgen-release-tag-parser-{}-{nonce}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )));
    std::fs::create_dir_all(&test_directory.0).expect("parser test directory should be creatable");
    let fake_git = test_directory.0.join("fake-git");
    let git_args_log = test_directory.0.join("git-args.log");
    write_executable(
        &fake_git,
        "#!/usr/bin/env bash\nprintf '%s\\0' \"$@\" > \"${RELEASE_TEST_GIT_ARGS_LOG:?}\"\nprintf '%s' \"${RELEASE_TEST_GIT_OUTPUT-}\"\n[[ \"${RELEASE_TEST_GIT_APPEND_NUL:-0}\" == 0 ]] || printf '\\0'\nexit \"${RELEASE_TEST_GIT_STATUS:-0}\"\n",
    );

    let common = root.join("scripts/release-common.sh");
    let run_parser = |refs: &str, status: &str, append_nul: bool| {
        Command::new("bash")
            .arg("-c")
            .arg(
                r#"source "$RELEASE_COMMON"
die() { printf '%s\n' "$*" >&2; exit 1; }
release_common_resolve_remote_tag_commit "$FAKE_GIT" https://example.invalid v0.6.0 "parser test"
"#,
            )
            .env("RELEASE_COMMON", &common)
            .env("FAKE_GIT", &fake_git)
            .env("RELEASE_TEST_GIT_ARGS_LOG", &git_args_log)
            .env("RELEASE_TEST_GIT_OUTPUT", refs)
            .env("RELEASE_TEST_GIT_STATUS", status)
            .env(
                "RELEASE_TEST_GIT_APPEND_NUL",
                if append_nul { "1" } else { "0" },
            )
            .output()
            .expect("tag parser subprocess should run")
    };
    let direct = "A".repeat(40);
    let direct_row = |sha: &str| format!("{sha}\trefs/tags/v0.6.0");
    let peeled_row = |sha: &str| format!("{sha}\trefs/tags/v0.6.0^{{}}");

    let output = run_parser(&direct_row(&direct), "0", false);
    assert!(
        output.status.success(),
        "lightweight tag should parse\nstderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        format!("{}\n", direct.to_lowercase())
    );
    let git_args = std::fs::read(&git_args_log).expect("fake Git should record its arguments");
    let git_args = git_args
        .split(|byte| *byte == 0)
        .filter(|argument| !argument.is_empty())
        .collect::<Vec<_>>();
    assert_eq!(
        git_args,
        [
            b"ls-remote".as_slice(),
            b"--exit-code".as_slice(),
            b"https://example.invalid".as_slice(),
            b"refs/tags/v0.6.0".as_slice(),
            b"refs/tags/v0.6.0^{}".as_slice(),
        ],
        "tag parser must request only the exact direct and peeled refs"
    );

    let output = run_parser(&format!("{}\n", direct_row(&direct)), "0", false);
    assert!(
        output.status.success(),
        "a normal terminal newline should parse\nstderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        format!("{}\n", direct.to_lowercase())
    );

    let annotated_object = "B".repeat(40);
    let annotated_commit = "C".repeat(40);
    let annotated = format!(
        "{}\n{}\n",
        direct_row(&annotated_object),
        peeled_row(&annotated_commit)
    );
    let output = run_parser(&annotated, "0", false);
    assert!(
        output.status.success(),
        "annotated tag should parse\nstderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        format!("{}\n", annotated_commit.to_lowercase())
    );

    let annotated_without_terminal_lf = format!(
        "{}\n{}",
        direct_row(&annotated_object),
        peeled_row(&annotated_commit)
    );
    let output = run_parser(&annotated_without_terminal_lf, "0", false);
    assert!(
        output.status.success(),
        "annotated tag without a terminal newline should parse\nstderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let invalid = "g".repeat(40);
    let short = "d".repeat(39);
    let malformed = [
        ("missing output", String::new()),
        ("missing direct row", peeled_row(&annotated_commit)),
        ("missing SHA field", "\trefs/tags/v0.6.0".to_owned()),
        ("missing ref field", format!("{direct}\t")),
        ("malformed row", format!("{direct} refs/tags/v0.6.0")),
        ("extra field", format!("{direct}\trefs/tags/v0.6.0\textra")),
        ("invalid direct SHA", direct_row(&invalid)),
        ("short direct SHA", direct_row(&short)),
        (
            "invalid peeled SHA",
            format!("{}\n{}", direct_row(&direct), peeled_row(&invalid)),
        ),
        ("unexpected ref", format!("{direct}\trefs/tags/other")),
        ("CRLF row", format!("{}\r\n", direct_row(&direct))),
        ("bare CR in ref", format!("{}\r", direct_row(&direct))),
        ("SOH in ref", format!("{}\u{1}", direct_row(&direct))),
        (
            "record separator in ref",
            format!("{}\u{1e}", direct_row(&direct)),
        ),
        (
            "duplicate direct ref",
            format!("{}\n{}", direct_row(&direct), direct_row(&annotated_object)),
        ),
        (
            "duplicate peeled ref",
            format!(
                "{}\n{}\n{}",
                direct_row(&direct),
                peeled_row(&annotated_commit),
                peeled_row(&annotated_object)
            ),
        ),
        (
            "blank row",
            format!(
                "{}\n\n{}",
                direct_row(&direct),
                peeled_row(&annotated_commit)
            ),
        ),
        ("trailing blank row", format!("{}\n\n", direct_row(&direct))),
        ("annotated trailing blank row", format!("{annotated}\n")),
    ];
    for (name, refs) in malformed {
        let output = run_parser(&refs, "0", false);
        assert!(
            !output.status.success(),
            "{name} must fail closed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let output = run_parser(&direct_row(&direct), "1", false);
    assert!(
        !output.status.success(),
        "an unreadable remote must fail closed"
    );

    let output = run_parser(&direct_row(&direct), "0", true);
    assert!(
        !output.status.success()
            && String::from_utf8_lossy(&output.stderr).contains("returned a NUL byte"),
        "a NUL byte in Git output must fail closed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(unix)]
#[test]
fn release_entrypoints_reject_xtrace_before_setup() {
    use std::process::Command;

    let Some(root) = workspace_root() else {
        return;
    };
    for (name, relative_script) in [
        ("release preflight", "scripts/release-preflight.sh"),
        ("workspace publisher", "scripts/publish-workspace.sh"),
    ] {
        let output = Command::new("bash")
            .args(["-p", "-x"])
            .arg(root.join(relative_script))
            .arg("--check")
            .env_remove("BASH_ENV")
            .env_remove("CARGO_REGISTRY_TOKEN")
            .env_remove("PS4")
            .output()
            .expect("xtrace rejection subprocess should run");
        assert!(
            !output.status.success()
                && output.stdout.is_empty()
                && String::from_utf8_lossy(&output.stderr).contains("xtrace (-x) is unsupported"),
            "{name} must reject xtrace before setup\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[cfg(unix)]
#[test]
fn release_scripts_stabilize_relative_tool_overrides_before_chdir() {
    use std::ffi::OsString;
    use std::os::unix::fs::PermissionsExt;
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

    fn write_executable(path: &Path, source: &str) {
        std::fs::write(path, source).expect("wrapper source should be writable");
        let mut permissions = std::fs::metadata(path)
            .expect("wrapper metadata should be readable")
            .permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(path, permissions).expect("wrapper should become executable");
    }

    fn assert_success(name: &str, output: &Output) {
        assert!(
            output.status.success(),
            "{name} failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let Some(root) = workspace_root() else {
        return;
    };
    let root = root
        .canonicalize()
        .expect("workspace root should canonicalize");
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should follow the Unix epoch")
        .as_nanos();
    let test_directory = TestDirectory(std::env::temp_dir().join(format!(
        "pyo3-bindgen-release-relative-tools-{}-{nonce}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )));
    let tools = test_directory.0.join("tools");
    std::fs::create_dir_all(&tools).expect("wrapper directory should be creatable");
    let cargo_wrapper = tools.join("cargo-wrapper");
    let python_wrapper = tools.join("python-wrapper");
    let git_wrapper = tools.join("git-wrapper");
    let curl_wrapper = tools.join("curl-wrapper");
    let rustup_wrapper = tools.join("rustup-wrapper");
    let fixture_toolchain = test_directory.0.join("fixture-toolchain");
    let fixture_toolchain_bin = fixture_toolchain.join("bin");
    let bash_env = test_directory.0.join("bash-env");
    let bash_env_log = test_directory.0.join("bash-env.log");
    let cargo_log = test_directory.0.join("cargo-cwds.log");
    let python_log = test_directory.0.join("python-cwds.log");
    let git_log = test_directory.0.join("git-invocations.log");
    let curl_log = test_directory.0.join("curl-invocations.log");
    let token_log = test_directory.0.join("token-inheritance.log");
    let bash_function_log = test_directory.0.join("bash-function-inheritance.log");
    std::fs::create_dir_all(&fixture_toolchain_bin)
        .expect("fixture toolchain directory should be creatable");
    write_executable(
        &fixture_toolchain_bin.join("cargo"),
        "#!/usr/bin/env bash\nexit 0\n",
    );
    write_executable(
        &fixture_toolchain_bin.join("rustc"),
        "#!/usr/bin/env bash\n[[ \"${1:-}\" == --version ]] || exit 97\nprintf '%s\\n' 'rustc 1.2.3 (release test fixture)'\n",
    );
    write_executable(
        &rustup_wrapper,
        r#"#!/usr/bin/env bash
set -Eeuo pipefail
[[ "${RUSTUP_AUTO_INSTALL:-}" == 0 ]] || exit 98
if [[ "$#" == 3 && "$1" == toolchain && "$2" == list && "$3" == --verbose ]]; then
    printf '%s\n' "1.2.3-x86_64-unknown-linux-gnu ${RELEASE_TEST_TOOLCHAIN_ROOT:?}"
    exit 0
fi
exit 97
"#,
    );
    write_executable(
        &cargo_wrapper,
        r#"#!/usr/bin/env bash
set -Eeuo pipefail
printf '%s\n' "$PWD" >> "${RELEASE_TEST_CARGO_LOG:?}"
if [[ -n "${RELEASE_TEST_BASH_FUNCTION_LOG:-}" ]]; then
    if declare -F builtin >/dev/null; then
        printf '%s\n' present >> "${RELEASE_TEST_BASH_FUNCTION_LOG}"
    else
        printf '%s\n' absent >> "${RELEASE_TEST_BASH_FUNCTION_LOG}"
    fi
fi
if [[ -n "${RELEASE_TEST_TOKEN_LOG:-}" ]]; then
    if [[ -v CARGO_REGISTRY_TOKEN || -v RELEASE_REGISTRY_TOKEN ]]; then
        printf '%s\n' present >> "${RELEASE_TEST_TOKEN_LOG}"
    else
        printf '%s\n' absent >> "${RELEASE_TEST_TOKEN_LOG}"
    fi
fi
if [[ "${1:-}" == +* ]]; then
    shift
fi
exec "${RELEASE_TEST_REAL_CARGO:?}" "$@"
"#,
    );
    write_executable(
        &python_wrapper,
        r#"#!/usr/bin/env bash
set -Eeuo pipefail
printf '%s\n' "$PWD" >> "${RELEASE_TEST_PYTHON_LOG:?}"
if [[ -n "${RELEASE_TEST_BASH_FUNCTION_LOG:-}" ]]; then
    if declare -F builtin >/dev/null; then
        printf '%s\n' present >> "${RELEASE_TEST_BASH_FUNCTION_LOG}"
    else
        printf '%s\n' absent >> "${RELEASE_TEST_BASH_FUNCTION_LOG}"
    fi
fi
if [[ -n "${RELEASE_TEST_TOKEN_LOG:-}" ]]; then
    if [[ -v CARGO_REGISTRY_TOKEN || -v RELEASE_REGISTRY_TOKEN ]]; then
        printf '%s\n' present >> "${RELEASE_TEST_TOKEN_LOG}"
    else
        printf '%s\n' absent >> "${RELEASE_TEST_TOKEN_LOG}"
    fi
fi
exec "${RELEASE_TEST_REAL_PYTHON:?}" "$@"
"#,
    );
    write_executable(
        &git_wrapper,
        "#!/usr/bin/env bash\nprintf '%s\\n' invoked >> \"${RELEASE_TEST_GIT_LOG:?}\"\nexit 0\n",
    );
    write_executable(
        &curl_wrapper,
        "#!/usr/bin/env bash\nprintf '%s\\n' invoked >> \"${RELEASE_TEST_CURL_LOG:?}\"\nexit 0\n",
    );
    write_executable(
        &bash_env,
        r#"printf '%s\n' "${0:-}" >> "${RELEASE_TEST_BASH_ENV_LOG:?}"
case "${0:-}" in
    */release-preflight.sh|*/publish-workspace.sh)
        exit 97
        ;;
esac
"#,
    );

    // CI installs stable and MSRV through rustup aliases. Their bare `rustc` output is numeric,
    // but that numeric spelling need not be present in the local toolchain inventory. Keep this
    // relative-path regression independent of that host detail with a self-contained exact
    // toolchain fixture; the production scripts still enforce the exact-version contract.
    let rust_version = "1.2.3";

    let mut python_candidates = Vec::<OsString>::new();
    if let Some(configured) = std::env::var_os("PYO3_PYTHON") {
        python_candidates.push(configured);
    }
    python_candidates.push(OsString::from("python3"));
    let (real_python, python_version) = python_candidates
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
        .expect("release script regression needs a shared local CPython");

    let real_cargo = PathBuf::from(env!("CARGO"));
    let real_cargo = if real_cargo.is_absolute() {
        real_cargo
    } else {
        std::env::current_dir()
            .expect("test process should have a current directory")
            .join(real_cargo)
    };
    let rejected_pyo3_overrides = [
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
    ];
    let command = |script: &Path, cargo_bin: &str, python_bin: &str| {
        let mut command = Command::new(script);
        command
            .arg("--check")
            .current_dir(&test_directory.0)
            .env("CARGO_BIN", cargo_bin)
            .env("PYTHON_BIN", python_bin)
            .env_remove("GIT_BIN")
            .env("RUSTUP_BIN", "./tools/rustup-wrapper")
            .env_remove("CURL_BIN")
            .env("RELEASE_RUST_TOOLCHAIN", rust_version)
            .env("RELEASE_PYTHON_VERSION", &python_version)
            .env("RELEASE_TEST_TOOLCHAIN_ROOT", &fixture_toolchain)
            .env("RELEASE_TEST_REAL_CARGO", &real_cargo)
            .env("RELEASE_TEST_REAL_PYTHON", &real_python)
            .env("RELEASE_TEST_CARGO_LOG", &cargo_log)
            .env("RELEASE_TEST_PYTHON_LOG", &python_log)
            .env("RELEASE_TEST_BASH_FUNCTION_LOG", &bash_function_log)
            .env("RUSTUP_AUTO_INSTALL", "1")
            .env_remove("CARGO_REGISTRY_TOKEN");
        for variable in rejected_pyo3_overrides {
            command.env_remove(variable);
        }
        command
    };

    let preflight = root.join("scripts/release-preflight.sh");
    let publisher = root.join("scripts/publish-workspace.sh");
    let relative_cargo = "./tools/cargo-wrapper";
    let relative_python = "./tools/python-wrapper";
    let mut relative_preflight = command(&preflight, relative_cargo, relative_python);
    relative_preflight
        .env("GIT_BIN", "./tools/git-wrapper")
        .env("RELEASE_TEST_GIT_LOG", &git_log);
    let output = relative_preflight
        .output()
        .expect("relative-wrapper preflight should run");
    assert_success("release preflight with relative wrappers", &output);
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("check OK"),
        "preflight should reach its post-chdir check result"
    );

    let mut relative_publisher = command(&publisher, relative_cargo, relative_python);
    relative_publisher
        .env("GIT_BIN", "./tools/git-wrapper")
        .env("CURL_BIN", "./tools/curl-wrapper")
        .env("RELEASE_TEST_GIT_LOG", &git_log)
        .env("RELEASE_TEST_CURL_LOG", &curl_log);
    let output = relative_publisher
        .output()
        .expect("relative-wrapper publisher should run");
    assert_success("workspace publisher with relative wrappers", &output);
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("check OK"),
        "publisher and its inherited preflight should reach their check results"
    );
    assert!(
        !git_log.exists() && !curl_log.exists(),
        "check modes must resolve Git/Curl helpers without invoking either"
    );

    for (name, script) in [
        ("release preflight", &preflight),
        ("workspace publisher", &publisher),
    ] {
        let mut hostile_startup = command(script, relative_cargo, relative_python);
        hostile_startup
            .env("BASH_ENV", &bash_env)
            .env("BASH_FUNC_builtin%%", "() { return 97; }")
            .env("RELEASE_TEST_BASH_ENV_LOG", &bash_env_log);
        let output = hostile_startup
            .output()
            .expect("hostile Bash startup check should run");
        assert_success(
            &format!("{name} with BASH_ENV and BASH_FUNC_builtin"),
            &output,
        );
        assert!(
            !bash_env_log.exists(),
            "{name} should ignore BASH_ENV in privileged mode"
        );
    }

    for (name, script) in [
        ("release preflight", &preflight),
        ("workspace publisher", &publisher),
    ] {
        let mut repeated_poison = command(script, relative_cargo, relative_python);
        repeated_poison
            .env("RELEASE_ENV_SANITIZED", "1")
            .env("BASH_FUNC_builtin%%", "() { return 97; }");
        let output = repeated_poison
            .output()
            .expect("repeated Bash startup poison check should run");
        assert!(
            !output.status.success()
                && String::from_utf8_lossy(&output.stderr)
                    .contains("could not sanitize inherited Bash environment"),
            "{name} should fail closed after one environment scrub attempt\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    for (name, script) in [
        ("release preflight", &preflight),
        ("workspace publisher", &publisher),
    ] {
        let mut token_probe = command(script, relative_cargo, relative_python);
        token_probe
            .env("CARGO_REGISTRY_TOKEN", "test-token")
            .env("RELEASE_REGISTRY_TOKEN", "attacker-exported-token")
            .env("BASH_ENV", &bash_env)
            .env("BASH_FUNC_builtin%%", "() { return 97; }")
            .env("RELEASE_TEST_BASH_ENV_LOG", &bash_env_log)
            .env("RELEASE_TEST_TOKEN_LOG", &token_log);
        let output = token_probe
            .output()
            .expect("token inheritance check should run");
        assert_success(&format!("{name} with an inherited token"), &output);
    }
    let token_log_contents =
        std::fs::read_to_string(&token_log).expect("token inheritance log should exist");
    let token_states = token_log_contents.lines().collect::<Vec<_>>();
    assert!(
        !token_states.is_empty() && token_states.iter().all(|state| *state == "absent"),
        "check-mode helper processes must not inherit release tokens: {token_states:?}"
    );
    assert!(
        !bash_env_log.exists(),
        "token checks must keep BASH_ENV out of child Bash helpers"
    );
    let function_log_contents = std::fs::read_to_string(&bash_function_log)
        .expect("child Bash function inheritance log should exist");
    let function_states = function_log_contents.lines().collect::<Vec<_>>();
    assert!(
        !function_states.is_empty() && function_states.iter().all(|state| *state == "absent"),
        "child Bash wrappers must not import BASH_FUNC_* exports: {function_states:?}"
    );

    let missing_rustup = tools.join("missing-rustup");
    let cargo_probe = tools.join("cargo-probe");
    let cargo_probe_log = test_directory.0.join("cargo-probe.log");
    write_executable(
        &missing_rustup,
        "#!/usr/bin/env bash\n[[ \"${RUSTUP_AUTO_INSTALL:-}\" == 0 ]] || exit 98\nprintf '%s\\n' '1.2.3-x86_64-unknown-linux-gnu /tmp/not-installed'\n",
    );
    write_executable(
        &cargo_probe,
        r#"#!/usr/bin/env bash
printf '%s\n' invoked >> "${RELEASE_TEST_CARGO_PROBE:?}"
exit 99
"#,
    );
    let cargo_probe = cargo_probe
        .to_str()
        .expect("cargo probe path should be UTF-8");
    let missing_rustup = missing_rustup
        .to_str()
        .expect("missing rustup path should be UTF-8");
    for (name, script) in [
        ("release preflight", &preflight),
        ("workspace publisher", &publisher),
    ] {
        let mut missing_toolchain = command(script, cargo_probe, relative_python);
        missing_toolchain
            .env("RUSTUP_BIN", missing_rustup)
            .env("RELEASE_TEST_CARGO_PROBE", &cargo_probe_log);
        let output = missing_toolchain
            .output()
            .expect("missing-toolchain check should run");
        assert!(
            !output.status.success()
                && String::from_utf8_lossy(&output.stderr).contains("not installed locally"),
            "{name} should reject an unavailable Rust toolchain before Cargo\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            !cargo_probe_log.exists(),
            "{name} must not invoke Cargo after local toolchain validation fails"
        );
    }

    let original_path = std::env::var_os("PATH").unwrap_or_default();
    let mut lookup_paths = vec![tools.clone()];
    lookup_paths.extend(std::env::split_paths(&original_path));
    let lookup_path =
        std::env::join_paths(lookup_paths).expect("wrapper PATH should be representable");
    let mut bare_lookup = command(&preflight, "cargo-wrapper", "python-wrapper");
    bare_lookup.env("PATH", lookup_path);
    let output = bare_lookup
        .output()
        .expect("bare PATH-wrapper preflight should run");
    assert_success("release preflight with bare PATH wrappers", &output);

    let not_executable = tools.join("not-executable");
    std::fs::write(&not_executable, "#!/usr/bin/env bash\nexit 0\n")
        .expect("non-executable fixture should be writable");
    let output = command(&preflight, "./tools/not-executable", relative_python)
        .output()
        .expect("non-executable override check should run");
    assert!(
        !output.status.success()
            && String::from_utf8_lossy(&output.stderr).contains("not an executable file"),
        "non-executable override must fail closed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let mut function_override = command(&preflight, "release_test_cargo_function", relative_python);
    function_override.env(
        "BASH_FUNC_release_test_cargo_function%%",
        "() { return 0; }",
    );
    let output = function_override
        .output()
        .expect("function-only override check should run");
    assert!(
        !output.status.success()
            && String::from_utf8_lossy(&output.stderr)
                .contains("required executable not found in PATH"),
        "function-only override must fail closed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let mut builtin_type_override = command(&preflight, relative_cargo, relative_python);
    builtin_type_override.env("BASH_FUNC_type%%", "() { return 1; }");
    let output = builtin_type_override
        .output()
        .expect("builtin type override check should run");
    assert_success("release preflight with a shadowed type function", &output);

    let mut git_function_override = command(&preflight, relative_cargo, relative_python);
    git_function_override
        .env("GIT_BIN", "release_test_git_function")
        .env("BASH_FUNC_release_test_git_function%%", "() { return 0; }");
    let output = git_function_override
        .output()
        .expect("function-only Git override check should run");
    assert!(
        !output.status.success()
            && String::from_utf8_lossy(&output.stderr)
                .contains("required executable not found in PATH"),
        "function-only Git override must fail closed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let mut curl_function_override = command(&publisher, relative_cargo, relative_python);
    curl_function_override
        .env("CURL_BIN", "release_test_curl_function")
        .env("BASH_FUNC_release_test_curl_function%%", "() { return 0; }");
    let output = curl_function_override
        .output()
        .expect("function-only curl override check should run");
    assert!(
        !output.status.success()
            && String::from_utf8_lossy(&output.stderr)
                .contains("required executable not found in PATH"),
        "function-only curl override must fail closed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let cargo_cwds = std::fs::read_to_string(&cargo_log).expect("cargo wrapper log should exist");
    let cargo_cwds = cargo_cwds.lines().collect::<Vec<_>>();
    assert!(
        cargo_cwds.len() >= 4
            && cargo_cwds
                .iter()
                .all(|directory| Path::new(directory) == root),
        "every Cargo wrapper invocation should survive the scripts' chdir: {cargo_cwds:?}"
    );
    let python_cwds =
        std::fs::read_to_string(&python_log).expect("Python wrapper log should exist");
    assert!(
        python_cwds
            .lines()
            .any(|directory| Path::new(directory) == root),
        "Python wrapper should be reusable after chdir: {python_cwds:?}"
    );
}

#[cfg(unix)]
#[test]
fn rust_setup_prefers_rustup_proxies_over_hostile_path_entries() {
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory(PathBuf);

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn write_executable(path: &Path, source: &str) {
        std::fs::write(path, source).expect("Rust setup fixture should be writable");
        let mut permissions = std::fs::metadata(path)
            .expect("Rust setup fixture metadata should be readable")
            .permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(path, permissions)
            .expect("Rust setup fixture should become executable");
    }

    let Some(root) = workspace_root() else {
        return;
    };
    let action = std::fs::read_to_string(root.join(".github/actions/setup-rust/action.yml"))
        .expect("Rust setup composite action should exist");
    assert_actions_are_sha_pinned("setup-rust", &action);
    for required in [
        "using: composite",
        "uses: dtolnay/rust-toolchain@2c7215f132e9ebf062739d9130488b56d53c060c",
        "cargo_home=\"${CARGO_HOME:-$HOME/.cargo}\"",
        "for proxy in cargo rustc rustup; do",
        "for proxy in rustc rustdoc cargo rust-lldb rust-gdb rust-gdbgui rls cargo-clippy clippy-driver cargo-miri rust-analyzer rustfmt cargo-fmt; do",
        "CARGO_HOME must be absolute",
        "export PATH=\"${rustup_bin}:${PATH}\"",
        "export RUSTUP_TOOLCHAIN=\"${RUST_TOOLCHAIN}\"",
        "printf '%s\\n' \"${rustup_bin}\" >> \"${GITHUB_PATH}\"",
        "printf 'RUSTUP_TOOLCHAIN=%s\\n' \"${RUST_TOOLCHAIN}\" >> \"${GITHUB_ENV}\"",
        "rustup which cargo --toolchain \"${RUST_TOOLCHAIN}\"",
        "rustup which rustc --toolchain \"${RUST_TOOLCHAIN}\"",
        "rustup run \"${RUST_TOOLCHAIN}\" cargo --version",
        "rustup run \"${RUST_TOOLCHAIN}\" rustc --version",
        "type -P rustup",
        "ensure_proxy \"${rustup_path}\" \"${rustup_bin}/rustup\"",
        "ln -s \"${source}\" \"${destination}\"",
    ] {
        assert!(
            action.contains(required),
            "Rust setup action should contain {required}"
        );
    }
    assert!(
        !action.contains("1.97") && !action.contains("1.98"),
        "Rust setup sanity checks must derive versions from the selected rustup toolchain"
    );
    let rust_workflow = std::fs::read_to_string(root.join(".github/workflows/rust.yml"))
        .expect("Rust workflow should exist");
    assert_eq!(
        rust_workflow
            .matches("uses: ./.github/actions/setup-rust")
            .count(),
        14,
        "every Rust job and selected cargo matrix lane should use the shared setup action"
    );
    assert!(
        !rust_workflow.contains("uses: dtolnay/rust-toolchain@"),
        "Rust workflow must not bypass the shared Rust setup action"
    );

    fn run_block(action: &str, step_name: &str) -> String {
        let marker = format!("    - name: {step_name}\n");
        let step = action
            .split_once(&marker)
            .map(|(_, remainder)| remainder)
            .unwrap_or_else(|| panic!("Rust setup action should contain {step_name}"));
        let script = step
            .split_once("      run: |\n")
            .map(|(_, remainder)| remainder)
            .unwrap_or_else(|| panic!("Rust setup {step_name} should contain a run block"));
        let script = script
            .split_once("\n    - ")
            .map_or(script, |(script, _)| script);
        script
            .lines()
            .map(|line| {
                if line.is_empty() {
                    ""
                } else {
                    line.strip_prefix("        ").unwrap_or_else(|| {
                        panic!("Rust setup run block line should be indented: {line:?}")
                    })
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    let bootstrap = run_block(&action, "Prefer rustup-managed Rust proxies");
    let repair = run_block(&action, "Repair missing rustup proxies");
    let verify = run_block(&action, "Verify selected Rust toolchain");
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should follow the Unix epoch")
        .as_nanos();
    let root_path = std::env::temp_dir().join(format!(
        "pyo3-bindgen-rust-setup-{}-{nonce}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir(&root_path).expect("Rust setup fixture root should be creatable");
    let test_directory = TestDirectory(root_path);
    let hostile_bin = test_directory.0.join("hostile-bin");
    let global_bin = test_directory.0.join("global-bin");
    let cargo_home = test_directory.0.join("cargo-home");
    let rustup_bin = cargo_home.join("bin");
    let global_rustup = global_bin.join("rustup");
    let selected_bin = cargo_home.join("toolchains/fixture/bin");
    let github_path = test_directory.0.join("github-path");
    let github_env = test_directory.0.join("github-env");
    let hostile_marker = test_directory.0.join("hostile-invoked");
    std::fs::create_dir(&hostile_bin).expect("hostile PATH directory should be creatable");
    std::fs::create_dir(&global_bin).expect("global Rust bin directory should be creatable");
    assert!(
        !rustup_bin.exists(),
        "Rust setup should begin with no installed rustup proxy directory"
    );

    let cargo_version = "cargo 9.9.9 (selected fixture)";
    let rustc_version = "rustc 9.9.9 (selected fixture)";
    let hostile_tool = format!(
        "#!/bin/sh\nprintf '%s\\n' called >> \"{}\"\nexit 97\n",
        hostile_marker.display()
    );
    write_executable(&hostile_bin.join("cargo"), &hostile_tool);
    write_executable(&hostile_bin.join("rustc"), &hostile_tool);
    let selected_cargo = selected_bin.join("cargo");
    let selected_rustc = selected_bin.join("rustc");

    let original_path = std::env::var_os("PATH").expect("PATH should be available");
    let hostile_path = format!(
        "{}:{}:{}",
        global_bin.display(),
        hostile_bin.display(),
        original_path.to_string_lossy()
    );
    let run = |script: &str, cargo_home: &Path, path: &str| {
        Command::new("bash")
            .arg("-c")
            .arg(format!("set -euo pipefail\n{script}"))
            .env("CARGO_HOME", cargo_home)
            .env("PATH", path)
            .env("GITHUB_PATH", &github_path)
            .env("GITHUB_ENV", &github_env)
            .env("RUST_TOOLCHAIN", "fixture")
            .output()
            .expect("Rust setup fixture should start")
    };

    let bootstrap_output = run(&bootstrap, &cargo_home, &hostile_path);
    assert!(
        bootstrap_output.status.success(),
        "Rust setup bootstrap should allow an absent proxy directory before installation\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&bootstrap_output.stdout),
        String::from_utf8_lossy(&bootstrap_output.stderr)
    );
    for proxy in ["cargo", "rustc", "rustup"] {
        assert!(
            !rustup_bin.join(proxy).exists(),
            "pre-install setup must not require or fabricate the {proxy} proxy"
        );
    }
    assert_eq!(
        std::fs::read_to_string(&github_path)
            .expect("bootstrap should publish the future proxy path"),
        format!("{}\n", rustup_bin.display()),
        "clean-runner bootstrap must publish the path before the installer runs"
    );
    assert!(
        !github_env.exists(),
        "bootstrap must not persist a toolchain before post-install validation"
    );

    // Simulate the pinned installer finding an existing rustup and installing the toolchain
    // without creating proxies in the configured Cargo home.
    std::fs::create_dir_all(&selected_bin)
        .expect("selected toolchain directory should be creatable");
    write_executable(
        &selected_cargo,
        &format!(
            "#!/bin/sh\n[ \"${{RUSTUP_TOOLCHAIN:-}}\" = fixture ] || exit 96\nprintf '%s\\n' '{cargo_version}'\n"
        ),
    );
    write_executable(
        &selected_rustc,
        &format!(
            "#!/bin/sh\n[ \"${{RUSTUP_TOOLCHAIN:-}}\" = fixture ] || exit 96\nprintf '%s\\n' '{rustc_version}'\n"
        ),
    );
    let rustup_script = format!(
        "#!/bin/sh\nset -eu\ncase \"${{0##*/}}\" in\n  cargo)\n    [ \"${{RUSTUP_TOOLCHAIN:-}}\" = fixture ] || exit 96\n    printf '%s\\n' '{cargo_version}'\n    ;;\n  rustc)\n    [ \"${{RUSTUP_TOOLCHAIN:-}}\" = fixture ] || exit 96\n    printf '%s\\n' '{rustc_version}'\n    ;;\n  rustup)\n    case \"${{1:-}}\" in\n      which)\n        [ \"${{3:-}}\" = --toolchain ] && [ \"${{4:-}}\" = fixture ] || exit 3\n        case \"${{2:-}}\" in\n          cargo) printf '%s\\n' '{cargo}' ;;\n          rustc) printf '%s\\n' '{rustc}' ;;\n          *) exit 2 ;;\n        esac\n        ;;\n      run)\n        [ \"${{2:-}}\" = fixture ] || exit 3\n        case \"${{3:-}}\" in\n          cargo) printf '%s\\n' '{cargo_version}' ;;\n          rustc) printf '%s\\n' '{rustc_version}' ;;\n          *) exit 2 ;;\n        esac\n        ;;\n      *) exit 2 ;;\n    esac\n    ;;\n  *) exit 2 ;;\nesac\n",
        cargo_version = cargo_version,
        rustc_version = rustc_version,
        cargo = selected_cargo.display(),
        rustc = selected_rustc.display(),
    );
    write_executable(&global_rustup, &rustup_script);

    let missing_proxy_before_repair = run(&verify, &cargo_home, &hostile_path);
    assert!(
        !missing_proxy_before_repair.status.success()
            && String::from_utf8_lossy(&missing_proxy_before_repair.stderr)
                .contains("missing rustup proxy"),
        "strict Rust setup verification should reproduce the installed-rustup proxy failure before repair\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&missing_proxy_before_repair.stdout),
        String::from_utf8_lossy(&missing_proxy_before_repair.stderr)
    );
    let repair_output = run(&repair, &cargo_home, &hostile_path);
    assert!(
        repair_output.status.success(),
        "Rust setup should repair proxies when an existing rustup skipped bootstrap\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&repair_output.stdout),
        String::from_utf8_lossy(&repair_output.stderr)
    );
    for proxy in [
        "cargo",
        "rustc",
        "rustup",
        "rustdoc",
        "rust-lldb",
        "rust-gdb",
        "rust-gdbgui",
        "rls",
        "cargo-clippy",
        "clippy-driver",
        "cargo-miri",
        "rustfmt",
        "cargo-fmt",
        "rust-analyzer",
    ] {
        assert!(
            rustup_bin.join(proxy).is_file(),
            "Rust setup should materialize the {proxy} rustup proxy"
        );
    }

    // Keep the pre-existing-proxy path covered as well.
    std::fs::write(&github_path, "").expect("Rust setup path fixture should be resettable");
    let preexisting_bootstrap = run(&bootstrap, &cargo_home, &hostile_path);
    assert!(
        preexisting_bootstrap.status.success(),
        "Rust setup bootstrap should accept pre-existing proxies\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&preexisting_bootstrap.stdout),
        String::from_utf8_lossy(&preexisting_bootstrap.stderr)
    );
    let published_path = std::fs::read_to_string(&github_path)
        .expect("Rust setup should write GITHUB_PATH")
        .trim()
        .to_owned();
    assert_eq!(
        published_path,
        rustup_bin.to_string_lossy(),
        "Rust setup should publish the absolute rustup proxy directory"
    );
    let selected_path = format!("{published_path}:{hostile_path}");
    let verify_output = run(&verify, &cargo_home, &selected_path);
    assert!(
        verify_output.status.success(),
        "Rust setup verification failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&verify_output.stdout),
        String::from_utf8_lossy(&verify_output.stderr)
    );
    let persisted_env =
        std::fs::read_to_string(&github_env).expect("Rust setup should write GITHUB_ENV");
    let persisted_toolchain = persisted_env
        .lines()
        .find_map(|line| line.strip_prefix("RUSTUP_TOOLCHAIN="))
        .expect("Rust setup should persist the selected toolchain");
    assert_eq!(persisted_toolchain, "fixture");
    let nested_output = Command::new("bash")
        .arg("-c")
        .arg("set -euo pipefail; test \"$(type -P cargo)\" = \"$EXPECTED_CARGO\"; cargo --version")
        .env("PATH", &selected_path)
        .env("RUSTUP_TOOLCHAIN", persisted_toolchain)
        .env("EXPECTED_CARGO", rustup_bin.join("cargo"))
        .output()
        .expect("nested Rust setup command should start");
    assert!(
        nested_output.status.success(),
        "nested Rust setup command should use the persisted proxy path\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&nested_output.stdout),
        String::from_utf8_lossy(&nested_output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&nested_output.stdout),
        format!("{cargo_version}\n"),
        "nested Rust setup command should use the selected Cargo version"
    );
    assert!(
        !hostile_marker.exists(),
        "system cargo/rustc must not execute after rustup bin is prepended"
    );

    let github_env_before_failure = std::fs::read_to_string(&github_env)
        .expect("Rust setup should persist GITHUB_ENV after successful verification");
    std::fs::remove_file(rustup_bin.join("cargo")).expect("cargo proxy should be removable");
    write_executable(
        &rustup_bin.join("cargo"),
        "#!/bin/sh\nprintf '%s\\n' 'cargo 8.8.8 (wrong proxy)'\n",
    );
    let preserve_wrong_proxy = run(&repair, &cargo_home, &selected_path);
    assert!(
        preserve_wrong_proxy.status.success(),
        "Rust setup repair should preserve an existing executable proxy\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&preserve_wrong_proxy.stdout),
        String::from_utf8_lossy(&preserve_wrong_proxy.stderr)
    );
    let wrong_version = run(&verify, &cargo_home, &selected_path);
    assert!(
        !wrong_version.status.success()
            && String::from_utf8_lossy(&wrong_version.stderr)
                .contains("cargo proxy selected the wrong toolchain"),
        "Rust setup must reject a proxy version mismatch\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&wrong_version.stdout),
        String::from_utf8_lossy(&wrong_version.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(&github_env)
            .expect("Rust setup should retain GITHUB_ENV after failed verification"),
        github_env_before_failure,
        "Rust setup must persist RUSTUP_TOOLCHAIN only after version validation"
    );

    std::fs::remove_file(rustup_bin.join("cargo")).expect("cargo proxy should be replaceable");
    write_executable(
        &rustup_bin.join("cargo"),
        &format!(
            "#!/bin/sh\n[ \"${{RUSTUP_TOOLCHAIN:-}}\" = fixture ] || exit 96\nprintf '%s\\n' '{cargo_version}'\n"
        ),
    );
    std::fs::remove_file(rustup_bin.join("rustc")).expect("rustc proxy should be removable");
    let missing_proxy = run(&verify, &cargo_home, &selected_path);
    assert!(
        !missing_proxy.status.success()
            && String::from_utf8_lossy(&missing_proxy.stderr).contains("missing rustup proxy"),
        "Rust setup must reject a missing post-install proxy\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&missing_proxy.stdout),
        String::from_utf8_lossy(&missing_proxy.stderr)
    );
    let relative_home = run(&bootstrap, Path::new("relative-cargo-home"), &hostile_path);
    assert!(
        !relative_home.status.success()
            && String::from_utf8_lossy(&relative_home.stderr)
                .contains("CARGO_HOME must be absolute"),
        "Rust setup must reject a relative CARGO_HOME\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&relative_home.stdout),
        String::from_utf8_lossy(&relative_home.stderr)
    );
}

#[test]
fn release_workflows_are_pinned_read_only_and_fail_closed() {
    let Some(root) = workspace_root() else {
        return;
    };
    let rust_workflow = std::fs::read_to_string(root.join(".github/workflows/rust.yml"))
        .expect("rust workflow should exist");
    let embedded_python_action =
        std::fs::read_to_string(root.join(".github/actions/setup-embedded-python/action.yml"))
            .expect("embedded Python composite action should exist");
    let pre_commit_workflow =
        std::fs::read_to_string(root.join(".github/workflows/pre-commit.yml"))
            .expect("pre-commit workflow should exist");
    let security_workflow = std::fs::read_to_string(root.join(".github/workflows/security.yml"))
        .expect("scheduled security workflow should exist");
    let cargo_deny_workflow =
        std::fs::read_to_string(root.join(".github/workflows/cargo-deny.yml"))
            .expect("reusable cargo-deny workflow should exist");
    let publish_workflow = std::fs::read_to_string(root.join(".github/workflows/publish.yml"))
        .expect("manual publish workflow should exist");
    let checklist = std::fs::read_to_string(root.join("docs/release_checklist.md"))
        .expect("docs/release_checklist.md should exist");
    let normalized_checklist = normalize_whitespace(&checklist);
    let security_policy = std::fs::read_to_string(root.join("SECURITY.md"))
        .expect("repository security policy should exist");
    let deny_config =
        std::fs::read_to_string(root.join("deny.toml")).expect("cargo-deny config should exist");
    let pre_commit_config = std::fs::read_to_string(root.join(".pre-commit-config.yaml"))
        .expect("pre-commit configuration should exist");
    let dependabot_config = std::fs::read_to_string(root.join(".github/dependabot.yml"))
        .expect("Dependabot configuration should exist");
    let release_preflight = std::fs::read_to_string(root.join("scripts/release-preflight.sh"))
        .expect("release preflight should exist");
    let workspace_publisher = std::fs::read_to_string(root.join("scripts/publish-workspace.sh"))
        .expect("workspace publisher should exist");
    let release_common = std::fs::read_to_string(root.join("scripts/release-common.sh"))
        .expect("shared release policy should exist");

    for (name, script) in [
        ("release preflight", release_preflight.as_str()),
        ("workspace publisher", workspace_publisher.as_str()),
    ] {
        assert!(
            script.contains("source \"${SCRIPT_DIR}/release-common.sh\""),
            "{name} should source the shared release policy"
        );
        assert!(
            !script.contains("resolve_executable()")
                && !script.contains("reject_pyo3_environment_overrides()")
                && !script.contains("metadata = json.load(sys.stdin)"),
            "{name} should not duplicate shared release-policy implementations"
        );
    }
    assert!(
        release_preflight.starts_with("#!/bin/bash -p\ncase $- in\n")
            && workspace_publisher.starts_with("#!/bin/bash -p\ncase $- in\n")
            && release_common.contains("release_common_require_privileged_mode")
            && release_common.contains("release_common_sanitize_environment")
            && release_common.contains("/usr/bin/env -0")
            && release_common.contains("BASH_FUNC_*")
            && release_preflight.contains(
                "release_common_sanitize_environment \"${SCRIPT_DIR}/${SCRIPT_SOURCE##*/}\" \"$@\""
            )
            && workspace_publisher.contains(
                "release_common_sanitize_environment \"${SCRIPT_DIR}/${SCRIPT_SOURCE##*/}\" \"$@\""
            ),
        "release entrypoints must scrub Bash startup exports before child helpers"
    );
    assert!(
        release_common.contains("metadata = json.load(sys.stdin)")
            && release_preflight.contains(
                "release_common_workspace_metadata \"$CARGO_BIN\" \"$PYTHON_BIN\" \"$RUST_TOOLCHAIN\" sorted \"$RUSTUP_BIN\""
            )
            && workspace_publisher.contains(
                "release_common_workspace_metadata \"$CARGO_BIN\" \"$PYTHON_BIN\" \"$RUST_TOOLCHAIN\" release \"$RUSTUP_BIN\""
            ),
        "both release entrypoints should use the shared manifest validator"
    );
    assert!(
        !release_common.contains("awk")
            && !workspace_publisher.contains("awk")
            && release_common.contains("while IFS= read -r line")
            && release_common.contains("duplicate direct refs")
            && release_common.contains("duplicate peeled refs")
            && release_common.contains("unexpected ref")
            && release_common.contains("invalid SHA row")
            && release_common.contains("${sha,,}"),
        "remote tag resolution must be strict pure Bash with Bash lowercase conversion"
    );
    assert!(
        release_common.contains("builtin type -P")
            && workspace_publisher.contains("builtin type -P -- \"$checksum_tool\"")
            && workspace_publisher
                .contains("release_common_resolve_executable \"$checksum_candidate\" SHA256_BIN")
            && workspace_publisher.contains("\"$SHA256_BIN\"")
            && workspace_publisher.contains("${checksum,,}")
            && !workspace_publisher.contains("| tr "),
        "checksum helper must be resolved absolutely and invoked through its stored path"
    );
    assert!(
        release_common.contains(
            "\"$cargo_bin\" \"+${rust_toolchain}\" metadata --format-version 1 --no-deps --locked --offline"
        ),
        "shared workspace metadata must use Cargo --offline for both check modes"
    );
    assert!(
        release_common.contains("release_common_validate_local_rust_toolchain")
            && release_common.contains("toolchain list --verbose")
            && release_preflight.contains("RUSTUP_BIN")
            && workspace_publisher.contains("RUSTUP_BIN")
            && release_preflight.contains("export RUSTUP_AUTO_INSTALL=0")
            && workspace_publisher.contains("export RUSTUP_AUTO_INSTALL=0"),
        "both release check modes must prove the exact local Rust toolchain before Cargo"
    );

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
        assert!(
            release_common.contains(variable),
            "shared release policy must reject the PyO3 override {variable}"
        );
    }

    for (name, workflow) in [
        ("Rust", rust_workflow.as_str()),
        ("pre-commit", pre_commit_workflow.as_str()),
        ("security", security_workflow.as_str()),
        ("publish", publish_workflow.as_str()),
        ("cargo-deny", cargo_deny_workflow.as_str()),
    ] {
        assert_actions_are_sha_pinned(name, workflow);
        assert_repository_permissions_are_read_only(name, workflow);
        assert!(
            workflow.contains("permissions:") && workflow.contains("contents: read"),
            "{name} workflow should default to read-only repository access"
        );
        assert!(
            !workflow.contains("persist-credentials: true"),
            "{name} workflow must not persist checkout credentials"
        );
        assert!(
            !workflow.contains("pull_request_target")
                && !workflow.to_lowercase().contains("auto-merge")
                && !workflow.contains("dependabot[bot]"),
            "{name} workflow must not grant Dependabot updates a review-bypassing auto-merge path"
        );
    }
    for (name, workflow) in [
        ("Rust", rust_workflow.as_str()),
        ("pre-commit", pre_commit_workflow.as_str()),
        ("publish", publish_workflow.as_str()),
        ("cargo-deny", cargo_deny_workflow.as_str()),
    ] {
        assert_checkouts_do_not_persist_credentials(name, workflow);
    }
    assert!(
        security_workflow.contains("workflow_call:")
            && security_workflow.contains("uses: ./.github/workflows/cargo-deny.yml")
            && !security_workflow.contains("- uses: actions/checkout@"),
        "security workflow should be validated as a local reusable caller"
    );
    assert!(
        cargo_deny_workflow.contains("workflow_call:"),
        "cargo-deny workflow should be reusable"
    );
    assert_actions_are_sha_pinned("setup-embedded-python", &embedded_python_action);
    let normalized_embedded_python_action = normalize_whitespace(&embedded_python_action);
    assert!(
        normalized_embedded_python_action.contains("using: composite")
            && normalized_embedded_python_action
                .contains("uses: astral-sh/setup-uv@c771a70e6277c0a99b617c7a806ffedaca235ff9",)
            && normalized_embedded_python_action.contains("version: \"0.12.10\""),
        "embedded Python composite action must use the pinned setup-uv 0.12.10 dependency"
    );
    for required in [
        "mktemp -d \"${RUNNER_TEMP}/embedded-python.XXXXXX\"",
        "export UV_PYTHON_INSTALL_DIR=\"${python_root}/interpreters\"",
        "uv python install --managed-python --no-bin",
        "printf '%s\\n' \"${python_root}/venv/bin\" >> \"${GITHUB_PATH}\"",
        "sysconfig.get_config_var(\"Py_GIL_DISABLED\")",
        "sysconfig.get_config_var(\"Py_ENABLE_SHARED\") == 1",
        "ctypes.CDLL(str(library), mode=ctypes.RTLD_GLOBAL)",
        "python_libdir",
        "export LD_LIBRARY_PATH=\"${python_libdir}",
        "printf 'PYO3_PYTHON=%s\\nLD_LIBRARY_PATH=%s\\n'",
    ] {
        assert!(
            embedded_python_action.contains(required),
            "embedded Python composite action should contain {required}"
        );
    }
    assert!(
        !embedded_python_action.contains("/opt/"),
        "embedded Python composite action must not hard-code an /opt interpreter path"
    );
    let cpython_jobs = [
        "cargo",
        "platform-runtime",
        "compile-sweep",
        "native-platforms",
        "python-compatibility",
        "abi3",
    ];
    for job_name in cpython_jobs {
        let job = workflow_job(&rust_workflow, job_name);
        assert_eq!(
            job.matches("uses: ./.github/actions/setup-embedded-python")
                .count(),
            1,
            "Rust {job_name} should use the private embedded Python composite action exactly once"
        );
        assert!(
            !job.contains("uses: actions/setup-python@"),
            "Rust {job_name} should not use the hosted setup-python action"
        );
    }
    let pypy_job = workflow_job(&rust_workflow, "pypy-external-ir");
    assert!(
        pypy_job.contains("uses: actions/setup-python@5fda3b95a4ea91299a34e894583c3862153e4b97")
            && pypy_job.contains("python-version: \"pypy3.11\"")
            && !pypy_job.contains("./.github/actions/setup-embedded-python"),
        "PyPy must retain the pinned hosted setup-python action"
    );
    assert_eq!(
        rust_workflow.matches("uses: actions/setup-python@").count(),
        1,
        "Rust workflow should retain hosted setup-python only for PyPy"
    );
    assert!(
        pre_commit_workflow
            .contains("uses: actions/setup-python@5fda3b95a4ea91299a34e894583c3862153e4b97")
            && !pre_commit_workflow.contains("./.github/actions/setup-embedded-python"),
        "pre-commit must retain the pinned hosted setup-python action"
    );
    for job_name in [
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
    ] {
        assert_cache_follows_toolchain(job_name, workflow_job(&rust_workflow, job_name));
    }
    let cargo_deny_job = workflow_job(&cargo_deny_workflow, "cargo-deny");
    let normalized_cargo_deny_job = normalize_whitespace(cargo_deny_job);
    assert!(
        cargo_deny_job
            .contains("uses: dtolnay/rust-toolchain@2c7215f132e9ebf062739d9130488b56d53c060c\n")
            && cargo_deny_job.contains("          toolchain: stable\n"),
        "cargo-deny must provision the stable Cargo toolchain before native checks"
    );
    assert!(
        cargo_deny_job
            .contains("uses: taiki-e/install-action@9458fed45835344deb5e69e06e831be047c96abf")
            && cargo_deny_job.contains("          tool: cargo-deny@0.20.2\n"),
        "cargo-deny must use the existing pinned native installer and version"
    );
    assert!(
        !cargo_deny_job.contains("EmbarkStudios/cargo-deny-action@"),
        "cargo-deny must not require the Docker-based action on self-hosted runners"
    );
    assert_eq!(
        cargo_deny_job
            .matches("cargo deny --manifest-path ")
            .count(),
        1,
        "cargo-deny should have exactly one native invocation"
    );
    assert!(
        normalized_cargo_deny_job.contains(
            r#"cargo deny --manifest-path "${{ matrix.manifest }}" --all-features --config deny.toml check --allow license-exception-not-encountered advisories bans licenses sources"#
        ),
        "native cargo-deny invocation must preserve the checked CLI ordering and policy"
    );
    for manifest in [
        "Cargo.toml",
        "crates/pyo3_bindgen_engine/tests/fixtures/num_complex_consumer/Cargo.toml",
        "crates/pyo3_bindgen_engine/tests/fixtures/numpy_consumer/Cargo.toml",
    ] {
        assert!(
            cargo_deny_workflow.contains(&format!("manifest: {manifest}")),
            "cargo-deny matrix must retain manifest {manifest}"
        );
    }
    assert_pre_commit_revisions_are_sha_pinned(&pre_commit_config);
    assert!(
        !dependabot_config.to_lowercase().contains("auto-merge")
            && !dependabot_config.to_lowercase().contains("automerge"),
        "Dependabot configuration must not enable auto-merge, bypassing human review"
    );
    let dependency_policy = workflow_job(&rust_workflow, "dependency-policy");
    let deny_aggregate = workflow_job(&rust_workflow, "deny");
    assert!(
        dependency_policy.contains("uses: ./.github/workflows/cargo-deny.yml")
            && dependency_policy.contains(&format!("if: {PRIVATE_FORK_ADMISSION}"))
            && !dependency_policy.contains("runs-on: "),
        "dependency-policy must guard private fork PRs without claiming a runner"
    );
    assert!(
        deny_aggregate.contains("name: deny")
            && deny_aggregate.contains("needs: dependency-policy")
            && deny_aggregate.contains(&format!("if: {DENY_ADMISSION}"))
            && deny_aggregate.contains("DEPENDENCY_POLICY_RESULT: ${{ needs['dependency-policy'].result }}")
            && deny_aggregate.contains("run: test \"$DEPENDENCY_POLICY_RESULT\" = success")
            && deny_aggregate.contains("timeout-minutes: 5"),
        "deny aggregate must preserve the required check, guard private forks, and reject non-success results"
    );
    for (name, workflow) in [
        ("Rust", rust_workflow.as_str()),
        ("pre-commit", pre_commit_workflow.as_str()),
        ("cargo-deny", cargo_deny_workflow.as_str()),
    ] {
        assert!(
            workflow.contains(DUAL_LINUX_RUNNER)
                && workflow.contains("github.event.repository.private == true")
                && workflow.contains(
                    "github.event.pull_request.head.repo.full_name == github.repository",
                )
                && workflow.contains("'ubuntu-24.04'"),
            "{name} must route only the exact private repository's same-repository jobs to persistent Linux"
        );
        assert!(
            !workflow.contains("runs-on: [self-hosted, Linux, X64]"),
            "{name} must not bypass the private repository and fork guards"
        );
    }
    assert!(
        workflow_job(&rust_workflow, "rustfmt").contains(&format!("if: {PRIVATE_FORK_ADMISSION}"))
            && workflow_job(&pre_commit_workflow, "pre-commit")
                .contains(&format!("if: {PRIVATE_FORK_ADMISSION}"))
            && workflow_job(&cargo_deny_workflow, "cargo-deny")
                .contains(&format!("if: {PRIVATE_FORK_ADMISSION}")),
        "private fork admission guard must cover root and reusable CI jobs"
    );
    for job in [
        workflow_job(&rust_workflow, "cargo"),
        dependency_policy,
        deny_aggregate,
        workflow_job(&cargo_deny_workflow, "cargo-deny"),
    ] {
        assert!(
            !job.contains("continue-on-error"),
            "CI matrix, reusable caller, and deny aggregate must not mask failures"
        );
    }
    for job_name in [
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
    ] {
        assert!(
            workflow_job(&rust_workflow, job_name).contains("needs: rustfmt"),
            "Rust {job_name} must stay behind the guarded rustfmt job"
        );
    }
    for job_name in [
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
    ] {
        assert!(
            workflow_job(&rust_workflow, job_name).contains(DUAL_LINUX_RUNNER),
            "Rust {job_name} must use the guarded dual Linux runner"
        );
    }
    assert!(
        workflow_job(&pre_commit_workflow, "pre-commit").contains(DUAL_LINUX_RUNNER)
            && workflow_job(&cargo_deny_workflow, "cargo-deny").contains(DUAL_LINUX_RUNNER),
        "routine pre-commit and cargo-deny jobs must use the guarded dual Linux runner"
    );
    assert!(
        rust_workflow.contains(r#"CARGO_TARGET_DIR: ${{ github.workspace }}/target"#),
        "Rust workflow must keep Cargo artifacts under the workspace"
    );
    for job_name in [
        "cargo",
        "platform-runtime",
        "native-platforms",
        "python-compatibility",
        "abi3",
    ] {
        let job = workflow_job(&rust_workflow, job_name);
        assert!(
            job.contains("cargo test --workspace --lib --bins --tests --examples"),
            "Rust {job_name} runtime tests must enumerate non-benchmark targets explicitly"
        );
    }
    for line in rust_workflow.lines() {
        let line = line.trim();
        let command = line.strip_prefix("run:").unwrap_or(line).trim();
        if command.starts_with("cargo test ") {
            assert!(
                !command.contains("--all-targets"),
                "Rust test commands must not execute benchmark targets: {command}"
            );
        }
    }
    let platform_runtime_job = workflow_job(&rust_workflow, "platform-runtime");
    assert!(
        platform_runtime_job.contains("label: linux-free-threaded")
            && platform_runtime_job.contains("python: \"3.14.0t\"")
            && !platform_runtime_job.contains("windows-")
            && !platform_runtime_job.contains("macos-")
            && !platform_runtime_job.contains("os:"),
        "platform-runtime must retain only the Linux free-threaded lane"
    );
    let native_platforms_job = workflow_job(&rust_workflow, "native-platforms");
    assert!(
        native_platforms_job.contains("name: native | Linux")
            && !native_platforms_job.contains("strategy:")
            && !native_platforms_job.contains("matrix:")
            && !native_platforms_job.contains("windows-")
            && !native_platforms_job.contains("macos-")
            && !native_platforms_job.contains("os:"),
        "native-platforms must be one Linux native lane without an OS matrix"
    );
    let apple_cross_check_job = workflow_job(&rust_workflow, "apple-cross-check");
    let normalized_apple_cross_check_job = normalize_whitespace(apple_cross_check_job);
    assert!(
        apple_cross_check_job
            .contains("          - aarch64-apple-darwin\n          - x86_64-apple-darwin")
            && apple_cross_check_job
                .contains("# No Apple SDK, linking, or target runtime is involved in cargo check.")
            && !apple_cross_check_job.contains("cargo test ")
            && !apple_cross_check_job.contains("actions/setup-python@")
            && !apple_cross_check_job.contains("continue-on-error")
            && apple_cross_check_job.matches("cargo check ").count() == 3,
        "Apple cross-check must be explicit compile-only assurance"
    );
    for required in [
        "cargo check -p pyo3_bindgen_engine --all-targets --all-features --locked --verbose --target ${{ matrix.target }}",
        "cargo check -p pyo3_bindgen --all-targets --features numpy,num-complex --locked --verbose --target ${{ matrix.target }}",
        "cargo check -p pyo3_bindgen_cli --all-targets --all-features --locked --verbose --target ${{ matrix.target }}",
    ] {
        assert!(
            normalized_apple_cross_check_job.contains(required),
            "Apple cross-check should contain {required}"
        );
    }
    assert!(
        !rust_workflow.contains("release:") && !rust_workflow.contains("cargo publish"),
        "publishing must not be coupled to publishing GitHub release metadata"
    );
    for required in [
        "x86_64-pc-windows-gnu",
        "x86_64-pc-windows-msvc",
        "aarch64-apple-darwin",
        "x86_64-apple-darwin",
        "\"3.8\"",
        "\"3.14\"",
        "\"3.14t\"",
        "3.14.0t",
        "pypy3.11",
        "graalpy-cross-check:",
        "PYO3_CROSS_PYTHON_IMPLEMENTATION: \"GraalVM\"",
        "PYO3_CROSS_PYTHON_VERSION: \"3.11\"",
        "cargo check --locked --all-features --lib -p pyo3_bindgen_engine",
        "cargo check --locked --features numpy,num-complex --lib -p pyo3_bindgen",
        "cargo check --locked --all-features --bin pyo3_bindgen -p pyo3_bindgen_cli",
        "pyo3/abi3-py310",
        "cargo-hack@0.6.45",
        "feature-powerset",
        "--locked",
    ] {
        assert!(
            rust_workflow.contains(required),
            "Rust workflow should cover {required}"
        );
    }
    assert!(
        rust_workflow.contains(
            r#"pyo3 = { version = "%s", default-features = false, features = ["py-clone"] }"#
        ) && rust_workflow.contains(r#""${pyo3_requirement}" > Cargo.toml"#),
        "the compile-sweep warmup must preserve the exact workspace PyO3 requirement"
    );
    assert!(
        !rust_workflow.contains(r#"pyo3 = { version = "=%s""#),
        "the compile-sweep warmup must not prepend a second `=` to the workspace requirement"
    );
    let windows_msrv_job = workflow_job(&rust_workflow, "windows-msrv-cross-check");
    let normalized_windows_msrv_job = normalize_whitespace(windows_msrv_job);
    for required in [
        "PYO3_CROSS: \"1\"",
        "PYO3_CROSS_PYTHON_VERSION: \"3.10\"",
        "RUSTFLAGS: \"-D warnings\"",
        "toolchain: ${{ env.MSRV }}",
        "targets: x86_64-pc-windows-gnu",
    ] {
        assert!(
            windows_msrv_job.contains(required),
            "Windows MSRV cross-check should contain {required}"
        );
    }
    assert_eq!(
        windows_msrv_job.matches("cargo check ").count(),
        3,
        "Windows MSRV cross-check should run exactly three scoped cargo checks"
    );
    for required in [
        "cargo check -p pyo3_bindgen_engine --all-targets --all-features --locked --verbose --target x86_64-pc-windows-gnu",
        "cargo check -p pyo3_bindgen --all-targets --features numpy,num-complex --locked --verbose --target x86_64-pc-windows-gnu",
        "cargo check -p pyo3_bindgen_cli --all-targets --all-features --locked --verbose --target x86_64-pc-windows-gnu",
    ] {
        assert!(
            normalized_windows_msrv_job.contains(required),
            "Windows MSRV cross-check should contain {required}"
        );
    }
    assert!(
        !windows_msrv_job.contains("--workspace")
            && !windows_msrv_job.contains("pyo3_bindgen_macros"),
        "Windows MSRV cross-check must exclude host-built proc macros"
    );
    let windows_msvc_job = workflow_job(&rust_workflow, "windows-msvc-cross-check");
    let normalized_windows_msvc_job = normalize_whitespace(windows_msvc_job);
    for required in [
        "PYO3_CROSS: \"1\"",
        "PYO3_CROSS_PYTHON_VERSION: \"3.10\"",
        "RUSTFLAGS: \"-D warnings\"",
        "toolchain: stable",
        "targets: x86_64-pc-windows-msvc",
    ] {
        assert!(
            windows_msvc_job.contains(required),
            "Windows MSVC cross-check should contain {required}"
        );
    }
    assert_eq!(
        windows_msvc_job.matches("cargo check ").count(),
        3,
        "Windows MSVC cross-check should run exactly three scoped cargo checks"
    );
    for required in [
        "cargo check -p pyo3_bindgen_engine --all-targets --all-features --locked --verbose --target x86_64-pc-windows-msvc",
        "cargo check -p pyo3_bindgen --all-targets --features numpy,num-complex --locked --verbose --target x86_64-pc-windows-msvc",
        "cargo check -p pyo3_bindgen_cli --all-targets --all-features --locked --verbose --target x86_64-pc-windows-msvc",
    ] {
        assert!(
            normalized_windows_msvc_job.contains(required),
            "Windows MSVC cross-check should contain {required}"
        );
    }
    assert!(
        windows_msvc_job.contains("compile-only")
            || windows_msvc_job.contains("compile only")
            || windows_msvc_job.contains("compile-only assurance"),
        "Windows MSVC cross-check should identify compile-only assurance"
    );
    assert!(
        !windows_msvc_job.contains("cargo test ")
            && !windows_msvc_job.contains("cargo run ")
            && !windows_msvc_job.contains("continue-on-error")
            && !windows_msvc_job.contains("--workspace")
            && !windows_msvc_job.contains("pyo3_bindgen_macros"),
        "Windows MSVC cross-check must remain scoped compile-only assurance"
    );
    let graalpy_job = workflow_job(&rust_workflow, "graalpy-cross-check");
    let normalized_graalpy_job = normalize_whitespace(graalpy_job);
    for required in [
        "PYO3_CROSS: \"1\"",
        "PYO3_CROSS_PYTHON_VERSION: \"3.11\"",
        "PYO3_CROSS_PYTHON_IMPLEMENTATION: \"GraalVM\"",
        "RUSTFLAGS: \"-D warnings\"",
    ] {
        assert!(
            graalpy_job.contains(required),
            "GraalPy job should contain {required}"
        );
    }
    assert_eq!(
        graalpy_job.matches("cargo check ").count(),
        4,
        "GraalPy cross-check should run exactly four scoped cargo checks"
    );
    for required in [
        "cargo check --locked --all-features --lib -p pyo3_bindgen_engine",
        "cargo check --locked --features numpy,num-complex --lib -p pyo3_bindgen",
        "cargo check --locked --all-features --bin pyo3_bindgen -p pyo3_bindgen_cli",
        "cargo check --locked -p pyo3_bindgen_macros",
    ] {
        assert!(
            normalized_graalpy_job.contains(required),
            "GraalPy job should contain {required}"
        );
    }
    assert!(
        publish_workflow.contains("RELEASE_RUST_TOOLCHAIN: \"1.97.0\"")
            && release_common.contains("PINNED_RUST_TOOLCHAIN=\"1.97.0\"")
            && release_preflight.contains("RELEASE_RUST_TOOLCHAIN:-${PINNED_RUST_TOOLCHAIN}")
            && workspace_publisher.contains("RELEASE_RUST_TOOLCHAIN:-${PINNED_RUST_TOOLCHAIN}")
            && release_preflight.contains("release_common_validate_release_versions")
            && workspace_publisher.contains("release_common_validate_release_versions")
            && !release_preflight.contains("+stable")
            && !workspace_publisher.contains("+stable"),
        "release packaging and publication should use one exact Rust toolchain"
    );
    assert!(
        publish_workflow.contains("RELEASE_PYTHON_VERSION: \"3.10.21\"")
            && release_common.contains("PINNED_PYTHON_VERSION=\"3.10.21\"")
            && release_preflight.contains("RELEASE_PYTHON_VERSION:-${PINNED_PYTHON_VERSION}")
            && workspace_publisher.contains("RELEASE_PYTHON_VERSION:-${PINNED_PYTHON_VERSION}")
            && release_common.contains("sys.implementation.name")
            && release_common.contains("release requires CPython")
            && release_common.contains("Py_ENABLE_SHARED")
            && release_common.contains("Py_ENABLE_SHARED\") == 1")
            && release_common.contains("release requires a shared, linkable CPython")
            && release_common.contains("export PYO3_PYTHON")
            && release_preflight.contains(
                "release_common_validate_python \"$PYTHON_BIN\" \"$PYTHON_VERSION\" true"
            )
            && workspace_publisher.contains(
                "release_common_validate_python \"$PYTHON_BIN\" \"$PYTHON_VERSION\" false"
            ),
        "release packaging should use one exact shared Python interpreter"
    );
    let version_validation = bounded_section(
        &release_common,
        "release_common_validate_release_versions() {",
        "release_common_validate_python() {",
        "shared release policy",
    );
    for required in [
        "[[ \"$rust_toolchain\" == \"$PINNED_RUST_TOOLCHAIN\" ]]",
        "normal release requires RELEASE_RUST_TOOLCHAIN=${PINNED_RUST_TOOLCHAIN}",
        "[[ \"$python_version\" == \"$PINNED_PYTHON_VERSION\" ]]",
        "normal release requires RELEASE_PYTHON_VERSION=${PINNED_PYTHON_VERSION}",
    ] {
        assert!(
            version_validation.contains(required),
            "shared release policy should enforce checked-in release pins in normal mode: {required}"
        );
    }
    for (name, script) in [
        ("release preflight", release_preflight.as_str()),
        ("workspace publisher", workspace_publisher.as_str()),
    ] {
        assert!(
            script.contains(
                "--check accepts exact-version overrides for local structural validation"
            ),
            "{name} should document that only check mode permits exact-version overrides"
        );
    }
    assert!(
        release_preflight.contains("--locked")
            && workspace_publisher.contains("--locked")
            && !workspace_publisher.contains("--no-verify"),
        "release scripts must preserve lockfile and package verification gates"
    );
    assert!(
        release_preflight
            .matches("\"$GIT_BIN\" status --porcelain=v1 --untracked-files=all")
            .count()
            == 2
            && release_preflight.contains(r#"tree_status="$("$GIT_BIN" status"#)
            && release_preflight.contains(r#"final_tree_status="$("$GIT_BIN" status"#)
            && release_preflight.contains("die \"could not inspect working tree\"")
            && release_preflight.contains("die \"could not re-inspect working tree\"")
            && !release_preflight.contains(r#"[[ -z "$("$GIT_BIN" status"#),
        "release preflight must fail when either working-tree inspection fails"
    );
    let token_flag_position = release_preflight
        .find("REGISTRY_TOKEN_WAS_PRESENT=false")
        .expect("release preflight should record token presence");
    let token_unset_position = release_preflight
        .find("unset CARGO_REGISTRY_TOKEN")
        .expect("release preflight should discard the crates.io token before helper commands");
    let package_position = release_preflight
        .find("package --workspace --exclude pyo3_bindgen_examples --locked")
        .expect("release preflight should package the workspace");
    assert!(
        token_flag_position < token_unset_position
            && token_unset_position < package_position
            && release_preflight.contains("unset RELEASE_REGISTRY_TOKEN")
            && release_preflight.contains("if [[ \"$REGISTRY_TOKEN_WAS_PRESENT\" != true ]]")
            && release_preflight.contains("export -n REGISTRY_TOKEN_WAS_PRESENT"),
        "release preflight must retain only a non-exported token-presence flag"
    );
    assert!(
        workspace_publisher.contains("unset RELEASE_REGISTRY_TOKEN")
            && workspace_publisher.contains("export -n RELEASE_REGISTRY_TOKEN")
            && workspace_publisher.contains(
                "CARGO_REGISTRY_TOKEN=\"$RELEASE_REGISTRY_TOKEN\" \\\n        \"$PREFLIGHT\""
            )
            && workspace_publisher.contains(
                "if ! CARGO_REGISTRY_TOKEN=\"$RELEASE_REGISTRY_TOKEN\" \\\n        \"$CARGO_BIN\""
            )
            && !workspace_publisher.contains("export CARGO_REGISTRY_TOKEN"),
        "publisher must keep the token private and pass it only to preflight and publish"
    );
    assert!(
        release_preflight.contains(
            r#"final_tag_commit="$("$GIT_BIN" rev-parse --verify "refs/tags/${expected_tag}^{commit}")""#
        ) && release_preflight.contains("[[ \"$final_tag_commit\" == \"$head_commit\" ]]")
            && release_preflight
                .contains("local tag ${expected_tag} changed during package verification"),
        "release preflight must recheck local tag identity after packaging"
    );
    let publisher_identity = bounded_section(
        &workspace_publisher,
        "verify_release_identity() {",
        "fetch_remote_checksum() {",
        "workspace publisher",
    );
    for required in [
        r#"current_head="$("$GIT_BIN" rev-parse --verify "HEAD^{commit}")" ||"#,
        "[[ \"$current_head\" == \"$release_head\" ]]",
        r#"tree_status="$("$GIT_BIN" status --porcelain=v1 --untracked-files=all)" ||"#,
        "[[ -z \"$tree_status\" ]]",
        r#"current_tag="$("$GIT_BIN" rev-parse --verify "refs/tags/${expected_tag}^{commit}")" ||"#,
        "[[ \"$current_tag\" == \"$release_head\" ]]",
        "unset CARGO_REGISTRY_TOKEN",
        "release_common_resolve_remote_tag_commit",
        "[[ \"$public_tag_commit\" == \"$release_head\" ]]",
    ] {
        assert!(
            publisher_identity.contains(required),
            "publisher identity check should contain {required}"
        );
    }
    assert!(
        workspace_publisher.contains(
            r#"release_head="$("$GIT_BIN" -C "$REPOSITORY_ROOT" rev-parse --verify 'HEAD^{commit}')" ||"#
        ) && workspace_publisher
            .contains(r#"post_preflight_head="$("$GIT_BIN" rev-parse --verify 'HEAD^{commit}')" ||"#)
            && workspace_publisher.contains("[[ \"$post_preflight_head\" == \"$release_head\" ]]")
            && workspace_publisher.contains("repository_url")
            && workspace_publisher.contains("public_remote"),
        "publisher must pin repository identity across preflight"
    );
    let publisher_loop = bounded_section(
        &workspace_publisher,
        "for index in \"${!PACKAGES[@]}\"; do",
        "note \"all release crates are present",
        "workspace publisher",
    );
    let publish_command = "\"$CARGO_BIN\" \"+${RUST_TOOLCHAIN}\" publish";
    let before_publish = publisher_loop
        .split_once(publish_command)
        .map(|(before, _)| before)
        .expect("publisher loop should contain cargo publish");
    assert!(
        before_publish.contains(
            "crate_file=\"${target_directory}/package/${package}-${workspace_version}.crate\""
        ),
        "publisher should reconstruct the package archive path inside the publish loop"
    );
    let registry_position = before_publish
        .rfind("fetch_remote_checksum")
        .expect("publisher should recheck crates.io before publishing");
    let archive_position = before_publish
        .rfind("current_checksum=\"$(sha256_file \"$crate_file\")\"")
        .expect("publisher should rehash archive before publishing");
    let identity_position = before_publish
        .rfind("verify_release_identity")
        .expect("publisher should verify identity before publishing");
    assert!(
        registry_position < archive_position && archive_position < identity_position,
        "publisher must verify registry, archive, then repository immediately before mutation"
    );
    assert!(
        publisher_loop.contains(
            "done\n\nverify_release_identity \"after confirming workspace ${workspace_version}\""
        ),
        "publisher must verify release identity once more after all registry confirmations"
    );
    for required in [
        "workflow_dispatch:",
        "sha:",
        "required: true",
        "environment: crates-io",
        "id-token: write",
        "rust-lang/crates-io-auth-action@",
        "scripts/release-preflight.sh",
        "scripts/publish-workspace.sh",
    ] {
        assert!(
            publish_workflow.contains(required),
            "publish workflow should contain {required}"
        );
    }
    assert!(
        publish_workflow.contains("description: Exact release tag present in the public repository (vX.Y.Z)")
            && publish_workflow.contains("ref: ${{ inputs.sha }}")
            && publish_workflow.contains("EVENT_REPOSITORY: ${{ github.repository }}")
            && publish_workflow
                .contains("EVENT_REPOSITORY_OWNER_ID: ${{ github.repository_owner_id }}")
            && publish_workflow.contains("EVENT_REPOSITORY_ID: ${{ github.repository_id }}")
            && publish_workflow
                .contains("EVENT_REPOSITORY_VISIBILITY: ${{ github.repository_visibility }}")
            && publish_workflow.contains("EVENT_REF: ${{ github.ref }}")
            && publish_workflow.contains("EVENT_REF_PROTECTED: ${{ github.ref_protected }}")
            && publish_workflow.contains("EVENT_SHA: ${{ github.sha }}")
            && publish_workflow.contains("EVENT_WORKFLOW_REF: ${{ github.workflow_ref }}")
            && publish_workflow.contains("EVENT_WORKFLOW_SHA: ${{ github.workflow_sha }}")
            && publish_workflow.contains(
                "PYO3_BINDGEN_CRATES_IO_POLICY_SHA: ${{ vars.PYO3_BINDGEN_CRATES_IO_POLICY_SHA }}"
            )
            && publish_workflow.contains(
                "[[ \"$PYO3_BINDGEN_CRATES_IO_POLICY_SHA\" == \"$CANDIDATE_SHA\" ]]"
            )
            && publish_workflow.contains("expected_repository='AndrejOrsula/pyo3_bindgen'")
            && publish_workflow.contains("expected_repository_owner_id='22929099'")
            && publish_workflow.contains("expected_repository_id='745882942'")
            && publish_workflow.contains("expected_repository_visibility='public'")
            && publish_workflow.contains(
                "[[ \"$EVENT_REPOSITORY\" == \"$expected_repository\" ]]"
            )
            && publish_workflow.contains(
                "[[ \"$EVENT_REPOSITORY_OWNER_ID\" == \"$expected_repository_owner_id\" ]]"
            )
            && publish_workflow.contains(
                "[[ \"$EVENT_REPOSITORY_ID\" == \"$expected_repository_id\" ]]"
            )
            && publish_workflow.contains(
                "[[ \"$EVENT_REPOSITORY_VISIBILITY\" == \"$expected_repository_visibility\" ]]"
            )
            && publish_workflow.contains(
                "[[ \"$EVENT_REF\" == \"refs/tags/${CANDIDATE_TAG}\" ]]"
            )
            && publish_workflow.contains(
                "[[ \"$EVENT_REF_TYPE\" == tag && \"$EVENT_REF_PROTECTED\" == true ]]"
            )
            && publish_workflow.contains("[[ \"$EVENT_SHA\" == \"$CANDIDATE_SHA\" ]]")
            && publish_workflow.contains(
                "expected_workflow_ref=\"${expected_repository}/.github/workflows/publish.yml@refs/tags/${CANDIDATE_TAG}\""
            )
            && publish_workflow.contains(
                "[[ \"$EVENT_WORKFLOW_REF\" == \"$expected_workflow_ref\" ]]"
            )
            && publish_workflow.contains(
                "[[ \"$EVENT_WORKFLOW_SHA\" == \"$CANDIDATE_SHA\" ]]"
            )
            && publish_workflow.contains("checked_out_head=\"$(git rev-parse --verify 'HEAD^{commit}')\"")
            && publish_workflow.contains("peeled_tag=\"$(git rev-parse --verify \"refs/tags/${CANDIDATE_TAG}^{commit}\")\"")
            && publish_workflow.contains("Validate immutable release identity")
            && normalized_checklist.contains("local `HEAD^{commit}`")
            && normalized_checklist.contains("PUBLIC_REMOTE")
            && !normalized_checklist.contains("PRIVATE_REMOTE")
            && !publish_workflow.contains("AndrejOrsula/pyo3_bindgen_private")
            && !publish_workflow.contains("SpaceRL/pyo3_bindgen_private")
            && !publish_workflow.contains("114816174")
            && !publish_workflow.contains("1241024680")
            && !publish_workflow.contains("expected_repository_visibility='private'"),
        "publish handoff should require the public candidate tag and reject private identities"
    );
    let identity_position = publish_workflow
        .find("Validate immutable release identity")
        .expect("publish workflow should validate identity");
    let auth_position = publish_workflow
        .find("rust-lang/crates-io-auth-action@")
        .expect("publish workflow should acquire OIDC credentials");
    let publish_position = publish_workflow
        .find("scripts/publish-workspace.sh")
        .expect("publish workflow should invoke the workspace publisher");
    assert!(
        identity_position < auth_position && auth_position < publish_position,
        "workflow identity gate must precede OIDC authentication and publication"
    );
    assert!(
        security_workflow.contains("schedule:")
            && cargo_deny_workflow
                .contains("taiki-e/install-action@9458fed45835344deb5e69e06e831be047c96abf",)
            && !cargo_deny_workflow.contains("cargo-deny-action@"),
        "security workflow should refresh advisory evidence on a schedule"
    );
    for required in [
        "crates/pyo3_bindgen_engine/tests/fixtures/num_complex_consumer/Cargo.toml",
        "crates/pyo3_bindgen_engine/tests/fixtures/numpy_consumer/Cargo.toml",
        "Cargo.toml.fixture",
        "Cargo.lock.fixture",
        "taiki-e/install-action@9458fed45835344deb5e69e06e831be047c96abf # cargo-deny",
        "tool: cargo-deny@0.20.2",
        "--all-features --config deny.toml",
        "check --allow license-exception-not-encountered advisories bans licenses sources",
    ] {
        assert!(
            cargo_deny_workflow.contains(required),
            "reusable dependency checks should cover {required}"
        );
    }
    assert!(
        deny_config.contains("BSD-2-Clause"),
        "the license policy should permit the locked NumPy consumer dependency"
    );
    assert!(
        security_policy.contains("security/advisories/new")
            && security_policy.contains("Do not open a public issue"),
        "security policy should provide a private disclosure route"
    );
    assert!(
        !publish_workflow.contains("--no-verify")
            && !publish_workflow.contains("CARGO_REGISTRY_TOKEN: ${{ secrets."),
        "publishing should verify packages and use short-lived trusted credentials"
    );
    for line in publish_workflow
        .lines()
        .filter(|line| line.contains("${{ inputs."))
    {
        assert!(
            matches!(
                line.trim(),
                "ref: ${{ inputs.sha }}"
                    | "CANDIDATE_TAG: ${{ inputs.tag }}"
                    | "CANDIDATE_SHA: ${{ inputs.sha }}"
                    | "RELEASE_TAG: ${{ inputs.tag }}"
                    | "RELEASE_SHA: ${{ inputs.sha }}"
            ),
            "untrusted workflow inputs may only select checkout ref or enter shell through an environment variable: {line}"
        );
    }
    assert_workflow_runs_on_default_branch("Rust", &rust_workflow);
    assert_workflow_runs_on_default_branch("pre-commit", &pre_commit_workflow);

    for required in [
        "gh api repos/actions/runner/releases/latest --jq .tag_name",
        "gh api repos/actions/setup-python/releases/latest --jq .tag_name",
        "gh api repos/rust-lang/crates-io-auth-action/releases/latest --jq .tag_name",
        "Node 24",
        "runner version v2.327.1 or newer",
        "default `dev` pushes",
        "cargo-deny 0.20.2",
        "After authorized local/public tagging and before any crates.io mutation",
        "assert sys.version_info[:3] == (3, 10, 21)",
        "cargo +1.83 test --workspace --lib --bins --tests --examples --all-features --locked",
    ] {
        assert!(
            normalized_checklist.contains(required),
            "release checklist should cover {required}"
        );
    }
    assert!(
        !normalized_checklist.contains("only runs automatically on `main` pushes"),
        "release checklist should not describe stale main-only default-branch CI"
    );
}

#[test]
fn pyo3_version_examples_track_workspace_pin() {
    let Some(root) = workspace_root() else {
        return;
    };
    let pyo3_version = workspace_pyo3_version(&root);
    let pyo3_minor_version = minor_version(&pyo3_version);

    let readme = std::fs::read_to_string(root.join("README.md")).expect("README.md should exist");
    let public_docs = std::fs::read_to_string(root.join("crates/pyo3_bindgen/src/lib.rs"))
        .expect("pyo3_bindgen public docs should exist");
    let notes = release_notes(&root, &release_minor_version(&root));

    let dependency_snippet = format!("pyo3 = {{ version = \"{pyo3_minor_version}\"");
    let api_snippet = format!("targets the `pyo3` `{pyo3_minor_version}` API");
    let release_notes_snippet = format!("PyO3 `{pyo3_minor_version}`");

    assert!(
        readme.contains(&dependency_snippet),
        "README dependency example should track workspace pyo3 minor version"
    );
    assert!(
        readme.contains(&api_snippet),
        "README API compatibility note should track workspace pyo3 minor version"
    );
    assert!(
        public_docs.contains(&dependency_snippet),
        "public crate docs dependency example should track workspace pyo3 minor version"
    );
    assert!(
        notes.contains(&release_notes_snippet),
        "release notes should name the workspace pyo3 minor version"
    );
}

#[test]
fn public_dependency_guidance_matches_generated_code_requirements() {
    let Some(root) = workspace_root() else {
        return;
    };
    let pyo3_minor_version = minor_version(&workspace_pyo3_version(&root));
    let release_minor_version = release_minor_version(&root);
    let pyo3_dependency = format!("pyo3 = {{ version = \"{pyo3_minor_version}\"");
    let facade_dependency = format!(
        "pyo3_bindgen = {{ version = \"{release_minor_version}\", features = [\"macros\"] }}"
    );

    let readme = std::fs::read_to_string(root.join("README.md")).expect("README.md should exist");
    let public_docs = std::fs::read_to_string(root.join("crates/pyo3_bindgen/src/lib.rs"))
        .expect("pyo3_bindgen public docs should exist");

    for (name, document) in [
        ("README", readme.as_str()),
        ("public docs", public_docs.as_str()),
    ] {
        let build_script = bounded_section(document, "Option 1: Build script", "Option 2:", name);
        let macros = bounded_section(document, "Option 2: Procedural macros", "Option 3:", name);

        assert!(
            build_script.contains(&pyo3_dependency),
            "{name} build-script instructions should require direct PyO3 dependency"
        );
        assert!(
            !build_script.contains("pyo3_bindgen::pyo3"),
            "{name} must not claim facade re-export can satisfy generated `::pyo3` paths"
        );
        assert!(
            macros.contains(&pyo3_dependency),
            "{name} macro instructions should require direct PyO3 dependency"
        );
        assert!(
            macros.contains(&facade_dependency),
            "{name} macro instructions should enable macros on the facade crate"
        );
        assert!(
            !macros.contains("pyo3_bindgen_macros =")
                && !macros.contains("add `pyo3_bindgen_macros`"),
            "{name} must not instruct consumers to depend on the internal macro crate"
        );
    }
}

#[test]
fn release_notes_cover_api_breakage_and_fallbacks() {
    let Some(root) = workspace_root() else {
        return;
    };
    let release_minor_version = release_minor_version(&root);
    let notes = release_notes(&root, &release_minor_version);
    let notes = normalize_whitespace(&notes).to_lowercase();

    for required in [
        "generated api breakage",
        "wrapper semantics",
        "enum mirror behavior",
        "report format",
        "known fallbacks",
        "gymnasium dogfood",
        "generic",
        "pyo3",
        "cpython",
    ] {
        assert!(
            notes.contains(required),
            "release notes should cover {required}"
        );
    }
}
