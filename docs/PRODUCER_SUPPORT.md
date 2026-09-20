# Producer naming and provenance support

Pure naming and report utilities let producers share generator rules without importing
downstream package policy into the engine.

## Enum naming producers

Rust callers use `enum_variant_ident(&str) -> Option<String>` for one canonical Rust variant name or
`plan_enum_variants(&[String]) -> Vec<EnumVariantPlan>` for the complete producer preflight policy.
The batch planner preserves input order, drops unrepresentable names, and keeps the first Python
member when canonical Rust names collide. The ordinary IR lowerer remains strict; this helper does
not silently change submitted schemas.

Python/non-Rust producers use the bounded versioned CLI protocol:

```text
pyo3_bindgen name-plan
```

stdin:

```json
{"protocol_version":1,"python_names":["AL_PHA","al_pha","3GAMMA","!!!"]}
```

stdout:

```json
{"protocol_version":1,"variants":[{"python_name":"AL_PHA","rust_name":"AlPha"},{"python_name":"3GAMMA","rust_name":"V3gamma"}]}
```

The request is capped at 8 MiB and 100,000 names. Unknown fields, malformed JSON, extra command-line
arguments, and unsupported protocol versions fail without partial stdout. Downstream ownership
conventions remain producer policy. Adoption follow-ups live in the release plan.

## Report/provenance consumers

`stable_hash(bytes)` is the canonical `fnv1a64:<16 lowercase hex>` digest already used by generated
provenance. Consumers can hash their own revision metadata using the same digest spelling.

`GenerationReportDocument` flattens the typed `GenerationReport` together with an ordered map of
consumer-owned root extensions. A consumer can deserialize diagnostics through
`GenerationDiagnostic`/`DiagnosticSeverity`, retain unknown extension values, and add a package
version without raw JSON splicing. The normal CLI still serializes `GenerationReport` directly;
report schema v1 and generated Rust are not replaced by this wrapper.
Extension keys must not collide with any field owned by `GenerationReport`, including an optional
field omitted from one report; serialization rejects a reserved key instead of producing duplicate,
overwritten, or lossy root fields.

The base report contract is checked in at `schemas/generation_report.v1.json`. Version 1 permits
non-reserved consumer fields at the root so documents emitted through `GenerationReportDocument`
remain valid; typed nested objects stay closed. That root compatibility choice also means a schema
consumer cannot distinguish a misspelled unknown root field from an extension. Tests retain the
same-owner DTO drift check and separately validate hand-authored valid/invalid documents plus a
serialized extended wrapper with an independent Draft 2020-12 implementation.

Repository checkout discovery, Git commands, revision pins/comparison, and environment policy stay
consumer-owned. There is no corresponding pyo3_bindgen implementation to centralize, and imposing
one would couple the generator to a downstream release policy.
