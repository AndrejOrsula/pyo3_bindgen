//! Canonical token/report emission over the private normalized graph.
//!
//! This module is deliberately downstream of source acquisition and normalization.  Its only
//! inputs are the immutable [`crate::normalized::NormalizedGraph`] and an explicit
//! [`EmissionPolicy`]; all token
//! construction, type indexing, model selection, and report assembly stay on this side of the
//! boundary.

use crate::{
    codegen::GeneratedBindings,
    normalized::{NormalizedCandidate, NormalizedCandidates, NormalizedPlan},
    planning::ImportPrefixes,
    report::{
        DiagnosticCode, DiscoveryOutcome, DiscoverySubject, DomainDiagnostic, GenerationDiagnostic,
        GenerationReport, ModuleReport, OutputMetadata, PythonMetadata,
    },
    resource::{BudgetTracker, ResourceKind, ResourcePhase},
    syntax::{GenerationState, GenerationStats},
    Result,
};

pub(crate) struct EmittedBindings {
    pub(crate) generated: GeneratedBindings,
    pub(crate) artifact: crate::RustArtifact,
    pub(crate) selected_model: &'static str,
    pub(crate) canonical_renders: usize,
    pub(crate) module_tree_clones: usize,
    pub(crate) validation_lowering_passes: usize,
    pub(crate) normalization_passes: usize,
}

impl EmittedBindings {
    pub(crate) const fn work(&self) -> crate::GenerationWork {
        crate::GenerationWork {
            validation_lowering_passes: self.validation_lowering_passes,
            normalization_passes: self.normalization_passes,
            module_tree_clones: self.module_tree_clones,
            canonical_renders: self.canonical_renders,
        }
    }
}

/// Select between the adaptive live model and its historical flat fallback.
///
/// Either model can fail independently. Preserve whichever model is valid and compare exact
/// canonical sizes only when both can be measured; if neither succeeds, prefer the historical
/// flat-path error.
fn select_live_candidate<T>(
    candidate: Result<T>,
    flat: Result<T>,
    measure: impl Fn(&T) -> Result<usize>,
) -> Result<T> {
    let (candidate, flat) = match (candidate, flat) {
        (Ok(candidate), Ok(flat)) => (candidate, flat),
        (Ok(candidate), Err(error)) => {
            tracing::debug!(
                selected_model = "deduplicated",
                flat_error = %error,
                "adaptive model selection used the only valid candidate"
            );
            return Ok(candidate);
        }
        (Err(error), Ok(flat)) => {
            tracing::debug!(
                selected_model = "flat",
                candidate_error = %error,
                "adaptive model selection used the only valid candidate"
            );
            return Ok(flat);
        }
        (Err(candidate_error), Err(flat_error)) => {
            tracing::error!(
                candidate_error = %candidate_error,
                flat_error = %flat_error,
                "both adaptive generation models failed"
            );
            return Err(flat_error);
        }
    };

    match (measure(&candidate), measure(&flat)) {
        (Ok(candidate_size), Ok(flat_size)) if candidate_size < flat_size => {
            tracing::debug!(
                selected_model = "deduplicated",
                candidate_bytes = candidate_size,
                flat_bytes = flat_size,
                "adaptive model selected"
            );
            Ok(candidate)
        }
        (Ok(candidate_size), Ok(flat_size)) => {
            tracing::debug!(
                selected_model = "flat",
                candidate_bytes = candidate_size,
                flat_bytes = flat_size,
                "adaptive model selected"
            );
            Ok(flat)
        }
        (Ok(candidate_size), Err(error)) => {
            tracing::debug!(
                selected_model = "deduplicated",
                candidate_bytes = candidate_size,
                flat_measure_error = %error,
                "adaptive model selected after measurement failure"
            );
            Ok(candidate)
        }
        (Err(error), Ok(flat_size)) => {
            tracing::debug!(
                selected_model = "flat",
                candidate_measure_error = %error,
                flat_bytes = flat_size,
                "adaptive model selected after measurement failure"
            );
            Ok(flat)
        }
        (Err(candidate_error), Err(flat_error)) => {
            tracing::error!(
                candidate_measure_error = %candidate_error,
                flat_measure_error = %flat_error,
                "both adaptive model measurements failed"
            );
            Err(flat_error)
        }
    }
}

