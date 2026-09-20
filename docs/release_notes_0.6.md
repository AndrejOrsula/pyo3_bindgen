# pyo3_bindgen 0.6 Release Notes

Version 0.6 restores the generator around PyO3 0.29, makes generated artifacts
and reports reproducible, and adds a Python-free external-IR input path. Generated
code checked in from an older release must be regenerated.

## Compatibility

- Generated bindings target the PyO3 `0.29` API and the workspace pins
  `pyo3 = 0.29.2`.
- The Rust MSRV is 1.83. Release CI checks Rust 1.83, stable, and beta; package
  publication is reproducibly pinned to Rust/Cargo 1.97.0 and CPython 3.10.21.
- The live-introspection surface targets CPython 3.8 through 3.14. Routine CI
  configures 3.8, 3.10, 3.14, and free-threaded 3.14 on Linux. Windows GNU/MSVC
  and Apple ARM64/x86_64 lanes compile only. The maintainer limits this release's
  Windows/macOS assurance to cross-compilation. Native Windows/macOS/UNC path,
  ACL, locking, replacement, and durability behavior remains unverified; see the
  [release plan](release_checklist.md). Configured lanes are not passing evidence.
- Python 3.14 deferred annotations use public `annotationlib.Format.STRING`
  introspection, preserving callable shape and typed lowering. Python may execute
  annotation metadata even in STRING mode; live introspection requires trusted
  inputs and is not side-effect-free. The generator does not evaluate captured
  annotation strings. See the
  [Python annotation security guidance](https://docs.python.org/3.14/library/annotationlib.html#security-implications-of-introspecting-annotations).
- The `abi3-py310` configuration has its own test and warning-denying Clippy
  gate.
- CI denies warnings across the complete workspace feature powerset. Optional
  NumPy and complex mappings also compile in locked standalone consumer fixtures
  on both the MSRV and stable toolchains.
- PyO3 cannot initialize an embedded PyPy or GraalPy interpreter, so live module
  introspection and `import_python!` are not supported there. CI compiles the
  supported library surface and executes the Python-free `--ir` CLI path with
  PyPy 3.11. A separate warning-denying cross-check compiles the Python-free
  engine, non-macro facade, CLI, and macro crate against PyO3's
  `GraalVM`/Python 3.11 configuration without invoking live introspection.
- Gymnasium dogfood is compatibility evidence, not product policy. The release gate imports the pinned
  Gymnasium 1.3.0 surface on CPython 3.10; the generator remains generic and
  package-agnostic.

## Generated API breakage

The output now follows PyO3 0.29's `Bound`, `Borrowed`, `FromPyObject`, and
`IntoPyObject` APIs. Regeneration can therefore change imports, conversion impls,
method signatures, helper names, enum types, and formatted bytes. Treat a
regenerated diff as an API migration and compile downstream crates before
accepting it. In particular, `bind`, `as_any`, and `clone_ref` moved off each
deduplicated wrapper's inherent `impl` onto the shared `PyObjectBase` /
`PyTypeMarker` traits, so call sites must bring those traits into scope.
`from_py_any` and `into_py_any` stay inherent under every model (see "Wrapper
semantics").

## Wrapper semantics

Each generated Python class is an owning newtype around `Py<PyAny>`.
`from_py_any` and `FromPyObject` verify the concrete Python class at runtime and
return `PyTypeError` on mismatch. `bind`/`as_any` borrow the object under a
`Python<'py>` token, `clone_ref` performs an explicit Python reference-counted
clone, and `into_py_any` transfers ownership. Owned and borrowed wrappers
implement PyO3 0.29 `IntoPyObject`; generated constructors, methods, and property
accessors preserve Python exceptions as `PyResult` instead of hiding failures.
Descriptor inspection preserves instance, static, and class-method semantics,
including built-in class-method descriptors. External IR uses Python's bound
class-method signature and requires explicit positional receivers only for
instance methods, constructors, and callable objects.

Where the wrapper boilerplate lives depends on the object model. A **flat**
wrapper (the non-deduplicated live path, or a live class with no in-scope
ancestor or descendant) carries all five helpers as inherent methods, unchanged.
A **deduplicated** wrapper (external IR, and live classes in a deduplicated
hierarchy) already implements the generated root trait `PyObjectBase` and the
type marker `PyTypeMarker`, so three class-independent helpers are emitted
once as `#[inline]` default methods there instead of once per class:
`PyObjectBase::bind`, `PyObjectBase::as_any` and `PyTypeMarker::clone_ref`
(`PyTypeMarker` therefore now requires `PyObjectBase`). Both traits are `pub` at
the generated root (the same place the `*Methods` traits are reachable from), so
a consumer writes `use bindings::{PyObjectBase as _, PyTypeMarker as _};` to
call them. The behavior of all three is unchanged.

`from_py_any` and `into_py_any` stay **inherent** on every wrapper under both
models. `from_py_any` names the concrete class and its runtime import.
`into_py_any` is held back deliberately: PyO3's `IntoPyObjectExt` has a blanket
`impl<T: IntoPyObject> IntoPyObjectExt for T`, so a same-named method on
`PyTypeMarker` would make `wrapper.into_py_any()` ambiguous
(`error[E0034]: multiple applicable items in scope`) for any consumer that
imports `IntoPyObjectExt`. Keeping it inherent preserves this crate's ambiguity
rule — an inherent method always wins resolution against a same-named trait
method — and it is unchanged from earlier releases. Generic code over the marker
trait uses `PyTypeMarker::__into_obj`, which is the same operation.

The external-IR class wrapper is `#[repr(transparent)]`, which supports its
generated zero-cost ancestor upcasts. External IR provides each class's direct
bases in declaration order; the engine validates them and expands the full
nearest-first transitive ancestry. Every base must resolve to another generated
class. Missing, non-class, and cyclic ancestry is rejected before code emission.

## Enum mirror behavior

Closed `Enum`, `IntEnum`, and `StrEnum` classes become Rust enums. Canonical
Python members become variants; Python aliases become associated constants that
point at the canonical variant. `as_py_name` and `from_py_name` preserve the
Python spelling, including aliases, and PyO3 conversions import the actual enum
class and validate its runtime type. Conversion never silently substitutes a
string. A consumer whose Python representation has no importable backing enum
class (external IR only, e.g. Blender RNA enum properties) opts an enum into
round-tripping through its member name string instead, via the document-wide
`--enum-string-backed` flag or the equivalent per-enum `EnumIr::string_backed`
field; either source enables it (logical OR) for a given enum, and the field
lets one document mix string-backed and class-backed enums.

A live `Flag`/`IntFlag` class is a value type rather than a fixed set of names, so
it mirrors as a **value newtype struct** over `u64` instead of a Rust `enum`.
Every canonical member becomes a bit-valued associated constant (composite members
such as `RW = 3` included) and every alias becomes a constant pointing at its
canonical target; constants keep the same identifier convention as plain-mirror
variants. The struct exposes `value`, `contains` and `is_empty` (all `const fn`)
plus `core::ops::BitOr`, `BitAnd` and `BitXor` for union, intersection and
symmetric difference. Two omissions are deliberate:

- **No `core::ops::Not`.** The complement of a flag set depends on the Python
  class's boundary policy (`STRICT`, `KEEP`, `CONFORM`), which only Python can
  apply; a Rust-side `!` would invent bits the class may reject.
- **No `as_py_name`/`from_py_name`** — the asymmetry with plain mirrors. A flag
  *value* such as `R|W` need not correspond to any single named Python member, so
  there is no canonical name to expose.

Conversions keep the plain-mirror contract: `FromPyObject` verifies the object is
an instance of the mirrored Python class (`PyTypeError` otherwise) and then reads
its `.value`; `IntoPyObject` imports the actual class and **calls** it with the
value, so Python itself applies the boundary policy and raises for out-of-boundary
bits. The mirror never fabricates a member.

Support is deliberately narrow: only a flag class whose members are all plain
non-negative `int`s fitting `u64`. Negative values, values above `u64::MAX`,
non-`int` values, and classes whose members cannot be introspected all keep the
previous behavior — the owning Python-object wrapper plus the
`enum_flag_fallback` warning, whose message now names the value constraint and the
introspection caveat.
`Config::enum_raw_fallback_paths` forces the same wrapper for named supported
enums and emits `enum_configured_raw_fallback`. External IR with `is_flag` or
`is_open` is rejected unless the enum sets `plain_mirror: true` (an explicit
producer opt-in to a plain mirror enum with the flag/open semantics dropped) or
lenient external IR is enabled; otherwise it must describe the value as a class
wrapper instead. External-IR flag enums do **not** map to the newtype mirror: that
path keeps its plain-mirror/lenient handling. If an otherwise supported live enum
cannot be mirrored safely, generation emits `enum_mirror_fallback` and keeps the
raw wrapper.

Enum reports include canonical Python/Rust names, aliases and their canonical
targets, the backing kind, and deterministic value descriptions. A mirrored flag
class reports the backing kind `Flag` or `IntFlag`. This is an **additive** change
to the existing `kind` string: the report schema version stays 1, and a consumer
that only knows `Enum`/`IntEnum`/`StrEnum` still parses the report unchanged — it
simply encounters a `kind` value it does not recognize. Live values
retain exact stable scalars, recursively normalized tuples, and sorted frozen
sets. Other objects use `<module.qualified_type instance>`; aliases reuse their
canonical value. External-IR `value_repr` remains caller-controlled.

## Names and type lowering

Definitions and references share one collision-aware naming plan. Python
callables that collide with generated helpers receive deterministic `py_...`
names; irreconcilable type/value namespace collisions are errors, including
identifiers that Rust normalizes to the same Unicode NFC spelling. Sanitized
local definitions and import aliases resolve through their actual emitted Rust
paths, while references to configuration-disabled items fall back to dynamic
types. Dynamic dictionary content is tracked semantically, local generated types
outrank optional adapters, and the NumPy adapter matches `numpy.ndarray` exactly.
Opaque live annotation objects lower to the untyped fallback instead of
injecting address-bearing `repr` text; real Python types, strings, `typing`, and
PEP 585/604 annotations keep structural lowering. Bare `typing` collection
aliases normalize to untyped containers consistently across CPython 3.8 and
newer instead of exposing 3.8's synthetic `~T` parameter.

Python `bytes` is hashable in typed external-IR containers. `dict[bytes, V]`
therefore lowers to `HashMap<Vec<u8>, V>` when `V` is typed, and `set[bytes]`
lowers to `HashSet<Vec<u8>>`, for both parameters and returns. This changes
generated signatures for those shapes. `bytearray` and containers with dynamic
content retain their native `PyDict`/`PySet` fallback.

## Report format (schema version 1)

`Codegen::generate_with_report` and `Codegen::build_with_report` expose
`GenerationReport`. The CLI writes the same structure as pretty JSON when
`--report PATH` is present. Its schema-version-1 top level contains `generator`,
optional `python`, `modules`, `diagnostics`, and `outputs`.

Version 1 permits non-reserved producer extensions flattened at the report root,
matching `GenerationReportDocument`; typed nested report objects remain closed.
This preserves additive producer metadata without a schema-version change, at
the accepted cost that a schema consumer cannot distinguish an unknown root
extension from a misspelled root key. Rust serialization still rejects extension
keys that collide with fields owned by `GenerationReport`.

Each module records its source kind, stable input fingerprint, optional source
hash, emitted item counts, and emitted enum details. Each diagnostic records
severity, stable code, optional Python path, and message. Each output records
kind, display path, byte count, and stable hash. Paths are human-readable and
may be lossy for non-Unicode OS paths.

- `generate_with_report` describes the unformatted `token_stream`.
- `build_with_report(PATH)` describes the exact canonical bytes in `PATH` as one
  `file` output.
- CLI stdout generation reports one `stdout` output. `--output FILE` and
  `--output-dir DIR` report the exact file bytes (`DIR/bindings.rs`).
- `--check` requires a file output and reports it only when existing bytes match.
- Parse, generation, warning-policy, drift, and delivery failures do not claim a
  delivered output; their `outputs` array is empty.

Imported live-module `input_hash` values now use version 3 of a deterministic
JSON fingerprint over the ordered inspected module tree, including direct class
bases, exact Python/Rust identifier spellings, and property-getter presence.
This intentionally changes prior live hashes once. Embedded-source hashes remain
hashes of raw source bytes. Authored external-IR modules hash their deterministic
parsed-IR JSON; synthetic or merged parent modules use a versioned composite
fingerprint over their node input and ordered child hashes, so authored or child
changes and sibling reordering remain visible.

## Artifact delivery

Build-script and CLI output use the same canonical Rust formatting, stable hash,
drift check, and atomic replacement implementation. Missing parent directories
are created. Existing Unix permissions and Windows ACLs are retained; new Unix
files use `0600`. Final symlink destinations are rejected. Through these
build-script and CLI publication paths, Windows artifact paths reject Win32
aliases, reserved device names, forbidden/control characters, leading/trailing
spaces, trailing dots, and UTF-16 NUL before any filesystem call. The IR input,
output, and report paths must resolve to three distinct artifacts. Per-artifact
locks serialize cooperating writers using the same account on the same host and
lock-root namespace, including hard-link and same-directory aliases. They do
not coordinate across accounts, hosts, or separate Unix mount namespaces.
Path revalidation and atomic publication assume the IR, output, and report parent
directories are trusted and are not concurrently replaced or renamed by an
uncooperative process. If Windows reports that a partial replacement may have
moved or removed the destination, the synchronized staged replacement is preserved
and its recovery path is returned in the error.

When the CLI publishes an output or report but the platform cannot prove parent-
directory synchronization, it preserves the published receipt and returns exit
class 3 for partial publication with the existing typed durability failure code.
Direct `RustArtifact`/`TextArtifact` writes and `Codegen::build` retain their
compatibility behavior; their detailed receipts still expose
`PlatformUnsupported` to callers.

## Warning policy and known fallbacks

Warnings are always present in `diagnostics` and do not fail generation by
default. `--deny-warnings` fails when at least one warning exists and appends a
`warning_policy` error. `--warning-budget N` fails only when the warning count is
greater than `N` and appends `warning_budget_exceeded`. When either policy fails,
the report is still written when possible, generated code is not delivered, and
`outputs` stays empty.

Warnings identify deliberate degradation: invalid names that were renamed or
skipped, dynamic or ambiguous annotations lowered to `PyAny`, incomplete
signatures, unsupported live enum semantics, and best-effort dependency imports.
External IR is stricter: malformed ownership, duplicate or colliding symbols,
malformed parameter signatures or preludes, unsupported enum semantics (unless
the enum opts in via `plain_mirror` or lenient external IR is enabled), and
invalid ancestry are errors rather than inferred behavior. A required
positional parameter following a defaulted one is accepted unconditionally
(`has_default` is emission-metadata only, so parameter order never reaches the
emitted Rust signature) and is no longer one of these malformed-signature
errors.

## Live-source boundaries

Live module paths may be registered only once per `Codegen`; duplicate embedded
sources are rejected before the second source executes. Invalid C-string inputs,
imports, and introspection failures return errors instead of panicking.
Submodules and members have deterministic ordering. Requested import aliases keep
their resolved `__name__`, while embedded sources must retain their registered
identity. Python stdout/stderr redirection is process-global: calls coordinate
access, same-thread nesting is reentrant, and cross-thread contention returns a
bounded error instead of racing or deadlocking.

Dependency discovery applies access policy to the resolved module object's dotted
`__name__`, not a class's caller-controlled `__module__`. A public-looking class
alias that resolves to a module with a private path segment is skipped by default
and is emitted only when `include_private` is enabled.

Live generation supports one main CPython interpreter identity for the process
lifetime. Finalization, reinitialization, interpreter switching, and
subinterpreters are unsupported; use a fresh process for lifecycle isolation.
Multiple live callers and threads are supported only while attached to that same
interpreter. Process-global caches and coordination are not keyed by interpreter
identity and do not sandbox arbitrary Python effects.

On Unix CPython, `import_python!` caches one best-effort `RTLD_GLOBAL` preload of
the interpreter library. The path comes from `sysconfig` and covers ordinary
shared-library installs, absolute install names, and macOS framework layouts.
Static-only Python builds and incomplete metadata produce an explicit compiler
warning; the requested module introspection still supplies the final success or
located compile error.
