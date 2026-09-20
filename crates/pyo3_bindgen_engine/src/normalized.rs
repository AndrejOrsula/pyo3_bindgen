//! Private, Python-free normalized generation domain.
//!
//! This is the first phase boundary in the generation pipeline.  Source adapters may use
//! PyO3 or serde before they reach this module, but the graph itself owns only the already
//! lowered generation model and immutable provenance/diagnostic facts.  Token-producing
//! methods are deliberately kept out of this module; they are consumed by `emission`.

use crate::{
    planning::ImportPrefixes,
    report::{DiagnosticCode, DiscoveryOutcome, DomainDiagnostic},
    syntax::{Class, Import, ImportMode, Module, Path},
    Config, Result,
};
use itertools::Itertools;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

/// The source kind that has already crossed its acquisition and validation boundary.
///
/// Unlike `codegen::SourceMode`, this type has no `Unset` state: a graph cannot be built until
/// the source adapter has selected one coherent mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum SourceKind {
    Live,
    ExternalIr,
}

impl SourceKind {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::ExternalIr => "external_ir",
        }
    }

    pub(crate) const fn is_external_ir(self) -> bool {
        matches!(self, Self::ExternalIr)
    }
}

/// Immutable facts produced by the source adapter before normalization begins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AcquisitionFacts {
    kind: SourceKind,
    provenance: Vec<String>,
    transport: SourceTransport,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SourceTransport {
    LiveInspection,
    MaterializedExternalIr,
    BoundedExternalIr {
        sha256: String,
        content_bytes: usize,
        byte_limit: usize,
    },
}

impl AcquisitionFacts {
    pub(crate) fn live(provenance: Vec<String>) -> Self {
        Self {
            kind: SourceKind::Live,
            provenance,
            transport: SourceTransport::LiveInspection,
        }
    }

    pub(crate) fn external_ir(
        provenance: Vec<String>,
        bounded: Option<(&str, usize, usize)>,
    ) -> Self {
        Self {
            kind: SourceKind::ExternalIr,
            provenance,
            transport: bounded.map_or(SourceTransport::MaterializedExternalIr, |facts| {
                SourceTransport::BoundedExternalIr {
                    sha256: facts.0.to_owned(),
                    content_bytes: facts.1,
                    byte_limit: facts.2,
                }
            }),
        }
    }

    pub(crate) const fn kind(&self) -> SourceKind {
        self.kind
    }
}

/// The validated identity of one normalized graph's source and root modules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SourceIdentity {
    kind: SourceKind,
    roots: Vec<Path>,
    content_hashes: HashMap<Path, String>,
    transport_sha256: Option<String>,
    transport_bytes: Option<usize>,
}

impl SourceIdentity {
    fn new(
        acquisition: &AcquisitionFacts,
        modules: &[Module],
        content_hashes: HashMap<Path, String>,
    ) -> Result<Self> {
        let roots: Vec<Path> = modules.iter().map(|module| module.name.clone()).collect();
        if roots.is_empty() {
            return Err(crate::PyBindgenError::CodegenError(
                "normalized graph requires at least one root module".to_owned(),
            ));
        }
        Ok(Self {
            kind: acquisition.kind,
            roots,
            content_hashes,
            transport_sha256: match &acquisition.transport {
                SourceTransport::BoundedExternalIr { sha256, .. } => Some(sha256.clone()),
                _ => None,
            },
            transport_bytes: match &acquisition.transport {
                SourceTransport::BoundedExternalIr { content_bytes, .. } => Some(*content_bytes),
                _ => None,
            },
        })
    }

    pub(crate) const fn kind(&self) -> SourceKind {
        self.kind
    }

    pub(crate) fn transport_sha256(&self) -> Option<&str> {
        self.transport_sha256.as_deref()
    }
}

/// Concrete acquisition chain that produced the normalized graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SourceProvenance {
    adapters: Vec<String>,
}

impl SourceProvenance {
    fn new(adapters: Vec<String>) -> Result<Self> {
        if adapters.is_empty() || adapters.iter().any(String::is_empty) {
            return Err(crate::PyBindgenError::CodegenError(
                "normalized graph requires non-empty source provenance".to_owned(),
            ));
        }
        Ok(Self { adapters })
    }
}

/// Resource-limit ownership at the normalized source boundary.
///
/// A `LiveModules` name list is a source descriptor, not a transport receipt: `Codegen` resolves
/// and imports those names during registration, while configured dependency/context imports may
/// occur later during terminal generation. Live objects are not byte-addressed here. External IR
/// reaches the engine as an already materialized whole document; only a caller that supplies
/// bounded transport facts (currently the CLI) has a byte limit represented at this boundary.
/// Custom source acquisition remains outside this boundary. Whole-graph model/output/time budgets
/// are enforced by [`crate::GenerationBudget`] during generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SourceLimitScope {
    NotByteAddressed,
    MaterializedWholeDocument,
    BoundedWholeDocument { byte_limit: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SourceLimits {
    content: SourceLimitScope,
}

impl SourceLimits {
    fn from_acquisition(acquisition: &AcquisitionFacts) -> Self {
        Self {
            content: match &acquisition.transport {
                SourceTransport::LiveInspection => SourceLimitScope::NotByteAddressed,
                SourceTransport::MaterializedExternalIr => {
                    SourceLimitScope::MaterializedWholeDocument
                }
                SourceTransport::BoundedExternalIr { byte_limit, .. } => {
                    SourceLimitScope::BoundedWholeDocument {
                        byte_limit: *byte_limit,
                    }
                }
            },
        }
    }
}

/// Complete source-side result passed into planning and emission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SourceContext {
    identity: SourceIdentity,
    provenance: SourceProvenance,
    limits: SourceLimits,
    completeness: Completeness,
}

impl SourceContext {
    pub(crate) const fn kind(&self) -> SourceKind {
        self.identity.kind()
    }

    pub(crate) fn transport_sha256(&self) -> Option<&str> {
        self.identity.transport_sha256()
    }

    pub(crate) const fn transport_bytes(&self) -> Option<usize> {
        self.identity.transport_bytes
    }

    pub(crate) const fn transport_byte_limit(&self) -> Option<usize> {
        match self.limits.content {
            SourceLimitScope::BoundedWholeDocument { byte_limit } => Some(byte_limit),
            _ => None,
        }
    }

    pub(crate) fn provenance(&self) -> &[String] {
        &self.provenance.adapters
    }

    pub(crate) fn trace(&self, operation_id: &str) {
        let (limit_kind, byte_limit) = match self.limits.content {
            SourceLimitScope::NotByteAddressed => ("not_byte_addressed", None),
            SourceLimitScope::MaterializedWholeDocument => ("materialized", None),
            SourceLimitScope::BoundedWholeDocument { byte_limit } => ("bounded", Some(byte_limit)),
        };
        tracing::debug!(
            observability_schema = 1,
            operation_id,
            source = self.kind().as_str(),
            provenance_count = self.provenance.adapters.len(),
            transport_sha256 = ?self.identity.transport_sha256,
            transport_bytes = ?self.identity.transport_bytes,
            limit_kind,
            byte_limit,
            complete = self.completeness.is_complete(),
            "normalized source context"
        );
    }
}

/// Completeness facts established before token emission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Completeness {
    complete: bool,
}

impl Completeness {
    fn from_diagnostics(diagnostics: &[DomainDiagnostic]) -> Self {
        let complete = diagnostics.iter().all(|diagnostic| {
            !matches!(
                diagnostic.typed_code(),
                DiagnosticCode::Discovery {
                    outcome: DiscoveryOutcome::Skipped
                        | DiscoveryOutcome::Failed
                        | DiscoveryOutcome::Truncated,
                    ..
                }
            )
        });
        Self { complete }
    }