/// Immutable choices that affect canonical emission or report metadata.
///
/// The policy contains no source adapter state, Python objects, wire DTOs, token streams, or
/// filesystem paths.  `Config` is copied at the phase boundary so a candidate cannot observe
/// later `Codegen` mutation.
#[derive(Debug, Clone)]
pub(crate) struct EmissionPolicy {
    token_stream_metadata: bool,
    python_metadata: Option<PythonMetadata>,
    operation_id: Option<String>,
}

impl EmissionPolicy {
    pub(crate) const fn new(token_stream_metadata: bool) -> Self {
        Self {
            token_stream_metadata,
            python_metadata: None,
            operation_id: None,
        }
    }

    pub(crate) fn token_stream_metadata(&self) -> bool {
        self.token_stream_metadata
    }

    pub(crate) fn with_python_metadata(mut self, metadata: Option<PythonMetadata>) -> Self {
        self.python_metadata = metadata;
        self
    }

    pub(crate) fn python_metadata(&self) -> Option<PythonMetadata> {
        self.python_metadata.clone()
    }

    pub(crate) fn with_operation_id(mut self, operation_id: impl Into<String>) -> Self {
        self.operation_id = Some(operation_id.into());
        self
    }

    pub(crate) fn operation_id(&self) -> &str {
        self.operation_id
            .as_deref()
            .expect("operation identity is attached before emission")
    }
}

/// Plan, emit, and select one immutable normalized candidate set.
pub(crate) fn emit_candidates(
    candidates: &NormalizedCandidates,
    live_import_prefixes: Option<ImportPrefixes>,
    policy: &EmissionPolicy,
    budget: &BudgetTracker,
) -> Result<EmittedBindings> {
    let canonical_renders = std::cell::Cell::new(0usize);
    let selection_span = tracing::debug_span!(
        "pyo3_bindgen.generate.selection",
        observability_schema = 1,
        operation_id = policy.operation_id(),
    );
    let _selection = selection_span.enter();
    let emit_candidate =
        |candidate: &NormalizedCandidate, policy: &EmissionPolicy, selected_model| {
            NormalizedPlan::build(
                candidate.graph(),
                candidate.config(),
                live_import_prefixes.clone(),
            )
            .and_then(|plan| emit_plan(&plan, policy))
            .and_then(|generated| {
                let artifact = crate::RustArtifact::from_tokens(generated.tokens.clone())?;
                canonical_renders.set(canonical_renders.get() + 1);
                budget.check_whole_resource(
                    ResourceKind::OutputBytes,
                    ResourcePhase::Emission,
                    artifact.len() as u128,
                )?;
                Ok(EmittedBindings {
                    generated,
                    artifact,
                    selected_model,
                    canonical_renders: canonical_renders.get(),
                    module_tree_clones: candidates.module_tree_clones(),
                    validation_lowering_passes: 0,
                    normalization_passes: 0,
                })
            })
        };
    let candidate = emit_candidate(
        candidates.primary(),
        policy,
        if candidates.flat().is_some() {
            "deduplicated"
        } else {
            "single"
        },
    );
    let selected = if let Some(flat) = candidates.flat() {
        // Once normalization elects the adaptive model, exact canonical byte size remains the
        // authoritative selector. The P2 proof-based early-winner optimization is intentionally
        // not performed here.
        let flat_policy = EmissionPolicy::new(policy.token_stream_metadata())
            .with_python_metadata(policy.python_metadata())
            .with_operation_id(policy.operation_id());
        let flat = emit_candidate(flat, &flat_policy, "flat");
        select_live_candidate(candidate, flat, |emitted| Ok(emitted.artifact.len())).map(
            |mut emitted| {
                emitted.canonical_renders = canonical_renders.get();
                emitted
            },
        )
    } else {
        candidate
    };
    let emitted = selected?;
    tracing::debug!(
        observability_schema = 1,
        operation_id = policy.operation_id(),
        selected_candidate = emitted.selected_model,
        candidate_bytes = emitted.artifact.len(),
        "generation candidate selected"
    );
    Ok(emitted)
}

