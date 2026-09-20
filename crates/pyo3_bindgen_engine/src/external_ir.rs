//! External-IR validation and lowering seam.
//!
//! Serialized [`SchemaIr`] values stop at this module. The rest of generation consumes
//! the private [`Module`] domain model, so wire compatibility can evolve without leaking
//! serde concerns into orchestration and emission.

use crate::{schema::SchemaIr, schema_validation::validate_schema, syntax::Module, Config, Result};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

/// Private acquisition state for incremental external-IR registration.
///
/// Registration deliberately keeps only the raw accumulated schema. A later document can add a
/// type beneath a standard-looking root and thereby change import resolution for an earlier
/// module, so validation and lowering belong to the terminal generation boundary.
#[derive(Debug, Clone)]
pub(crate) struct ExternalIrState {
    schema: SchemaIr,
    registration_count: usize,
}

/// The one terminal validation/lowering result for an accumulated external schema.
#[derive(Debug)]
pub(crate) struct ExternalIrFinish {
    pub(crate) modules: Vec<Module>,
    pub(crate) registration_count: usize,
    pub(crate) validation_lowering_passes: usize,
}

impl ExternalIrState {
    /// Merge one raw schema into the pending external graph without validation or lowering.
    pub(crate) fn register(existing: Option<Self>, incoming: SchemaIr) -> Self {
        let (mut schema, registration_count) = existing.map_or_else(
            || {
                (
                    SchemaIr {
                        modules: Vec::new(),
                    },
                    0,
                )
            },
            |state| (state.schema, state.registration_count),
        );
        schema.modules.extend(incoming.modules);

        Self {
            schema,
            registration_count: registration_count + 1,
        }
    }

    pub(crate) fn pending_module_count(&self) -> usize {
        self.schema.modules.len()
    }

    pub(crate) fn registration_count(&self) -> usize {
        self.registration_count
    }

