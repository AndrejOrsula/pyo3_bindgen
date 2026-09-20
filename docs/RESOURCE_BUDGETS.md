# Generation resource budgets

`GenerationBudget` bounds the canonical generation phase reached by terminal
`Codegen::generate`, `generate_with_report`, `build`, or `build_with_report`. It does not bound the
whole `build` call: publication planning, path validation, and publication-session lock acquisition
(including lock wait) happen before budget tracking, while output and any report-file filesystem
delivery happen after generation. Pre-generation source acquisition and parser/materialization are
also outside the budget; live work performed inside terminal generation falls within the measured
phase and its cooperative checks. Select it with `Codegen::with_budget`; it is intentionally
separate from `Config` and has no CLI flag.

The limits cover generation after source acquisition, not every allocation made by the process.
The boundaries are:

- **Transport:** For CLI external IR, the reader is given a `limit + 1` byte sentinel window. The
  CLI rejects `raw_bytes.len() > limit` before JSON decoding or hashing, so `limit` is the accepted
  maximum; an oversized input can be retained transiently with that one sentinel byte. This
  transport check does not bound parser working memory or the decoded graph. `BindingSource` has
  no transport receipt or transport-byte budget.
- **Parser and materialization:** Decoding can hold raw bytes, parser values, and the resulting
  `SchemaIr` at the same time, so materialized memory can exceed the raw-byte cap. Programmatic
  callers and `BindingSource` adapters provide an already-materialized source; source construction,
  custom acquisition, and any pre-operation allocation needed to retain it occur outside
  `GenerationBudget`.
- **Structural generation:** The generation budget checks the complete materialized graph before
  recursive external-IR validation/lowering, and checks the normalized candidate set again. These
  are iterative shape/count checks, not byte limits over the wire, parser values, or allocator
  overhead.
- **Output:** The canonical-output limit applies to exact `prettyplease`-formatted Rust bytes. In
  adaptive live generation it is candidate-local: an oversized candidate may lose to one that fits.
- **RSS:** The peak-RSS baseline is sampled when terminal generation starts, after pre-generation
  source loading and external-IR decoding/materialization. The delta therefore measures later
  process high-water growth from work performed inside terminal generation. A live module may have
  been imported or introspected before this baseline; a module-name or `LiveModules` value is only a
  source descriptor until registration resolves it, while dependency or context acquisition can
  import more modules during terminal generation. RSS is a process-wide observation, not allocator
  attribution to this operation or an allocation cap. It does not retroactively include memory
  already consumed by transport buffers, parser values, or a programmatic `SchemaIr`.

The generation budget is therefore not a second transport-byte limit, and a successful RSS check
does not prove that total process memory stayed below the configured delta.

| Resource | Default | Measurement |
|---|---:|---|
| Definitions | 100,000 | Modules, classes, enums, enum variants/aliases, functions, properties, and type variables in the complete graph. Imports and preludes are references, not definitions. |
| Recursion depth | 32 | Maximum explicit module nesting or dotted-path expansion, parsed type-relation depth, and class-inheritance chain in external IR or live acquisition with inherited-member deduplication enabled. Non-deduplicated live acquisition retains direct-base identity but does not materialize a transitive hierarchy for this limit. Existing parser hard guards remain last-resort ceilings above this default. |
| Normalized model nodes | 1,000,000 | Domain entities and type-relation nodes in the complete raw or normalized graph; field labels and string bytes are not nodes. |
| Canonical output | 512 MiB | Exact `prettyplease`-formatted Rust bytes. In adaptive live generation this is candidate-local: an oversized candidate may lose to one that fits. |
| Wall time | 300 s | Cooperative elapsed time checked at validation, lowering, the terminal live dependency/context checkpoint, normalization, and emission seams. Initial source acquisition/registration precedes this timer. |
| Peak RSS delta | 2 GiB | Increase in process high-water RSS after the terminal-generation baseline, using `getrusage` on supported Unix targets and `K32GetProcessMemoryInfo` on Windows. |

Structural limits are measured by iterative graph walks, without serializing or recursively cloning
the graph. Here, "raw external IR" means the already-materialized `SchemaIr`, not its JSON bytes or
parser representation. The raw graph is checked before recursive validation and before dotted
module names are recursively canonicalized, then again against the normalized candidate set. Raw
metering combines each dotted or explicitly nested module depth with its direct member/type depth;
neither dimension can hide an over-limit combined shape until normalization.
Wall/RSS failures are operation-global and cannot be swallowed by adaptive fallback. Every failure
is typed: `ResourceLimitExceeded` carries `ResourceKind`,
`ResourcePhase`, observed value, and limit. A requested finite RSS limit on an unsupported target
returns `ResourceMeasurementUnavailable` instead of silently disabling enforcement.

`ResourceLimitExceeded` and `ResourceMeasurementUnavailable` are additive public
`PyBindgenError` variants. Downstream exhaustive matches must add arms for both typed failures when
updating.

External-IR registration only accumulates raw schemas. Source-mode and bounded transport errors
remain immediate; graph-semantic validation and lowering errors now occur at terminal generation.
This permits any number of incremental registrations without repeated whole-graph work.

Run the reproducible timing suite with `cargo bench -p pyo3_bindgen_engine --bench scale`. The
NumPy row and its frozen byte oracle require `numpy==2.2.4`, matching the compile-sweep pin; the
matrix fails with an explicit version error otherwise. Run the isolated child-process RSS matrix
with:

```text
cargo test -p pyo3_bindgen_engine --test scale_workloads \
  isolated_scale_resource_matrix -- --ignored --nocapture --test-threads=1
```

The matrix covers wide schemas, deep inheritance, dense imports, 1,000 incremental registrations,
adaptive live candidates, and NumPy-scale live generation. Budget failures through `build` are
tested to publish no output. `GeneratedArtifact::work` and each matrix row record exact whole-graph
validation/lowering passes, normalization passes, preserved module-tree clones, and canonical
renders without adding runtime measurements to deterministic report JSON. The parent matrix parses
each child record and pins deterministic output/work oracles in
[`scale_workloads.rs`](../crates/pyo3_bindgen_engine/tests/scale_workloads.rs).

Elapsed time and peak-RSS delta remain printed and parsed as useful environment evidence, but are
intentionally not exact assertions because scheduler and allocator behavior are not deterministic.