/// Emit deterministic tokens and a report from a fully validated immutable plan.
pub(crate) fn emit_plan(
    plan: &NormalizedPlan<'_>,
    policy: &EmissionPolicy,
) -> Result<GeneratedBindings> {
    let graph = plan.graph();
    let cfg = plan.config();
    graph.source().trace(policy.operation_id());
    let source_kind = graph.source().kind().as_str();
    let candidate_span = tracing::debug_span!(
        "pyo3_bindgen.generate.candidate",
        observability_schema = 1,
        operation_id = policy.operation_id(),
        source = source_kind,
        import_mode = plan.import_mode_name(),
    );
    let _candidate = candidate_span.enter();

    let mut generation_diagnostics = Vec::new();
    let mut generation_stats = GenerationStats::with_input_hashes(graph.input_hashes().clone());
    let module_tokens: proc_macro2::TokenStream = {
        let mut generation_state = GenerationState::new(
            &mut generation_diagnostics,
            &mut generation_stats,
            plan.inherited_members(),
        );
        graph
            .modules()
            .iter()
            .map(|module| {
                module.generate(
                    cfg,
                    graph.modules(),
                    plan.all_types(),
                    plan.all_classes(),
                    plan.non_newtype_paths(),
                    plan.emit_collection_wrapper(),
                    plan.import_mode(),
                    &mut generation_state,
                )
            })
            .collect::<Result<_>>()?
    };

    // Deduplicated object-model support is generation-global and remains in exactly the same
    // position as before: before the top-level module tokens.
    let tokens = if plan.emit_root_support() {
        let mut tokens = crate::syntax::class::generate_root_trait();
        if plan.emit_collection_wrapper() {
            tokens.extend(crate::syntax::class::generate_collection_wrapper());
        }
        tokens.extend(module_tokens);
        tokens
    } else {
        module_tokens
    };

    let report = report_for_tokens(
        plan,
        policy,
        &tokens,
        generation_diagnostics,
        &generation_stats,
    )?;
    Ok(GeneratedBindings { tokens, report })
}

pub(crate) fn discovery_counts(
    diagnostics: &[GenerationDiagnostic],
    subject: DiscoverySubject,
) -> [usize; 5] {
    let mut counts = [0; 5];
    for diagnostic in diagnostics {
        let DiagnosticCode::Discovery {
            subject: diagnostic_subject,
            outcome,
        } = diagnostic.typed_code()
        else {
            continue;
        };
        if diagnostic_subject != subject {
            continue;
        }
        match outcome {
            DiscoveryOutcome::Attempted => counts[0] += 1,
            DiscoveryOutcome::Emitted => counts[1] += 1,
            DiscoveryOutcome::Skipped => counts[2] += 1,
            DiscoveryOutcome::Failed => {
                counts[2] += 1;
                counts[3] += 1;
            }
            DiscoveryOutcome::Truncated => {
                counts[2] += 1;
                counts[4] += 1;
            }
            DiscoveryOutcome::Summary => {}
        }
    }
    counts
}

pub(crate) fn discovery_summary_message(counts: [usize; 5]) -> String {
    format!(
        "attempted={};emitted={};skipped={};failed={};truncated={}",
        counts[0], counts[1], counts[2], counts[3], counts[4]
    )
}