    pub(crate) fn resources(&self) -> Result<crate::resource::StructuralResources> {
        fn annotation_shape(annotation: &str) -> (usize, usize) {
            let mut nodes = 1;
            let mut depth = 1usize;
            let mut max_depth = 1;
            for character in annotation.chars() {
                match character {
                    '[' | '(' => {
                        nodes += 1;
                        depth += 1;
                        max_depth = max_depth.max(depth);
                    }
                    ']' | ')' => depth = depth.saturating_sub(1).max(1),
                    _ => {}
                }
            }
            (nodes, max_depth)
        }

        fn function_shape(function: &crate::schema::FunctionIr) -> (usize, usize) {
            let (return_nodes, mut max_depth) = annotation_shape(&function.return_type);
            let mut nodes = 1 + return_nodes;
            for parameter in &function.params {
                let (annotation_nodes, depth) = annotation_shape(&parameter.annotation);
                nodes += 1 + annotation_nodes;
                max_depth = max_depth.max(depth);
            }
            (nodes, max_depth)
        }

        fn property_shape(property: &crate::schema::PropertyIr) -> (usize, usize) {
            let (getter_nodes, getter_depth) = annotation_shape(&property.py_type);
            let (setter_nodes, setter_depth) = property
                .setter_type
                .as_deref()
                .map_or((0, 1), annotation_shape);
            (
                1 + getter_nodes + setter_nodes,
                getter_depth.max(setter_depth),
            )
        }

        fn add_member_resources(
            resources: &mut crate::resource::StructuralResources,
            module_depth: usize,
            shape: (usize, usize),
        ) {
            let (nodes, depth) = shape;
            resources.definitions += 1;
            resources.model_nodes += nodes;
            resources.recursion_depth = resources.recursion_depth.max(module_depth + depth);
        }

        fn inheritance_depth(classes: &HashMap<&str, &[String]>) -> usize {
            let mut depths = HashMap::<&str, usize>::default();
            for root in classes.keys().copied() {
                if depths.contains_key(root) {
                    continue;
                }
                let mut visiting = HashSet::default();
                let mut pending = vec![(root, false)];
                while let Some((name, expanded)) = pending.pop() {
                    if depths.contains_key(name) {
                        continue;
                    }
                    if expanded {
                        let depth = 1 + classes[name]
                            .iter()
                            .filter_map(|base| depths.get(base.as_str()).copied())
                            .max()
                            .unwrap_or(0);
                        depths.insert(name, depth);
                        visiting.remove(name);
                        continue;
                    }
                    if !visiting.insert(name) {
                        continue;
                    }
                    pending.push((name, true));
                    for base in classes[name].iter().rev() {
                        let base = base.as_str();
                        if classes.contains_key(base)
                            && !depths.contains_key(base)
                            && !visiting.contains(base)
                        {
                            pending.push((base, false));
                        }
                    }
                }
            }
            depths.values().copied().max().unwrap_or(1)
        }

        let mut resources = crate::resource::StructuralResources {
            recursion_depth: 1,
            model_nodes: 1,
            ..crate::resource::StructuralResources::default()
        };
        let mut classes = HashMap::default();
        let mut pending = self
            .schema
            .modules
            .iter()
            .map(|module| (module, 1usize))
            .collect::<Vec<_>>();
        while let Some((module, module_depth)) = pending.pop() {
            resources.definitions += 1 + module.type_vars.len();
            resources.model_nodes +=
                1 + module.prelude.len() + module.imports.len() + module.type_vars.len();
            // Validation precedes canonicalization, which expands every dotted module name into
            // one path segment per synthetic parent. Count that future depth without building the
            // recursive canonical tree so an over-depth raw name fails in the validation phase.
            let dotted_module_depth = module
                .name
                .chars()
                .filter(|&character| character == '.')
                .count()
                + 1;
            let canonical_module_depth = module_depth.max(dotted_module_depth);
            resources.recursion_depth = resources.recursion_depth.max(canonical_module_depth + 1);

            for item in &module.enums {
                let nodes = 1 + item.variants.len() + item.aliases.len();
                resources.definitions += nodes;
                resources.model_nodes += nodes;
            }
            for item in &module.classes {
                resources.definitions += 1;
                resources.model_nodes += 1 + item.base_classes.len();
                classes.insert(item.name.as_str(), item.base_classes.as_slice());
                for method in &item.methods {
                    add_member_resources(
                        &mut resources,
                        canonical_module_depth,
                        function_shape(method),
                    );
                }
                for property in &item.properties {
                    add_member_resources(
                        &mut resources,
                        canonical_module_depth,
                        property_shape(property),
                    );
                }
            }
            for function in &module.functions {
                add_member_resources(
                    &mut resources,
                    canonical_module_depth,
                    function_shape(function),
                );
            }
            for property in &module.properties {
                add_member_resources(
                    &mut resources,
                    canonical_module_depth,
                    property_shape(property),
                );
            }
            pending.extend(
                module
                    .submodules
                    .iter()
                    .map(|submodule| (submodule, module_depth + 1)),
            );
        }
        resources.recursion_depth = resources.recursion_depth.max(inheritance_depth(&classes));
        Ok(resources)
    }

    /// Validate and lower the complete accumulated graph exactly once at terminal generation.
    pub(crate) fn finish(self, cfg: &Config) -> Result<ExternalIrFinish> {
        let schema = validate_schema(self.schema, cfg)?.into_schema();

        // Prepared once for the whole schema: the mappings it holds are schema-wide, and
        // rebuilding them per module made lowering quadratic in module count.
        let schema_annotations = crate::syntax::IrSchemaAnnotations::new(&schema.modules);
        let modules = schema
            .modules
            .iter()
            .map(|module_ir| Module::from_ir_in_schema(cfg, module_ir, &schema_annotations))
            .collect::<Result<Vec<_>>>()?;

        Ok(ExternalIrFinish {
            modules,
            registration_count: self.registration_count,
            validation_lowering_passes: 1,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn module(name: &str) -> crate::schema::ModuleIr {
        crate::schema::ModuleIr {
            name: name.to_owned(),
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
        }
    }

    #[test]
    fn raw_registrations_finish_with_one_validation_and_lowering_pass() {
        let state = ExternalIrState::register(
            None,
            SchemaIr {
                modules: vec![module("first")],
            },
        );
        let state = ExternalIrState::register(
            Some(state),
            SchemaIr {
                modules: vec![module("second")],
            },
        );

        assert_eq!(state.registration_count(), 2);
        assert_eq!(state.pending_module_count(), 2);

        let finish = state
            .finish(&Config::default())
            .expect("the accumulated schema must finish");
        assert_eq!(finish.registration_count, 2);
        assert_eq!(finish.validation_lowering_passes, 1);
        assert_eq!(finish.modules.len(), 2);
    }
}
