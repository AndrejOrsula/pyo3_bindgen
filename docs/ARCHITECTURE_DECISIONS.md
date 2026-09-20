# Architecture decisions

This record describes accepted boundaries. Unfinished work and reopening conditions live
only in the [release plan](release_checklist.md).

## Ownership

`CLI → facade → engine`; `proc-macro → engine`; `examples → facade`.
The engine remains domain-neutral. Source acquisition feeds private normalization,
planning, emission, reporting, and publication modules. No crate extraction is approved.

- `ExternalIrState` accumulates authored `SchemaIr`; terminal generation validates and
  lowers the whole graph once. Wire DTOs remain distinct from the private normalized model.
  Built-in external IR executes no Python; custom source acquisition owns its own effects.
- Live sessions own priming, introspection, dependencies, protected sentinels, and stream
  cleanup. Support is one main interpreter identity for the process lifetime. Metadata is
  sampled after coordinated restoration; moving it under the permit recreates reentrancy
  risk. Arbitrary imports, descriptors, annotations, callbacks, and finalizers remain trusted.
  For Python 3.14 deferred annotations, use public `annotationlib.Format.STRING`
  introspection and retain exact callable shape/type lowering. STRING is not a
  no-evaluation guarantee: CPython may execute annotation metadata while producing
  strings. The maintainer accepted that public-API behavior; no private CPython
  workaround or older-interpreter pin is part of the contract. Regression tests
  compare metadata effects with independent public introspection and retain the
  exact generated shape/type assertions. The generator does not evaluate captured strings.
- External IR always uses deduplicated hierarchy. Live selection compares exact canonical
  formatted bytes of valid candidates. Preserve deterministic ordering and error precedence.
- Candidate-local, token-free forwarder plans preserve first-seen members, nearest-first
  ancestors, own-member exclusion, multiplicity, and unresolved winners. Emission reuses
  collision-selected signatures from ordered ancestor validation. Missing required plans
  are invariant errors; no process-global type/token cache exists.
- Emission's no-host-import regression observes all imports in an isolated test
  subprocess. A process-wide import hook in a parallel test process can attribute
  unrelated tests' imports to emission, especially without the GIL. Preserve the
  complete observer rather than filtering module names; bound and reap the child,
  and restore the hook on both normal and panic paths.
- Schema annotation mappings/indexes and module rewrite tables are prepared once per scope.
  Identifier fast paths must agree with the parser, including fallback/error behavior.
- `artifact/value.rs` owns pure construction, formatting, content, and hashing. Public
  wrappers and effectful compatibility methods retain their identities in `artifact.rs`.
- Facade `publication::{Request, Session, Outcome}` owns command delivery state; the engine
  owns filesystem mechanics. CLI owns arguments, bounded JSON, diagnostics, and warning policy.
  A session accepts one primary operation in its requested mode; reports may be retried.
  Finalization rejects unfinished deliveries and preserves typed failures and correlation.
  Absent report IDs never erase an established ID. Legacy `RunOutcome` conversion remains
  the `run_cli_detailed` compatibility boundary, not an opacity or capability guarantee.
- Keep the 39 engine and 37 facade hidden reexport identities pinned by `tooling_contract`.
  Test fault injection remains non-default `internal-test-hooks`, unsupported for consumers.
- Report v1 permits non-reserved flattened root extensions; nested typed objects stay strict.
  Root typos are consequently indistinguishable from extensions. Preserve reserved-key
  rejection and independent IR/report conformance corpora.
- Transport, parsing/materialization, structural generation, canonical output, and process
  high-water RSS are separate limits; see [resource budgets](RESOURCE_BUDGETS.md).

## Publication policies

Atomicity is per file, not across output/report pairs. Engine sessions own path identity,
ordered cross-process locks, revalidation, replacement, receipts, and recovery. Parent
directories must remain trusted. The Windows identity key uses deterministic Unicode
uppercase over decoded UTF-16, preserves unpaired surrogates, and does not normalize.

| Caller | Path/locking boundary | Durability |
|---|---|---|
| CLI file output/report | Full session identity and lexical policy, distinct artifacts, locks and revalidation | Visible output with unsupported or failed directory sync retains its receipt and returns partial-publication exit 3. |
| `Codegen::build` / `build_with_report` | Session policy for the generated output | Directory-sync failure is an error; unsupported synchronization retains compatibility success. |
| Direct `RustArtifact` / `TextArtifact` writers | Regular-file/symlink checks and atomic replacement; no session lock or full Windows lexical policy | Ordinary writes report sync failure but accept unsupported synchronization; detailed receipts expose the distinction. |

