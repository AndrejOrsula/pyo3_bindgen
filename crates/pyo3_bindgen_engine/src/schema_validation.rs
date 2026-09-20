use crate::{schema::SchemaIr, PyBindgenError, Result};

#[derive(Clone, Copy)]
enum DefinitionKind {
    Module,
    Class,
    Enum,
    Function,
    Property,
    TypeVar,
}

impl DefinitionKind {
    const fn symbol_kind(self) -> crate::syntax::naming::SymbolKind {
        match self {
            Self::Module => crate::syntax::naming::SymbolKind::Module,
            Self::Class => crate::syntax::naming::SymbolKind::Class,
            Self::Enum => crate::syntax::naming::SymbolKind::Enum,
            Self::Function | Self::Property => crate::syntax::naming::SymbolKind::Function,
            Self::TypeVar => crate::syntax::naming::SymbolKind::TypeVar,
        }
    }

    const fn is_value(self) -> bool {
        matches!(self, Self::Function | Self::Property)
    }
}

fn emitted_definition_kind(
    module: &crate::schema::ModuleIr,
    path: &str,
    cfg: &crate::Config,
) -> Option<DefinitionKind> {
    if module.name == path {
        return Some(DefinitionKind::Module);
    }
    if cfg.generate_classes && module.classes.iter().any(|item| item.name == path) {
        return Some(DefinitionKind::Class);
    }
    if cfg.generate_enums && module.enums.iter().any(|item| item.name == path) {
        return Some(DefinitionKind::Enum);
    }
    if cfg.generate_functions && module.functions.iter().any(|item| item.name == path) {
        return Some(DefinitionKind::Function);
    }
    if cfg.generate_properties && module.properties.iter().any(|item| item.name == path) {
        return Some(DefinitionKind::Property);
    }
    if cfg.generate_type_vars && module.type_vars.iter().any(|item| item == path) {
        return Some(DefinitionKind::TypeVar);
    }
    cfg.traverse_submodules.then(|| {
        module
            .submodules
            .iter()
            .find_map(|submodule| emitted_definition_kind(submodule, path, cfg))
    })?
}

fn emitted_definition_kind_in_schema(
    modules: &[crate::schema::ModuleIr],
    path: &str,
    cfg: &crate::Config,
) -> Option<DefinitionKind> {
    fn contains_module_path_or_descendant(modules: &[crate::schema::ModuleIr], path: &str) -> bool {
        modules.iter().any(|module| {
            dotted_path_suffix(&module.name, path).is_some()
                || contains_module_path_or_descendant(&module.submodules, path)
        })
    }

    // Canonicalization synthesizes every missing parent of a dotted module. The root
    // is always emitted; deeper synthetic parents are emitted when traversal is on.
    if (!path.contains('.') || cfg.traverse_submodules)
        && contains_module_path_or_descendant(modules, path)
    {
        return Some(DefinitionKind::Module);
    }
    modules
        .iter()
        // Flat dotted top-level modules are canonicalized below a synthetic root.
        // With traversal disabled that child namespace is structural input only,
        // matching `validate_modules`/generation rather than pretending its members
        // are available import origins.
        .filter(|module| !module.name.contains('.') || cfg.traverse_submodules)
        .find_map(|module| emitted_definition_kind(module, path, cfg))
}

fn dotted_path_suffix<'a>(path: &'a str, prefix: &str) -> Option<&'a str> {
    if path == prefix {
        Some("")
    } else {
        path.strip_prefix(prefix)?.strip_prefix('.')
    }
}

fn collect_emitted_imports_matching<'a>(
    modules: &'a [crate::schema::ModuleIr],
    path: &str,
    cfg: &crate::Config,
    parent_emitted: bool,
    top_level: bool,
    imports: &mut Vec<&'a crate::schema::ImportIr>,
) {
    for module in modules {
        let emitted =
            parent_emitted && (!top_level || !module.name.contains('.') || cfg.traverse_submodules);
        if !emitted {
            continue;
        }
        imports.extend(module.imports.iter().filter(|import| {
            import.origin != import.target && dotted_path_suffix(path, &import.target).is_some()
        }));
        if cfg.traverse_submodules {
            collect_emitted_imports_matching(
                &module.submodules,
                path,
                cfg,
                emitted,
                false,
                imports,
            );
        }
    }
}

