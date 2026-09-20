#!/bin/bash -p
case $- in
    *x*)
        builtin printf 'crate publisher: xtrace (-x) is unsupported; invoke directly or with bash -p\n' >&2
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
    printf 'crate publisher: %s\n' "$*" >&2
    exit 1
}

note() {
    printf 'crate publisher: %s\n' "$*"
}

[[ "${BASH_SOURCE[0]}" == "$0" ]] || die "release entrypoints must be executed, not sourced"

# shellcheck disable=SC1091 # SCRIPT_DIR is resolved at runtime.
source "${SCRIPT_DIR}/release-common.sh"
release_common_require_privileged_mode || die "privileged Bash mode is required"
release_common_sanitize_environment "${SCRIPT_DIR}/${SCRIPT_SOURCE##*/}" "$@" || \
    die "could not sanitize inherited Bash environment"

# Capture the credential without retaining its export attribute. An inherited
# internal variable must be removed first; otherwise Bash preserves export on
# assignment and leaks it to every child process.
unset BASH_ENV
unset RELEASE_REGISTRY_TOKEN
RELEASE_REGISTRY_TOKEN="${CARGO_REGISTRY_TOKEN:-}"
export -n RELEASE_REGISTRY_TOKEN
readonly RELEASE_REGISTRY_TOKEN
unset CARGO_REGISTRY_TOKEN

readonly PREFLIGHT="${SCRIPT_DIR}/release-preflight.sh"
CARGO_BIN="${CARGO_BIN:-cargo}"
PYTHON_BIN="${PYTHON_BIN:-python3}"
GIT_BIN="${GIT_BIN:-git}"
RUSTUP_BIN="${RUSTUP_BIN:-rustup}"
CURL_BIN="${CURL_BIN:-curl}"
readonly RUST_TOOLCHAIN="${RELEASE_RUST_TOOLCHAIN:-${PINNED_RUST_TOOLCHAIN}}"
readonly PYTHON_VERSION="${RELEASE_PYTHON_VERSION:-${PINNED_PYTHON_VERSION}}"
readonly CRATES_IO_API="https://crates.io/api/v1"
readonly POLL_TIMEOUT_SECONDS="${PUBLISH_POLL_TIMEOUT_SECONDS:-300}"
readonly POLL_INTERVAL_SECONDS="${PUBLISH_POLL_INTERVAL_SECONDS:-10}"
readonly CURL_TIMEOUT_SECONDS="${PUBLISH_CURL_TIMEOUT_SECONDS:-30}"
readonly -a PACKAGES=("${RELEASE_PACKAGES[@]}")

CHECK_ONLY=false
REQUESTED_TAG=""
release_head=""
REMOTE_STATE=""
REMOTE_CHECKSUM=""
TEMP_FILES=()
LOCAL_CHECKSUMS=()
PUBLISH_NEEDED=()

usage() {
    cat <<'EOF'
Usage: scripts/publish-workspace.sh [--check] [--tag vX.Y.Z]

Publish the four pyo3_bindgen crates in dependency order. Normal mode runs the
full release preflight first. Reruns are safe only when crates.io's checksum for
an already-published exact version equals the locally packaged .crate checksum.

  --check       Validate metadata and print the publish plan only. The local
                Rust toolchain is checked before Cargo; no token, network
                access, build, or publication occurs.
  --tag TAG     Require TAG to equal v<workspace version> and pass it through to
                the release preflight.
  -h, --help    Show this help.

Polling can be tuned with PUBLISH_POLL_TIMEOUT_SECONDS,
PUBLISH_POLL_INTERVAL_SECONDS, and PUBLISH_CURL_TIMEOUT_SECONDS.
Normal releases require the checked-in Rust 1.97.0 and shared CPython 3.10.21
pins. --check accepts exact-version overrides for local structural validation.
EOF
}