    pub(crate) const fn is_complete(self) -> bool {
        self.complete
    }
}

/// Immutable, owned graph passed from source normalization to canonical emission.
///
/// The stored `Module` values are the existing private, Python-free domain values. Their fields
/// contain paths, names, typing values, hierarchy, and report facts only; PyO3 handles, wire DTOs,
/// filesystem paths, and token streams remain outside this interface. Some values
/// derive `Serialize` solely for versioned deterministic fingerprints; no `SchemaIr` or report
/// DTO is stored. The wrapper is intentionally narrow so later behavior clusters can replace
/// individual syntax-owned member families without reopening source acquisition or report
/// publication.
#[derive(Debug, Clone)]
pub(crate) struct NormalizedGraph {
    modules: Vec<Module>,
    source: SourceContext,
    diagnostics: Vec<DomainDiagnostic>,
}

impl NormalizedGraph {
    pub(crate) fn operation_roots(&self) -> Vec<String> {
        self.source.identity.roots.iter().map(Path::to_py).collect()
    }

    pub(crate) fn operation_input_hashes(&self) -> Vec<(String, String)> {
        let mut hashes = self
            .source
            .identity
            .content_hashes
            .iter()
            .map(|(path, hash)| (path.to_py(), hash.clone()))
            .collect::<Vec<_>>();
        hashes.sort();
        hashes
    }

    pub(crate) fn resources(&self) -> Result<crate::resource::StructuralResources> {
        Ok(structural_resources(&self.modules))
    }

    /// Normalize a live adapter result into the immutable graph boundary.
    #[cfg(test)]
    pub(crate) fn from_live(
        modules: Vec<Module>,
        input_hashes: HashMap<Path, String>,
        diagnostics: Vec<DomainDiagnostic>,
        provenance: Vec<String>,
    ) -> Result<Self> {
        Self::new(
            AcquisitionFacts::live(provenance),
            modules,
            input_hashes,
            diagnostics,
        )
    }

    /// Normalize an external-IR adapter result into the same immutable graph boundary.
    #[cfg(test)]
    pub(crate) fn from_external_ir(
        modules: Vec<Module>,
        input_hashes: HashMap<Path, String>,
        diagnostics: Vec<DomainDiagnostic>,
        provenance: Vec<String>,
    ) -> Result<Self> {
        Self::new(
            AcquisitionFacts::external_ir(provenance, None),
            modules,
            input_hashes,
            diagnostics,
        )
    }

    fn from_acquisition(
        acquisition: AcquisitionFacts,
        modules: Vec<Module>,
        input_hashes: HashMap<Path, String>,
        diagnostics: Vec<DomainDiagnostic>,
    ) -> Result<Self> {
        Self::new(acquisition, modules, input_hashes, diagnostics)
    }

    fn new(
        acquisition: AcquisitionFacts,
        modules: Vec<Module>,
        input_hashes: HashMap<Path, String>,
        diagnostics: Vec<DomainDiagnostic>,
    ) -> Result<Self> {
        let kind = acquisition.kind();
        let mut paths = HashSet::default();
        validate_module_tree(kind, &modules, &mut paths)?;

        if paths.len() != input_hashes.len()
            || input_hashes.keys().any(|path| !paths.contains(path))
        {
            return Err(crate::PyBindgenError::CodegenError(
                "normalized graph input identities do not match its module tree".to_owned(),
            ));
        }

        let mut completeness_diagnostics = diagnostics.clone();
        collect_module_diagnostics(&modules, &mut completeness_diagnostics);
        let completeness = Completeness::from_diagnostics(&completeness_diagnostics);
        let identity = SourceIdentity::new(&acquisition, &modules, input_hashes)?;
        debug_assert_eq!(identity.roots.len(), modules.len());
        let source = SourceContext {
            identity,
            provenance: SourceProvenance::new(acquisition.provenance.clone())?,
            limits: SourceLimits::from_acquisition(&acquisition),
            completeness,
        };

        Ok(Self {
            modules,
            source,
            diagnostics,
        })
    }

    pub(crate) fn modules(&self) -> &[Module] {
        &self.modules
    }

    pub(crate) fn source(&self) -> &SourceContext {
        &self.source
    }

    pub(crate) fn input_hashes(&self) -> &HashMap<Path, String> {
        &self.source.identity.content_hashes
    }

    pub(crate) fn completeness(&self) -> Completeness {
        self.source.completeness
    }

    pub(crate) fn diagnostics(&self) -> &[DomainDiagnostic] {
        &self.diagnostics
    }

    #[cfg(test)]
    pub(crate) fn required_import_paths(&self, config: &Config) -> Vec<Path> {
        required_import_paths(&self.modules, config)
    }
}

fn structural_resources(modules: &[Module]) -> crate::resource::StructuralResources {
    let mut resources = crate::resource::StructuralResources {
        model_nodes: 1,
        recursion_depth: 1,
        ..crate::resource::StructuralResources::default()
    };
    let mut pending = modules
        .iter()
        .map(|module| (module, 1usize))
        .collect::<Vec<_>>();
    while let Some((module, module_depth)) = pending.pop() {
        let shape = module.direct_resource_shape();
        // Canonicalization wraps dotted top-level names in one synthetic module per path
        // segment. Account for that projected depth before `canonicalize` performs the recursive
        // ordered merge; explicit nesting and dotted identity can each be the deeper shape.
        let canonical_module_depth = module_depth.max(module.name.len());
        resources.definitions += shape.definitions;
        resources.model_nodes += shape.model_nodes;
        resources.recursion_depth = resources
            .recursion_depth
            .max(canonical_module_depth + shape.recursion_depth);
        pending.extend(
            module
                .submodules
                .iter()
                .map(|submodule| (submodule, module_depth + 1)),
        );
    }
    resources
}

/// Stable set of object paths whose importable package prefix live acquisition must resolve.
///
/// This inventory depends only on acquired domain values, not canonicalization. Keeping it at
/// the normalized-domain boundary lets the live adapter acquire all host facts before pure
/// normalization starts.
pub(crate) fn required_import_paths(modules: &[Module], config: &Config) -> Vec<Path> {
    fn collect(modules: &[Module], config: &Config, paths: &mut Vec<Path>) {
        for module in modules {
            if config.generate_classes {
                paths.extend(
                    module
                        .classes
                        .iter()
                        .flat_map(|class| class.required_import_paths(config)),
                );
            }
            if config.generate_enums {
                paths.extend(module.enums.iter().map(|py_enum| py_enum.name.clone()));
            }
            if config.generate_functions {
                paths.extend(
                    module
                        .functions
                        .iter()
                        .map(|function| function.import_path()),
                );
            }
            if config.generate_properties {
                paths.extend(
                    module
                        .properties
                        .iter()
                        .map(|property| property.import_path()),
                );
            }
            if config.traverse_submodules {
                collect(&module.submodules, config, paths);
            }
        }
    }

    let mut paths = Vec::new();
    collect(modules, config, &mut paths);
    paths.into_iter().unique().collect()
}

/// Ordered live parent identities required to wrap dotted top-level modules.
pub(crate) fn required_live_parent_paths(modules: &[Module]) -> Vec<Path> {
    let mut paths = Vec::new();
    for module in modules {
        if module.name.len() > 1 {
            for index in (0..module.name.len() - 1).rev() {
                paths.push(Path::from(&module.name[..=index]));
            }
        }
    }
    paths
}

