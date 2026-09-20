use crate::{
    emission::EmissionPolicy,
    normalized::{normalize, AcquisitionFacts, NormalizationInput, SourceKind},
    report::{DomainDiagnostic, GenerationReport, OutputMetadata},
    resource::{BudgetTracker, ResourcePhase, StructuralResources},
    schema::SchemaIr,
    syntax::{Module, Path},
    BindingSource, Config, GenerationBudget, PyBindgenError, Result, SchemaSource,
};
use pyo3::prelude::*;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum SourceMode {
    #[default]
    Unset,
    Live,
    ExternalIr,
}

impl SourceMode {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Unset => "unset",
            Self::Live => "live",
            Self::ExternalIr => "external_ir",
        }
    }
}

/// Engine for automatic generation of Rust FFI bindings to Python modules.
///
/// # Examples
///
/// Here is a simple example of how to use the `Codegen` engine to generate
/// Rust FFI bindings for the full `os` and `sys` Python modules. With the
/// default configuration, all submodules, classes, functions, and parameters
/// will be recursively parsed and included in the generated bindings.
///
/// ```
/// # use pyo3_bindgen_engine::{Codegen, Config};
/// fn main() -> Result<(), Box<dyn std::error::Error>> {
///     Codegen::new(Config::default())
///         .module_name("os")?
///         .module_name("sys")?
///         .generate()?;
///     Ok(())
/// }
/// ```
///
/// For more focused generation, paths to specific submodules can be provided.
/// In the following example, only the `entities` and `parser` submodules of the
/// `html` module will be included in the generated bindings alongside their
/// respective submodules, classes, functions, and parameters. No direct attributes
/// or submodules of the `html` top-level module will be included.
///
/// ```
/// # use pyo3_bindgen_engine::Codegen;
/// fn main() -> Result<(), Box<dyn std::error::Error>> {
///     Codegen::default()
///         .module_names(["html.entities", "html.parser"])?
///         .generate()?;
///     Ok(())
/// }
/// ```
#[derive(Debug, Default, Clone)]
pub struct Codegen {
    cfg: Config,
    budget: GenerationBudget,
    modules: Vec<Module>,
    /// Python source code included by [`Self::module_from_str()`] in the generated Rust bindings.
    embedded_source_code: HashMap<String, String>,
    /// A generation run has exactly one source adapter. Mixing live Python objects
    /// with external IR would apply one global import strategy to incompatible modules.
    source_mode: SourceMode,
    /// Raw external-IR state retained until terminal generation.
    external_ir_state: Option<crate::external_ir::ExternalIrState>,
    /// Exact bounded transport facts when an adapter can prove them.
    external_ir_source_context: Option<crate::source::ExternalIrSourceContext>,
    /// Discovery diagnostics are collected outside the parsed module tree so speculative
    /// live candidates can carry the same per-origin facts into their selected report.
    dependency_diagnostics: Vec<DomainDiagnostic>,
    /// Ordered, deduplicated acquisition chain retained by the private normalized source context.
    source_provenance: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct GeneratedBindings {
    pub tokens: proc_macro2::TokenStream,
    pub report: GenerationReport,
}

/// Canonically formatted bindings and their structured generation report.
#[derive(Debug, Clone)]
pub struct GeneratedArtifact {
    pub artifact: crate::RustArtifact,
    pub report: GenerationReport,
    /// Deterministic whole-operation work accounting for scale evidence.
    pub work: GenerationWork,
}

/// A generation error together with the report context available at the failure boundary.
#[derive(Debug)]
pub struct GeneratedArtifactFailure {
    error: PyBindgenError,
    report: Box<GenerationReport>,
}

impl GeneratedArtifactFailure {
    fn new(error: PyBindgenError, operation_id: Option<String>) -> Self {
        let mut report = GenerationReport::empty();
        if let Some(operation_id) = operation_id {
            report.operation_id = Some(operation_id);
        }
        Self {
            error,
            report: Box::new(report),
        }
    }

    /// Return the underlying typed generation error.
    #[must_use]
    pub const fn error(&self) -> &PyBindgenError {
        &self.error
    }

    /// Return the failure report carrying any established operation identity.
    #[must_use]
    pub fn report(&self) -> &GenerationReport {
        self.report.as_ref()
    }

    /// Split the failure into its typed error and report context.
    #[must_use]
    pub fn into_parts(self) -> (PyBindgenError, GenerationReport) {
        (self.error, *self.report)
    }
}

impl std::fmt::Display for GeneratedArtifactFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.error.fmt(formatter)
    }
}

impl std::error::Error for GeneratedArtifactFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}

/// Deterministic terminal work performed by one generation operation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct GenerationWork {
    /// Complete external graphs validated and lowered. Live generation records zero.
    pub validation_lowering_passes: usize,
    /// Complete candidate sets normalized.
    pub normalization_passes: usize,
    /// Full module-tree clones used to preserve adaptive candidates.
    pub module_tree_clones: usize,
    /// Canonical Rust artifacts rendered across all attempted candidates.
    pub canonical_renders: usize,
}