cleanup() {
    if ((${#TEMP_FILES[@]} > 0)); then
        rm -f -- "${TEMP_FILES[@]}"
    fi
}
trap cleanup EXIT

is_positive_integer() {
    [[ "$1" =~ ^[1-9][0-9]*$ ]]
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

is_positive_integer "$POLL_TIMEOUT_SECONDS" || \
    die "PUBLISH_POLL_TIMEOUT_SECONDS must be a positive integer"
is_positive_integer "$POLL_INTERVAL_SECONDS" || \
    die "PUBLISH_POLL_INTERVAL_SECONDS must be a positive integer"
is_positive_integer "$CURL_TIMEOUT_SECONDS" || \
    die "PUBLISH_CURL_TIMEOUT_SECONDS must be a positive integer"

release_common_resolve_executable "$CARGO_BIN" CARGO_BIN
release_common_resolve_executable "$PYTHON_BIN" PYTHON_BIN
release_common_resolve_executable "$GIT_BIN" GIT_BIN
release_common_resolve_executable "$RUSTUP_BIN" RUSTUP_BIN
release_common_resolve_executable "$CURL_BIN" CURL_BIN
readonly CARGO_BIN PYTHON_BIN GIT_BIN RUSTUP_BIN CURL_BIN
export CARGO_BIN PYTHON_BIN GIT_BIN RUSTUP_BIN
release_common_reject_pyo3_environment_overrides
release_common_validate_release_versions "$CHECK_ONLY" "$RUST_TOOLCHAIN" "$PYTHON_VERSION"
release_common_validate_python "$PYTHON_BIN" "$PYTHON_VERSION" false
[[ -x "$PREFLIGHT" ]] || die "release preflight is not executable: ${PREFLIGHT}"

preflight_args=()
if [[ "$CHECK_ONLY" == true ]]; then
    preflight_args+=(--check)
fi
if [[ -n "$REQUESTED_TAG" ]]; then
    preflight_args+=(--tag "$REQUESTED_TAG")
fi
if [[ "$CHECK_ONLY" == false ]]; then
    release_head="$("$GIT_BIN" -C "$REPOSITORY_ROOT" rev-parse --verify 'HEAD^{commit}')" || \
        die "could not resolve release HEAD before preflight"
fi
if [[ "$CHECK_ONLY" == false ]]; then
    CARGO_REGISTRY_TOKEN="$RELEASE_REGISTRY_TOKEN" \
        "$PREFLIGHT" "${preflight_args[@]}"
else
    "$PREFLIGHT" "${preflight_args[@]}"
fi

cd -- "$REPOSITORY_ROOT"
if [[ "$CHECK_ONLY" == false ]]; then
    post_preflight_head="$("$GIT_BIN" rev-parse --verify 'HEAD^{commit}')" || \
        die "could not resolve release HEAD after preflight"
    [[ "$post_preflight_head" == "$release_head" ]] || \
        die "HEAD changed across preflight: started at ${release_head}, ended at ${post_preflight_head}"
fi

metadata_line="$(release_common_workspace_metadata "$CARGO_BIN" "$PYTHON_BIN" "$RUST_TOOLCHAIN" release "$RUSTUP_BIN")" || \
    die "could not read release package metadata"

IFS=$'\t' read -r workspace_version target_directory repository_url <<<"$metadata_line"
[[ -n "$workspace_version" && -n "$target_directory" && -n "$repository_url" ]] || \
    die "incomplete package metadata"

expected_tag="v${workspace_version}"
if [[ -n "$REQUESTED_TAG" && "$REQUESTED_TAG" != "$expected_tag" ]]; then
    die "requested tag ${REQUESTED_TAG} does not equal workspace tag ${expected_tag}"
fi

repository_url="$(release_common_validate_public_repository_url "$repository_url")"
readonly public_remote="${repository_url}.git"

SHA256_TOOL=""
SHA256_BIN=""
for checksum_tool in sha256sum shasum openssl; do
    if checksum_candidate="$(builtin type -P -- "$checksum_tool")"; then
        release_common_resolve_executable "$checksum_candidate" SHA256_BIN
        SHA256_TOOL="$checksum_tool"
        break
    fi
done
[[ -n "$SHA256_TOOL" && -n "$SHA256_BIN" ]] || \
    die "required SHA-256 tool not found (sha256sum, shasum, or openssl)"
readonly SHA256_TOOL SHA256_BIN

if [[ "$CHECK_ONLY" == true ]]; then
    note "check OK: would publish version ${workspace_version} with Rust ${RUST_TOOLCHAIN} and Python ${PYTHON_VERSION} in this order:"
    printf '  %s\n' "${PACKAGES[@]}"
    exit 0
fi

sha256_file() {
    local file="$1"
    local checksum

    case "$SHA256_TOOL" in
        sha256sum)
            checksum="$("$SHA256_BIN" -- "$file")"
            checksum="${checksum%% *}"
            ;;
        shasum)
            checksum="$("$SHA256_BIN" -a 256 -- "$file")"
            checksum="${checksum%% *}"
            ;;
        openssl)
            checksum="$("$SHA256_BIN" dgst -sha256 -r -- "$file")"
            checksum="${checksum%% *}"
            ;;
        *)
            die "internal error: unknown SHA-256 tool"
            ;;
    esac
    [[ "$checksum" =~ ^[0-9a-fA-F]{64}$ ]] || die "invalid local SHA-256 for ${file}"
    printf '%s\n' "${checksum,,}"
}