/// The fixed generation-global cost charged by the adaptive live hierarchy decision.
///
/// The root trait, marker trait, and dotted-path resolver are emitted once whenever at least one
/// live inheritance component is deduplicated. Keep the measured cost with the normalization
/// policy that owns the decision rather than with the orchestration layer.
pub(crate) const ROOT_SUPPORT_COST: i64 = 4322;

/// Inputs crossing from source acquisition into the private normalization phase.
///
/// The input owns ordinary Rust model values and report facts only. Interpreter handles, wire
/// DTOs, and emission state are intentionally absent from this boundary.
#[derive(Debug)]
pub(crate) struct NormalizationInput {
    pub(crate) acquisition: AcquisitionFacts,
    pub(crate) config: Config,
    pub(crate) modules: Vec<Module>,
    pub(crate) dependency_diagnostics: Vec<DomainDiagnostic>,
    /// Already-acquired synthetic parents for live dotted modules. External IR leaves this
    /// empty and creates Python-free parents during normalization.
    pub(crate) live_parents: Vec<Module>,
}

/// One immutable normalized candidate. Planning and emission borrow this result but cannot mutate
/// the graph or change the candidate's configuration.
#[derive(Debug)]
pub(crate) struct NormalizedCandidate {
    graph: NormalizedGraph,
    config: Config,
}

impl NormalizedCandidate {
    pub(crate) const fn graph(&self) -> &NormalizedGraph {
        &self.graph
    }

    pub(crate) const fn config(&self) -> &Config {
        &self.config
    }
}

/// Immutable results of one normalization pass.
///
/// Live generation may return the adaptive deduplicated candidate and its preserved historical
/// flat candidate. External IR and live runs without an eligible deduplication pass return only
/// the primary candidate. Planning is deliberately downstream so a planning/emission failure in
/// either live candidate can still be handled by the adaptive selector.
#[derive(Debug)]
pub(crate) struct NormalizedCandidates {
    primary: NormalizedCandidate,
    flat: Option<NormalizedCandidate>,
    module_tree_clones: usize,
}

impl NormalizedCandidates {
    pub(crate) const fn primary(&self) -> &NormalizedCandidate {
        &self.primary
    }

    pub(crate) const fn flat(&self) -> Option<&NormalizedCandidate> {
        self.flat.as_ref()
    }

    /// Exact number of full module-tree clones performed to preserve adaptive candidates.
    pub(crate) const fn module_tree_clones(&self) -> usize {
        self.module_tree_clones
    }
}

/// Canonicalize source-model inputs and construct the immutable adaptive candidates.
///
/// Live parents cross this boundary as ordinary acquired domain values, so normalization is a
/// callback-free deterministic transform. External IR creates its synthetic parents locally.
pub(crate) fn normalize(
    input: NormalizationInput,
    budget: &crate::resource::BudgetTracker,
) -> Result<NormalizedCandidates> {
    let NormalizationInput {
        acquisition,
        config,
        modules,
        dependency_diagnostics,
        live_parents,
    } = input;
    let source = acquisition.kind();
    budget.check_structure(
        crate::ResourcePhase::Normalization,
        structural_resources(&modules),
    )?;

    let modules = {
        let phase =
            tracing::debug_span!("pyo3_bindgen.generate.phase", phase = "canonicalization",);
        let _entered = phase.enter();
        canonicalize(source, modules, live_parents)?
    };
    tracing::debug!(modules = modules.len(), "module tree canonicalized");
    budget.check_structure(
        crate::ResourcePhase::Normalization,
        structural_resources(&modules),
    )?;

    let input_hashes = {
        let phase = tracing::debug_span!(
            "pyo3_bindgen.generate.phase",
            phase = "input_fingerprinting",
        );
        let _entered = phase.enter();
        snapshot_input_hashes(&modules)?
    };

    // Preserve the canonical flat model before the adaptive live pass mutates class hierarchies.
    // The exact formatted bytes are still compared downstream after both candidates are planned
    // and emitted.
    let preserve_flat = source == SourceKind::Live
        && config.deduplicate_inherited_members
        && config.generate_classes;
    let flat_modules = preserve_flat.then(|| modules.clone());
    let module_tree_clones = usize::from(preserve_flat);

    let mut primary_modules = modules;
    if source == SourceKind::ExternalIr {
        expand_ir_base_closures(&mut primary_modules)?;
    } else if config.deduplicate_inherited_members && config.generate_classes {
        deduplicate_live_inheritance(&mut primary_modules, &config);
    }

    let used_live_dedup =
        source == SourceKind::Live && has_any_dedup_participant(&primary_modules, &config);
    let primary_graph = build_graph(
        acquisition.clone(),
        primary_modules,
        input_hashes.clone(),
        dependency_diagnostics.clone(),
        budget,
    )?;
    let primary = NormalizedCandidate {
        graph: primary_graph,
        config: config.clone(),
    };

    let flat = if used_live_dedup {
        let mut flat_config = config;
        flat_config.deduplicate_inherited_members = false;
        let flat_modules = flat_modules.ok_or_else(|| {
            crate::PyBindgenError::CodegenError(
                "live deduplication selected without a preserved flat candidate".to_owned(),
            )
        })?;
        Some(NormalizedCandidate {
            graph: build_graph(
                acquisition,
                flat_modules,
                input_hashes,
                dependency_diagnostics,
                budget,
            )?,
            config: flat_config,
        })
    } else {
        None
    };

    Ok(NormalizedCandidates {
        primary,
        flat,
        module_tree_clones,
    })
}

fn build_graph(
    acquisition: AcquisitionFacts,
    modules: Vec<Module>,
    input_hashes: HashMap<Path, String>,
    diagnostics: Vec<DomainDiagnostic>,
    budget: &crate::resource::BudgetTracker,
) -> Result<NormalizedGraph> {
    budget.check_structure(
        crate::ResourcePhase::Normalization,
        structural_resources(&modules),
    )?;
    NormalizedGraph::from_acquisition(acquisition, modules, input_hashes, diagnostics)
}

fn snapshot_input_hashes(modules: &[Module]) -> Result<HashMap<Path, String>> {
    fn collect(modules: &[Module], hashes: &mut HashMap<Path, String>) -> Result<()> {
        for module in modules {
            hashes.insert(module.name.clone(), module.input_hash()?);
            collect(&module.submodules, hashes)?;
        }
        Ok(())
    }

    let mut hashes = HashMap::default();
    collect(modules, &mut hashes)?;
    Ok(hashes)
}

fn canonicalize(
    source: SourceKind,
    mut modules: Vec<Module>,
    live_parents: Vec<Module>,
) -> Result<Vec<Module>> {
    match source {
        SourceKind::ExternalIr => {
            if !live_parents.is_empty() {
                return Err(crate::PyBindgenError::CodegenError(
                    "external-IR normalization cannot receive live parent modules".to_owned(),
                ));
            }
            // External IR remains Python-free. Dotted top-level modules are wrapped under
            // synthetic parents before the order-preserving merge collapses equal siblings.
            for module in &mut modules {
                if module.name.len() > 1 {
                    let module_name = module.name.clone();
                    *module =
                        (0..module_name.len() - 1)
                            .rev()
                            .fold(module.clone(), |package, index| {
                                let name = Path::from(&module_name[0..=index]);
                                let mut parent_package = Module::empty_local(name);
                                parent_package.submodules.push(package);
                                parent_package
                            });
                }
            }
            merge_modules_ordered(modules)
        }
        SourceKind::Live => {
            // Match the former live operation's parent-construction order: one module request
            // per dotted segment, deepest parent first, with the top-level module processed first.
            let parent_paths = required_live_parent_paths(&modules);
            let parents = live_parents;
            if parents.len() != parent_paths.len() {
                return Err(crate::PyBindgenError::CodegenError(format!(
                    "live parent factory returned {} modules for {} requested parents",
                    parents.len(),
                    parent_paths.len()
                )));
            }
            let mut parents = parents.into_iter();
            for module in &mut modules {
                if module.name.len() > 1 {
                    let module_name = module.name.clone();
                    let package = (0..module_name.len() - 1).rev().try_fold(
                        module.clone(),
                        |package, _| {
                            let mut parent_package = parents.next().ok_or_else(|| {
                                crate::PyBindgenError::CodegenError(
                                    "live parent factory exhausted its module list".to_owned(),
                                )
                            })?;
                            parent_package.submodules.push(package);
                            Ok::<_, crate::PyBindgenError>(parent_package)
                        },
                    )?;
                    *module = package;
                }
            }
            if parents.next().is_some() {
                return Err(crate::PyBindgenError::CodegenError(
                    "live parent factory returned unused modules".to_owned(),
                ));
            }
            merge_duplicate_modules(&mut modules);
            Ok(modules)
        }
    }
}

