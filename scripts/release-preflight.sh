#!/bin/bash -p
case $- in
    *x*)
        builtin printf 'release preflight: xtrace (-x) is unsupported; invoke directly or with bash -p\n' >&2
        builtin exit 1
        ;;
esac

set -Eeuo pipefail
IFS=$'\n\t'
umask 077

# A rustup proxy otherwise may install a missing active toolchain. Check the
# local inventory first and keep every later Cargo invocation fail-closed.
export RUSTUP_AUTO_INSTALL=0

SCRIPT_SOURCE="${BASH_SOURCE[0]}"
SCRIPT_DIR="${SCRIPT_SOURCE%/*}"
if [[ "$SCRIPT_DIR" == "$SCRIPT_SOURCE" ]]; then
    SCRIPT_DIR="."
fi
SCRIPT_DIR="$(cd -- "$SCRIPT_DIR" && pwd -P)"
readonly SCRIPT_DIR
REPOSITORY_ROOT="$(cd -- "${SCRIPT_DIR}/.." && pwd -P)"
readonly REPOSITORY_ROOT

die() {
    printf 'release preflight: %s\n' "$*" >&2
    exit 1
}

note() {
    printf 'release preflight: %s\n' "$*"
}

[[ "${BASH_SOURCE[0]}" == "$0" ]] || die "release entrypoints must be executed, not sourced"

# shellcheck disable=SC1091 # SCRIPT_DIR is resolved at runtime.
source "${SCRIPT_DIR}/release-common.sh"
release_common_require_privileged_mode || die "privileged Bash mode is required"
release_common_sanitize_environment "${SCRIPT_DIR}/${SCRIPT_SOURCE##*/}" "$@" || \
    die "could not sanitize inherited Bash environment"

# Keep only whether a non-empty token was supplied. The actual credential must
# never reach helper subprocesses such as metadata, Python, or package gates.
unset BASH_ENV
unset RELEASE_REGISTRY_TOKEN
unset REGISTRY_TOKEN_WAS_PRESENT
REGISTRY_TOKEN_WAS_PRESENT=false
if [[ -n "${CARGO_REGISTRY_TOKEN:-}" ]]; then
    REGISTRY_TOKEN_WAS_PRESENT=true
fi
export -n REGISTRY_TOKEN_WAS_PRESENT
readonly REGISTRY_TOKEN_WAS_PRESENT
unset CARGO_REGISTRY_TOKEN

CARGO_BIN="${CARGO_BIN:-cargo}"
PYTHON_BIN="${PYTHON_BIN:-python3}"
GIT_BIN="${GIT_BIN:-git}"
RUSTUP_BIN="${RUSTUP_BIN:-rustup}"
readonly RUST_TOOLCHAIN="${RELEASE_RUST_TOOLCHAIN:-${PINNED_RUST_TOOLCHAIN}}"
readonly PYTHON_VERSION="${RELEASE_PYTHON_VERSION:-${PINNED_PYTHON_VERSION}}"

CHECK_ONLY=false
REQUESTED_TAG=""

usage() {
    cat <<'EOF'
Usage: scripts/release-preflight.sh [--check] [--tag vX.Y.Z]

Fail-closed release preflight for the pyo3_bindgen workspace.

  --check       Validate local configuration and print the release plan only.
                This mode validates the local Rust toolchain inventory before
                Cargo and does not require a token, network access, or a build.
  --tag TAG     Require TAG to equal v<workspace version>. Normal mode also
                requires HEAD and the advertised public GitHub tag to resolve
                to the exact same commit.
  -h, --help    Show this help.

Normal releases require the checked-in Rust 1.97.0 and shared CPython 3.10.21
pins. --check accepts exact-version overrides for local structural validation.
EOF
}

