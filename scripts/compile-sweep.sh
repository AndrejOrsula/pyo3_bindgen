#!/usr/bin/env bash

set -Eeuo pipefail
IFS=$'\n\t'

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
readonly SCRIPT_DIR
REPOSITORY_ROOT="$(cd -- "${SCRIPT_DIR}/.." && pwd -P)"
readonly REPOSITORY_ROOT
readonly CARGO_BIN="${CARGO_BIN:-cargo}"
SWEEP_TARGET_DIR="${PYO3_BINDGEN_SWEEP_TARGET_DIR:-${REPOSITORY_ROOT}/target/compile-sweep}"
readonly SWEEP_TARGET_DIR

TIER=all
REQUIRE_HEAVY=0

usage() {
    cat <<'EOF'
Usage: scripts/compile-sweep.sh [--tier stdlib|heavy|all] [--require-heavy]

Generate real Python modules with pyo3_bindgen and assert the output compiles. This is a
thin wrapper around `crates/pyo3_bindgen_engine/tests/compile_sweep.rs`, which owns the target
matrix, so the script and CI can never drift apart.

  --tier stdlib    stdlib targets only (hermetic: no third-party package required)
  --tier heavy     heavy third-party targets only
                   (numpy/pandas/scipy/yaml/requests/gymnasium)
  --tier all       both (default)
  --require-heavy  fail instead of skipping when a heavy package is not importable
  -h, --help       Show this help.

The heavy tier needs:
  python -m pip install numpy==2.2.4 pandas==2.3.3 scipy==1.15.3 \
      PyYAML==6.0.2 requests==2.32.3 gymnasium==1.3.0

Environment:
  CARGO_BIN                      cargo binary (default: cargo)
  PYO3_BINDGEN_SWEEP_TARGET_DIR  target directory shared by the throwaway crates
                                 (default: target/compile-sweep). It is kept between runs
                                 on purpose -- a cold directory rebuilds pyo3.
EOF
}

die() {
    printf 'compile sweep: %s\n' "$*" >&2
    exit 2
}

while (($#)); do
    case "$1" in
        --tier)
            TIER="${2:-}"
            [[ -n ${TIER} ]] || die '--tier requires a value'
            shift 2
            ;;
        --require-heavy)
            REQUIRE_HEAVY=1
            shift
            ;;
        -h | --help)
            usage
            exit 0
            ;;
        *)
            printf 'compile sweep: unknown argument: %s\n\n' "$1" >&2
            usage >&2
            exit 2
            ;;
    esac
done

readonly HEAVY_TEST=heavy_traversed_targets_generate_and_compile

# The tests are `#[ignore]`d, so each tier selects by libtest filter. The stdlib tier is
# expressed as "everything except the heavy test" rather than an explicit name list: a name
# list silently runs NOTHING when a test is renamed, because libtest exits 0 on 0 matches.
case "${TIER}" in
    stdlib) SELECT=(--skip "${HEAVY_TEST}") ;;
    heavy) SELECT=(--exact "${HEAVY_TEST}") ;;
    all) SELECT=() ;;
    *) die "unknown tier: ${TIER}" ;;
esac

mkdir -p "${SWEEP_TARGET_DIR}"
export PYO3_BINDGEN_SWEEP_TARGET_DIR="${SWEEP_TARGET_DIR}"
if ((REQUIRE_HEAVY)); then
    export PYO3_BINDGEN_REQUIRE_HEAVY=1
fi

cd -- "${REPOSITORY_ROOT}"

LOG="$(mktemp)"
readonly LOG
trap 'rm -f -- "${LOG}"' EXIT

# `--test-threads=1` is mandatory: the batches share one cargo target directory.
# `--nocapture` is mandatory too: libtest swallows the `SKIPPED heavy target ...` notices of
# a PASSING test, so without it an all-skipped heavy run is indistinguishable from success.
"${CARGO_BIN}" test -p pyo3_bindgen_engine --test compile_sweep --locked \
    -- --ignored --test-threads=1 --nocapture "${SELECT[@]+"${SELECT[@]}"}" 2>&1 | tee -- "${LOG}"

# libtest exits 0 when a filter matches nothing, so a green exit code alone does not prove
# anything ran. Require at least one executed test.
if grep -qE '^test result: ok\. 0 passed' -- "${LOG}"; then
    die "tier=${TIER} matched no tests -- the filter has drifted from the test names"
fi

printf 'compile sweep OK (tier=%s)\n' "${TIER}"
