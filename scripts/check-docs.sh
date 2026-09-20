#!/usr/bin/env bash

set -Eeuo pipefail
IFS=$'\n\t'

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
readonly SCRIPT_DIR
REPOSITORY_ROOT="$(cd -- "${SCRIPT_DIR}/.." && pwd -P)"
readonly REPOSITORY_ROOT
PYTHON_BIN="${PYTHON_BIN:-python3}"

"$PYTHON_BIN" - "$REPOSITORY_ROOT" <<'PY'
from pathlib import Path
import re
import subprocess
import sys

root = Path(sys.argv[1]).resolve()
docs = root / "docs"
index = docs / "README.md"
link_pattern = re.compile(r"(?<!!)\[[^]]*\]\(([^)]+)\)")


def local_markdown_targets(source: Path):
    text = source.read_text(encoding="utf-8")
    for raw in link_pattern.findall(text):
        target = raw.strip().split(maxsplit=1)[0].strip("<>")
        if target.startswith(("http://", "https://", "mailto:", "#")):
            continue
        path_part = target.split("#", 1)[0]
        if path_part.endswith(".md"):
            yield target, (source.parent / path_part).resolve()


indexed = {target for _, target in local_markdown_targets(index)}
# Include nested and new documentation, but not ignored build/tool output.
paths = subprocess.check_output(
    ["git", "-C", str(root), "ls-files", "--cached", "--others", "--exclude-standard", "-z", "--", "*.md"]
).decode("utf-8").split("\0")
documents = {
    (root / path).resolve() for path in paths if path and (root / path).is_file()
}
active = documents - {index.resolve()}
missing_from_index = sorted(active - indexed)
if missing_from_index:
    names = ", ".join(path.relative_to(root).as_posix() for path in missing_from_index)
    raise SystemExit(f"Markdown documents missing from docs/README.md: {names}")

plan = docs / "release_checklist.md"
if not plan.is_file() or plan.resolve() not in indexed:
    raise SystemExit("docs/release_checklist.md must exist and be indexed as the release plan")

errors = []
for source in sorted({index.resolve(), *documents, *indexed}):
    try:
        source.relative_to(root)
    except ValueError:
        errors.append(f"indexed document escapes repository: {source}")
        continue
    if not source.is_file():
        errors.append(f"indexed document does not exist: {source.relative_to(root)}")
        continue
    for raw, target in local_markdown_targets(source):
        try:
            target.relative_to(root)
        except ValueError:
            errors.append(f"{source.relative_to(root)}: local link escapes repository: {raw}")
            continue
        if not target.is_file():
            errors.append(f"{source.relative_to(root)}: missing local Markdown target: {raw}")

if errors:
    raise SystemExit("\n".join(errors))

print(f"documentation contract OK: {len(active)} documents indexed; local Markdown links valid")
PY