/// Resolve an emitted schema path through zero or more explicit import aliases.
///
/// Returning the final definition kind (rather than merely inspecting the immediate
/// origin) keeps schema validation aligned with generation for chained re-exports.
/// Ambiguous aliases and cycles are rejected before lowering can silently drop them.
fn resolve_emitted_definition_kind_in_schema(
    modules: &[crate::schema::ModuleIr],
    path: &str,
    cfg: &crate::Config,
    resolving: &mut Vec<String>,
    applied_targets: &mut Vec<String>,
) -> Result<Option<DefinitionKind>> {
    if let Some(kind) = emitted_definition_kind_in_schema(modules, path, cfg) {
        return Ok(Some(kind));
    }
    if !cfg.generate_imports {
        return Ok(None);
    }
    const MAX_IMPORT_CHAIN: usize = 256;
    if resolving.len() >= MAX_IMPORT_CHAIN {
        return Err(PyBindgenError::CodegenError(format!(
            "external IR import chain while resolving '{path}' exceeds the supported depth \
             ({MAX_IMPORT_CHAIN})"
        )));
    }
    if let Some(cycle_start) = resolving.iter().position(|candidate| candidate == path) {
        let mut cycle = resolving[cycle_start..].to_vec();
        cycle.push(path.to_owned());
        return Err(PyBindgenError::CodegenError(format!(
            "external IR import cycle while resolving '{path}': {}",
            cycle.join(" -> ")
        )));
    }

    let mut imports = Vec::new();
    collect_emitted_imports_matching(modules, path, cfg, true, true, &mut imports);
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
            let suffix = dotted_path_suffix(path, &import.target)
                .unwrap_or_else(|| unreachable!("collected only matching import prefixes"));
            let rewritten = if suffix.is_empty() {
                import.origin.clone()
            } else {
                format!("{}.{suffix}", import.origin)
            };
            if let Some(cycle_start) = applied_targets
                .iter()
                .position(|target| target == &import.target)
                .filter(|_| rewritten.split('.').count() >= path.split('.').count())
            {
                let mut cycle = resolving[cycle_start..].to_vec();
                cycle.push(path.to_owned());
                return Err(PyBindgenError::CodegenError(format!(
                    "external IR import cycle while resolving '{path}': {}",
                    cycle.join(" -> ")
                )));
            }
            resolving.push(path.to_owned());
            applied_targets.push(import.target.clone());
            let resolved = resolve_emitted_definition_kind_in_schema(
                modules,
                &rewritten,
                cfg,
                resolving,
                applied_targets,
            );
            applied_targets.pop();
            resolving.pop();
            resolved
        }
        _ => Err(PyBindgenError::CodegenError(format!(
            "external IR import target '{path}' is ambiguous: it aliases multiple origins ({})",
            imports
                .iter()
                .map(|import| import.origin.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

/// External schema proven safe to lower into syntax modules.
///
/// Validation is centralized here so every IR entry point observes the same
/// invariants before `Path::from_py` can normalize malformed input.
pub(crate) struct ValidatedSchema(SchemaIr);

impl ValidatedSchema {
    pub(crate) fn into_schema(self) -> SchemaIr {
        self.0
    }
}

pub(crate) fn validate_schema(schema: SchemaIr, cfg: &crate::Config) -> Result<ValidatedSchema> {
    let mut module_paths = rustc_hash::FxHashSet::default();
    let mut definition_paths = rustc_hash::FxHashMap::default();
    let mut class_paths = rustc_hash::FxHashSet::default();
    // Schema-wide annotation mappings are prepared once here and reused by every module
    // scan below; rebuilding them per module made validation quadratic in module count.
    let schema_annotations = crate::syntax::IrSchemaAnnotations::new(&schema.modules);
    validate_modules(
        &schema.modules,
        &schema.modules,
        &schema_annotations,
        None,
        cfg,
        true,
        &mut module_paths,
        &mut definition_paths,
        &mut class_paths,
        0,
    )?;
    let canonical_module_paths = module_paths
        .iter()
        .flat_map(|path| {
            let segments = path.split('.').collect::<Vec<_>>();
            (1..=segments.len()).map(move |length| segments[..length].join("."))
        })
        .collect::<rustc_hash::FxHashSet<_>>();
    let emitted_canonical_module_paths = canonical_module_paths
        .iter()
        .filter(|path| !path.contains('.') || cfg.traverse_submodules)
        .cloned()
        .collect::<rustc_hash::FxHashSet<_>>();
    let definition_is_emitted = |path: &str, kind: &str| match kind {
        "module" => true,
        "class" => cfg.generate_classes,
        "enum" => cfg.generate_enums,
        "function" => cfg.generate_functions,
        "method" => cfg.generate_classes && cfg.generate_functions,
        "property" => {
            let owner = path.rsplit_once('.').map(|(owner, _)| owner);
            cfg.generate_properties
                && owner.is_none_or(|owner| !class_paths.contains(owner) || cfg.generate_classes)
        }
        "type variable" => cfg.generate_type_vars,
        _ => true,
    };
    for canonical_module_path in &emitted_canonical_module_paths {
        if let Some(kind) = definition_paths
            .get(canonical_module_path)
            .filter(|kind| **kind != "module")
            .filter(|kind| definition_is_emitted(canonical_module_path, kind))
        {
            return Err(PyBindgenError::CodegenError(format!(
                "canonical external IR module path '{canonical_module_path}' collides with an authored {kind} at the same Python path"
            )));
        }
    }
    validate_canonical_module_import_collisions(
        &schema.modules,
        &schema.modules,
        cfg,
        &emitted_canonical_module_paths,
        true,
        true,
    )?;
    if cfg.generate_preludes {
        validate_prelude_targets(
            &schema.modules,
            &schema.modules,
            cfg,
            &canonical_module_paths,
            true,
            true,
        )?;
    }

    let mut root_type_symbols = crate::syntax::naming::SymbolTable::new("type");
    if schema_requires_root_support(&schema.modules, &schema_annotations, cfg) {
        for generated_name in [
            crate::syntax::class::ROOT_TRAIT_NAME,
            crate::syntax::class::PY_TYPE_MARKER_NAME,
        ] {
            register_rust_type_symbol(
                &mut root_type_symbols,
                generated_name,
                "<generated root support>",
            )?;
        }
        // The `PyCollection` wrapper is emitted (and thus reserved) ONLY when a
        // collection type is actually used -- mirroring `Codegen::has_any_collection`
        // and the conditional `generate_collection_wrapper` emission. Reserving it for
        // every class-bearing schema would falsely reject a class-only IR that defines a
        // user type literally named `PyCollection`.
        if schema_mentions_collection(&schema.modules, &schema_annotations, cfg) {
            register_rust_type_symbol(
                &mut root_type_symbols,
                crate::syntax::class::COLLECTION_WRAPPER_NAME,
                "<generated root support>",
            )?;
        }
    }
    let mut root_modules = rustc_hash::FxHashSet::default();
    for module in &schema.modules {
        let root = module
            .name
            .split_once('.')
            .map_or(module.name.as_str(), |(root, _)| root);
        if !root_modules.insert(root) {
            continue;
        }
        let python_ident = crate::syntax::Ident::from_py(root);
        let rust_ident = crate::syntax::naming::rust_ident(
            &python_ident,
            crate::syntax::naming::SymbolKind::Module,
        )
        .ok_or_else(|| {
            PyBindgenError::CodegenError(format!(
                "external IR module '{}' cannot be represented as a Rust type symbol",
                root
            ))
        })?;
        register_rust_type_symbol(&mut root_type_symbols, &rust_ident.to_string(), root)?;
    }
    validate_class_bases(&schema.modules, &class_paths)?;
    validate_inheritance(&schema.modules)?;
    Ok(ValidatedSchema(schema))
}

fn validate_canonical_module_import_collisions(
    modules: &[crate::schema::ModuleIr],
    all_modules: &[crate::schema::ModuleIr],
    cfg: &crate::Config,
    canonical_module_paths: &rustc_hash::FxHashSet<String>,
    parent_emitted: bool,
    top_level: bool,
) -> Result<()> {
    for module in modules {
        let emitted =
            parent_emitted && (!top_level || !module.name.contains('.') || cfg.traverse_submodules);
        if !emitted {
            continue;
        }
        if cfg.generate_imports {
            for import in module.imports.iter().filter(|import| {
                import.origin != import.target && canonical_module_paths.contains(&import.target)
            }) {
                if resolve_emitted_definition_kind_in_schema(
                    all_modules,
                    &import.origin,
                    cfg,
                    &mut Vec::new(),
                    &mut Vec::new(),
                )?
                .is_some()
                {
                    return Err(PyBindgenError::CodegenError(format!(
                        "external IR import target '{}' collides with a canonical emitted module path",
                        import.target
                    )));
                }
            }
        }
        validate_canonical_module_import_collisions(
            &module.submodules,
            all_modules,
            cfg,
            canonical_module_paths,
            emitted && cfg.traverse_submodules,
            false,
        )?;
    }
    Ok(())
}

fn function_mentions_collection(
    function: &crate::schema::FunctionIr,
    local_annotation_types: &crate::syntax::IrAnnotationTypes,
) -> bool {
    annotation_mentions_collection(&function.return_type, local_annotation_types)
        || function.params.iter().any(|parameter| {
            annotation_mentions_collection(&parameter.annotation, local_annotation_types)
        })
}

fn property_mentions_collection(
    property: &crate::schema::PropertyIr,
    local_annotation_types: &crate::syntax::IrAnnotationTypes,
) -> bool {
    annotation_mentions_collection(&property.py_type, local_annotation_types)
        || property.setter_type.as_deref().is_some_and(|annotation| {
            annotation_mentions_collection(annotation, local_annotation_types)
        })
}

fn annotation_mentions_collection(
    annotation: &str,
    local_annotation_types: &crate::syntax::IrAnnotationTypes,
) -> bool {
    crate::syntax::Function::parse_ir_annotation(annotation, local_annotation_types)
        .is_ok_and(|annotation| annotation.mentions_collection())
}

/// Walk the emitted module tree looking for what needs generated root-type support.
/// `count_classes` selects the reservation scope: with it set, the mere presence of an
/// emitted class counts (the root trait + type marker back every class); with it clear,
/// only an actually-used collection counts (the `PyCollection` wrapper). Collection use
/// inside a class's own methods/properties always counts regardless.
fn scan_root_support(
    modules: &[crate::schema::ModuleIr],
    schema_annotations: &crate::syntax::IrSchemaAnnotations<'_>,
    cfg: &crate::Config,
    parent_emitted: bool,
    top_level: bool,
    count_classes: bool,
) -> bool {
    modules.iter().any(|module| {
        let emits_namespace = !top_level || !module.name.contains('.') || cfg.traverse_submodules;
        let emitted = parent_emitted && emits_namespace;
        let local_annotation_types = crate::syntax::IrAnnotationTypes::new(
            crate::syntax::Module::ir_annotation_types(module, schema_annotations),
        );
        (emitted
            && ((count_classes && cfg.generate_classes && !module.classes.is_empty())
                || (cfg.generate_functions
                    && module.functions.iter().any(|function| {
                        function_mentions_collection(function, &local_annotation_types)
                    }))
                || (cfg.generate_properties
                    && module.properties.iter().any(|property| {
                        property_mentions_collection(property, &local_annotation_types)
                    }))
                || (cfg.generate_classes
                    && module.classes.iter().any(|class| {
                        (cfg.generate_functions
                            && class.methods.iter().any(|function| {
                                function_mentions_collection(function, &local_annotation_types)
                            }))
                            || (cfg.generate_properties
                                && class.properties.iter().any(|property| {
                                    property_mentions_collection(property, &local_annotation_types)
                                }))
                    }))))
            || scan_root_support(
                &module.submodules,
                schema_annotations,
                cfg,
                emitted && cfg.traverse_submodules,
                false,
                count_classes,
            )
    })
}

/// Whether the crate needs the root trait + type marker: any emitted class or any used
/// collection. Matches `Codegen`'s `has_any_class() || has_any_collection()` gate.
fn schema_requires_root_support(
    modules: &[crate::schema::ModuleIr],
    schema_annotations: &crate::syntax::IrSchemaAnnotations<'_>,
    cfg: &crate::Config,
) -> bool {
    scan_root_support(modules, schema_annotations, cfg, true, true, true)
}

/// Whether the crate actually uses a collection type (so the `PyCollection` wrapper is
/// emitted). Matches `Codegen::has_any_collection`.
fn schema_mentions_collection(
    modules: &[crate::schema::ModuleIr],
    schema_annotations: &crate::syntax::IrSchemaAnnotations<'_>,
    cfg: &crate::Config,
) -> bool {
    scan_root_support(modules, schema_annotations, cfg, true, true, false)
}

#[allow(clippy::too_many_arguments)]
fn validate_modules(
    modules: &[crate::schema::ModuleIr],
    all_modules: &[crate::schema::ModuleIr],
    schema_annotations: &crate::syntax::IrSchemaAnnotations<'_>,
    parent_module: Option<&str>,
    cfg: &crate::Config,
    namespace_emitted: bool,
    module_paths: &mut rustc_hash::FxHashSet<String>,
    definition_paths: &mut rustc_hash::FxHashMap<String, &'static str>,
    class_paths: &mut rustc_hash::FxHashSet<String>,
    depth: usize,
) -> Result<()> {
    // Bound submodule-nesting recursion so a pathologically deep in-memory `SchemaIr` (the
    // JSON path is already capped by serde's recursion limit) fails with a clear error
    // instead of overflowing the stack.
    const MAX_SUBMODULE_DEPTH: usize = 256;
    if depth > MAX_SUBMODULE_DEPTH {
        return Err(PyBindgenError::CodegenError(format!(
            "external IR submodule nesting exceeds the supported depth ({MAX_SUBMODULE_DEPTH})"
        )));
    }
    for module in modules {
        // Flat dotted top-level IR is canonicalized under a synthetic root before
        // generation. With traversal disabled, that authored child is structural
        // input only and contributes no Rust namespace.
        let namespace_emitted = namespace_emitted
            && (parent_module.is_some() || !module.name.contains('.') || cfg.traverse_submodules);
        let mut rust_type_symbols = crate::syntax::naming::SymbolTable::new("type");
        let mut rust_value_symbols = crate::syntax::naming::SymbolTable::new("value");
        validate_dotted_path("module", &module.name)?;
        validate_prelude(&module.name, &module.prelude)?;
        if let Some(parent_module) = parent_module {
            validate_owner("module", &module.name, "module", parent_module)?;
        }
        if !module_paths.insert(module.name.clone()) {
            return Err(PyBindgenError::CodegenError(format!(
                "duplicate module path '{}' in external IR",
                module.name
            )));
        }
        register_definition(definition_paths, &module.name, "module")?;
        let local_annotation_types = crate::syntax::IrAnnotationTypes::new(
            crate::syntax::Module::ir_annotation_types(module, schema_annotations),
        );
        for class in &module.classes {
            let mut rust_value_symbols = crate::syntax::naming::SymbolTable::new("value");
            validate_dotted_path("class", &class.name)?;
            validate_owner("class", &class.name, "module", &module.name)?;
            register_definition(definition_paths, &class.name, "class")?;
            class_paths.insert(class.name.clone());
            if namespace_emitted && cfg.generate_classes {
                let python_ident = crate::syntax::Ident::from_py(
                    class
                        .name
                        .rsplit_once('.')
                        .map_or(&class.name, |(_, name)| name),
                );
                let rust_ident = crate::syntax::naming::rust_ident(
                    &python_ident,
                    crate::syntax::naming::SymbolKind::Class,
                )
                .ok_or_else(|| {
                    PyBindgenError::CodegenError(format!(
                        "external IR class '{}' cannot be represented as a Rust type symbol",
                        class.name
                    ))
                })?;
                register_rust_type_symbol(
                    &mut rust_type_symbols,
                    &rust_ident.to_string(),
                    &class.name,
                )?;
                register_rust_type_symbol(
                    &mut rust_type_symbols,
                    &format!("{rust_ident}Methods"),
                    &class.name,
                )?;
            }
            for method in &class.methods {
                if !matches!(method.kind, crate::schema::FunctionKindIr::Method { .. }) {
                    return Err(PyBindgenError::CodegenError(format!(
                        "external IR class member '{}' must use FunctionKindIr::Method",
                        method.name
                    )));
                }
                let method_path = owned_member_path("method", &method.name, &class.name)?;
                validate_function_annotations(
                    &method_path,
                    &method.params,
                    &method.return_type,
                    &local_annotation_types,
                )?;
                register_definition(definition_paths, &method_path, "method")?;
                let method_name = method_path
                    .rsplit_once('.')
                    .map_or(method_path.as_str(), |(_, name)| name);
                if namespace_emitted
                    && cfg.generate_classes
                    && cfg.generate_functions
                    && !matches!(
                        method.kind,
                        crate::schema::FunctionKindIr::Method {
                            method_type: crate::schema::MethodKindIr::Constructor
                                | crate::schema::MethodKindIr::Callable
                        }
                    )
                {
                    let python_ident = crate::syntax::Ident::from_py(method_name);
                    let rust_ident = crate::syntax::naming::rust_ident(
                        &python_ident,
                        crate::syntax::naming::SymbolKind::Function,
                    )
                    .ok_or_else(|| {
                        PyBindgenError::CodegenError(format!(
                            "external IR method '{method_path}' cannot be represented as a Rust value symbol"
                        ))
                    })?;
                    register_rust_value_symbol(
                        &mut rust_value_symbols,
                        &rust_ident.to_string(),
                        &method_path,
                    )?;
                }
            }
            for property in &class.properties {
                let property_path = owned_member_path("property", &property.name, &class.name)?;
                validate_property_annotations(
                    &property_path,
                    &property.py_type,
                    property.setter_type.as_deref(),
                    &local_annotation_types,
                )?;
                register_definition(definition_paths, &property_path, "property")?;
                if namespace_emitted && cfg.generate_classes && cfg.generate_properties {
                    let property_name = property_path
                        .rsplit_once('.')
                        .map_or(property_path.as_str(), |(_, name)| name);
                    register_rust_value_symbol(
                        &mut rust_value_symbols,
                        property_name,
                        &property_path,
                    )?;
                    if !property.readonly {
                        register_rust_value_symbol(
                            &mut rust_value_symbols,
                            &format!("set_{property_name}"),
                            &property_path,
                        )?;
                    }
                }
            }
        }
        for py_enum in &module.enums {
            validate_dotted_path("enum", &py_enum.name)?;
            validate_owner("enum", &py_enum.name, "module", &module.name)?;
            if !cfg.lenient_external_ir
                && !py_enum.plain_mirror
                && (py_enum.is_flag || py_enum.is_open)
            {
                return Err(PyBindgenError::CodegenError(format!(
                    "external IR enum '{}' requests unsupported enum semantics (is_flag={}, is_open={}); use a raw class wrapper until these semantics are implemented, or set \"plain_mirror\": true on the enum to explicitly accept a plain mirror enum with the flag/open semantics dropped, or enable lenient external IR",
                    py_enum.name, py_enum.is_flag, py_enum.is_open
                )));
            }
            register_definition(definition_paths, &py_enum.name, "enum")?;
            if namespace_emitted && cfg.generate_enums {
                let python_ident = crate::syntax::Ident::from_py(
                    py_enum
                        .name
                        .rsplit_once('.')
                        .map_or(&py_enum.name, |(_, name)| name),
                );
                let rust_ident = crate::syntax::naming::rust_ident(
                    &python_ident,
                    crate::syntax::naming::SymbolKind::Enum,
                )
                .ok_or_else(|| {
                    PyBindgenError::CodegenError(format!(
                        "external IR enum '{}' cannot be represented as a Rust type symbol",
                        py_enum.name
                    ))
                })?;
                register_rust_type_symbol(
                    &mut rust_type_symbols,
                    &rust_ident.to_string(),
                    &py_enum.name,
                )?;
            }
        }
        for function in &module.functions {
            if matches!(function.kind, crate::schema::FunctionKindIr::Method { .. }) {
                return Err(PyBindgenError::CodegenError(format!(
                    "external IR module function '{}' cannot use FunctionKindIr::Method",
                    function.name
                )));
            }
            validate_dotted_path("function", &function.name)?;
            validate_owner("function", &function.name, "module", &module.name)?;
            validate_function_annotations(
                &function.name,
                &function.params,
                &function.return_type,
                &local_annotation_types,
            )?;
            register_definition(definition_paths, &function.name, "function")?;
            if namespace_emitted && cfg.generate_functions {
                let python_ident = crate::syntax::Ident::from_py(
                    function
                        .name
                        .rsplit_once('.')
                        .map_or(&function.name, |(_, name)| name),
                );
                let rust_ident = crate::syntax::naming::rust_ident(
                    &python_ident,
                    crate::syntax::naming::SymbolKind::Function,
                )
                .ok_or_else(|| {
                    PyBindgenError::CodegenError(format!(
                        "external IR function '{}' cannot be represented as a Rust value symbol",
                        function.name
                    ))
                })?;
                register_rust_value_symbol(
                    &mut rust_value_symbols,
                    &rust_ident.to_string(),
                    &function.name,
                )?;
            }
        }
        for property in &module.properties {
            validate_dotted_path("property", &property.name)?;
            validate_owner("property", &property.name, "module", &module.name)?;
            validate_property_annotations(
                &property.name,
                &property.py_type,
                property.setter_type.as_deref(),
                &local_annotation_types,
            )?;
            register_definition(definition_paths, &property.name, "property")?;
            if namespace_emitted && cfg.generate_properties {
                let property_name = property
                    .name
                    .rsplit_once('.')
                    .map_or(property.name.as_str(), |(_, name)| name);
                register_rust_value_symbol(&mut rust_value_symbols, property_name, &property.name)?;
                if !property.readonly {
                    register_rust_value_symbol(
                        &mut rust_value_symbols,
                        &format!("set_{property_name}"),
                        &property.name,
                    )?;
                }
            }
        }
        for type_var in &module.type_vars {
            validate_dotted_path("type variable", type_var)?;
            validate_owner("type variable", type_var, "module", &module.name)?;
            register_definition(definition_paths, type_var, "type variable")?;
            if namespace_emitted && cfg.generate_type_vars {
                let python_ident = crate::syntax::Ident::from_py(
                    type_var
                        .rsplit_once('.')
                        .map_or(type_var.as_str(), |(_, name)| name),
                );
                let rust_ident = crate::syntax::naming::rust_ident(
                    &python_ident,
                    crate::syntax::naming::SymbolKind::TypeVar,
                )
                .ok_or_else(|| {
                    PyBindgenError::CodegenError(format!(
                        "external IR type variable '{type_var}' cannot be represented as a Rust type symbol"
                    ))
                })?;
                register_rust_type_symbol(
                    &mut rust_type_symbols,
                    &rust_ident.to_string(),
                    type_var,
                )?;
            }
        }
        for import in &module.imports {
            validate_dotted_path("import origin", &import.origin)?;
            validate_dotted_path("import target", &import.target)?;
            validate_owner("import target", &import.target, "module", &module.name)?;
            let parsed = crate::syntax::Import::new(
                crate::syntax::Path::from_py(&import.origin),
                crate::syntax::Path::from_py(&import.target),
            );
            if namespace_emitted && cfg.generate_imports && parsed.is_nonidentity() {
                if let Some(kind) = resolve_emitted_definition_kind_in_schema(
                    all_modules,
                    &import.origin,
                    cfg,
                    &mut Vec::new(),
                    &mut Vec::new(),
                )? {
                    let alias =
                        crate::syntax::naming::rust_ident(parsed.target.name(), kind.symbol_kind())
                            .ok_or_else(|| {
                                PyBindgenError::CodegenError(format!(
                            "external IR import target '{}' cannot be represented as a Rust symbol",
                            import.target
                        ))
                            })?;
                    let origin = format!("import '{}' as '{}'", import.origin, import.target);
                    if kind.is_value() {
                        register_rust_value_symbol(
                            &mut rust_value_symbols,
                            &alias.to_string(),
                            &origin,
                        )?;
                    } else {
                        register_rust_type_symbol(
                            &mut rust_type_symbols,
                            &alias.to_string(),
                            &origin,
                        )?;
                    }
                }
            }
        }
        if namespace_emitted && cfg.traverse_submodules {
            for submodule in &module.submodules {
                // Validate the raw submodule name before lowering it to an `Ident`,
                // exactly as every sibling item kind above does. The recursive
                // `validate_modules` call below re-checks it, but only after this loop
                // has already constructed an `Ident` from it -- and `Ident::from_py`
                // asserts non-emptiness, so an empty submodule name would abort here
                // instead of returning this clean error.
                validate_dotted_path("module", &submodule.name)?;
                let python_ident = crate::syntax::Ident::from_py(
                    submodule
                        .name
                        .rsplit_once('.')
                        .map_or(&submodule.name, |(_, name)| name),
                );
                let rust_ident = crate::syntax::naming::rust_ident(
                    &python_ident,
                    crate::syntax::naming::SymbolKind::Module,
                )
                .ok_or_else(|| {
                    PyBindgenError::CodegenError(format!(
                        "external IR module '{}' cannot be represented as a Rust type symbol",
                        submodule.name
                    ))
                })?;
                register_rust_type_symbol(
                    &mut rust_type_symbols,
                    &rust_ident.to_string(),
                    &submodule.name,
                )?;
            }
        }
        validate_modules(
            &module.submodules,
            all_modules,
            schema_annotations,
            Some(&module.name),
            cfg,
            namespace_emitted && cfg.traverse_submodules,
            module_paths,
            definition_paths,
            class_paths,
            depth + 1,
        )?;
    }
    Ok(())
}

fn validate_prelude(module: &str, prelude: &[String]) -> Result<()> {
    let mut names = rustc_hash::FxHashSet::default();
    for name in prelude {
        if name.is_empty() || name.contains('.') {
            return Err(PyBindgenError::CodegenError(format!(
                "external IR prelude name '{name}' in module '{module}' must be a non-empty identifier, not a dotted path"
            )));
        }
        if !names.insert(name) {
            return Err(PyBindgenError::CodegenError(format!(
                "duplicate external IR prelude name '{name}' in module '{module}'"
            )));
        }
    }
    Ok(())
}

fn validate_prelude_targets(
    modules: &[crate::schema::ModuleIr],
    all_modules: &[crate::schema::ModuleIr],
    cfg: &crate::Config,
    canonical_module_paths: &rustc_hash::FxHashSet<String>,
    parent_emitted: bool,
    top_level: bool,
) -> Result<()> {
    use crate::schema::FunctionKindIr;

    for module in modules {
        let emitted =
            parent_emitted && (!top_level || !module.name.contains('.') || cfg.traverse_submodules);
        if !emitted {
            continue;
        }
        for name in &module.prelude {
            let direct_child = format!("{}.{name}", module.name);
            let has_module = canonical_module_paths.contains(&direct_child);
            let has_class = module.classes.iter().any(|item| {
                item.name
                    .rsplit_once('.')
                    .map_or(item.name.as_str(), |(_, name)| name)
                    == name
            });
            let has_enum = module.enums.iter().any(|item| {
                item.name
                    .rsplit_once('.')
                    .map_or(item.name.as_str(), |(_, name)| name)
                    == name
            });
            let has_function = module.functions.iter().any(|item| {
                !matches!(item.kind, FunctionKindIr::Method { .. })
                    && item
                        .name
                        .rsplit_once('.')
                        .map_or(item.name.as_str(), |(_, name)| name)
                        == name
            });
            let has_property = module.properties.iter().any(|item| {
                item.name
                    .rsplit_once('.')
                    .map_or(item.name.as_str(), |(_, name)| name)
                    == name
            });
            let has_type_var = module.type_vars.iter().any(|item| {
                item.rsplit_once('.')
                    .map_or(item.as_str(), |(_, name)| name)
                    == name
            });
            let matching_imports = module
                .imports
                .iter()
                .filter(|import| import.target == direct_child)
                .collect::<Vec<_>>();

            // Prefer enabled declarations. A disabled declaration does not occupy the
            // emitted namespace, so an enabled import alias with the same Python target
            // may legitimately supply the prelude item instead.
            let target = if has_module && cfg.traverse_submodules {
                Some(("module", true))
            } else if has_class && cfg.generate_classes {
                Some(("class", true))
            } else if has_enum && cfg.generate_enums {
                Some(("enum", true))
            } else if has_function && cfg.generate_functions {
                Some(("function", true))
            } else if has_property && cfg.generate_properties {
                Some(("property", true))
            } else if has_type_var && cfg.generate_type_vars {
                Some(("type variable", true))
            } else if !matching_imports.is_empty() {
                if !cfg.generate_imports {
                    Some(("import", false))
                } else if matching_imports
                    .iter()
                    .all(|import| import.origin == import.target)
                {
                    return Err(PyBindgenError::CodegenError(format!(
                        "external IR prelude target '{}.{name}' is an identity import and emits no Rust item",
                        module.name
                    )));
                } else if resolve_emitted_definition_kind_in_schema(
                    all_modules,
                    &direct_child,
                    cfg,
                    &mut Vec::new(),
                    &mut Vec::new(),
                )?
                .is_none()
                {
                    let origin = matching_imports
                        .iter()
                        .find(|import| import.origin != import.target)
                        .map_or("<identity import>", |import| import.origin.as_str());
                    return Err(PyBindgenError::CodegenError(format!(
                        "external IR prelude target '{}.{name}' imports unresolved or disabled origin '{}'",
                        module.name, origin
                    )));
                } else {
                    Some(("import", true))
                }
            } else if has_module {
                Some(("module", false))
            } else if has_class {
                Some(("class", false))
            } else if has_enum {
                Some(("enum", false))
            } else if has_function {
                Some(("function", false))
            } else if has_property {
                Some(("property", false))
            } else if has_type_var {
                Some(("type variable", false))
            } else {
                None
            };

            let Some((kind, enabled)) = target else {
                return Err(PyBindgenError::CodegenError(format!(
                    "external IR prelude target '{}.{name}' does not resolve to a declared module member",
                    module.name
                )));
            };
            if !enabled {
                return Err(PyBindgenError::CodegenError(format!(
                    "external IR prelude target '{}.{name}' is a {kind} disabled by generation configuration",
                    module.name
                )));
            }
        }
        validate_prelude_targets(
            &module.submodules,
            all_modules,
            cfg,
            canonical_module_paths,
            emitted && cfg.traverse_submodules,
            false,
        )?;
    }

    Ok(())
}

fn validate_function_parameters(
    function: &str,
    parameters: &[crate::schema::ParameterIr],
) -> Result<()> {
    use crate::schema::ParameterKindIr;

    let mut names = rustc_hash::FxHashSet::default();
    let mut rust_names = rustc_hash::FxHashSet::default();
    let mut previous_rank = None;
    let mut saw_var_positional = false;
    let mut saw_var_keyword = false;

    for parameter in parameters {
        if parameter.name.is_empty() {
            return Err(PyBindgenError::CodegenError(format!(
                "external IR function '{function}' parameter name must be non-empty"
            )));
        }
        if !names.insert(&parameter.name) {
            return Err(PyBindgenError::CodegenError(format!(
                "external IR function '{function}' has duplicate parameter '{}'",
                parameter.name
            )));
        }

        let generated_name = crate::syntax::Ident::from_py(&format!("p_{}", parameter.name));
        let generated_ident: syn::Ident = (&generated_name).try_into().map_err(|_| {
            PyBindgenError::CodegenError(format!(
                "external IR function '{function}' parameter '{}' cannot be represented as a Rust identifier",
                parameter.name
            ))
        })?;
        let canonical_name =
            crate::syntax::naming::canonical_rust_name(&generated_ident.to_string());
        if !rust_names.insert(canonical_name.clone()) {
            return Err(PyBindgenError::CodegenError(format!(
                "external IR function '{function}' has colliding Rust parameter name '{canonical_name}'"
            )));
        }

        let rank = match parameter.kind {
            ParameterKindIr::PositionalOnly => 0,
            ParameterKindIr::PositionalOrKeyword => 1,
            ParameterKindIr::VarPositional => 2,
            ParameterKindIr::KeywordOnly => 3,
            ParameterKindIr::VarKeyword => 4,
        };
        if previous_rank.is_some_and(|previous| rank < previous) {
            return Err(PyBindgenError::CodegenError(format!(
                "external IR function '{function}' has invalid parameter kind order at '{}'",
                parameter.name
            )));
        }
        previous_rank = Some(rank);

        match parameter.kind {
            ParameterKindIr::VarPositional => {
                if std::mem::replace(&mut saw_var_positional, true) {
                    return Err(PyBindgenError::CodegenError(format!(
                        "external IR function '{function}' has multiple variadic positional parameters"
                    )));
                }
                if parameter.has_default {
                    return Err(PyBindgenError::CodegenError(format!(
                        "external IR function '{function}' variadic parameter '{}' cannot have a default",
                        parameter.name
                    )));
                }
            }
            ParameterKindIr::VarKeyword => {
                if std::mem::replace(&mut saw_var_keyword, true) {
                    return Err(PyBindgenError::CodegenError(format!(
                        "external IR function '{function}' has multiple variadic keyword parameters"
                    )));
                }
                if parameter.has_default {
                    return Err(PyBindgenError::CodegenError(format!(
                        "external IR function '{function}' variadic parameter '{}' cannot have a default",
                        parameter.name
                    )));
                }
            }
            // A required positional after a defaulted one is Python-invalid, but a
            // default never reaches the emitted Rust signature (`has_default` is
            // metadata only), so the lowering is order-independent -- both strict and
            // lenient IR accept it and emit the same code the pinned generator did.
            ParameterKindIr::PositionalOnly | ParameterKindIr::PositionalOrKeyword => {}
            ParameterKindIr::KeywordOnly => {}
        }
    }

    Ok(())
}

fn validate_function_annotations(
    function: &str,
    parameters: &[crate::schema::ParameterIr],
    return_type: &str,
    local_annotation_types: &crate::syntax::IrAnnotationTypes,
) -> Result<()> {
    validate_function_parameters(function, parameters)?;
    for parameter in parameters {
        validate_external_callable_annotations(
            &format!(
                "external IR function '{function}' parameter '{}' annotation",
                parameter.name
            ),
            &parameter.annotation,
            local_annotation_types,
        )?;
    }
    validate_external_callable_annotations(
        &format!("external IR function '{function}' return annotation"),
        return_type,
        local_annotation_types,
    )
}

fn validate_property_annotations(
    property: &str,
    getter_type: &str,
    setter_type: Option<&str>,
    local_annotation_types: &crate::syntax::IrAnnotationTypes,
) -> Result<()> {
    validate_external_callable_annotations(
        &format!("external IR property '{property}' getter annotation"),
        getter_type,
        local_annotation_types,
    )?;
    if let Some(setter_type) = setter_type {
        validate_external_callable_annotations(
            &format!("external IR property '{property}' setter annotation"),
            setter_type,
            local_annotation_types,
        )?;
    }
    Ok(())
}

const CALLABLE_FORM: &str = "expected `Callable[[...], result]` or `Callable[..., result]`";

fn validate_external_callable_annotations(
    location: &str,
    annotation: &str,
    local_annotation_types: &crate::syntax::IrAnnotationTypes,
) -> Result<()> {
    let bytes = annotation.as_bytes();
    let mut index = 0;
    let mut quote = None;
    let mut escaped = false;
    while index < bytes.len() {
        if let Some(quote_character) = quote {
            if escaped {
                escaped = false;
            } else if bytes[index] == b'\\' {
                escaped = true;
            } else if bytes[index] == quote_character {
                quote = None;
            }
            index += 1;
            continue;
        }

        if matches!(bytes[index], b'\'' | b'"') {
            quote = Some(bytes[index]);
            index += 1;
            continue;
        }

        const CALLABLE: &[u8] = b"Callable";
        if bytes[index..].starts_with(CALLABLE) {
            let end = index + CALLABLE.len();
            // A non-ASCII byte may be part of a valid Python identifier. Treat it as
            // an identifier continuation here: falsely accepting an exotic spelling
            // preserves the established dynamic fallback, while treating the ASCII
            // suffix of `ÉCallable` as the standard `Callable` would reject valid
            // authored IR.
            let boundary = |character: u8| {
                character.is_ascii() && !(character.is_ascii_alphanumeric() || character == b'_')
            };
            if (index == 0 || boundary(bytes[index - 1]))
                && (end == bytes.len() || boundary(bytes[end]))
            {
                let mut path_start = index;
                while path_start > 0 {
                    let character = bytes[path_start - 1];
                    if character.is_ascii_alphanumeric() || matches!(character, b'_' | b'.') {
                        path_start -= 1;
                    } else {
                        break;
                    }
                }
                let path = &annotation[path_start..end];
                let mut open = end;
                while open < bytes.len() && bytes[open].is_ascii_whitespace() {
                    open += 1;
                }
                if open < bytes.len()
                    && bytes[open] == b'['
                    && is_standard_callable_path(path, local_annotation_types)
                {
                    if let Some(arguments) = callable_arguments(annotation, open) {
                        if !valid_callable_arguments(arguments) {
                            return Err(PyBindgenError::CodegenError(format!(
                                "{location} {annotation:?} has malformed Callable; {CALLABLE_FORM}"
                            )));
                        }
                    }
                }
            }
            index = end;
            continue;
        }

        index += 1;
    }
    Ok(())
}

fn is_standard_callable_path(
    path: &str,
    local_annotation_types: &crate::syntax::IrAnnotationTypes,
) -> bool {
    let path = local_annotation_types
        .get(path)
        .map_or(path, String::as_str);
    matches!(
        path,
        "Callable"
            | "builtins.Callable"
            | "collections.Callable"
            | "collections.abc.Callable"
            | "typing.Callable"
            | "typing_extensions.Callable"
    )
}

fn callable_arguments(annotation: &str, open: usize) -> Option<&str> {
    let bytes = annotation.as_bytes();
    let mut depth = 0_usize;
    let mut quote = None;
    let mut escaped = false;
    for index in open..bytes.len() {
        let character = bytes[index];
        if let Some(quote_character) = quote {
            if escaped {
                escaped = false;
            } else if character == b'\\' {
                escaped = true;
            } else if character == quote_character {
                quote = None;
            }
            continue;
        }

        match character {
            b'\'' | b'"' => quote = Some(character),
            b'[' => depth += 1,
            b']' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(&annotation[open + 1..index]);
                }
            }
            _ => {}
        }
    }
    None
}

fn valid_callable_arguments(arguments: &str) -> bool {
    let mut arguments = split_annotation_arguments(arguments);
    if arguments
        .last()
        .is_some_and(|argument| argument.trim().is_empty())
    {
        arguments.pop();
    }
    if arguments.len() != 2 {
        return false;
    }
    let parameters = arguments[0].trim();
    let result = arguments[1].trim();
    if parameters.is_empty() || result.is_empty() || result == "..." {
        return false;
    }
    if parameters == "..." || !parameters.starts_with('[') {
        // CPython also accepts the shorthand `Callable[param, result]`.
        return true;
    }
    if !parameters.ends_with(']') {
        return false;
    }

    let parameters = parameters[1..parameters.len() - 1].trim();
    if parameters.is_empty() {
        return true;
    }
    let mut parameters = split_annotation_arguments(parameters);
    if parameters
        .last()
        .is_some_and(|parameter| parameter.trim().is_empty())
    {
        parameters.pop();
    }
    !parameters.is_empty()
        && parameters
            .iter()
            .all(|parameter| !parameter.trim().is_empty())
}

fn split_annotation_arguments(arguments: &str) -> Vec<&str> {
    let bytes = arguments.as_bytes();
    let mut parts = Vec::new();
    let mut start = 0;
    let mut depth = 0_usize;
    let mut quote = None;
    let mut escaped = false;
    for (index, &character) in bytes.iter().enumerate() {
        if let Some(quote_character) = quote {
            if escaped {
                escaped = false;
            } else if character == b'\\' {
                escaped = true;
            } else if character == quote_character {
                quote = None;
            }
            continue;
        }

        match character {
            b'\'' | b'"' => quote = Some(character),
            b'[' | b'(' | b'{' => depth += 1,
            b']' | b')' | b'}' => depth = depth.saturating_sub(1),
            b',' if depth == 0 => {
                parts.push(&arguments[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    parts.push(&arguments[start..]);
    parts
}

fn owned_member_path(kind: &str, path: &str, owner: &str) -> Result<String> {
    validate_dotted_path(kind, path)?;
    if path.contains('.') {
        validate_owner(kind, path, "class", owner)?;
        Ok(path.to_owned())
    } else {
        Ok(format!("{owner}.{path}"))
    }
}

fn register_rust_type_symbol(
    symbols: &mut crate::syntax::naming::SymbolTable,
    rust_ident: &str,
    python_path: &str,
) -> Result<()> {
    symbols.register(rust_ident, python_path)
}

fn register_rust_value_symbol(
    symbols: &mut crate::syntax::naming::SymbolTable,
    rust_ident: &str,
    python_path: &str,
) -> Result<()> {
    symbols.register(rust_ident, python_path)
}

fn validate_class_bases(
    modules: &[crate::schema::ModuleIr],
    class_paths: &rustc_hash::FxHashSet<String>,
) -> Result<()> {
    for module in modules {
        for class in &module.classes {
            let mut direct_bases = rustc_hash::FxHashSet::default();
            for base in &class.base_classes {
                validate_dotted_path("class base", base)?;
                if !class_paths.contains(base) {
                    return Err(PyBindgenError::CodegenError(format!(
                        "base '{base}' of class '{}' does not resolve to a class in external IR",
                        class.name
                    )));
                }
                if !direct_bases.insert(base) {
                    return Err(PyBindgenError::CodegenError(format!(
                        "class '{}' lists duplicate direct base '{base}'; Python rejects duplicate base classes",
                        class.name
                    )));
                }
            }
        }
        validate_class_bases(&module.submodules, class_paths)?;
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum VisitState {
    Visiting,
    Complete,
}

fn validate_inheritance(modules: &[crate::schema::ModuleIr]) -> Result<()> {
    fn collect(
        modules: &[crate::schema::ModuleIr],
        graph: &mut rustc_hash::FxHashMap<String, Vec<String>>,
    ) {
        for module in modules {
            for class in &module.classes {
                graph.insert(class.name.clone(), class.base_classes.clone());
            }
            collect(&module.submodules, graph);
        }
    }

    let mut graph = rustc_hash::FxHashMap::default();
    collect(modules, &mut graph);
    let mut states = rustc_hash::FxHashMap::default();
    let mut postorder = Vec::with_capacity(graph.len());

    // Explicit DFS avoids overflowing the process stack on valid, deeply chained
    // schemas. Each frame stores the next base edge to inspect.
    //
    // Roots are visited in sorted order, not hash order: a schema containing more than one cycle
    // reports whichever cycle is reached first, and `FxHashMap` iteration follows hash values that
    // differ between 32- and 64-bit targets. Sorting makes the reported cycle, and its starting
    // node, identical on every platform.
    let mut roots = graph.keys().cloned().collect::<Vec<_>>();
    roots.sort();
    for class in &roots {
        if states.get(class) == Some(&VisitState::Complete) {
            continue;
        }
        states.insert(class.clone(), VisitState::Visiting);
        let mut stack = vec![(class.clone(), 0_usize)];
        while let Some((node, next_base)) = stack.last_mut() {
            let bases = graph.get(node).map(Vec::as_slice).unwrap_or_default();
            if let Some(base) = bases.get(*next_base) {
                *next_base += 1;
                match states.get(base) {
                    Some(VisitState::Complete) => {}
                    Some(VisitState::Visiting) => {
                        let cycle_start =
                            stack.iter().position(|(item, _)| item == base).unwrap_or(0);
                        let mut cycle = stack[cycle_start..]
                            .iter()
                            .map(|(item, _)| item.clone())
                            .collect::<Vec<_>>();
                        cycle.push(base.clone());
                        return Err(PyBindgenError::CodegenError(format!(
                            "external IR inheritance cycle: {}",
                            cycle.join(" -> ")
                        )));
                    }
                    None => {
                        states.insert(base.clone(), VisitState::Visiting);
                        stack.push((base.clone(), 0));
                    }
                }
            } else {
                let (node, _) = stack.pop().expect("stack is non-empty");
                states.insert(node.clone(), VisitState::Complete);
                postorder.push(node);
            }
        }
    }

    // Pure single inheritance is always C3-consistent. Avoid materializing every
    // ancestor closure here: a deep chain would otherwise turn linear-time validation
    // into quadratic memory before generation has even been requested.
    if graph.values().all(|bases| bases.len() <= 1) {
        return Ok(());
    }

    // Validate Python's C3 merge while base linearizations are available in
    // dependency-first postorder. Acyclic multiple inheritance can still be invalid
    // (for example A(X,Y), B(Y,X), C(A,B)); CPython rejects such a class definition.
    let mut linearizations: rustc_hash::FxHashMap<String, Vec<String>> =
        rustc_hash::FxHashMap::default();
    for class in postorder {
        let bases = graph.get(&class).cloned().unwrap_or_default();
        let mut sequences = bases
            .iter()
            .map(|base| {
                linearizations
                    .get(base)
                    .cloned()
                    .expect("postorder places bases before derived classes")
            })
            .collect::<Vec<_>>();
        if !bases.is_empty() {
            sequences.push(bases);
        }

        let mut linearization = vec![class.clone()];
        loop {
            sequences.retain(|sequence| !sequence.is_empty());
            if sequences.is_empty() {
                break;
            }
            let candidate = sequences
                .iter()
                .map(|sequence| &sequence[0])
                .find(|candidate| {
                    !sequences
                        .iter()
                        .any(|sequence| sequence[1..].contains(candidate))
                })
                .cloned()
                .ok_or_else(|| {
                    PyBindgenError::CodegenError(format!(
                        "external IR class hierarchy for '{class}' has no consistent Python C3 method resolution order"
                    ))
                })?;
            for sequence in &mut sequences {
                if sequence.first() == Some(&candidate) {
                    sequence.remove(0);
                }
            }
            linearization.push(candidate);
        }
        linearizations.insert(class, linearization);
    }
    Ok(())
}

fn validate_owner(kind: &str, path: &str, owner_kind: &str, expected_owner: &str) -> Result<()> {
    let actual_owner = path.rsplit_once('.').map(|(owner, _)| owner);
    if actual_owner != Some(expected_owner) {
        return Err(PyBindgenError::CodegenError(format!(
            "external IR {kind} path '{path}' must be owned by {owner_kind} '{expected_owner}'"
        )));
    }
    Ok(())
}

fn register_definition(
    definitions: &mut rustc_hash::FxHashMap<String, &'static str>,
    path: &str,
    kind: &'static str,
) -> Result<()> {
    if let Some(previous_kind) = definitions.insert(path.to_owned(), kind) {
        return Err(PyBindgenError::CodegenError(format!(
            "duplicate definition path '{path}' in external IR ({previous_kind} and {kind})"
        )));
    }
    Ok(())
}

fn validate_dotted_path(kind: &str, path: &str) -> Result<()> {
    if path.is_empty()
        || path.contains('\0')
        || path.contains("::")
        || path.split('.').any(str::is_empty)
    {
        return Err(PyBindgenError::CodegenError(format!(
            "external IR {kind} path {path:?} must be a non-empty dotted Python path with no empty segments, NUL bytes, or Rust `::` separators"
        )));
    }
    Ok(())
}
