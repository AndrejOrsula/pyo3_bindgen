# Observability contract

The engine emits `tracing` spans/events but never installs a subscriber, exporter, metrics
recorder, or global state. The CLI installs the formatting subscriber. Stable structured events use
`observability_schema=1`; fields outside this catalog, especially debug diagnostics, are not a
machine contract.

## Correlation identity

Successful normalized operations carry one deterministic `operation_id` with the form
`op-v1:fnv1a64:<16 lowercase hex>`. The versioned seed contains source kind, ordered roots, sorted
module/input hashes, bounded external transport identity, source adapter chain, generator version,
and canonicalized output-affecting `Config`. It excludes paths, time, process/thread identity,
locks, candidate choice, diagnostics, and resource measurements.

The ID is a correlation fingerprint, not an authorization or cryptographic integrity token.
Identical source/config operations share an ID. It appears in generation/source/candidate traces,
the additive `GenerationReport.operation_id`, CLI lifecycle events, and publication events. Failures
before normalized source identity exists use `operation_id="unavailable"`; no second random ID is
invented. Generated Rust bytes and provenance constants never contain the operation ID.

For the built-in external-IR route, source events describe an already-materialized `SchemaIr`; the
route does not initialize, attach to, import from, or execute Python. Any custom `BindingSource`
acquisition happens outside this engine execution-free guarantee and may occur before a normalized
source identity or stable operation ID exists.

`Codegen::generate_artifact_with_report_detailed` returns `GeneratedArtifactFailure` so failures
after identity derivation retain that ID in a failure report. The CLI uses this detailed boundary;
the compatibility `generate_artifact_with_report` method continues to return only
`PyBindgenError`.

## Event catalog

| Event/span | Stable bounded fields |
|---|---|
| `pyo3_bindgen.cli.run` | operation ID, source kind, check flag, output/report requested booleans; final events add exit class and operation code (`ok` on success). Pre-identity start/failure uses `unavailable`; successful completion records the derived ID. |
| `normalized source context` | operation ID, source kind, adapter count, bounded transport hash/bytes/limit, limit kind, completeness. |
| `pyo3_bindgen.generate.selection`, `adaptive model selected`, and `generation candidate selected` | operation ID, selected candidate (`single`, `deduplicated`, `flat`), exact canonical candidate bytes, measurement errors by stable error display. The final summary is emitted for single and adaptive generation. At most two candidates. |
| `generation completed` | operation ID, selected candidate, canonical render count, exact preserved module-tree clone count, module/diagnostic/warning/error totals, strict/completeness flag, and aggregate attempted/emitted/skipped/failed/truncated counts for submodules and dependencies. |
| `pyo3_bindgen.publication.output` | operation ID and target kind; nested publication completion records bytes/hash/durability. |
| `artifact targets revalidated` | operation ID, target count, and `ok`/`failed`. |
| `report delivery completed` | operation ID, target kind, recovery boolean, and `published`/`failed`. |
| `publication operation completed` | operation ID, outcome, exit class, and stable operation code. Typed delivery state distinguishes published, possibly published, durability failure, and report recovery. |
| Python coordination/restoration events | These can precede normalized identity and therefore use `unavailable`; expose schema version, bounded wait milliseconds, component, outcome, and restore pass count. Exception details are debug-only. |
| Artifact lock events | The schema-versioned `artifact lock set acquired` summary carries requested target and acquired lock counts; per-lock events carry contention result, path hash, and bounded acquisition milliseconds. Locks are acquired before normalized identity, so they intentionally have no operation ID. |

The live-operation permit coordinates only engine-managed process globals: output streams and the
known standard-library sentinel dictionaries. It is not a transaction or sandbox for arbitrary
Python imports, descriptors, callbacks, finalizers, or other Python effects. Live report metadata
(`platform` and `sys`) is sampled after sentinel restoration and permit release. That placement is
intentional: metadata reads may overlap a later operation's output suppression, and moving them
under the permit would reintroduce the reentrant coordination deadlock this contract avoids.

Completeness uses the existing diagnostic-derived counters; there is no second discovery algorithm.
Best-effort incomplete generation reports `complete=false` and may publish. Strict incomplete
generation fails before output publication. Visible-but-not-durable or uncertain delivery remains
typed partial publication (exit class 3).

## Cardinality and redaction

- Stable fields use allowlisted source kinds, candidates, target kinds, outcomes, durability values,
  and operation codes.
- One generation summary, at most two candidate records, one record per requested delivery, and
  aggregate discovery counts are emitted. No stable per-module/import series exists.
- Raw filesystem/lock/staging paths, module origins, adapter Rust type names, PIDs, thread IDs, and
  exception strings are excluded from the stable catalog. Existing human diagnostics and debug-only
  path fields remain compatibility output, not telemetry labels.
- Content hashes and external SHA-256 identities are retained. Operation IDs and hashes are
  correlation payloads, never metric labels.
- Timing/resource values stay in tracing; deterministic reports contain no timestamps or latency.

## CLI color policy

ANSI is enabled only when stderr is a terminal and `NO_COLOR` is absent. `NO_COLOR` always disables
color. Non-terminal stderr is never colored; `FORCE_COLOR` is ignored. Generated stdout and typed
error protocol output are uncolored. Tests remove ambient color variables and verify the non-TTY
contract directly.

No OpenTelemetry, metrics, or exporter dependency is included because no in-scope collector consumes
one. Applications embedding the engine remain free to install their own subscriber.