impl Codegen {
    fn select_source_mode(&mut self, requested: SourceMode) -> Result<()> {
        match self.source_mode {
            SourceMode::Unset => {
                self.source_mode = requested;
                Ok(())
            }
            current if current == requested => Ok(()),
            _ => Err(PyBindgenError::CodegenError(
                "cannot mix live Python introspection and external IR in one Codegen; use separate Codegen instances"
                    .to_owned(),
            )),
        }
    }

    fn record_source_provenance(&mut self, provenance: impl Into<String>) {
        let provenance = provenance.into();
        if !self.source_provenance.contains(&provenance) {
            self.source_provenance.push(provenance);
        }
    }

    fn ensure_live_module_path_available(&self, module_path: &Path) -> Result<()> {
        fn contains(modules: &[Module], module_path: &Path) -> bool {
            modules.iter().any(|module| {
                &module.name == module_path || contains(&module.submodules, module_path)
            })
        }

        if contains(&self.modules, module_path) {
            Err(PyBindgenError::CodegenError(format!(
                "duplicate live module path '{module_path}'"
            )))
        } else {
            Ok(())
        }
    }

    fn ensure_live_module_tree_available(&self, module: &Module) -> Result<()> {
        fn check(codegen: &Codegen, module: &Module, seen: &mut HashSet<Path>) -> Result<()> {
            if !seen.insert(module.name.clone()) {
                return Err(PyBindgenError::CodegenError(format!(
                    "duplicate live module path '{}'",
                    module.name
                )));
            }
            codegen.ensure_live_module_path_available(&module.name)?;
            for submodule in &module.submodules {
                check(codegen, submodule, seen)?;
            }
            Ok(())
        }

        check(self, module, &mut HashSet::default())
    }

    fn add_live_module(mut self, parsed: Module) -> Result<Self> {
        let actual_path = parsed.name.clone();
        self.ensure_live_module_path_available(&actual_path)?;
        self.ensure_live_module_tree_available(&parsed)?;
        tracing::debug!(
            source = "live",
            module = %parsed.name,
            submodules = parsed.submodules.len(),
            "binding source registered"
        );
        self.modules.push(parsed);
        Ok(self)
    }

    /// Create a new `Codegen` engine with the given configuration.
    #[must_use]
    pub fn new(cfg: Config) -> Self {
        Self {
            cfg,
            ..Default::default()
        }
    }

    /// Select operation resource limits independently from generation configuration.
    #[must_use]
    pub fn with_budget(mut self, budget: GenerationBudget) -> Self {
        self.budget = budget;
        self
    }

    /// Snapshot stdlib identities used by live introspection.
    ///
    /// [`Self::module_name`] and [`Self::module_from_str`] call this automatically
    /// before importing or executing target code. Call it explicitly before executing
    /// a module which mutates public attributes of `inspect`, `functools`,
    /// `_functools`, `enum`, `typing`, `dataclasses`, `types`, `pkgutil`,
    /// `annotationlib`, or `builtins`, then pass that live module to
    /// [`Self::module`]. First use validates stable stdlib anchor provenance before any
    /// identities become the restoration baseline; an already-replaced anchor is rejected and
    /// leaves the runtime unprimed.
    pub fn prime_live_runtime(py: Python<'_>) -> Result<()> {
        crate::live_operation::prime(py)
    }

    /// Add a caller-created Python module to the generation input.
    ///
    /// Call [`Self::prime_live_runtime`] before importing or executing the target module. This
    /// method rejects an unprimed interpreter, and priming itself rejects replaced stdlib anchors,
    /// so target mutations cannot silently become the sentinel baseline.
    pub fn module(mut self, module: &pyo3::Bound<pyo3::types::PyModule>) -> Result<Self> {
        self.select_source_mode(SourceMode::Live)?;
        self.record_source_provenance("live:preloaded_module");
        let policy = crate::live_operation::LivePolicy::from_config(&self.cfg);
        crate::live_operation::with_preloaded(module, policy, |session| {
            let parsed = session.inspect_module(&self.cfg, module, None)?;
            self.add_live_module(parsed)
        })
    }

    /// Add a Python module by its name to the list of modules for which to generate bindings.
    pub fn module_name(mut self, module_name: &str) -> Result<Self> {
        let span = tracing::info_span!(
            "pyo3_bindgen.source.load",
            source = "live",
            module = module_name,
        );
        let _entered = span.enter();
        self.select_source_mode(SourceMode::Live)?;
        self.record_source_provenance("live:module_import");
        let expected_path = Path::from_live_module(module_name)?;
        self.ensure_live_module_path_available(&expected_path)?;
        let policy = crate::live_operation::LivePolicy::from_config(&self.cfg);
        crate::live_operation::with_session(policy, |session| {
            // Python may expose an imported module through an alias whose requested path
            // differs from `module.__name__` (for example, `os.path` -> `posixpath`).
            let parsed = session.import_module(&self.cfg, &expected_path)?;
            self.add_live_module(parsed)
        })
    }