verify_release_identity() {
    local context="$1"
    local public_tag_commit current_head tree_status current_tag

    public_tag_commit="$(
        unset CARGO_REGISTRY_TOKEN
        release_common_resolve_remote_tag_commit \
            "$GIT_BIN" "$public_remote" "$expected_tag" "${context}: advertised-public tag"
    )"
    [[ "$public_tag_commit" == "$release_head" ]] || \
        die "${context}: advertised-public tag ${expected_tag} changed to ${public_tag_commit}; expected ${release_head}"

    current_head="$("$GIT_BIN" rev-parse --verify "HEAD^{commit}")" || \
        die "${context}: could not resolve HEAD"
    tree_status="$("$GIT_BIN" status --porcelain=v1 --untracked-files=all)" || \
        die "${context}: could not inspect working tree"
    current_tag="$("$GIT_BIN" rev-parse --verify "refs/tags/${expected_tag}^{commit}")" || \
        die "${context}: local tag ${expected_tag} is missing"
    [[ "$current_head" == "$release_head" ]] || \
        die "${context}: HEAD changed to ${current_head}; expected ${release_head}"
    [[ -z "$tree_status" ]] || \
        die "${context}: working tree is not clean"
    [[ "$current_tag" == "$release_head" ]] || \
        die "${context}: local tag ${expected_tag} changed to ${current_tag}; expected ${release_head}"
}

fetch_remote_checksum() {
    local package="$1"
    local version="$2"
    local body_file http_status version_state checksum

    body_file="$(mktemp)"
    TEMP_FILES+=("$body_file")
    if ! http_status="$("$CURL_BIN" --silent --show-error --location \
        --connect-timeout "$CURL_TIMEOUT_SECONDS" \
        --max-time "$CURL_TIMEOUT_SECONDS" \
        --output "$body_file" --write-out '%{http_code}' \
        --user-agent "pyo3_bindgen-release-publisher/${version}" \
        "${CRATES_IO_API}/crates/${package}/${version}")"; then
        return 1
    fi

    case "$http_status" in
        200)
            version_state="$("$PYTHON_BIN" -c '
import json
import re
import sys

with open(sys.argv[1], "rb") as response:
    version = json.load(response)["version"]
value = version["checksum"]
if not isinstance(value, str) or not re.fullmatch(r"[0-9a-fA-F]{64}", value):
    raise SystemExit("crates.io returned an invalid checksum")
state = "yanked" if version.get("yanked") else "active"
print(state + ":" + value.lower())
' "$body_file")" || return 1
            if [[ "$version_state" == yanked:* ]]; then
                die "${package} ${version} exists on crates.io but is yanked; resolve that release state manually"
            fi
            [[ "$version_state" == active:* ]] || return 1
            checksum="${version_state#active:}"
            REMOTE_STATE=exists
            REMOTE_CHECKSUM="$checksum"
            ;;
        404)
            REMOTE_STATE=missing
            REMOTE_CHECKSUM=""
            ;;
        *)
            printf 'crate publisher: crates.io returned HTTP %s for %s %s\n' \
                "$http_status" "$package" "$version" >&2
            return 1
            ;;
    esac
}

wait_for_exact_checksum() {
    local package="$1"
    local version="$2"
    local local_checksum="$3"
    local deadline=$((SECONDS + POLL_TIMEOUT_SECONDS))

    while ((SECONDS < deadline)); do
        if fetch_remote_checksum "$package" "$version"; then
            if [[ "$REMOTE_STATE" == exists ]]; then
                if [[ "$REMOTE_CHECKSUM" == "$local_checksum" ]]; then
                    if CARGO_HTTP_TIMEOUT="$CURL_TIMEOUT_SECONDS" \
                        "$CARGO_BIN" "+${RUST_TOOLCHAIN}" info "${package}@${version}" \
                        --registry crates-io >/dev/null 2>&1; then
                        note "crates.io API and registry index confirm ${package} ${version} checksum ${local_checksum}"
                        return 0
                    fi
                    note "${package} ${version} has reached the API but not the Cargo registry index; retrying"
                fi
                if [[ "$REMOTE_CHECKSUM" != "$local_checksum" ]]; then
                    die "checksum mismatch for ${package} ${version}: crates.io=${REMOTE_CHECKSUM} local=${local_checksum}"
                fi
            fi
        else
            note "transient crates.io query failure for ${package} ${version}; retrying"
        fi
        sleep "$POLL_INTERVAL_SECONDS"
    done
    return 1
}

