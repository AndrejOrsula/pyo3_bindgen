//! Shared deterministic external-IR workload for the scale test and benchmark targets.

use pyo3_bindgen_engine::{
    ClassIr, FunctionIr, FunctionKindIr, ImportIr, MethodKindIr, ModuleIr, ParameterIr,
    ParameterKindIr, PropertyIr, SchemaIr,
};

fn empty_module(name: impl Into<String>) -> ModuleIr {
    ModuleIr {
        name: name.into(),
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

fn combined_method(name: String, value_type: String, return_type: String) -> FunctionIr {
    FunctionIr {
        name,
        kind: FunctionKindIr::Method {
            method_type: MethodKindIr::Instance,
        },
        params: vec![
            ParameterIr {
                name: "self".to_owned(),
                kind: ParameterKindIr::PositionalOrKeyword,
                annotation: String::new(),
                has_default: false,
            },
            ParameterIr {
                name: "value".to_owned(),
                kind: ParameterKindIr::PositionalOrKeyword,
                annotation: value_type,
                has_default: false,
            },
            ParameterIr {
                name: "items".to_owned(),
                kind: ParameterKindIr::KeywordOnly,
                annotation: "list[int]".to_owned(),
                has_default: true,
            },
        ],
        return_type,
        docstring: Some("Combined external scale method.".to_owned()),
    }
}

fn combined_function(
    module: &str,
    name: String,
    argument_type: String,
    return_type: String,
) -> FunctionIr {
    FunctionIr {
        name: format!("{module}.{name}"),
        kind: FunctionKindIr::Function,
        params: vec![ParameterIr {
            name: "value".to_owned(),
            kind: ParameterKindIr::PositionalOrKeyword,
            annotation: argument_type,
            has_default: false,
        }],
        return_type,
        docstring: Some("Combined external scale function.".to_owned()),
    }
}

fn combined_property(name: &str, py_type: String) -> PropertyIr {
    PropertyIr {
        name: name.to_owned(),
        py_type: py_type.clone(),
        setter_type: Some(py_type),
        readonly: false,
        docstring: Some("Combined external scale property.".to_owned()),
        setter_docstring: Some("Combined external scale setter.".to_owned()),
    }
}

fn combined_dict(value_type: &str) -> String {
    format!("dict[str, list[{value_type}]]")
}

fn combined_methods(
    prefix: &str,
    scale: usize,
    value_type: &str,
    return_type: &str,
) -> Vec<FunctionIr> {
    (0..scale)
        .map(|index| {
            combined_method(
                format!("{prefix}_{index}"),
                value_type.to_owned(),
                return_type.to_owned(),
            )
        })
        .collect()
}

fn combined_class(
    name: String,
    base_classes: Vec<String>,
    methods: Vec<FunctionIr>,
    property: Option<PropertyIr>,
) -> ClassIr {
    ClassIr {
        name,
        base_classes,
        docstring: Some("Combined external class.".to_owned()),
        methods,
        properties: property.into_iter().collect(),
    }
}

/// Build one deterministic graph combining scaled inheritance, aliases, normalization, and
/// incremental registration without relying on a live Python runtime.
pub(crate) fn combined_external_schema(scale: usize) -> SchemaIr {
    assert!(scale > 0);
    let root = "scale_combined";
    let model = format!("{root}.model");
    let aliases = format!("{root}.aliases");
    let reexports = format!("{root}.reexports");
    let entry = format!("{root}.entry");
    let chain_tail = format!("{model}.Chain{}", scale - 1);
    let terminal_class = format!("{model}.DiamondLeaf");

    let mut model_module = empty_module(model.clone());
    model_module.classes = (0..scale)
        .map(|index| {
            let class_name = format!("{model}.Chain{index}");
            let base_classes = if index == 0 {
                vec![]
            } else {
                vec![format!("{model}.Chain{}", index - 1)]
            };
            let property = (index == 0)
                .then(|| combined_property(&format!("{class_name}.root_value"), "int".to_owned()));
            combined_class(
                class_name,
                base_classes,
                combined_methods(
                    &format!("chain_{index}"),
                    scale,
                    "DiamondLeaf",
                    "DiamondLeaf",
                ),
                property,
            )
        })
        .chain([
            combined_class(
                format!("{model}.BranchLeft"),
                vec![chain_tail.clone()],
                combined_methods("diamond", scale, "Chain0", "Chain0"),
                None,
            ),
            combined_class(
                format!("{model}.BranchRight"),
                vec![chain_tail.clone()],
                combined_methods("diamond", scale, "Chain0", "Chain0"),
                None,
            ),
            combined_class(
                terminal_class.clone(),
                vec![
                    format!("{model}.BranchLeft"),
                    format!("{model}.BranchRight"),
                ],
                combined_methods("leaf", scale, "BranchLeft", "DiamondLeaf"),
                Some(combined_property(
                    &format!("{terminal_class}.leaf_value"),
                    "DiamondLeaf".to_owned(),
                )),
            ),
        ])
        .collect();
    model_module.functions = (0..scale)
        .map(|index| {
            combined_function(
                &model,
                format!("make_{index}"),
                combined_dict("DiamondLeaf"),
                combined_dict("Chain0"),
            )
        })
        .collect();

    let mut alias_module = empty_module(aliases.clone());
    alias_module.imports = (0..scale)
        .map(|index| ImportIr {
            origin: if index % 2 == 0 {
                terminal_class.clone()
            } else {
                chain_tail.clone()
            },
            target: format!("{aliases}.TypeAlias{index}"),
        })
        .collect();
    alias_module.functions = (0..scale)
        .map(|index| {
            combined_function(
                &aliases,
                format!("convert_{index}"),
                combined_dict(&format!("TypeAlias{index}")),
                combined_dict(&format!("TypeAlias{}", (index + 1) % scale)),
            )
        })
        .collect();
    alias_module.properties = vec![combined_property(
        &format!("{aliases}.current"),
        "TypeAlias0".to_owned(),
    )];

    let mut reexport_module = empty_module(reexports.clone());
    reexport_module.imports = (0..scale)
        .flat_map(|index| {
            [
                ImportIr {
                    origin: format!("{aliases}.TypeAlias{index}"),
                    target: format!("{reexports}.PublicType{index}"),
                },
                ImportIr {
                    origin: format!("{aliases}.convert_{index}"),
                    target: format!("{reexports}.reexported_convert_{index}"),
                },
            ]
        })
        .collect();
    reexport_module.functions = (0..scale)
        .map(|index| {
            combined_function(
                &reexports,
                format!("public_convert_{index}"),
                combined_dict(&format!("PublicType{index}")),
                combined_dict(&format!("PublicType{}", (index + 1) % scale)),
            )
        })
        .collect();

    let mut entry_module = empty_module(entry.clone());
    entry_module.imports = (0..scale)
        .flat_map(|index| {
            [
                ImportIr {
                    origin: format!("{reexports}.PublicType{index}"),
                    target: format!("{entry}.EntryType{index}"),
                },
                ImportIr {
                    origin: format!("{reexports}.public_convert_{index}"),
                    target: format!("{entry}.reexported_convert_{index}"),
                },
            ]
        })
        .collect();
    entry_module.functions = (0..scale)
        .map(|index| {
            combined_function(
                &entry,
                format!("entry_convert_{index}"),
                combined_dict(&format!("EntryType{index}")),
                combined_dict(&format!("EntryType{}", (index + 1) % scale)),
            )
        })
        .collect();
    entry_module.properties = vec![combined_property(
        &format!("{entry}.current"),
        "EntryType0".to_owned(),
    )];

    let satellite_modules = (0..scale)
        .map(|index| {
            let satellite = format!("{root}.satellite_{index}");
            let mut module = empty_module(satellite.clone());
            module.imports = vec![ImportIr {
                origin: format!("{reexports}.PublicType{index}"),
                target: format!("{satellite}.SatelliteType"),
            }];
            module.functions = vec![combined_function(
                &satellite,
                "forward".to_owned(),
                combined_dict("SatelliteType"),
                combined_dict("SatelliteType"),
            )];
            module
        })
        .collect::<Vec<_>>();

    let mut modules = vec![model_module, alias_module, reexport_module, entry_module];
    modules.extend(satellite_modules);
    SchemaIr { modules }
}