fn report_for_tokens(
    plan: &NormalizedPlan<'_>,
    policy: &EmissionPolicy,
    tokens: &proc_macro2::TokenStream,
    mut generation_diagnostics: Vec<DomainDiagnostic>,
    generation_stats: &GenerationStats,
) -> Result<GenerationReport> {
    let graph = plan.graph();
    let cfg = plan.config();
    let mut modules = Vec::<ModuleReport>::new();
    let mut diagnostics = Vec::<DomainDiagnostic>::new();
    for module in graph.modules() {
        module.collect_report_entries(
            cfg,
            generation_stats,
            true,
            &mut modules,
            &mut diagnostics,
        )?;
    }
    if let Some(sha256) = graph.source().transport_sha256() {
        for module in &mut modules {
            if module.input_kind == "external_ir" {
                module.source_hash = Some(sha256.to_owned());
            }
        }
    }
    diagnostics.append(&mut generation_diagnostics);
    diagnostics.extend(graph.diagnostics().iter().cloned());
    let diagnostics = diagnostics
        .into_iter()
        .map(GenerationDiagnostic::from)
        .collect::<Vec<_>>();
    // Recursive submodule parsing stores facts on each owning module, while dependency facts
    // are generation-global. Normalize only these additive facts by full origin so report order
    // is independent of tree depth; preserve every pre-existing diagnostic's relative order.
    let (mut discovery_facts, mut ordinary_diagnostics): (Vec<_>, Vec<_>) =
        diagnostics.into_iter().partition(|diagnostic| {
            matches!(diagnostic.typed_code(), DiagnosticCode::Discovery { .. })
        });
    discovery_facts.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then_with(|| {
                let order = |diagnostic: &GenerationDiagnostic| {
                    u8::from(!matches!(
                        diagnostic.typed_code(),
                        DiagnosticCode::Discovery {
                            outcome: DiscoveryOutcome::Attempted,
                            ..
                        }
                    ))
                };
                order(left).cmp(&order(right))
            })
            .then_with(|| left.code.cmp(&right.code))
    });
    ordinary_diagnostics.append(&mut discovery_facts);
    let mut diagnostics = ordinary_diagnostics;
    let submodule_counts = discovery_counts(&diagnostics, DiscoverySubject::Submodule);
    let dependency_counts = discovery_counts(&diagnostics, DiscoverySubject::Dependency);
    if submodule_counts[0] > 0 {
        diagnostics.push(GenerationDiagnostic::info(
            DiagnosticCode::discovery(DiscoverySubject::Submodule, DiscoveryOutcome::Summary),
            "generation",
            discovery_summary_message(submodule_counts),
        ));
    }
    if dependency_counts[0] > 0 {
        diagnostics.push(GenerationDiagnostic::info(
            DiagnosticCode::discovery(DiscoverySubject::Dependency, DiscoveryOutcome::Summary),
            "generation",
            discovery_summary_message(dependency_counts),
        ));
    }

    // Streamed, not materialized: the hash and byte count are identical to those of
    // `tokens.to_string()`, without allocating that (multi-megabyte) rendering.
    let outputs = if policy.token_stream_metadata() {
        vec![OutputMetadata::from_display("token_stream", None, tokens)]
    } else {
        Vec::new()
    };
    let mut report =
        GenerationReport::new_with_python(modules, diagnostics, outputs, policy.python_metadata());
    report.operation_id = Some(policy.operation_id().to_owned());
    report.complete = graph.completeness().is_complete();
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        normalized::NormalizedGraph,
        planning::ImportPrefixes,
        syntax::{Module, Path},
        Config,
    };
    use pyo3::types::{PyAnyMethods, PyModule};
    use rustc_hash::FxHashMap as HashMap;
    use std::{
        fs::{self, OpenOptions},
        io::{self, Write},
        path::{Path as FsPath, PathBuf},
        process::{Child, Command, ExitStatus, Stdio},
        thread,
        time::{Duration, Instant, SystemTime, UNIX_EPOCH},
    };

    const LIVE_EMISSION_CHILD_ENV: &str = "PYO3_BINDGEN_LIVE_EMISSION_NO_HOST_IMPORTS_CHILD";
    const LIVE_EMISSION_CHILD_ROLE: &str = "pyo3-bindgen-live-emission-no-host-imports-child-v1";
    const LIVE_EMISSION_CHILD_MARKER_ENV: &str =
        "PYO3_BINDGEN_LIVE_EMISSION_NO_HOST_IMPORTS_MARKER";
    const LIVE_EMISSION_CHILD_MARKER: &[u8] =
        b"pyo3-bindgen-live-emission-no-host-imports-success-v1\n";
    const LIVE_EMISSION_TEST_NAME: &str = "emission::tests::live_emission_performs_no_host_imports";
    const LIVE_EMISSION_CHILD_TIMEOUT: Duration = Duration::from_secs(30);
    const LIVE_EMISSION_CHILD_POLL_INTERVAL: Duration = Duration::from_millis(10);

    struct LiveEmissionMarker {
        directory: PathBuf,
        path: PathBuf,
    }

    impl LiveEmissionMarker {
        fn allocate() -> io::Result<Self> {
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            for attempt in 0..128_u32 {
                let directory = std::env::temp_dir().join(format!(
                    "pyo3_bindgen_live_emission_{}_{}_{}",
                    std::process::id(),
                    timestamp,
                    attempt,
                ));
                match fs::create_dir(&directory) {
                    Ok(()) => {
                        return Ok(Self {
                            path: directory.join("completed"),
                            directory,
                        });
                    }
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(error) => return Err(error),
                }
            }
            Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "could not allocate an exclusive live-emission marker directory",
            ))
        }

        fn path(&self) -> &FsPath {
            &self.path
        }

        fn verify(&self) -> io::Result<()> {
            let marker = fs::read(&self.path)?;
            if marker == LIVE_EMISSION_CHILD_MARKER {
                Ok(())
            } else {
                Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "live-emission child completion marker has unexpected contents",
                ))
            }
        }
    }

    impl Drop for LiveEmissionMarker {
        fn drop(&mut self) {
            if let Err(error) = fs::remove_dir_all(&self.directory) {
                if error.kind() != io::ErrorKind::NotFound {
                    eprintln!(
                        "failed to remove live-emission marker directory {}: {error}",
                        self.directory.display()
                    );
                }
            }
        }
    }

    struct LiveEmissionChild {
        child: Option<Child>,
    }

    impl LiveEmissionChild {
        fn new(child: Child) -> Self {
            Self { child: Some(child) }
        }

        fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
            self.child
                .as_mut()
                .expect("live-emission child ownership must be present")
                .try_wait()
        }

        fn release_reaped(&mut self) {
            self.child.take();
        }

        fn kill_and_wait(&mut self) -> io::Result<ExitStatus> {
            let Some(child) = self.child.as_mut() else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "live-emission child ownership was already released",
                ));
            };
            let kill_error = child.kill().err();
            let wait_result = child.wait();
            if wait_result.is_ok() {
                self.child.take();
            }
            match (kill_error, wait_result) {
                (None, result) => result,
                (Some(kill_error), Ok(status)) => Err(io::Error::other(format!(
                    "live-emission child kill failed after wait reaped status {status}: {kill_error}"
                ))),
                (Some(kill_error), Err(wait_error)) => Err(io::Error::other(format!(
                    "live-emission child kill failed: {kill_error}; wait failed: {wait_error}"
                ))),
            }
        }
    }

    impl Drop for LiveEmissionChild {
        fn drop(&mut self) {
            let Some(mut child) = self.child.take() else {
                return;
            };
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    fn selection_error(message: &str) -> crate::PyBindgenError {
        crate::PyBindgenError::CodegenError(message.to_owned())
    }

    #[test]
    fn adaptive_selection_covers_win_tie_and_fallback() {
        let measure = |value: &&str| match *value {
            "candidate" => Ok(10),
            "flat" => Ok(20),
            "bad_candidate_artifact" => Err(selection_error("candidate artifact")),
            "bad_flat_artifact" => Err(selection_error("flat artifact")),
            _ => unreachable!(),
        };

        assert_eq!(
            select_live_candidate(Ok("candidate"), Err(selection_error("flat")), measure).unwrap(),
            "candidate",
        );
        assert_eq!(
            select_live_candidate(Err(selection_error("candidate")), Ok("flat"), measure).unwrap(),
            "flat",
        );
        assert_eq!(
            select_live_candidate(Ok("candidate"), Ok("flat"), measure).unwrap(),
            "candidate",
            "the exact smaller canonical artifact wins",
        );
        assert_eq!(
            select_live_candidate(Ok("candidate"), Ok("flat"), |_| Ok(10)).unwrap(),
            "flat",
            "an exact-size tie preserves the historical flat model",
        );
        assert_eq!(
            select_live_candidate(Ok("candidate"), Ok("bad_flat_artifact"), measure).unwrap(),
            "candidate",
        );
        assert_eq!(
            select_live_candidate(Ok("bad_candidate_artifact"), Ok("flat"), measure).unwrap(),
            "flat",
        );
        assert!(select_live_candidate(
            Err::<&str, _>(selection_error("candidate")),
            Err(selection_error("flat")),
            measure,
        )
        .unwrap_err()
        .to_string()
        .contains("flat"));
        assert!(select_live_candidate(
            Ok("bad_candidate_artifact"),
            Ok("bad_flat_artifact"),
            measure,
        )
        .unwrap_err()
        .to_string()
        .contains("flat artifact"));
    }

    #[test]
    fn live_emission_performs_no_host_imports() {
        if std::env::var(LIVE_EMISSION_CHILD_ENV).as_deref() == Ok(LIVE_EMISSION_CHILD_ROLE) {
            run_live_emission_no_host_imports();
            let marker_path = PathBuf::from(
                std::env::var_os(LIVE_EMISSION_CHILD_MARKER_ENV)
                    .expect("live-emission child marker path must be provided"),
            );
            let mut marker = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(marker_path)
                .expect("live-emission child completion marker must be created exclusively");
            marker
                .write_all(LIVE_EMISSION_CHILD_MARKER)
                .expect("live-emission child completion marker must be written");
            marker
                .sync_all()
                .expect("live-emission child completion marker must be synchronized");
            return;
        }

        let marker = LiveEmissionMarker::allocate()
            .expect("live-emission marker directory must be allocated exclusively");
        let mut child = LiveEmissionChild::new(
            Command::new(std::env::current_exe().expect("current test executable must exist"))
                .args(["--exact", LIVE_EMISSION_TEST_NAME, "--nocapture"])
                .env(LIVE_EMISSION_CHILD_ENV, LIVE_EMISSION_CHILD_ROLE)
                .env(LIVE_EMISSION_CHILD_MARKER_ENV, marker.path())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .spawn()
                .expect("live-emission child process must spawn"),
        );
        let deadline = Instant::now() + LIVE_EMISSION_CHILD_TIMEOUT;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    child.release_reaped();
                    break status;
                }
                Ok(None) if Instant::now() < deadline => {
                    thread::sleep(LIVE_EMISSION_CHILD_POLL_INTERVAL);
                }
                Ok(None) => {
                    let cleanup = child.kill_and_wait();
                    panic!(
                        "live-emission child exceeded {:?}; cleanup: {cleanup:?}",
                        LIVE_EMISSION_CHILD_TIMEOUT
                    );
                }
                Err(error) => {
                    let cleanup = child.kill_and_wait();
                    panic!(
                        "live-emission child status check failed: {error}; cleanup: {cleanup:?}"
                    );
                }
            }
        };
        assert!(
            status.success(),
            "live-emission child failed with status {status}"
        );
        marker
            .verify()
            .expect("live-emission child must prove the exact test completed");
    }

    fn run_live_emission_no_host_imports() {
        let config = Config::default();
        let module = crate::live_operation::with_attached(
            crate::live_operation::LivePolicy::from_config(&config),
            |py| {
                let module = PyModule::from_code(
                    py,
                    pyo3::ffi::c_str!(
                        "class Item:\n    pass\n\ndef make() -> Item:\n    return Item()\n"
                    ),
                    pyo3::ffi::c_str!("emission_probe.py"),
                    pyo3::ffi::c_str!("emission_probe"),
                )?;
                Module::parse(&config, &module)
            },
        )
        .expect("live fixture parses");
        let mut hashes = HashMap::default();
        hashes.insert(module.name.clone(), module.input_hash().unwrap());
        let graph = NormalizedGraph::from_live(
            vec![module],
            hashes,
            Vec::new(),
            vec!["test:live_emission".to_owned()],
        )
        .unwrap();
        let prefixes = ImportPrefixes::new(
            graph
                .required_import_paths(&config)
                .into_iter()
                .map(|path| (path, Path::from_py("emission_probe"))),
        )
        .unwrap();
        let plan = NormalizedPlan::build(&graph, &config, Some(prefixes)).unwrap();

        pyo3::Python::attach(|py| {
            let tracker_module = PyModule::from_code(
                py,
                pyo3::ffi::c_str!(
                    "import builtins\n\nclass Tracker:\n    def __init__(self):\n        self.original = builtins.__import__\n        self.calls = []\n    def __call__(self, name, globals=None, locals=None, fromlist=(), level=0):\n        self.calls.append(name)\n        return self.original(name, globals, locals, fromlist, level)\n\ntracker = Tracker()\n"
                ),
                pyo3::ffi::c_str!("emission_import_tracker.py"),
                pyo3::ffi::c_str!("emission_import_tracker"),
            )
            .unwrap();
            let builtins = py.import(pyo3::intern!(py, "builtins")).unwrap();
            let tracker = tracker_module.getattr("tracker").unwrap();
            let _runtime_permit = crate::io_utils::acquire_python_runtime(py).unwrap();
            let _restore =
                crate::runtime_sentinels::TestAttributeRestore::new(py, &builtins, "__import__")
                    .unwrap();
            builtins.setattr("__import__", &tracker).unwrap();
            py.import(pyo3::intern!(py, "typing")).unwrap();
            let positive_control_calls: Vec<String> =
                tracker.getattr("calls").unwrap().extract().unwrap();
            assert!(
                !positive_control_calls.is_empty(),
                "the import observer must see the deliberate positive-control import"
            );
            tracker
                .getattr("calls")
                .unwrap()
                .call_method0(pyo3::intern!(py, "clear"))
                .unwrap();
            let result = emit_plan(
                &plan,
                &EmissionPolicy::new(true).with_operation_id("op-v1:test"),
            );
            result.expect("pure emission succeeds");
            // Report the offending module names, not just a count: this invariant has held on
            // GIL builds while failing on free-threaded ones, and a bare count says nothing
            // about which import to chase.
            let calls: Vec<String> = tracker.getattr("calls").unwrap().extract().unwrap();
            assert!(
                calls.is_empty(),
                "emission must not import host modules, but imported {calls:?}"
            );
        });
    }
}
