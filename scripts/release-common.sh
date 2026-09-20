#!/usr/bin/env bash

# Shared release-script policy. This file is sourced by the two public
# release entrypoints after they have enabled strict Bash mode.

readonly PINNED_RUST_TOOLCHAIN="1.97.0"
readonly PINNED_PYTHON_VERSION="3.10.21"
readonly PINNED_PUBLIC_REPOSITORY_URL="https://github.com/AndrejOrsula/pyo3_bindgen"
readonly -a RELEASE_PACKAGES=(
    pyo3_bindgen_engine
    pyo3_bindgen_macros
    pyo3_bindgen
    pyo3_bindgen_cli
)

release_common_validate_public_repository_url() {
    local repository_url="$1"

    # Preserve the manifest spellings accepted by the release scripts while
    # comparing the resulting URL against the approved public identity.
    repository_url="${repository_url%/}"
    repository_url="${repository_url%.git}"
    [[ "$repository_url" =~ ^https://github\.com/[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$ ]] || \
        die "workspace repository must be a public HTTPS GitHub URL, got: ${repository_url}"
    [[ "$repository_url" == "$PINNED_PUBLIC_REPOSITORY_URL" ]] || \
        die "workspace repository must equal ${PINNED_PUBLIC_REPOSITORY_URL}, got: ${repository_url}"

    printf '%s\n' "$repository_url"
}

release_common_resolve_executable() {
    local requested="$1"
    local destination="$2"
    local candidate directory executable_name absolute_directory

    if [[ "$requested" == */* ]]; then
        candidate="$requested"
    else
        candidate="$(builtin type -P -- "$requested")" || \
            die "required executable not found in PATH: $requested"
    fi
    [[ -f "$candidate" && -x "$candidate" ]] || \
        die "required command is not an executable file: $requested"

    if [[ "$candidate" == */* ]]; then
        directory="${candidate%/*}"
        executable_name="${candidate##*/}"
        [[ -n "$directory" ]] || directory="/"
    else
        directory="."
        executable_name="$candidate"
    fi
    absolute_directory="$(cd -- "$directory" && pwd -P)" || \
        die "could not resolve executable directory for: $requested"
    candidate="${absolute_directory}/${executable_name}"
    [[ -f "$candidate" && -x "$candidate" ]] || \
        die "resolved command is not an executable file: $candidate"
    printf -v "$destination" '%s' "$candidate"
}

release_common_reject_pyo3_environment_overrides() {
    local variable
    local -a variables=(
        PYO3_BUILD_EXTENSION_MODULE
        PYO3_CONFIG_FILE
        PYO3_CROSS
        PYO3_CROSS_LIB_DIR
        PYO3_CROSS_PYTHON_VERSION
        PYO3_CROSS_PYTHON_IMPLEMENTATION
        PYO3_NO_PYTHON
        PYO3_PRINT_CONFIG
        PYO3_USE_ABI3_FORWARD_COMPATIBILITY
        PYO3_USE_STABLE_ABI_FORWARD_COMPATIBILITY
        UNSAFE_PYO3_SKIP_VERSION_CHECK
    )
    for variable in "${variables[@]}"; do
        [[ ! -v "$variable" ]] || \
            die "${variable} must be unset; release configuration is derived from PYTHON_BIN"
    done
}

release_common_validate_release_versions() {
    local check_only="$1"
    local rust_toolchain="$2"
    local python_version="$3"

    [[ "$rust_toolchain" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || \
        die "RELEASE_RUST_TOOLCHAIN must be an exact X.Y.Z version"
    [[ "$python_version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || \
        die "RELEASE_PYTHON_VERSION must be an exact X.Y.Z version"
    if [[ "$check_only" == false ]]; then
        [[ "$rust_toolchain" == "$PINNED_RUST_TOOLCHAIN" ]] || \
            die "normal release requires RELEASE_RUST_TOOLCHAIN=${PINNED_RUST_TOOLCHAIN}; got ${rust_toolchain}"
        [[ "$python_version" == "$PINNED_PYTHON_VERSION" ]] || \
            die "normal release requires RELEASE_PYTHON_VERSION=${PINNED_PYTHON_VERSION}; got ${python_version}"
    fi
}

release_common_validate_python() {
    local python_bin="$1"
    local expected_version="$2"
    local require_linkable_cpython="$3"
    local actual_python_implementation actual_python_version python_shared

    if [[ "$require_linkable_cpython" == true ]]; then
        actual_python_implementation="$("$python_bin" -c \
            'import sys; print(sys.implementation.name)')" || \
            die "could not query ${python_bin} implementation"
        [[ "$actual_python_implementation" == "cpython" ]] || \
            die "${python_bin} is ${actual_python_implementation}; release requires CPython"
    elif [[ "$require_linkable_cpython" != false ]]; then
        die "internal error: invalid shared CPython validation mode"
    fi

    actual_python_version="$("$python_bin" -c \
        'import sys; print(".".join(map(str, sys.version_info[:3])))')" || \
        die "could not query ${python_bin} version"
    [[ "$actual_python_version" == "$expected_version" ]] || \
        die "${python_bin} is Python ${actual_python_version}; release requires ${expected_version}"

    if [[ "$require_linkable_cpython" == true ]]; then
        python_shared="$("$python_bin" -c \
            'import sysconfig; print(int(sysconfig.get_config_var("Py_ENABLE_SHARED") == 1))')" || \
            die "could not query whether ${python_bin} provides shared libpython"
        [[ "$python_shared" == "1" ]] || \
            die "${python_bin} is static-only; release requires a shared, linkable CPython"
    fi
    export PYO3_PYTHON="$python_bin"
}

release_common_require_privileged_mode() {
    case "$-" in
        *x*)
            printf 'release scripts do not support xtrace (-x)\n' >&2
            return 1
            ;;
        *p*) ;;
        *)
            printf 'release scripts must be invoked directly or with bash -p; plain bash, sh, and source are unsupported\n' >&2
            return 1
            ;;
    esac
}

release_common_sanitize_environment() {
    local script_path="$1"
    shift
    local entry name
    local cargo_token=""
    local cargo_token_present=false
    local cargo_token_exported=false
    local scrub_marker=false
    local env_pid env_source_fd scan_fd token_fd token_length
    local scan_failed=false
    local -a function_names=()

    [[ "${RELEASE_ENV_SANITIZED:-}" == 1 ]] && scrub_marker=true
    unset RELEASE_ENV_SANITIZED
    if [[ -v CARGO_REGISTRY_TOKEN ]]; then
        cargo_token_present=true
        cargo_token="${CARGO_REGISTRY_TOKEN}"
        if [[ "$(declare -p CARGO_REGISTRY_TOKEN 2>/dev/null)" == "declare -x"* ]]; then
            cargo_token_exported=true
        fi
    fi
    # Keep credentials and release-internal state out of the environment scanner
    # and out of the environment-removal command. The token is restored below
    # before continuing, or transferred over a private descriptor to the
    # sanitized Bash bootstrap.
    unset CARGO_REGISTRY_TOKEN
    unset BASH_ENV
    unset RELEASE_REGISTRY_TOKEN
    unset REGISTRY_TOKEN_WAS_PRESENT

    [[ -x /usr/bin/env ]] || return 1

    # Bash cannot represent the `%` suffix in BASH_FUNC_* names as shell
    # variables. Read the NUL-delimited process environment so every such entry,
    # including malformed function exports, can be removed before the re-exec.
    coproc RELEASE_ENV_DUMP { /usr/bin/env -0; }
    env_pid="$!"
    env_source_fd="${RELEASE_ENV_DUMP[0]}"
    if ! exec {scan_fd}<&"$env_source_fd"; then
        wait "$env_pid" || true
        return 1
    fi
    unset RELEASE_ENV_DUMP
    while IFS= read -r -d '' -u "$scan_fd" entry; do
        [[ "$entry" == *=* ]] || {
            scan_failed=true
            break
        }
        name="${entry%%=*}"
        [[ "$name" == BASH_FUNC_* ]] && function_names+=("$name")
    done
    exec {scan_fd}<&-
    if ! wait "$env_pid"; then
        scan_failed=true
    fi
    [[ "$scan_failed" == false ]] || return 1

    if [[ "${#function_names[@]}" == 0 ]]; then
        if [[ "$cargo_token_present" == true ]]; then
            CARGO_REGISTRY_TOKEN="$cargo_token"
            if [[ "$cargo_token_exported" == true ]]; then
                export CARGO_REGISTRY_TOKEN
            else
                export -n CARGO_REGISTRY_TOKEN
            fi
        fi
        return 0
    fi

    [[ "$scrub_marker" == false ]] || return 1

    if ! exec {token_fd}<<<"$cargo_token"; then
        return 1
    fi
    token_length="${#cargo_token}"

    # env(1) removes names that Bash cannot unset while preserving every other
    # inherited variable natively. It is invoked by its fixed system path so a
    # PATH override cannot replace the environment scrubber. The registry token
    # remains unset here and is restored only by the privileged bootstrap.
    local -a remove_args=()
    for name in "${function_names[@]}"; do
        remove_args+=(-u "$name")
    done
    # shellcheck disable=SC2016 # The single-quoted program expands in child Bash.
    exec /usr/bin/env "${remove_args[@]}" RELEASE_ENV_SANITIZED=1 /bin/bash -p -c '
        set -Eeuo pipefail
        IFS= read -r -N "$1" cargo_registry_token <&"$2"
        token_fd="$2"
        exec {token_fd}<&-
        export CARGO_REGISTRY_TOKEN="$cargo_registry_token"
        exec /bin/bash -p "$3" "${@:4}"
    ' release-environment-sanitizer "$token_length" "$token_fd" "$script_path" "$@"
}

release_common_validate_local_rust_toolchain() {
    local rustup_bin="$1"
    local rust_toolchain="$2"
    local listing line toolchain_name remainder toolchain_root rustc_version

    # `rustup toolchain list --verbose` reads the local inventory only. Resolve and
    # inspect the toolchain binaries before invoking the rustup Cargo proxy; a
    # missing exact toolchain must fail before rustup can offer a network install.
    listing="$("$rustup_bin" toolchain list --verbose)" || \
        die "could not inspect the local Rust toolchain inventory"
    while IFS= read -r line; do
        [[ -n "$line" ]] || continue
        toolchain_name="${line%% *}"
        [[ "$toolchain_name" == "$rust_toolchain" || "$toolchain_name" == "${rust_toolchain}-"* ]] || continue
        remainder="${line#"$toolchain_name"}"
        remainder="${remainder# }"
        if [[ "$remainder" == *") "* ]]; then
            toolchain_root="${remainder#*) }"
        else
            toolchain_root="$remainder"
        fi
        [[ -x "$toolchain_root/bin/cargo" && -x "$toolchain_root/bin/rustc" ]] || continue
        rustc_version="$("$toolchain_root/bin/rustc" --version)" || continue
        rustc_version="${rustc_version#rustc }"
        rustc_version="${rustc_version%% *}"
        [[ "$rustc_version" == "$rust_toolchain" ]] || continue
        return 0
    done <<<"$listing"

    die "Rust toolchain ${rust_toolchain} is not installed locally; refusing Cargo before network access"
}

release_common_resolve_remote_tag_commit() {
    local git_bin="$1"
    local remote="$2"
    local tag="$3"
    local context="${4:-remote tag}"
    local direct_ref="refs/tags/${tag}"
    local peeled_ref="refs/tags/${tag}^{}"
    local refs line sha ref direct="" peeled=""
    local git_pid refs_fd git_status=0
    local saw_nul=false
    local direct_count=0
    local peeled_count=0

    coproc RELEASE_REMOTE_REFS {
        "$git_bin" ls-remote --exit-code "$remote" "$direct_ref" "$peeled_ref"
    }
    git_pid="$!"
    refs_fd="${RELEASE_REMOTE_REFS[0]}"
    unset RELEASE_REMOTE_REFS RELEASE_REMOTE_REFS_PID
    if IFS= read -r -d '' -u "$refs_fd" refs; then
        saw_nul=true
    fi
    exec {refs_fd}<&-
    if wait "$git_pid"; then
        git_status=0
    else
        git_status=$?
    fi
    [[ "$saw_nul" == false ]] || \
        die "${context}: tag ${tag} returned a NUL byte in its ref response at ${remote}"
    if [[ "$git_status" != 0 ]]; then
        die "${context}: tag ${tag} is missing or unreadable at ${remote}"
    fi
    # The here-string below appends one newline. Remove only the one normal
    # terminal newline emitted by `git ls-remote`; a second newline remains
    # visible to the row parser and is rejected as a blank row.
    if [[ "$refs" == *$'\n' ]]; then
        refs="${refs%$'\n'}"
    fi

    # `git ls-remote` is a two-column protocol. Parse each row ourselves so a
    # malformed or unrelated row cannot be silently ignored by a field filter.
    # Git emits a direct row for both lightweight and annotated tags; annotated
    # tags additionally emit exactly one peeled `^{}` row.
    while IFS= read -r line || [[ -n "$line" ]]; do
        [[ -n "$line" ]] || \
            die "${context}: tag ${tag} returned a blank ref row at ${remote}"
        [[ "$line" == *$'\t'* ]] || \
            die "${context}: tag ${tag} returned a malformed ref row at ${remote}"
        sha="${line%%$'\t'*}"
        ref="${line#*$'\t'}"
        [[ "$ref" != *$'\t'* && -n "$sha" && -n "$ref" ]] || \
            die "${context}: tag ${tag} returned a malformed ref row at ${remote}"
        [[ "$sha" =~ ^[0-9a-fA-F]{40}$ ]] || \
            die "${context}: tag ${tag} returned an invalid SHA row at ${remote}"
        case "$ref" in
            "$direct_ref")
                direct_count=$((direct_count + 1))
                [[ "$direct_count" == 1 ]] || \
                    die "${context}: tag ${tag} returned duplicate direct refs at ${remote}"
                direct="${sha,,}"
                ;;
            "$peeled_ref")
                peeled_count=$((peeled_count + 1))
                [[ "$peeled_count" == 1 ]] || \
                    die "${context}: tag ${tag} returned duplicate peeled refs at ${remote}"
                peeled="${sha,,}"
                ;;
            *)
                die "${context}: tag ${tag} returned an unexpected ref row at ${remote}"
                ;;
        esac
    done <<<"$refs"

    [[ "$direct_count" == 1 ]] || \
        die "${context}: tag ${tag} did not return exactly one direct ref at ${remote}"
    case "$peeled_count" in
        0)
            # A direct-only result is a lightweight tag: its direct SHA is the
            # commit. Annotated tags must include the peeled ^{} result below.
            printf '%s\n' "$direct"
            ;;
        1)
            printf '%s\n' "$peeled"
            ;;
        *) die "${context}: tag ${tag} returned multiple peeled refs at ${remote}" ;;
    esac
}