    /// Add a Python module from its source code and name to the list of modules for which to generate bindings.
    ///
    /// # Note
    ///
    /// When including a module in this way, the Python source code must be available also during runtime for
    /// the underlying Python interpreter.
    ///
    /// For convenience, you can call `module_name::pyo3_embed_python_source_code()` that is automatically
    /// generated in the Rust bindings. This function must be called before attempting to use any functions
    /// of classes from the module.
    pub fn module_from_str(mut self, source_code: &str, module_name: &str) -> Result<Self> {
        let span = tracing::info_span!(
            "pyo3_bindgen.source.load",
            source = "embedded_python",
            module = module_name,
            source_bytes = source_code.len(),
        );
        let _entered = span.enter();
        self.select_source_mode(SourceMode::Live)?;
        self.record_source_provenance("live:embedded_source");
        let expected_path = Path::from_live_module(module_name)?;
        self.ensure_live_module_path_available(&expected_path)?;
        let canonical_name = expected_path.to_py();
        let policy = crate::live_operation::LivePolicy::from_config(&self.cfg);
        let mut codegen = crate::live_operation::with_session(policy, |session| {
            let parsed = session.execute_module(&self.cfg, source_code, &expected_path)?;
            self.add_live_module(parsed)
        })?;
        codegen
            .embedded_source_code
            .insert(canonical_name, source_code.to_owned());
        Ok(codegen)
    }

    /// Add multiple caller-created Python modules.
    ///
    /// As with [`Self::module`], prime the runtime before importing or executing any target.
    pub fn modules<'py>(
        mut self,
        modules: impl AsRef<[pyo3::Bound<'py, pyo3::types::PyModule>]>,
    ) -> Result<Self> {
        self.select_source_mode(SourceMode::Live)?;
        let modules = modules.as_ref();
        self.modules.reserve(modules.len());
        for module in modules {
            self = self.module(module)?;
        }
        Ok(self)
    }