fn merge_duplicate_modules(modules: &mut Vec<Module>) {
    fn duplicate_ranges(modules: &mut [Module]) -> Vec<std::ops::Range<usize>> {
        modules.sort_by(|left, right| left.name.cmp(&right.name));
        let mut index = 0;
        let mut duplicates = Vec::new();
        while index < modules.len() {
            let name = modules[index].name.clone();
            let span = modules
                .iter()
                .skip(index)
                .take_while(|module| module.name == name)
                .count();
            if span > 1 {
                duplicates.push(index..index + span);
            }
            index += span;
        }
        duplicates
    }

    fn merge_recursive(input: &[Module]) -> Module {
        Module {
            prelude: input
                .iter()
                .flat_map(|module| module.prelude.iter().cloned())
                .unique()
                .collect(),
            imports: input
                .iter()
                .flat_map(|module| module.imports.iter().cloned())
                .unique()
                .collect(),
            submodules: {
                let mut submodules = input.iter().fold(Vec::default(), |mut all, module| {
                    all.extend(module.submodules.iter().cloned());
                    all
                });
                duplicate_ranges(&mut submodules)
                    .into_iter()
                    .rev()
                    .for_each(|range| {
                        submodules[range.start] = merge_recursive(&submodules[range.clone()]);
                        submodules.drain(range.start + 1..range.end);
                    });
                submodules
            },
            enums: input
                .iter()
                .flat_map(|module| module.enums.iter().cloned())
                .unique()
                .collect(),
            classes: input
                .iter()
                .flat_map(|module| module.classes.iter().cloned())
                .unique()
                .collect(),
            functions: input
                .iter()
                .flat_map(|module| module.functions.iter().cloned())
                .unique()
                .collect(),
            type_vars: input
                .iter()
                .flat_map(|module| module.type_vars.iter().cloned())
                .unique()
                .collect(),
            properties: input
                .iter()
                .flat_map(|module| module.properties.iter().cloned())
                .unique()
                .collect(),
            diagnostics: input
                .iter()
                .flat_map(|module| module.diagnostics.iter().cloned())
                .collect(),
            ..input[0].clone()
        }
    }

    for range in duplicate_ranges(modules).into_iter().rev() {
        modules[range.start] = merge_recursive(&modules[range.clone()]);
        modules.drain(range.start + 1..range.end);
    }
}

/// Order-preserving merge for the external-IR synthetic package tree.
fn merge_modules_ordered(modules: Vec<Module>) -> Result<Vec<Module>> {
    let mut order = Vec::<Path>::new();
    let mut groups: HashMap<Path, Vec<Module>> = HashMap::default();
    for module in modules {
        if !groups.contains_key(&module.name) {
            order.push(module.name.clone());
        }
        groups.entry(module.name.clone()).or_default().push(module);
    }

    order
        .into_iter()
        .map(|name| {
            let mut group = groups.remove(&name).unwrap_or_else(|| unreachable!());
            if group.len() == 1 {
                let mut only = group.pop().unwrap_or_else(|| unreachable!());
                only.submodules = merge_modules_ordered(only.submodules)?;
                if only.is_synthetic_external_ir_parent {
                    let synthetic_input_hash = only.ir_input_hash.clone().ok_or_else(|| {
                        crate::PyBindgenError::CodegenError(format!(
                            "synthetic external-IR parent '{}' has no input hash",
                            only.name
                        ))
                    })?;
                    only.ir_input_hash =
                        Some(merged_external_ir_input_hash(&only, &synthetic_input_hash)?);
                }
                Ok(only)
            } else {
                // Synthetic wrappers carry no authored scalar metadata. An explicit parent owns
                // the scalar fields even when the dotted child appeared first in caller order.
                let authored = group
                    .iter()
                    .find(|module| !module.is_synthetic_external_ir_parent)
                    .unwrap_or(&group[0]);
                let mut merged = authored.clone();
                let node_input_hash = merged.ir_input_hash.clone().ok_or_else(|| {
                    crate::PyBindgenError::CodegenError(format!(
                        "external-IR module '{}' has no input hash",
                        merged.name
                    ))
                })?;
                merged.submodules = Vec::new();
                merged.classes = Vec::new();
                merged.enums = Vec::new();
                merged.functions = Vec::new();
                merged.properties = Vec::new();
                merged.type_vars = Vec::new();
                merged.prelude = Vec::new();
                merged.imports = Vec::new();
                merged.diagnostics = Vec::new();
                for module in &group {
                    merged.submodules.extend(module.submodules.iter().cloned());
                    merged.classes.extend(module.classes.iter().cloned());
                    merged.enums.extend(module.enums.iter().cloned());
                    merged.functions.extend(module.functions.iter().cloned());
                    merged.properties.extend(module.properties.iter().cloned());
                    merged.type_vars.extend(module.type_vars.iter().cloned());
                    merged.prelude.extend(module.prelude.iter().cloned());
                    merged.imports.extend(module.imports.iter().cloned());
                    merged
                        .diagnostics
                        .extend(module.diagnostics.iter().cloned());
                }
                merged.submodules = merge_modules_ordered(merged.submodules)?;
                merged.ir_input_hash =
                    Some(merged_external_ir_input_hash(&merged, &node_input_hash)?);
                Ok(merged)
            }
        })
        .collect()
}

const MERGED_EXTERNAL_IR_FINGERPRINT_KIND: &str = "pyo3_bindgen.merged_external_ir";
const MERGED_EXTERNAL_IR_FINGERPRINT_VERSION: u32 = 1;

#[derive(serde::Serialize)]
struct MergedExternalIrFingerprint<'a> {
    kind: &'static str,
    version: u32,
    module: &'a Path,
    node_input_hash: &'a str,
    submodules: Vec<MergedExternalIrSubmodule<'a>>,
}

#[derive(serde::Serialize)]
struct MergedExternalIrSubmodule<'a> {
    module: &'a Path,
    input_hash: &'a str,
}

