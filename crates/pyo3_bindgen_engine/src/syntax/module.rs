use super::naming::{rust_ident, NameAllocator, SymbolKind, SymbolTable};
use super::{
    AttributeVariant, Class, Function, FunctionImplementation, FunctionType, Ident, Import,
    ImportMode, Path, Property, PropertyOwner, PyEnum, PyEnumParse, TypeVar,
};
use crate::{
    report::{DomainDiagnostic, GeneratedCounts, ModuleReport},
    Config, Result,
};
use itertools::Itertools;
use pyo3::prelude::*;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};
use serde::Serialize;

pub(crate) use crate::planning::{LocalType, LocalTypes};

const IMPORTED_MODULE_FINGERPRINT_KIND: &str = "pyo3_bindgen.imported_module";
// v3: direct class bases, exact Python/Rust identifier spellings, and getter presence
// are part of the imported shape. Each can change emitted bindings while `dir()` still
// exposes the same member set.
const IMPORTED_MODULE_FINGERPRINT_VERSION: u32 = 3;

#[derive(Serialize)]
struct ImportedModuleFingerprint<'a> {
    kind: &'static str,
    version: u32,
    module: &'a Module,
}

#[derive(Default)]
pub(crate) struct GenerationStats {
    counts: HashMap<Path, GeneratedCounts>,
    emitted_modules: HashSet<Path>,
    emitted_enums: HashSet<Path>,
    /// `Module::input_hash()` results memoized by module name, recorded during
    /// `generate()` and reused by `collect_report_entries()`. The live path's
    /// imported-module fingerprint serializes the module's whole subtree, and both
    /// call sites would otherwise compute the identical value twice per module.
    /// Keyed by name, like `counts`/`emitted_modules`: names are unique after
    /// normalized canonicalization (see `get_duplicate_modules`).
    input_hashes: HashMap<Path, String>,
}

pub(crate) struct GenerationState<'a> {
    diagnostics: &'a mut Vec<DomainDiagnostic>,
    stats: &'a mut GenerationStats,
    inherited_members: &'a HashMap<Path, super::class::InheritedMemberPlan>,
}

struct EmittedImport<'a> {
    import: &'a Import,
    resolved_python_origin: Path,
    emitted_origin: Path,
    alias: syn::Ident,
    kind: SymbolKind,
}

enum IrAnnotationOrigin {
    AuthoredType,
    CanonicalStandard(String),
    Other,
}

#[derive(Default)]
struct PreludeExtraction<'py> {
    names: Vec<Ident>,
    resolved_attrs: HashMap<Ident, pyo3::Bound<'py, pyo3::types::PyAny>>,
    failed_names: HashSet<Ident>,
}

impl<'a> GenerationState<'a> {
    pub(crate) fn new(
        diagnostics: &'a mut Vec<DomainDiagnostic>,
        stats: &'a mut GenerationStats,
        inherited_members: &'a HashMap<Path, super::class::InheritedMemberPlan>,
    ) -> Self {
        Self {
            diagnostics,
            stats,
            inherited_members,
        }
    }
}

impl GenerationStats {
    pub(crate) fn with_input_hashes(input_hashes: HashMap<Path, String>) -> Self {
        Self {
            input_hashes,
            ..Self::default()
        }
    }
}

/// The valid Rust identifier a class would be generated under (its `s_`-sanitized
/// newtype name), or [`None`] if it would be skipped. (#18)
fn class_rust_ident(class: &Class) -> Option<syn::Ident> {
    rust_ident(class.name.name(), SymbolKind::Class)
}

/// The valid Rust identifier an enum would be generated under (its `e_`-sanitized name),
/// or [`None`] if it would be skipped. (#18)
fn enum_rust_ident(py_enum: &PyEnum) -> Option<syn::Ident> {
    rust_ident(py_enum.name.name(), SymbolKind::Enum)
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub(crate) struct Module {
    pub(crate) name: Path,
    pub(crate) prelude: Vec<Ident>,
    pub(crate) imports: Vec<Import>,
    pub(crate) submodules: Vec<Module>,
    pub(crate) enums: Vec<PyEnum>,
    pub(crate) classes: Vec<Class>,
    pub(crate) type_vars: Vec<TypeVar>,
    pub(crate) functions: Vec<Function>,
    pub(crate) properties: Vec<Property>,
    pub(crate) docstring: Option<String>,
    pub(crate) is_package: bool,
    #[serde(skip)]
    pub(crate) source_code: Option<String>,
    #[serde(skip)]
    pub(crate) diagnostics: Vec<DomainDiagnostic>,
    /// Content-derived hash of the source IR (external-IR path only).
    ///
    /// `Some` only when this module was built via [`Self::from_ir_in_schema`]. Live
    /// imported modules instead derive their drift hash from the complete inspected
    /// structure.
    #[serde(skip)]
    pub(crate) ir_input_hash: Option<String>,
    /// Whether canonicalization synthesized this node solely to own dotted IR modules.
    /// Authored nodes take precedence when same-named siblings are merged.
    #[serde(skip)]
    pub(crate) is_synthetic_external_ir_parent: bool,
}

fn register_qualified_annotation_paths(
    modules: &[crate::schema::ModuleIr],
    index: &IrSchemaIndex<'_>,
    annotation_types: &mut rustc_hash::FxHashMap<String, String>,
) {
    for module in modules {
        for path in module
            .classes
            .iter()
            .map(|class| class.name.as_str())
            .chain(module.enums.iter().map(|py_enum| py_enum.name.as_str()))
            .chain(module.type_vars.iter().map(String::as_str))
            .filter(|path| Module::is_standard_ir_annotation_path(path))
        {
            annotation_types.insert(path.to_owned(), Function::forced_custom_ir_annotation(path));
        }
        for import in &module.imports {
            match Module::resolve_ir_annotation_origin(index, &import.target) {
                IrAnnotationOrigin::CanonicalStandard(origin) => {
                    if import.target != origin {
                        annotation_types.insert(import.target.clone(), origin.clone());
                    }
                    if Module::is_standard_ir_annotation_root(&origin) {
                        annotation_types
                            .insert(format!("{}.", import.target), format!("{origin}."));
                    }
                }
                IrAnnotationOrigin::AuthoredType
                    if Module::is_standard_ir_annotation_path(&import.target) =>
                {
                    annotation_types.insert(
                        import.target.clone(),
                        Function::forced_custom_ir_annotation(&import.target),
                    );
                }
                IrAnnotationOrigin::AuthoredType | IrAnnotationOrigin::Other => {}
            }
        }
        register_qualified_annotation_paths(&module.submodules, index, annotation_types);
    }
}

// A standard module can itself be imported under a generated alias (for
// example `typing -> pkg.t`). The exact module mapping handles ordinary
// descendants such as `t.Type`, while these more-specific entries ensure an
// authored child under a standard-looking root still wins longest-prefix
// resolution (`typing.Type` authored by the schema is not the stdlib alias).
fn collect_authored_type_paths<'a>(
    modules: &'a [crate::schema::ModuleIr],
    paths: &mut Vec<&'a str>,
) {
    for module in modules {
        paths.extend(module.classes.iter().map(|class| class.name.as_str()));
        paths.extend(module.enums.iter().map(|py_enum| py_enum.name.as_str()));
        paths.extend(module.type_vars.iter().map(String::as_str));
        collect_authored_type_paths(&module.submodules, paths);
    }
}

fn register_authored_standard_module_aliases(
    modules: &[crate::schema::ModuleIr],
    index: &IrSchemaIndex<'_>,
    authored_paths: &[&str],
    annotation_types: &mut rustc_hash::FxHashMap<String, String>,
) {
    for module in modules {
        for import in &module.imports {
            let IrAnnotationOrigin::CanonicalStandard(origin) =
                Module::resolve_ir_annotation_origin(index, &import.target)
            else {
                continue;
            };
            for authored_path in authored_paths {
                let Some(suffix) = authored_path.strip_prefix(&origin) else {
                    continue;
                };
                if !suffix.starts_with('.') {
                    continue;
                }
                let alias_path = format!("{}{suffix}", import.target);
                if matches!(
                    Module::resolve_ir_annotation_origin(index, &alias_path),
                    IrAnnotationOrigin::AuthoredType
                ) {
                    annotation_types.insert(
                        alias_path.clone(),
                        Function::forced_custom_ir_annotation(&alias_path),
                    );
                }
            }
        }
        register_authored_standard_module_aliases(
            &module.submodules,
            index,
            authored_paths,
            annotation_types,
        );
    }
}

/// Whole-schema lookup tables for external-IR annotation origin resolution.
///
/// `Module::resolve_ir_annotation_origin` walks import chains, and each step used to scan
/// every module twice: once for a directly declared type and once to collect the imports
/// whose target covers the queried path. Both scans are answered here in constant time per
/// dotted prefix, which is what makes origin resolution independent of schema size.
///
/// The tables are exact rather than approximate. `declares_direct_type` holds precisely the
/// class, enum, and type-variable names the old recursive scan compared against.
/// `imports_covering` returns the same imports the old collector did, in schema traversal
/// order: an import matches when its target equals the path or is a dotted prefix of it, so
/// enumerating the path's dotted prefixes finds exactly those. Two distinct targets of equal
/// length cannot both prefix one path, so the caller's longest-target filter still selects a
/// single target's imports, in their original relative order.
#[derive(Debug, Default)]
pub(crate) struct IrSchemaIndex<'a> {
    direct_types: rustc_hash::FxHashSet<&'a str>,
    imports_by_target: rustc_hash::FxHashMap<&'a str, Vec<&'a crate::schema::ImportIr>>,
}

impl<'a> IrSchemaIndex<'a> {
    pub(crate) fn new(all_modules: &'a [crate::schema::ModuleIr]) -> Self {
        let mut index = Self::default();
        index.register(all_modules);
        index
    }

    fn register(&mut self, modules: &'a [crate::schema::ModuleIr]) {
        for module in modules {
            self.direct_types
                .extend(module.classes.iter().map(|class| class.name.as_str()));
            self.direct_types
                .extend(module.enums.iter().map(|py_enum| py_enum.name.as_str()));
            self.direct_types
                .extend(module.type_vars.iter().map(String::as_str));
            for import in &module.imports {
                if import.origin != import.target {
                    self.imports_by_target
                        .entry(import.target.as_str())
                        .or_default()
                        .push(import);
                }
            }
            self.register(&module.submodules);
        }
    }

    fn declares_direct_type(&self, path: &str) -> bool {
        self.direct_types.contains(path)
    }

    /// Imports whose target equals `path` or is a dotted prefix of it, schema order first
    /// by target length descending is *not* applied here: the caller owns that choice.
    fn imports_covering(&self, path: &str) -> Vec<&'a crate::schema::ImportIr> {
        let mut covering = Vec::new();
        if let Some(imports) = self.imports_by_target.get(path) {
            covering.extend(imports.iter().copied());
        }
        let mut boundary = path.len();
        while let Some(dot) = path[..boundary].rfind('.') {
            if let Some(imports) = self.imports_by_target.get(&path[..dot]) {
                covering.extend(imports.iter().copied());
            }
            boundary = dot;
        }
        covering
    }
}

/// Schema-wide external-IR annotation mappings, prepared once per schema.
///
/// `Module::ir_annotation_types` mixes module-local mappings with two passes that depend
/// only on the whole schema: qualified annotation paths and authored aliases of
/// standard-looking modules. Those passes walk every module, and the function runs once
/// per module in lowering and again in each validation scan, so recomputing them made
/// ingestion quadratic in module count (measured 2026-09-03: 4.5 ms at 8 modules against
/// 871 ms at 64). They are computed here instead and merged per module.
#[derive(Debug, Default)]
pub(crate) struct IrSchemaAnnotations<'a> {
    qualified: rustc_hash::FxHashMap<String, String>,
    index: IrSchemaIndex<'a>,
}

impl<'a> IrSchemaAnnotations<'a> {
    pub(crate) fn new(all_modules: &'a [crate::schema::ModuleIr]) -> Self {
        let index = IrSchemaIndex::new(all_modules);
        let mut qualified = rustc_hash::FxHashMap::default();
        register_qualified_annotation_paths(all_modules, &index, &mut qualified);
        let mut authored_paths = Vec::new();
        collect_authored_type_paths(all_modules, &mut authored_paths);
        register_authored_standard_module_aliases(
            all_modules,
            &index,
            &authored_paths,
            &mut qualified,
        );
        Self { qualified, index }
    }

    pub(crate) fn index(&self) -> &IrSchemaIndex<'a> {
        &self.index
    }
}

/// One module's external-IR annotation rewrite table, prepared once for that module.
///
/// [`super::Function::parse_ir_annotation`] rewrites annotation spellings by
/// longest-match-first prefix replacement. It used to rebuild and sort that candidate
/// list on every parameter and return annotation, which made lowering quadratic in the
/// module's definition count (measured 2026-09-03: 6.9 ms at 80 classes against 511 ms
/// at 640, for the same per-class shape). The list is therefore prepared here instead.
///
/// Candidates are bucketed by their first byte. A candidate can only apply where the
/// annotation's next byte equals the candidate's own first byte, so bucketing removes
/// non-matching comparisons without changing which candidate wins.
#[derive(Debug, Default)]
pub(crate) struct IrAnnotationTypes {
    by_path: rustc_hash::FxHashMap<String, String>,
    candidates_by_first_byte: rustc_hash::FxHashMap<u8, Vec<(String, String)>>,
}