release_common_workspace_metadata() {
    local cargo_bin="$1"
    local python_bin="$2"
    local rust_toolchain="$3"
    local package_order="${4:-release}"
    local rustup_bin="${5:-rustup}"

    release_common_validate_local_rust_toolchain "$rustup_bin" "$rust_toolchain"
    {
        "$cargo_bin" "+${rust_toolchain}" metadata --format-version 1 --no-deps --locked --offline
    } |
    "$python_bin" -c '
import json
import sys

package_order = sys.argv[1]
package_names = sys.argv[2:]
if package_order == "sorted":
    selected_names = sorted(package_names)
elif package_order == "release":
    selected_names = package_names
else:
    raise SystemExit("invalid release package ordering")

metadata = json.load(sys.stdin)
packages = {package["name"]: package for package in metadata["packages"]}
missing = [name for name in selected_names if name not in packages]
if missing:
    raise SystemExit("missing release packages: " + ", ".join(missing))

selected = [packages[name] for name in selected_names]
versions = {package["version"] for package in selected}
repositories = {package.get("repository") for package in selected}
if len(versions) != 1:
    raise SystemExit("release packages do not share one version")
if len(repositories) != 1 or None in repositories:
    raise SystemExit("release packages do not share one repository URL")
for package in selected:
    registries = package.get("publish")
    if registries is not None and "crates-io" not in registries:
        raise SystemExit(
            "{} does not permit publishing to crates-io".format(package["name"])
        )

print(
    next(iter(versions))
    + "\t"
    + metadata["target_directory"]
    + "\t"
    + next(iter(repositories))
)
' "$package_order" "${RELEASE_PACKAGES[@]}"
}