fn merged_external_ir_input_hash(module: &Module, node_input_hash: &str) -> Result<String> {
    let submodules = module
        .submodules
        .iter()
        .map(|submodule| {
            let input_hash = submodule.ir_input_hash.as_deref().ok_or_else(|| {
                crate::PyBindgenError::CodegenError(format!(
                    "external-IR submodule '{}' has no input hash",
                    submodule.name
                ))
            })?;
            Ok(MergedExternalIrSubmodule {
                module: &submodule.name,
                input_hash,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let fingerprint = MergedExternalIrFingerprint {
        kind: MERGED_EXTERNAL_IR_FINGERPRINT_KIND,
        version: MERGED_EXTERNAL_IR_FINGERPRINT_VERSION,
        module: &module.name,
        node_input_hash,
        submodules,
    };
    let bytes = serde_json::to_vec(&fingerprint).map_err(|error| {
        crate::PyBindgenError::CodegenError(format!(
            "failed to serialize merged external-IR fingerprint for '{}': {error}",
            module.name
        ))
    })?;
    Ok(crate::report::stable_hash(bytes))
}

/// Expand external-IR class bases to the full nearest-first C3 ancestor closure.
fn expand_ir_base_closures(modules: &mut [Module]) -> Result<()> {
    let mut direct: HashMap<Path, Vec<Path>> = HashMap::default();
    fn collect_direct(modules: &[Module], direct: &mut HashMap<Path, Vec<Path>>) {
        for module in modules {
            for class in &module.classes {
                direct.insert(class.name.clone(), class.direct_base_classes.clone());
            }
            collect_direct(&module.submodules, direct);
        }
    }
    collect_direct(modules, &mut direct);

    fn merge(mut sequences: Vec<Vec<Path>>, class: &Path) -> Result<Vec<Path>> {
        let mut result = Vec::new();
        loop {
            sequences.retain(|sequence| !sequence.is_empty());
            if sequences.is_empty() {
                return Ok(result);
            }
            let head = sequences
                .iter()
                .map(|sequence| &sequence[0])
                .find(|candidate| {
                    !sequences
                        .iter()
                        .any(|sequence| sequence[1..].contains(candidate))
                })
                .cloned()
                .ok_or_else(|| {
                    crate::PyBindgenError::CodegenError(format!(
                        "class hierarchy for '{class}' has no consistent Python C3 method resolution order"
                    ))
                })?;
            for sequence in &mut sequences {
                if sequence.first() == Some(&head) {
                    sequence.remove(0);
                }
            }
            result.push(head);
        }
    }

    fn linearize_all(direct: &HashMap<Path, Vec<Path>>) -> Result<HashMap<Path, Vec<Path>>> {
        let mut memo: HashMap<Path, Vec<Path>> = HashMap::default();
        for root in direct.keys() {
            if memo.contains_key(root) {
                continue;
            }
            let mut stack = vec![(root.clone(), false)];
            while let Some((class, expanded)) = stack.pop() {
                if memo.contains_key(&class) {
                    continue;
                }
                let bases = direct.get(&class).cloned().unwrap_or_default();
                if !expanded {
                    stack.push((class, true));
                    for base in bases.iter().rev() {
                        if !memo.contains_key(base) {
                            stack.push((base.clone(), false));
                        }
                    }
                    continue;
                }
                let mut sequences = bases
                    .iter()
                    .map(|base| {
                        memo.get(base).cloned().ok_or_else(|| {
                            crate::PyBindgenError::CodegenError(format!(
                                "base '{base}' of class '{class}' was not linearized"
                            ))
                        })
                    })
                    .collect::<Result<Vec<_>>>()?;
                if !bases.is_empty() {
                    sequences.push(bases);
                }
                let mut mro = vec![class.clone()];
                mro.extend(merge(sequences, &class)?);
                memo.insert(class, mro);
            }
        }
        Ok(memo)
    }

    fn apply(modules: &mut [Module], memo: &HashMap<Path, Vec<Path>>) -> Result<()> {
        for module in modules.iter_mut() {
            for class in module.classes.iter_mut() {
                let mut mro = memo.get(&class.name).cloned().ok_or_else(|| {
                    crate::PyBindgenError::CodegenError(format!(
                        "class '{}' has no computed method resolution order",
                        class.name
                    ))
                })?;
                if !mro.is_empty() {
                    mro.remove(0);
                }
                class.base_classes = mro;
            }
            apply(&mut module.submodules, memo)?;
        }
        Ok(())
    }

    let memo = linearize_all(&direct)?;
    apply(modules, &memo)
}

/// Reconcile live captured hierarchies with the generated class set and decide which connected
/// inheritance components can use the compact object model without increasing output size.
fn deduplicate_live_inheritance(modules: &mut [Module], config: &Config) {
    let mut class_paths: HashSet<Path> = HashSet::default();
    fn collect(modules: &[Module], traverse_submodules: bool, out: &mut HashSet<Path>) {
        for module in modules {
            for class in &module.classes {
                if crate::syntax::naming::rust_ident(
                    class.name.name(),
                    crate::syntax::naming::SymbolKind::Class,
                )
                .is_some()
                {
                    out.insert(class.name.clone());
                }
            }
            if traverse_submodules {
                collect(&module.submodules, traverse_submodules, out);
            }
        }
    }
    collect(modules, config.traverse_submodules, &mut class_paths);

    struct ClassStat {
        path: Path,
        saving: i64,
        own: usize,
        ancestors: usize,
        bases: Vec<Path>,
    }
    let mut stats = Vec::<ClassStat>::new();
    fn gather(
        modules: &mut [Module],
        class_paths: &HashSet<Path>,
        config: &Config,
        stats: &mut Vec<ClassStat>,
    ) {
        for module in modules.iter_mut() {
            for class in module.classes.iter_mut() {
                let (dropped_units, saving) =
                    class.filter_bases_report_droppable(class_paths, config);
                stats.push(ClassStat {
                    path: class.name.clone(),
                    saving,
                    own: class.forwarder_units() - dropped_units,
                    ancestors: class.ancestor_count(),
                    bases: class.base_class_paths().to_vec(),
                });
            }
            if config.traverse_submodules {
                gather(&mut module.submodules, class_paths, config, stats);
            }
        }
    }
    gather(modules, &class_paths, config, &mut stats);

    let ancestor_sets: HashMap<Path, HashSet<Path>> = stats
        .iter()
        .map(|stat| (stat.path.clone(), stat.bases.iter().cloned().collect()))
        .collect();
    let effective_direct_bases: HashMap<Path, Vec<Path>> = stats
        .iter()
        .map(|stat| {
            let bases = stat
                .bases
                .iter()
                .filter(|base| {
                    !stat.bases.iter().any(|nearer| {
                        nearer != *base
                            && ancestor_sets
                                .get(nearer)
                                .is_some_and(|ancestors| ancestors.contains(*base))
                    })
                })
                .cloned()
                .collect();
            (stat.path.clone(), bases)
        })
        .collect();

    let class_indices: HashMap<Path, usize> = stats
        .iter()
        .enumerate()
        .map(|(index, stat)| (stat.path.clone(), index))
        .collect();
    let mut parent: Vec<usize> = (0..stats.len()).collect();
    fn find(parent: &mut [usize], mut node: usize) -> usize {
        while parent[node] != node {
            parent[node] = parent[parent[node]];
            node = parent[node];
        }
        node
    }
    for (index, stat) in stats.iter().enumerate() {
        for base in &stat.bases {
            if let Some(&base_index) = class_indices.get(base) {
                let (left, right) = (find(&mut parent, index), find(&mut parent, base_index));
                if left != right {
                    parent[left] = right;
                }
            }
        }
    }
    let roots: Vec<usize> = (0..stats.len())
        .map(|index| find(&mut parent, index))
        .collect();

    let mut totals: HashMap<usize, (i64, usize, usize, usize)> = HashMap::default();
    for (index, stat) in stats.iter().enumerate() {
        let entry = totals.entry(roots[index]).or_default();
        entry.0 += stat.saving;
        entry.1 += stat.own;
        entry.2 += stat.ancestors;
        entry.3 += 1;
    }

    const COST_PER_OWN: i64 = 140;
    const COST_PER_ANCESTOR: i64 = 415;
    const COST_PER_CLASS: i64 = 30;
    let component_net: HashMap<usize, i64> = totals
        .iter()
        .map(|(&root, &(saving, own, ancestors, count))| {
            let cost = COST_PER_OWN * own as i64
                + COST_PER_ANCESTOR * ancestors as i64
                + COST_PER_CLASS * count as i64;
            (root, saving - cost)
        })
        .collect();
    let total_beneficial: i64 = component_net.values().copied().filter(|net| *net > 0).sum();
    let enable_dedup = total_beneficial > ROOT_SUPPORT_COST;
    let dedup_component: HashMap<usize, bool> = component_net
        .iter()
        .map(|(&root, &net)| (root, enable_dedup && net > 0))
        .collect();
    let decision: HashMap<Path, bool> = stats
        .iter()
        .enumerate()
        .map(|(index, stat)| (stat.path.clone(), dedup_component[&roots[index]]))
        .collect();

    fn apply(
        modules: &mut [Module],
        traverse_submodules: bool,
        decision: &HashMap<Path, bool>,
        effective_direct_bases: &HashMap<Path, Vec<Path>>,
    ) {
        for module in modules.iter_mut() {
            for class in module.classes.iter_mut() {
                let deduplicate = decision.get(&class.name).copied().unwrap_or(false);
                if deduplicate {
                    class.set_effective_direct_bases(
                        effective_direct_bases
                            .get(&class.name)
                            .cloned()
                            .unwrap_or_default(),
                    );
                    class.apply_inheritance_dedup();
                }
                class.set_dedup_participant(deduplicate);
            }
            if traverse_submodules {
                apply(
                    &mut module.submodules,
                    traverse_submodules,
                    decision,
                    effective_direct_bases,
                );
            }
        }
    }
    apply(
        modules,
        config.traverse_submodules,
        &decision,
        &effective_direct_bases,
    );
}

fn validate_module_tree(
    kind: SourceKind,
    modules: &[Module],
    paths: &mut HashSet<Path>,
) -> Result<()> {
    for module in modules {
        if module.name.is_empty() {
            return Err(crate::PyBindgenError::CodegenError(
                "normalized graph contains an empty module identity".to_owned(),
            ));
        }
        if !paths.insert(module.name.clone()) {
            return Err(crate::PyBindgenError::CodegenError(format!(
                "normalized graph contains duplicate module identity '{}'",
                module.name
            )));
        }
        let has_ir_provenance = module.ir_input_hash.is_some();
        if has_ir_provenance != kind.is_external_ir() {
            return Err(crate::PyBindgenError::CodegenError(format!(
                "normalized {} graph contains module '{}' with {} provenance",
                kind.as_str(),
                module.name,
                if has_ir_provenance {
                    "external-IR"
                } else {
                    "live"
                }
            )));
        }
        validate_module_tree(kind, &module.submodules, paths)?;
    }
    Ok(())
}

/// Immutable result of normalization and generation planning.
///
/// This boundary owns every policy-derived index needed by emission. It contains no wire DTO,
/// Python handle, filesystem path, or token stream; token construction can therefore consume this
/// result without rediscovering graph-wide names or hierarchy facts.
pub(crate) struct NormalizedPlan<'graph> {
    graph: &'graph NormalizedGraph,
    config: Config,
    import_mode: ImportMode,
    all_types: Vec<(Path, Path)>,
    non_newtype_paths: HashSet<Path>,
    all_classes: HashMap<Path, &'graph Class>,
    inherited_members: HashMap<Path, crate::syntax::class::InheritedMemberPlan>,
    emit_collection_wrapper: bool,
    emit_root_support: bool,
}

impl<'graph> NormalizedPlan<'graph> {
    pub(crate) fn build(
        graph: &'graph NormalizedGraph,
        config: &Config,
        live_import_prefixes: Option<ImportPrefixes>,
    ) -> Result<Self> {
        let config = config.clone();
        let external_ir = graph.source().kind().is_external_ir();
        let import_mode = match (external_ir, live_import_prefixes) {
            (true, None) => ImportMode::Static(collect_module_paths(graph.modules())),
            (false, Some(prefixes)) => ImportMode::Live {
                prefixes,
                deduplicated: config.deduplicate_inherited_members,
            },
            (true, Some(_)) => {
                return Err(crate::PyBindgenError::CodegenError(
                    "external-IR plan cannot contain live import prefixes".to_owned(),
                ));
            }
            (false, None) => {
                return Err(crate::PyBindgenError::CodegenError(
                    "live plan requires pre-resolved import prefixes".to_owned(),
                ));
            }
        };
        let any_class = has_any_class(graph.modules(), &config);
        let any_collection = has_any_collection(graph.modules(), &config);
        let any_dedup_participant = has_any_dedup_participant(graph.modules(), &config);
        validate_root_type_namespace(
            graph.modules(),
            external_ir,
            any_class,
            any_collection,
            any_dedup_participant,
        )?;
        let GeneratedTypeIndex {
            paths: all_types,
            non_newtype_paths,
        } = get_all_types_with_import_aliases(graph.modules(), &config, &import_mode)?;
        let all_classes = get_all_classes(graph.modules(), &config);
        let inherited_members = all_classes
            .iter()
            .filter(|(_, class)| class.uses_dedup_model(&import_mode))
            .map(|(path, class)| {
                (
                    path.clone(),
                    crate::syntax::class::InheritedMemberPlan::build(class, &all_classes),
                )
            })
            .collect();
        let emit_collection_wrapper = external_ir && any_collection;
        let emit_root_support =
            (external_ir && (any_class || emit_collection_wrapper)) || any_dedup_participant;
        Ok(Self {
            graph,
            config,
            import_mode,
            all_types,
            non_newtype_paths,
            all_classes,
            inherited_members,
            emit_collection_wrapper,
            emit_root_support,
        })
    }

    pub(crate) const fn graph(&self) -> &'graph NormalizedGraph {
        self.graph
    }

    pub(crate) const fn config(&self) -> &Config {
        &self.config
    }

    pub(crate) const fn import_mode(&self) -> &ImportMode {
        &self.import_mode
    }

    pub(crate) fn import_mode_name(&self) -> &'static str {
        match &self.import_mode {
            ImportMode::Live {
                deduplicated: false,
                ..
            } => "live_flat",
            ImportMode::Live {
                deduplicated: true, ..
            } => "live_deduplicated",
            ImportMode::Static(_) => "static_external_ir",
        }
    }

    pub(crate) fn all_types(&self) -> &[(Path, Path)] {
        &self.all_types
    }

    pub(crate) const fn non_newtype_paths(&self) -> &HashSet<Path> {
        &self.non_newtype_paths
    }

    pub(crate) const fn all_classes(&self) -> &HashMap<Path, &'graph Class> {
        &self.all_classes
    }

    pub(crate) const fn inherited_members(
        &self,
    ) -> &HashMap<Path, crate::syntax::class::InheritedMemberPlan> {
        &self.inherited_members
    }

    pub(crate) const fn emit_collection_wrapper(&self) -> bool {
        self.emit_collection_wrapper
    }

    pub(crate) const fn emit_root_support(&self) -> bool {
        self.emit_root_support
    }
}