impl IrAnnotationTypes {
    pub(crate) fn new(by_path: rustc_hash::FxHashMap<String, String>) -> Self {
        let mut candidates_by_first_byte: rustc_hash::FxHashMap<u8, Vec<(String, String)>> =
            rustc_hash::FxHashMap::default();
        for (path, replacement) in &by_path {
            if path == replacement {
                continue;
            }
            // An empty candidate carries no first byte. It could only ever "match" without
            // consuming input, which would not advance the scan, so it is dropped here.
            let Some(&first_byte) = path.as_bytes().first() else {
                continue;
            };
            candidates_by_first_byte
                .entry(first_byte)
                .or_default()
                .push((path.clone(), replacement.clone()));
        }
        for candidates in candidates_by_first_byte.values_mut() {
            // Longest first, exactly as the per-annotation sort ordered it. Ties are
            // broken by spelling so preparation is deterministic; two distinct candidates
            // of equal length can never both match at one position, so tie order cannot
            // change the rewrite.
            candidates.sort_unstable_by(|(left, _), (right, _)| {
                right.len().cmp(&left.len()).then_with(|| left.cmp(right))
            });
        }
        Self {
            by_path,
            candidates_by_first_byte,
        }
    }

    /// Exact-path lookup for a whole annotation token.
    pub(crate) fn get(&self, path: &str) -> Option<&String> {
        self.by_path.get(path)
    }

    /// Rewrite candidates that can start at a byte equal to `first_byte`, longest first.
    pub(crate) fn candidates_starting_with(&self, first_byte: u8) -> &[(String, String)] {
        self.candidates_by_first_byte
            .get(&first_byte)
            .map_or(&[], Vec::as_slice)
    }
}

impl Module {
    /// Structural resources directly owned by this module (excluding submodules).
    ///
    /// Imports and preludes are references rather than definitions. Enum variants and aliases,
    /// class members, module members, type variables, and every module node are definitions.
    pub(crate) fn direct_resource_shape(&self) -> crate::resource::StructuralResources {
        let mut resources = crate::resource::StructuralResources {
            definitions: 1 + self.type_vars.len(),
            recursion_depth: 1,
            model_nodes: 1
                + self.prelude.len()
                + self.imports.len()
                + self.type_vars.len()
                + self.diagnostics.len(),
        };
        for shape in self
            .enums
            .iter()
            .map(PyEnum::resource_shape)
            .chain(self.classes.iter().map(Class::resource_shape))
            .chain(self.functions.iter().map(Function::resource_shape))
            .chain(self.properties.iter().map(Property::resource_shape))
        {
            resources.definitions += shape.definitions;
            resources.model_nodes += shape.model_nodes;
            resources.recursion_depth = resources.recursion_depth.max(shape.recursion_depth);
        }
        resources
    }

    pub(crate) fn empty(py: pyo3::Python, name: Path) -> Result<Self> {
        let module = py.import(name.to_py().as_str())?;

        // Extract the docstring of the module
        let docstring = {
            let docstring = module.getattr(pyo3::intern!(py, "__doc__"))?.to_string();
            if docstring.is_empty() || docstring == "None" {
                None
            } else {
                Some(docstring)
            }
        };

        Ok(Self {
            name,
            prelude: Vec::default(),
            imports: Vec::default(),
            submodules: Vec::default(),
            enums: Vec::default(),
            classes: Vec::default(),
            type_vars: Vec::default(),
            functions: Vec::default(),
            properties: Vec::default(),
            docstring,
            is_package: true,
            source_code: None,
            diagnostics: Vec::default(),
            ir_input_hash: None,
            is_synthetic_external_ir_parent: false,
        })
    }

    /// `pyo3`-free empty-parent constructor used while canonicalizing the IR tree.
    ///
    /// Unlike [`Self::empty`], this never imports anything: it produces a bare
    /// package node with no docstring. Its deterministic synthetic hash marks the
    /// node as external-IR-derived in reports and generated provenance metadata.
    pub(crate) fn empty_local(name: Path) -> Self {
        let synthetic_identity = format!(
            "pyo3_bindgen.synthetic_external_ir_parent.v1\0{}",
            name.to_py()
        );
        Self {
            name,
            prelude: Vec::default(),
            imports: Vec::default(),
            submodules: Vec::default(),
            enums: Vec::default(),
            classes: Vec::default(),
            type_vars: Vec::default(),
            functions: Vec::default(),
            properties: Vec::default(),
            docstring: None,
            is_package: true,
            source_code: None,
            diagnostics: Vec::default(),
            ir_input_hash: Some(crate::report::stable_hash(synthetic_identity.as_bytes())),
            is_synthetic_external_ir_parent: true,
        }
    }

    /// Resolve an annotation import origin through the complete external schema.
    ///
    /// Authored type detection intentionally ignores generation toggles. A declaration
    /// under a standard-looking root such as `typing.Type` must never acquire the
    /// standard alias's parser semantics merely because it is disabled. Conversely,
    /// chained imports of a real standard alias must retain its canonical terminal
    /// spelling instead of degrading to an unresolved generated type.
    fn resolve_ir_annotation_origin(index: &IrSchemaIndex<'_>, path: &str) -> IrAnnotationOrigin {
        fn resolve(
            index: &IrSchemaIndex<'_>,
            path: &str,
            visiting: &mut rustc_hash::FxHashSet<String>,
            depth: usize,
        ) -> IrAnnotationOrigin {
            if index.declares_direct_type(path) {
                return IrAnnotationOrigin::AuthoredType;
            }

            const MAX_IMPORT_CHAIN: usize = 256;
            if depth >= MAX_IMPORT_CHAIN || !visiting.insert(path.to_owned()) {
                // Validation diagnoses emitted alias cycles separately. For annotation
                // classification, avoid silently assigning builtin/typing semantics.
                return IrAnnotationOrigin::Other;
            }

            let imports = index.imports_covering(path);
            let longest_target = imports.iter().map(|import| import.target.len()).max();
            let resolved = if let Some(longest_target) = longest_target {
                let mut outcomes = imports
                    .into_iter()
                    .filter(|import| import.target.len() == longest_target)
                    .map(|import| {
                        let suffix = path
                            .strip_prefix(&import.target)
                            .expect("collected only matching import targets");
                        let rewritten = format!("{}{suffix}", import.origin);
                        resolve(index, &rewritten, visiting, depth + 1)
                    });
                let first = outcomes.next().unwrap_or(IrAnnotationOrigin::Other);
                outcomes.fold(first, |combined, outcome| match (combined, outcome) {
                    (IrAnnotationOrigin::AuthoredType, _)
                    | (_, IrAnnotationOrigin::AuthoredType) => IrAnnotationOrigin::AuthoredType,
                    (
                        IrAnnotationOrigin::CanonicalStandard(left),
                        IrAnnotationOrigin::CanonicalStandard(right),
                    ) if left == right => IrAnnotationOrigin::CanonicalStandard(left),
                    _ => IrAnnotationOrigin::Other,
                })
            } else if Module::is_standard_ir_annotation_path(path) {
                IrAnnotationOrigin::CanonicalStandard(path.to_owned())
            } else {
                IrAnnotationOrigin::Other
            };
            visiting.remove(path);
            resolved
        }

        resolve(index, path, &mut rustc_hash::FxHashSet::default(), 0)
    }

    fn is_standard_ir_annotation_root(path: &str) -> bool {
        matches!(path, "typing" | "typing_extensions" | "builtins")
    }

    fn is_standard_ir_annotation_path(path: &str) -> bool {
        Self::is_standard_ir_annotation_root(path)
            || ["typing.", "typing_extensions.", "builtins."]
                .iter()
                .any(|prefix| path.starts_with(prefix))
    }

    /// Build the per-module name map used to disambiguate external-IR annotations.
    ///
    /// Direct definitions retain their emitted local path. Generated cross-module
    /// re-exports do too, so signatures expose the authored public alias rather than
    /// bypassing it through the canonical origin. Standard typing/builtin imports are
    /// different: their target is not an emitted generated type, so preserve the
    /// canonical origin spelling that [`super::Function::parse_ir_annotation`] knows
    /// how to lower.
    pub(crate) fn ir_annotation_types(
        ir: &crate::schema::ModuleIr,
        schema_annotations: &IrSchemaAnnotations<'_>,
    ) -> rustc_hash::FxHashMap<String, String> {
        let mut local_annotation_types = rustc_hash::FxHashMap::default();
        let mut register_local_annotation_type = |path: &str, replacement: &str| {
            if let Some(local_path) = path
                .strip_prefix(&ir.name)
                .and_then(|suffix| suffix.strip_prefix('.'))
            {
                local_annotation_types.insert(local_path.to_owned(), replacement.to_owned());
            }
        };
        for path in ir
            .classes
            .iter()
            .map(|class| class.name.as_str())
            .chain(ir.enums.iter().map(|py_enum| py_enum.name.as_str()))
            .chain(ir.type_vars.iter().map(String::as_str))
        {
            let replacement = if Self::is_standard_ir_annotation_path(path) {
                Function::forced_custom_ir_annotation(path)
            } else {
                path.to_owned()
            };
            register_local_annotation_type(path, &replacement);
        }
        let mut registered_import_targets = rustc_hash::FxHashSet::default();
        for import in &ir.imports {
            if !registered_import_targets.insert(&import.target) {
                continue;
            }
            let replacement = match Self::resolve_ir_annotation_origin(
                schema_annotations.index(),
                &import.target,
            ) {
                IrAnnotationOrigin::CanonicalStandard(origin) => origin,
                IrAnnotationOrigin::AuthoredType
                    if Self::is_standard_ir_annotation_path(&import.target) =>
                {
                    Function::forced_custom_ir_annotation(&import.target)
                }
                IrAnnotationOrigin::AuthoredType | IrAnnotationOrigin::Other => {
                    import.target.clone()
                }
            };
            register_local_annotation_type(&import.target, &replacement);
        }

        // Schema-wide qualified and authored-alias mappings do not depend on `ir`, so they
        // are prepared once per schema and merged here. Merging after the module-local
        // registrations preserves the original insertion order, where a schema-wide entry
        // overwrites a module-local one with the same key.
        local_annotation_types.extend(
            schema_annotations
                .qualified
                .iter()
                .map(|(path, replacement)| (path.clone(), replacement.clone())),
        );

        // Qualified mappings are global, but annotations in this module may use any
        // descendant relative to the module root (`pkg.t.Type` -> `t.Type`).
        let local_mappings = local_annotation_types
            .iter()
            .filter_map(|(path, replacement)| {
                path.strip_prefix(&ir.name)
                    .and_then(|suffix| suffix.strip_prefix('.'))
                    .map(|local_path| (local_path.to_owned(), replacement.clone()))
            })
            .collect::<Vec<_>>();
        local_annotation_types.extend(local_mappings);
        local_annotation_types
    }

    /// Construct a [`Module`] from the generic IR with zero `pyo3` usage.
    ///
    /// Builds the same struct as [`Self::parse`] but from strings, recursing for
    /// submodules. The IR `Vec` order is preserved exactly. `ir_input_hash` is set
    /// to a content-derived hash of the source [`crate::schema::ModuleIr`], making
    /// the drift gate deterministic and independent of any Python interpreter.
    #[cfg(test)]
    pub(crate) fn from_ir(cfg: &Config, ir: &crate::schema::ModuleIr) -> Result<Self> {
        let all_modules = std::slice::from_ref(ir);
        Self::from_ir_in_schema(cfg, ir, &IrSchemaAnnotations::new(all_modules))
    }