while (($# > 0)); do
    case "$1" in
        --check)
            CHECK_ONLY=true
            shift
            ;;
        --tag)
            (($# >= 2)) || die "--tag requires a value"
            REQUESTED_TAG="$2"
            shift 2
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            die "unknown argument: $1"
            ;;
    esac
done

release_common_resolve_executable "$CARGO_BIN" CARGO_BIN
release_common_resolve_executable "$PYTHON_BIN" PYTHON_BIN
release_common_resolve_executable "$GIT_BIN" GIT_BIN
release_common_resolve_executable "$RUSTUP_BIN" RUSTUP_BIN
readonly CARGO_BIN PYTHON_BIN GIT_BIN RUSTUP_BIN
release_common_reject_pyo3_environment_overrides
release_common_validate_release_versions "$CHECK_ONLY" "$RUST_TOOLCHAIN" "$PYTHON_VERSION"
release_common_validate_python "$PYTHON_BIN" "$PYTHON_VERSION" true

cd -- "$REPOSITORY_ROOT"

metadata_line="$(release_common_workspace_metadata "$CARGO_BIN" "$PYTHON_BIN" "$RUST_TOOLCHAIN" sorted "$RUSTUP_BIN")" || \
    die "could not read release metadata with stable Cargo"

IFS=$'\t' read -r workspace_version _target_directory repository_url <<<"$metadata_line"
[[ -n "$workspace_version" && -n "$repository_url" ]] || die "incomplete release metadata"

expected_tag="v${workspace_version}"
if [[ -n "$REQUESTED_TAG" && "$REQUESTED_TAG" != "$expected_tag" ]]; then
    die "requested tag ${REQUESTED_TAG} does not equal workspace tag ${expected_tag}"
fi

repository_url="$(release_common_validate_public_repository_url "$repository_url")"
readonly public_remote="${repository_url}.git"

if [[ "$CHECK_ONLY" == true ]]; then
    note "check OK: version=${workspace_version} tag=${expected_tag} repository=${repository_url} Rust=${RUST_TOOLCHAIN} Python=${PYTHON_VERSION}"
    note "normal mode will verify a clean tagged tree, token presence, public tag parity, and locked packages"
    exit 0
fi

tree_status="$("$GIT_BIN" status --porcelain=v1 --untracked-files=all)" || \
    die "could not inspect working tree"
[[ -z "$tree_status" ]] || \
    die "working tree is not clean; commit or remove every tracked and untracked change"

head_commit="$("$GIT_BIN" rev-parse --verify "HEAD^{commit}")" || die "could not resolve HEAD"
tag_commit="$("$GIT_BIN" rev-parse --verify "refs/tags/${expected_tag}^{commit}")" || \
    die "required local tag does not exist: ${expected_tag}"
[[ "$head_commit" == "$tag_commit" ]] || \
    die "HEAD ${head_commit} is not exactly local tag ${expected_tag} (${tag_commit})"

if [[ "$REGISTRY_TOKEN_WAS_PRESENT" != true ]]; then
    die "CARGO_REGISTRY_TOKEN is required"
fi

remote_commit="$(release_common_resolve_remote_tag_commit \
    "$GIT_BIN" "$public_remote" "$expected_tag" "public tag")"
[[ "$remote_commit" == "$head_commit" ]] || \
    die "public tag ${expected_tag} resolves to ${remote_commit}, expected ${head_commit}"

note "public tag parity OK: ${expected_tag} -> ${head_commit}"
note "running locked Rust ${RUST_TOOLCHAIN} package gate (default features)"
"$CARGO_BIN" "+${RUST_TOOLCHAIN}" package --workspace --exclude pyo3_bindgen_examples --locked
note "running locked Rust ${RUST_TOOLCHAIN} package gate (all features)"
"$CARGO_BIN" "+${RUST_TOOLCHAIN}" package --workspace --exclude pyo3_bindgen_examples --all-features --locked
final_head_commit="$("$GIT_BIN" rev-parse --verify "HEAD^{commit}")" || die "could not re-resolve HEAD"
[[ "$final_head_commit" == "$head_commit" ]] || \
    die "HEAD changed during preflight: started at ${head_commit}, ended at ${final_head_commit}"
final_tree_status="$("$GIT_BIN" status --porcelain=v1 --untracked-files=all)" || \
    die "could not re-inspect working tree"
[[ -z "$final_tree_status" ]] || \
    die "working tree changed during package verification"
final_tag_commit="$("$GIT_BIN" rev-parse --verify "refs/tags/${expected_tag}^{commit}")" || \
    die "local tag ${expected_tag} disappeared during package verification"
[[ "$final_tag_commit" == "$head_commit" ]] || \
    die "local tag ${expected_tag} changed during package verification: now ${final_tag_commit}, expected ${head_commit}"
final_remote_commit="$(release_common_resolve_remote_tag_commit \
    "$GIT_BIN" "$public_remote" "$expected_tag" "public tag")"
[[ "$final_remote_commit" == "$head_commit" ]] || \
    die "public tag ${expected_tag} changed during package verification: now ${final_remote_commit}, expected ${head_commit}"
note "all release preflight gates passed"