struct GeneratedTypeIndex {
    paths: Vec<(Path, Path)>,
    non_newtype_paths: HashSet<Path>,
}

fn has_any_class(modules: &[Module], config: &Config) -> bool {
    fn any_class(modules: &[Module], traverse_submodules: bool) -> bool {
        modules.iter().any(|module| {
            !module.classes.is_empty()
                || (traverse_submodules && any_class(&module.submodules, traverse_submodules))
        })
    }
    config.generate_classes && any_class(modules, config.traverse_submodules)
}

fn has_any_dedup_participant(modules: &[Module], config: &Config) -> bool {
    fn any_participant(modules: &[Module], traverse_submodules: bool) -> bool {
        modules.iter().any(|module| {
            module.classes.iter().any(Class::is_dedup_participant)
                || (traverse_submodules && any_participant(&module.submodules, traverse_submodules))
        })
    }
    any_participant(modules, config.traverse_submodules)
}

fn has_any_collection(modules: &[Module], config: &Config) -> bool {
    modules
        .iter()
        .any(|module| module.mentions_enabled_collection(config))
}

fn validate_root_type_namespace(
    modules: &[Module],
    external_ir: bool,
    any_class: bool,
    any_collection: bool,
    any_dedup_participant: bool,
) -> Result<()> {
    let mut symbols = crate::syntax::naming::SymbolTable::new("type");
    if (external_ir && (any_class || any_collection)) || any_dedup_participant {
        for generated_name in [
            crate::syntax::class::ROOT_TRAIT_NAME,
            crate::syntax::class::PY_TYPE_MARKER_NAME,
        ] {
            symbols.register(generated_name, "generated root support")?;
        }
    }
    if external_ir && any_collection {
        symbols.register(
            crate::syntax::class::COLLECTION_WRAPPER_NAME,
            "generated root support",
        )?;
    }
    for module in modules {
        if let Some(ident) = crate::syntax::naming::rust_ident(
            module.name.name(),
            crate::syntax::naming::SymbolKind::Module,
        ) {
            symbols.register(&ident.to_string(), module.name.to_py())?;
        }
    }
    Ok(())
}