The maintainer limits Windows/macOS assurance to cross-compilation for this release.
Do not run native Windows/macOS or UNC tests under the current authorization.
Filesystem, ACL, locking, replacement, and durability runtime behavior on those
platforms remains unverified; compilation is not runtime proof. Retain the
direct-writer regression for a deliberately non-default Windows DACL on both
artifact types. Future authorized UNC execution requires a writable
`PYO3_BINDGEN_TEST_UNC_ROOT`; tests own only their fresh child directory.
Cross-check engine, facade, and CLI separately so feature unification cannot hide
missing platform dependencies.

## CI and release ownership

Release preparation selects exact PyO3 `=0.29.2` and shared packaging CPython
`3.10.21`, retaining Rust/Cargo `1.97.0` and MSRV `1.83`. These patch updates
incorporate free-threaded FFI corrections and interpreter security fixes.
Update dependency/provenance fixtures and exact packaging guards together;
older-pin package hashes and candidate evidence cannot qualify the new release.
Keep generated shape/type assertions, supported interpreter boundaries, and
compile-only platform assurance unchanged.
The exact `3.14.0t` lane remains a minimum-patch compatibility check, alongside
the current-patch `3.14t` lane; it is not the packaging or publication interpreter.

Routine CI uses self-hosted Linux runners only in the identified private CI
repository; public CI uses standard GitHub-hosted `ubuntu-24.04` runners.
Fork pull requests must not execute on persistent runners. Public fork CI may
run on ephemeral hosted runners with read-only tokens and no persisted checkout
credentials. Reusable dependency-policy jobs use the same routing boundary.
Jobs own their Cargo targets; benchmark targets are compiled but not run.
Retain required `cargo (MSRV)`, `cargo (stable)`, and `deny` check contexts;
the `deny` result must fail unless every dependency-policy matrix job succeeds.
Rust setup must put the configured Cargo home's rustup proxies before system tools
for both top-level and nested commands, then verify actual selected Cargo/Rust versions.
An installed toolchain alone is insufficient when a runner resolves older system binaries.
Before installation, an absent proxy directory is valid: the pinned toolchain
action owns bootstrap on clean runners. If that action finds an existing rustup
but leaves the configured Cargo home's proxy set incomplete, setup repairs the
missing proxies from that already-selected executable after installation; it
must not download an unpinned installer. Require executable proxies and matching
toolchain selection after installation, not before the installer can create them.

The private embedded-Python composite provisions a pinned uv-managed interpreter and seeded
virtual environment per job, checks shared-library loading and GIL mode, and exports paths
before Rust caching. Dependency policy uses native pinned cargo-deny without Docker.
Windows GNU/MSVC and both Apple lanes compile only; Apple checks require no SDK or linking.
Archive metadata is filtered to the actual compiler host on MSRV; shared Python fixtures
enter existing runtime coordination before using mutable sentinels.

Publishing is manual and binds repository identity, protected tag, workflow ref/SHA,
checked-out candidate, and environment-scoped policy SHA before OIDC. External protection,
reviewer, deployment-policy, trusted-publisher, and security settings require direct evidence.
The maintainer approved a sole-maintainer release exception: `main` still requires
a pull request and passing CI, but no independent approval or last-push approval.
Only `AndrejOrsula` (user ID `22929099`) may approve the `crates-io` environment;
`prevent_self_review=false` permits that sole maintainer to approve their own run.
Keep `can_admins_bypass=false`, immutable release tags, exact-SHA guards, and
explicit publication authorization. This is not independent two-person review.
The maintainer selected public `AndrejOrsula/pyo3_bindgen` (repository ID `745882942`,
owner ID `22929099`) as the publishing repository. The temporary private CI
repository is self-hosted CI infrastructure, not a trusted publishing identity.
Release guards must require the public identity and visibility, and external controls
must be verified there. Temporary CI evidence is usable only for the exact reviewed
candidate SHA; publication never trusts a redirect or the temporary repository's tags.
Both release entrypoints must bind the manifests' repository URL to the same pinned
public repository before accepting structural checks or resolving remote tags. A
syntactically valid but different GitHub URL is not an authorized release identity.