    /// Add multiple Python modules by their names to the list of modules for which to generate bindings.
    pub fn module_names<'a>(mut self, module_names: impl AsRef<[&'a str]>) -> Result<Self> {
        self.select_source_mode(SourceMode::Live)?;
        let module_names = module_names.as_ref();
        self.modules.reserve(module_names.len());
        for module_name in module_names {
            self = self.module_name(module_name)?;
        }
        Ok(self)
    }

    /// Add all modules from a pre-built external schema ([`SchemaIr`]) to the list
    /// of modules for which to generate bindings, **without** any live Python
    /// introspection.
    ///
    /// The caller-supplied module order is preserved verbatim. This path performs no
    /// Python interpreter initialization, attachment, import, or introspection. Raw schemas are
    /// accumulated here; schema validation and lowering occur once at [`Self::generate`] or
    /// [`Self::build`]. The engine crate itself still uses PyO3 types and build configuration.
    pub fn modules_from_ir(self, schema: SchemaIr) -> Result<Self> {
        self.register_external_ir(schema, None)
    }

    /// Register external IR together with exact bounded transport facts.
    ///
    /// This adapter-facing entry point preserves the public [`SchemaIr`] identity while allowing
    /// byte-addressed callers such as the CLI to carry acquisition identity and limits through
    /// normalization and report assembly.
    #[doc(hidden)]
    pub fn modules_from_ir_with_source_context(
        self,
        schema: SchemaIr,
        context: crate::source::ExternalIrSourceContext,
    ) -> Result<Self> {
        let span = tracing::info_span!(
            "pyo3_bindgen.source.acquire",
            adapter = "bounded_external_ir",
        );
        let _entered = span.enter();
        self.register_external_ir(schema, Some(context))
    }

    fn register_external_ir(
        mut self,
        schema: SchemaIr,
        context: Option<crate::source::ExternalIrSourceContext>,
    ) -> Result<Self> {
        let incoming_modules = schema.modules.len();
        let span = tracing::info_span!(
            "pyo3_bindgen.source.load",
            source = "external_ir",
            incoming_modules,
        );
        let _entered = span.enter();
        self.select_source_mode(SourceMode::ExternalIr)?;
        if self.external_ir_state.is_some()
            && (self.external_ir_source_context.is_some() || context.is_some())
        {
            return Err(PyBindgenError::CodegenError(
                "a bounded external-IR source context describes one complete schema snapshot and cannot be combined with incremental IR registration"
                    .to_owned(),
            ));
        }
        self.record_source_provenance("external_ir:materialized_schema");
        if let Some(context) = &context {
            self.record_source_provenance(context.provenance());
        }
        let state =
            crate::external_ir::ExternalIrState::register(self.external_ir_state.take(), schema);
        let pending_modules = state.pending_module_count();
        let registrations = state.registration_count();
        self.external_ir_state = Some(state);
        if context.is_some() {
            self.external_ir_source_context = context;
        }
        tracing::debug!(
            source = "external_ir",
            pending_modules,
            registrations,
            "binding source registered"
        );
        Ok(self)
    }

    /// Add a single module from a pre-built external schema ([`crate::ModuleIr`]).
    pub fn module_from_ir(self, module_ir: crate::schema::ModuleIr) -> Result<Self> {
        self.modules_from_ir(SchemaIr {
            modules: vec![module_ir],
        })
    }

    /// Register an input adapter at the binding-source seam.
    ///
    /// The built-in adapters are [`crate::LiveModules`] and [`SchemaIr`]. This generic
    /// entry point also accepts downstream adapters that acquire a schema or live module
    /// through another transport before delegating back to the engine. Because an arbitrary
    /// adapter does not declare its mode until [`BindingSource::load`] returns, generic
    /// acquisition is accepted only while this `Codegen` has no registered source. Use the
    /// mode-specific registration methods for additional inputs of an established mode.
    pub fn source<S: BindingSource>(self, source: S) -> Result<Self> {
        if self.source_mode != SourceMode::Unset {
            return Err(PyBindgenError::CodegenError(
                "cannot mix or acquire an unclassified BindingSource after a Codegen source mode has been selected; use a fresh Codegen or a mode-specific registration method"
                    .to_owned(),
            ));
        }
        let loaded = {
            let span = tracing::info_span!(
                "pyo3_bindgen.source.acquire",
                adapter = std::any::type_name::<S>(),
            );
            let _entered = span.enter();
            let loaded = source.load();
            match &loaded {
                Ok(SchemaSource::Live(names)) => tracing::debug!(
                    source = "live",
                    modules = names.len(),
                    "binding source acquired"
                ),
                Ok(SchemaSource::Ir(schema)) => tracing::debug!(
                    source = "external_ir",
                    modules = schema.modules.len(),
                    "binding source acquired"
                ),
                Err(error) => {
                    tracing::error!(
                        observability_schema = 1,
                        operation_id = "unavailable",
                        failure_kind = "source_acquisition",
                        "binding source acquisition failed"
                    );
                    tracing::debug!(error = %error, "binding source acquisition failure detail");
                }
            }
            loaded
        }?;
        let mut codegen = self.schema_source(loaded)?;
        codegen.record_source_provenance(format!("binding_source:{}", std::any::type_name::<S>()));
        Ok(codegen)
    }

    /// Add modules from either a live source (module names) or a pre-built external
    /// schema, via the compatibility [`SchemaSource`] discriminator.
    pub fn schema_source(self, source: SchemaSource) -> Result<Self> {
        match source {
            SchemaSource::Live(names) => {
                let names = names.iter().map(String::as_str).collect::<Vec<_>>();
                self.module_names(names)
            }
            SchemaSource::Ir(schema) => self.modules_from_ir(schema),
        }
    }

    /// Generate the Rust FFI bindings for all modules added to the engine.
    pub fn generate(self) -> Result<proc_macro2::TokenStream> {
        // The report is dropped wholesale here (this is also the `import_python!` path),
        // so do not pay for the token-stream hash nobody will read.
        let policy = EmissionPolicy::new(false);
        Ok(self
            .generate_with_report_inner_tracking(policy, None)?
            .generated
            .tokens)
    }

    /// Generate the Rust FFI bindings and return a structured report describing the generated output.
    pub fn generate_with_report(self) -> Result<GeneratedBindings> {
        let policy = EmissionPolicy::new(true);
        Ok(self
            .generate_with_report_inner_tracking(policy, None)?
            .generated)
    }

    /// Generate one canonical artifact and its report without requiring callers to re-render the
    /// selected token candidate. The report's output list is empty until a caller delivers it.
    pub fn generate_artifact_with_report(self) -> Result<GeneratedArtifact> {
        self.generate_artifact_with_report_detailed()
            .map_err(|failure| failure.error)
    }

    /// Generate one canonical artifact while preserving report correlation on failure.
    pub fn generate_artifact_with_report_detailed(
        self,
    ) -> std::result::Result<GeneratedArtifact, GeneratedArtifactFailure> {
        let mut operation_id = None;
        let emitted = self
            .generate_with_report_inner_tracking(
                EmissionPolicy::new(false),
                Some(&mut operation_id),
            )
            .map_err(|error| GeneratedArtifactFailure::new(error, operation_id))?;
        let work = emitted.work();
        Ok(GeneratedArtifact {
            artifact: emitted.artifact,
            report: emitted.generated.report,
            work,
        })
    }

    fn generate_with_report_inner_tracking(
        self,
        policy: EmissionPolicy,
        operation_id: Option<&mut Option<String>>,
    ) -> Result<crate::emission::EmittedBindings> {
        let emitted = self.generate_with_report_unemitted(policy, operation_id)?;
        crate::report::emit_warnings(&emitted.generated.report);
        Ok(emitted)
    }

    /// Build and select the final result without exposing diagnostics from speculative
    /// candidates. The public generation entry points emit the selected warnings exactly
    /// once after this returns.
    fn generate_with_report_unemitted(
        mut self,
        policy: EmissionPolicy,
        operation_id_out: Option<&mut Option<String>>,
    ) -> Result<crate::emission::EmittedBindings> {
        let budget = BudgetTracker::new(self.budget);
        let source = self.source_mode.as_str();
        let pending_external_modules = self
            .external_ir_state
            .as_ref()
            .map_or(0, crate::external_ir::ExternalIrState::pending_module_count);
        let input_modules = if self.source_mode == SourceMode::ExternalIr {
            pending_external_modules
        } else {
            self.modules.len()
        };
        let span = tracing::info_span!(
            "pyo3_bindgen.generate",
            observability_schema = 1,
            operation_id = tracing::field::Empty,
            source,
            input_modules,
            pending_external_modules,
            generate_dependencies = self.cfg.generate_dependencies,
            deduplicate_inherited_members = self.cfg.deduplicate_inherited_members,
        );
        let _entered = span.enter();
        tracing::info!(
            observability_schema = 1,
            operation_id = "unavailable",
            "generation started"
        );

        let mut validation_lowering_passes = 0;
        if self.source_mode == SourceMode::ExternalIr {
            if let Some(state) = self.external_ir_state.take() {
                let pending_modules = state.pending_module_count();
                let registrations = state.registration_count();
                let phase = tracing::debug_span!(
                    "pyo3_bindgen.generate.phase",
                    phase = "external_ir_validation_lowering",
                    pending_modules,
                    registrations,
                );
                let _entered = phase.enter();
                budget.check_structure(ResourcePhase::Validation, state.resources()?)?;
                budget.checkpoint_wall(ResourcePhase::Validation)?;
                budget.checkpoint_peak_rss_delta(ResourcePhase::Validation)?;
                let finish = state.finish(&self.cfg)?;
                validation_lowering_passes += finish.validation_lowering_passes;
                tracing::debug!(
                    pending_modules,
                    modules = finish.modules.len(),
                    registrations = finish.registration_count,
                    validation_lowering_passes = finish.validation_lowering_passes,
                    "external IR graph validated and lowered"
                );
                self.modules = finish.modules;
                budget.checkpoint_wall(ResourcePhase::Lowering)?;
                budget.checkpoint_peak_rss_delta(ResourcePhase::Lowering)?;
            }
        }

        if self.modules.is_empty() {
            let error = PyBindgenError::CodegenError(
                "There are no modules for which to generate bindings".to_string(),
            );
            tracing::error!(
                observability_schema = 1,
                operation_id = "unavailable",
                failure_kind = "empty_input",
                "generation failed"
            );
            tracing::debug!(error = %error, "generation failure detail");
            return Err(error);
        }

        // Tag directly registered source modules before dotted paths are wrapped under synthetic
        // parents during normalization.
        self.modules.iter_mut().for_each(|module| {
            if let Some(source_code) = self.embedded_source_code.get(&module.name.to_py()) {
                module.source_code = Some(source_code.clone());
            }
        });

        let source_kind = match self.source_mode {
            SourceMode::Live => SourceKind::Live,
            SourceMode::ExternalIr => SourceKind::ExternalIr,
            SourceMode::Unset => {
                return Err(PyBindgenError::CodegenError(
                    "cannot normalize a Codegen without a selected source mode".to_owned(),
                ));
            }
        };
        let config = self.cfg.clone();
        let acquisition = match source_kind {
            SourceKind::Live => AcquisitionFacts::live(self.source_provenance.clone()),
            SourceKind::ExternalIr => AcquisitionFacts::external_ir(
                self.source_provenance.clone(),
                self.external_ir_source_context.as_ref().map(|context| {
                    (
                        context.sha256(),
                        context.content_bytes(),
                        context.byte_limit(),
                    )
                }),
            ),
        };
        let mut modules = std::mem::take(&mut self.modules);
        let (live_parents, live_import_prefixes, python_metadata) = match source_kind {
            SourceKind::ExternalIr => (Vec::new(), None, None),
            SourceKind::Live => {
                let live_policy = crate::live_operation::LivePolicy::from_config(&config);
                let (context, metadata) =
                    crate::live_operation::with_generation_session(live_policy, |session| {
                        if config.generate_dependencies {
                            let dependency_span = tracing::debug_span!(
                                "pyo3_bindgen.generate.phase",
                                phase = "dependency_discovery",
                            );
                            let _phase = dependency_span.enter();
                            let acquired = session.discover_dependencies(&config, &modules)?;
                            modules.extend(acquired.modules);
                            self.dependency_diagnostics.extend(acquired.diagnostics);
                        }
                        session.generation_context(&config, &modules)
                    })?;
                let (parents, prefixes) = context.into_parts();
                (parents, Some(prefixes), metadata)
            }
        };
        budget.checkpoint_wall(ResourcePhase::Registration)?;
        budget.checkpoint_peak_rss_delta(ResourcePhase::Registration)?;
        let normalized = {
            let phase =
                tracing::debug_span!("pyo3_bindgen.generate.phase", phase = "normalization",);
            let _entered = phase.enter();
            let input = NormalizationInput {
                acquisition,
                config: config.clone(),
                modules,
                dependency_diagnostics: std::mem::take(&mut self.dependency_diagnostics),
                live_parents,
            };
            normalize(input, &budget)?
        };
        let operation_id =
            crate::operation::OperationId::for_graph(normalized.primary().graph(), &config)?;
        if let Some(operation_id_out) = operation_id_out {
            *operation_id_out = Some(operation_id.as_str().to_owned());
        }
        span.record("operation_id", operation_id.as_str());
        let mut resources = normalized.primary().graph().resources()?;
        if let Some(flat) = normalized.flat() {
            let flat = flat.graph().resources()?;
            resources = StructuralResources {
                definitions: resources.definitions.max(flat.definitions),
                recursion_depth: resources.recursion_depth.max(flat.recursion_depth),
                model_nodes: resources.model_nodes.max(flat.model_nodes),
            };
        }
        budget.check_structure(ResourcePhase::Normalization, resources)?;
        budget.checkpoint_wall(ResourcePhase::Normalization)?;
        budget.checkpoint_peak_rss_delta(ResourcePhase::Normalization)?;
        let primary = normalized.primary();
        let graph_complete = primary.graph().completeness().is_complete();
        let policy = policy
            .with_python_metadata(python_metadata)
            .with_operation_id(operation_id.as_str());
        let result =
            crate::emission::emit_candidates(&normalized, live_import_prefixes, &policy, &budget)
                .and_then(|mut emitted| {
                    budget.checkpoint_wall(ResourcePhase::Emission)?;
                    budget.checkpoint_peak_rss_delta(ResourcePhase::Emission)?;
                    emitted.validation_lowering_passes = validation_lowering_passes;
                    emitted.normalization_passes = 1;
                    Ok(emitted)
                });

        let result = if config.strict_completeness && !source_kind.is_external_ir() {
            match result {
                Ok(emitted) => {
                    let submodule_counts = crate::emission::discovery_counts(
                        &emitted.generated.report.diagnostics,
                        crate::report::DiscoverySubject::Submodule,
                    );
                    let dependency_counts = crate::emission::discovery_counts(
                        &emitted.generated.report.diagnostics,
                        crate::report::DiscoverySubject::Dependency,
                    );
                    if !graph_complete || submodule_counts[2] > 0 || dependency_counts[2] > 0 {
                        tracing::error!(
                            observability_schema = 1,
                            operation_id = operation_id.as_str(),
                            failure_kind = "strict_completeness",
                            complete = graph_complete,
                            submodule_attempted = submodule_counts[0],
                            submodule_emitted = submodule_counts[1],
                            submodule_skipped = submodule_counts[2],
                            submodule_failed = submodule_counts[3],
                            submodule_truncated = submodule_counts[4],
                            dependency_attempted = dependency_counts[0],
                            dependency_emitted = dependency_counts[1],
                            dependency_skipped = dependency_counts[2],
                            dependency_failed = dependency_counts[3],
                            dependency_truncated = dependency_counts[4],
                            "strict completeness failed"
                        );
                        Err(PyBindgenError::CodegenError(format!(
                            "strict completeness failed: submodule={};dependency={}",
                            crate::emission::discovery_summary_message(submodule_counts),
                            crate::emission::discovery_summary_message(dependency_counts),
                        )))
                    } else {
                        Ok(emitted)
                    }
                }
                Err(error) => Err(error),
            }
        } else {
            result
        };

        match &result {
            Ok(emitted) => {
                let submodules = crate::emission::discovery_counts(
                    &emitted.generated.report.diagnostics,
                    crate::report::DiscoverySubject::Submodule,
                );
                let dependencies = crate::emission::discovery_counts(
                    &emitted.generated.report.diagnostics,
                    crate::report::DiscoverySubject::Dependency,
                );
                tracing::info!(
                    observability_schema = 1,
                    operation_id = operation_id.as_str(),
                    selected_candidate = emitted.selected_model,
                    canonical_renders = emitted.canonical_renders,
                    candidate_clones = emitted.module_tree_clones,
                    validation_lowering_passes = emitted.validation_lowering_passes,
                    normalization_passes = emitted.normalization_passes,
                    modules = emitted.generated.report.modules.len(),
                    diagnostics = emitted.generated.report.diagnostics.len(),
                    warnings = emitted.generated.report.warning_count(),
                    errors = emitted.generated.report.error_count(),
                    complete = graph_complete,
                    strict_requested = config.strict_completeness,
                    submodule_attempted = submodules[0],
                    submodule_emitted = submodules[1],
                    submodule_skipped = submodules[2],
                    submodule_failed = submodules[3],
                    submodule_truncated = submodules[4],
                    dependency_attempted = dependencies[0],
                    dependency_emitted = dependencies[1],
                    dependency_skipped = dependencies[2],
                    dependency_failed = dependencies[3],
                    dependency_truncated = dependencies[4],
                    "generation completed"
                );
            }
            Err(error) => {
                let failure_kind = match error {
                    PyBindgenError::ResourceLimitExceeded(_) => "resource_limit_exceeded",
                    PyBindgenError::ResourceMeasurementUnavailable(_) => {
                        "resource_measurement_unavailable"
                    }
                    _ => "generation_failed",
                };
                tracing::error!(
                    observability_schema = 1,
                    operation_id = operation_id.as_str(),
                    failure_kind,
                    "generation failed"
                );
                tracing::debug!(error = %error, "generation failure detail");
            }
        }
        result
    }

    /// Generate the Rust FFI bindings and atomically write their canonical formatting.
    pub fn build(self, output_path: impl AsRef<std::path::Path>) -> Result<()> {
        self.build_with_report(output_path)?;
        Ok(())
    }

    /// Generate the Rust FFI bindings, atomically write their canonical formatting,
    /// and report the exact bytes written to the caller-provided path.
    pub fn build_with_report(
        self,
        output_path: impl AsRef<std::path::Path>,
    ) -> Result<GenerationReport> {
        let output_path = output_path.as_ref();
        let mut publication = crate::PublicationPlan::snapshot(
            crate::PublicationPolicy::codegen(),
            None,
            Some(output_path),
            None,
        )
        .map_err(PyBindgenError::CodegenError)?
        .acquire()
        .map_err(PyBindgenError::CodegenError)?;
        // `report.outputs` is replaced with the written-file entry below, so the
        // token-stream hash would be computed only to be thrown away.
        let policy = EmissionPolicy::new(false);
        let emitted = self.generate_with_report_inner_tracking(policy, None)?;
        publication.set_operation_id(emitted.generated.report.operation_id.as_deref());
        let mut report = emitted.generated.report;
        let publication_attempt = publication.publish_output(&emitted.artifact);
        if let Some(failure) = publication_attempt.failure {
            return Err(PyBindgenError::CodegenError(failure.message));
        }
        report.outputs = vec![OutputMetadata::from_path_bytes(
            "file",
            Some(output_path),
            emitted.artifact.bytes(),
        )];
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::panic::{catch_unwind, AssertUnwindSafe};

    #[test]
    fn execution_accounting_counts_terminal_work_exactly() {
        let external = Codegen::default()
            .modules_from_ir(crate::SchemaIr {
                modules: vec![crate::ModuleIr {
                    name: "accounting_external".to_owned(),
                    docstring: None,
                    is_package: false,
                    classes: vec![],
                    enums: vec![],
                    functions: vec![],
                    properties: vec![],
                    submodules: vec![],
                    prelude: vec![],
                    imports: vec![],
                    type_vars: vec![],
                }],
            })
            .unwrap()
            .generate_with_report_inner_tracking(EmissionPolicy::new(false), None)
            .unwrap();
        assert_eq!(external.canonical_renders, 1);
        assert_eq!(external.module_tree_clones, 0);
        assert_eq!(external.validation_lowering_passes, 1);
        assert_eq!(external.normalization_passes, 1);

        let methods = (0..20)
            .map(|index| {
                format!(
                    "    def method_{index}(self, value: int, text: str) -> int:\n\
                     \x20       \"\"\"A deliberately substantial inherited method.\"\"\"\n\
                     \x20       return value\n"
                )
            })
            .collect::<String>();
        let source = format!("class Base:\n{methods}\nclass Derived(Base):\n    pass\n");
        let adaptive = Codegen::default()
            .module_from_str(&source, "accounting_live")
            .unwrap()
            .generate_with_report_inner_tracking(EmissionPolicy::new(false), None)
            .unwrap();
        assert_eq!(adaptive.canonical_renders, 2);
        assert_eq!(adaptive.module_tree_clones, 1);
        assert_eq!(adaptive.validation_lowering_passes, 0);
        assert_eq!(adaptive.normalization_passes, 1);
    }

    /// `ROOT_SUPPORT_COST` is a hand-measured byte count of the root-support block that the
    /// adaptive dedup decision charges globally. The block itself is ordinary generated code
    /// and has silently drifted away from the constant twice. Re-measure the real emission
    /// here and fail when the two diverge by more than a tolerance -- the model only needs the
    /// right order of magnitude, but a 4322-byte constant guarding a block that has grown to
    /// (say) 6 KB flips the "strict size win" decision for marginal generations.
    #[test]
    fn root_support_cost_matches_the_emitted_root_block() {
        let tokens = crate::syntax::class::generate_root_trait();
        let file: syn::File =
            syn::parse2(tokens).expect("the root-support block must parse as a Rust file");
        let measured = prettyplease::unparse(&file).len() as i64;

        // +-10% of the recorded constant.
        let tolerance = crate::normalized::ROOT_SUPPORT_COST / 10;
        assert!(
            (measured - crate::normalized::ROOT_SUPPORT_COST).abs() <= tolerance,
            "the emitted root-support block is {measured} bytes but ROOT_SUPPORT_COST is \
             {} (tolerance +-{tolerance}). The block changed: re-measure the formatted emission \
             of `syntax::class::generate_root_trait` and update ROOT_SUPPORT_COST in normalized.rs.",
            crate::normalized::ROOT_SUPPORT_COST,
        );
    }

    #[test]
    fn adaptive_generation_emits_selected_warning_once() {
        const CHILD_ENV: &str = "PYO3_BINDGEN_WARNING_EMISSION_CHILD";
        const CHILD_ROLE: &str = "pyo3-bindgen-warning-emission-child-v1";
        const TEST_NAME: &str = "codegen::tests::adaptive_generation_emits_selected_warning_once";

        if std::env::var(CHILD_ENV).as_deref() == Ok(CHILD_ROLE) {
            let subscriber = tracing_subscriber::fmt()
                .with_max_level(tracing::Level::WARN)
                .without_time()
                .with_target(false)
                // Keep parent-side substring assertions hermetic against ambient color variables.
                .with_ansi(false)
                .with_writer(std::io::stderr)
                .finish();
            let _subscriber = tracing::subscriber::set_default(subscriber);
            let methods = (0..20)
                .map(|index| {
                    format!(
                        "    def method_{index}(self, value: int, text: str) -> int:\n\
                         \x20       \"\"\"A deliberately substantial inherited method.\"\"\"\n\
                         \x20       ...\n"
                    )
                })
                .collect::<String>();
            let source = format!(
                "def pyo3_embed_python_source_code():\n\
                 \x20   pass\n\n\
                 class Base:\n\
                 {methods}\
                 class Derived(Base):\n\
                 \x20   pass\n"
            );
            let generated = Codegen::default()
                .module_from_str(&source, "mod_selected_warning")
                .unwrap()
                .generate_with_report()
                .unwrap();
            assert_eq!(
                generated
                    .report
                    .diagnostics
                    .iter()
                    .filter(|diagnostic| diagnostic.code == "function_generated_name_renamed")
                    .count(),
                1,
                "the selected report itself must contain one warning",
            );
            return;
        }

        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", TEST_NAME, "--nocapture"])
            .env(CHILD_ENV, CHILD_ROLE)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "warning-emission child failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(
            stderr
                .matches("conflicts with generated module support")
                .count(),
            1,
            "only the selected candidate may emit its warning:\n{stderr}",
        );
    }

    #[test]
    fn empty_external_import_origin_is_an_error_instead_of_a_panic() {
        pyo3::Python::initialize();
        let mut module = Module::empty_local(Path::from_py("empty_import_origin"));
        module.imports.push(crate::syntax::Import::new(
            Path::default(),
            Path::from_py("external.Type"),
        ));
        let cfg = Config {
            generate_dependencies: true,
            ..Config::default()
        };
        let codegen = Codegen {
            cfg,
            modules: vec![module],
            source_mode: SourceMode::Live,
            ..Codegen::default()
        };

        let result = catch_unwind(AssertUnwindSafe(|| codegen.generate()));
        let error = result
            .expect("empty import origin must not panic")
            .expect_err("empty import origin must be rejected");
        assert!(
            error.to_string().contains("empty external import origin"),
            "unexpected error: {error}",
        );
    }

    #[test]
    fn candidate_live_tree_rejects_internal_duplicate_paths() {
        let duplicate = Module::empty_local(Path::from_py("candidate.shared"));
        let mut candidate = Module::empty_local(Path::from_py("candidate"));
        candidate.submodules = vec![duplicate.clone(), duplicate];

        let error = Codegen::default()
            .ensure_live_module_tree_available(&candidate)
            .expect_err("candidate tree must reject duplicate paths");
        assert!(
            error
                .to_string()
                .contains("duplicate live module path 'candidate.shared'"),
            "unexpected error: {error}",
        );
    }

    #[test]
    fn single_candidate_generation_emits_selection_summary() {
        const CHILD_ENV: &str = "PYO3_BINDGEN_SELECTION_SUMMARY_CHILD";
        const CHILD_ROLE: &str = "pyo3-bindgen-selection-summary-child-v1";
        const TEST_NAME: &str =
            "codegen::tests::single_candidate_generation_emits_selection_summary";

        if std::env::var(CHILD_ENV).as_deref() == Ok(CHILD_ROLE) {
            let subscriber = tracing_subscriber::fmt()
                .with_max_level(tracing::Level::DEBUG)
                .without_time()
                .with_target(false)
                // The parent asserts plain-text `key=value` substrings; ANSI styling would split
                // them, and the default depends on ambient `NO_COLOR`/`FORCE_COLOR`.
                .with_ansi(false)
                .with_writer(std::io::stderr)
                .finish();
            let _subscriber = tracing::subscriber::set_default(subscriber);
            Codegen::default()
                .modules_from_ir(crate::SchemaIr {
                    modules: vec![crate::ModuleIr {
                        name: "selection_summary".to_owned(),
                        docstring: None,
                        is_package: false,
                        classes: vec![],
                        enums: vec![],
                        functions: vec![],
                        properties: vec![],
                        submodules: vec![],
                        prelude: vec![],
                        imports: vec![],
                        type_vars: vec![],
                    }],
                })
                .unwrap()
                .generate_artifact_with_report()
                .unwrap();
            return;
        }

        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", TEST_NAME, "--nocapture"])
            .env(CHILD_ENV, CHILD_ROLE)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "selection-summary child failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        let summaries = stderr
            .lines()
            .filter(|line| line.contains("generation candidate selected"))
            .collect::<Vec<_>>();
        assert_eq!(
            summaries.len(),
            1,
            "selection child must emit exactly one summary: {stderr}"
        );
        let summary = summaries[0];
        for field in [
            "observability_schema=1",
            "operation_id=\"op-v1:fnv1a64:",
            "selected_candidate=\"single\"",
            "candidate_bytes=",
        ] {
            assert!(summary.contains(field), "missing {field:?}: {summary}");
        }
    }
}