fn collect_module_paths(modules: &[Module]) -> HashSet<Path> {
    fn collect(modules: &[Module], paths: &mut HashSet<Path>) {
        for module in modules {
            paths.insert(module.name.clone());
            collect(&module.submodules, paths);
        }
    }
    let mut paths = HashSet::default();
    collect(modules, &mut paths);
    paths
}

fn get_all_types(modules: &[Module], config: &Config) -> Vec<(Path, Path)> {
    fn collect(input: &[Module], config: &Config) -> Vec<(Path, Path)> {
        let mut types = Vec::new();
        for module in input {
            if config.generate_classes {
                types.extend(module.classes.iter().filter_map(|class| {
                    crate::syntax::naming::rust_path(
                        &class.name,
                        crate::syntax::naming::SymbolKind::Class,
                    )
                    .map(|rust_path| (class.name.clone(), rust_path))
                }));
            }
            if config.generate_enums {
                types.extend(module.enums.iter().filter_map(|py_enum| {
                    crate::syntax::naming::rust_path(
                        &py_enum.name,
                        crate::syntax::naming::SymbolKind::Enum,
                    )
                    .map(|rust_path| (py_enum.name.clone(), rust_path))
                }));
            }
            if config.traverse_submodules {
                types.extend(collect(&module.submodules, config));
            }
        }
        types
    }
    collect(modules, config)
        .into_iter()
        .unique_by(|(python_path, _)| python_path.clone())
        .collect()
}

fn get_non_newtype_base_types(modules: &[Module], config: &Config) -> HashSet<Path> {
    fn collect(modules: &[Module], config: &Config, paths: &mut HashSet<Path>) {
        for module in modules {
            if config.generate_enums {
                paths.extend(module.enums.iter().map(|py_enum| py_enum.name.clone()));
            }
            if config.generate_type_vars {
                paths.extend(
                    module
                        .type_vars
                        .iter()
                        .map(|type_var| type_var.name.clone()),
                );
            }
            if config.traverse_submodules {
                collect(&module.submodules, config, paths);
            }
        }
    }
    let mut paths = HashSet::default();
    collect(modules, config, &mut paths);
    paths
}

fn get_all_types_with_import_aliases(
    modules: &[Module],
    config: &Config,
    import_mode: &ImportMode,
) -> Result<GeneratedTypeIndex> {
    fn collect_imports<'module>(
        modules: &'module [Module],
        config: &Config,
        import_mode: &ImportMode,
        imports: &mut Vec<&'module Import>,
    ) {
        for module in modules {
            imports.extend(
                module
                    .imports
                    .iter()
                    .filter(|import| Module::import_is_emitted(import, import_mode)),
            );
            if config.traverse_submodules {
                collect_imports(&module.submodules, config, import_mode, imports);
            }
        }
    }

    let direct_types = get_all_types(modules, config);
    let direct_non_newtypes = get_non_newtype_base_types(modules, config);
    if !config.generate_imports {
        return Ok(GeneratedTypeIndex {
            paths: direct_types,
            non_newtype_paths: direct_non_newtypes,
        });
    }
    let mut all_types = direct_types.clone();
    let mut all_non_newtypes = direct_non_newtypes.clone();
    let mut imports = Vec::new();
    collect_imports(modules, config, import_mode, &mut imports);
    for import in imports {
        let Some((canonical_origin, _, _)) = Module::resolve_emitted_rust_path(
            modules,
            &import.origin,
            config,
            import_mode,
            &mut Vec::new(),
            &mut Vec::new(),
        )?
        else {
            continue;
        };
        for (python_path, rust_path) in &direct_types {
            if python_path.starts_with(&canonical_origin) {
                let suffix = Path::from(&python_path[canonical_origin.len()..]);
                all_types.push((import.target.concat(&suffix), rust_path.clone()));
            }
        }
        for python_path in &direct_non_newtypes {
            if python_path.starts_with(&canonical_origin) {
                let suffix = Path::from(&python_path[canonical_origin.len()..]);
                all_non_newtypes.insert(import.target.concat(&suffix));
            }
        }
    }
    Ok(GeneratedTypeIndex {
        paths: all_types
            .into_iter()
            .unique_by(|(python_path, _)| python_path.clone())
            .collect(),
        non_newtype_paths: all_non_newtypes,
    })
}

fn get_all_classes<'module>(
    modules: &'module [Module],
    config: &Config,
) -> HashMap<Path, &'module Class> {
    fn collect<'module>(
        modules: &'module [Module],
        config: &Config,
        classes: &mut HashMap<Path, &'module Class>,
    ) {
        for module in modules {
            if config.generate_classes {
                for class in &module.classes {
                    classes.entry(class.name.clone()).or_insert(class);
                }
            }
            if config.traverse_submodules {
                collect(&module.submodules, config, classes);
            }
        }
    }
    let mut classes = HashMap::default();
    collect(modules, config, &mut classes);
    classes
}