    /// Schema-aware external-IR constructor used by [`crate::Codegen`].
    ///
    /// The complete validated schema is required to distinguish true standard
    /// annotation imports from generated types authored beneath standard-looking
    /// module names.
    pub(crate) fn from_ir_in_schema(
        cfg: &Config,
        ir: &crate::schema::ModuleIr,
        schema_annotations: &IrSchemaAnnotations<'_>,
    ) -> Result<Self> {
        let name = Path::from_py(&ir.name);
        // The module's own name is validated here too (not only its children below): an empty
        // name reaches `Path::name()` during generation and would otherwise surface far from
        // the ingestion boundary.
        if name.is_empty() {
            return Err(crate::PyBindgenError::CodegenError(
                "Module name must not be empty".to_owned(),
            ));
        }

        // Every emitted item derives its Rust ident from the last segment of its dotted
        // name via `Path::name()` (`segments.last()`), which panics on an empty path.
        // A blank name in the IR (e.g. `""` or a stray `"."`) would otherwise crash deep
        // in `generate` instead of surfacing here. Reject such names at the ingestion
        // boundary with a clear error.
        for class in &ir.classes {
            if Path::from_py(&class.name).is_empty() {
                return Err(crate::PyBindgenError::CodegenError(format!(
                    "Class name must not be empty (in module '{}')",
                    ir.name
                )));
            }
            for method in &class.methods {
                if Path::from_py(&method.name).is_empty() {
                    return Err(crate::PyBindgenError::CodegenError(format!(
                        "Method name must not be empty (in class '{}')",
                        class.name
                    )));
                }
            }
            for property in &class.properties {
                if Path::from_py(&property.name).is_empty() {
                    return Err(crate::PyBindgenError::CodegenError(format!(
                        "Property name must not be empty (in class '{}')",
                        class.name
                    )));
                }
            }
        }
        for py_enum in &ir.enums {
            if Path::from_py(&py_enum.name).is_empty() {
                return Err(crate::PyBindgenError::CodegenError(format!(
                    "Enum name must not be empty (in module '{}')",
                    ir.name
                )));
            }
        }

        // Module-level functions and properties are dispatched via their parent module
        // (`self.name.parent()`), so their names must be fully-qualified (dotted).
        // Reject a bare name up front with a clear error instead of panicking on the
        // missing parent deep in `generate`.
        for function in &ir.functions {
            if Path::from_py(&function.name).len() < 2 {
                return Err(crate::PyBindgenError::CodegenError(format!(
                    "Module-level function '{}' must be a fully-qualified dotted name \
                     (e.g. '{}.{}')",
                    function.name, ir.name, function.name
                )));
            }
        }
        for property in &ir.properties {
            if Path::from_py(&property.name).len() < 2 {
                return Err(crate::PyBindgenError::CodegenError(format!(
                    "Module-level property '{}' must be a fully-qualified dotted name \
                     (e.g. '{}.{}')",
                    property.name, ir.name, property.name
                )));
            }
        }

        let mut diagnostics = Vec::new();
        let prelude = ir.prelude.iter().map(|item| Ident::from_py(item)).collect();
        let imports = ir
            .imports
            .iter()
            .map(|import| Import::new(Path::from_py(&import.origin), Path::from_py(&import.target)))
            .collect();
        let submodules = ir
            .submodules
            .iter()
            .map(|submodule| Self::from_ir_in_schema(cfg, submodule, schema_annotations))
            .collect::<Result<Vec<_>>>()?;
        let mut enums = Vec::new();
        let mut enum_fallback_classes: Vec<crate::schema::ClassIr> = Vec::new();
        if cfg.generate_enums {
            for py_enum in &ir.enums {
                match PyEnum::from_ir(cfg, py_enum) {
                    Ok(parsed) => enums.push(parsed),
                    // Opt-in fallback: an enum that cannot be mirrored as a Rust enum
                    // (unrepresentable/colliding member idents, a dangling alias target, or no
                    // members) degrades to an owning Python object wrapper instead of aborting
                    // the whole generation. This mirrors the live path's enum fallback
                    // (`Module::parse`), but carries the distinct `enum_ir_mirror_fallback` code
                    // so report consumers can tell the live and IR fallbacks apart. The schema
                    // validator has already rejected an `EnumIr` sharing a dotted path with a
                    // `ClassIr` (`register_definition`), so this synthesized class cannot
                    // duplicate a real one.
                    Err(err) if cfg.enum_ir_class_fallback => {
                        crate::report::warn(
                            &mut diagnostics,
                            "enum_ir_mirror_fallback",
                            py_enum.name.clone(),
                            format!(
                                "External-IR enum '{name}' could not be generated as a Rust enum \
                                 ({err}). Falling back to a Python object wrapper; references to it \
                                 lower to the owning newtype and `set[{name}]` lowers to \
                                 `Bound<PySet>` instead of `HashSet<_>`.",
                                name = py_enum.name,
                            ),
                        );
                        enum_fallback_classes.push(crate::schema::ClassIr {
                            name: py_enum.name.clone(),
                            base_classes: Vec::new(),
                            docstring: py_enum.docstring.clone(),
                            methods: Vec::new(),
                            properties: Vec::new(),
                        });
                    }
                    // Flag off (the default): the loud abort is
                    // preserved exactly -- identical to the previous `collect::<Result<_>>()?`.
                    Err(err) => return Err(err),
                }
            }
        }

        // Direct definitions and import aliases can shadow spellings consumed specially
        // by the annotation parser (`Type`, `Annotated`, `list`, ...). Use the same map
        // as schema root-support validation so reservation and final lowering cannot
        // disagree about the rewritten annotation.
        let local_annotation_types =
            IrAnnotationTypes::new(Self::ir_annotation_types(ir, schema_annotations));

        let classes = ir
            .classes
            .iter()
            .chain(enum_fallback_classes.iter())
            .map(|class| Class::from_ir(cfg, class, &mut diagnostics, &local_annotation_types))
            .collect::<Result<Vec<_>>>()?;
        let type_vars = ir
            .type_vars
            .iter()
            .map(|type_var| TypeVar::from_ir(type_var))
            .collect();
        let functions = ir
            .functions
            .iter()
            .map(|function| {
                if matches!(
                    &function.kind,
                    crate::schema::FunctionKindIr::Method { .. }
                ) {
                    return Err(crate::PyBindgenError::CodegenError(format!(
                        "module lowering invariant violated: module-level function '{}' cannot use FunctionKindIr::Method",
                        function.name
                    )));
                }
                Function::from_ir(
                    cfg,
                    function,
                    FunctionType::from_ir_kind(&function.kind, None),
                    &mut diagnostics,
                    &local_annotation_types,
                )
            })
            .collect::<Result<Vec<_>>>()?;
        let properties = ir
            .properties
            .iter()
            .map(|property| {
                Property::from_ir(
                    cfg,
                    property,
                    PropertyOwner::Module,
                    &local_annotation_types,
                )
            })
            .collect::<Result<Vec<_>>>()?;

        // Content-derived hash of the source IR (stable across regenerations).
        let ir_bytes = serde_json::to_vec(ir).map_err(|error| {
            crate::PyBindgenError::CodegenError(format!(
                "failed to serialize external IR input for module '{}': {error}",
                ir.name
            ))
        })?;
        let ir_input_hash = Some(crate::report::stable_hash(ir_bytes));

        Ok(Self {
            name,
            prelude,
            imports,
            submodules,
            enums,
            classes,
            type_vars,
            functions,
            properties,
            docstring: ir.docstring.clone(),
            is_package: ir.is_package,
            source_code: None,
            diagnostics,
            ir_input_hash,
            is_synthetic_external_ir_parent: false,
        })
    }

    /// Whether this module (recursively, including submodules) references a
    /// collection-protocol type that lowers to the generic `PyCollection<T>` wrapper.
    pub(crate) fn mentions_enabled_collection(&self, cfg: &Config) -> bool {
        (cfg.generate_functions && self.functions.iter().any(Function::mentions_collection))
            || (cfg.generate_properties
                && self.properties.iter().any(Property::mentions_collection))
            || (cfg.generate_classes
                && self
                    .classes
                    .iter()
                    .any(|class| class.mentions_enabled_collection(cfg)))
            || (cfg.traverse_submodules
                && self
                    .submodules
                    .iter()
                    .any(|module| module.mentions_enabled_collection(cfg)))
    }