note "checking every local archive against crates.io before any publication"
for package in "${PACKAGES[@]}"; do
    crate_file="${target_directory}/package/${package}-${workspace_version}.crate"
    [[ -f "$crate_file" ]] || \
        die "preflight did not produce expected package archive: ${crate_file}"
    local_checksum="$(sha256_file "$crate_file")"
    LOCAL_CHECKSUMS+=("$local_checksum")

    if ! fetch_remote_checksum "$package" "$workspace_version"; then
        die "could not determine crates.io state for ${package} ${workspace_version}"
    fi

    if [[ "$REMOTE_STATE" == exists ]]; then
        if [[ "$REMOTE_CHECKSUM" == "$local_checksum" ]]; then
            note "verified ${package} ${workspace_version}: exact checksum already published"
            PUBLISH_NEEDED+=(false)
        else
            die "refusing release: ${package} ${workspace_version} has crates.io=${REMOTE_CHECKSUM} local=${local_checksum}"
        fi
    else
        PUBLISH_NEEDED+=(true)
    fi
done

for index in "${!PACKAGES[@]}"; do
    package="${PACKAGES[$index]}"
    local_checksum="${LOCAL_CHECKSUMS[$index]}"
    crate_file="${target_directory}/package/${package}-${workspace_version}.crate"

    if [[ "${PUBLISH_NEEDED[$index]}" == false ]]; then
        wait_for_exact_checksum "$package" "$workspace_version" "$local_checksum" || \
            die "existing ${package} ${workspace_version} did not reach the Cargo registry index within ${POLL_TIMEOUT_SECONDS}s"
        note "skip ${package} ${workspace_version}: exact checksum already published"
        continue
    fi

    # Close the race between the all-package scan and this upload. A concurrent
    # matching upload is safe to skip; any other exact-version upload is fatal.
    if ! fetch_remote_checksum "$package" "$workspace_version"; then
        die "could not recheck crates.io state for ${package} ${workspace_version}"
    fi
    if [[ "$REMOTE_STATE" == exists ]]; then
        if [[ "$REMOTE_CHECKSUM" == "$local_checksum" ]]; then
            wait_for_exact_checksum "$package" "$workspace_version" "$local_checksum" || \
                die "matching concurrent upload did not reach the Cargo registry index within ${POLL_TIMEOUT_SECONDS}s"
            note "skip ${package} ${workspace_version}: matching upload appeared concurrently"
            continue
        fi
        die "refusing concurrent version conflict for ${package} ${workspace_version}: crates.io=${REMOTE_CHECKSUM} local=${local_checksum}"
    fi

    current_checksum="$(sha256_file "$crate_file")" || \
        die "could not rehash package archive: ${crate_file}"
    [[ "$current_checksum" == "$local_checksum" ]] || \
        die "package archive changed before publishing ${package}: now=${current_checksum} expected=${local_checksum}"
    verify_release_identity "before publishing ${package} ${workspace_version}"

    note "publishing ${package} ${workspace_version}"
    if ! CARGO_REGISTRY_TOKEN="$RELEASE_REGISTRY_TOKEN" \
        "$CARGO_BIN" "+${RUST_TOOLCHAIN}" publish --package "$package" --registry crates-io --locked; then
        note "cargo publish failed; checking whether an identical upload became visible"
        if wait_for_exact_checksum "$package" "$workspace_version" "$local_checksum"; then
            continue
        fi
        die "publish failed and no matching crates.io release appeared for ${package} ${workspace_version}"
    fi

    wait_for_exact_checksum "$package" "$workspace_version" "$local_checksum" || \
        die "timed out after ${POLL_TIMEOUT_SECONDS}s waiting for ${package} ${workspace_version} on crates.io"
done

verify_release_identity "after confirming workspace ${workspace_version}"
note "all release crates are present on crates.io with matching checksums"