fn collect_module_diagnostics(modules: &[Module], diagnostics: &mut Vec<DomainDiagnostic>) {
    for module in modules {
        diagnostics.extend(module.diagnostics.iter().cloned());
        collect_module_diagnostics(&module.submodules, diagnostics);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        schema::{
            ClassIr, EnumIr, EnumKindIr, EnumVariantIr, FunctionIr, FunctionKindIr, ModuleIr,
            ParameterIr, ParameterKindIr,
        },
        Config,
    };

    fn representative_external_module() -> Module {
        let ir = ModuleIr {
            name: "tracer".to_owned(),
            docstring: Some("tracer module".to_owned()),
            is_package: true,
            classes: vec![
                ClassIr {
                    name: "tracer.Derived".to_owned(),
                    base_classes: vec!["tracer.Base".to_owned()],
                    docstring: None,
                    methods: vec![],
                    properties: vec![],
                },
                ClassIr {
                    name: "tracer.Base".to_owned(),
                    base_classes: vec![],
                    docstring: None,
                    methods: vec![],
                    properties: vec![],
                },
            ],
            enums: vec![EnumIr {
                name: "tracer.Kind".to_owned(),
                kind: EnumKindIr::Enum,
                variants: vec![EnumVariantIr {
                    py_name: "ONE".to_owned(),
                    value_repr: "1".to_owned(),
                }],
                aliases: vec![],
                docstring: None,
                is_flag: false,
                is_open: false,
                string_backed: false,
                plain_mirror: false,
            }],
            functions: vec![FunctionIr {
                name: "tracer.make".to_owned(),
                kind: FunctionKindIr::Function,
                params: vec![ParameterIr {
                    name: "value".to_owned(),
                    kind: ParameterKindIr::PositionalOrKeyword,
                    annotation: "int".to_owned(),
                    has_default: false,
                }],
                return_type: "int".to_owned(),
                docstring: None,
            }],
            properties: vec![],
            submodules: vec![],
            prelude: vec![],
            imports: vec![],
            type_vars: vec![],
        };
        Module::from_ir(&Config::default(), &ir).expect("representative module lowers")
    }

    fn representative_live_module() -> Module {
        pyo3::Python::initialize();
        pyo3::Python::attach(|py| {
            crate::runtime_sentinels::prime(py).expect("live runtime primes");
            crate::runtime_sentinels::with_restored(py, || {
                let source = std::ffi::CString::new(
                    "import enum\n\
                     class Base:\n    pass\n\
                     class Derived(Base):\n    pass\n\
                     class Kind(enum.Enum):\n    ONE = 1\n\
                     def make(value: int) -> int:\n    return value\n\
                     def __dir__():\n    return ['Base', 'Derived', 'Kind', 'make', 'ghost']\n",
                )
                .unwrap();
                let filename = std::ffi::CString::new("tracer.py").unwrap();
                let name = std::ffi::CString::new("tracer").unwrap();
                let module = pyo3::types::PyModule::from_code(
                    py,
                    source.as_c_str(),
                    filename.as_c_str(),
                    name.as_c_str(),
                )?;
                Module::parse(&Config::default(), &module)
            })
        })
        .expect("representative live module parses")
    }

    fn graph(kind: SourceKind) -> NormalizedGraph {
        let module = match kind {
            SourceKind::Live => representative_live_module(),
            SourceKind::ExternalIr => representative_external_module(),
        };
        let mut hashes = HashMap::default();
        hashes.insert(module.name.clone(), module.input_hash().unwrap());
        match kind {
            SourceKind::Live => NormalizedGraph::from_live(
                vec![module],
                hashes,
                Vec::new(),
                vec!["test_live_adapter".to_owned()],
            ),
            SourceKind::ExternalIr => NormalizedGraph::from_external_ir(
                vec![module],
                hashes,
                Vec::new(),
                vec!["test_ir_adapter".to_owned()],
            ),
        }
        .expect("representative graph validates")
    }

    #[test]
    fn interface_plans_real_live_and_external_graphs() {
        for kind in [SourceKind::Live, SourceKind::ExternalIr] {
            let graph = graph(kind);
            let prefixes = (kind == SourceKind::Live).then(|| {
                ImportPrefixes::new(
                    graph
                        .required_import_paths(&Config::default())
                        .into_iter()
                        .map(|path| {
                            let package = path.root().expect("non-empty path");
                            (path, package)
                        }),
                )
                .expect("test prefixes validate")
            });
            let plan = NormalizedPlan::build(&graph, &Config::default(), prefixes)
                .expect("representative graph plans");
            assert_eq!(graph.source().kind(), kind);
            assert_eq!(graph.source().identity.roots[0].to_py(), "tracer");
            assert_eq!(graph.source().identity.content_hashes.len(), 1);
            assert_eq!(graph.source().provenance.adapters.len(), 1);
            assert_eq!(
                graph.source().limits.content,
                match kind {
                    SourceKind::Live => SourceLimitScope::NotByteAddressed,
                    SourceKind::ExternalIr => SourceLimitScope::MaterializedWholeDocument,
                }
            );
            assert!(graph.completeness().is_complete());
            assert_eq!(plan.all_classes().len(), 2);
            assert_eq!(
                plan.inherited_members().len(),
                if kind == SourceKind::ExternalIr { 2 } else { 0 },
                "live nonparticipants must not allocate unused inherited-member plans"
            );
            assert!(plan.all_types().len() >= 3);
            assert_eq!(plan.emit_root_support(), kind == SourceKind::ExternalIr);
            if kind == SourceKind::Live {
                assert!(graph.modules()[0]
                    .diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.code.as_str() == "attribute_access_failed"));
            }
        }
    }

    #[test]
    fn structural_resource_measurements_are_deterministic() {
        let graph = graph(SourceKind::ExternalIr);
        let first = graph.resources().expect("resource measurement succeeds");
        let second = graph.resources().expect("resource measurement repeats");

        assert_eq!(first, second);
        assert_eq!(first.definitions, 6);
        assert_eq!(first.recursion_depth, 3);
        assert!(first.model_nodes > first.definitions);
    }

    #[test]
    fn structural_budget_runs_before_recursive_normalization_validation() {
        let mut module = Module::empty_local(Path::from_py("deep.leaf"));
        for depth in (0..64).rev() {
            let mut parent = Module::empty_local(Path::from_py(&format!("deep.level_{depth}")));
            parent.submodules.push(module);
            module = parent;
        }
        let budget = crate::GenerationBudget {
            max_recursion_depth: 8,
            max_peak_rss_delta_bytes: None,
            ..crate::GenerationBudget::default()
        };
        let error = normalize(
            NormalizationInput {
                acquisition: AcquisitionFacts::live(vec!["test".to_owned()]),
                config: Config::default(),
                modules: vec![module],
                dependency_diagnostics: Vec::new(),
                live_parents: Vec::new(),
            },
            &crate::resource::BudgetTracker::new(budget),
        )
        .expect_err("structural budget must reject before recursive validation");
        let crate::PyBindgenError::ResourceLimitExceeded(error) = error else {
            panic!("expected typed structural limit, got {error}");
        };
        assert_eq!(error.kind, crate::ResourceKind::RecursionDepth);
        assert_eq!(error.phase, crate::ResourcePhase::Normalization);
        assert!(error.observed > error.limit);
    }

    #[test]
    fn interface_rejects_duplicate_module_identity() {
        let first = representative_live_module();
        let second = first.clone();
        let mut hashes = HashMap::default();
        hashes.insert(first.name.clone(), first.input_hash().unwrap());
        let error = NormalizedGraph::from_live(
            vec![first, second],
            hashes,
            Vec::new(),
            vec!["test".to_owned()],
        )
        .expect_err("duplicate module identities must not enter emission");
        assert!(error.to_string().contains("duplicate module identity"));
    }

    #[test]
    fn interface_rejects_mislabeled_source_provenance() {
        let live = representative_live_module();
        let mut live_hashes = HashMap::default();
        live_hashes.insert(live.name.clone(), live.input_hash().unwrap());
        let error = NormalizedGraph::from_external_ir(
            vec![live],
            live_hashes,
            Vec::new(),
            vec!["test".to_owned()],
        )
        .expect_err("live modules cannot enter an external graph");
        assert!(error.to_string().contains("with live provenance"));

        let external = representative_external_module();
        let mut external_hashes = HashMap::default();
        external_hashes.insert(external.name.clone(), external.input_hash().unwrap());
        let error = NormalizedGraph::from_live(
            vec![external],
            external_hashes,
            Vec::new(),
            vec!["test".to_owned()],
        )
        .expect_err("external modules cannot enter a live graph");
        assert!(error.to_string().contains("with external-IR provenance"));
    }
}
