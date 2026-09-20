#!/usr/bin/env bash
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd -P)"
REPOSITORY_DIR="$(dirname "${SCRIPT_DIR}")"

if command -v python >/dev/null 2>&1; then
    PYTHON_BIN=python
elif command -v python3 >/dev/null 2>&1; then
    PYTHON_BIN=python3
else
    printf '%s\n' 'python or python3 is required to install pre-commit' >&2
    exit 1
fi

PRE_COMMIT_VERSION=4.6.0

pre_commit_matches() {
    "$1" -c 'import importlib.metadata as metadata, sys; sys.exit(0 if metadata.version("pre-commit") == sys.argv[1] else 1)' "${PRE_COMMIT_VERSION}" >/dev/null 2>&1
}

select_venv_python() {
    local environment="$1"
    if [ -x "${environment}/bin/python" ]; then
        PRE_COMMIT_PYTHON="${environment}/bin/python"
    elif [ -x "${environment}/Scripts/python.exe" ]; then
        PRE_COMMIT_PYTHON="${environment}/Scripts/python.exe"
    else
        return 1
    fi
}

PRE_COMMIT_PYTHON="${PYTHON_BIN}"
if ! pre_commit_matches "${PRE_COMMIT_PYTHON}"; then
    if "${PYTHON_BIN}" -c 'import sys; sys.exit(0 if sys.prefix != getattr(sys, "base_prefix", sys.prefix) else 1)' >/dev/null 2>&1; then
        "${PYTHON_BIN}" -m pip install --upgrade "pre-commit==${PRE_COMMIT_VERSION}"
    else
        if ! command -v git >/dev/null 2>&1; then
            printf '%s\n' 'git is required to create the repository-local pre-commit environment' >&2
            exit 1
        fi
        GIT_DIR="$(git -C "${REPOSITORY_DIR}" rev-parse --absolute-git-dir)"
        PRE_COMMIT_ENV="${GIT_DIR}/pyo3_bindgen-pre-commit-${PRE_COMMIT_VERSION}"
        if ! select_venv_python "${PRE_COMMIT_ENV}"; then
            "${PYTHON_BIN}" -m venv "${PRE_COMMIT_ENV}"
            if ! select_venv_python "${PRE_COMMIT_ENV}"; then
                printf '%s\n' 'failed to create the pre-commit virtual environment' >&2
                exit 1
            fi
        fi
        if ! pre_commit_matches "${PRE_COMMIT_PYTHON}"; then
            "${PRE_COMMIT_PYTHON}" -m pip install --upgrade "pre-commit==${PRE_COMMIT_VERSION}"
        fi
    fi
fi

if ! pre_commit_matches "${PRE_COMMIT_PYTHON}"; then
    printf 'pre-commit %s is unavailable through %s\n' "${PRE_COMMIT_VERSION}" "${PRE_COMMIT_PYTHON}" >&2
    exit 1
fi

# Install local git hooks for this repository
cd "${REPOSITORY_DIR}"
"${PRE_COMMIT_PYTHON}" -m pre_commit install --install-hooks --config "${REPOSITORY_DIR}/.pre-commit-config.yaml"
cd -