    /// Read an exact module-dictionary entry before invoking Python's dynamic attribute
    /// machinery. Module members must pass name-only policy before calling this helper;
    /// metadata such as `__all__` is acquired directly. An absent dictionary key retains
    /// normal `getattr` semantics without creating it.
    fn lookup_module_attr<'py>(
        module: &pyo3::Bound<'py, pyo3::types::PyModule>,
        attr_name: &str,
    ) -> pyo3::PyResult<pyo3::Bound<'py, pyo3::types::PyAny>> {
        if let Some(attr) = pyo3::types::PyDictMethods::get_item(&module.dict(), attr_name)? {
            Ok(attr)
        } else {
            module.getattr(attr_name)
        }
    }

    pub(crate) fn parse(cfg: &Config, module: &pyo3::Bound<pyo3::types::PyModule>) -> Result<Self> {
        let name = Path::from_live_module(&module.name()?.to_string())?;
        Self::parse_named(cfg, module, name)
    }

    pub(crate) fn parse_named(
        cfg: &Config,
        module: &pyo3::Bound<pyo3::types::PyModule>,
        name: Path,
    ) -> Result<Self> {
        let py = module.py();

        // Resolve `dir()` once. Custom modules may implement a stateful `__dir__`,
        // and prelude comparison must describe the same attribute snapshot that is
        // subsequently parsed.
        let attr_names = module
            .dir()?
            .iter()
            .map(|attr_name| {
                let attr_name = attr_name.to_string();
                Ident::try_from_py(&attr_name, || {
                    format!("module '{name}' __dir__ returned an empty attribute name")
                })
            })
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .unique()
            .collect_vec();

        let diagnostics = std::cell::RefCell::new(Vec::new());

        // Extract the index of the module as prelude (if enabled)
        let prelude = if cfg.generate_preludes {
            Self::extract_prelude(
                cfg,
                module,
                &name,
                &attr_names,
                &mut diagnostics.borrow_mut(),
            )?
        } else {
            PreludeExtraction::default()
        };

        // Determine if the module is a package that contains submodules
        let is_package = module.hasattr(pyo3::intern!(py, "__path__"))?;

        // Extract the list of all submodules for packages
        let mut submodules_to_process = if is_package {
            Self::extract_submodules(cfg, module, &name, &mut diagnostics.borrow_mut())?
        } else {
            HashSet::default()
        };

        // Initialize lists for all other members of the module
        let mut imports = Vec::new();
        let mut conflicting_imports = Vec::new();
        let mut enums = Vec::new();
        let mut classes: Vec<Class> = Vec::new();
        let mut type_vars = Vec::new();
        let mut functions = Vec::new();
        let mut properties = Vec::new();
        // Extract the list of all attribute names in the module
        attr_names
            .into_iter()
            // Expand each attribute to a tuple of (attr, attr_name, attr_module, attr_type)
            .filter_map(|attr_name| {
                // Reject on the name alone before dynamic access. An excluded attribute can be
                // supplied by a hostile module-level __getattr__/descriptor, so even attempting
                // to fetch it would violate the live-introspection policy.
                if !cfg.is_name_allowed(&attr_name) {
                    return None;
                }
                let attr = if let Some(attr) = prelude.resolved_attrs.get(&attr_name) {
                    attr.clone()
                } else if prelude.failed_names.contains(&attr_name) {
                    // Prelude resolution already emitted the one access diagnostic. Never
                    // execute or report the same failed name again during the `__dir__` walk.
                    return None;
                } else {
                    match Self::lookup_module_attr(module, attr_name.as_py()) {
                        Ok(attr) => attr,
                        Err(_) => {
                            crate::report::warn(
                                &mut diagnostics.borrow_mut(),
                                "attribute_access_failed",
                                name.to_py(),
                                format!(
                                    "Cannot get attribute '{attr_name}' of '{name}' even though it is listed in its `__dir__`. Bindings will not be generated.",
                                ),
                            );
                            return None;
                        }
                    }
                };
                let attr_module = Path::from_py(
                    &attr
                        .getattr(pyo3::intern!(py, "__module__"))
                        .map(|a| a.to_string())
                        .unwrap_or_default(),
                );
                let attr_type = attr.get_type();

                Some((attr, attr_name, attr_module, attr_type))
            })
            // Filter attributes based on the remaining (module-path) conditions.
            .filter(|(_attr, _attr_name, attr_module, _attr_type)| {
                cfg.is_module_allowed(attr_module)
            })
            // Iterate over the remaining attributes and parse them
            .try_for_each(|(attr, attr_name, attr_module, attr_type)| {
                let attr_name_full = name.join(&attr_name.clone().into());
                match AttributeVariant::determine(py, &attr, &attr_type, &attr_module, &name, true)
                    ?
                {
                    AttributeVariant::Import => {
                        let origin = attr_module.join(&Path::from_py(
                            &attr
                                .getattr(pyo3::intern!(py, "__name__"))
                                .map(|a| a.to_string())
                                .unwrap_or(attr_name.as_py().to_owned()),
                        ));

                        // Skip if the origin is the same as the target
                        if origin == attr_name_full {
                            return Ok(());
                        }

                        // Make sure the origin attribute is allowed (each segment of the path)
                        let is_origin_attr_allowed = (0..origin.len()).all(|i| {
                            let attr_name = &origin[i];
                            let attr_module = origin[..i].into();
                            let attr_type = if i == origin.len() - 1 {
                                attr_type.clone()
                            } else {
                                py.get_type::<pyo3::types::PyModule>()
                            };
                            cfg.is_attr_allowed(attr_name, &attr_module, &attr_type)
                        });
                        if !is_origin_attr_allowed {
                            return Ok(());
                        }

                        // Determine if the import overwrites a submodule. Only divert the
                        // import when the submodule will actually be emitted: with
                        // traversal disabled no `mod` is generated for it, so a same-named
                        // re-export whose origin is resolvable (e.g. an in-module alias) is
                        // valid and must not be silently dropped by the traversal-gated
                        // conflict handling below.
                        let import_overwrites_submodule =
                            cfg.traverse_submodules && submodules_to_process.contains(&attr_name);

                        // Generate the import
                        let import = Import::new(origin, attr_name_full);

                        // Add the import to the appropriate list
                        if import_overwrites_submodule {
                            conflicting_imports.push(import);
                        } else {
                            imports.push(import);
                        }
                    }
                    AttributeVariant::Module => {
                        // Note: This should technically not be necessary as `Self::extract_submodules` is supposed to extract all submodules
                        submodules_to_process.insert(attr_name.clone());
                    }
                    AttributeVariant::Class => {
                        let class_type = attr
                            .cast::<pyo3::types::PyType>()
                            .unwrap_or_else(|_| {
                                unreachable!("The attribute is known to be a class at this point")
                            });
                        let is_protocol = class_type
                            .getattr(pyo3::intern!(py, "_is_protocol"))
                            .and_then(|value| value.is_truthy())
                            .unwrap_or(false);
                        let is_runtime_protocol = class_type
                            .getattr(pyo3::intern!(py, "_is_runtime_protocol"))
                            .and_then(|value| value.is_truthy())
                            .unwrap_or(false);
                        if is_protocol && !is_runtime_protocol {
                            crate::report::warn(
                                &mut diagnostics.borrow_mut(),
                                "protocol_non_runtime_checkable_fallback",
                                attr_name_full.to_py(),
                                format!(
                                    "Protocol '{name}.{attr_name}' is not runtime-checkable. Emitting an untyped alias instead of a nominal wrapper whose isinstance check would raise TypeError.",
                                ),
                            );
                            type_vars.push(TypeVar::new(attr_name_full));
                            return Ok(());
                        }
                        let force_raw_enum_fallback = cfg
                            .enum_raw_fallback_paths
                            .iter()
                            .any(|path| path == &attr_name_full.to_py());
                        // `generate_enums` owns every enum-derived output. Classify while the
                        // gate is disabled so an enum cannot fall through to the owning class
                        // fallback; ordinary classes continue through `Class::parse` below.
                        if !cfg.generate_enums
                            && !matches!(PyEnum::classify(class_type)?, PyEnumParse::NotEnum)
                        {
                            return Ok(());
                        }
                        if cfg.generate_enums && !force_raw_enum_fallback {
                            match PyEnum::classify(class_type)? {
                                PyEnumParse::Supported => {
                                    match PyEnum::parse(cfg, class_type, attr_name_full.clone()) {
                                        Ok(py_enum) => {
                                            enums.push(py_enum);
                                            return Ok(());
                                        }
                                        Err(err) => {
                                            crate::report::warn(
                                                &mut diagnostics.borrow_mut(),
                                                "enum_mirror_fallback",
                                                attr_name_full.to_py(),
                                                format!(
                                                    "Python enum '{name}.{attr_name}' could not be generated as a Rust enum ({err}). Falling back to a Python object wrapper.",
                                                ),
                                            );
                                        }
                                    }
                                }
                                PyEnumParse::Unsupported => {
                                    crate::report::warn(
                                        &mut diagnostics.borrow_mut(),
                                        "enum_flag_fallback",
                                        attr_name_full.to_py(),
                                        format!(
                                            "Flag-style enum '{name}.{attr_name}' cannot be mirrored as a Rust flag newtype because its member values are not all plain non-negative integers fitting u64, or its members could not be introspected. Falling back to a Python object wrapper.",
                                        ),
                                    );
                                }
                                PyEnumParse::NotEnum => {}
                            }
                        } else if force_raw_enum_fallback
                            && matches!(PyEnum::classify(class_type)?, PyEnumParse::Supported)
                        {
                            crate::report::warn(
                                &mut diagnostics.borrow_mut(),
                                "enum_configured_raw_fallback",
                                attr_name_full.to_py(),
                                format!(
                                    "Python enum '{name}.{attr_name}' is configured for raw fallback. Generating a Python object wrapper.",
                                ),
                            );
                        }
                        let class = Class::parse(
                            cfg,
                            class_type,
                            attr_name_full,
                            &mut diagnostics.borrow_mut(),
                        )?;
                        classes.push(class);
                    }
                    AttributeVariant::TypeVar => {
                        let type_var = TypeVar::new(attr_name_full);
                        type_vars.push(type_var);
                    }
                    AttributeVariant::Function | AttributeVariant::Method => {
                        let function = Function::parse(
                            cfg,
                            &attr,
                            attr_name_full,
                            FunctionType::Function,
                            &mut diagnostics.borrow_mut(),
                        )?;
                        functions.push(function);
                    }
                    AttributeVariant::Closure => {
                        let function = Function::parse(
                            cfg,
                            &attr,
                            attr_name_full,
                            FunctionType::Closure,
                            &mut diagnostics.borrow_mut(),
                        )?;
                        functions.push(function);
                    }
                    AttributeVariant::Property => {
                        let property = Property::parse(
                            cfg,
                            &attr,
                            attr_name_full,
                            PropertyOwner::Module,
                            &mut diagnostics.borrow_mut(),
                        )
                        ?;
                        properties.push(property);
                    }
                }
                Result::Ok(())
            })?;

        // Process submodules
        let submodules = if cfg.traverse_submodules {
            let mut submodules_to_process = submodules_to_process.into_iter().collect_vec();
            submodules_to_process.sort_unstable();
            let mut parsed_submodules = Vec::with_capacity(submodules_to_process.len());
            for submodule_name in submodules_to_process {
                let full_submodule_name = name.join(&submodule_name.clone().into());
                let full_submodule_name_py = full_submodule_name.to_py();
                diagnostics.borrow_mut().push(DomainDiagnostic::info(
                    "discovery_submodule_attempted",
                    full_submodule_name_py.clone(),
                    format!("Attempting to discover submodule '{full_submodule_name}'."),
                ));

                // Handle submodules that are overwritten by imports separately
                if let Some(conflicting_import) = conflicting_imports
                    .iter()
                    .find(|import| import.target == full_submodule_name)
                {
                    // A same-named attribute (e.g. `from .foo import Foo as foo`)
                    // shadows the `foo` submodule. Rust cannot bind both a module
                    // and another item under one name, so the attribute wins and
                    // the submodule is not emitted. Only the shadowing member is
                    // recovered below; any OTHER members of the submodule are
                    // unreachable through this name and are dropped. Surface that,
                    // rather than dropping the whole submodule silently.
                    crate::report::warn(
                        &mut diagnostics.borrow_mut(),
                        "submodule_shadowed_by_import",
                        full_submodule_name_py.clone(),
                        format!(
                            "Submodule '{full_submodule_name}' is shadowed by a same-named imported attribute; the attribute is generated instead. Members of the submodule other than that attribute will not be generated."
                        ),
                    );

                    let conflicting_origin = conflicting_import.origin.clone();
                    let conflicting_target = conflicting_import.target.clone();
                    // Recovery needs only the direct shadowing member. Disabling recursive
                    // traversal prevents recording descendant discovery facts for a module
                    // that is intentionally not emitted as a module at all.
                    let mut recovery_cfg = cfg.clone();
                    recovery_cfg.traverse_submodules = false;
                    let recovery = py
                        .import(full_submodule_name_py.as_str())
                        .map_err(|error| {
                            format!(
                                "could not import shadowed submodule '{full_submodule_name}': {error}"
                            )
                        })
                        .and_then(|module| {
                            Self::parse(&recovery_cfg, &module).map_err(|error| {
                                format!(
                                    "could not parse shadowed submodule '{full_submodule_name}': {error}"
                                )
                            })
                        });

                    match recovery {
                        Ok(submodule) => {
                            let mut recovered_items = 0;
                            // It could be any attribute, so all of them need to be checked.
                            if let Some(import) = submodule
                                .imports
                                .into_iter()
                                .find(|import| import.target == conflicting_origin)
                            {
                                // Rebuild through `Import::new` rather than reassigning
                                // `target` in place: `import_type` is a cached function
                                // of (origin, target), so mutating `target` alone would
                                // leave it stale. Reconstructing keeps the invariant.
                                imports
                                    .push(Import::new(import.origin, conflicting_target.clone()));
                                recovered_items += 1;
                            }
                            if let Some(mut class) = submodule
                                .classes
                                .into_iter()
                                .find(|class| class.name == conflicting_origin)
                            {
                                class.name = conflicting_target.clone();
                                classes.push(class);
                                recovered_items += 1;
                            }
                            if let Some(mut py_enum) = submodule
                                .enums
                                .into_iter()
                                .find(|py_enum| py_enum.name == conflicting_origin)
                            {
                                py_enum.name = conflicting_target.clone();
                                enums.push(py_enum);
                                recovered_items += 1;
                            }
                            if let Some(mut type_var) = submodule
                                .type_vars
                                .into_iter()
                                .find(|type_var| type_var.name == conflicting_origin)
                            {
                                type_var.name = conflicting_target.clone();
                                type_vars.push(type_var);
                                recovered_items += 1;
                            }
                            if let Some(mut function) = submodule
                                .functions
                                .into_iter()
                                .find(|function| function.name == conflicting_origin)
                            {
                                function.name = conflicting_target.clone();
                                functions.push(function);
                                recovered_items += 1;
                            }
                            if let Some(mut property) = submodule
                                .properties
                                .into_iter()
                                .find(|property| property.name == conflicting_origin)
                            {
                                property.name = conflicting_target;
                                properties.push(property);
                                recovered_items += 1;
                            }

                            let (code, message) = if recovered_items == 0 {
                                (
                                    "discovery_submodule_skipped",
                                    format!(
                                        "Submodule '{full_submodule_name}' was shadowed by an import and had no recoverable matching member; the child was skipped."
                                    ),
                                )
                            } else {
                                (
                                    "discovery_submodule_truncated",
                                    format!(
                                        "Submodule '{full_submodule_name}' was shadowed by an import; recovered {recovered_items} matching member(s), but the child module was not emitted."
                                    ),
                                )
                            };
                            diagnostics.borrow_mut().push(DomainDiagnostic::warning(
                                code,
                                full_submodule_name_py,
                                message,
                            ));
                        }
                        Err(error) => {
                            diagnostics.borrow_mut().push(DomainDiagnostic::warning(
                                "discovery_submodule_failed",
                                full_submodule_name_py,
                                error,
                            ));
                        }
                    }
                    continue;
                }

                // Try to import both as a package and as an attribute of the current module.
                // Keep both failures so best-effort omission remains path-qualified and
                // actionable instead of ending in the previous `.ok()`-based silence.
                let submodule = match py.import(full_submodule_name_py.as_str()) {
                    Ok(submodule) => Ok(submodule),
                    Err(import_error) => {
                        let import_error = import_error.to_string();
                        match module.getattr(submodule_name.as_py()) {
                            Ok(attr) => attr.cast_into::<pyo3::types::PyModule>().map_err(|error| {
                                format!(
                                    "could not import submodule '{full_submodule_name}': {import_error}; fallback attribute lookup succeeded but the value was not a module: {error}"
                                )
                            }),
                            Err(getattr_error) => Err(format!(
                                "could not import submodule '{full_submodule_name}': {import_error}; fallback attribute lookup failed: {getattr_error}"
                            )),
                        }
                    }
                };
                let submodule = match submodule {
                    Ok(submodule) => submodule,
                    Err(error) => {
                        diagnostics.borrow_mut().push(DomainDiagnostic::warning(
                            "discovery_submodule_failed",
                            full_submodule_name_py,
                            error,
                        ));
                        continue;
                    }
                };

                match Self::parse(cfg, &submodule) {
                    Ok(submodule) => {
                        parsed_submodules.push(submodule);
                        diagnostics.borrow_mut().push(DomainDiagnostic::info(
                            "discovery_submodule_emitted",
                            full_submodule_name_py,
                            format!("Discovered submodule '{full_submodule_name}' was emitted."),
                        ));
                    }
                    Err(error) => {
                        diagnostics.borrow_mut().push(DomainDiagnostic::warning(
                            "discovery_submodule_failed",
                            full_submodule_name_py,
                            format!(
                                "Could not parse discovered submodule '{full_submodule_name}': {error}"
                            ),
                        ));
                    }
                }
            }
            parsed_submodules
        } else {
            Vec::default()
        };

        // Extract the docstring of the module
        let docstring = {
            let docstring = module.getattr(pyo3::intern!(py, "__doc__"))?.to_string();
            if docstring.is_empty() || docstring == "None" {
                None
            } else {
                Some(docstring)
            }
        };

        Ok(Self {
            name,
            prelude: prelude.names,
            imports,
            submodules,
            enums,
            classes,
            type_vars,
            functions,
            properties,
            docstring,
            is_package,
            source_code: None,
            diagnostics: diagnostics.into_inner(),
            ir_input_hash: None,
            is_synthetic_external_ir_parent: false,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn generate(
        &self,
        cfg: &Config,
        top_level_modules: &[Self],
        all_types: &[(Path, Path)],
        all_classes: &HashMap<Path, &Class>,
        enum_types: &HashSet<Path>,
        emit_collection_wrapper: bool,
        import_mode: &ImportMode,
        state: &mut GenerationState<'_>,
    ) -> Result<proc_macro2::TokenStream> {
        let rust_module_path = super::naming::rust_path(&self.name, SymbolKind::Module)
            .ok_or_else(|| {
                crate::PyBindgenError::CodegenError(format!(
                    "module '{}' cannot be represented as a Rust path",
                    self.name
                ))
            })?;
        let emitted_imports = if cfg.generate_imports {
            self.emitted_imports(cfg, top_level_modules, import_mode, state.diagnostics)?
        } else {
            Vec::new()
        };
        self.validate_type_namespace(cfg, &rust_module_path, &emitted_imports)?;
        let mut counts = GeneratedCounts::default();
        let mut output = proc_macro2::TokenStream::new();

        // Extra configuration for top-level modules
        let is_top_level = top_level_modules.contains(self);
        if is_top_level {
            output.extend(quote::quote! {
                #[allow(
                    clippy::all,
                    clippy::nursery,
                    clippy::pedantic,
                    non_camel_case_types,
                    non_snake_case,
                    non_upper_case_globals,
                    unused
                )]
            });
        }

        // Documentation
        if cfg.generate_docs {
            if let Some(mut docstring) = self.docstring.clone() {
                crate::utils::text::format_docstring(&mut docstring);
                output.extend(quote::quote! {
                    #[doc = #docstring]
                });
            }
        }

        // Get the names of all functions to avoid name clashes
        let mut scoped_function_names = if cfg.generate_functions {
            NameAllocator::from_python_names(
                self.functions.iter().map(|function| function.name.name()),
                SymbolKind::Function,
            )
        } else {
            NameAllocator::default()
        };
        scoped_function_names
            .reserve_generated_module_values(cfg.generate_provenance, self.source_code.is_some());

        // Get all local types mapped to their emitted Rust paths. The *validity* of the
        // emitted Rust path (e.g. that a sanitized raw name like `Self` -> `s_Self`
        // parses) is still decided here, at the same point and in the same iteration
        // order as before, so a sanitizer bug still surfaces as the identical error at
        // the identical moment. Only the comparatively expensive `syn` tokenization is
        // deferred: for the overwhelmingly common shape a cheap allocation-free segment
        // scan (`Path::is_syn_parseable`) proves the parse would succeed and leaves it to
        // the first (rare) read of `LocalType::rust_path`; only the rest is parsed
        // eagerly here. This closure runs `all_types.len()` times per module -- M x T,
        // ~56k for numpy -- so skipping the tokenizer on the common path is the win.
        let resolve_local_type = |python_path: Path, relative_path: Path, is_enum: bool| {
            let local_type = LocalType::try_from_path(relative_path.clone(), is_enum).map_err(
                |error| {
                    crate::PyBindgenError::CodegenError(format!(
                        "failed to resolve Python type '{python_path}' to emitted Rust path '{}': {error}",
                        relative_path.to_rs()
                    ))
                },
            )?;
            Result::Ok((python_path, local_type))
        };
        let mut imported_local_types = Vec::new();
        for emitted_import in &emitted_imports {
            let import = emitted_import.import;
            for (python_path, emitted_rust_path) in all_types.iter().filter(|(python_path, _)| {
                python_path.starts_with(&emitted_import.resolved_python_origin)
                    // `all_types` also contains global qualified aliases. Expand only
                    // canonical definitions here, or a module alias rooted under its
                    // own origin could recursively manufacture `alias.alias.*` keys.
                    && top_level_modules
                        .iter()
                        .any(|module| module.emitted_rust_path(python_path, cfg).is_some())
            }) {
                let suffix_len = python_path.len() - emitted_import.resolved_python_origin.len();
                let python_suffix =
                    Path::from(&python_path[emitted_import.resolved_python_origin.len()..]);
                let short_python_path =
                    Path::from(import.target.name().to_owned()).concat(&python_suffix);
                let qualified_python_path = import.target.concat(&python_suffix);

                let imported_rust_root = super::naming::rust_path(
                    &import.target.parent().unwrap_or_default(),
                    SymbolKind::Module,
                )
                .map(|path| {
                    path.join(&Path::from(Ident::from_rs(
                        &emitted_import.alias.to_string(),
                    )))
                })
                .ok_or_else(|| {
                    crate::PyBindgenError::CodegenError(format!(
                        "import target '{}' cannot be represented as an emitted Rust path",
                        import.target
                    ))
                })?;
                let emitted_suffix =
                    Path::from(&emitted_rust_path[emitted_rust_path.len() - suffix_len..]);
                let imported_rust_path = imported_rust_root.concat(&emitted_suffix);
                let relative_path = rust_module_path.relative_to(&imported_rust_path, false);
                // Both aliases re-export the same origin type, so they share its kind.
                let origin_is_enum = enum_types.contains(python_path);
                imported_local_types.push(resolve_local_type(
                    short_python_path,
                    relative_path.clone(),
                    origin_is_enum,
                )?);
                imported_local_types.push(resolve_local_type(
                    qualified_python_path,
                    relative_path,
                    origin_is_enum,
                )?);
            }
        }
        // Preserve the fully-qualified global namespace separately from the merged
        // module-local namespace. Inherited member signatures use the former through an
        // owner-scoped view, preventing a descendant's short alias from rebinding an
        // annotation authored in the ancestor module.
        let global_by_python_path = all_types
            .iter()
            .cloned()
            .map(|(python_path, rust_path)| {
                let relative_path = rust_module_path.relative_to(&rust_path, false);
                let is_enum = enum_types.contains(&python_path);
                resolve_local_type(python_path, relative_path, is_enum)
            })
            .collect::<Result<HashMap<_, _>>>()?;
        let local_by_python_path = all_types
            .iter()
            // Deferred/quoted annotations commonly spell a class as `User` rather
            // than `package.module.User`. Register unqualified aliases only for
            // definitions directly owned by this module; sibling submodule types stay
            // qualified, avoiding ambiguous short-name capture.
            .filter(|(python_path, _)| python_path.parent().as_ref() == Some(&self.name))
            .map(|(python_path, rust_path)| {
                let short_python_path = Path::from(python_path.name().clone());
                let relative_path = rust_module_path.relative_to(rust_path, false);
                let is_enum = enum_types.contains(python_path);
                resolve_local_type(short_python_path, relative_path, is_enum)
            })
            .chain(imported_local_types.into_iter().map(Result::Ok))
            .collect::<Result<HashMap<_, _>>>()?;
        // Keep generated support out of the Python-name map. A legitimate local class
        // named `PyCollection` must remain resolvable even when the generic collection
        // wrapper is also emitted at the generated root.
        let collection_wrapper = emit_collection_wrapper
            .then(|| {
                let wrapper_name = Path::from_rs(crate::syntax::class::COLLECTION_WRAPPER_NAME);
                let relative_path = rust_module_path.relative_to(&wrapper_name, false);
                resolve_local_type(wrapper_name, relative_path, false)
                    .map(|(_, local_type)| std::rc::Rc::new(local_type))
            })
            .transpose()?;
        let local_types = LocalTypes::from_maps(
            std::rc::Rc::new(local_by_python_path),
            std::rc::Rc::new(global_by_python_path),
            collection_wrapper,
        );

        // Generate the module content
        let mut module_content = proc_macro2::TokenStream::new();
        // Imports
        for emitted_import in &emitted_imports {
            let generated = emitted_import.import.generate(
                &rust_module_path,
                &emitted_import.emitted_origin,
                &emitted_import.alias,
            )?;
            if !generated.is_empty() {
                counts.imports += 1;
                module_content.extend(generated);
            }
        }
        // Type variables
        if cfg.generate_type_vars {
            for type_var in &self.type_vars {
                let generated = type_var.generate(cfg, state.diagnostics)?;
                if !generated.is_empty() {
                    counts.type_vars += 1;
                    module_content.extend(generated);
                }
            }
        }
        // Enums
        if cfg.generate_enums {
            for py_enum in &self.enums {
                let generated = py_enum.generate(cfg, import_mode, state.diagnostics)?;
                if !generated.is_empty() {
                    counts.enums += 1;
                    state.stats.emitted_enums.insert(py_enum.name.clone());
                    module_content.extend(generated);
                }
            }
        }
        // Classes
        if cfg.generate_classes {
            let empty_inherited_members = super::class::InheritedMemberPlan::default();
            for class in &self.classes {
                let inherited_members = if class.uses_dedup_model(import_mode) {
                    state.inherited_members.get(&class.name).ok_or_else(|| {
                        crate::PyBindgenError::CodegenError(format!(
                            "inherited-member planning invariant violated: no plan for deduplicated class '{}'",
                            class.name
                        ))
                    })?
                } else {
                    &empty_inherited_members
                };
                let generated = class.generate(
                    cfg,
                    &local_types,
                    enum_types,
                    state.diagnostics,
                    import_mode,
                    all_classes,
                    inherited_members,
                )?;
                if !generated.is_empty() {
                    counts.classes += 1;
                    module_content.extend(generated);
                }
            }
        }
        // Functions
        if cfg.generate_functions {
            for function in &self.functions {
                let generated = function.generate(
                    cfg,
                    &mut scoped_function_names,
                    &local_types,
                    state.diagnostics,
                    import_mode,
                )?;
                let FunctionImplementation::Function(generated) = generated else {
                    unreachable!("Methods in modules are not possible")
                };
                if !generated.is_empty() {
                    counts.functions += 1;
                    module_content.extend(generated);
                }
            }
        }
        // Properties
        if cfg.generate_properties {
            for property in &self.properties {
                let generated = property.generate(
                    cfg,
                    &mut scoped_function_names,
                    &local_types,
                    import_mode,
                )?;
                let FunctionImplementation::Function(generated) = generated else {
                    unreachable!("Methods in modules are not possible")
                };
                if !generated.is_empty() {
                    counts.properties += 1;
                    module_content.extend(generated);
                }
            }
        }
        scoped_function_names.validate_resolutions()?;
        // Prelude. Generated after functions/properties so aliases allocated by the
        // naming plan are re-exported instead of their original colliding names.
        if cfg.generate_preludes {
            module_content.extend(self.generate_prelude(
                cfg,
                &scoped_function_names,
                &emitted_imports,
            )?);
        }
        // Submodules
        if cfg.traverse_submodules {
            for module in &self.submodules {
                let generated = module.generate(
                    cfg,
                    top_level_modules,
                    all_types,
                    all_classes,
                    enum_types,
                    emit_collection_wrapper,
                    import_mode,
                    state,
                )?;
                if !generated.is_empty() {
                    counts.submodules += 1;
                    module_content.extend(generated);
                }
            }
        }

        // Embed the source code if the module was parsed directly from source code
        let embed_source_code_fn = if let Some(source_code) = &self.source_code {
            let module_name = self.name.to_py();
            let file_name = format!("{module_name}/__init__.py");
            quote::quote! {
                /// Embed the Python source code of the module into the Python interpreter
                /// in order to enable the use of the generated Rust bindings.
                pub fn pyo3_embed_python_source_code<'py>(py: ::pyo3::marker::Python<'py>) -> ::pyo3::PyResult<()> {
                    pyo3::types::PyAnyMethods::set_item(
                        &pyo3::types::PyAnyMethods::getattr(
                            py.import(pyo3::intern!(py, "sys"))?.as_any(),
                            pyo3::intern!(py, "modules"),
                        )?,
                        #module_name,
                        pyo3::types::PyModule::from_code(
                            py,
                            ::pyo3::ffi::c_str!(#source_code),
                            ::pyo3::ffi::c_str!(#file_name),
                            ::pyo3::ffi::c_str!(#module_name),
                        )?,
                    )
                }
            }
        } else {
            proc_macro2::TokenStream::new()
        };
        let provenance = if cfg.generate_provenance {
            let generator_version = env!("CARGO_PKG_VERSION");
            let module_name = self.name.to_py();
            let input_hash = match state.stats.input_hashes.get(&self.name) {
                Some(input_hash) => input_hash.clone(),
                None => self.input_hash()?,
            };
            // Memoize the (potentially whole-subtree-serializing) input hash so
            // `collect_report_entries` reuses it instead of recomputing. First writer
            // wins so a hypothetical name collision cannot diverge from the value
            // emitted into the provenance tokens below.
            state
                .stats
                .input_hashes
                .entry(self.name.clone())
                .or_insert_with(|| input_hash.clone());
            // A module that puts its own `str` type at module scope (e.g. `builtins`'s
            // newtype) shadows the primitive, so `&str` would bind to that item (E0308).
            // Two things land at module scope: the module's own generated items
            // (`declares_item_named`) *and* generated re-export aliases (`use ... as str;`),
            // the latter in the type namespace for every non-value kind. Qualify the
            // provenance consts iff either names `str`, so every other module's bytes are
            // unchanged (no corpus module aliases or declares a type named `str`).
            let str_shadowed = self.declares_item_named("str")
                || emitted_imports.iter().any(|emitted_import| {
                    !emitted_import.kind.is_value() && emitted_import.alias == "str"
                });
            let str_ty: syn::Type = if str_shadowed {
                syn::parse_quote!(&::core::primitive::str)
            } else {
                syn::parse_quote!(&str)
            };
            quote::quote! {
                #[doc(hidden)]
                pub const PYO3_BINDGEN_GENERATOR_VERSION: #str_ty = #generator_version;
                #[doc(hidden)]
                pub const PYO3_BINDGEN_MODULE_NAME: #str_ty = #module_name;
                #[doc(hidden)]
                pub const PYO3_BINDGEN_INPUT_HASH: #str_ty = #input_hash;
            }
        } else {
            proc_macro2::TokenStream::new()
        };

        // Finalize the module with its content
        let name = self.name.name();
        let module_ident: syn::Ident = match rust_ident(name, SymbolKind::Module) {
            Some(ident) => {
                if TryInto::<syn::Ident>::try_into(name).is_err() {
                    crate::report::warn(
                        state.diagnostics,
                        "module_invalid_ident_renamed",
                        self.name.to_py(),
                        format!(
                            "Module '{}' is an invalid Rust ident for a module name. Renamed to '{ident}'.",
                            self.name,
                        ),
                    );
                }
                ident
            }
            None => {
                crate::report::warn(
                    state.diagnostics,
                    "module_invalid_ident_skipped",
                    self.name.to_py(),
                    format!(
                        "Module '{}' is an invalid Rust ident for a module name. Renaming failed. Bindings will not be generated.",
                        self.name
                    ),
                );
                return Ok(proc_macro2::TokenStream::new());
            }
        };
        output.extend(quote::quote! {
            pub mod #module_ident {
                #provenance
                #embed_source_code_fn
                #module_content
            }
        });

        state.stats.emitted_modules.insert(self.name.clone());
        state.stats.counts.insert(self.name.clone(), counts);

        Ok(output)
    }

    fn emitted_imports<'a>(
        &'a self,
        cfg: &Config,
        top_level_modules: &[Self],
        import_mode: &ImportMode,
        diagnostics: &mut Vec<DomainDiagnostic>,
    ) -> Result<Vec<EmittedImport<'a>>> {
        let mut value_names = self.module_value_name_plan(cfg);
        let mut emitted = Vec::new();
        for import in self
            .imports
            .iter()
            // Live introspection distinguishes actual external imports from
            // generated-tree package reexports. External IR cannot make that decision
            // from path prefixes alone: two generated roots (`other.Widget` reexported
            // as `pkg.Widget`) are valid peers. Attempt every explicit nonidentity IR
            // import and let the global emitted-symbol lookup below decide.
            .filter(|import| {
                if import_mode.is_static() {
                    import.is_nonidentity()
                } else {
                    import.is_generated_reexport()
                }
            })
        {
            let Some((resolved_python_origin, emitted_origin, kind)) =
                Self::resolve_emitted_rust_path(
                    top_level_modules,
                    &import.origin,
                    cfg,
                    import_mode,
                    &mut Vec::new(),
                    &mut Vec::new(),
                )?
            else {
                crate::report::warn(
                    diagnostics,
                    "import_origin_unresolved",
                    import.target.to_py(),
                    format!(
                        "re-exported import '{}' -> '{}' was dropped: its origin is not an \
                         emitted symbol in any top-level module.",
                        import.target, import.origin
                    ),
                );
                continue;
            };
            let mut alias = rust_ident(import.target.name(), kind).ok_or_else(|| {
                crate::PyBindgenError::CodegenError(format!(
                    "import target '{}' cannot be represented as an emitted Rust symbol",
                    import.target
                ))
            })?;
            if kind.is_value() && value_names.is_generated(import.target.name()) {
                let preferred = alias.to_string();
                let preferred = preferred.strip_prefix("r#").unwrap_or(&preferred);
                alias = value_names.allocate(&format!("py_{preferred}"));
            }
            emitted.push(EmittedImport {
                import,
                resolved_python_origin,
                emitted_origin,
                alias,
                kind,
            });
        }
        Ok(emitted)
    }

    pub(crate) fn import_is_emitted(import: &Import, import_mode: &ImportMode) -> bool {
        if import_mode.is_static() {
            import.is_nonidentity()
        } else {
            import.is_generated_reexport()
        }
    }

    fn collect_emitted_imports_matching<'a>(
        &'a self,
        path: &Path,
        cfg: &Config,
        import_mode: &ImportMode,
        imports: &mut Vec<&'a Import>,
    ) {
        imports.extend(self.imports.iter().filter(|import| {
            path.starts_with(&import.target) && Self::import_is_emitted(import, import_mode)
        }));
        if cfg.traverse_submodules {
            for submodule in &self.submodules {
                submodule.collect_emitted_imports_matching(path, cfg, import_mode, imports);
            }
        }
    }

    /// Resolve a generated Python path through zero or more emitted re-export aliases.
    ///
    /// The canonical Python origin is returned with the Rust path so local-type aliases
    /// can be populated from `all_types` even when the immediate import origin is itself
    /// an alias.
    pub(crate) fn resolve_emitted_rust_path(
        top_level_modules: &[Self],
        path: &Path,
        cfg: &Config,
        import_mode: &ImportMode,
        resolving: &mut Vec<Path>,
        applied_targets: &mut Vec<Path>,
    ) -> Result<Option<(Path, Path, SymbolKind)>> {
        if let Some((rust_path, kind)) = top_level_modules
            .iter()
            .find_map(|module| module.emitted_rust_path(path, cfg))
        {
            return Ok(Some((path.clone(), rust_path, kind)));
        }
        if let Some(cycle_start) = resolving.iter().position(|candidate| candidate == path) {
            let cycle = resolving[cycle_start..]
                .iter()
                .chain(std::iter::once(path))
                .map(Path::to_py)
                .join(" -> ");
            return Err(crate::PyBindgenError::CodegenError(format!(
                "import cycle while resolving '{path}': {cycle}"
            )));
        }
        const MAX_IMPORT_CHAIN: usize = 256;
        if resolving.len() >= MAX_IMPORT_CHAIN {
            return Err(crate::PyBindgenError::CodegenError(format!(
                "import chain while resolving '{path}' exceeds the supported depth \
                 ({MAX_IMPORT_CHAIN})"
            )));
        }

        let mut imports = Vec::new();
        for module in top_level_modules {
            module.collect_emitted_imports_matching(path, cfg, import_mode, &mut imports);
        }
        let longest_target = imports.iter().map(|import| import.target.len()).max();
        let imports = longest_target.map_or_else(Vec::new, |longest_target| {
            imports
                .into_iter()
                .filter(|import| import.target.len() == longest_target)
                .collect::<Vec<_>>()
        });
        match imports.as_slice() {
            [] => Ok(None),
            [import] => {
                let suffix = Path::from(&path[import.target.len()..]);
                let rewritten = import.origin.concat(&suffix);
                if let Some(cycle_start) = applied_targets
                    .iter()
                    .position(|target| target == &import.target)
                    .filter(|_| rewritten.len() >= path.len())
                {
                    let cycle = resolving[cycle_start..]
                        .iter()
                        .chain(std::iter::once(path))
                        .map(Path::to_py)
                        .join(" -> ");
                    return Err(crate::PyBindgenError::CodegenError(format!(
                        "import cycle while resolving '{path}': {cycle}"
                    )));
                }
                resolving.push(path.clone());
                applied_targets.push(import.target.clone());
                let resolved = Self::resolve_emitted_rust_path(
                    top_level_modules,
                    &rewritten,
                    cfg,
                    import_mode,
                    resolving,
                    applied_targets,
                );
                applied_targets.pop();
                resolving.pop();
                resolved
            }
            _ => Err(crate::PyBindgenError::CodegenError(format!(
                "import target '{path}' is ambiguous: it aliases multiple origins ({})",
                imports
                    .iter()
                    .map(|import| import.origin.to_py())
                    .join(", ")
            ))),
        }
    }

    fn emitted_rust_path(&self, path: &Path, cfg: &Config) -> Option<(Path, SymbolKind)> {
        if self.name == *path {
            return super::naming::rust_path(&self.name, SymbolKind::Module)
                .map(|path| (path, SymbolKind::Module));
        }
        if cfg.generate_classes {
            if let Some(class) = self.classes.iter().find(|class| class.name == *path) {
                return super::naming::rust_path(&class.name, SymbolKind::Class)
                    .map(|path| (path, SymbolKind::Class));
            }
        }
        if cfg.generate_enums {
            if let Some(py_enum) = self.enums.iter().find(|py_enum| py_enum.name == *path) {
                return super::naming::rust_path(&py_enum.name, SymbolKind::Enum)
                    .map(|path| (path, SymbolKind::Enum));
            }
        }
        if cfg.generate_functions && self.functions.iter().any(|function| function.name == *path) {
            if let Some(ident) = self.module_value_name_plan(cfg).resolved(path.name()) {
                return self
                    .emitted_value_path(&ident)
                    .map(|path| (path, SymbolKind::Function));
            }
        }
        if cfg.generate_properties
            && self
                .properties
                .iter()
                .any(|property| property.name == *path)
        {
            if let Some(ident) = self.module_value_name_plan(cfg).resolved(path.name()) {
                return self
                    .emitted_value_path(&ident)
                    .map(|path| (path, SymbolKind::Function));
            }
        }
        if cfg.generate_type_vars {
            if let Some(type_var) = self
                .type_vars
                .iter()
                .find(|type_var| type_var.name == *path)
            {
                return super::naming::rust_path(&type_var.name, SymbolKind::TypeVar)
                    .map(|path| (path, SymbolKind::TypeVar));
            }
        }
        if cfg.traverse_submodules {
            return self
                .submodules
                .iter()
                .find_map(|module| module.emitted_rust_path(path, cfg));
        }
        None
    }

    fn emitted_value_path(&self, ident: &syn::Ident) -> Option<Path> {
        super::naming::rust_path(&self.name, SymbolKind::Module)
            .map(|module_path| module_path.join(&Path::from(Ident::from_rs(&ident.to_string()))))
    }

    fn module_value_name_plan(&self, cfg: &Config) -> NameAllocator {
        let mut names = if cfg.generate_functions {
            NameAllocator::from_python_names(
                self.functions.iter().map(|function| function.name.name()),
                SymbolKind::Function,
            )
        } else {
            NameAllocator::default()
        };
        names.reserve_generated_module_values(cfg.generate_provenance, self.source_code.is_some());
        if cfg.generate_functions {
            for function in &self.functions {
                let name = function.name.name();
                if let Some(ident) = rust_ident(name, SymbolKind::Function) {
                    let ident = if names.is_generated(name) {
                        names.allocate(&format!("py_{}", name.as_py()))
                    } else {
                        ident
                    };
                    names.record_resolution(name, &ident);
                }
            }
        }
        if cfg.generate_properties {
            for property in &self.properties {
                property.plan_module_names(&mut names);
            }
        }
        names
    }

    fn validate_type_namespace(
        &self,
        cfg: &Config,
        rust_module_path: &Path,
        emitted_imports: &[EmittedImport<'_>],
    ) -> Result<()> {
        let mut symbols = SymbolTable::new("type");

        if cfg.generate_classes {
            for class in &self.classes {
                if let Some(ident) = class_rust_ident(class) {
                    symbols.register(&ident.to_string(), class.name.to_py())?;
                    symbols.register(
                        &format!("{ident}Methods"),
                        format!("{} methods trait", class.name.to_py()),
                    )?;
                }
            }
        }
        if cfg.generate_enums {
            for py_enum in &self.enums {
                if let Some(ident) = enum_rust_ident(py_enum) {
                    symbols.register(&ident.to_string(), py_enum.name.to_py())?;
                }
            }
        }
        if cfg.generate_type_vars {
            for type_var in &self.type_vars {
                if let Some(ident) = rust_ident(type_var.name.name(), SymbolKind::TypeVar) {
                    symbols.register(&ident.to_string(), type_var.name.to_py())?;
                }
            }
        }
        let mut value_imports = Vec::new();
        for emitted_import in emitted_imports {
            let import = emitted_import.import;
            if !import
                .generate(
                    rust_module_path,
                    &emitted_import.emitted_origin,
                    &emitted_import.alias,
                )?
                .is_empty()
            {
                let origin = format!("import {} as {}", import.origin, import.target);
                if emitted_import.kind.is_value() {
                    value_imports.push((emitted_import.alias.clone(), origin));
                } else {
                    symbols.register(&emitted_import.alias.to_string(), origin)?;
                }
            }
        }
        self.module_value_name_plan(cfg)
            .validate_resolutions_with(&value_imports)?;
        if cfg.traverse_submodules {
            for submodule in &self.submodules {
                if let Some(ident) = rust_ident(submodule.name.name(), SymbolKind::Module) {
                    symbols.register(&ident.to_string(), submodule.name.to_py())?;
                }
            }
        }

        Ok(())
    }

    /// Whether this module emits a top-level item (class newtype, mirror enum, or type
    /// variable) under the given Rust identifier. Mirrors the emission name set built by
    /// `class_rust_ident`/`enum_rust_ident`/`rust_ident`. Used to detect when a generated
    /// item shadows a primitive at module scope (e.g. `builtins`'s `str`), so the
    /// module-level provenance consts can be qualified out of that shadow. This covers a
    /// module's *own* generated items only; generated re-export aliases (`use ... as str;`)
    /// also land at module scope and are checked separately at the provenance call site.
    fn declares_item_named(&self, name: &str) -> bool {
        self.classes
            .iter()
            .filter_map(class_rust_ident)
            .chain(self.enums.iter().filter_map(enum_rust_ident))
            .chain(
                self.type_vars
                    .iter()
                    .filter_map(|type_var| rust_ident(type_var.name.name(), SymbolKind::TypeVar)),
            )
            .any(|ident| ident == name)
    }

    pub(crate) fn collect_report_entries(
        &self,
        cfg: &Config,
        stats: &GenerationStats,
        parent_emitted: bool,
        modules: &mut Vec<ModuleReport>,
        diagnostics: &mut Vec<DomainDiagnostic>,
    ) -> Result<()> {
        let emitted = parent_emitted && stats.emitted_modules.contains(&self.name);
        modules.push(ModuleReport {
            name: self.name.to_py(),
            input_kind: if self.ir_input_hash.is_some() {
                "external_ir".to_owned()
            } else if self.source_code.is_some() {
                "embedded_source".to_owned()
            } else {
                "imported_module".to_owned()
            },
            input_hash: match stats.input_hashes.get(&self.name) {
                Some(cached) => cached.clone(),
                None => self.input_hash()?,
            },
            source_hash: self
                .source_code
                .as_ref()
                .map(|source_code| crate::report::stable_hash(source_code.as_bytes())),
            enums: if emitted {
                self.enums
                    .iter()
                    .filter(|py_enum| stats.emitted_enums.contains(&py_enum.name))
                    .map(PyEnum::report)
                    .collect()
            } else {
                Vec::new()
            },
            counts: if emitted {
                stats.counts.get(&self.name).cloned().unwrap_or_default()
            } else {
                GeneratedCounts::default()
            },
        });
        diagnostics.extend(self.diagnostics.iter().cloned());
        if cfg.traverse_submodules {
            for submodule in &self.submodules {
                submodule.collect_report_entries(cfg, stats, emitted, modules, diagnostics)?;
            }
        }
        Ok(())
    }

    pub(crate) fn input_hash(&self) -> Result<String> {
        if let Some(ir_input_hash) = &self.ir_input_hash {
            Ok(ir_input_hash.clone())
        } else if let Some(source_code) = &self.source_code {
            Ok(crate::report::stable_hash(source_code.as_bytes()))
        } else {
            self.imported_module_fingerprint()
        }
    }

    fn imported_module_fingerprint(&self) -> Result<String> {
        let fingerprint = ImportedModuleFingerprint {
            kind: IMPORTED_MODULE_FINGERPRINT_KIND,
            version: IMPORTED_MODULE_FINGERPRINT_VERSION,
            module: self,
        };
        let bytes = serde_json::to_vec(&fingerprint).map_err(|error| {
            crate::PyBindgenError::CodegenError(format!(
                "failed to serialize imported-module fingerprint for '{}': {error}",
                self.name
            ))
        })?;
        Ok(crate::report::stable_hash(bytes))
    }

    fn extract_submodules(
        cfg: &Config,
        module: &pyo3::Bound<pyo3::types::PyModule>,
        module_name: &Path,
        diagnostics: &mut Vec<DomainDiagnostic>,
    ) -> Result<HashSet<Ident>> {
        let py = module.py();

        // Extract the paths of the module
        let module_paths = module
            .getattr(pyo3::intern!(py, "__path__"))?
            .try_iter()?
            .map(|x| Ok(x?.to_string()))
            .collect::<Result<Vec<_>>>()?;

        // Extract the names of all submodules via `pkgutil.iter_modules`
        let iter_modules = crate::runtime_sentinels::attr(
            py,
            crate::runtime_sentinels::StdlibModule::Pkgutil,
            "iter_modules",
        )?;
        iter_modules
            .call1((module_paths,))?
            .try_iter()?
            .map(|submodule| {
                let submodule_name = submodule?.getattr(pyo3::intern!(py, "name"))?.to_string();
                Ident::try_from_py(&submodule_name, || {
                    format!("pkgutil returned an empty submodule name for module '{module_name}'")
                })
            })
            // Filter based on various configurable conditions
            .filter_map(|submodule_name| match submodule_name {
                Ok(submodule_name)
                    if cfg.is_attr_allowed(
                        &submodule_name,
                        module_name,
                        &py.get_type::<pyo3::types::PyModule>(),
                    ) =>
                {
                    Some(Ok(submodule_name))
                }
                Ok(submodule_name) => {
                    let full_submodule_name = module_name.join(&submodule_name.clone().into());
                    diagnostics.push(DomainDiagnostic::info(
                        "discovery_submodule_attempted",
                        full_submodule_name.to_py(),
                        format!(
                            "Attempting to discover submodule '{full_submodule_name}'."
                        ),
                    ));
                    diagnostics.push(DomainDiagnostic::warning(
                        "discovery_submodule_skipped",
                        full_submodule_name.to_py(),
                        format!(
                            "Discovered submodule '{full_submodule_name}' was excluded by configuration policy."
                        ),
                    ));
                    None
                }
                Err(error) => Some(Err(error)),
            })
            .collect()
    }

    fn extract_prelude<'py>(
        cfg: &Config,
        module: &pyo3::Bound<'py, pyo3::types::PyModule>,
        module_name: &Path,
        attr_names: &[Ident],
        diagnostics: &mut Vec<DomainDiagnostic>,
    ) -> Result<PreludeExtraction<'py>> {
        // Read `__all__` without invoking a custom module accessor when the exact value is
        // present in the module dictionary. A genuinely absent `__all__` is ordinary and
        // remains absent; unlike `PyModuleMethods::index`, this never creates an empty list.
        let index = match Self::lookup_module_attr(module, "__all__") {
            Ok(index) => Some(index),
            Err(error)
                if error.is_instance_of::<pyo3::exceptions::PyAttributeError>(module.py()) =>
            {
                None
            }
            Err(error) => {
                crate::report::warn(
                    diagnostics,
                    "attribute_access_failed",
                    module_name.to_py(),
                    format!(
                        "Cannot access attribute '__all__' of '{module_name}': {error}. Prelude bindings will not be generated."
                    ),
                );
                None
            }
        };

        let mut index_attr_names = Vec::new();
        if let Some(index) = index {
            match index.try_iter() {
                Ok(iter) => {
                    let mut iteration_failed = false;
                    for item in iter {
                        let item = match item {
                            Ok(item) => item,
                            Err(error) => {
                                crate::report::warn(
                                    diagnostics,
                                    "attribute_access_failed",
                                    module_name.to_py(),
                                    format!(
                                        "Cannot iterate attribute '__all__' of '{module_name}': {error}. Prelude bindings will not be generated."
                                    ),
                                );
                                iteration_failed = true;
                                break;
                            }
                        };
                        let attr_name = item.to_string();
                        index_attr_names.push(Ident::try_from_py(&attr_name, || {
                            format!(
                                "module '{module_name}' __all__ contains an empty attribute name"
                            )
                        })?);
                    }
                    if iteration_failed {
                        index_attr_names.clear();
                    }
                }
                Err(error) => {
                    crate::report::warn(
                        diagnostics,
                        "attribute_access_failed",
                        module_name.to_py(),
                        format!(
                            "Cannot iterate attribute '__all__' of '{module_name}': {error}. Prelude bindings will not be generated."
                        ),
                    );
                }
            }
        }
        index_attr_names = index_attr_names.into_iter().unique().collect();

        // Compare the index with public attrs of the module
        // Return an empty vector if they are identical (no need to generate a prelude)
        {
            let public_attr_names_set: HashSet<_> = attr_names
                .iter()
                .filter(|attr_name| !attr_name.as_py().starts_with('_'))
                .cloned()
                .collect();
            let index_attr_names_set: HashSet<_> = index_attr_names.iter().cloned().collect();

            if index_attr_names_set == public_attr_names_set {
                return Ok(PreludeExtraction::default());
            }
        }

        // Resolve every allowed `__all__` member exactly once. The cached object then
        // feeds re-export/type filtering and the later `__dir__` traversal. Keep every
        // successful allowed resolution, including names filtered out of the prelude.
        let mut resolved_names = Vec::with_capacity(index_attr_names.len());
        let mut resolved_attrs = HashMap::default();
        let mut failed_names = HashSet::default();
        for attr_name in index_attr_names {
            if !cfg.is_name_allowed(&attr_name) {
                continue;
            }
            match Self::lookup_module_attr(module, attr_name.as_py()) {
                Ok(attr) => {
                    resolved_names.push(attr_name.clone());
                    resolved_attrs.insert(attr_name, attr);
                }
                Err(error) => {
                    crate::report::warn(
                        diagnostics,
                        "attribute_access_failed",
                        module_name.to_py(),
                        format!(
                            "Cannot get attribute '{attr_name}' of '{module_name}' even though it is listed in its `__all__`: {error}. Prelude binding will not be generated."
                        ),
                    );
                    failed_names.insert(attr_name);
                }
            }
        }

        // If the generation of dependencies is disabled, retain only reexports.
        if !cfg.generate_dependencies {
            resolved_names.retain(|attr_name| {
                let attr = resolved_attrs
                    .get(attr_name)
                    .expect("resolved names and attributes are populated together");
                module_name.root().is_some_and(|root_module| {
                    let attr_module = Path::from_py(
                        &attr
                            .getattr(pyo3::intern!(module.py(), "__module__"))
                            .map(|a| a.to_string())
                            .unwrap_or_default(),
                    );
                    attr_module.starts_with(&root_module)
                })
            });
        }

        // Retain only allowed attributes
        resolved_names.retain(|attr_name| {
            let attr = resolved_attrs
                .get(attr_name)
                .expect("resolved names and attributes are populated together");
            let attr_type = attr.get_type();
            cfg.is_attr_allowed(attr_name, module_name, &attr_type)
        });

        // The prelude re-exports carry no inherent order -- they lower to a single
        // `pub use ...::{...}` group -- yet `__all__` is otherwise emitted verbatim. Some
        // modules (numpy, for one) build `__all__` from a `set`, whose iteration order is
        // randomized per interpreter process (`PYTHONHASHSEED`). Preserving that order
        // makes the generated bindings, and their input-hash fingerprint, differ from run
        // to run -- non-reproducible output that also defeats `--check` drift detection.
        // Sort by the Python-facing name for a stable, reproducible order independent of
        // how upstream happened to construct `__all__`.
        resolved_names.sort_unstable_by(|a, b| a.as_py().cmp(b.as_py()));

        Ok(PreludeExtraction {
            names: resolved_names,
            resolved_attrs,
            failed_names,
        })
    }

    fn generate_prelude(
        &self,
        cfg: &Config,
        name_plan: &NameAllocator,
        emitted_imports: &[EmittedImport<'_>],
    ) -> Result<proc_macro2::TokenStream> {
        // Skip if the prelude is empty
        if self.prelude.is_empty() {
            return Ok(proc_macro2::TokenStream::new());
        }

        // Generate the prelude content (re-export all prelude items)
        let exports = self
            .prelude
            .iter()
            .filter_map(|ident| self.prelude_rust_ident(ident, cfg, name_plan, emitted_imports))
            .map(|ident| {
                quote::quote! {
                    #ident,
                }
            })
            .collect::<proc_macro2::TokenStream>();

        // Return empty prelude if there are no exports
        if exports.is_empty() {
            return Ok(proc_macro2::TokenStream::new());
        }

        // Finalize the prelude with its content
        let mut names = super::naming::NameAllocator::default();
        let mut reserve_items = |items: &[(&Ident, SymbolKind)]| {
            for (name, kind) in items {
                if let Some(ident) = rust_ident(name, *kind) {
                    names.reserve_ident(&ident);
                }
            }
        };
        if cfg.traverse_submodules {
            let items = self
                .submodules
                .iter()
                .map(|item| (item.name.name(), SymbolKind::Module))
                .collect::<Vec<_>>();
            reserve_items(&items);
        }
        if cfg.generate_classes {
            let items = self
                .classes
                .iter()
                .map(|item| (item.name.name(), SymbolKind::Class))
                .collect::<Vec<_>>();
            reserve_items(&items);
        }
        if cfg.generate_enums {
            let items = self
                .enums
                .iter()
                .map(|item| (item.name.name(), SymbolKind::Enum))
                .collect::<Vec<_>>();
            reserve_items(&items);
        }
        if cfg.generate_type_vars {
            let items = self
                .type_vars
                .iter()
                .map(|item| (item.name.name(), SymbolKind::TypeVar))
                .collect::<Vec<_>>();
            reserve_items(&items);
        }
        if cfg.generate_functions {
            for function in &self.functions {
                if let Some(ident) = name_plan.resolved(function.name.name()) {
                    names.reserve_ident(&ident);
                }
            }
        }
        if cfg.generate_properties {
            for property in &self.properties {
                if let Some(ident) = name_plan.resolved(property.name.name()) {
                    names.reserve_ident(&ident);
                }
            }
        }
        for emitted_import in emitted_imports {
            names.reserve_ident(&emitted_import.alias);
        }
        let prelude_ident = names.allocate("prelude");
        Ok(quote::quote! {
            pub mod #prelude_ident {
                pub use super::{#exports};
            }
        })
    }

    fn prelude_rust_ident(
        &self,
        name: &Ident,
        cfg: &Config,
        name_plan: &NameAllocator,
        emitted_imports: &[EmittedImport<'_>],
    ) -> Option<syn::Ident> {
        if cfg.traverse_submodules
            && self
                .submodules
                .iter()
                .any(|module| module.name.name() == name)
        {
            return rust_ident(name, SymbolKind::Module);
        }
        if cfg.generate_enums && self.enums.iter().any(|py_enum| py_enum.name.name() == name) {
            return rust_ident(name, SymbolKind::Enum);
        }
        if cfg.generate_classes && self.classes.iter().any(|class| class.name.name() == name) {
            return rust_ident(name, SymbolKind::Class);
        }
        if cfg.generate_functions
            && self
                .functions
                .iter()
                .any(|function| function.name.name() == name)
        {
            return name_plan
                .resolved(name)
                .or_else(|| rust_ident(name, SymbolKind::Function));
        }
        if cfg.generate_type_vars
            && self
                .type_vars
                .iter()
                .any(|type_var| type_var.name.name() == name)
        {
            return rust_ident(name, SymbolKind::TypeVar);
        }
        if cfg.generate_properties
            && self
                .properties
                .iter()
                .any(|property| property.name.name() == name)
        {
            return name_plan
                .resolved(name)
                .or_else(|| rust_ident(name, SymbolKind::Function));
        }
        // `__all__` can list a name that this module only re-exports via a plain
        // Python import (e.g. `from .sub import Foo` in an `__init__.py`), which
        // `AttributeVariant::determine` classifies as `Import` rather than as a
        // local class/enum/function/etc. Resolve those against the same emitted
        // imports that back the module's own `pub use` re-export, so the prelude
        // stays consistent with what is actually reachable at this module's path
        // instead of silently dropping the export.
        if let Some(emitted_import) = emitted_imports
            .iter()
            .find(|emitted_import| emitted_import.import.target.name() == name)
        {
            return Some(emitted_import.alias.clone());
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pyo3::types::PyAnyMethods;

    #[test]
    fn private_ir_lowering_rejects_module_method_invariant() {
        let ir = crate::schema::ModuleIr {
            name: "module_method_invariant".to_owned(),
            docstring: None,
            is_package: false,
            classes: Vec::new(),
            enums: Vec::new(),
            functions: vec![crate::schema::FunctionIr {
                name: "module_method_invariant.bound".to_owned(),
                kind: crate::schema::FunctionKindIr::Method {
                    method_type: crate::schema::MethodKindIr::Instance,
                },
                params: Vec::new(),
                return_type: String::new(),
                docstring: None,
            }],
            properties: Vec::new(),
            submodules: Vec::new(),
            prelude: Vec::new(),
            imports: Vec::new(),
            type_vars: Vec::new(),
        };

        let error = Module::from_ir(&Config::default(), &ir)
            .expect_err("private IR bypass must reject module methods explicitly");
        match error {
            crate::PyBindgenError::CodegenError(message) => assert_eq!(
                message,
                "module lowering invariant violated: module-level function \
                 'module_method_invariant.bound' cannot use FunctionKindIr::Method"
            ),
            other => panic!("unexpected lowering error: {other:?}"),
        }
    }

    fn imported_fingerprint_fixture() -> Module {
        let ir = serde_json::from_value(serde_json::json!({
            "name": "fingerprint_fixture",
            "docstring": "fixed fixture",
            "is_package": true,
            "prelude": ["Widget", "Mode"],
            "imports": [{
                "origin": "fingerprint_fixture.child.Item",
                "target": "fingerprint_fixture.Item"
            }],
            "type_vars": ["fingerprint_fixture.T"],
            "enums": [{
                "name": "fingerprint_fixture.Mode",
                "kind": "StrEnum",
                "variants": [{"py_name": "FAST", "value_repr": "'fast'"}],
                "aliases": [{
                    "py_name": "QUICK",
                    "target_py_name": "FAST",
                    "value_repr": "'fast'"
                }]
            }],
            "classes": [{
                "name": "fingerprint_fixture.Widget",
                "docstring": "fixture class",
                "methods": [{
                    "name": "fingerprint_fixture.Widget.render",
                    "kind": {"Method": {"method_type": "Instance"}},
                    "params": [
                        {
                            "name": "self",
                            "kind": "PositionalOrKeyword",
                            "annotation": "fingerprint_fixture.Widget"
                        },
                        {
                            "name": "values",
                            "kind": "PositionalOrKeyword",
                            "annotation": "list[int]",
                            "has_default": true
                        }
                    ],
                    "return_type": "str",
                    "docstring": "render values"
                }],
                "properties": [{
                    "name": "fingerprint_fixture.Widget.enabled",
                    "py_type": "bool",
                    "setter_type": "bool",
                    "readonly": false,
                    "docstring": "enabled state",
                    "setter_docstring": "set enabled state"
                }]
            }],
            "functions": [{
                "name": "fingerprint_fixture.make_widget",
                "kind": "Function",
                "params": [{
                    "name": "name",
                    "kind": "KeywordOnly",
                    "annotation": "str"
                }],
                "return_type": "fingerprint_fixture.Widget"
            }],
            "properties": [{
                "name": "fingerprint_fixture.version",
                "py_type": "int",
                "readonly": true
            }],
            "submodules": [{
                "name": "fingerprint_fixture.child",
                "is_package": false,
                "type_vars": ["fingerprint_fixture.child.Item"]
            }]
        }))
        .unwrap();
        let mut module = Module::from_ir(&Config::default(), &ir).unwrap();

        fn mark_imported(module: &mut Module) {
            module.ir_input_hash = None;
            module.submodules.iter_mut().for_each(mark_imported);
        }
        mark_imported(&mut module);
        module
    }

    #[test]
    fn imported_module_fingerprint_has_versioned_golden_value() {
        let module = imported_fingerprint_fixture();

        assert_eq!(
            module.input_hash().unwrap(),
            "fnv1a64:3fff463c1c7a58d6",
            "changing the imported-module fingerprint requires a version bump",
        );
    }

    #[test]
    fn imported_module_fingerprint_ignores_non_shape_fields() {
        let module = imported_fingerprint_fixture();
        let expected = module.imported_module_fingerprint().unwrap();

        let mut changed = module.clone();
        changed.source_code = Some("print('not part of imported shape')".to_owned());
        changed.ir_input_hash = Some("fnv1a64:ffffffffffffffff".to_owned());
        changed.diagnostics.push(DomainDiagnostic {
            severity: crate::report::DomainSeverity::Warning,
            code: "fixture_warning".into(),
            path: Some("fingerprint_fixture".to_owned()),
            message: "not part of imported shape".to_owned(),
        });

        assert_eq!(changed.imported_module_fingerprint().unwrap(), expected);
    }

    #[test]
    fn imported_module_fingerprint_tracks_shape_fields() {
        let module = imported_fingerprint_fixture();
        let expected = module.imported_module_fingerprint().unwrap();

        let mut changed = module.clone();
        changed.docstring = Some("changed fixture".to_owned());
        assert_ne!(changed.imported_module_fingerprint().unwrap(), expected);

        let mut changed = module.clone();
        let mut import = changed.imports[0].clone();
        import.target = Path::from_py("fingerprint_fixture.OtherItem");
        changed.imports.push(import);
        assert_ne!(changed.imported_module_fingerprint().unwrap(), expected);

        let mut changed = module.clone();
        let mut submodule = changed.submodules[0].clone();
        submodule.name = Path::from_py("fingerprint_fixture.other_child");
        changed.submodules.push(submodule);
        assert_ne!(changed.imported_module_fingerprint().unwrap(), expected);

        let mut changed = module.clone();
        let mut py_enum = changed.enums[0].clone();
        py_enum.name = Path::from_py("fingerprint_fixture.OtherMode");
        changed.enums.push(py_enum);
        assert_ne!(changed.imported_module_fingerprint().unwrap(), expected);

        let mut changed = module.clone();
        let mut class = changed.classes[0].clone();
        class.name = Path::from_py("fingerprint_fixture.OtherWidget");
        changed.classes.push(class);
        assert_ne!(changed.imported_module_fingerprint().unwrap(), expected);

        let mut changed = module.clone();
        changed.classes[0]
            .direct_base_classes
            .push(Path::from_py("fingerprint_fixture.Base"));
        assert_ne!(
            changed.imported_module_fingerprint().unwrap(),
            expected,
            "direct-base changes can alter MRO without changing dir() members",
        );

        let mut changed = module.clone();
        let mut function = changed.functions[0].clone();
        function.name = Path::from_py("fingerprint_fixture.other_widget");
        changed.functions.push(function);
        assert_ne!(changed.imported_module_fingerprint().unwrap(), expected);

        let mut reordered = module.clone();
        let mut function = reordered.functions[0].clone();
        function.name = Path::from_py("fingerprint_fixture.other_widget");
        reordered.functions.push(function);
        let ordered_hash = reordered.imported_module_fingerprint().unwrap();
        reordered.functions.swap(0, 1);
        assert_ne!(
            reordered.imported_module_fingerprint().unwrap(),
            ordered_hash,
            "fingerprints must preserve generation-relevant vector order",
        );
    }

    #[test]
    fn submodule_discovery_uses_canonical_pkgutil_and_restores_target_poison() {
        pyo3::Python::initialize();
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let package_dir = std::env::temp_dir().join(format!(
            "pyo3_bindgen_pkgutil_guard_{}_{}",
            std::process::id(),
            unique,
        ));
        std::fs::create_dir_all(&package_dir).unwrap();
        struct RemoveDirectory(std::path::PathBuf);
        impl Drop for RemoveDirectory {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _package_dir_cleanup = RemoveDirectory(package_dir.clone());
        std::fs::write(package_dir.join("child.py"), "").unwrap();

        let (discovered, poison_restored) = pyo3::Python::attach(|py| {
            crate::io_utils::with_suppressed_python_output(py, false, false, || {
                crate::runtime_sentinels::prime(py)?;
                let pkgutil = py.import(pyo3::intern!(py, "pkgutil"))?;
                let _restore = crate::runtime_sentinels::TestAttributeRestore::new(
                    py,
                    &pkgutil,
                    "iter_modules",
                )?;
                let poison = py.eval(pyo3::ffi::c_str!("lambda paths: ()"), None, None)?;
                pkgutil.setattr(pyo3::intern!(py, "iter_modules"), &poison)?;

                let module = pyo3::types::PyModule::new(py, "pkgutil_guard_fixture")?;
                module.setattr(
                    pyo3::intern!(py, "__path__"),
                    [package_dir.to_string_lossy().as_ref()],
                )?;
                let mut diagnostics = Vec::new();
                let result = crate::runtime_sentinels::with_restored(py, || {
                    Module::extract_submodules(
                        &Config::default(),
                        &module,
                        &Path::from_py("pkgutil_guard_fixture"),
                        &mut diagnostics,
                    )
                });
                let poison_restored = pkgutil
                    .getattr(pyo3::intern!(py, "iter_modules"))?
                    .is(&poison);
                Ok((result, poison_restored))
            })
        })
        .unwrap();

        let discovered = discovered.unwrap();
        assert!(
            discovered.contains(&Ident::from_py("child")),
            "a poisoned pkgutil.iter_modules silently dropped the real child module",
        );
        assert!(
            poison_restored,
            "the canonical-global guard did not restore the target's pkgutil mutation",
        );
    }

    #[test]
    fn deduplicated_class_rejects_a_missing_inherited_member_plan() {
        let ir = serde_json::from_value(serde_json::json!({
            "name": "missing_plan",
            "is_package": false,
            "classes": [{"name": "missing_plan.A"}]
        }))
        .unwrap();
        let cfg = Config::default();
        let module = Module::from_ir(&cfg, &ir).unwrap();
        let mut diagnostics = Vec::new();
        let mut stats = GenerationStats::default();
        let plans = HashMap::default();
        let mut state = GenerationState::new(&mut diagnostics, &mut stats, &plans);
        let error = module
            .generate(
                &cfg,
                std::slice::from_ref(&module),
                &[],
                &HashMap::default(),
                &HashSet::default(),
                false,
                &ImportMode::Static(HashSet::default()),
                &mut state,
            )
            .expect_err("a required missing plan must not silently erase inherited members");
        assert!(
            error.to_string().contains(
                "inherited-member planning invariant violated: no plan for deduplicated class 'missing_plan.A'"
            ),
            "{error}"
        );
    }

    /// Reproduces the common `from .sub import Foo` + `__all__ = ["Foo"]` idiom: `Foo`
    /// is only reachable through `self.imports` (classified as an `Import` by
    /// `AttributeVariant::determine`, not as a class/enum/function/etc. directly owned
    /// by this module). The prelude must still re-export it instead of silently
    /// dropping it. This unit-level fixture exercises
    /// `Module::generate`/`prelude_rust_ident` directly; public external IR covers the
    /// same contract in the cross-root compile regression.
    #[test]
    fn prelude_reexports_all_listed_import_of_a_submodule_class() {
        let ir = serde_json::from_value(serde_json::json!({
            "name": "prelude_import_fixture",
            "is_package": true,
            "prelude": ["Foo"],
            "imports": [{
                "origin": "prelude_import_fixture.sub.Foo",
                "target": "prelude_import_fixture.Foo"
            }],
            "submodules": [{
                "name": "prelude_import_fixture.sub",
                "is_package": false,
                "classes": [{"name": "prelude_import_fixture.sub.Foo"}]
            }]
        }))
        .unwrap();
        let module = Module::from_ir(&Config::default(), &ir).unwrap();
        let top_level_modules = [module.clone()];

        let mut diagnostics = Vec::new();
        let mut stats = GenerationStats::default();
        let inherited_members = [(
            Path::from_py("prelude_import_fixture.sub.Foo"),
            super::super::class::InheritedMemberPlan::default(),
        )]
        .into_iter()
        .collect();
        let mut state = GenerationState::new(&mut diagnostics, &mut stats, &inherited_members);
        let known_modules: HashSet<Path> = HashSet::default();
        let all_classes: HashMap<Path, &Class> = HashMap::default();
        let tokens = module
            .generate(
                &Config::default(),
                &top_level_modules,
                &[],
                &all_classes,
                &HashSet::default(),
                false,
                &ImportMode::Static(known_modules),
                &mut state,
            )
            .unwrap();
        let generated = prettyplease::unparse(&syn::parse2(tokens).unwrap());

        assert!(
            generated.contains("pub use self::sub::Foo;"),
            "expected the module-level re-export of the submodule class:\n{generated}"
        );
        // Bound the slice to just past the `pub mod prelude { ... }` block itself (it
        // is only ever a couple of short lines) so a match can't accidentally come
        // from the unrelated `pub mod sub { ... }` content emitted right after it.
        // Match on the literal opening brace (`"pub mod prelude {"`, not just
        // `"pub mod prelude"`) since the fixture's own top-level module name
        // ("prelude_import_fixture") starts with "prelude" and would otherwise match
        // its own `pub mod prelude_import_fixture {` declaration first.
        let prelude_start = generated
            .find("pub mod prelude {")
            .unwrap_or_else(|| panic!("expected a generated prelude module:\n{generated}"));
        let prelude_end = (prelude_start + 200).min(generated.len());
        let prelude_section = &generated[prelude_start..prelude_end];
        assert!(
            prelude_section.contains("Foo"),
            "prelude must re-export the `__all__`-listed class imported from a submodule, \
             instead of silently dropping it:\n{generated}"
        );
    }
}
