//! Tests for the external schema-source (IR) seam (CR1).

use pyo3_bindgen_engine::{
    ClassIr, Codegen, Config, EnumAliasIr, EnumIr, EnumKindIr, EnumVariantIr,
    ExternalIrSourceContext, FunctionIr, FunctionKindIr, GenerationBudget, ImportIr, MethodKindIr,
    ModuleIr, ParameterIr, ParameterKindIr, PropertyIr, SchemaIr,
};
use std::process::Command;

#[path = "../test_support/bounded_consumer.rs"]
mod bounded_consumer;
use bounded_consumer::{OwnedTempDir, CARGO_COMMAND_TIMEOUT};

/// A `ModuleIr` with every collection empty; individual tests fill in what they need.
fn empty_module(name: &str, is_package: bool) -> ModuleIr {
    ModuleIr {
        name: name.to_owned(),
        docstring: None,
        is_package,
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

/// A base-less `ClassIr` with no members.
fn class(name: &str, base_classes: Vec<String>) -> ClassIr {
    ClassIr {
        name: name.to_owned(),
        base_classes,
        docstring: None,
        methods: vec![],
        properties: vec![],
    }
}

fn function(name: &str, return_type: &str) -> FunctionIr {
    FunctionIr {
        name: name.to_owned(),
        kind: FunctionKindIr::Function,
        params: vec![],
        return_type: return_type.to_owned(),
        docstring: None,
    }
}

fn generate_authored_parent_with_dotted_child(
    child_first: bool,
    parent_docstring: &str,
    child_return_type: &str,
) -> pyo3_bindgen_engine::GeneratedBindings {
    let mut parent = empty_module("pkg", false);
    parent.docstring = Some(parent_docstring.to_owned());
    parent.functions.push(FunctionIr {
        name: "pkg.root_value".to_owned(),
        kind: FunctionKindIr::Function,
        params: vec![ParameterIr {
            name: "self".to_owned(),
            kind: ParameterKindIr::PositionalOrKeyword,
            annotation: "int".to_owned(),
            has_default: false,
        }],
        return_type: "int".to_owned(),
        docstring: None,
    });

    let mut child = empty_module("pkg.child", false);
    child.functions.push(FunctionIr {
        name: "pkg.child.child_value".to_owned(),
        kind: FunctionKindIr::Function,
        params: vec![],
        return_type: child_return_type.to_owned(),
        docstring: None,
    });

    let modules = if child_first {
        vec![child, parent]
    } else {
        vec![parent, child]
    };
    Codegen::default()
        .modules_from_ir(SchemaIr { modules })
        .unwrap()
        .generate_with_report()
        .unwrap()
}

fn cstr(value: &str) -> std::ffi::CString {
    std::ffi::CString::new(value).unwrap()
}

#[test]
fn bounded_external_source_identity_reaches_report_assembly() {
    let sha256 = format!("sha256:{}", "a".repeat(64));
    let context = ExternalIrSourceContext::bounded(&sha256, 128, 256, "test:bounded_snapshot")
        .expect("valid bounded source context");
    let generated = Codegen::default()
        .modules_from_ir_with_source_context(
            SchemaIr {
                modules: vec![empty_module("bounded_source", false)],
            },
            context,
        )
        .unwrap()
        .generate_with_report()
        .unwrap();

    assert_eq!(generated.report.modules.len(), 1);
    assert_eq!(
        generated.report.modules[0].source_hash.as_deref(),
        Some(sha256.as_str())
    );
}

#[test]
fn bounded_external_source_context_cannot_claim_incremental_inputs() {
    let context = ExternalIrSourceContext::bounded(
        format!("sha256:{}", "b".repeat(64)),
        128,
        256,
        "test:bounded_snapshot",
    )
    .unwrap();
    let codegen = Codegen::default()
        .modules_from_ir_with_source_context(
            SchemaIr {
                modules: vec![empty_module("first_snapshot", false)],
            },
            context,
        )
        .unwrap();
    let error = codegen
        .modules_from_ir(SchemaIr {
            modules: vec![empty_module("second_snapshot", false)],
        })
        .expect_err("bounded transport identity must cover exactly one complete schema");
    assert!(error.to_string().contains("one complete schema snapshot"));
}

#[test]
fn external_ir_semantic_errors_are_terminal_but_source_errors_are_immediate() {
    let invalid = SchemaIr {
        modules: vec![
            empty_module("duplicate", false),
            empty_module("duplicate", false),
        ],
    };
    let codegen = Codegen::default()
        .modules_from_ir(invalid)
        .expect("schema registration must not validate semantic errors");
    let error = codegen
        .generate()
        .expect_err("duplicate module paths must fail at terminal generation");
    assert!(error
        .to_string()
        .contains("duplicate module path 'duplicate'"));

    let context = ExternalIrSourceContext::bounded(
        format!("sha256:{}", "c".repeat(64)),
        64,
        128,
        "test:complete_snapshot",
    )
    .unwrap();
    let codegen = Codegen::default()
        .modules_from_ir_with_source_context(
            SchemaIr {
                modules: vec![empty_module("complete", false)],
            },
            context,
        )
        .unwrap();
    let error = codegen
        .modules_from_ir(SchemaIr {
            modules: vec![empty_module("incremental", false)],
        })
        .expect_err("bounded-context mixing must remain an immediate registration error");
    assert!(error.to_string().contains("one complete schema snapshot"));
}

#[test]
fn empty_external_ir_is_rejected_at_terminal_generation() {
    let codegen = Codegen::default()
        .modules_from_ir(SchemaIr { modules: vec![] })
        .expect("empty schema registration must be accepted before generation");
    let error = codegen
        .generate()
        .expect_err("empty external IR must fail at terminal generation");
    assert!(error
        .to_string()
        .contains("There are no modules for which to generate bindings"));
}

#[test]
fn module_function_named_obj_is_generated() {
    let mut module = empty_module("pkg", false);
    module.functions.push(FunctionIr {
        name: "pkg.obj".to_owned(),
        kind: FunctionKindIr::Function,
        params: vec![],
        return_type: "int".to_owned(),
        docstring: None,
    });

    let tokens = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .unwrap()
        .generate()
        .unwrap()
        .to_string();

    assert!(
        tokens.contains("fn obj"),
        "a valid Python module function must not be silently dropped: {tokens}"
    );
}

#[test]
fn unqualified_shadowable_ir_annotations_prefer_generated_local_types() {
    let mut module = empty_module("pkg", false);
    module.classes = ["Type", "List", "Annotated", "typing"]
        .into_iter()
        .map(|name| class(&format!("pkg.{name}"), vec![]))
        .collect();
    let function = |name: &str, annotation: &str| FunctionIr {
        name: format!("pkg.{name}"),
        kind: FunctionKindIr::Function,
        params: vec![ParameterIr {
            name: "value".to_owned(),
            kind: ParameterKindIr::PositionalOrKeyword,
            annotation: annotation.to_owned(),
            has_default: false,
        }],
        return_type: annotation.to_owned(),
        docstring: None,
    };
    module.functions = vec![
        // These arities are invalid for the standard aliases. They remain valid here
        // because the unqualified heads name generated local classes.
        function("local_type", "Type[int, str]"),
        function("local_list", "List[int, str]"),
        function("local_annotated", "Annotated[int]"),
        function("typing_type", "typing.Type[int]"),
        function("typing_list", "typing.List[int]"),
        function("typing_annotated", "typing.Annotated[int, 'metadata']"),
    ];

    let tokens = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .expect("shadowable annotations must remain valid external IR")
        .generate()
        .expect("local-first annotation resolution must generate");
    let source = prettyplease::unparse(
        &syn::parse2(tokens).expect("generated bindings must remain valid Rust syntax"),
    );
    let signature = |name: &str| {
        source
            .split(&format!("pub fn {name}<'py>("))
            .nth(1)
            .and_then(|rest| rest.split('{').next())
            .map(str::to_owned)
            .unwrap_or_else(|| panic!("missing generated function `{name}`:\n{source}"))
    };

    for (function, local_type) in [
        ("local_type", "Type"),
        ("local_list", "List"),
        ("local_annotated", "Annotated"),
    ] {
        let signature = signature(function);
        assert!(
            signature.contains(&format!("p_value: &{local_type}"))
                && signature.contains(&format!("PyResult<{local_type}>")),
            "`{function}` must resolve through the generated local class:\n{signature}",
        );
    }
    let typing_type = signature("typing_type");
    assert!(
        typing_type.contains("types::PyType"),
        "qualified typing.Type must remain a Python type object:\n{typing_type}",
    );
    let typing_list = signature("typing_list");
    assert!(
        typing_list.contains("p_value: &[i64]") && typing_list.contains("PyResult<Vec<i64>>"),
        "qualified typing.List must remain the standard list alias:\n{typing_list}",
    );
    let typing_annotated = signature("typing_annotated");
    assert!(
        typing_annotated.contains("p_value: i64") && typing_annotated.contains("PyResult<i64>"),
        "qualified typing.Annotated must unwrap to its standard payload:\n{typing_annotated}",
    );
}

#[test]
fn unqualified_standard_aliases_remain_strict_without_local_shadows() {
    for annotation in ["Type[int, str]", "List[int, str]", "Annotated[int]"] {
        let mut module = empty_module("pkg", false);
        module.functions.push(FunctionIr {
            name: "pkg.invalid".to_owned(),
            kind: FunctionKindIr::Function,
            params: vec![],
            return_type: annotation.to_owned(),
            docstring: None,
        });

        let error = Codegen::default()
            .modules_from_ir(SchemaIr {
                modules: vec![module],
            })
            .expect("external IR registration must defer schema validation")
            .generate()
            .expect_err("invalid standard alias arity must remain strict");
        assert!(
            error.to_string().contains("Invalid"),
            "unexpected error for `{annotation}`: {error}",
        );
    }
}

#[test]
fn local_alias_hides_collection_payload_from_root_support_scan() {
    let mut module = empty_module("PyCollection", false);
    module.classes = vec![class("PyCollection.List", vec![])];
    module.functions = vec![FunctionIr {
        name: "PyCollection.echo".to_owned(),
        kind: FunctionKindIr::Function,
        params: vec![ParameterIr {
            name: "value".to_owned(),
            kind: ParameterKindIr::PositionalOrKeyword,
            annotation: "List[Collection[int]]".to_owned(),
            has_default: false,
        }],
        return_type: "List[Collection[int]]".to_owned(),
        docstring: None,
    }];

    let tokens = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .expect("a local List alias must hide its unused generic payload during validation")
        .generate()
        .expect("the same local-first annotation must lower successfully")
        .to_string();

    assert!(
        !tokens.contains("PyCollection < T"),
        "the hidden Collection payload must not emit the generic root wrapper: {tokens}",
    );
}

#[test]
fn imported_collection_alias_reserves_root_support_before_lowering() {
    let mut module = empty_module("PyCollection", false);
    module.imports = vec![ImportIr {
        origin: "typing.Collection".to_owned(),
        target: "PyCollection.C".to_owned(),
    }];
    module.functions = vec![FunctionIr {
        name: "PyCollection.echo".to_owned(),
        kind: FunctionKindIr::Function,
        params: vec![ParameterIr {
            name: "value".to_owned(),
            kind: ParameterKindIr::PositionalOrKeyword,
            annotation: "C[int]".to_owned(),
            has_default: false,
        }],
        return_type: "C[int]".to_owned(),
        docstring: None,
    }];

    let error = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .expect("external IR registration must defer schema validation")
        .generate()
        .expect_err("the imported Collection alias must reserve its generated root wrapper");

    assert!(
        error.to_string().contains("PyCollection")
            && error.to_string().contains("<generated root support>"),
        "the imported alias must surface the root symbol collision before lowering: {error}",
    );
}

#[test]
fn imported_typing_type_alias_lowers_to_py_type() {
    let mut module = empty_module("pkg", false);
    module.imports = vec![ImportIr {
        origin: "typing.Type".to_owned(),
        target: "pkg.Type".to_owned(),
    }];
    module.functions = vec![FunctionIr {
        name: "pkg.echo_type".to_owned(),
        kind: FunctionKindIr::Function,
        params: vec![ParameterIr {
            name: "value".to_owned(),
            kind: ParameterKindIr::PositionalOrKeyword,
            annotation: "Type[int]".to_owned(),
            has_default: false,
        }],
        return_type: "Type[int]".to_owned(),
        docstring: None,
    }];

    let tokens = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .expect("typing.Type import aliases must remain valid external IR")
        .generate()
        .expect("typing.Type import aliases must lower")
        .to_string();

    assert!(
        tokens.matches("pyo3 :: types :: PyType").count() >= 2
            && !tokens.contains("p_value : impl"),
        "an imported typing.Type alias must lower to PyType, not dynamic PyAny: {tokens}",
    );
}

#[test]
fn chained_typing_type_alias_lowers_to_canonical_standard_type() {
    let mut aliases = empty_module("aliases", false);
    aliases.imports = vec![ImportIr {
        origin: "typing.Type".to_owned(),
        target: "aliases.First".to_owned(),
    }];

    let mut consumer = empty_module("pkg", false);
    consumer.imports = vec![ImportIr {
        origin: "aliases.First".to_owned(),
        target: "pkg.Second".to_owned(),
    }];
    consumer.functions = vec![FunctionIr {
        name: "pkg.echo_type".to_owned(),
        kind: FunctionKindIr::Function,
        params: vec![ParameterIr {
            name: "value".to_owned(),
            kind: ParameterKindIr::PositionalOrKeyword,
            annotation: "Second[int]".to_owned(),
            has_default: false,
        }],
        return_type: "Second[int]".to_owned(),
        docstring: None,
    }];

    let tokens = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![aliases, consumer],
        })
        .expect("chained standard annotation aliases must remain valid")
        .generate()
        .expect("chained standard annotation aliases must lower")
        .to_string();

    assert!(
        tokens.matches("pyo3 :: types :: PyType").count() >= 2,
        "a chained typing.Type alias must retain its canonical standard semantics: {tokens}",
    );
}

#[test]
fn qualified_chained_standard_aliases_reach_their_canonical_origin() {
    let mut aliases = empty_module("aliases", false);
    aliases.imports = vec![ImportIr {
        origin: "typing.Type".to_owned(),
        target: "aliases.First".to_owned(),
    }];

    let mut second = empty_module("second", false);
    second.imports = vec![ImportIr {
        origin: "aliases.First".to_owned(),
        target: "second.Second".to_owned(),
    }];

    let function = |name: &str, annotation: &str| FunctionIr {
        name: format!("pkg.{name}"),
        kind: FunctionKindIr::Function,
        params: vec![ParameterIr {
            name: "value".to_owned(),
            kind: ParameterKindIr::PositionalOrKeyword,
            annotation: annotation.to_owned(),
            has_default: false,
        }],
        return_type: annotation.to_owned(),
        docstring: None,
    };
    let mut consumer = empty_module("pkg", false);
    consumer.functions = vec![
        function("direct_alias", "aliases.First[int]"),
        function("chained_alias", "second.Second[int]"),
    ];

    let tokens = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![aliases, second, consumer],
        })
        .expect("qualified standard aliases must resolve through re-export chains")
        .generate()
        .expect("qualified standard aliases must lower")
        .to_string();

    assert!(
        tokens.matches("pyo3 :: types :: PyType").count() >= 4,
        "both qualified aliases must retain canonical typing.Type semantics: {tokens}",
    );
}

#[test]
fn standard_module_aliases_preserve_member_semantics_through_prefixes() {
    let mut aliases = empty_module("aliases", false);
    aliases.imports = vec![ImportIr {
        origin: "typing".to_owned(),
        target: "aliases.t".to_owned(),
    }];
    aliases.functions = vec![
        function("aliases.local_type", "t.Type[int]"),
        function("aliases.local_collection", "t.Collection[int]"),
    ];

    let mut consumer = empty_module("pkg", false);
    consumer.imports = vec![ImportIr {
        origin: "aliases.t".to_owned(),
        target: "pkg.u".to_owned(),
    }];
    consumer.functions = vec![
        function("pkg.chained_type", "u.Type[int]"),
        function("pkg.qualified_type", "aliases.t.Type[int]"),
        function(
            "pkg.nested_chained_collection",
            "dict[str, list[u.Collection[int]]]",
        ),
    ];

    let tokens = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![aliases, consumer],
        })
        .expect("standard module aliases must validate through local and qualified prefixes")
        .generate()
        .expect("standard module aliases must lower through chained prefixes")
        .to_string();

    assert!(
        tokens.matches("pyo3 :: types :: PyType").count() >= 3,
        "local, qualified, and chained module aliases must retain typing.Type semantics: {tokens}",
    );
    assert!(
        tokens.contains("struct PyCollection < T")
            && tokens.matches("PyCollection < i64 >").count() >= 2,
        "Collection members behind module aliases must reserve and use root support: {tokens}",
    );
}

#[test]
fn repeated_ir_ingestion_rebuilds_standard_module_prefix_aliases() {
    let mut aliases = empty_module("aliases", false);
    aliases.imports = vec![ImportIr {
        origin: "typing".to_owned(),
        target: "aliases.t".to_owned(),
    }];

    let mut bridge = empty_module("bridge", false);
    bridge.imports = vec![ImportIr {
        origin: "aliases.t".to_owned(),
        target: "bridge.u".to_owned(),
    }];

    let mut consumer = empty_module("pkg", false);
    consumer.imports = vec![ImportIr {
        origin: "bridge.u".to_owned(),
        target: "pkg.v".to_owned(),
    }];
    consumer.functions = vec![
        function("pkg.echo_type", "v.Type[int]"),
        function("pkg.echo_collection", "dict[str, list[v.Collection[int]]]"),
    ];

    let batched = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![consumer.clone(), bridge.clone(), aliases.clone()],
        })
        .expect("batched standard module-prefix aliases must validate")
        .generate_artifact_with_report()
        .expect("batched standard module-prefix aliases must lower")
        .artifact;
    let incremental = Codegen::default()
        .module_from_ir(consumer)
        .expect("the consumer must remain valid before its alias modules arrive")
        .module_from_ir(bridge)
        .expect("the intermediate alias module must trigger a valid rebuild")
        .module_from_ir(aliases)
        .expect("the terminal standard module must reclassify the full alias chain")
        .generate_artifact_with_report()
        .expect("incremental standard module-prefix aliases must lower")
        .artifact;

    assert_eq!(
        incremental.bytes(),
        batched.bytes(),
        "incremental ingestion must rebuild annotations and root support byte-identically",
    );
    let batched = batched.contents();
    assert!(
        batched.contains("pyo3::types::PyType") && batched.contains("struct PyCollection<T>"),
        "the rebuilt alias chain must retain Type and Collection semantics: {batched}",
    );
}

#[test]
fn authored_standard_root_children_win_through_module_aliases() {
    let mut typing_module = empty_module("typing", false);
    typing_module.classes = vec![
        class("typing.Type", vec![]),
        class("typing.Collection", vec![]),
    ];

    let mut aliases = empty_module("aliases", false);
    aliases.imports = vec![ImportIr {
        origin: "typing".to_owned(),
        target: "aliases.t".to_owned(),
    }];

    let mut consumer = empty_module("pkg", false);
    consumer.imports = vec![ImportIr {
        origin: "aliases.t".to_owned(),
        target: "pkg.u".to_owned(),
    }];
    consumer.functions = vec![
        function("pkg.local_authored", "u.Type[int, str]"),
        function("pkg.qualified_authored", "aliases.t.Type[int, str]"),
        function(
            "pkg.nested_authored_collection",
            "list[u.Collection[int, str] | None]",
        ),
    ];

    let generated = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![typing_module, aliases, consumer],
        })
        .expect("authored standard-root children must override aliased stdlib members")
        .generate()
        .expect("authored children behind module aliases must lower");
    let source = prettyplease::unparse(
        &syn::parse2(generated).expect("generated bindings must remain valid Rust syntax"),
    );

    assert!(
        source.contains("u::Type")
            && source.contains("typing::Type")
            && source.contains("u::Collection"),
        "aliased authored children must remain generated custom types:\n{source}",
    );
    assert!(
        !source.contains("pyo3::types::PyType"),
        "authored typing.Type must not regain stdlib typing.Type semantics:\n{source}",
    );
    assert!(
        !source.contains("struct PyCollection<"),
        "authored typing.Collection must not emit generic Collection root support:\n{source}",
    );
}

#[test]
fn forced_custom_ir_parse_errors_never_expose_internal_markers() {
    for (owner, annotation) in [
        ("pkg", "typing.Type[int"),
        ("pkg", "list[typing.Type[int]"),
        ("typing", "list[Type[int]"),
    ] {
        let mut typing_module = empty_module("typing", false);
        typing_module.classes = vec![class("typing.Type", vec![])];

        let mut consumer = empty_module("pkg", false);
        if owner == "typing" {
            typing_module.functions = vec![function("typing.invalid", annotation)];
        } else {
            consumer.functions = vec![function("pkg.invalid", annotation)];
        }

        let error = Codegen::default()
            .modules_from_ir(SchemaIr {
                modules: vec![typing_module, consumer],
            })
            .expect("external IR registration must defer schema validation")
            .generate()
            .expect_err("malformed authored standard-root annotations must be rejected");
        let message = error.to_string();
        assert!(
            message.contains(annotation),
            "the diagnostic must preserve the submitted annotation `{annotation}`: {message}",
        );
        assert!(
            !message.contains('\0') && !message.contains("pyo3_bindgen.ir_custom"),
            "internal forced-custom markers must never escape into diagnostics: {message:?}",
        );
    }
}

#[test]
fn external_ir_rejects_balanced_malformed_callable_return_annotations_with_path() {
    for annotation in [
        "Callable[int]",
        "Callable[, str]",
        "Callable[[int]]",
        "Callable[[int],]",
        "Callable[[int], str, bool]",
        "Callable[[],]",
    ] {
        let mut module = empty_module("pkg", false);
        module.functions = vec![function("pkg.invalid", annotation)];

        let error = Codegen::default()
            .modules_from_ir(SchemaIr {
                modules: vec![module],
            })
            .expect("external IR registration must defer schema validation")
            .generate()
            .expect_err("balanced malformed Callable annotations must be rejected");
        assert_eq!(
            error.to_string(),
            format!(
                "Failed to generate Rust code: external IR function 'pkg.invalid' return annotation {annotation:?} has malformed Callable; expected `Callable[[...], result]` or `Callable[..., result]`"
            )
        );
    }
}

#[test]
fn external_ir_rejects_balanced_malformed_callable_parameter_annotations_with_path() {
    let annotation = "Callable[int]";
    let mut module = empty_module("pkg", false);
    module.functions = vec![FunctionIr {
        name: "pkg.invalid".to_owned(),
        kind: FunctionKindIr::Function,
        params: vec![ParameterIr {
            name: "callback".to_owned(),
            kind: ParameterKindIr::PositionalOrKeyword,
            annotation: annotation.to_owned(),
            has_default: false,
        }],
        return_type: "int".to_owned(),
        docstring: None,
    }];

    let error = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .expect("external IR registration must defer schema validation")
        .generate()
        .expect_err("balanced malformed Callable parameter annotations must be rejected");
    assert_eq!(
        error.to_string(),
        format!(
            "Failed to generate Rust code: external IR function 'pkg.invalid' parameter 'callback' annotation {annotation:?} has malformed Callable; expected `Callable[[...], result]` or `Callable[..., result]`"
        )
    );
}

#[test]
fn valid_external_callable_annotations_retain_dynamic_output() {
    let mut module = empty_module("pkg", false);
    for (name, annotation) in [
        ("callable_args", "Callable[[int, str], bool]"),
        ("callable_any_args", "Callable[..., int]"),
        ("callable_no_args", "Callable[[], int]"),
        ("callable_shorthand", "Callable[int, str]"),
        ("callable_trailing", "Callable[[int], str,]"),
        ("callable_list_ellipsis", "Callable[[...], str]"),
    ] {
        module
            .functions
            .push(function(&format!("pkg.{name}"), annotation));
    }

    let generated = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .expect("valid Callable annotations must remain accepted")
        .generate()
        .expect("valid Callable annotations must retain dynamic lowering");
    let source = prettyplease::unparse(
        &syn::parse2(generated).expect("generated bindings must remain valid Rust syntax"),
    );
    for name in [
        "callable_args",
        "callable_any_args",
        "callable_no_args",
        "callable_shorthand",
        "callable_trailing",
        "callable_list_ellipsis",
    ] {
        let signature = source
            .split(&format!("pub fn {name}<'py>("))
            .nth(1)
            .and_then(|rest| rest.split('{').next())
            .unwrap_or_else(|| panic!("missing generated function `{name}`:\n{source}"));
        assert!(
            signature.contains("PyAny"),
            "valid Callable `{name}` must retain dynamic PyAny output:\n{signature}"
        );
    }
}

#[test]
fn callable_suffix_inside_unicode_identifier_is_not_treated_as_standard_callable() {
    let mut module = empty_module("pkg", false);
    module.functions = vec![function("pkg.custom", "ÉCallable[int]")];

    Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .expect("a custom Unicode identifier ending in Callable must retain dynamic fallback")
        .generate()
        .expect("custom Unicode annotation must remain generatable");
}

#[test]
fn live_malformed_callable_annotations_retain_best_effort_dynamic_fallback() {
    pyo3::Python::initialize();
    let generated = pyo3::Python::attach(|py| {
        Codegen::prime_live_runtime(py).unwrap();
        let module = pyo3::types::PyModule::from_code(
            py,
            cstr(
                "def callback(value: 'Callable[int]') -> 'Callable[[int], str]':\n    return value\n",
            )
            .as_c_str(),
            cstr("callable_fallback.py").as_c_str(),
            cstr("callable_fallback").as_c_str(),
        )
        .unwrap();
        Codegen::default()
            .module(&module)
            .unwrap()
            .generate()
            .unwrap()
    });
    let source = prettyplease::unparse(
        &syn::parse2(generated).expect("generated bindings must remain valid Rust syntax"),
    );
    let signature = source
        .split("pub fn callback<'py>(")
        .nth(1)
        .and_then(|rest| rest.split('{').next())
        .expect("the live callback must be generated");
    assert!(
        signature.contains("PyAny"),
        "live malformed Callable annotations must retain dynamic fallback:\n{signature}"
    );
}

#[test]
fn standard_identity_import_does_not_hide_canonical_alias_semantics() {
    let mut typing_module = empty_module("typing", false);
    typing_module.imports = vec![ImportIr {
        origin: "typing.Type".to_owned(),
        target: "typing.Type".to_owned(),
    }];

    let mut consumer = empty_module("pkg", false);
    consumer.imports = vec![ImportIr {
        origin: "typing.Type".to_owned(),
        target: "pkg.Alias".to_owned(),
    }];
    consumer.functions = vec![FunctionIr {
        name: "pkg.echo_type".to_owned(),
        kind: FunctionKindIr::Function,
        params: vec![ParameterIr {
            name: "value".to_owned(),
            kind: ParameterKindIr::PositionalOrKeyword,
            annotation: "Alias[int]".to_owned(),
            has_default: false,
        }],
        return_type: "Alias[int]".to_owned(),
        docstring: None,
    }];

    let tokens = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![typing_module, consumer],
        })
        .expect("identity imports must be inert during standard alias resolution")
        .generate()
        .expect("the canonical alias behind an identity import must lower")
        .to_string();

    assert!(
        tokens.matches("pyo3 :: types :: PyType").count() >= 2,
        "an inert identity import must not turn typing.Type into dynamic PyAny: {tokens}",
    );
}

#[test]
fn qualified_authored_types_under_standard_roots_remain_custom() {
    let mut typing_module = empty_module("typing", false);
    typing_module.classes = vec![
        class("typing.Type", vec![]),
        class("typing.Collection", vec![]),
    ];
    typing_module.functions = vec![FunctionIr {
        name: "typing.echo_local".to_owned(),
        kind: FunctionKindIr::Function,
        params: vec![ParameterIr {
            name: "value".to_owned(),
            kind: ParameterKindIr::PositionalOrKeyword,
            annotation: "Type[int, str]".to_owned(),
            has_default: false,
        }],
        return_type: "Type[int, str]".to_owned(),
        docstring: None,
    }];

    let mut typing_extensions = empty_module("typing_extensions", false);
    typing_extensions.classes = vec![class("typing_extensions.List", vec![])];

    let mut builtins = empty_module("builtins", false);
    builtins.classes = vec![class("builtins.int", vec![])];

    let function = |name: &str, annotation: &str| FunctionIr {
        name: format!("pkg.{name}"),
        kind: FunctionKindIr::Function,
        params: vec![ParameterIr {
            name: "value".to_owned(),
            kind: ParameterKindIr::PositionalOrKeyword,
            annotation: annotation.to_owned(),
            has_default: false,
        }],
        return_type: annotation.to_owned(),
        docstring: None,
    };
    let mut consumer = empty_module("pkg", false);
    consumer.functions = vec![
        function("qualified_type", "typing.Type[int, str]"),
        function("qualified_list", "typing_extensions.List[int, str]"),
        function("qualified_int", "builtins.int"),
        function(
            "nested_qualified_collection",
            "list[typing.Collection[int]]",
        ),
    ];

    let tokens = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![typing_module, typing_extensions, builtins, consumer],
        })
        .expect("authored types under every standard root must bypass standard arity rules")
        .generate()
        .expect("qualified authored standard-root references must lower");
    let source = prettyplease::unparse(
        &syn::parse2(tokens).expect("generated bindings must remain valid Rust syntax"),
    );
    let signature = |name: &str| {
        source
            .split(&format!("pub fn {name}<'py>("))
            .nth(1)
            .and_then(|rest| rest.split('{').next())
            .unwrap_or_else(|| panic!("missing generated function `{name}`:\n{source}"))
    };

    for (name, expected_path) in [
        ("echo_local", "Type"),
        ("qualified_type", "typing::Type"),
        ("qualified_list", "typing_extensions::List"),
        ("qualified_int", "builtins::int"),
    ] {
        let signature = signature(name);
        assert!(
            signature.contains(&format!("&{expected_path}"))
                || signature.contains(&format!("&super::{expected_path}"))
                || signature.contains(&format!("&crate::{expected_path}")),
            "`{name}` must use its authored generated type `{expected_path}`:\n{signature}",
        );
    }
    let nested_collection = signature("nested_qualified_collection");
    assert!(
        nested_collection.contains("typing::Collection")
            && !nested_collection.contains("types::PyAny"),
        "nested authored paths must survive recursive annotation parsing:\n{nested_collection}",
    );
    assert!(
        !source.contains("struct PyCollection<"),
        "an authored typing.Collection must not emit the generic wrapper:\n{source}",
    );
}

fn standard_root_authored_alias_schema() -> (ModuleIr, ModuleIr) {
    let mut typing_module = empty_module("typing", false);
    typing_module.classes = vec![class("typing.Type", vec![])];

    let mut consumer = empty_module("pkg", false);
    consumer.imports = vec![ImportIr {
        origin: "typing.Type".to_owned(),
        target: "pkg.Alias".to_owned(),
    }];
    consumer.functions = vec![FunctionIr {
        name: "pkg.echo_alias".to_owned(),
        kind: FunctionKindIr::Function,
        params: vec![ParameterIr {
            name: "value".to_owned(),
            kind: ParameterKindIr::PositionalOrKeyword,
            annotation: "Alias[int, str]".to_owned(),
            has_default: false,
        }],
        return_type: "Alias[int, str]".to_owned(),
        docstring: None,
    }];
    (consumer, typing_module)
}

fn assert_authored_standard_root_alias(generated: proc_macro2::TokenStream) {
    let source = prettyplease::unparse(
        &syn::parse2(generated).expect("generated bindings must remain valid Rust syntax"),
    );
    let signature = source
        .split("pub fn echo_alias<'py>(")
        .nth(1)
        .and_then(|rest| rest.split('{').next())
        .unwrap_or_else(|| panic!("missing generated alias function:\n{source}"));
    assert!(
        signature.contains("p_value: &Alias") && signature.contains("PyResult<Alias>"),
        "the authored `typing.Type` must retain its generated alias type:\n{signature}",
    );
    assert!(
        !signature.contains("types::PyType"),
        "an authored type beneath the `typing` root must not acquire standard \
         `typing.Type` semantics:\n{signature}",
    );
}

#[test]
fn authored_type_beneath_typing_root_is_not_a_standard_alias() {
    let (consumer, typing_module) = standard_root_authored_alias_schema();
    let generated = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![consumer, typing_module],
        })
        .expect("authored standard-root types must resolve across the complete schema")
        .generate()
        .expect("authored standard-root aliases must lower");

    assert_authored_standard_root_alias(generated);
}

#[test]
fn repeated_ir_ingestion_rebuilds_earlier_standard_root_aliases() {
    let (mut consumer, typing_module) = standard_root_authored_alias_schema();
    // The first call must also be independently valid while the origin still denotes
    // the real standard alias. A single argument is valid for `typing.Type`.
    consumer.functions[0].params[0].annotation = "Alias[int]".to_owned();
    consumer.functions[0].return_type = "Alias[int]".to_owned();

    let codegen = Codegen::default()
        .module_from_ir(consumer)
        .expect("the initial unresolved standard import must remain valid")
        .module_from_ir(typing_module)
        .expect("later authored modules must reclassify earlier aliases");
    let generated = codegen
        .generate()
        .expect("incrementally supplied authored aliases must lower");

    assert_authored_standard_root_alias(generated);
}

#[test]
fn live_module_function_named_obj_is_generated() {
    pyo3::Python::initialize();
    let tokens = pyo3::Python::attach(|py| {
        Codegen::prime_live_runtime(py).unwrap();
        let module = pyo3::types::PyModule::from_code(
            py,
            cstr("def obj() -> int:\n    return 1\n").as_c_str(),
            cstr("reserved_name.py").as_c_str(),
            cstr("reserved_name").as_c_str(),
        )
        .unwrap();
        Codegen::default()
            .module(&module)
            .unwrap()
            .generate()
            .unwrap()
            .to_string()
    });

    assert!(
        tokens.contains("fn obj"),
        "live introspection must not silently drop a valid Python function: {tokens}"
    );
}

/// (a) A hand-built `SchemaIr` for a tiny module emits byte-identical tokens to the
/// equivalent live-introspected module.
#[test]
fn ir_emits_byte_identical_tokens_to_live_module() {
    // Compare the actual generated bindings; the provenance hash is intentionally
    // content-derived (and therefore differs by design between the two paths), and
    // the prelude adds an orthogonal re-export module, so both are disabled here.
    let cfg = Config::builder()
        .generate_provenance(false)
        .generate_preludes(false)
        .build();

    // Live introspection of a tiny module.
    pyo3::Python::initialize();
    let live_codegen = pyo3::Python::attach(|py| {
        Codegen::prime_live_runtime(py).unwrap();
        let module = pyo3::types::PyModule::from_code(
            py,
            cstr("\"\"\"Tiny module.\"\"\"\n\ndef foo(x: int) -> int:\n    return x\n").as_c_str(),
            cstr("tinymod/__init__.py").as_c_str(),
            cstr("tinymod").as_c_str(),
        )
        .unwrap();
        Codegen::new(cfg.clone()).module(&module).unwrap()
    });
    let live_tokens = live_codegen.generate().unwrap().to_string();

    // Equivalent hand-built IR (no live introspection).
    let ir = SchemaIr {
        modules: vec![ModuleIr {
            name: "tinymod".to_owned(),
            docstring: Some("Tiny module.".to_owned()),
            is_package: false,
            classes: vec![],
            enums: vec![],
            functions: vec![FunctionIr {
                name: "tinymod.foo".to_owned(),
                kind: FunctionKindIr::Function,
                params: vec![ParameterIr {
                    name: "x".to_owned(),
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
        }],
    };
    let ir_tokens = Codegen::new(cfg)
        .modules_from_ir(ir)
        .unwrap()
        .generate()
        .unwrap()
        .to_string();

    assert_eq!(
        live_tokens, ir_tokens,
        "IR-generated tokens must be byte-identical to the live path"
    );
}

/// Build a small but representative IR: a package with a typed-pointer getter that
/// resolves to another class newtype, plus a mirrored enum.
fn sample_schema() -> SchemaIr {
    SchemaIr {
        modules: vec![ModuleIr {
            name: "pkg".to_owned(),
            docstring: Some("Package.".to_owned()),
            is_package: true,
            classes: vec![],
            enums: vec![],
            functions: vec![],
            properties: vec![],
            submodules: vec![ModuleIr {
                name: "pkg.types".to_owned(),
                docstring: None,
                is_package: true,
                enums: vec![EnumIr {
                    name: "pkg.types.Color".to_owned(),
                    kind: EnumKindIr::Enum,
                    variants: vec![
                        EnumVariantIr {
                            py_name: "RED".to_owned(),
                            value_repr: "1".to_owned(),
                        },
                        EnumVariantIr {
                            py_name: "GREEN".to_owned(),
                            value_repr: "2".to_owned(),
                        },
                    ],
                    aliases: vec![],
                    docstring: None,
                    is_flag: false,
                    is_open: false,
                    string_backed: false,
                    plain_mirror: false,
                }],
                classes: vec![
                    ClassIr {
                        name: "pkg.types.Inner".to_owned(),
                        base_classes: vec![],
                        docstring: None,
                        methods: vec![],
                        properties: vec![],
                    },
                    ClassIr {
                        name: "pkg.types.Outer".to_owned(),
                        base_classes: vec![],
                        docstring: None,
                        methods: vec![],
                        properties: vec![
                            PropertyIr {
                                name: "inner".to_owned(),
                                py_type: "pkg.types.Inner".to_owned(),
                                setter_type: None,
                                readonly: true,
                                docstring: None,
                                setter_docstring: None,
                            },
                            PropertyIr {
                                name: "color".to_owned(),
                                py_type: "pkg.types.Color".to_owned(),
                                setter_type: None,
                                readonly: true,
                                docstring: None,
                                setter_docstring: None,
                            },
                        ],
                    },
                ],
                functions: vec![],
                properties: vec![],
                submodules: vec![],
                prelude: vec![],
                imports: vec![],
                type_vars: vec![],
            }],
            prelude: vec![],
            imports: vec![],
            type_vars: vec![],
        }],
    }
}

/// (b) The IR path produces bindings without reaching any `pyo3` interpreter: it
/// never imports Python, and the report carries no Python metadata.
#[test]
fn ir_path_is_python_free() {
    // NOTE: deliberately do NOT call `pyo3::Python::initialize()`.
    let generated = Codegen::new(Config::default())
        .modules_from_ir(sample_schema())
        .unwrap()
        .generate_with_report()
        .unwrap();

    // The report must not attach to a Python interpreter for the IR path.
    assert!(
        generated.report.python.is_none(),
        "IR path must not populate Python metadata"
    );
    assert!(
        generated
            .report
            .modules
            .iter()
            .all(|module| module.input_kind == "external_ir"),
        "IR modules should be reported as `external_ir`"
    );

    // Sanity: the expected emitted shapes are present.
    let tokens = generated.tokens.to_string();
    assert!(tokens.contains("pub struct Outer"), "missing Outer newtype");
    assert!(tokens.contains("pub struct Inner"), "missing Inner newtype");
    assert!(tokens.contains("pub enum Color"), "missing Color enum");
    // The typed pointer getter resolves to the `Inner` newtype, not a bare PyAny.
    assert!(
        tokens.contains("PyResult < Inner >"),
        "typed getter should resolve to the Inner newtype: {tokens}"
    );
}

#[test]
fn external_ir_enum_value_repr_remains_caller_controlled() {
    let mut module = empty_module("pkg", false);
    module.enums.push(EnumIr {
        name: "pkg.Value".to_owned(),
        kind: EnumKindIr::Enum,
        variants: vec![EnumVariantIr {
            py_name: "PRIMARY".to_owned(),
            value_repr: "<CallerValue object at 0xCAFE>".to_owned(),
        }],
        aliases: vec![EnumAliasIr {
            py_name: "ALIAS".to_owned(),
            target_py_name: "PRIMARY".to_owned(),
            value_repr: "caller-supplied alias repr".to_owned(),
        }],
        docstring: None,
        is_flag: false,
        is_open: false,
        string_backed: false,
        plain_mirror: false,
    });

    let generated = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .unwrap()
        .generate_with_report()
        .unwrap();
    let enumeration = &generated.report.modules[0].enums[0];

    assert_eq!(
        enumeration.variants[0].value_repr,
        "<CallerValue object at 0xCAFE>"
    );
    assert_eq!(
        enumeration.aliases[0].value_repr,
        "caller-supplied alias repr"
    );
}

#[test]
fn external_ir_enum_alias_requires_exact_python_target() {
    let mut module = empty_module("pkg", false);
    module.enums = vec![EnumIr {
        name: "pkg.Mode".to_owned(),
        kind: EnumKindIr::Enum,
        variants: vec![EnumVariantIr {
            py_name: "foo-bar".to_owned(),
            value_repr: "1".to_owned(),
        }],
        aliases: vec![EnumAliasIr {
            py_name: "ALIAS".to_owned(),
            target_py_name: "foo_bar".to_owned(),
            value_repr: "1".to_owned(),
        }],
        docstring: None,
        is_flag: false,
        is_open: false,
        string_backed: false,
        plain_mirror: false,
    }];

    let error = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .expect("external IR registration must defer schema validation")
        .generate()
        .expect_err("an alias target must match a canonical Python member exactly");

    assert!(
        error
            .to_string()
            .contains("alias 'ALIAS' targets missing canonical member 'foo_bar'"),
        "unexpected error: {error}"
    );
}

/// A tiny RNA-flavoured schema: a string-valued enum plus a class property typed by it.
/// Mirrors the Blender case where `obj.rotation_mode` is the `str` `'XYZ'` and there is no
/// backing `bpy.types.*` enum class to import.
fn rna_enum_schema() -> SchemaIr {
    let mut module = empty_module("pkg", false);
    module.enums.push(EnumIr {
        name: "pkg.RotationMode".to_owned(),
        kind: EnumKindIr::StrEnum,
        variants: vec![
            EnumVariantIr {
                py_name: "XYZ".to_owned(),
                value_repr: "'XYZ'".to_owned(),
            },
            EnumVariantIr {
                py_name: "XZY".to_owned(),
                value_repr: "'XZY'".to_owned(),
            },
        ],
        aliases: vec![],
        docstring: None,
        is_flag: false,
        is_open: false,
        string_backed: false,
        plain_mirror: false,
    });
    module.classes.push(ClassIr {
        name: "pkg.Object".to_owned(),
        base_classes: vec![],
        docstring: None,
        methods: vec![],
        properties: vec![PropertyIr {
            name: "rotation_mode".to_owned(),
            py_type: "pkg.RotationMode".to_owned(),
            setter_type: None,
            readonly: false,
            docstring: None,
            setter_docstring: None,
        }],
    });
    SchemaIr {
        modules: vec![module],
    }
}

fn generate_rna_enum(string_backed: bool) -> String {
    let cfg = Config::builder()
        .enum_ir_string_backed(string_backed)
        .build();
    Codegen::new(cfg)
        .modules_from_ir(rna_enum_schema())
        .unwrap()
        .generate()
        .unwrap()
        .to_string()
}

/// With `enum_ir_string_backed` ON, the mirror enum round-trips through its member STRING:
/// `into_py_any` builds a `PyString` from `as_py_name`, and `extract` reads a `String`
/// straight off the object and maps it back via `from_py_name`. There is no Python enum
/// class to import, so neither `is_instance` nor a `.name` `getattr` appears.
#[test]
fn external_ir_string_backed_enum_round_trips_via_member_string() {
    let tokens = generate_rna_enum(true);

    // `into_py_any` emits the member string, not a class import + `getattr(class, name)`.
    assert!(
        tokens.contains(":: pyo3 :: types :: PyString :: new (py , self . as_py_name ())"),
        "string-backed into_py_any must build a PyString from as_py_name: {tokens}"
    );
    // `extract` reads a `String` directly off `obj` and feeds it to `from_py_name`.
    assert!(
        tokens.contains(":: std :: string :: String , > (obj . as_any ())"),
        "string-backed extract must read a String straight off obj: {tokens}"
    );
    assert!(
        tokens.contains("Self :: from_py_name (& member_name)"),
        "string-backed extract must map the member string via from_py_name: {tokens}"
    );
    // No Python enum class is imported: neither the class-import `is_instance` guard (its
    // enum-specific error message) nor the `.name` read of the class-import mirror is present.
    assert!(
        !tokens.contains("expected Python enum member of"),
        "string-backed enum must not emit the class-import is_instance guard: {tokens}"
    );
    assert!(
        !tokens.contains(":: std :: string :: String , > (& name_attr)"),
        "string-backed extract must not read `.name` off an enum member: {tokens}"
    );
}

/// With the flag OFF (the default), the historical class-import mirror is retained
/// unchanged: `extract` guards with `is_instance` and reads `.name`, and `into_py_any`
/// resolves the member off the imported Python enum class.
#[test]
fn external_ir_class_import_enum_is_retained_by_default() {
    let tokens = generate_rna_enum(false);

    assert!(
        tokens.contains("expected Python enum member of"),
        "default enum must keep the class-import is_instance guard: {tokens}"
    );
    assert!(
        tokens.contains(":: std :: string :: String , > (& name_attr)"),
        "default extract must read `.name` off the enum member: {tokens}"
    );
    assert!(
        !tokens.contains(":: pyo3 :: types :: PyString :: new (py , self . as_py_name ())"),
        "default into_py_any must not build a PyString from as_py_name: {tokens}"
    );
}

/// The same RNA-flavoured document as [`rna_enum_schema`], expressed as the JSON an
/// external producer actually writes: the per-enum `string_backed`/`plain_mirror` fields are
/// ABSENT, exactly as in every IR document authored before they existed.
const RNA_ENUM_IR_JSON_WITHOUT_NEW_FIELDS: &str = r#"{
  "modules": [
    {
      "name": "pkg",
      "is_package": false,
      "classes": [
        {
          "name": "pkg.Object",
          "properties": [
            {
              "name": "rotation_mode",
              "py_type": "pkg.RotationMode",
              "readonly": false
            }
          ]
        }
      ],
      "enums": [
        {
          "name": "pkg.RotationMode",
          "kind": "StrEnum",
          "variants": [
            { "py_name": "XYZ", "value_repr": "'XYZ'" },
            { "py_name": "XZY", "value_repr": "'XZY'" }
          ]
        }
      ]
    }
  ]
}"#;

/// The `PYO3_BINDGEN_INPUT_HASH` this document produced BEFORE the per-enum
/// `string_backed`/`plain_mirror` fields existed, measured on the pre-wave base commit
/// `953344f` with the identical JSON text and `Config::default()`.
///
/// `Module::from_ir` hashes the *re-serialized* deserialized IR, so any new field that
/// serializes unconditionally silently rewrites this provenance constant for every
/// pre-existing enum-bearing document. The new fields therefore skip serialization when
/// `false`, which is what keeps this golden stable. A `true` value still perturbs the hash
/// -- correctly, since it changes lowering.
const RNA_ENUM_IR_PRE_WAVE_INPUT_HASH: &str = "fnv1a64:7b9699f4208270c6";

/// Generate with provenance ENABLED and return `(tokens, module input hash)`.
fn generate_rna_enum_json_with_provenance(schema: SchemaIr) -> (String, String) {
    let generated = Codegen::new(Config::default())
        .modules_from_ir(schema)
        .expect("RNA enum document must build")
        .generate_with_report()
        .expect("RNA enum document must generate");
    let input_hash = generated.report.modules[0].input_hash.clone();
    (generated.tokens.to_string(), input_hash)
}

/// Adding the per-enum `string_backed`/`plain_mirror` fields must be byte-neutral for every
/// pre-existing IR document -- INCLUDING the provenance constants. Absent fields must hash
/// exactly as they did before the fields existed, and an explicit `false` must be
/// indistinguishable from absence.
#[test]
fn external_ir_input_hash_is_byte_neutral_for_absent_per_enum_fields() {
    let absent: SchemaIr = serde_json::from_str(RNA_ENUM_IR_JSON_WITHOUT_NEW_FIELDS)
        .expect("the pre-wave IR document must still deserialize");
    let (absent_tokens, absent_hash) = generate_rna_enum_json_with_provenance(absent);

    assert_eq!(
        absent_hash, RNA_ENUM_IR_PRE_WAVE_INPUT_HASH,
        "an IR document without the new per-enum fields must keep its pre-wave input hash"
    );
    assert!(
        absent_tokens.contains(RNA_ENUM_IR_PRE_WAVE_INPUT_HASH),
        "the emitted PYO3_BINDGEN_INPUT_HASH constant must carry the pre-wave hash: {absent_tokens}"
    );

    // Explicitly writing `false` is the same document: same tokens, same provenance.
    let mut explicit_false: SchemaIr = serde_json::from_str(RNA_ENUM_IR_JSON_WITHOUT_NEW_FIELDS)
        .expect("the pre-wave IR document must still deserialize");
    explicit_false.modules[0].enums[0].string_backed = false;
    explicit_false.modules[0].enums[0].plain_mirror = false;
    let (explicit_tokens, explicit_hash) = generate_rna_enum_json_with_provenance(explicit_false);

    assert_eq!(
        explicit_hash, absent_hash,
        "an explicit `false` per-enum field must hash identically to an absent one"
    );
    assert_eq!(
        explicit_tokens, absent_tokens,
        "an explicit `false` per-enum field must generate byte-identical output"
    );
}

/// The converse guard: a `true` per-enum field DOES change lowering, so it must also change
/// the provenance hash. Byte-neutrality is a claim about absent/false fields only.
#[test]
fn external_ir_input_hash_changes_when_a_per_enum_field_is_enabled() {
    let absent: SchemaIr = serde_json::from_str(RNA_ENUM_IR_JSON_WITHOUT_NEW_FIELDS).unwrap();
    let (_, absent_hash) = generate_rna_enum_json_with_provenance(absent);

    let mut string_backed: SchemaIr =
        serde_json::from_str(RNA_ENUM_IR_JSON_WITHOUT_NEW_FIELDS).unwrap();
    string_backed.modules[0].enums[0].string_backed = true;
    let (_, string_backed_hash) = generate_rna_enum_json_with_provenance(string_backed);

    assert_ne!(
        string_backed_hash, absent_hash,
        "`string_backed: true` changes lowering and must change the input hash"
    );
}

/// Same RNA-flavoured fixture as [`rna_enum_schema`], except the enum object itself carries
/// `string_backed` instead of (or in addition to) the global `Config::enum_ir_string_backed`
/// flag.
fn rna_enum_schema_with_field(string_backed_field: bool) -> SchemaIr {
    let mut schema = rna_enum_schema();
    schema.modules[0].enums[0].string_backed = string_backed_field;
    schema
}

fn generate_rna_enum_with(cfg_string_backed: bool, enum_string_backed_field: bool) -> String {
    // Provenance embeds a content hash of the *IR document itself*. Since the per-enum
    // fields are skipped when `false` (see `EnumIr::string_backed`), an ABSENT/`false` field
    // no longer perturbs that hash -- see
    // `external_ir_input_hash_is_byte_neutral_for_absent_per_enum_fields`. But the two
    // fixtures compared through this helper are genuinely DIFFERENT documents: one sets the
    // per-enum field to `true` (which is serialized, and must be) while the other leaves it
    // `false` and drives the behaviour from `Config` instead, which is not part of the IR at
    // all. Disable provenance here so the comparison targets emitted code, not the hash of
    // two documents that are legitimately distinct.
    let cfg = Config::builder()
        .enum_ir_string_backed(cfg_string_backed)
        .generate_provenance(false)
        .build();
    Codegen::new(cfg)
        .modules_from_ir(rna_enum_schema_with_field(enum_string_backed_field))
        .unwrap()
        .generate()
        .unwrap()
        .to_string()
}

/// The per-enum `string_backed` field, with no global `Config::enum_ir_string_backed`, drives
/// the exact same string round-trip as the global flag: `into_py_any` builds a `PyString` from
/// `as_py_name`, `extract` reads a `String` straight off the object and maps it back via
/// `from_py_name`, and no Python enum class is imported.
#[test]
fn enum_ir_per_enum_string_backed_field_parses_and_emits_string_round_trip() {
    let tokens = generate_rna_enum_with(false, true);

    assert!(
        tokens.contains(":: pyo3 :: types :: PyString :: new (py , self . as_py_name ())"),
        "per-enum string-backed into_py_any must build a PyString from as_py_name: {tokens}"
    );
    assert!(
        tokens.contains(":: std :: string :: String , > (obj . as_any ())"),
        "per-enum string-backed extract must read a String straight off obj: {tokens}"
    );
    assert!(
        tokens.contains("Self :: from_py_name (& member_name)"),
        "per-enum string-backed extract must map the member string via from_py_name: {tokens}"
    );
    assert!(
        !tokens.contains("expected Python enum member of"),
        "per-enum string-backed enum must not emit the class-import is_instance guard: {tokens}"
    );
    assert!(
        !tokens.contains(":: std :: string :: String , > (& name_attr)"),
        "per-enum string-backed extract must not read `.name` off an enum member: {tokens}"
    );
}

/// The per-enum field and the global flag are combined with a logical OR and must produce
/// byte-for-byte identical output: (a) global ON, field absent (defaults `false`), versus
/// (b) global OFF, field `true` on the enum.
#[test]
fn enum_ir_per_enum_string_backed_matches_global_flag_byte_for_byte() {
    let via_global = generate_rna_enum_with(true, false);
    let via_per_enum = generate_rna_enum_with(false, true);

    assert_eq!(
        via_global, via_per_enum,
        "global-flag and per-enum-field string-backing must generate identical output"
    );
}

/// With the per-enum `string_backed` field absent (serde-defaulted `false`) and the global
/// flag also off, output is unchanged from the historical class-import mirror: `extract`
/// guards with `is_instance` and reads `.name`, and `into_py_any` resolves the member off the
/// imported Python enum class. Guards serde-default correctness for the new field.
#[test]
fn enum_ir_string_backed_field_absent_is_byte_neutral() {
    let tokens = generate_rna_enum(false);

    assert!(
        tokens.contains("expected Python enum member of"),
        "default enum must keep the class-import is_instance guard: {tokens}"
    );
    assert!(
        tokens.contains(":: std :: string :: String , > (& name_attr)"),
        "default extract must read `.name` off the enum member: {tokens}"
    );
    assert!(
        !tokens.contains(":: pyo3 :: types :: PyString :: new (py , self . as_py_name ())"),
        "default into_py_any must not build a PyString from as_py_name: {tokens}"
    );
}

/// (b') The same IR regenerated twice yields a stable, content-derived drift hash.
#[test]
fn ir_input_hash_is_stable_across_regenerations() {
    let first = Codegen::new(Config::default())
        .modules_from_ir(sample_schema())
        .unwrap()
        .generate_with_report()
        .unwrap();
    let second = Codegen::new(Config::default())
        .modules_from_ir(sample_schema())
        .unwrap()
        .generate_with_report()
        .unwrap();

    let hashes = |report: &pyo3_bindgen_engine::GenerationReport| {
        report
            .modules
            .iter()
            .map(|module| module.input_hash.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(hashes(&first.report), hashes(&second.report));
    assert_eq!(first.tokens.to_string(), second.tokens.to_string());
}

/// (c) `SchemaIr` round-trips losslessly through JSON.
#[test]
fn schema_ir_serde_round_trip() {
    let schema = sample_schema();
    let json = serde_json::to_string(&schema).unwrap();
    let restored: SchemaIr = serde_json::from_str(&json).unwrap();
    assert_eq!(schema, restored);
}

/// Malformed root module paths must be rejected at the external-IR seam. They must
/// never reach generation, where an empty path segment can trigger a panic.
#[test]
fn malformed_root_module_paths_are_rejected_without_panicking() {
    let mut failures = Vec::new();

    for malformed in [
        "",
        ".",
        ".pkg",
        "pkg.",
        "pkg..child",
        "pkg::child",
        "pkg\0child",
    ] {
        let outcome = std::panic::catch_unwind(|| {
            Codegen::new(Config::default())
                .modules_from_ir(SchemaIr {
                    modules: vec![empty_module(malformed, false)],
                })
                .and_then(|codegen| codegen.generate())
        });

        match outcome {
            Ok(Err(_)) => {}
            Ok(Ok(_)) => failures.push(format!("{malformed:?}: accepted")),
            Err(_) => failures.push(format!("{malformed:?}: panicked")),
        }
    }

    assert!(
        failures.is_empty(),
        "malformed root module paths must return Err: {}",
        failures.join(", ")
    );
}

#[test]
fn duplicate_external_ir_module_paths_are_rejected() {
    let error = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![empty_module("pkg", true), empty_module("pkg", true)],
        })
        .expect("external IR registration must defer schema validation")
        .generate()
        .expect_err("duplicate source modules must not be silently merged");

    assert!(
        error.to_string().contains("duplicate module path 'pkg'"),
        "unexpected error: {error}",
    );
}

#[test]
fn duplicate_external_ir_definition_paths_are_rejected() {
    let mut module = empty_module("pkg", true);
    module.classes = vec![class("pkg.Widget", vec![]), class("pkg.Widget", vec![])];

    let error = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .expect("external IR registration must defer schema validation")
        .generate()
        .expect_err("duplicate definitions must not reach Rust code generation");

    assert!(
        error
            .to_string()
            .contains("duplicate definition path 'pkg.Widget'"),
        "unexpected error: {error}",
    );
}

#[test]
fn external_ir_class_must_belong_to_its_owning_module() {
    let mut module = empty_module("pkg", true);
    module.classes = vec![class("other.Widget", vec![])];

    let error = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .expect("external IR registration must defer schema validation")
        .generate()
        .expect_err("a class outside its owning module must be rejected");

    assert!(
        error
            .to_string()
            .contains("class path 'other.Widget' must be owned by module 'pkg'"),
        "unexpected error: {error}",
    );
}

#[test]
fn external_ir_enum_must_belong_to_its_owning_module() {
    let mut module = empty_module("pkg", true);
    module.enums.push(EnumIr {
        name: "other.Color".to_owned(),
        kind: EnumKindIr::Enum,
        variants: vec![EnumVariantIr {
            py_name: "RED".to_owned(),
            value_repr: "1".to_owned(),
        }],
        aliases: vec![],
        docstring: None,
        is_flag: false,
        is_open: false,
        string_backed: false,
        plain_mirror: false,
    });

    let error = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .expect("external IR registration must defer schema validation")
        .generate()
        .expect_err("an enum outside its owning module must be rejected");

    assert!(
        error
            .to_string()
            .contains("enum path 'other.Color' must be owned by module 'pkg'"),
        "unexpected error: {error}",
    );
}

#[test]
fn external_ir_function_must_belong_to_its_owning_module() {
    let mut module = empty_module("pkg", true);
    module.functions.push(FunctionIr {
        name: "other.run".to_owned(),
        kind: FunctionKindIr::Function,
        params: vec![],
        return_type: String::new(),
        docstring: None,
    });

    let error = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .expect("external IR registration must defer schema validation")
        .generate()
        .expect_err("a function outside its owning module must be rejected");

    assert!(
        error
            .to_string()
            .contains("function path 'other.run' must be owned by module 'pkg'"),
        "unexpected error: {error}",
    );
}

#[test]
fn external_ir_property_must_belong_to_its_owning_module() {
    let mut module = empty_module("pkg", true);
    module.properties.push(PropertyIr {
        name: "other.value".to_owned(),
        py_type: "int".to_owned(),
        setter_type: None,
        readonly: true,
        docstring: None,
        setter_docstring: None,
    });

    let error = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .expect("external IR registration must defer schema validation")
        .generate()
        .expect_err("a property outside its owning module must be rejected");

    assert!(
        error
            .to_string()
            .contains("property path 'other.value' must be owned by module 'pkg'"),
        "unexpected error: {error}",
    );
}

#[test]
fn external_ir_nested_module_must_belong_to_its_parent_module() {
    let mut module = empty_module("pkg", true);
    module.submodules = vec![empty_module("other.child", false)];

    let error = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .expect("external IR registration must defer schema validation")
        .generate()
        .expect_err("a nested module outside its declared parent must be rejected");

    assert!(
        error
            .to_string()
            .contains("module path 'other.child' must be owned by module 'pkg'"),
        "unexpected error: {error}",
    );
}

#[test]
fn external_ir_method_must_belong_to_its_owning_class() {
    let mut widget = class("pkg.Widget", vec![]);
    widget.methods.push(FunctionIr {
        name: "other.Widget.run".to_owned(),
        kind: FunctionKindIr::Method {
            method_type: MethodKindIr::Static,
        },
        params: vec![],
        return_type: String::new(),
        docstring: None,
    });
    let mut module = empty_module("pkg", true);
    module.classes = vec![widget];

    let error = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .expect("external IR registration must defer schema validation")
        .generate()
        .expect_err("a method outside its owning class must be rejected");

    assert!(
        error
            .to_string()
            .contains("method path 'other.Widget.run' must be owned by class 'pkg.Widget'"),
        "unexpected error: {error}",
    );
}

#[test]
fn external_ir_method_can_use_owner_relative_name() {
    let mut widget = class("pkg.Widget", vec![]);
    widget.methods.push(FunctionIr {
        name: "make".to_owned(),
        kind: FunctionKindIr::Method {
            method_type: MethodKindIr::Static,
        },
        params: vec![],
        return_type: String::new(),
        docstring: None,
    });
    let mut module = empty_module("pkg", true);
    module.classes = vec![widget];

    let tokens = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .expect("ClassIr permits owner-relative method names")
        .generate()
        .unwrap()
        .to_string();

    assert!(
        tokens.contains("fn make"),
        "missing relative method: {tokens}"
    );
}

#[test]
fn external_ir_method_property_name_collision_is_rejected() {
    let mut widget = class("pkg.Widget", vec![]);
    widget.methods.push(FunctionIr {
        name: "pkg.Widget.value".to_owned(),
        kind: FunctionKindIr::Method {
            method_type: MethodKindIr::Static,
        },
        params: vec![],
        return_type: String::new(),
        docstring: None,
    });
    widget.properties.push(PropertyIr {
        name: "value".to_owned(),
        py_type: "int".to_owned(),
        setter_type: None,
        readonly: true,
        docstring: None,
        setter_docstring: None,
    });
    let mut module = empty_module("pkg", true);
    module.classes = vec![widget];

    let error = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .expect("external IR registration must defer schema validation")
        .generate()
        .expect_err("method/property Rust symbol collisions must be rejected");

    assert!(
        error
            .to_string()
            .contains("duplicate definition path 'pkg.Widget.value'"),
        "unexpected error: {error}",
    );
}

#[test]
fn external_ir_property_setter_method_collision_is_rejected() {
    let mut widget = class("pkg.Widget", vec![]);
    widget.methods.push(FunctionIr {
        name: "pkg.Widget.set_value".to_owned(),
        kind: FunctionKindIr::Method {
            method_type: MethodKindIr::Static,
        },
        params: vec![],
        return_type: String::new(),
        docstring: None,
    });
    widget.properties.push(PropertyIr {
        name: "value".to_owned(),
        py_type: "int".to_owned(),
        setter_type: None,
        readonly: false,
        docstring: None,
        setter_docstring: None,
    });
    let mut module = empty_module("pkg", true);
    module.classes = vec![widget];

    let error = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .expect("external IR registration must defer schema validation")
        .generate()
        .expect_err("generated setter/method collisions must be rejected");

    assert!(
        error
            .to_string()
            .contains("Rust value symbol collision 'set_value'"),
        "unexpected error: {error}",
    );
}

#[test]
fn external_ir_module_property_setter_function_name_collision_is_rejected() {
    let mut module = empty_module("pkg", true);
    module.functions.push(FunctionIr {
        name: "pkg.set_value".to_owned(),
        kind: FunctionKindIr::Function,
        params: vec![],
        return_type: String::new(),
        docstring: None,
    });
    module.properties.push(PropertyIr {
        name: "pkg.value".to_owned(),
        py_type: "int".to_owned(),
        setter_type: None,
        readonly: false,
        docstring: None,
        setter_docstring: None,
    });

    let error = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .expect("external IR registration must defer schema validation")
        .generate()
        .expect_err(
            "a module-level property setter colliding with a module-level function must be rejected",
        );

    assert!(
        error
            .to_string()
            .contains("Rust value symbol collision 'set_value'"),
        "unexpected error: {error}",
    );
}

#[test]
fn sanitized_external_ir_type_name_collisions_are_rejected() {
    let mut module = empty_module("pkg", true);
    module.classes = vec![class("pkg.Self", vec![]), class("pkg.s_Self", vec![])];

    let error = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .expect("external IR registration must defer schema validation")
        .generate()
        .expect_err("distinct Python names must not emit one Rust type symbol");

    assert!(
        error
            .to_string()
            .contains("Rust type symbol collision 's_Self'"),
        "unexpected error: {error}",
    );
}

#[test]
fn sanitized_external_ir_class_submodule_collisions_are_rejected() {
    let mut module = empty_module("pkg", true);
    module.classes = vec![class("pkg.Self", vec![])];
    module.submodules = vec![empty_module("pkg.s_Self", false)];

    let error = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .expect("external IR registration must defer schema validation")
        .generate()
        .expect_err("sanitized class/submodule symbols must not collide");

    assert!(
        error
            .to_string()
            .contains("Rust type symbol collision 's_Self'"),
        "unexpected error: {error}",
    );
}

/// Two dotted-named top-level modules sharing a root must collapse into a single
/// `pub mod` for that root, not emit it twice (which would be `E0428`).
#[test]
fn dotted_sibling_modules_merge_into_one_root() {
    let mut a = empty_module("pkg.a", true);
    a.functions.push(FunctionIr {
        name: "pkg.a.foo".to_owned(),
        kind: FunctionKindIr::Function,
        params: vec![],
        return_type: "int".to_owned(),
        docstring: None,
    });
    let mut b = empty_module("pkg.b", true);
    b.functions.push(FunctionIr {
        name: "pkg.b.bar".to_owned(),
        kind: FunctionKindIr::Function,
        params: vec![],
        return_type: "int".to_owned(),
        docstring: None,
    });
    let ir = SchemaIr {
        modules: vec![a, b],
    };

    let generated = Codegen::new(Config::default())
        .modules_from_ir(ir)
        .unwrap()
        .generate_with_report()
        .unwrap();
    let tokens = generated.tokens.to_string();

    assert_eq!(
        tokens.matches("mod pkg ").count(),
        1,
        "the shared `pkg` root must be emitted exactly once: {tokens}"
    );
    // Both siblings survived the merge.
    assert!(tokens.contains("mod a "), "missing merged submodule `a`");
    assert!(tokens.contains("mod b "), "missing merged submodule `b`");
    assert!(
        generated
            .report
            .modules
            .iter()
            .filter(|module| ["pkg", "pkg.a", "pkg.b"].contains(&module.name.as_str()))
            .all(|module| module.input_kind == "external_ir"),
        "synthetic and authored modules must retain external-IR provenance: {:?}",
        generated.report.modules
    );
}

#[test]
fn synthetic_module_path_cannot_collide_with_authored_definition() {
    let mut parent = empty_module("pkg.child", true);
    parent.functions.push(FunctionIr {
        name: "pkg.child.foo".to_owned(),
        kind: FunctionKindIr::Function,
        params: vec![],
        return_type: "int".to_owned(),
        docstring: None,
    });
    let descendant = empty_module("pkg.child.foo.deep", false);

    let error = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![parent, descendant],
        })
        .expect("external IR registration must defer schema validation")
        .generate()
        .unwrap_err();

    assert!(
        error.to_string().contains("pkg.child.foo")
            && error
                .to_string()
                .contains("canonical external IR module path"),
        "canonicalization must not create a module over an authored definition: {error}"
    );
}

#[test]
fn synthetic_module_path_cannot_collide_with_emitted_import_alias() {
    let other = empty_module("other", false);
    let mut pkg = empty_module("pkg", true);
    pkg.imports.push(ImportIr {
        origin: "other".to_owned(),
        target: "pkg.foo".to_owned(),
    });
    let descendant = empty_module("pkg.foo.deep", false);

    let error = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![other, pkg, descendant],
        })
        .expect("external IR registration must defer schema validation")
        .generate()
        .unwrap_err();

    assert!(
        error.to_string().contains("pkg.foo")
            && error.to_string().contains("canonical emitted module path"),
        "canonicalization must not create a module over an emitted alias: {error}"
    );
}

#[test]
fn disabled_side_of_synthetic_module_collision_is_allowed() {
    fn schema() -> SchemaIr {
        let mut pkg = empty_module("pkg", true);
        pkg.functions.push(FunctionIr {
            name: "pkg.foo".to_owned(),
            kind: FunctionKindIr::Function,
            params: vec![],
            return_type: "int".to_owned(),
            docstring: None,
        });
        SchemaIr {
            modules: vec![pkg, empty_module("pkg.foo.deep", false)],
        }
    }

    let no_traversal = Codegen::new(Config::builder().traverse_submodules(false).build())
        .modules_from_ir(schema())
        .unwrap()
        .generate()
        .unwrap()
        .to_string();
    assert!(no_traversal.contains("fn foo"), "{no_traversal}");
    assert!(!no_traversal.contains("mod deep"), "{no_traversal}");

    let no_functions = Codegen::new(Config::builder().generate_functions(false).build())
        .modules_from_ir(schema())
        .unwrap()
        .generate()
        .unwrap()
        .to_string();
    assert!(!no_functions.contains("fn foo"), "{no_functions}");
    assert!(no_functions.contains("mod foo"), "{no_functions}");
}

#[test]
fn sibling_package_reexport_is_emitted_and_compiles() {
    let mut a = empty_module("pkg.a", false);
    a.classes.push(class("pkg.a.Widget", vec![]));
    let mut b = empty_module("pkg.b", false);
    b.imports.push(ImportIr {
        origin: "pkg.a.Widget".to_owned(),
        target: "pkg.b.WidgetAlias".to_owned(),
    });
    let mut root = empty_module("pkg", true);
    root.submodules = vec![a, b];

    let tokens = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![root],
        })
        .unwrap()
        .generate()
        .unwrap();
    let generated = prettyplease::unparse(&syn::parse2(tokens).unwrap());

    assert!(
        generated.contains("pub use super::a::Widget as WidgetAlias;"),
        "sibling package reexport must not be dropped: {generated}"
    );
    assert_ir_source_compiles(&generated, "sibling_package_reexport");
}

#[test]
fn cross_root_external_ir_reexport_is_emitted_and_compiles() {
    let mut other = empty_module("other", true);
    other.classes.push(class("other.Widget", vec![]));

    let mut pkg = empty_module("pkg", true);
    pkg.imports.push(ImportIr {
        origin: "other.Widget".to_owned(),
        target: "pkg.WidgetAlias".to_owned(),
    });
    pkg.functions.push(FunctionIr {
        name: "pkg.make".to_owned(),
        kind: FunctionKindIr::Function,
        params: vec![],
        return_type: "WidgetAlias".to_owned(),
        docstring: None,
    });

    let tokens = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![other, pkg],
        })
        .unwrap()
        .generate()
        .unwrap();
    let generated = prettyplease::unparse(&syn::parse2(tokens).unwrap());

    assert!(
        generated.contains("pub use super::other::Widget as WidgetAlias;"),
        "cross-root reexport must resolve against every generated root: {generated}"
    );
    assert!(
        generated.contains("PyResult<WidgetAlias>"),
        "unqualified alias annotations must resolve to the reexport: {generated}"
    );
    assert_ir_source_compiles(&generated, "cross_root_reexport");
}

#[test]
fn cross_root_external_ir_reexport_can_appear_in_prelude() {
    let mut other = empty_module("other", true);
    other.classes.push(class("other.Widget", vec![]));

    let mut pkg = empty_module("pkg", true);
    pkg.imports.push(ImportIr {
        origin: "other.Widget".to_owned(),
        target: "pkg.WidgetAlias".to_owned(),
    });
    pkg.prelude.push("WidgetAlias".to_owned());

    let tokens = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![other, pkg],
        })
        .unwrap()
        .generate()
        .unwrap();
    let generated = prettyplease::unparse(&syn::parse2(tokens).unwrap());

    assert!(
        generated.contains("pub use super::other::Widget as WidgetAlias;"),
        "cross-root reexport must be emitted: {generated}"
    );
    assert!(
        generated.contains("pub use super::WidgetAlias;"),
        "prelude must reexport the resolved import alias: {generated}"
    );
    assert_ir_source_compiles(&generated, "cross_root_reexport_prelude");
}

#[test]
fn chained_cross_root_external_ir_reexport_is_emitted_typed_and_compiles() {
    let mut other = empty_module("other", true);
    other.classes.push(class("other.Widget", vec![]));

    let mut pkg = empty_module("pkg", true);
    pkg.imports.push(ImportIr {
        origin: "other.Widget".to_owned(),
        target: "pkg.Widget".to_owned(),
    });

    let mut app = empty_module("app", true);
    app.imports.push(ImportIr {
        origin: "pkg.Widget".to_owned(),
        target: "app.WidgetAlias".to_owned(),
    });
    app.prelude.push("WidgetAlias".to_owned());
    app.functions.push(FunctionIr {
        name: "app.make".to_owned(),
        kind: FunctionKindIr::Function,
        params: vec![],
        return_type: "WidgetAlias".to_owned(),
        docstring: None,
    });

    let generated = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![other, pkg, app],
        })
        .unwrap()
        .generate_with_report()
        .unwrap();
    let source = prettyplease::unparse(&syn::parse2(generated.tokens).unwrap());

    assert!(
        source.contains("pub use super::other::Widget as WidgetAlias;"),
        "the chained alias must resolve to its canonical emitted origin: {source}"
    );
    assert!(
        source.contains("PyResult<WidgetAlias>"),
        "the chained alias must populate local type resolution: {source}"
    );
    assert!(
        source.contains("pub use super::WidgetAlias;"),
        "the prelude must include the chained alias: {source}"
    );
    assert!(
        generated
            .report
            .diagnostics
            .iter()
            .all(|diagnostic| diagnostic.code != "import_origin_unresolved"),
        "a resolved chain must not report an unresolved import: {:?}",
        generated.report.diagnostics
    );
    assert_ir_source_compiles(&source, "chained_cross_root_reexport");
}

#[test]
fn chained_module_alias_prefix_resolves_members_and_compiles() {
    let mut types = empty_module("other.types", false);
    types.classes.push(class("other.types.Widget", vec![]));
    let mut other = empty_module("other", true);
    other.submodules.push(types);

    let mut pkg = empty_module("pkg", true);
    pkg.imports.push(ImportIr {
        origin: "other.types".to_owned(),
        target: "pkg.types_alias".to_owned(),
    });

    let mut app = empty_module("app", true);
    app.imports.push(ImportIr {
        origin: "pkg.types_alias.Widget".to_owned(),
        target: "app.Widget".to_owned(),
    });
    app.prelude.push("Widget".to_owned());
    app.functions.push(FunctionIr {
        name: "app.make".to_owned(),
        kind: FunctionKindIr::Function,
        params: vec![],
        return_type: "Widget".to_owned(),
        docstring: None,
    });

    let tokens = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![other, pkg, app],
        })
        .unwrap()
        .generate()
        .unwrap();
    let source = prettyplease::unparse(&syn::parse2(tokens).unwrap());

    assert!(
        source.contains("pub use super::other::types::Widget;"),
        "a member reached through a module alias prefix must resolve canonically: {source}"
    );
    assert!(
        source.contains("PyResult<Widget>"),
        "the chained member alias must remain typed: {source}"
    );
    assert_ir_source_compiles(&source, "chained_module_alias_prefix");
}

#[test]
fn qualified_import_alias_is_visible_to_other_modules() {
    let mut other = empty_module("other", true);
    other.classes.push(class("other.Widget", vec![]));
    let mut pkg = empty_module("pkg", true);
    pkg.imports.push(ImportIr {
        origin: "other.Widget".to_owned(),
        target: "pkg.WidgetAlias".to_owned(),
    });
    let mut app = empty_module("app", true);
    app.functions.push(FunctionIr {
        name: "app.consume".to_owned(),
        kind: FunctionKindIr::Function,
        params: vec![ParameterIr {
            name: "widget".to_owned(),
            kind: ParameterKindIr::PositionalOrKeyword,
            annotation: "pkg.WidgetAlias".to_owned(),
            has_default: false,
        }],
        return_type: "pkg.WidgetAlias".to_owned(),
        docstring: None,
    });

    let tokens = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![other, pkg, app],
        })
        .unwrap()
        .generate()
        .unwrap();
    let source = prettyplease::unparse(&syn::parse2(tokens).unwrap());

    assert!(
        source.contains("p_widget: &super::other::Widget")
            && source.contains("PyResult<super::other::Widget>"),
        "qualified aliases must resolve globally instead of degrading to PyAny: {source}"
    );
    assert_ir_source_compiles(&source, "qualified_import_alias");
}

#[test]
fn synthetic_dotted_module_root_can_be_imported_without_traversal() {
    let child = empty_module("pkg.child", false);
    let mut app = empty_module("app", true);
    app.imports.push(ImportIr {
        origin: "pkg".to_owned(),
        target: "app.pkg_alias".to_owned(),
    });
    app.prelude.push("pkg_alias".to_owned());

    let tokens = Codegen::new(Config::builder().traverse_submodules(false).build())
        .modules_from_ir(SchemaIr {
            modules: vec![child, app],
        })
        .unwrap()
        .generate()
        .unwrap();
    let source = prettyplease::unparse(&syn::parse2(tokens).unwrap());

    assert!(
        source.contains("pub use super::pkg as pkg_alias;"),
        "the canonical synthetic root must be a resolvable emitted module: {source}"
    );
    assert!(
        !source.contains("pub mod child"),
        "disabled traversal must still hide the authored child: {source}"
    );
    assert_ir_source_compiles(&source, "synthetic_root_import");
}

#[test]
fn external_ir_import_cycle_is_rejected_cleanly() {
    let mut pkg = empty_module("pkg", true);
    pkg.imports.push(ImportIr {
        origin: "other.Widget".to_owned(),
        target: "pkg.Widget".to_owned(),
    });
    let mut other = empty_module("other", true);
    other.imports.push(ImportIr {
        origin: "pkg.Widget".to_owned(),
        target: "other.Widget".to_owned(),
    });

    let error = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![pkg, other],
        })
        .expect("external IR registration must defer schema validation")
        .generate()
        .unwrap_err();

    assert!(
        error.to_string().contains("import cycle"),
        "cycles must return a codegen error rather than recurse indefinitely: {error}"
    );
}

#[test]
fn prefixed_external_ir_import_cycle_is_rejected_cleanly() {
    let mut pkg = empty_module("pkg", true);
    pkg.imports.push(ImportIr {
        origin: "other.types.Widget".to_owned(),
        target: "pkg.Widget".to_owned(),
    });
    let mut other = empty_module("other", true);
    other.imports.push(ImportIr {
        origin: "pkg.Widget".to_owned(),
        target: "other.types".to_owned(),
    });

    let error = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![pkg, other],
        })
        .expect("external IR registration must defer schema validation")
        .generate()
        .unwrap_err();

    assert!(
        error.to_string().contains("import cycle"),
        "prefix rewrites must detect cycles without unbounded recursion: {error}"
    );
}

#[test]
fn import_cycle_is_inert_when_import_generation_is_disabled() {
    let mut pkg = empty_module("pkg", true);
    pkg.imports.push(ImportIr {
        origin: "other.Widget".to_owned(),
        target: "pkg.Widget".to_owned(),
    });
    let mut other = empty_module("other", true);
    other.imports.push(ImportIr {
        origin: "pkg.Widget".to_owned(),
        target: "other.Widget".to_owned(),
    });

    let tokens = Codegen::new(Config::builder().generate_imports(false).build())
        .modules_from_ir(SchemaIr {
            modules: vec![pkg, other],
        })
        .unwrap()
        .generate()
        .unwrap()
        .to_string();

    assert!(!tokens.contains("pub use"), "{tokens}");
}

#[test]
fn external_ir_import_chain_has_a_clean_depth_limit() {
    let mut pkg = empty_module("pkg", true);
    pkg.imports = (0..258)
        .map(|index| ImportIr {
            origin: format!("pkg.A{}", index + 1),
            target: format!("pkg.A{index}"),
        })
        .collect();

    let error = Codegen::default()
        .modules_from_ir(SchemaIr { modules: vec![pkg] })
        .expect("external IR registration must defer schema validation")
        .generate()
        .unwrap_err();

    assert!(
        error.to_string().contains("exceeds the supported depth"),
        "pathological alias chains must fail cleanly before stack exhaustion: {error}"
    );
}

#[test]
fn duplicate_enabled_import_target_is_rejected() {
    let mut other = empty_module("other", true);
    other
        .classes
        .extend([class("other.First", vec![]), class("other.Second", vec![])]);
    let mut pkg = empty_module("pkg", true);
    pkg.imports.extend([
        ImportIr {
            origin: "other.First".to_owned(),
            target: "pkg.Alias".to_owned(),
        },
        ImportIr {
            origin: "other.Second".to_owned(),
            target: "pkg.Alias".to_owned(),
        },
    ]);

    let error = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![other, pkg],
        })
        .expect("external IR registration must defer schema validation")
        .generate()
        .unwrap_err();

    assert!(
        error.to_string().contains("Alias"),
        "duplicate emitted aliases must be rejected deterministically: {error}"
    );
}

#[test]
fn enabled_import_can_supply_prelude_name_of_disabled_declaration() {
    let other = empty_module("other", true);
    let mut pkg = empty_module("pkg", true);
    pkg.classes.push(class("pkg.Widget", vec![]));
    pkg.imports.push(ImportIr {
        origin: "other".to_owned(),
        target: "pkg.Widget".to_owned(),
    });
    pkg.prelude.push("Widget".to_owned());

    let tokens = Codegen::new(Config::builder().generate_classes(false).build())
        .modules_from_ir(SchemaIr {
            modules: vec![other, pkg],
        })
        .unwrap()
        .generate()
        .unwrap();
    let source = prettyplease::unparse(&syn::parse2(tokens).unwrap());

    assert!(
        source.contains("pub use super::other as Widget;"),
        "the emitted import must occupy the name vacated by the disabled class: {source}"
    );
    assert!(
        source.contains("pub use super::Widget;"),
        "the prelude must reexport the emitted import alias: {source}"
    );
    assert!(
        !source.contains("pub struct Widget"),
        "the disabled class must remain disabled: {source}"
    );
    assert_ir_source_compiles(&source, "disabled_declaration_import_prelude");
}

#[test]
fn cross_root_import_alias_cannot_collide_with_local_class() {
    let mut other = empty_module("other", true);
    other.classes.push(class("other.Widget", vec![]));

    let mut pkg = empty_module("pkg", true);
    pkg.classes.push(class("pkg.WidgetAlias", vec![]));
    pkg.imports.push(ImportIr {
        origin: "other.Widget".to_owned(),
        target: "pkg.WidgetAlias".to_owned(),
    });

    let error = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![other, pkg],
        })
        .expect("external IR registration must defer schema validation")
        .generate()
        .unwrap_err();

    assert!(
        error.to_string().contains("WidgetAlias"),
        "cross-root import aliases must participate in Rust type collision validation: {error}"
    );
}

#[test]
fn prelude_import_from_unemitted_dotted_root_is_rejected() {
    let mut hidden = empty_module("pkg.child", false);
    hidden.classes.push(class("pkg.child.Widget", vec![]));

    let mut visible = empty_module("visible", true);
    visible.imports.push(ImportIr {
        origin: "pkg.child.Widget".to_owned(),
        target: "visible.WidgetAlias".to_owned(),
    });
    visible.prelude.push("WidgetAlias".to_owned());

    let error = Codegen::new(Config::builder().traverse_submodules(false).build())
        .modules_from_ir(SchemaIr {
            modules: vec![hidden, visible],
        })
        .expect("external IR registration must defer schema validation")
        .generate()
        .unwrap_err();

    assert!(
        error.to_string().contains("unresolved or disabled origin"),
        "a prelude cannot expose an import whose dotted root is not emitted: {error}"
    );
}

#[test]
fn unresolved_cross_root_external_ir_reexport_warns() {
    let mut pkg = empty_module("pkg", true);
    pkg.imports.push(ImportIr {
        origin: "missing.Widget".to_owned(),
        target: "pkg.WidgetAlias".to_owned(),
    });

    let generated = Codegen::default()
        .modules_from_ir(SchemaIr { modules: vec![pkg] })
        .unwrap()
        .generate_with_report()
        .unwrap();

    assert!(
        generated
            .report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "import_origin_unresolved"),
        "an explicit IR import must not disappear without a diagnostic"
    );
}

#[test]
fn authored_parent_metadata_survives_dotted_child_in_either_order() {
    let child_first =
        generate_authored_parent_with_dotted_child(true, "Authored package documentation.", "int");
    let parent_first =
        generate_authored_parent_with_dotted_child(false, "Authored package documentation.", "int");
    let root_hash = |generated: &pyo3_bindgen_engine::GeneratedBindings| {
        let root = generated
            .report
            .modules
            .iter()
            .find(|module| module.name == "pkg")
            .unwrap();
        assert_eq!(root.input_kind, "external_ir");
        assert!(generated
            .tokens
            .to_string()
            .contains("Authored package documentation."));
        assert!(generated.report.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "function_self_parameter_renamed"
                && diagnostic.path.as_deref() == Some("pkg.root_value")
        }));
        root.input_hash.clone()
    };

    assert_eq!(root_hash(&child_first), root_hash(&parent_first));
}

#[test]
fn merged_parent_hash_tracks_authored_and_dotted_child_input() {
    let root_hash = |child_first, parent_docstring, child_return_type| {
        generate_authored_parent_with_dotted_child(child_first, parent_docstring, child_return_type)
            .report
            .modules
            .into_iter()
            .find(|module| module.name == "pkg")
            .unwrap()
            .input_hash
    };

    for child_first in [false, true] {
        let baseline = root_hash(child_first, "Authored package documentation.", "int");
        assert_ne!(
            baseline,
            root_hash(child_first, "Changed authored documentation.", "int"),
            "merged root hash must track authored-parent input"
        );
        assert_ne!(
            baseline,
            root_hash(child_first, "Authored package documentation.", "str"),
            "merged root hash must track separately supplied child input"
        );
    }

    assert_eq!(
        root_hash(false, "Authored package documentation.", "int"),
        root_hash(true, "Authored package documentation.", "int"),
        "semantically identical merged trees must hash identically in either registration order"
    );
}

#[test]
fn merged_parent_hash_tracks_dotted_child_order() {
    let root_hash = |child_names: [&str; 2]| {
        let modules = child_names
            .into_iter()
            .map(|name| empty_module(&format!("pkg.{name}"), false))
            .collect();
        Codegen::default()
            .modules_from_ir(SchemaIr { modules })
            .unwrap()
            .generate_with_report()
            .unwrap()
            .report
            .modules
            .into_iter()
            .find(|module| module.name == "pkg")
            .unwrap()
            .input_hash
    };

    assert_ne!(
        root_hash(["alpha", "beta"]),
        root_hash(["beta", "alpha"]),
        "merged root hash must preserve caller-supplied sibling order"
    );
}

/// A class supplying only its direct base must still receive an empty impl for every
/// transitive ancestor's `Methods` trait (otherwise the supertrait bound is
/// unsatisfied — `E0277`).
#[test]
fn direct_bases_expand_to_full_ancestor_impls() {
    let mut module = empty_module("pkg", true);
    module.classes = vec![
        class("pkg.A", vec![]),
        class("pkg.B", vec!["pkg.A".to_owned()]),
        // Only the direct base is listed; the engine must complete the closure.
        class("pkg.C", vec!["pkg.B".to_owned()]),
    ];
    let ir = SchemaIr {
        modules: vec![module],
    };

    let tokens = Codegen::new(Config::default())
        .modules_from_ir(ir)
        .unwrap()
        .generate()
        .unwrap()
        .to_string();

    assert!(
        tokens.contains("BMethods for C"),
        "missing direct-base impl for C: {tokens}"
    );
    assert!(
        tokens.contains("AMethods for C"),
        "transitive ancestor impl (A) was not emitted for C: {tokens}"
    );
}

#[test]
fn multiple_inheritance_exposes_every_direct_base_trait() {
    let mut module = empty_module("pkg", true);
    module.classes = vec![
        class("pkg.Left", vec![]),
        class("pkg.Right", vec![]),
        class(
            "pkg.Child",
            vec!["pkg.Left".to_owned(), "pkg.Right".to_owned()],
        ),
    ];

    let tokens = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .unwrap()
        .generate()
        .unwrap()
        .to_string();

    assert!(
        tokens.contains("trait ChildMethods : LeftMethods + RightMethods"),
        "ChildMethods must expose both direct base interfaces: {tokens}",
    );
}

/// One `self`-receiver instance method for the forwarder-shape tests below.
fn instance_method_ir(qualified_name: &str, extra_params: &[&str]) -> FunctionIr {
    let mut params = vec![ParameterIr {
        name: "self".to_owned(),
        kind: ParameterKindIr::PositionalOrKeyword,
        annotation: String::new(),
        has_default: false,
    }];
    params.extend(extra_params.iter().map(|param| ParameterIr {
        name: (*param).to_owned(),
        kind: ParameterKindIr::PositionalOrKeyword,
        annotation: "int".to_owned(),
        has_default: false,
    }));
    FunctionIr {
        name: qualified_name.to_owned(),
        kind: FunctionKindIr::Method {
            method_type: MethodKindIr::Instance,
        },
        params,
        return_type: "int".to_owned(),
        docstring: None,
    }
}

/// A same-named override must (a) leave the ancestor's per-type impl EMPTY — the
/// broken body-splice is gone — and (b) carry an inherent forwarder that delegates to
/// the class's own `Methods` trait via UFCS. The inherent method is what makes the
/// name unambiguous for a caller importing both traits.
#[test]
fn override_emits_empty_ancestor_impl_and_inherent_forwarder() {
    let mut module = empty_module("pkg", true);
    let mut a = class("pkg.A", vec![]);
    a.methods = vec![instance_method_ir("pkg.A.foo", &["a"])];
    let mut b = class("pkg.B", vec!["pkg.A".to_owned()]);
    b.methods = vec![instance_method_ir("pkg.B.foo", &["a"])];
    module.classes = vec![a, b];

    let tokens = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .unwrap()
        .generate()
        .unwrap()
        .to_string();

    assert!(
        tokens.contains("impl AMethods for B { }"),
        "the ancestor impl must be empty (no body splice): {tokens}",
    );
    assert!(
        tokens.contains("< Self as BMethods > :: foo"),
        "B must carry an inherent forwarder delegating to its own trait: {tokens}",
    );
}

/// A diamond where two ancestors declare the same name, and the child declares none,
/// must synthesize an inherent forwarder on the child delegating to the NEAREST
/// ancestor's trait (MRO order).
#[test]
fn diamond_synthesizes_inherent_forwarder_to_nearest_ancestor() {
    let mut module = empty_module("pkg", true);
    let mut p1 = class("pkg.P1", vec![]);
    p1.methods = vec![instance_method_ir("pkg.P1.bar", &["a"])];
    let mut p2 = class("pkg.P2", vec![]);
    p2.methods = vec![instance_method_ir("pkg.P2.bar", &["a"])];
    let j = class("pkg.J", vec!["pkg.P1".to_owned(), "pkg.P2".to_owned()]);
    module.classes = vec![p1, p2, j];

    let tokens = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .unwrap()
        .generate()
        .unwrap()
        .to_string();

    // P1 forwards `bar` to itself (rule (a)); J forwards the inherited `bar` to its
    // nearest ancestor P1 (rule (b)) -> two `P1Methods::bar` delegations. P2 only
    // forwards `bar` to itself -> exactly one `P2Methods::bar` delegation. If J wrongly
    // picked the farther ancestor P2, the P2 count would be two.
    assert_eq!(
        tokens.matches("< Self as P1Methods > :: bar").count(),
        2,
        "J must forward the inherited 'bar' to its nearest ancestor P1: {tokens}",
    );
    assert_eq!(
        tokens.matches("< Self as P2Methods > :: bar").count(),
        1,
        "only the nearest ancestor wins the forwarder (P2 keeps only its own): {tokens}",
    );
}

/// A non-linear diamond: `XD(XB, XC)`, `XB(X)`, `XC(X)` where `XC` overrides `foo`.
/// Python's C3 MRO is `[XD, XB, XC, X]`, so `XD.foo` resolves to `XC.foo`. A DFS pre-
/// order of the bases would instead reach `X` before `XC` and bind the inherited
/// forwarder to the wrong (farther) ancestor -- a silent, still-compiling mis-binding.
#[test]
fn nonlinear_diamond_forwarder_targets_c3_nearest_not_dfs() {
    let mut module = empty_module("pkg", true);
    let mut x = class("pkg.X", vec![]);
    x.methods = vec![instance_method_ir("pkg.X.foo", &["a"])];
    let xb = class("pkg.XB", vec!["pkg.X".to_owned()]);
    let mut xc = class("pkg.XC", vec!["pkg.X".to_owned()]);
    xc.methods = vec![instance_method_ir("pkg.XC.foo", &["a"])];
    let xd = class("pkg.XD", vec!["pkg.XB".to_owned(), "pkg.XC".to_owned()]);
    module.classes = vec![x, xb, xc, xd];

    let tokens = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .unwrap()
        .generate()
        .unwrap()
        .to_string();

    // XC forwards `foo` to itself (rule a); XD forwards the inherited `foo` to its C3-
    // nearest declarer XC (rule b) -> two `XCMethods::foo`. X forwards `foo` to itself
    // only -> one `XMethods::foo`. A DFS winner would flip these counts (picking X).
    assert_eq!(
        tokens.matches("< Self as XCMethods > :: foo").count(),
        2,
        "XD must forward inherited 'foo' to its C3-nearest ancestor XC: {tokens}",
    );
    assert_eq!(
        tokens.matches("< Self as XMethods > :: foo").count(),
        1,
        "the farther ancestor X must not win the diamond forwarder: {tokens}",
    );
}

/// A constructor (`__init__` -> inherent `pub fn new`) whose class also inherits a
/// method literally named `new` from two ancestors must NOT emit a second inherent
/// `new` (the rule-(b) diamond forwarder) -- that is `E0201`. The constructor's Rust
/// ident (`new`) differs from its Python name (`__init__`), so the collision is caught
/// by the emitted-ident set, not the Python-name `owns_name` check.
#[test]
fn constructor_new_does_not_duplicate_inherited_new_forwarder() {
    let mut module = empty_module("pkg", true);
    let mut n1 = class("pkg.N1", vec![]);
    n1.methods = vec![instance_method_ir("pkg.N1.new", &["a"])];
    let mut n2 = class("pkg.N2", vec![]);
    n2.methods = vec![instance_method_ir("pkg.N2.new", &["a"])];
    let mut nc = class("pkg.NC", vec!["pkg.N1".to_owned(), "pkg.N2".to_owned()]);
    nc.methods = vec![FunctionIr {
        name: "pkg.NC.__init__".to_owned(),
        kind: FunctionKindIr::Method {
            method_type: MethodKindIr::Constructor,
        },
        params: vec![ParameterIr {
            name: "self".to_owned(),
            kind: ParameterKindIr::PositionalOrKeyword,
            annotation: String::new(),
            has_default: false,
        }],
        return_type: String::new(),
        docstring: None,
    }];
    module.classes = vec![n1, n2, nc];

    let tokens = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .unwrap()
        .generate()
        .unwrap()
        .to_string();

    assert!(
        tokens.contains("pub fn new"),
        "NC must still emit its constructor: {tokens}",
    );
    // Only N1's OWN rule-(a) forwarder delegates to `N1Methods::new`; NC must NOT add a
    // second (rule-b) one, or the two inherent `new` methods collide (E0201).
    assert_eq!(
        tokens.matches("< Self as N1Methods > :: new").count(),
        1,
        "the constructor must suppress NC's rule-(b) 'new' forwarder (else E0201): {tokens}",
    );
}

/// A member named `downcast` collides with the generic `PyObjectBase::downcast`
/// implemented for every newtype. On the external-IR path it must be renamed (to
/// `py_downcast`) so it neither shadows the generic downcast nor makes an inherited
/// call ambiguous, while the generic downcast itself is still emitted.
#[test]
fn member_named_downcast_is_renamed_off_the_root_trait_api() {
    let mut module = empty_module("pkg", true);
    let mut d = class("pkg.Down", vec![]);
    d.methods = vec![instance_method_ir("pkg.Down.downcast", &["a"])];
    module.classes = vec![d];

    let tokens = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .unwrap()
        .generate()
        .unwrap()
        .to_string();

    assert!(
        tokens.contains("py_downcast"),
        "a 'downcast' member must be renamed to 'py_downcast': {tokens}",
    );
    assert!(
        tokens.contains("fn downcast < U"),
        "the generic PyObjectBase::downcast must still be emitted: {tokens}",
    );
}

/// An `IntEnum` whose member value exceeds `i64::MAX` must compile: the mirror carries
/// an explicit `#[repr(u64)]` so the discriminant literal does not overflow the
/// default `isize` discriminant type.
#[test]
fn int_enum_large_nonnegative_value_gets_repr_u64() {
    let mut module = empty_module("pkg", true);
    module.enums = vec![EnumIr {
        name: "pkg.Flags".to_owned(),
        kind: EnumKindIr::IntEnum,
        variants: vec![
            EnumVariantIr {
                py_name: "SMALL".to_owned(),
                value_repr: "1".to_owned(),
            },
            EnumVariantIr {
                py_name: "BIG".to_owned(),
                value_repr: "18446744073709551615".to_owned(),
            },
        ],
        aliases: vec![],
        docstring: None,
        is_flag: false,
        is_open: false,
        string_backed: false,
        plain_mirror: false,
    }];

    let tokens = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .unwrap()
        .generate()
        .unwrap()
        .to_string();

    assert!(
        tokens.contains("repr (u64)"),
        "an all-nonnegative IntEnum above i64::MAX must carry #[repr(u64)]: {tokens}",
    );
    assert!(
        tokens.contains("= 18446744073709551615"),
        "the large member value must be mirrored as an explicit discriminant: {tokens}",
    );
}

/// A negative member and a member above `i64::MAX` cannot share an MSRV-supported
/// integer enum representation. The mirror must degrade as one unit instead of using
/// unstable `repr(i128)` or emitting only some discriminants.
#[test]
fn int_enum_mixed_signed_and_large_unsigned_values_degrades() {
    let mut module = empty_module("pkg", true);
    module.enums = vec![EnumIr {
        name: "pkg.MixedRange".to_owned(),
        kind: EnumKindIr::IntEnum,
        variants: vec![
            EnumVariantIr {
                py_name: "NEGATIVE".to_owned(),
                value_repr: "-1".to_owned(),
            },
            EnumVariantIr {
                py_name: "LARGE".to_owned(),
                value_repr: "18446744073709551615".to_owned(),
            },
        ],
        aliases: vec![],
        docstring: None,
        is_flag: false,
        is_open: false,
        string_backed: false,
        plain_mirror: false,
    }];

    let generated = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .unwrap()
        .generate_with_report()
        .unwrap();
    let tokens = generated.tokens.to_string();

    assert!(
        !tokens.contains("repr (i64)")
            && !tokens.contains("repr (u64)")
            && !tokens.contains("= - 1")
            && !tokens.contains("= 18446744073709551615"),
        "a range with no common stable repr must emit no repr or discriminants: {tokens}",
    );
    assert!(
        !tokens.contains("PartialOrd") && !tokens.contains(" Ord"),
        "declaration order must not impersonate Python integer ordering: {tokens}",
    );
    assert!(
        generated
            .report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "enum_intenum_ord_dropped"),
        "the loss of exact value ordering must be observable: {:?}",
        generated.report.diagnostics,
    );
}

/// An `IntEnum` with a non-integer / missing `value_repr` on any member emits NO
/// discriminants at all (all-or-nothing), so a partial set can never auto-assign a
/// colliding discriminant (E0081).
#[test]
fn int_enum_with_a_missing_value_repr_emits_no_discriminants() {
    let mut module = empty_module("pkg", true);
    module.enums = vec![EnumIr {
        name: "pkg.Mixed".to_owned(),
        kind: EnumKindIr::IntEnum,
        variants: vec![
            EnumVariantIr {
                py_name: "A".to_owned(),
                value_repr: "10".to_owned(),
            },
            EnumVariantIr {
                py_name: "B".to_owned(),
                value_repr: String::new(),
            },
        ],
        aliases: vec![],
        docstring: None,
        is_flag: false,
        is_open: false,
        string_backed: false,
        plain_mirror: false,
    }];

    let tokens = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .unwrap()
        .generate()
        .unwrap()
        .to_string();

    assert!(
        !tokens.contains("repr (i64)") && !tokens.contains("repr (u64)"),
        "a mixed-value IntEnum must not carry an integer repr: {tokens}",
    );
    assert!(
        !tokens.contains("= 10"),
        "a mixed-value IntEnum must emit no discriminants (all-or-nothing): {tokens}",
    );
    assert!(
        !tokens.contains("PartialOrd"),
        "without value discriminants, an IntEnum must not derive `Ord` -- it would silently follow declaration order and disagree with Python: {tokens}",
    );
}

/// Two canonical `IntEnum` members with the SAME integer value cannot emit explicit
/// discriminants -- `enum { A = 5, B = 5 }` is E0081. The all-or-nothing
/// gate must reject on non-uniqueness, not just non-parseability, dropping discriminants
/// and the value-mirroring `Ord` entirely.
#[test]
fn int_enum_duplicate_values_emit_no_discriminants_and_no_ord() {
    let mut module = empty_module("pkg", true);
    module.enums = vec![EnumIr {
        name: "pkg.Dup".to_owned(),
        kind: EnumKindIr::IntEnum,
        variants: vec![
            EnumVariantIr {
                py_name: "A".to_owned(),
                value_repr: "5".to_owned(),
            },
            EnumVariantIr {
                py_name: "B".to_owned(),
                value_repr: "5".to_owned(),
            },
        ],
        aliases: vec![],
        docstring: None,
        is_flag: false,
        is_open: false,
        string_backed: false,
        plain_mirror: false,
    }];

    let tokens = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .unwrap()
        .generate()
        .unwrap()
        .to_string();

    assert!(
        !tokens.contains("repr (i64)") && !tokens.contains("repr (u64)"),
        "a duplicate-value IntEnum must not carry an integer repr: {tokens}",
    );
    assert!(
        !tokens.contains("= 5"),
        "duplicate values must not be emitted as discriminants (E0081): {tokens}",
    );
    assert!(
        !tokens.contains("PartialOrd"),
        "without discriminants, no `Ord` may be derived: {tokens}",
    );
}

/// A `value_repr` such as "+5" parses as an integer but is not a valid raw discriminant
/// token (`= + 5`). The discriminant must be the CANONICAL decimal of the parsed value.
#[test]
fn int_enum_plus_prefixed_value_emits_canonical_discriminant() {
    let mut module = empty_module("pkg", true);
    module.enums = vec![EnumIr {
        name: "pkg.Signed".to_owned(),
        kind: EnumKindIr::IntEnum,
        variants: vec![
            EnumVariantIr {
                py_name: "P".to_owned(),
                value_repr: "+5".to_owned(),
            },
            EnumVariantIr {
                py_name: "Q".to_owned(),
                value_repr: "10".to_owned(),
            },
        ],
        aliases: vec![],
        docstring: None,
        is_flag: false,
        is_open: false,
        string_backed: false,
        plain_mirror: false,
    }];

    let tokens = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .unwrap()
        .generate()
        .unwrap()
        .to_string();

    assert!(
        tokens.contains("= 5"),
        "the '+5' value must emit as the canonical discriminant `= 5`: {tokens}",
    );
    assert!(
        !tokens.contains("= + 5"),
        "the raw '+5' token is not a valid discriminant and must not be emitted: {tokens}",
    );
}

/// A metaclass-convention receiver (`mcs`/`mcls`/`metacls`) on an `Unknown`-kind IR
/// method is a class method too: the receiver must be stripped, not forwarded to Python
/// as a positional (which would raise `TypeError` at every call).
#[test]
fn unknown_method_ir_with_metacls_receiver_is_a_classmethod() {
    let mut widget = class("pkg.Widget", vec![]);
    widget.methods = vec![FunctionIr {
        name: "pkg.Widget.make".to_owned(),
        kind: FunctionKindIr::Method {
            method_type: MethodKindIr::Unknown,
        },
        params: vec![
            ParameterIr {
                name: "metacls".to_owned(),
                kind: ParameterKindIr::PositionalOrKeyword,
                annotation: String::new(),
                has_default: false,
            },
            ParameterIr {
                name: "value".to_owned(),
                kind: ParameterKindIr::PositionalOrKeyword,
                annotation: "int".to_owned(),
                has_default: false,
            },
        ],
        return_type: "int".to_owned(),
        docstring: None,
    }];
    let mut module = empty_module("pkg", true);
    module.classes = vec![widget];

    let tokens = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .unwrap()
        .generate()
        .unwrap()
        .to_string();

    assert!(
        tokens.contains("p_value"),
        "the real argument must survive: {tokens}",
    );
    assert!(
        !tokens.contains("p_metacls"),
        "the implicit 'metacls' receiver must be stripped, not forwarded to Python: {tokens}",
    );
    assert!(
        !tokens.contains("fn make < 'py > (& self"),
        "a metaclass classmethod is an associated function, not an instance method: {tokens}",
    );
}

/// A class-only IR uses no collection type, so `Codegen` emits no `PyCollection<T>`
/// wrapper -- and validation must NOT reserve that name, or a user class literally named
/// `PyCollection` would be falsely rejected. The validation and codegen reservation sites
/// must agree on "collection actually used".
#[test]
fn class_only_ir_with_type_named_pycollection_is_accepted() {
    let mut module = empty_module("pkg", true);
    module.classes = vec![class("pkg.PyCollection", vec![])];
    module.functions.push(FunctionIr {
        name: "pkg.make".to_owned(),
        kind: FunctionKindIr::Function,
        params: vec![],
        return_type: "PyCollection".to_owned(),
        docstring: None,
    });

    let tokens = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .expect("a class-only IR must not reserve the collection-wrapper name")
        .generate()
        .unwrap()
        .to_string();

    assert!(
        !tokens.contains("PyCollection < T"),
        "no collection is used, so the generic PyCollection<T> wrapper must not be emitted: {tokens}",
    );
    assert!(
        !tokens.contains("super :: PyCollection"),
        "the absent synthetic wrapper must not overwrite the user class's local alias: {tokens}",
    );
    assert_ir_source_compiles(&tokens, "user_pycollection_type");
}

#[test]
fn user_pycollection_type_and_generated_wrapper_do_not_alias() {
    let mut module = empty_module("pkg", true);
    module.classes = vec![class("pkg.PyCollection", vec![])];
    module.functions.extend([
        FunctionIr {
            name: "pkg.make_user".to_owned(),
            kind: FunctionKindIr::Function,
            params: vec![],
            return_type: "PyCollection".to_owned(),
            docstring: None,
        },
        FunctionIr {
            name: "pkg.make_items".to_owned(),
            kind: FunctionKindIr::Function,
            params: vec![],
            return_type: "Collection[int]".to_owned(),
            docstring: None,
        },
    ]);

    let tokens = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .unwrap()
        .generate()
        .unwrap();
    let source = prettyplease::unparse(&syn::parse2(tokens).unwrap());

    assert!(
        source.contains("PyResult<PyCollection>"),
        "the local user class must remain a non-generic return type: {source}"
    );
    assert!(
        source.contains("PyResult<super::PyCollection<i64>>"),
        "Collection[int] must still use the generated generic wrapper: {source}"
    );
    assert_ir_source_compiles(&source, "user_and_generated_pycollection");
}

#[test]
fn collection_substring_does_not_reserve_wrapper_name() {
    let mut module = empty_module("pkg", true);
    module.classes = vec![class("pkg.PyCollection", vec![])];
    module.functions.push(FunctionIr {
        name: "pkg.make".to_owned(),
        kind: FunctionKindIr::Function,
        params: vec![],
        return_type: "NotACollection[int]".to_owned(),
        docstring: None,
    });

    let tokens = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .expect("substring-only Collection spelling must not reserve PyCollection")
        .generate()
        .unwrap()
        .to_string();

    assert!(
        !tokens.contains("PyCollection < T"),
        "only a parsed Collection node may emit the wrapper: {tokens}"
    );
}

/// The external-IR collection wrapper is an opaque handle at extraction time. Its
/// operations perform the first protocol/element work, and only the exception that
/// corresponds to the operation's missing-item case is converted to `None`.
#[test]
fn external_ir_collection_wrapper_is_lazy_and_preserves_lookup_errors() {
    let mut module = empty_module("pkg", true);
    module.functions.push(FunctionIr {
        name: "pkg.collection".to_owned(),
        kind: FunctionKindIr::Function,
        params: vec![],
        return_type: "Collection[int]".to_owned(),
        docstring: None,
    });

    let generated = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .expect("Collection[int] must be accepted by external IR")
        .generate()
        .expect("the external-IR collection wrapper must generate")
        .to_string();

    let source = [
        generated.as_str(),
        r#"
#[cfg(test)]
mod collection_contract_tests {
    use super::PyCollection;
    use pyo3::types::{PyAnyMethods, PyDict, PyDictMethods, PyList, PyModule};

    #[test]
    fn collection_operations_are_lazy_and_narrowly_typed() -> pyo3::PyResult<()> {
        pyo3::Python::attach(|py| {
            let opaque = PyModule::from_code(
                py,
                pyo3::ffi::c_str!(
                    "class Opaque:\n    pass\n\nclass KeyedTypeError:\n    def __getitem__(self, key):\n        raise TypeError('custom keyed type error')\n\nclass KeyedIndexError:\n    def __getitem__(self, key):\n        raise IndexError('custom keyed index error')\n"
                ),
                pyo3::ffi::c_str!("collection_contract.py"),
                pyo3::ffi::c_str!("collection_contract"),
            )?;

            let opaque_object = opaque.getattr("Opaque")?.call0()?;
            let opaque_view = opaque_object.extract::<PyCollection<i64>>()?;
            let error = opaque_view
                .get(py, 0)
                .expect_err("a non-subscriptable object must fail on first operation");
            assert!(error.is_instance_of::<pyo3::exceptions::PyTypeError>(py));

            let empty_list = PyList::empty(py);
            let list_view = empty_list.as_any().extract::<PyCollection<i64>>()?;
            assert_eq!(list_view.get(py, 0)?, None);

            let empty_dict = PyDict::new(py);
            let dict_view = empty_dict.as_any().extract::<PyCollection<i64>>()?;
            assert_eq!(dict_view.get_by_key(py, "missing")?, None);
            let error = dict_view
                .get(py, 0)
                .expect_err("a dict integer lookup must preserve KeyError");
            assert!(error.is_instance_of::<pyo3::exceptions::PyKeyError>(py));

            let wrong_list = PyList::new(py, ["not an integer"])?;
            let wrong_list_view = wrong_list.as_any().extract::<PyCollection<i64>>()?;
            let error = wrong_list_view
                .get(py, 0)
                .expect_err("get must propagate a successful item lookup's extraction error");
            assert!(error.is_instance_of::<pyo3::exceptions::PyTypeError>(py));
            let error = wrong_list_view
                .iter(py)
                .expect_err("iter must propagate an element extraction error");
            assert!(error.is_instance_of::<pyo3::exceptions::PyTypeError>(py));

            let wrong_dict = PyDict::new(py);
            wrong_dict.set_item("present", "not an integer")?;
            let wrong_dict_view = wrong_dict.as_any().extract::<PyCollection<i64>>()?;
            let error = wrong_dict_view
                .get_by_key(py, "present")
                .expect_err("get_by_key must propagate a successful lookup's extraction error");
            assert!(error.is_instance_of::<pyo3::exceptions::PyTypeError>(py));

            let keyed_type_error = opaque.getattr("KeyedTypeError")?.call0()?;
            let keyed_type_error = keyed_type_error.extract::<PyCollection<i64>>()?;
            let error = keyed_type_error
                .get_by_key(py, "key")
                .expect_err("custom keyed TypeError must propagate");
            assert!(error.is_instance_of::<pyo3::exceptions::PyTypeError>(py));

            let keyed_index_error = opaque.getattr("KeyedIndexError")?.call0()?;
            let keyed_index_error = keyed_index_error.extract::<PyCollection<i64>>()?;
            let error = keyed_index_error
                .get_by_key(py, "key")
                .expect_err("custom keyed IndexError must propagate");
            assert!(error.is_instance_of::<pyo3::exceptions::PyIndexError>(py));

            Ok(())
        })
    }
}
"#,
    ]
    .join("\n");
    assert_ir_source_runs(&source, "collection_contract");
}

/// A base that resolves to an enum is not a class newtype: no `<Enum>Methods` trait
/// exists and no `#[repr(transparent)]` upcast is sound, so malformed ancestry must be
/// rejected rather than silently weakened.
#[test]
fn enum_base_is_rejected_at_ir_validation() {
    let mut module = empty_module("pkg", true);
    module.enums = vec![EnumIr {
        name: "pkg.Color".to_owned(),
        kind: EnumKindIr::Enum,
        variants: vec![EnumVariantIr {
            py_name: "RED".to_owned(),
            value_repr: "1".to_owned(),
        }],
        aliases: vec![],
        docstring: None,
        is_flag: false,
        is_open: false,
        string_backed: false,
        plain_mirror: false,
    }];
    module.classes = vec![class("pkg.Widget", vec!["pkg.Color".to_owned()])];
    let ir = SchemaIr {
        modules: vec![module],
    };

    let error = Codegen::new(Config::default())
        .modules_from_ir(ir)
        .expect("external IR registration must defer schema validation")
        .generate()
        .expect_err("an enum cannot satisfy a generated class-trait base");

    assert!(
        error
            .to_string()
            .contains("base 'pkg.Color' of class 'pkg.Widget' does not resolve to a class"),
        "unexpected error: {error}"
    );
}

#[test]
fn unsupported_flag_and_open_enums_are_rejected_in_external_ir() {
    for (is_flag, is_open) in [(true, false), (false, true), (true, true)] {
        let mut module = empty_module("pkg", true);
        module.enums.push(EnumIr {
            name: "pkg.Permission".to_owned(),
            kind: EnumKindIr::Enum,
            variants: vec![EnumVariantIr {
                py_name: "READ".to_owned(),
                value_repr: "1".to_owned(),
            }],
            aliases: vec![],
            docstring: None,
            is_flag,
            is_open,
            string_backed: false,
            plain_mirror: false,
        });

        let error = Codegen::default()
            .modules_from_ir(SchemaIr {
                modules: vec![module],
            })
            .expect("external IR registration must defer schema validation")
            .generate()
            .expect_err("unsupported enum semantics must not be silently ignored");
        assert!(
            error.to_string().contains("unsupported enum semantics"),
            "unexpected error: {error}",
        );
    }
}

#[test]
fn lenient_external_ir_emits_flag_and_open_enums_as_plain_mirrors() {
    for (is_flag, is_open) in [(true, false), (false, true), (true, true)] {
        let mut module = empty_module("pkg", true);
        module.enums.push(EnumIr {
            name: "pkg.Permission".to_owned(),
            kind: EnumKindIr::IntEnum,
            variants: vec![
                EnumVariantIr {
                    py_name: "READ".to_owned(),
                    value_repr: "1".to_owned(),
                },
                EnumVariantIr {
                    py_name: "WRITE".to_owned(),
                    value_repr: "2".to_owned(),
                },
            ],
            aliases: vec![],
            docstring: None,
            is_flag,
            is_open,
            string_backed: false,
            plain_mirror: false,
        });

        let tokens = Codegen::new(Config {
            lenient_external_ir: true,
            ..Config::default()
        })
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .expect("lenient IR must accept flag/open enum semantics")
        .generate()
        .expect("lenient flag/open enum must generate")
        .to_string();

        assert!(
            tokens.contains("enum Permission")
                && tokens.contains("Read")
                && tokens.contains("Write"),
            "a flag/open enum must lower to a plain mirror enum under lenient IR: {tokens}"
        );
    }
}

#[test]
fn lenient_external_ir_accepts_required_positional_after_default() {
    let mut module = empty_module("pkg", false);
    module.functions.push(FunctionIr {
        name: "pkg.run".to_owned(),
        kind: FunctionKindIr::Function,
        params: vec![
            ParameterIr {
                name: "first".to_owned(),
                kind: ParameterKindIr::PositionalOrKeyword,
                annotation: "int".to_owned(),
                has_default: true,
            },
            ParameterIr {
                name: "second".to_owned(),
                kind: ParameterKindIr::PositionalOrKeyword,
                annotation: "int".to_owned(),
                has_default: false,
            },
        ],
        return_type: "int".to_owned(),
        docstring: None,
    });

    let tokens = Codegen::new(Config {
        lenient_external_ir: true,
        ..Config::default()
    })
    .modules_from_ir(SchemaIr {
        modules: vec![module],
    })
    .expect("lenient IR must accept a required positional after a defaulted one")
    .generate()
    .expect("lenient required-after-default must generate")
    .to_string();

    assert!(
        tokens.contains("fn run"),
        "the function must lower normally under lenient IR: {tokens}"
    );

    // Leniency is surgical: it relaxes only the Python-signature quirks, not the
    // structural parameter grammar. A variadic with a default is still a hard error.
    let mut broken = empty_module("pkg", false);
    broken.functions.push(FunctionIr {
        name: "pkg.bad".to_owned(),
        kind: FunctionKindIr::Function,
        params: vec![ParameterIr {
            name: "args".to_owned(),
            kind: ParameterKindIr::VarPositional,
            annotation: String::new(),
            has_default: true,
        }],
        return_type: String::new(),
        docstring: None,
    });
    let error = Codegen::new(Config {
        lenient_external_ir: true,
        ..Config::default()
    })
    .modules_from_ir(SchemaIr {
        modules: vec![broken],
    })
    .expect("external IR registration must defer schema validation")
    .generate()
    .expect_err("lenient IR must still reject a variadic with a default");
    assert!(
        error
            .to_string()
            .contains("variadic parameter 'args' cannot have a default"),
        "unexpected error: {error}"
    );
}

/// A `plain_mirror: true` enum is an explicit producer sign-off to drop flag/open
/// semantics; strict external IR (the default, non-lenient config) must accept it and
/// lower it exactly like lenient mode lowers the same flag/open enum today.
#[test]
fn enum_ir_plain_mirror_accepts_flag_semantics_in_strict_mode() {
    let mut module = empty_module("pkg", true);
    module.enums.push(EnumIr {
        name: "pkg.Permission".to_owned(),
        kind: EnumKindIr::IntEnum,
        variants: vec![
            EnumVariantIr {
                py_name: "READ".to_owned(),
                value_repr: "1".to_owned(),
            },
            EnumVariantIr {
                py_name: "WRITE".to_owned(),
                value_repr: "2".to_owned(),
            },
        ],
        aliases: vec![],
        docstring: None,
        is_flag: true,
        is_open: false,
        string_backed: false,
        plain_mirror: true,
    });

    let tokens = Codegen::new(Config::default())
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .expect("plain_mirror must accept flag enum semantics under strict external IR")
        .generate()
        .expect("plain_mirror flag enum must generate")
        .to_string();

    assert!(
        tokens.contains("enum Permission") && tokens.contains("Read") && tokens.contains("Write"),
        "a plain_mirror flag enum must lower to a plain mirror enum under strict IR: {tokens}"
    );
}

/// The wave's target scenario, with all three per-enum fields composed at once: a flag enum
/// that a producer signs off as a plain mirror (`plain_mirror`) AND declares string-backed
/// (`string_backed`). Strict external IR must accept it, emit the string round-trip (the same
/// shape [`external_ir_string_backed_enum_round_trips_via_member_string`] asserts), and lower
/// it byte-identically to the lenient run of the very same document.
#[test]
fn enum_ir_string_backed_plain_mirror_flag_lowers_in_strict_mode() {
    let mut module = empty_module("pkg", false);
    module.enums.push(EnumIr {
        name: "pkg.Permission".to_owned(),
        kind: EnumKindIr::StrEnum,
        variants: vec![
            EnumVariantIr {
                py_name: "READ".to_owned(),
                value_repr: "'READ'".to_owned(),
            },
            EnumVariantIr {
                py_name: "WRITE".to_owned(),
                value_repr: "'WRITE'".to_owned(),
            },
        ],
        aliases: vec![],
        docstring: None,
        is_flag: true,
        is_open: false,
        string_backed: true,
        plain_mirror: true,
    });
    module.classes.push(ClassIr {
        name: "pkg.Object".to_owned(),
        base_classes: vec![],
        docstring: None,
        methods: vec![],
        properties: vec![PropertyIr {
            name: "permission".to_owned(),
            py_type: "pkg.Permission".to_owned(),
            setter_type: None,
            readonly: false,
            docstring: None,
            setter_docstring: None,
        }],
    });
    let schema = SchemaIr {
        modules: vec![module],
    };

    let generate = |lenient: bool| {
        Codegen::new(Config {
            lenient_external_ir: lenient,
            generate_provenance: false,
            ..Config::default()
        })
        .modules_from_ir(schema.clone())
        .expect("a string-backed plain_mirror flag enum must build")
        .generate()
        .expect("a string-backed plain_mirror flag enum must generate")
        .to_string()
    };
    let strict_tokens = generate(false);

    // It is a plain mirror enum, not a flag newtype.
    assert!(
        strict_tokens.contains("enum Permission")
            && strict_tokens.contains("Read")
            && strict_tokens.contains("Write"),
        "plain_mirror must lower the flag enum as a plain mirror enum: {strict_tokens}"
    );
    assert!(
        !strict_tokens.contains("pub struct Permission (u64)"),
        "plain_mirror must not emit the flag value newtype: {strict_tokens}"
    );
    // ...and it is string-backed: the same round-trip shape the global flag produces.
    assert!(
        strict_tokens.contains(":: pyo3 :: types :: PyString :: new (py , self . as_py_name ())"),
        "string-backed into_py_any must build a PyString from as_py_name: {strict_tokens}"
    );
    assert!(
        strict_tokens.contains(":: std :: string :: String , > (obj . as_any ())"),
        "string-backed extract must read a String straight off obj: {strict_tokens}"
    );
    assert!(
        strict_tokens.contains("Self :: from_py_name (& member_name)"),
        "string-backed extract must map the member string via from_py_name: {strict_tokens}"
    );
    assert!(
        !strict_tokens.contains("expected Python enum member of"),
        "string-backed enum must not emit the class-import is_instance guard: {strict_tokens}"
    );
    assert!(
        !strict_tokens.contains(":: std :: string :: String , > (& name_attr)"),
        "string-backed extract must not read `.name` off an enum member: {strict_tokens}"
    );

    assert_eq!(
        strict_tokens,
        generate(true),
        "strict and lenient must lower this document identically"
    );
}

/// Without `plain_mirror`, strict external IR keeps rejecting `is_flag`/`is_open`; the
/// error must still name the enum and now also mention `plain_mirror` as an escape hatch.
#[test]
fn enum_ir_flag_without_plain_mirror_still_rejected_in_strict_mode() {
    let mut module = empty_module("pkg", true);
    module.enums.push(EnumIr {
        name: "pkg.Permission".to_owned(),
        kind: EnumKindIr::Enum,
        variants: vec![EnumVariantIr {
            py_name: "READ".to_owned(),
            value_repr: "1".to_owned(),
        }],
        aliases: vec![],
        docstring: None,
        is_flag: true,
        is_open: false,
        string_backed: false,
        plain_mirror: false,
    });

    let error = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .expect("external IR registration must defer schema validation")
        .generate()
        .expect_err("is_flag without plain_mirror must still be rejected under strict IR");
    let message = error.to_string();
    assert!(
        message.contains("pkg.Permission"),
        "unexpected error: {message}"
    );
    assert!(
        message.contains("plain_mirror"),
        "unexpected error: {message}"
    );
}

/// `has_default` is emission-metadata-only and lowering is order-independent, so a
/// required positional parameter after a defaulted one is now accepted unconditionally --
/// strict external IR produces byte-identical output to the lenient run of the same
/// document.
#[test]
fn required_after_defaulted_param_accepted_in_strict_mode() {
    let mut module = empty_module("pkg", false);
    module.functions.push(FunctionIr {
        name: "pkg.run".to_owned(),
        kind: FunctionKindIr::Function,
        params: vec![
            ParameterIr {
                name: "first".to_owned(),
                kind: ParameterKindIr::PositionalOrKeyword,
                annotation: "int".to_owned(),
                has_default: true,
            },
            ParameterIr {
                name: "second".to_owned(),
                kind: ParameterKindIr::PositionalOrKeyword,
                annotation: "int".to_owned(),
                has_default: false,
            },
        ],
        return_type: "int".to_owned(),
        docstring: None,
    });

    let strict_tokens = Codegen::new(Config {
        generate_provenance: false,
        ..Config::default()
    })
    .modules_from_ir(SchemaIr {
        modules: vec![module.clone()],
    })
    .expect("strict IR must accept a required positional after a defaulted one")
    .generate()
    .expect("strict required-after-default must generate")
    .to_string();

    let lenient_tokens = Codegen::new(Config {
        lenient_external_ir: true,
        generate_provenance: false,
        ..Config::default()
    })
    .modules_from_ir(SchemaIr {
        modules: vec![module],
    })
    .expect("lenient IR must accept a required positional after a defaulted one")
    .generate()
    .expect("lenient required-after-default must generate")
    .to_string();

    assert_eq!(
        strict_tokens, lenient_tokens,
        "strict and lenient must lower this document identically"
    );
}

#[test]
fn cyclic_external_ir_inheritance_is_rejected() {
    let cases = [
        vec![class("pkg.A", vec!["pkg.A".to_owned()])],
        vec![
            class("pkg.A", vec!["pkg.B".to_owned()]),
            class("pkg.B", vec!["pkg.A".to_owned()]),
        ],
        vec![
            class("pkg.A", vec!["pkg.B".to_owned()]),
            class("pkg.B", vec!["pkg.C".to_owned()]),
            class("pkg.C", vec!["pkg.A".to_owned()]),
        ],
    ];

    for classes in cases {
        let mut module = empty_module("pkg", true);
        module.classes = classes;
        let error = Codegen::default()
            .modules_from_ir(SchemaIr {
                modules: vec![module],
            })
            .expect("external IR registration must defer schema validation")
            .generate()
            .expect_err("inheritance cycles must be rejected before trait generation");
        assert!(
            error.to_string().contains("inheritance cycle"),
            "unexpected error: {error}",
        );
    }
}

#[test]
fn inconsistent_c3_external_ir_inheritance_is_rejected() {
    let mut module = empty_module("pkg", true);
    module.classes = vec![
        class("pkg.X", vec![]),
        class("pkg.Y", vec![]),
        class("pkg.A", vec!["pkg.X".to_owned(), "pkg.Y".to_owned()]),
        class("pkg.B", vec!["pkg.Y".to_owned(), "pkg.X".to_owned()]),
        class("pkg.C", vec!["pkg.A".to_owned(), "pkg.B".to_owned()]),
    ];

    let error = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .expect("external IR registration must defer schema validation")
        .generate()
        .expect_err("CPython-inconsistent C3 hierarchy must be rejected");

    assert!(
        error.to_string().contains("consistent Python C3"),
        "unexpected error: {error}"
    );
}

#[test]
fn duplicate_direct_external_ir_base_is_rejected() {
    let mut module = empty_module("pkg", true);
    module.classes = vec![
        class("pkg.Base", vec![]),
        class(
            "pkg.Child",
            vec!["pkg.Base".to_owned(), "pkg.Base".to_owned()],
        ),
    ];

    let error = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .expect("external IR registration must defer schema validation")
        .generate()
        .expect_err("Python rejects a class that repeats the same direct base");

    assert!(
        error
            .to_string()
            .contains("duplicate direct base 'pkg.Base'"),
        "unexpected error: {error}"
    );
}

#[test]
fn deep_flat_external_ir_inheritance_validation_is_stack_safe() {
    // Registration is intentionally O(1) with respect to validation now, so this fixture must
    // exercise the terminal finish path. Keep it deep enough to cover recursion safety without
    // turning the full terminal lowering/emission pipeline into a minute-long integration test.
    const CLASS_COUNT: usize = 256;

    let mut module = empty_module("pkg", true);
    module.classes.reserve(CLASS_COUNT);
    for index in 0..CLASS_COUNT {
        let bases = if index > 0 {
            vec![format!("pkg.C{}", index - 1)]
        } else {
            Vec::new()
        };
        module.classes.push(class(&format!("pkg.C{index}"), bases));
    }

    Codegen::default()
        .with_budget(GenerationBudget {
            max_recursion_depth: CLASS_COUNT + 1,
            ..GenerationBudget::default()
        })
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .expect("external IR registration must defer schema validation")
        .generate()
        .expect("deep acyclic inheritance must validate without process-stack recursion");
}

#[test]
fn class_method_with_non_method_kind_is_rejected() {
    let mut widget = class("pkg.Widget", vec![]);
    widget.methods.push(FunctionIr {
        name: "pkg.Widget.run".to_owned(),
        kind: FunctionKindIr::Function,
        params: vec![],
        return_type: String::new(),
        docstring: None,
    });
    let mut module = empty_module("pkg", true);
    module.classes.push(widget);

    let error = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .expect("external IR registration must defer schema validation")
        .generate()
        .expect_err("class methods must carry FunctionKindIr::Method");

    assert!(
        error
            .to_string()
            .contains("must use FunctionKindIr::Method"),
        "unexpected error: {error}"
    );
}

#[test]
fn module_function_with_method_kind_is_rejected() {
    let mut module = empty_module("pkg", true);
    module.functions.push(FunctionIr {
        name: "pkg.run".to_owned(),
        kind: FunctionKindIr::Method {
            method_type: MethodKindIr::Static,
        },
        params: vec![],
        return_type: String::new(),
        docstring: None,
    });

    let error = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .expect("external IR registration must defer schema validation")
        .generate()
        .expect_err("module functions cannot carry method kinds");

    assert!(
        error
            .to_string()
            .contains("cannot use FunctionKindIr::Method"),
        "unexpected error: {error}"
    );
}

/// A constructor declared with no parameters is malformed IR: the live path always
/// includes the receiver, and the downstream generator strips exactly one parameter as
/// the receiver. It must be rejected at the ingestion boundary with a clear error rather
/// than silently no-op'ing the strip and emitting a subtly wrong binding (and it must not
/// panic on `Vec::remove(0)` over an empty parameter list).
#[test]
fn empty_constructor_is_rejected() {
    let mut widget = class("pkg.Widget", vec![]);
    widget.methods = vec![FunctionIr {
        name: "pkg.Widget.__init__".to_owned(),
        kind: FunctionKindIr::Method {
            method_type: MethodKindIr::Constructor,
        },
        params: vec![],
        return_type: String::new(),
        docstring: None,
    }];
    let mut module = empty_module("pkg", true);
    module.classes = vec![widget];
    let ir = SchemaIr {
        modules: vec![module],
    };

    let result = Codegen::new(Config::default())
        .modules_from_ir(ir)
        .and_then(|codegen| codegen.generate());
    assert!(
        result.is_err(),
        "a receiver-binding constructor with no parameters must be rejected, not silently accepted"
    );
}

/// An instance method declared with no parameters (missing its leading `self`) is the
/// same class of malformed IR as the empty constructor: the live path never produces it,
/// so it must be surfaced as an error rather than quietly rewritten to a static method.
#[test]
fn empty_instance_method_is_rejected() {
    let mut widget = class("pkg.Widget", vec![]);
    widget.methods = vec![FunctionIr {
        name: "pkg.Widget.do_thing".to_owned(),
        kind: FunctionKindIr::Method {
            method_type: MethodKindIr::Instance,
        },
        params: vec![],
        return_type: "int".to_owned(),
        docstring: None,
    }];
    let mut module = empty_module("pkg", true);
    module.classes = vec![widget];
    let ir = SchemaIr {
        modules: vec![module],
    };

    let result = Codegen::new(Config::default())
        .modules_from_ir(ir)
        .and_then(|codegen| codegen.generate());
    assert!(
        result.is_err(),
        "an instance method with no `self` parameter must be rejected, not silently accepted"
    );
}

#[test]
fn receiver_binding_methods_reject_a_non_receiver_leading_name() {
    for (method_type, method_name) in [
        (MethodKindIr::Instance, "run"),
        (MethodKindIr::Constructor, "__init__"),
        (MethodKindIr::Callable, "__call__"),
    ] {
        let mut widget = class("pkg.Widget", vec![]);
        widget.methods = vec![FunctionIr {
            name: format!("pkg.Widget.{method_name}"),
            kind: FunctionKindIr::Method { method_type },
            params: vec![ParameterIr {
                name: "value".to_owned(),
                kind: ParameterKindIr::PositionalOrKeyword,
                annotation: "int".to_owned(),
                has_default: false,
            }],
            return_type: "int".to_owned(),
            docstring: None,
        }];
        let mut module = empty_module("pkg", true);
        module.classes = vec![widget];

        let error = Codegen::default()
            .modules_from_ir(SchemaIr {
                modules: vec![module],
            })
            .expect("external IR registration must defer schema validation")
            .generate()
            .expect_err("receiver-binding methods must not strip an ordinary argument");

        assert!(
            error
                .to_string()
                .contains("leading receiver named 'self' or 'cls'"),
            "unexpected error for {method_type:?}: {error}",
        );
    }
}

#[test]
fn receiver_binding_methods_reject_a_non_positional_receiver() {
    for (method_type, method_name, parameter_kind) in [
        (
            MethodKindIr::Instance,
            "run",
            ParameterKindIr::VarPositional,
        ),
        (
            MethodKindIr::Constructor,
            "__init__",
            ParameterKindIr::KeywordOnly,
        ),
        (
            MethodKindIr::Callable,
            "__call__",
            ParameterKindIr::VarKeyword,
        ),
    ] {
        let mut widget = class("pkg.Widget", vec![]);
        widget.methods = vec![FunctionIr {
            name: format!("pkg.Widget.{method_name}"),
            kind: FunctionKindIr::Method { method_type },
            params: vec![ParameterIr {
                name: "self".to_owned(),
                kind: parameter_kind,
                annotation: String::new(),
                has_default: false,
            }],
            return_type: "int".to_owned(),
            docstring: None,
        }];
        let mut module = empty_module("pkg", true);
        module.classes = vec![widget];

        let error = Codegen::default()
            .modules_from_ir(SchemaIr {
                modules: vec![module],
            })
            .expect("external IR registration must defer schema validation")
            .generate()
            .expect_err("receiver-binding methods require a positional receiver");

        assert!(
            error
                .to_string()
                .contains("receiver must be positional-only or positional-or-keyword"),
            "unexpected error for {method_type:?}/{parameter_kind:?}: {error}",
        );
    }
}

/// A static method legitimately has no receiver, so an empty parameter list is valid and
/// must still generate. This pins the boundary of the empty-receiver rejection so it does
/// not over-reach into non-receiver-binding kinds.
#[test]
fn empty_static_method_is_accepted() {
    let mut widget = class("pkg.Widget", vec![]);
    widget.methods = vec![FunctionIr {
        name: "pkg.Widget.make".to_owned(),
        kind: FunctionKindIr::Method {
            method_type: MethodKindIr::Static,
        },
        params: vec![],
        return_type: "int".to_owned(),
        docstring: None,
    }];
    let mut module = empty_module("pkg", true);
    module.classes = vec![widget];
    let ir = SchemaIr {
        modules: vec![module],
    };

    let generated = Codegen::new(Config::default())
        .modules_from_ir(ir)
        .unwrap()
        .generate();
    assert!(
        generated.is_ok(),
        "a parameterless static method is valid and must still generate"
    );
}

#[test]
fn explicit_static_method_keeps_parameter_named_self() {
    let mut widget = class("pkg.Widget", vec![]);
    widget.methods = vec![FunctionIr {
        name: "pkg.Widget.make".to_owned(),
        kind: FunctionKindIr::Method {
            method_type: MethodKindIr::Static,
        },
        params: vec![ParameterIr {
            name: "self".to_owned(),
            kind: ParameterKindIr::PositionalOrKeyword,
            annotation: "int".to_owned(),
            has_default: false,
        }],
        return_type: "int".to_owned(),
        docstring: None,
    }];
    let mut module = empty_module("pkg", true);
    module.classes = vec![widget];

    let tokens = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .unwrap()
        .generate()
        .unwrap()
        .to_string();

    assert!(
        tokens.contains("pub fn make < 'py > (py : :: pyo3 :: marker :: Python < 'py > , p_self"),
        "an explicit static method must keep its legal 'self' parameter: {tokens}"
    );
    assert!(
        !tokens.contains("fn make < 'py > (& self"),
        "an explicit static method must not become an instance method: {tokens}"
    );
}

#[test]
fn class_method_ir_uses_the_bound_signature_without_a_receiver() {
    let mut widget = class("pkg.Widget", vec![]);
    widget.methods = vec![FunctionIr {
        name: "pkg.Widget.build".to_owned(),
        kind: FunctionKindIr::Method {
            method_type: MethodKindIr::Class,
        },
        params: vec![ParameterIr {
            name: "value".to_owned(),
            kind: ParameterKindIr::PositionalOrKeyword,
            annotation: "int".to_owned(),
            has_default: false,
        }],
        return_type: "int".to_owned(),
        docstring: None,
    }];
    let mut module = empty_module("pkg", true);
    module.classes = vec![widget];

    let tokens = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .unwrap()
        .generate()
        .unwrap()
        .to_string();

    assert!(
        tokens.contains("pub fn build < 'py > (py : :: pyo3 :: marker :: Python < 'py > , p_value"),
        "the first declared class-method argument must remain explicit: {tokens}",
    );
    assert!(
        !tokens.contains("fn build < 'py > (& self"),
        "class methods are associated functions over an already-bound Python class: {tokens}",
    );
}

#[test]
fn class_method_ir_preserves_receiver_shaped_bound_parameters() {
    for parameter_name in ["cls", "self"] {
        let mut widget = class("pkg.Widget", vec![]);
        widget.methods = vec![FunctionIr {
            name: "pkg.Widget.build".to_owned(),
            kind: FunctionKindIr::Method {
                method_type: MethodKindIr::Class,
            },
            params: vec![ParameterIr {
                name: parameter_name.to_owned(),
                kind: ParameterKindIr::PositionalOrKeyword,
                annotation: String::new(),
                has_default: false,
            }],
            return_type: "int".to_owned(),
            docstring: None,
        }];
        let mut module = empty_module("pkg", true);
        module.classes = vec![widget];

        let tokens = Codegen::default()
            .modules_from_ir(SchemaIr {
                modules: vec![module],
            })
            .unwrap()
            .generate()
            .unwrap()
            .to_string();

        assert!(
            tokens.contains(&format!("p_{parameter_name}")),
            "bound class-method parameter '{parameter_name}' must remain explicit: {tokens}",
        );
        assert!(
            !tokens.contains("fn build < 'py > (& self"),
            "a receiver-shaped bound argument must not turn the class method into an instance method: {tokens}",
        );
    }
}

/// An `Unknown`-kind method whose first parameter is `cls` is a classmethod by
/// convention: its explicit `cls` receiver must be stripped (like `self` for an
/// instance method), not forwarded to Python. Forwarding it -- the previous
/// behavior, which fell through to a static method -- would duplicate the class
/// receiver and raise `TypeError` on every call.
#[test]
fn unknown_method_ir_with_cls_receiver_is_a_classmethod() {
    let mut widget = class("pkg.Widget", vec![]);
    widget.methods = vec![FunctionIr {
        name: "pkg.Widget.make".to_owned(),
        kind: FunctionKindIr::Method {
            method_type: MethodKindIr::Unknown,
        },
        params: vec![
            ParameterIr {
                name: "cls".to_owned(),
                kind: ParameterKindIr::PositionalOrKeyword,
                annotation: String::new(),
                has_default: false,
            },
            ParameterIr {
                name: "value".to_owned(),
                kind: ParameterKindIr::PositionalOrKeyword,
                annotation: "int".to_owned(),
                has_default: false,
            },
        ],
        return_type: "int".to_owned(),
        docstring: None,
    }];
    let mut module = empty_module("pkg", true);
    module.classes = vec![widget];

    let tokens = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .unwrap()
        .generate()
        .unwrap()
        .to_string();

    assert!(
        tokens.contains("p_value"),
        "the real argument must survive: {tokens}",
    );
    assert!(
        !tokens.contains("p_cls"),
        "the implicit 'cls' receiver must be stripped, not forwarded to Python: {tokens}",
    );
    assert!(
        !tokens.contains("fn make < 'py > (& self"),
        "a classmethod is an associated function over an already-bound class: {tokens}",
    );
}

/// The symmetric `self`-receiver case still resolves to an instance method.
#[test]
fn unknown_method_ir_with_self_receiver_is_an_instance_method() {
    let mut widget = class("pkg.Widget", vec![]);
    widget.methods = vec![FunctionIr {
        name: "pkg.Widget.step".to_owned(),
        kind: FunctionKindIr::Method {
            method_type: MethodKindIr::Unknown,
        },
        params: vec![ParameterIr {
            name: "self".to_owned(),
            kind: ParameterKindIr::PositionalOrKeyword,
            annotation: String::new(),
            has_default: false,
        }],
        return_type: "int".to_owned(),
        docstring: None,
    }];
    let mut module = empty_module("pkg", true);
    module.classes = vec![widget];

    let tokens = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .unwrap()
        .generate()
        .unwrap()
        .to_string();

    assert!(
        tokens.contains("fn step < 'py > (& self"),
        "a leading 'self' receiver must resolve to an instance method: {tokens}",
    );
}

#[test]
fn unknown_method_ir_with_var_positional_self_keeps_explicit_argument() {
    let mut widget = class("pkg.Widget", vec![]);
    widget.methods = vec![FunctionIr {
        name: "pkg.Widget.collect_star".to_owned(),
        kind: FunctionKindIr::Method {
            method_type: MethodKindIr::Unknown,
        },
        params: vec![ParameterIr {
            name: "self".to_owned(),
            kind: ParameterKindIr::VarPositional,
            annotation: String::new(),
            has_default: false,
        }],
        return_type: "int".to_owned(),
        docstring: None,
    }];
    let mut module = empty_module("pkg", true);
    module.classes = vec![widget];

    let tokens = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .unwrap()
        .generate()
        .unwrap()
        .to_string();

    assert!(
        tokens.contains("pub fn collect_star") && tokens.contains("p_self"),
        "a non-positional 'self' must remain an explicit variadic argument: {tokens}",
    );
    assert!(
        !tokens.contains("fn collect_star < 'py > (& self"),
        "a non-positional 'self' must not infer an instance receiver: {tokens}",
    );
}

#[test]
fn unknown_method_ir_with_var_keyword_self_keeps_explicit_argument() {
    let mut widget = class("pkg.Widget", vec![]);
    widget.methods = vec![FunctionIr {
        name: "pkg.Widget.collect_kwargs".to_owned(),
        kind: FunctionKindIr::Method {
            method_type: MethodKindIr::Unknown,
        },
        params: vec![ParameterIr {
            name: "self".to_owned(),
            kind: ParameterKindIr::VarKeyword,
            annotation: String::new(),
            has_default: false,
        }],
        return_type: "int".to_owned(),
        docstring: None,
    }];
    let mut module = empty_module("pkg", true);
    module.classes = vec![widget];

    let tokens = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .unwrap()
        .generate()
        .unwrap()
        .to_string();

    assert!(
        tokens.contains("pub fn collect_kwargs") && tokens.contains("p_self"),
        "a non-positional 'self' must remain an explicit variadic argument: {tokens}",
    );
    assert!(
        !tokens.contains("fn collect_kwargs < 'py > (& self"),
        "a non-positional 'self' must not infer an instance receiver: {tokens}",
    );
}

#[test]
fn empty_external_ir_parameter_name_is_rejected() {
    let mut module = empty_module("pkg", false);
    module.functions = vec![FunctionIr {
        name: "pkg.run".to_owned(),
        kind: FunctionKindIr::Function,
        params: vec![ParameterIr {
            name: String::new(),
            kind: ParameterKindIr::PositionalOrKeyword,
            annotation: "int".to_owned(),
            has_default: false,
        }],
        return_type: String::new(),
        docstring: None,
    }];

    let error = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .expect("external IR registration must defer schema validation")
        .generate()
        .expect_err("empty parameter names must fail before identifier lowering");

    assert!(error
        .to_string()
        .contains("parameter name must be non-empty"));
}

#[test]
fn duplicate_external_ir_parameters_are_rejected() {
    let mut module = empty_module("pkg", false);
    module.functions = vec![FunctionIr {
        name: "pkg.run".to_owned(),
        kind: FunctionKindIr::Function,
        params: [
            ParameterKindIr::PositionalOnly,
            ParameterKindIr::PositionalOrKeyword,
        ]
        .into_iter()
        .map(|kind| ParameterIr {
            name: "value".to_owned(),
            kind,
            annotation: "int".to_owned(),
            has_default: false,
        })
        .collect(),
        return_type: String::new(),
        docstring: None,
    }];

    let error = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .expect("external IR registration must defer schema validation")
        .generate()
        .expect_err("duplicate parameter names must not reach Rust generation");

    assert!(error.to_string().contains("duplicate parameter 'value'"));
}

#[test]
fn canonically_equivalent_parameter_names_are_rejected() {
    let mut module = empty_module("pkg", false);
    module.functions = vec![FunctionIr {
        name: "pkg.run".to_owned(),
        kind: FunctionKindIr::Function,
        params: ["é", "e\u{301}"]
            .into_iter()
            .map(|name| ParameterIr {
                name: name.to_owned(),
                kind: ParameterKindIr::PositionalOrKeyword,
                annotation: "int".to_owned(),
                has_default: false,
            })
            .collect(),
        return_type: "int".to_owned(),
        docstring: None,
    }];

    let error = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .expect("external IR registration must defer schema validation")
        .generate()
        .expect_err("Rust normalizes identifiers to NFC before binding parameters");

    assert!(
        error.to_string().contains("colliding Rust parameter name"),
        "unexpected error: {error}"
    );
}

#[test]
fn invalid_external_ir_parameter_kind_order_is_rejected() {
    let mut module = empty_module("pkg", false);
    module.functions = vec![FunctionIr {
        name: "pkg.run".to_owned(),
        kind: FunctionKindIr::Function,
        params: [
            ("kwargs", ParameterKindIr::VarKeyword),
            ("value", ParameterKindIr::PositionalOrKeyword),
        ]
        .into_iter()
        .map(|(name, kind)| ParameterIr {
            name: name.to_owned(),
            kind,
            annotation: String::new(),
            has_default: false,
        })
        .collect(),
        return_type: String::new(),
        docstring: None,
    }];

    let error = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .expect("external IR registration must defer schema validation")
        .generate()
        .expect_err("parameter kinds must follow Python signature ordering");

    assert!(error.to_string().contains("invalid parameter kind order"));
}

#[test]
fn duplicate_external_ir_variadic_kinds_are_rejected() {
    let mut module = empty_module("pkg", false);
    module.functions = vec![FunctionIr {
        name: "pkg.run".to_owned(),
        kind: FunctionKindIr::Function,
        params: ["args", "more"]
            .into_iter()
            .map(|name| ParameterIr {
                name: name.to_owned(),
                kind: ParameterKindIr::VarPositional,
                annotation: String::new(),
                has_default: false,
            })
            .collect(),
        return_type: String::new(),
        docstring: None,
    }];

    let error = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .expect("external IR registration must defer schema validation")
        .generate()
        .expect_err("multiple variadic positional parameters must be rejected");

    assert!(error
        .to_string()
        .contains("multiple variadic positional parameters"));
}

#[test]
fn remaining_external_ir_parameter_grammar_errors_are_rejected() {
    let parameter = |name: &str, kind, has_default| ParameterIr {
        name: name.to_owned(),
        kind,
        annotation: String::new(),
        has_default,
    };
    let cases = [
        (
            vec![
                parameter("kwargs", ParameterKindIr::VarKeyword, false),
                parameter("more", ParameterKindIr::VarKeyword, false),
            ],
            "multiple variadic keyword parameters",
        ),
        (
            vec![parameter("args", ParameterKindIr::VarPositional, true)],
            "variadic parameter 'args' cannot have a default",
        ),
        (
            vec![parameter("kwargs", ParameterKindIr::VarKeyword, true)],
            "variadic parameter 'kwargs' cannot have a default",
        ),
        (
            vec![parameter(
                "not-valid",
                ParameterKindIr::PositionalOrKeyword,
                false,
            )],
            "parameter 'not-valid' cannot be represented as a Rust identifier",
        ),
    ];
    // A required positional parameter after a defaulted one is no longer a grammar error:
    // `has_default` is emission-metadata-only, so strict external IR now accepts it
    // unconditionally (see `required_after_defaulted_param_accepted_in_strict_mode`).

    for (params, expected) in cases {
        let mut module = empty_module("pkg", false);
        module.functions = vec![FunctionIr {
            name: "pkg.run".to_owned(),
            kind: FunctionKindIr::Function,
            params,
            return_type: String::new(),
            docstring: None,
        }];

        let error = Codegen::default()
            .modules_from_ir(SchemaIr {
                modules: vec![module],
            })
            .expect("external IR registration must defer schema validation")
            .generate()
            .expect_err("malformed parameter grammar must be rejected");
        assert!(
            error.to_string().contains(expected),
            "expected '{expected}', got: {error}"
        );
    }
}

#[test]
fn empty_and_duplicate_external_ir_prelude_names_are_rejected() {
    for prelude in [vec![String::new()], vec!["Widget".to_owned(); 2]] {
        let mut module = empty_module("pkg", false);
        module.prelude = prelude;
        let error = Codegen::default()
            .modules_from_ir(SchemaIr {
                modules: vec![module],
            })
            .expect("external IR registration must defer schema validation")
            .generate()
            .expect_err("malformed prelude names must fail before identifier lowering");
        assert!(error.to_string().contains("prelude"), "{error}");
    }
}

#[test]
fn unresolved_external_ir_prelude_target_is_rejected() {
    let mut module = empty_module("pkg", false);
    module.prelude = vec!["Missing".to_owned()];

    let error = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .expect("external IR registration must defer schema validation")
        .generate()
        .expect_err("unknown prelude targets must not be silently omitted");

    assert!(
        error
            .to_string()
            .contains("prelude target 'pkg.Missing' does not resolve"),
        "unexpected error: {error}"
    );
}

#[test]
fn disabled_external_ir_prelude_target_is_rejected() {
    let mut module = empty_module("pkg", false);
    module.classes = vec![class("pkg.Widget", vec![])];
    module.prelude = vec!["Widget".to_owned()];

    let error = Codegen::new(Config::builder().generate_classes(false).build())
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .expect("external IR registration must defer schema validation")
        .generate()
        .expect_err("disabled prelude targets must not be silently omitted");

    assert!(
        error
            .to_string()
            .contains("prelude target 'pkg.Widget' is a class disabled"),
        "unexpected error: {error}"
    );
}

#[test]
fn non_traversed_submodule_preludes_do_not_reject_ignored_descendants() {
    let grandchild = empty_module("pkg.child.grand", false);
    let mut child = empty_module("pkg.child", true);
    child.prelude = vec!["grand".to_owned()];
    child.submodules = vec![grandchild];
    let mut root = empty_module("pkg", true);
    root.submodules = vec![child];

    let generated = Codegen::new(Config::builder().traverse_submodules(false).build())
        .modules_from_ir(SchemaIr {
            modules: vec![root],
        })
        .expect("ignored submodule preludes must not reject an otherwise valid schema")
        .generate()
        .unwrap()
        .to_string();

    assert!(!generated.contains("mod child"), "{generated}");
}

#[test]
fn external_ir_finalize_diagnostics_reach_generation_report() {
    let mut module = empty_module("pkg", false);
    module.functions = vec![FunctionIr {
        name: "pkg.run".to_owned(),
        kind: FunctionKindIr::Function,
        params: vec![ParameterIr {
            name: "self".to_owned(),
            kind: ParameterKindIr::PositionalOrKeyword,
            annotation: String::new(),
            has_default: false,
        }],
        return_type: String::new(),
        docstring: None,
    }];

    let generated = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .unwrap()
        .generate_with_report()
        .unwrap();

    assert!(generated.report.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "function_self_parameter_renamed"
            && diagnostic.path.as_deref() == Some("pkg.run")
    }));
}

/// An empty item name (here an empty class name) would panic inside `Path::name()`
/// (`segments.last().unwrap()`) during generation. It must instead be rejected at the
/// terminal generation boundary with a clear error.
#[test]
fn empty_class_name_is_rejected() {
    let mut module = empty_module("pkg", true);
    module.classes = vec![class("", vec![])];
    let ir = SchemaIr {
        modules: vec![module],
    };

    let result = Codegen::new(Config::default())
        .modules_from_ir(ir)
        .and_then(|codegen| codegen.generate());
    assert!(
        result.is_err(),
        "an empty class name must be rejected at terminal generation, not panic"
    );
}

/// An empty enum name is the same panic class as an empty class name and must likewise be
/// rejected at terminal generation.
#[test]
fn empty_enum_name_is_rejected() {
    let mut module = empty_module("pkg", true);
    module.enums = vec![EnumIr {
        name: String::new(),
        kind: EnumKindIr::Enum,
        variants: vec![EnumVariantIr {
            py_name: "RED".to_owned(),
            value_repr: "1".to_owned(),
        }],
        aliases: vec![],
        docstring: None,
        is_flag: false,
        is_open: false,
        string_backed: false,
        plain_mirror: false,
    }];
    let ir = SchemaIr {
        modules: vec![module],
    };

    let result = Codegen::new(Config::default())
        .modules_from_ir(ir)
        .and_then(|codegen| codegen.generate());
    assert!(
        result.is_err(),
        "an empty enum name must be rejected at terminal generation, not panic"
    );
}

/// A property with a distinct `setter_type` must type the generated setter's parameter
/// from that type, not from the getter type.
#[test]
fn setter_type_is_honored() {
    let mut widget = class("pkg.Widget", vec![]);
    widget.properties = vec![PropertyIr {
        name: "label".to_owned(),
        py_type: "int".to_owned(),
        setter_type: Some("str".to_owned()),
        readonly: false,
        docstring: None,
        setter_docstring: None,
    }];
    let mut module = empty_module("pkg", true);
    module.classes = vec![widget];
    let ir = SchemaIr {
        modules: vec![module],
    };

    let tokens = Codegen::new(Config::default())
        .modules_from_ir(ir)
        .unwrap()
        .generate()
        .unwrap()
        .to_string();

    assert!(
        tokens.contains("p_value : & str"),
        "the setter parameter must use `setter_type` (`str`), not the getter type: {tokens}"
    );
}

/// An `Optional[<in-IR class>]` property must lower per the contract rows
/// `Optional[T]` -> `Option<T_owned>` / `Option<T_param>` and class-reference -> newtype
/// (docs/EXTERNAL_IR_LOWERING_CONTRACT.md): the getter returns
/// `PyResult<Option<A>>` and the setter takes `p_value: Option<A>` — never a degraded
/// `Bound<PyAny>` and never a dangling reference. Downstream IR emitters
/// author exactly this shape, and no other test exercises `Optional[Class]` in
/// property position.
#[test]
fn optional_class_property_lowers_to_option_of_newtype() {
    let a = class("pkg.A", vec![]);
    let mut b = class("pkg.B", vec![]);
    b.properties = vec![PropertyIr {
        name: "parent".to_owned(),
        py_type: "Optional[pkg.A]".to_owned(),
        setter_type: None,
        readonly: false,
        docstring: None,
        setter_docstring: None,
    }];
    let mut module = empty_module("pkg", true);
    module.classes = vec![a, b];
    let ir = SchemaIr {
        modules: vec![module],
    };

    let tokens = Codegen::new(Config::default())
        .modules_from_ir(ir)
        .unwrap()
        .generate()
        .unwrap()
        .to_string();

    assert!(
        tokens.contains("fn parent < 'py > (& self , py : :: pyo3 :: marker :: Python < 'py > ,) -> :: pyo3 :: PyResult < :: std :: option :: Option < A > >"),
        "the getter must return `PyResult<Option<A>>` (Option of the class newtype): {tokens}"
    );
    assert!(
        tokens.contains("p_value : :: std :: option :: Option < A >"),
        "the setter must take `p_value: Option<A>` (by-value Option of the class newtype): {tokens}"
    );
}

/// A `*args`/`**kwargs` IR parameter must lower to the same synthetic tuple/dict type
/// the live path forces by kind, regardless of the supplied annotation string.
#[test]
fn var_args_ir_matches_live_forced_types() {
    let cfg = Config::builder()
        .generate_provenance(false)
        .generate_preludes(false)
        .build();

    pyo3::Python::initialize();
    let live_codegen = pyo3::Python::attach(|py| {
        Codegen::prime_live_runtime(py).unwrap();
        let module = pyo3::types::PyModule::from_code(
            py,
            cstr("def f(value, *args, **kwargs):\n    return None\n").as_c_str(),
            cstr("vmod/__init__.py").as_c_str(),
            cstr("vmod").as_c_str(),
        )
        .unwrap();
        Codegen::new(cfg.clone()).module(&module).unwrap()
    });
    let live_tokens = live_codegen.generate().unwrap().to_string();

    // The IR omits the variadic annotations entirely; the kind alone must drive the
    // type, matching the live path.
    let ir = SchemaIr {
        modules: vec![ModuleIr {
            functions: vec![FunctionIr {
                name: "vmod.f".to_owned(),
                kind: FunctionKindIr::Function,
                params: vec![
                    ParameterIr {
                        name: "value".to_owned(),
                        kind: ParameterKindIr::PositionalOrKeyword,
                        annotation: String::new(),
                        has_default: false,
                    },
                    ParameterIr {
                        name: "args".to_owned(),
                        kind: ParameterKindIr::VarPositional,
                        annotation: String::new(),
                        has_default: false,
                    },
                    ParameterIr {
                        name: "kwargs".to_owned(),
                        kind: ParameterKindIr::VarKeyword,
                        annotation: String::new(),
                        has_default: false,
                    },
                ],
                return_type: "None".to_owned(),
                docstring: None,
            }],
            ..empty_module("vmod", false)
        }],
    };
    let ir_tokens = Codegen::new(cfg)
        .modules_from_ir(ir)
        .unwrap()
        .generate()
        .unwrap()
        .to_string();

    // (#14 guard) The synthetic `**kwargs` param must still MATERIALIZE to a real dict
    // (`PyDict::new(py)`), NOT receive the Optional-preserving `.transpose()` treatment
    // that genuine `Optional[dict]` params now get — its call path forwards a plain dict.
    assert!(
        ir_tokens.contains("PyDict :: new (py)"),
        "**kwargs must still materialize to a PyDict, got: {ir_tokens}"
    );
    assert!(
        ir_tokens.contains("PyTupleMethods :: len (p_args)")
            && ir_tokens.contains("PyTupleMethods :: iter (p_args)")
            && !ir_tokens.contains("PyTupleMethods :: len (& p_args)")
            && !ir_tokens.contains("PyTupleMethods :: iter (& p_args)"),
        "*args assembly must use the borrowed PyTuple directly: {ir_tokens}"
    );

    assert_eq!(
        live_tokens, ir_tokens,
        "variadic IR params must produce the same forced tuple/dict types as the live path"
    );
}

#[test]
fn explicit_keyword_collision_with_var_kwargs_is_rejected_at_runtime() {
    let mut module = empty_module("pkg", false);
    module.functions.push(FunctionIr {
        name: "pkg.run".to_owned(),
        kind: FunctionKindIr::Function,
        params: vec![
            ParameterIr {
                name: "mode".to_owned(),
                kind: ParameterKindIr::KeywordOnly,
                annotation: "int".to_owned(),
                has_default: false,
            },
            ParameterIr {
                name: "kwargs".to_owned(),
                kind: ParameterKindIr::VarKeyword,
                annotation: String::new(),
                has_default: false,
            },
        ],
        return_type: String::new(),
        docstring: None,
    });

    let tokens = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .unwrap()
        .generate()
        .unwrap()
        .to_string();

    assert!(
        tokens.contains("PyDictMethods :: copy")
            && tokens.contains("PyDictMethods :: contains")
            && tokens.contains("got multiple values for keyword argument"),
        "explicit keyword + **kwargs must copy kwargs and reject duplicate keys: {tokens}"
    );
}

/// A class-free module that uses a `Collection[...]` type must still emit the
/// `PyCollection` wrapper it lowers to (otherwise the reference is undefined).
#[test]
fn collection_without_class_emits_wrapper() {
    let mut module = empty_module("m", false);
    module.functions.push(FunctionIr {
        name: "m.f".to_owned(),
        kind: FunctionKindIr::Function,
        params: vec![],
        return_type: "Collection[int]".to_owned(),
        docstring: None,
    });
    let ir = SchemaIr {
        modules: vec![module],
    };

    let tokens = Codegen::new(Config::default())
        .modules_from_ir(ir)
        .unwrap()
        .generate()
        .unwrap()
        .to_string();

    assert!(
        tokens.contains("struct PyCollection"),
        "the `PyCollection` wrapper must be defined when a collection type is used \
         even without any class: {tokens}"
    );

    let generated = prettyplease::unparse(
        &syn::parse_str::<syn::File>(&tokens).expect("generated collection source must parse"),
    );
    let consumer = format!(
        r#"{generated}

pub fn use_primitive_collection<'py>(
    py: ::pyo3::Python<'py>,
    collection: &PyCollection<i64>,
) -> ::pyo3::PyResult<()> {{
    let _: Option<i64> = collection.get(py, 0)?;
    let _: Option<i64> = collection.get_by_key(py, "key")?;
    let _: Vec<i64> = collection.iter(py)?;
    Ok(())
}}
"#
    );
    assert_ir_source_compiles(&consumer, "primitive_collection");
}

/// A bare (non-dotted) module-level function name must be rejected with an error
/// rather than panicking at generation on the missing parent module.
#[test]
fn bare_module_function_name_is_rejected() {
    let mut module = empty_module("m", false);
    module.functions.push(FunctionIr {
        // Missing the `m.` prefix — a contract violation that used to panic.
        name: "justfoo".to_owned(),
        kind: FunctionKindIr::Function,
        params: vec![],
        return_type: "int".to_owned(),
        docstring: None,
    });
    let ir = SchemaIr {
        modules: vec![module],
    };

    let result = Codegen::new(Config::default())
        .modules_from_ir(ir)
        .and_then(|codegen| codegen.generate());
    assert!(
        result.is_err(),
        "a bare module-level function name must be rejected, not panic"
    );
}

/// FREEZE GUARD — a representative external-IR schema (a module with a class that has a
/// base class, an instance method, and a property; a standalone enum; and a free
/// function) must generate all of the load-bearing binding shapes the downstream consumer
/// depends on. This locks the external-IR → Rust codegen contract so accidental drift is
/// caught here before a downstream regeneration inherits it. It is intentionally
/// deterministic (no live introspection, no interpreter) and asserts on the emitted
/// token text rather than exact formatting.
#[test]
fn external_ir_codegen_contract_is_frozen() {
    // Base class (root) so the derived class exercises the supertrait/ancestor path.
    let base = class("pkg.Base", vec![]);

    // Derived class with a base, one instance method, and one property.
    let mut widget = class("pkg.Widget", vec!["pkg.Base".to_owned()]);
    widget.methods = vec![FunctionIr {
        name: "pkg.Widget.compute".to_owned(),
        kind: FunctionKindIr::Method {
            method_type: MethodKindIr::Instance,
        },
        params: vec![
            // Leading receiver, stripped by the same logic as the live path.
            ParameterIr {
                name: "self".to_owned(),
                kind: ParameterKindIr::PositionalOrKeyword,
                annotation: String::new(),
                has_default: false,
            },
            ParameterIr {
                name: "factor".to_owned(),
                kind: ParameterKindIr::PositionalOrKeyword,
                annotation: "int".to_owned(),
                has_default: false,
            },
        ],
        return_type: "int".to_owned(),
        docstring: None,
    }];
    widget.properties = vec![PropertyIr {
        name: "label".to_owned(),
        py_type: "str".to_owned(),
        setter_type: None,
        readonly: true,
        docstring: None,
        setter_docstring: None,
    }];

    let mut module = empty_module("pkg", true);
    module.classes = vec![base, widget];
    module.enums = vec![EnumIr {
        name: "pkg.Mode".to_owned(),
        kind: EnumKindIr::Enum,
        variants: vec![
            EnumVariantIr {
                py_name: "FAST".to_owned(),
                value_repr: "1".to_owned(),
            },
            EnumVariantIr {
                py_name: "SLOW".to_owned(),
                value_repr: "2".to_owned(),
            },
        ],
        aliases: vec![],
        docstring: None,
        is_flag: false,
        is_open: false,
        string_backed: false,
        plain_mirror: false,
    }];
    module.functions = vec![FunctionIr {
        name: "pkg.helper".to_owned(),
        kind: FunctionKindIr::Function,
        params: vec![ParameterIr {
            name: "value".to_owned(),
            kind: ParameterKindIr::PositionalOrKeyword,
            annotation: "int".to_owned(),
            has_default: false,
        }],
        return_type: "int".to_owned(),
        docstring: None,
    }];

    let ir = SchemaIr {
        modules: vec![module],
    };

    let tokens = Codegen::new(Config::default())
        .modules_from_ir(ir)
        .unwrap()
        .generate()
        .unwrap()
        .to_string();

    // Class newtypes.
    assert!(
        tokens.contains("pub struct Base"),
        "missing base class newtype `Base`: {tokens}"
    );
    assert!(
        tokens.contains("pub struct Widget"),
        "missing derived class newtype `Widget`: {tokens}"
    );
    // The derived class carries its base's methods-trait bound / impl.
    assert!(
        tokens.contains("BaseMethods for Widget"),
        "missing base-class methods impl (`BaseMethods for Widget`): {tokens}"
    );
    // The instance method survives as a trait method on the class.
    assert!(
        tokens.contains("fn compute"),
        "missing instance method `compute`: {tokens}"
    );
    // The read-only property surfaces as a getter.
    assert!(
        tokens.contains("fn label") || tokens.contains("fn get_label"),
        "missing property getter for `label`: {tokens}"
    );
    // The enum is mirrored.
    assert!(
        tokens.contains("pub enum Mode"),
        "missing enum `Mode`: {tokens}"
    );
    assert!(
        tokens.contains("FAST") && tokens.contains("SLOW"),
        "missing enum variants: {tokens}"
    );
    // The free function is emitted.
    assert!(
        tokens.contains("fn helper"),
        "missing free function `helper`: {tokens}"
    );
    // The synthetic universal-base machinery is emitted once when any class exists.
    assert!(
        tokens.contains("trait PyObjectBase"),
        "missing the synthetic universal base trait emitted with any class: {tokens}"
    );
    // But the generic `PyCollection` wrapper is NOT carried by this class-only IR (no
    // collection type is used), so it is not dead-emitted.
    assert!(
        !tokens.contains("struct PyCollection"),
        "a class-only IR must not emit the unused `PyCollection` wrapper: {tokens}"
    );
}

/// (A4) IP-address annotations lower to Rust std-net types, not to the catch-all PyAny.
/// Pinned because `ipaddress.IPv4Address` / `ipaddress.IPv6Address` used to fall through
/// to `Other(...)` → `Bound<PyAny>` before the IpV4Addr / IpV6Addr variants were wired.
///
/// Return position (owned/extraction direction) lowers to the shared `::std::net::IpAddr`,
/// not the family-specific `Ipv4Addr`/`Ipv6Addr` structs: pyo3 0.29 only implements
/// `FromPyObject` for the combined `IpAddr` enum, so a bare `Ipv4Addr`/`Ipv6Addr` return
/// type would fail to compile. Parameter position (borrowed/into-python direction) keeps
/// the precise `Ipv4Addr`/`Ipv6Addr` type, since `IntoPyObject` IS implemented for both.
#[test]
fn ip_address_annotations_lower_to_std_net_types() {
    let mut module = empty_module("m", false);
    module.functions.push(FunctionIr {
        name: "m.get_v4".to_owned(),
        kind: FunctionKindIr::Function,
        params: vec![],
        return_type: "ipaddress.IPv4Address".to_owned(),
        docstring: None,
    });
    module.functions.push(FunctionIr {
        name: "m.get_v6".to_owned(),
        kind: FunctionKindIr::Function,
        params: vec![],
        return_type: "ipaddress.IPv6Address".to_owned(),
        docstring: None,
    });
    module.functions.push(FunctionIr {
        name: "m.take_v4".to_owned(),
        kind: FunctionKindIr::Function,
        params: vec![ParameterIr {
            name: "addr".to_owned(),
            kind: ParameterKindIr::PositionalOrKeyword,
            annotation: "ipaddress.IPv4Address".to_owned(),
            has_default: false,
        }],
        return_type: "None".to_owned(),
        docstring: None,
    });
    module.functions.push(FunctionIr {
        name: "m.take_v6".to_owned(),
        kind: FunctionKindIr::Function,
        params: vec![ParameterIr {
            name: "addr".to_owned(),
            kind: ParameterKindIr::PositionalOrKeyword,
            annotation: "ipaddress.IPv6Address".to_owned(),
            has_default: false,
        }],
        return_type: "None".to_owned(),
        docstring: None,
    });
    let ir = SchemaIr {
        modules: vec![module],
    };

    let tokens = Codegen::new(Config::default())
        .modules_from_ir(ir)
        .unwrap()
        .generate()
        .unwrap()
        .to_string();

    assert!(
        tokens.contains("IpAddr"),
        "ipaddress.IPv4Address / IPv6Address return position must lower to \
         ::std::net::IpAddr (FromPyObject only exists for the combined enum), not PyAny: {tokens}"
    );
    assert!(
        tokens.contains("Ipv4Addr"),
        "ipaddress.IPv4Address parameter position must lower to ::std::net::Ipv4Addr: {tokens}"
    );
    assert!(
        tokens.contains("Ipv6Addr"),
        "ipaddress.IPv6Address parameter position must lower to ::std::net::Ipv6Addr: {tokens}"
    );
}

/// (A5) A `dict` annotation whose key type is not Rust-Hashable (e.g. `frozenset`)
/// must NOT emit a `HashMap` — that would fail to compile because `frozenset` /
/// `PyFrozenSet` does not implement `Hash` + `Eq`. It must fall back to the untyped
/// `Bound<PyDict>` form instead.
#[test]
fn dict_with_unhashable_key_falls_back_to_untyped_py_dict() {
    let mut module = empty_module("m", false);
    module.functions.push(FunctionIr {
        name: "m.get_dict".to_owned(),
        kind: FunctionKindIr::Function,
        params: vec![],
        return_type: "dict[frozenset, int]".to_owned(),
        docstring: None,
    });
    let ir = SchemaIr {
        modules: vec![module],
    };

    let tokens = Codegen::new(Config::default())
        .modules_from_ir(ir)
        .unwrap()
        .generate()
        .unwrap()
        .to_string();

    assert!(
        !tokens.contains("HashMap"),
        "dict[frozenset, int] must not emit HashMap (frozenset is not Hash): {tokens}"
    );
    assert!(
        tokens.contains("PyDict"),
        "dict[frozenset, int] must fall back to Bound<PyDict>, not disappear: {tokens}"
    );
}

/// (A5) Positive control: a `dict` whose key type IS Rust-Hashable (`str` -> `String`)
/// must emit the typed `HashMap` form. Pairs with the negative case above so a
/// regression that over-restricts `is_hashable` (dropping a genuinely hashable key)
/// would be caught too.
#[test]
fn dict_with_hashable_key_emits_typed_hash_map() {
    let mut module = empty_module("m", false);
    module.functions.push(FunctionIr {
        name: "m.get_dict".to_owned(),
        kind: FunctionKindIr::Function,
        params: vec![],
        return_type: "dict[str, int]".to_owned(),
        docstring: None,
    });
    let ir = SchemaIr {
        modules: vec![module],
    };

    let tokens = Codegen::new(Config::default())
        .modules_from_ir(ir)
        .unwrap()
        .generate()
        .unwrap()
        .to_string();

    assert!(
        tokens.contains("HashMap"),
        "dict[str, int] must emit a typed HashMap (str/int are Hash+Eq): {tokens}"
    );
}

/// Python `bytes` is immutable and hashable, so typed dict/set lowering must preserve its
/// `Vec<u8>` representation in both call directions. The binary key exercises the full
/// conversion path instead of only proving that the generated signatures compile.
#[test]
fn bytes_are_hashable_dict_and_set_external_ir_round_trip() {
    let mut module = empty_module("bytes_hashability", false);
    module.functions = vec![
        FunctionIr {
            name: "bytes_hashability.take_dict".to_owned(),
            kind: FunctionKindIr::Function,
            params: vec![ParameterIr {
                name: "values".to_owned(),
                kind: ParameterKindIr::PositionalOrKeyword,
                annotation: "dict[bytes, int]".to_owned(),
                has_default: false,
            }],
            return_type: "None".to_owned(),
            docstring: None,
        },
        FunctionIr {
            name: "bytes_hashability.take_set".to_owned(),
            kind: FunctionKindIr::Function,
            params: vec![ParameterIr {
                name: "values".to_owned(),
                kind: ParameterKindIr::PositionalOrKeyword,
                annotation: "set[bytes]".to_owned(),
                has_default: false,
            }],
            return_type: "None".to_owned(),
            docstring: None,
        },
        function("bytes_hashability.get_dict", "dict[bytes, int]"),
        function("bytes_hashability.get_set", "set[bytes]"),
        function(
            "bytes_hashability.get_bytearray_dict",
            "dict[bytearray, int]",
        ),
        function("bytes_hashability.get_bytearray_set", "set[bytearray]"),
        function(
            "bytes_hashability.get_untyped_value_dict",
            "dict[bytes, Any]",
        ),
        function("bytes_hashability.get_untyped_set", "set[Any]"),
    ];

    let generated = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .expect("bytes hashability IR must validate")
        .generate()
        .expect("bytes hashability IR must generate")
        .to_string();
    let source = prettyplease::unparse(
        &syn::parse_str::<syn::File>(&generated).expect("generated bytes source must parse"),
    );
    let signature = |name: &str| {
        let start = source
            .find(&format!("pub fn {name}<'py>"))
            .unwrap_or_else(|| panic!("generated function {name} missing: {source}"));
        let end = source[start..]
            .find('{')
            .map(|offset| start + offset)
            .unwrap_or_else(|| panic!("generated function {name} has no body: {source}"));
        &source[start..end]
    };

    assert!(
        signature("take_dict").contains("p_values: &::std::collections::HashMap<Vec<u8>, i64>"),
        "bytes dict parameters must use HashMap<Vec<u8>, i64>: {source}"
    );
    assert!(
        signature("take_set").contains("p_values: &::std::collections::HashSet<Vec<u8>>"),
        "bytes set parameters must use HashSet<Vec<u8>>: {source}"
    );
    assert!(
        signature("get_dict")
            .contains("-> ::pyo3::PyResult<::std::collections::HashMap<Vec<u8>, i64>>"),
        "bytes dict returns must use HashMap<Vec<u8>, i64>: {source}"
    );
    assert!(
        signature("get_set").contains("-> ::pyo3::PyResult<::std::collections::HashSet<Vec<u8>>>"),
        "bytes set returns must use HashSet<Vec<u8>>: {source}"
    );

    // Controls: bytearray keys/items and dynamic values stay on native Python handles.
    for name in ["get_bytearray_dict", "get_untyped_value_dict"] {
        assert!(
            signature(name)
                .contains("-> ::pyo3::PyResult<::pyo3::Bound<'py, ::pyo3::types::PyDict>>"),
            "{name} must retain the untyped PyDict fallback: {source}"
        );
    }
    for name in ["get_bytearray_set", "get_untyped_set"] {
        assert!(
            signature(name)
                .contains("-> ::pyo3::PyResult<::pyo3::Bound<'py, ::pyo3::types::PySet>>"),
            "{name} must retain the untyped PySet fallback: {source}"
        );
    }

    let source = [
        source.as_str(),
        r#"
#[cfg(test)]
mod bytes_hashability_contract_tests {
    use super::bytes_hashability;
    use pyo3::types::{PyAnyMethods, PyModule};
    use std::collections::{HashMap, HashSet};

    #[test]
    fn binary_bytes_keys_round_trip_through_python() -> pyo3::PyResult<()> {
        pyo3::Python::attach(|py| {
            let runtime_module = PyModule::from_code(
                py,
                pyo3::ffi::c_str!(
                    "def take_dict(values):\n    assert type(values) is dict\n    assert len(values) == 1\n    key = next(iter(values))\n    assert type(key) is bytes\n    assert key == bytes([0, 255])\n    assert type(values[key]) is int\n    assert values[key] == 17\n\ndef take_set(values):\n    assert type(values) is set\n    assert len(values) == 1\n    value = next(iter(values))\n    assert type(value) is bytes\n    assert value == bytes([0, 255])\n\ndef get_dict():\n    return {bytes([0, 255]): 17}\n\ndef get_set():\n    return {bytes([0, 255])}\n"
                ),
                pyo3::ffi::c_str!("bytes_hashability.py"),
                pyo3::ffi::c_str!("bytes_hashability"),
            )?;
            py.import(pyo3::intern!(py, "sys"))?
                .getattr(pyo3::intern!(py, "modules"))?
                .set_item(pyo3::intern!(py, "bytes_hashability"), &runtime_module)?;

            let key = vec![0_u8, 255];
            let mut values = HashMap::new();
            values.insert(key.clone(), 17_i64);
            bytes_hashability::take_dict(py, &values)?;

            let mut values = HashSet::new();
            values.insert(key.clone());
            bytes_hashability::take_set(py, &values)?;

            let returned: HashMap<Vec<u8>, i64> = bytes_hashability::get_dict(py)?;
            assert_eq!(returned.get(&key), Some(&17));
            let returned: HashSet<Vec<u8>> = bytes_hashability::get_set(py)?;
            assert!(returned.contains(&key));
            Ok(())
        })
    }
}
"#,
    ]
    .join("\n");
    assert_ir_source_runs(&source, "bytes_hashability");
}

/// (A6) A malformed `dict[int]` annotation (one type argument instead of two) must
/// return an `Err` from the generation pipeline rather than silently emitting a binding
/// with unknown key/value types (or panicking on an out-of-bounds index). This pins the
/// boundary: valid two-argument dict annotations continue to work; a one-argument dict
/// is structurally malformed and must be rejected.
#[test]
fn malformed_dict_annotation_with_one_arg_returns_err() {
    let mut module = empty_module("m", false);
    module.functions.push(FunctionIr {
        name: "m.bad".to_owned(),
        kind: FunctionKindIr::Function,
        params: vec![],
        return_type: "dict[int]".to_owned(),
        docstring: None,
    });
    let ir = SchemaIr {
        modules: vec![module],
    };

    // The malformed annotation must surface as Err somewhere in the pipeline (it is
    // caught while building the model from IR) rather than silently emitting a binding
    // with unknown types.
    let result = Codegen::new(Config::default())
        .modules_from_ir(ir)
        .and_then(|codegen| codegen.generate());
    assert!(
        result.is_err(),
        "a malformed dict[int] annotation (missing value type) must surface as Err, \
         not silently emit a binding with unknown types"
    );
}

/// Generate a single free function `m.f(x: <annotation>)` and return its token string.
fn tokens_for_param_annotation(annotation: &str) -> String {
    let mut module = empty_module("m", false);
    module.functions.push(FunctionIr {
        name: "m.f".to_owned(),
        kind: FunctionKindIr::Function,
        params: vec![ParameterIr {
            name: "x".to_owned(),
            kind: ParameterKindIr::PositionalOrKeyword,
            annotation: annotation.to_owned(),
            has_default: false,
        }],
        return_type: "None".to_owned(),
        docstring: None,
    });
    Codegen::new(Config::default())
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .unwrap()
        .generate()
        .unwrap()
        .to_string()
}

/// (#14) An `Optional[dict[str, Any]]` parameter must forward a caller `None` as Python
/// `None`, not materialize it into an empty `{}`. The generated preprocessing must keep
/// the `Option` wrapper (`.transpose()`) rather than the old `if let Some .. else
/// PyDict::new(py)` collapse that turned every `None` into an empty dict.
#[test]
fn optional_dict_param_preserves_none() {
    let tokens = tokens_for_param_annotation("dict[str, Any] | None");
    assert!(
        tokens.contains("transpose"),
        "Optional[dict] must keep the Option (via `.transpose()`), got: {tokens}"
    );
    assert!(
        !tokens.contains("PyDict :: new (py)"),
        "Optional[dict] must NOT collapse a caller `None` into an empty `{{}}`: {tokens}"
    );
}

/// (#17) A variadic `tuple[X, ...]` parameter must stay a native `PyTuple` handle.
/// Rebuilding that handle with `PyTuple::new` is redundant and can fail to compile
/// because iteration yields `PyResult<Bound<PyAny>>`.
#[test]
fn variadic_tuple_param_is_native_py_tuple_without_preconversion() {
    let tokens = tokens_for_param_annotation("tuple[int, ...]");
    assert!(
        tokens.contains("& :: pyo3 :: Bound < 'py , :: pyo3 :: types :: PyTuple >"),
        "tuple[X, ...] must accept a native PyTuple handle: {tokens}"
    );
    assert!(
        !tokens.contains("PyTuple :: new (py , p_x)"),
        "a native PyTuple parameter must not be rebuilt: {tokens}"
    );
}

#[test]
fn nested_variadic_tuple_keeps_python_tuple_shape() {
    let tokens = tokens_for_param_annotation("list[tuple[int, ...]]");
    assert!(
        tokens.contains("PyTuple"),
        "nested tuple[X, ...] must remain a Python tuple handle: {tokens}"
    );
    assert!(
        !tokens.contains("Vec < Vec < i64 > >") && !tokens.contains("& [Vec < i64 >]"),
        "nested tuple[X, ...] must not lower to Vec/list and change Python shape: {tokens}"
    );
}

/// A class and sibling submodule cannot occupy the same Python/Rust symbol. External IR
/// is authoritative input, so silently dropping either definition would corrupt it.
#[test]
fn class_vs_submodule_name_clash_is_rejected() {
    let mut root = empty_module("pkg", true);
    root.classes = vec![class("pkg.Foo", vec![])];
    let mut sub = empty_module("pkg.Foo", true);
    sub.functions.push(FunctionIr {
        name: "pkg.Foo.bar".to_owned(),
        kind: FunctionKindIr::Function,
        params: vec![],
        return_type: "int".to_owned(),
        docstring: None,
    });
    root.submodules = vec![sub];
    let ir = SchemaIr {
        modules: vec![root],
    };

    let error = Codegen::new(Config::default())
        .modules_from_ir(ir)
        .expect("external IR registration must defer schema validation")
        .generate()
        .expect_err("class/submodule collisions must fail IR validation");

    assert!(
        error.to_string().contains("collision") && error.to_string().contains("pkg.Foo"),
        "unexpected error: {error}",
    );
}

/// (#19) A class whose Python name requires sanitization (`Self` -> `s_Self`) that is used
/// as a base class must have its `<Base>Methods` supertrait and upcast references resolve
/// to the SANITIZED name, not the raw Python name (which would be a dangling reference,
/// E0405/E0412). Definition and reference sites must stay in lock-step.
#[test]
fn sanitized_class_name_is_applied_at_reference_sites() {
    let mut module = empty_module("pkg", true);
    module.classes = vec![
        class("pkg.Self", vec![]),
        class("pkg.Child", vec!["pkg.Self".to_owned()]),
    ];
    let ir = SchemaIr {
        modules: vec![module],
    };

    let tokens = Codegen::new(Config::default())
        .modules_from_ir(ir)
        .unwrap()
        .generate()
        .unwrap()
        .to_string();

    // The trait defined for the sanitized base is `s_SelfMethods`; the child's supertrait
    // and ancestor impl must reference that exact name.
    assert!(
        tokens.contains("ChildMethods : s_SelfMethods"),
        "the child's supertrait must reference the sanitized `s_SelfMethods`: {tokens}"
    );
    assert!(
        tokens.contains("impl s_SelfMethods for Child"),
        "the ancestor impl must reference the sanitized `s_SelfMethods`: {tokens}"
    );
    // No dangling reference to a bare `SelfMethods` / `Self` newtype must survive.
    assert!(
        !tokens.contains("AsRef < Self >"),
        "the upcast must target the sanitized newtype `s_Self`, not a dangling `Self`: {tokens}"
    );
}

/// (#20) A parameter literally named `_` must be RETAINED (under a synthetic name) so it
/// occupies its real positional slot; dropping it shifts every following parameter's index
/// and breaks positional call arity.
#[test]
fn underscore_param_is_retained_to_preserve_arity() {
    let mut module = empty_module("m", false);
    module.functions.push(FunctionIr {
        name: "m.f".to_owned(),
        kind: FunctionKindIr::Function,
        params: vec![
            ParameterIr {
                name: "_".to_owned(),
                kind: ParameterKindIr::PositionalOrKeyword,
                annotation: "int".to_owned(),
                has_default: false,
            },
            ParameterIr {
                name: "real".to_owned(),
                kind: ParameterKindIr::PositionalOrKeyword,
                annotation: "int".to_owned(),
                has_default: false,
            },
        ],
        return_type: "None".to_owned(),
        docstring: None,
    });
    let ir = SchemaIr {
        modules: vec![module],
    };

    let tokens = Codegen::new(Config::default())
        .modules_from_ir(ir)
        .unwrap()
        .generate()
        .unwrap()
        .to_string();

    // Both parameters must survive, in order: the `_` param under a synthetic name and the
    // real one after it (two positional arguments forwarded to Python).
    assert!(
        tokens.contains("p__arg0") && tokens.contains("p_real"),
        "the `_` param must be retained (synthetic name) alongside `real`: {tokens}"
    );
    // The forwarded positional argument tuple must carry both values.
    assert!(
        tokens.contains("into_bound_py_any (p__arg0")
            && tokens.contains("into_bound_py_any (p_real"),
        "both parameters must be forwarded as positional arguments: {tokens}"
    );
}

/// (#20) A `_` parameter's synthetic name must not collide with a real parameter literally
/// named like the synthetic (`_arg0`). Without the collision guard both would lower to
/// `p__arg0` in the same signature → E0415; the synthetic must be bumped instead.
#[test]
fn underscore_param_synthetic_name_avoids_real_param_collision() {
    let mut module = empty_module("m", false);
    module.functions.push(FunctionIr {
        name: "m.f".to_owned(),
        kind: FunctionKindIr::Function,
        params: vec![
            ParameterIr {
                name: "_".to_owned(),
                kind: ParameterKindIr::PositionalOrKeyword,
                annotation: "int".to_owned(),
                has_default: false,
            },
            ParameterIr {
                name: "_arg0".to_owned(),
                kind: ParameterKindIr::PositionalOrKeyword,
                annotation: "int".to_owned(),
                has_default: false,
            },
        ],
        return_type: "None".to_owned(),
        docstring: None,
    });
    let ir = SchemaIr {
        modules: vec![module],
    };

    let tokens = Codegen::new(Config::default())
        .modules_from_ir(ir)
        .unwrap()
        .generate()
        .unwrap()
        .to_string();

    // The real `_arg0` keeps `p__arg0`; the `_` param's synthetic name is bumped to
    // `_arg0_` (→ `p__arg0_`) so the two never share a binding. Without the guard the
    // synthetic would also be `p__arg0` and this bumped form would never appear.
    assert!(
        tokens.contains("p__arg0_"),
        "the `_` synthetic name must be bumped away from the real `_arg0`: {tokens}"
    );
}

/// A Python static method whose name collides with the generated inherent class API must
/// remain callable under a deterministic Rust alias instead of disappearing.
#[test]
fn static_method_colliding_with_inherent_api_is_renamed() {
    let mut widget = class("pkg.Widget", vec![]);
    widget.methods = vec![FunctionIr {
        name: "pkg.Widget.bind".to_owned(),
        kind: FunctionKindIr::Method {
            method_type: MethodKindIr::Static,
        },
        params: vec![ParameterIr {
            name: "x".to_owned(),
            kind: ParameterKindIr::PositionalOrKeyword,
            annotation: "int".to_owned(),
            has_default: false,
        }],
        return_type: "int".to_owned(),
        docstring: None,
    }];
    let mut module = empty_module("pkg", true);
    module.classes = vec![widget];
    let ir = SchemaIr {
        modules: vec![module],
    };

    let tokens = Codegen::new(Config::default())
        .modules_from_ir(ir)
        .unwrap()
        .generate()
        .unwrap()
        .to_string();

    assert!(
        tokens.contains("pub fn py_bind") && tokens.contains("intern ! (py , \"bind\")"),
        "a static method named `bind` must be emitted as `py_bind`: {tokens}"
    );
}

#[test]
fn static_method_named_repr_is_generated() {
    let mut widget = class("pkg.Widget", vec![]);
    widget.methods = vec![FunctionIr {
        name: "pkg.Widget.repr".to_owned(),
        kind: FunctionKindIr::Method {
            method_type: MethodKindIr::Static,
        },
        params: vec![],
        return_type: "str".to_owned(),
        docstring: None,
    }];
    let mut module = empty_module("pkg", true);
    module.classes = vec![widget];

    let tokens = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .unwrap()
        .generate()
        .unwrap()
        .to_string();

    assert!(
        tokens.contains("intern ! (py , \"repr\")"),
        "a non-colliding Python static method must be emitted: {tokens}"
    );
}

#[test]
fn module_property_named_bind_keeps_its_name() {
    let mut module = empty_module("pkg", false);
    module.properties.push(PropertyIr {
        name: "pkg.bind".to_owned(),
        py_type: "int".to_owned(),
        setter_type: None,
        readonly: true,
        docstring: None,
        setter_docstring: None,
    });

    let tokens = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .unwrap()
        .generate()
        .unwrap()
        .to_string();

    assert!(
        tokens.contains("fn bind"),
        "module properties do not share the class inherent namespace: {tokens}"
    );
    assert!(
        !tokens.contains("fn get_bind"),
        "module property was unnecessarily renamed: {tokens}"
    );
}

#[test]
fn generated_all_reexports_use_a_prelude_module() {
    let mut module = empty_module("pkg", false);
    module.prelude.push("value".to_owned());
    module.functions.push(FunctionIr {
        name: "pkg.value".to_owned(),
        kind: FunctionKindIr::Function,
        params: vec![],
        return_type: "int".to_owned(),
        docstring: None,
    });

    let tokens = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .unwrap()
        .generate()
        .unwrap()
        .to_string();

    assert!(
        tokens.contains("pub mod prelude"),
        "Python __all__ re-exports should live in a self-describing module: {tokens}"
    );
    assert!(
        !tokens.contains("pub mod call"),
        "the prelude module must not use callable-method naming: {tokens}"
    );
}

#[test]
fn class_name_cannot_collide_with_generated_methods_trait() {
    let mut module = empty_module("pkg", false);
    module.classes = vec![
        class("pkg.Widget", vec![]),
        class("pkg.WidgetMethods", vec![]),
    ];

    let error = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .expect("external IR registration must defer schema validation")
        .generate()
        .unwrap_err();

    assert!(
        error.to_string().contains("WidgetMethods"),
        "collision with a generated methods trait must be diagnosed: {error}"
    );
}

#[test]
fn live_class_name_cannot_collide_with_generated_methods_trait() {
    pyo3::Python::initialize();
    let error = pyo3::Python::attach(|py| {
        Codegen::prime_live_runtime(py).unwrap();
        let module = pyo3::types::PyModule::from_code(
            py,
            cstr("class Widget:\n    pass\n\nclass WidgetMethods:\n    pass\n").as_c_str(),
            cstr("live_collision.py").as_c_str(),
            cstr("live_collision").as_c_str(),
        )
        .unwrap();
        Codegen::default().module(&module).unwrap().generate()
    })
    .unwrap_err();

    assert!(
        error.to_string().contains("WidgetMethods"),
        "live naming collisions must be rejected before emitting invalid Rust: {error}"
    );
}

#[test]
fn type_var_cannot_collide_with_class_symbol() {
    let mut module = empty_module("pkg", false);
    module.classes.push(class("pkg.Widget", vec![]));
    module.type_vars.push("pkg.Widget".to_owned());

    let error = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .expect("external IR registration must defer schema validation")
        .generate()
        .unwrap_err();

    assert!(
        error.to_string().contains("Widget"),
        "type aliases and classes share Rust's type namespace: {error}"
    );
}

#[test]
fn sanitized_function_names_cannot_collide() {
    let mut module = empty_module("pkg", false);
    module.functions = ["pkg.not-valid", "pkg.f_not_valid"]
        .into_iter()
        .map(|name| FunctionIr {
            name: name.to_owned(),
            kind: FunctionKindIr::Function,
            params: vec![],
            return_type: "int".to_owned(),
            docstring: None,
        })
        .collect();

    let error = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .expect("external IR registration must defer schema validation")
        .generate()
        .unwrap_err();

    assert!(
        error.to_string().contains("f_not_valid"),
        "sanitized Rust value collisions must be diagnosed: {error}"
    );
}

#[test]
fn canonically_equivalent_function_names_cannot_collide() {
    let mut module = empty_module("pkg", false);
    module.functions = ["pkg.é", "pkg.e\u{301}"]
        .into_iter()
        .map(|name| FunctionIr {
            name: name.to_owned(),
            kind: FunctionKindIr::Function,
            params: vec![],
            return_type: "int".to_owned(),
            docstring: None,
        })
        .collect();

    let error = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .expect("external IR registration must defer schema validation")
        .generate()
        .expect_err("rustc normalizes emitted function identifiers to NFC");

    assert!(
        error
            .to_string()
            .contains("Rust value symbol collision 'é'"),
        "unexpected error: {error}"
    );
}

#[test]
fn sanitized_live_function_names_cannot_collide() {
    pyo3::Python::initialize();
    let error = pyo3::Python::attach(|py| {
        Codegen::prime_live_runtime(py).unwrap();
        let module = pyo3::types::PyModule::from_code(
            py,
            cstr(
                "def original() -> int:\n    return 1\n\nglobals()['not-valid'] = original\n\ndef f_not_valid() -> int:\n    return 2\n",
            )
            .as_c_str(),
            cstr("live_value_collision.py").as_c_str(),
            cstr("live_value_collision").as_c_str(),
        )
        .unwrap();
        Codegen::default().module(&module).unwrap().generate()
    })
    .unwrap_err();

    assert!(
        error.to_string().contains("f_not_valid"),
        "live Rust value collisions must be diagnosed: {error}"
    );
}

#[test]
fn dict_value_named_pyanything_remains_typed() {
    let mut module = empty_module("pkg", false);
    module.classes.push(class("pkg.PyAnything", vec![]));
    module.functions.push(FunctionIr {
        name: "pkg.consume".to_owned(),
        kind: FunctionKindIr::Function,
        params: vec![ParameterIr {
            name: "values".to_owned(),
            kind: ParameterKindIr::PositionalOrKeyword,
            annotation: "dict[str, pkg.PyAnything]".to_owned(),
            has_default: false,
        }],
        return_type: "None".to_owned(),
        docstring: None,
    });

    let tokens = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .unwrap()
        .generate()
        .unwrap()
        .to_string();

    assert!(
        tokens.contains("HashMap < :: std :: string :: String , PyAnything >"),
        "a concrete local type must not be mistaken for PyAny: {tokens}"
    );
    assert!(
        !tokens.contains("IntoPyDict :: into_py_dict (p_values"),
        "typed HashMap input should not be preconverted to PyDict: {tokens}"
    );
}

#[test]
fn dict_with_nested_any_value_uses_py_dict() {
    let tokens = tokens_for_param_annotation("dict[str, list[Any]]");

    assert!(tokens.contains("IntoPyDict < 'py >"), "{tokens}");
    assert!(
        tokens.contains("IntoPyDict :: into_py_dict (p_x"),
        "nested PyAny must retain PyDict preprocessing: {tokens}"
    );
    assert!(!tokens.contains("HashMap"), "{tokens}");
}

#[test]
fn sanitized_method_names_cannot_collide() {
    let mut widget = class("pkg.Widget", vec![]);
    widget.methods = ["not-valid", "f_not_valid"]
        .into_iter()
        .map(|name| FunctionIr {
            name: name.to_owned(),
            kind: FunctionKindIr::Method {
                method_type: MethodKindIr::Static,
            },
            params: vec![],
            return_type: "int".to_owned(),
            docstring: None,
        })
        .collect();
    let mut module = empty_module("pkg", false);
    module.classes.push(widget);

    let error = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .expect("external IR registration must defer schema validation")
        .generate()
        .unwrap_err();

    assert!(
        error.to_string().contains("f_not_valid"),
        "sanitized class-method collisions must be diagnosed: {error}"
    );
}

#[test]
fn sanitized_top_level_module_names_cannot_collide() {
    let error = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![empty_module("Self", false), empty_module("m_Self", false)],
        })
        .expect("external IR registration must defer schema validation")
        .generate()
        .unwrap_err();

    assert!(
        error.to_string().contains("m_Self"),
        "top-level Rust module collisions must be diagnosed: {error}"
    );
}

#[test]
fn sanitized_live_top_level_module_names_cannot_collide() {
    pyo3::Python::initialize();
    let error = pyo3::Python::attach(|py| {
        Codegen::prime_live_runtime(py).unwrap();
        let first = pyo3::types::PyModule::from_code(
            py,
            cstr("").as_c_str(),
            cstr("Self.py").as_c_str(),
            cstr("Self").as_c_str(),
        )
        .unwrap();
        let second = pyo3::types::PyModule::from_code(
            py,
            cstr("").as_c_str(),
            cstr("m_Self.py").as_c_str(),
            cstr("m_Self").as_c_str(),
        )
        .unwrap();
        Codegen::default()
            .module(&first)
            .unwrap()
            .module(&second)
            .unwrap()
            .generate()
    })
    .unwrap_err();

    assert!(
        error.to_string().contains("m_Self"),
        "live root module collisions must be diagnosed: {error}"
    );
}

#[test]
fn top_level_module_cannot_collide_with_generated_root_type() {
    let reserved = empty_module("PyObjectBase", false);
    let mut package = empty_module("pkg", false);
    package.classes.push(class("pkg.Widget", vec![]));

    let error = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![reserved, package],
        })
        .expect("external IR registration must defer schema validation")
        .generate()
        .unwrap_err();

    assert!(
        error.to_string().contains("PyObjectBase"),
        "generated root symbols must participate in collision validation: {error}"
    );
}

#[test]
fn import_alias_cannot_collide_with_class_symbol() {
    let mut module = empty_module("pkg", true);
    module.classes.push(class("pkg.Widget", vec![]));
    module.submodules.push(empty_module("pkg.bar", false));
    module.imports.push(ImportIr {
        origin: "pkg.bar".to_owned(),
        target: "pkg.Widget".to_owned(),
    });

    let error = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .expect("external IR registration must defer schema validation")
        .generate()
        .unwrap_err();

    assert!(
        error.to_string().contains("Widget"),
        "import aliases must participate in Rust type collision validation: {error}"
    );
}

#[test]
fn disabled_type_import_alias_does_not_collide_with_emitted_class() {
    let mut module = empty_module("pkg", true);
    module.classes.push(class("pkg.Alias", vec![]));
    let mut child = empty_module("pkg.child", false);
    child.classes.push(class("pkg.child.C", vec![]));
    module.submodules.push(child);
    module.imports.push(ImportIr {
        origin: "pkg.child.C".to_owned(),
        target: "pkg.Alias".to_owned(),
    });

    let tokens = Codegen::new(Config::builder().generate_imports(false).build())
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .unwrap()
        .generate()
        .unwrap()
        .to_string();

    assert!(tokens.contains("pub struct Alias"), "{tokens}");
    assert!(tokens.contains("pub struct C"), "{tokens}");
    assert!(!tokens.contains("pub use"), "{tokens}");
}

#[test]
fn disabled_import_cannot_override_authored_annotation_type() {
    let mut module = empty_module("pkg", true);
    module.classes.push(class("pkg.X", vec![]));
    module.imports.push(ImportIr {
        origin: "typing.Type".to_owned(),
        target: "pkg.X".to_owned(),
    });
    module.functions.push(FunctionIr {
        name: "pkg.echo".to_owned(),
        kind: FunctionKindIr::Function,
        params: vec![ParameterIr {
            name: "value".to_owned(),
            kind: ParameterKindIr::PositionalOrKeyword,
            annotation: "X[int, str]".to_owned(),
            has_default: false,
        }],
        return_type: "X[int, str]".to_owned(),
        docstring: None,
    });

    let tokens = Codegen::new(Config::builder().generate_imports(false).build())
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .expect("a disabled import must not override an emitted declaration")
        .generate()
        .expect("the authored annotation type must lower");
    let source = prettyplease::unparse(
        &syn::parse2(tokens).expect("generated bindings must remain valid Rust syntax"),
    );
    let signature = source
        .split("pub fn echo<'py>(")
        .nth(1)
        .and_then(|rest| rest.split('{').next())
        .unwrap_or_else(|| panic!("missing generated function:\n{source}"));
    assert!(
        signature.contains("p_value: &X") && signature.contains("PyResult<X>"),
        "the authored class must retain declaration precedence:\n{signature}",
    );
}

#[test]
fn ambiguous_disabled_annotation_imports_are_order_independent() {
    fn signatures(mut imports: Vec<ImportIr>) -> (String, String) {
        let function = |name: &str, annotation: &str| FunctionIr {
            name: format!("pkg.{name}"),
            kind: FunctionKindIr::Function,
            params: vec![ParameterIr {
                name: "value".to_owned(),
                kind: ParameterKindIr::PositionalOrKeyword,
                annotation: annotation.to_owned(),
                has_default: false,
            }],
            return_type: annotation.to_owned(),
            docstring: None,
        };
        let mut module = empty_module("pkg", true);
        module.imports.append(&mut imports);
        module.functions = vec![
            function("unqualified", "X[int]"),
            function("qualified", "pkg.X[int]"),
        ];

        let tokens = Codegen::new(Config::builder().generate_imports(false).build())
            .modules_from_ir(SchemaIr {
                modules: vec![module],
            })
            .expect("disabled ambiguous imports must remain inert")
            .generate()
            .expect("ambiguous annotations must safely degrade");
        let source = prettyplease::unparse(
            &syn::parse2(tokens).expect("generated bindings must remain valid Rust syntax"),
        );
        let signature = |name: &str| {
            source
                .split(&format!("pub fn {name}<'py>("))
                .nth(1)
                .and_then(|rest| rest.split('{').next())
                .map(str::to_owned)
                .unwrap_or_else(|| panic!("missing generated function `{name}`:\n{source}"))
        };
        (signature("unqualified"), signature("qualified"))
    }

    let imports = vec![
        ImportIr {
            origin: "typing.Type".to_owned(),
            target: "pkg.X".to_owned(),
        },
        ImportIr {
            origin: "typing.List".to_owned(),
            target: "pkg.X".to_owned(),
        },
    ];
    let forward = signatures(imports.clone());
    let reverse = signatures(imports.into_iter().rev().collect());

    assert_eq!(
        forward, reverse,
        "ambiguous disabled imports must not be resolved by last-wins input order",
    );
    for signature in [&forward.0, &forward.1] {
        assert!(
            signature.contains("IntoPyObject")
                && signature.contains("types::PyAny")
                && !signature.contains("types::PyType"),
            "ambiguous aliases must degrade dynamically:\n{signature}",
        );
    }
}

#[test]
fn disabled_value_import_alias_does_not_collide_with_emitted_function() {
    let mut module = empty_module("pkg", true);
    module.functions.push(FunctionIr {
        name: "pkg.Alias".to_owned(),
        kind: FunctionKindIr::Function,
        params: vec![],
        return_type: "int".to_owned(),
        docstring: None,
    });
    let mut child = empty_module("pkg.child", false);
    child.functions.push(FunctionIr {
        name: "pkg.child.C".to_owned(),
        kind: FunctionKindIr::Function,
        params: vec![],
        return_type: "int".to_owned(),
        docstring: None,
    });
    module.submodules.push(child);
    module.imports.push(ImportIr {
        origin: "pkg.child.C".to_owned(),
        target: "pkg.Alias".to_owned(),
    });

    let tokens = Codegen::new(Config::builder().generate_imports(false).build())
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .unwrap()
        .generate()
        .unwrap()
        .to_string();

    assert!(tokens.contains("pub fn Alias"), "{tokens}");
    assert!(tokens.contains("pub fn C"), "{tokens}");
    assert!(!tokens.contains("pub use"), "{tokens}");
}

#[test]
fn ignored_submodule_type_symbols_do_not_collide() {
    let mut module = empty_module("pkg", true);
    let mut child = empty_module("pkg.child", false);
    child.classes.push(class("pkg.child.Self", vec![]));
    child.classes.push(class("pkg.child.s_Self", vec![]));
    module.submodules.push(child);

    let tokens = Codegen::new(Config::builder().traverse_submodules(false).build())
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .unwrap()
        .generate()
        .unwrap()
        .to_string();

    assert!(!tokens.contains("pub mod child"), "{tokens}");
}

#[test]
fn ignored_submodule_value_symbols_do_not_collide() {
    let mut module = empty_module("pkg", true);
    let mut child = empty_module("pkg.child", false);
    for name in ["not-valid", "f_not_valid"] {
        child.functions.push(FunctionIr {
            name: format!("pkg.child.{name}"),
            kind: FunctionKindIr::Function,
            params: vec![],
            return_type: "int".to_owned(),
            docstring: None,
        });
    }
    module.submodules.push(child);

    let tokens = Codegen::new(Config::builder().traverse_submodules(false).build())
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .unwrap()
        .generate()
        .unwrap()
        .to_string();

    assert!(!tokens.contains("pub mod child"), "{tokens}");
}

#[test]
fn ignored_flat_dotted_type_symbols_do_not_collide() {
    let mut child = empty_module("pkg.child", false);
    child.classes.push(class("pkg.child.Self", vec![]));
    child.classes.push(class("pkg.child.s_Self", vec![]));

    let tokens = Codegen::new(Config::builder().traverse_submodules(false).build())
        .modules_from_ir(SchemaIr {
            modules: vec![child],
        })
        .unwrap()
        .generate()
        .unwrap()
        .to_string();

    assert!(tokens.contains("pub mod pkg"), "{tokens}");
    assert!(!tokens.contains("pub mod child"), "{tokens}");
}

#[test]
fn ignored_flat_dotted_value_symbols_do_not_collide() {
    let mut child = empty_module("pkg.child", false);
    for name in ["not-valid", "f_not_valid"] {
        child.functions.push(FunctionIr {
            name: format!("pkg.child.{name}"),
            kind: FunctionKindIr::Function,
            params: vec![],
            return_type: "int".to_owned(),
            docstring: None,
        });
    }

    let tokens = Codegen::new(Config::builder().traverse_submodules(false).build())
        .modules_from_ir(SchemaIr {
            modules: vec![child],
        })
        .unwrap()
        .generate()
        .unwrap()
        .to_string();

    assert!(tokens.contains("pub mod pkg"), "{tokens}");
    assert!(!tokens.contains("pub mod child"), "{tokens}");
}

#[test]
fn ignored_flat_dotted_prelude_target_is_not_validated() {
    let mut child = empty_module("pkg.child", false);
    child.prelude.push("Missing".to_owned());

    let tokens = Codegen::new(Config::builder().traverse_submodules(false).build())
        .modules_from_ir(SchemaIr {
            modules: vec![child],
        })
        .unwrap()
        .generate()
        .unwrap()
        .to_string();

    assert!(tokens.contains("pub mod pkg"), "{tokens}");
    assert!(!tokens.contains("pub mod child"), "{tokens}");
}

#[test]
fn ignored_flat_dotted_class_does_not_require_root_support() {
    let mut child = empty_module("PyObjectBase.child", false);
    child
        .classes
        .push(class("PyObjectBase.child.Widget", vec![]));

    let tokens = Codegen::new(Config::builder().traverse_submodules(false).build())
        .modules_from_ir(SchemaIr {
            modules: vec![child],
        })
        .unwrap()
        .generate()
        .unwrap()
        .to_string();

    assert!(tokens.contains("pub mod PyObjectBase"), "{tokens}");
    assert!(!tokens.contains("pub trait PyObjectBase"), "{tokens}");
    assert!(!tokens.contains("pub mod child"), "{tokens}");
}

#[test]
fn prelude_resolves_flat_dotted_direct_child() {
    let mut parent = empty_module("pkg", true);
    parent.prelude.push("child".to_owned());
    let child = empty_module("pkg.child", false);

    let tokens = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![parent, child],
        })
        .unwrap()
        .generate()
        .unwrap()
        .to_string();

    assert!(tokens.contains("pub mod child"), "{tokens}");
    assert!(tokens.contains("pub use super :: { child"), "{tokens}");
}

#[test]
fn prelude_resolves_deep_flat_dotted_synthetic_child() {
    let mut parent = empty_module("pkg", true);
    parent.prelude.push("middle".to_owned());
    let descendant = empty_module("pkg.middle.child", false);

    let tokens = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![parent, descendant],
        })
        .unwrap()
        .generate()
        .unwrap()
        .to_string();

    assert!(tokens.contains("pub mod middle"), "{tokens}");
    assert!(tokens.contains("pub use super :: { middle"), "{tokens}");
}

#[test]
fn disabled_traversal_reports_flat_prelude_child_as_disabled() {
    let mut parent = empty_module("pkg", true);
    parent.prelude.push("child".to_owned());
    let child = empty_module("pkg.child", false);

    let error = Codegen::new(Config::builder().traverse_submodules(false).build())
        .modules_from_ir(SchemaIr {
            modules: vec![parent, child],
        })
        .expect("external IR registration must defer schema validation")
        .generate()
        .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("module disabled by generation configuration"),
        "{error}"
    );
}

#[test]
fn prelude_uses_the_same_sanitized_name_as_its_class() {
    let mut module = empty_module("pkg", false);
    module.classes.push(class("pkg.Self", vec![]));
    module.prelude.push("Self".to_owned());

    let tokens = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .unwrap()
        .generate()
        .unwrap()
        .to_string();

    assert!(
        tokens.contains("pub struct s_Self") && tokens.contains("super :: { s_Self"),
        "prelude references must share the class definition's canonical name: {tokens}"
    );
}

#[test]
fn module_function_colliding_with_provenance_constant_is_renamed() {
    let mut module = empty_module("pkg", false);
    module.prelude.push("PYO3_BINDGEN_MODULE_NAME".to_owned());
    module.functions.push(FunctionIr {
        name: "pkg.PYO3_BINDGEN_MODULE_NAME".to_owned(),
        kind: FunctionKindIr::Function,
        params: vec![],
        return_type: "str".to_owned(),
        docstring: None,
    });

    let tokens = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .unwrap()
        .generate()
        .unwrap()
        .to_string();

    assert!(
        tokens.contains("fn py_PYO3_BINDGEN_MODULE_NAME")
            && tokens.contains("\"PYO3_BINDGEN_MODULE_NAME\""),
        "generated metadata must not erase or collide with Python functions: {tokens}"
    );
    assert!(
        tokens.contains("super :: { py_PYO3_BINDGEN_MODULE_NAME"),
        "prelude must re-export the resolved function alias, not the metadata constant: {tokens}"
    );
}

#[test]
fn provenance_name_is_available_when_provenance_is_disabled() {
    let mut module = empty_module("pkg", false);
    module.functions.push(FunctionIr {
        name: "pkg.PYO3_BINDGEN_MODULE_NAME".to_owned(),
        kind: FunctionKindIr::Function,
        params: vec![],
        return_type: "str".to_owned(),
        docstring: None,
    });

    let tokens = Codegen::new(Config::builder().generate_provenance(false).build())
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .unwrap()
        .generate()
        .unwrap()
        .to_string();

    assert!(tokens.contains("fn PYO3_BINDGEN_MODULE_NAME"));
    assert!(!tokens.contains("fn py_PYO3_BINDGEN_MODULE_NAME"));
}

/// `IntEnum` members are declared out of ascending value order (`HIGH` first),
/// plus a negative value. Every variant must carry its real Python integer
/// value as an explicit Rust discriminant: otherwise the mirror's derived
/// `Ord` would silently follow declaration order (`High < Low < Medium`)
/// instead of Python's `IntEnum` value order (`BelowZero < Low < Medium <
/// High`).
#[test]
fn external_ir_int_enum_gets_value_discriminants_for_correct_ord() {
    let mut module = empty_module("pkg", false);
    module.enums.push(EnumIr {
        name: "pkg.Priority".to_owned(),
        kind: EnumKindIr::IntEnum,
        variants: vec![
            EnumVariantIr {
                py_name: "HIGH".to_owned(),
                value_repr: "3".to_owned(),
            },
            EnumVariantIr {
                py_name: "LOW".to_owned(),
                value_repr: "1".to_owned(),
            },
            EnumVariantIr {
                py_name: "MEDIUM".to_owned(),
                value_repr: "2".to_owned(),
            },
            EnumVariantIr {
                py_name: "BELOW_ZERO".to_owned(),
                value_repr: "-5".to_owned(),
            },
        ],
        aliases: vec![],
        docstring: None,
        is_flag: false,
        is_open: false,
        string_backed: false,
        plain_mirror: false,
    });

    let tokens = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .unwrap()
        .generate()
        .unwrap()
        .to_string();

    for expected in [
        "High = 3 ,",
        "Low = 1 ,",
        "Medium = 2 ,",
        "BelowZero = - 5 ,",
    ] {
        assert!(
            tokens.contains(expected),
            "missing expected discriminant `{expected}` in: {tokens}"
        );
    }
    assert!(
        tokens.contains("PartialOrd , Ord ,"),
        "IntEnum mirror must derive Ord for the discriminant fix to matter: {tokens}"
    );
}

/// A Rust discriminant is an integer and cannot encode a `StrEnum` member's
/// real string value, so the mirror must not derive `Ord`/`PartialOrd` for
/// `StrEnum` (it would silently order by declaration position instead of
/// Python's lexicographic string order, or worse, not compile at all).
#[test]
fn external_ir_str_enum_does_not_derive_ord() {
    let mut module = empty_module("pkg", false);
    module.enums.push(EnumIr {
        name: "pkg.Mode".to_owned(),
        kind: EnumKindIr::StrEnum,
        variants: vec![
            EnumVariantIr {
                py_name: "FAST".to_owned(),
                value_repr: "'fast'".to_owned(),
            },
            EnumVariantIr {
                py_name: "SLOW".to_owned(),
                value_repr: "'slow'".to_owned(),
            },
        ],
        aliases: vec![],
        docstring: None,
        is_flag: false,
        is_open: false,
        string_backed: false,
        plain_mirror: false,
    });

    let tokens = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .unwrap()
        .generate()
        .unwrap()
        .to_string();

    assert!(
        tokens.contains("pub enum Mode"),
        "missing Mode enum: {tokens}"
    );
    assert!(
        !tokens.contains("PartialOrd"),
        "StrEnum mirror must not derive PartialOrd/Ord: {tokens}"
    );
}

#[test]
fn raw_keyword_methods_trait_collision_is_rejected() {
    // Regression: a class named after a raw-escapable keyword (`match` -> `r#match`)
    // emits a `*Methods` supertrait spelled `r#matchMethods`, which rustc treats as the
    // SAME identifier as a sibling type literally named `matchMethods` (E0428). The
    // symbol table must reject it -- at parity with the non-keyword `Foo`/`FooMethods`
    // case -- rather than silently emit uncompilable bindings. Guards the `r#`-stripping
    // in `canonical_rust_name`.
    let mut module = empty_module("pkg", true);
    module.classes = vec![
        class("pkg.match", vec![]),
        class("pkg.matchMethods", vec![]),
    ];

    let error = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .expect("external IR registration must defer schema validation")
        .generate()
        .expect_err("raw-keyword *Methods trait must not silently collide with a sibling type");

    assert!(
        error
            .to_string()
            .contains("Rust type symbol collision 'matchMethods'"),
        "unexpected error: {error}",
    );
}

#[test]
fn free_function_self_rename_avoids_parameter_collision() {
    // Regression: a free function whose first parameter is `self` has that receiver
    // renamed to `__unknown_self__`; a second parameter literally named
    // `__unknown_self__` must not share the same Rust binding `p___unknown_self__`
    // (E0415: bound more than once). The rename is routed through the same
    // collision-avoider as the `_` -> `_arg{N}` branch, so the receiver takes a distinct
    // suffixed name.
    let mut module = empty_module("pkg", false);
    module.functions.push(FunctionIr {
        name: "pkg.f".to_owned(),
        kind: FunctionKindIr::Function,
        params: vec![
            ParameterIr {
                name: "self".to_owned(),
                kind: ParameterKindIr::PositionalOrKeyword,
                annotation: String::new(),
                has_default: false,
            },
            ParameterIr {
                name: "__unknown_self__".to_owned(),
                kind: ParameterKindIr::PositionalOrKeyword,
                annotation: "int".to_owned(),
                has_default: false,
            },
        ],
        return_type: String::new(),
        docstring: None,
    });

    let tokens = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .unwrap()
        .generate()
        .unwrap()
        .to_string();

    // The literal parameter keeps `p___unknown_self__`; the renamed receiver is bumped to
    // a distinct binding with an extra underscore. Both distinct spellings must appear --
    // pre-fix they collapsed to one and rustc rejected the duplicate binding.
    assert!(
        tokens.contains("p___unknown_self__"),
        "literal __unknown_self__ parameter binding missing: {tokens}"
    );
    assert!(
        tokens.contains("p___unknown_self___"),
        "renamed receiver must take a distinct, collision-avoided binding: {tokens}"
    );
}

/// The external-IR path must keep HARD-ERRORING on parameter names that collide only
/// after NFC normalization -- for a class method just as for a free function. The
/// canonical-binding pass in `Function::finalize_parameters` is a live-path backstop and
/// must NOT be mistaken for a licence to soften this validator into a warn-and-rename:
/// an IR producer emitting such a pair has a bug the consumer needs told about, and
/// silently renaming would change the generated API behind its back.
#[test]
fn canonically_equivalent_method_parameter_names_are_rejected() {
    let mut module = empty_module("pkg", false);
    let mut cls = class("pkg.Cls", vec![]);
    cls.methods = vec![FunctionIr {
        name: "pkg.Cls.run".to_owned(),
        kind: FunctionKindIr::Method {
            method_type: MethodKindIr::Instance,
        },
        params: ["self", "\u{e9}", "e\u{301}"]
            .into_iter()
            .map(|name| ParameterIr {
                name: name.to_owned(),
                kind: ParameterKindIr::PositionalOrKeyword,
                annotation: "int".to_owned(),
                has_default: false,
            })
            .collect(),
        return_type: "int".to_owned(),
        docstring: None,
    }];
    module.classes = vec![cls];

    let error = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .expect("external IR registration must defer schema validation")
        .generate()
        .expect_err("Rust normalizes identifiers to NFC before binding parameters");

    assert!(
        error.to_string().contains("colliding Rust parameter name"),
        "unexpected error: {error}"
    );
}

// --- External-IR enum class-fallback (`enum_ir_class_fallback`) -----------------------------

/// A plain `enum.Enum` whose two canonical members `red` and `RED` both case-fold to the
/// same Rust variant `Red`, so it cannot be mirrored as a Rust enum.
fn colliding_color_enum(name: &str) -> EnumIr {
    EnumIr {
        name: name.to_owned(),
        kind: EnumKindIr::Enum,
        variants: vec![
            EnumVariantIr {
                py_name: "red".to_owned(),
                value_repr: "1".to_owned(),
            },
            EnumVariantIr {
                py_name: "RED".to_owned(),
                value_repr: "2".to_owned(),
            },
        ],
        aliases: vec![],
        docstring: None,
        is_flag: false,
        is_open: false,
        string_backed: false,
        plain_mirror: false,
    }
}

/// Write `source` into a throwaway crate that depends on the pinned pyo3 and run
/// `cargo check --offline`, asserting it compiles. `tag` disambiguates the temp dir.
fn assert_ir_source_compiles(source: &str, tag: &str) {
    let mut guard = OwnedTempDir::allocate(tag).unwrap_or_else(|error| {
        panic!("failed to allocate IR compile fixture directory for `{tag}`: {error}")
    });
    let crate_dir = guard.path().to_owned();
    std::fs::create_dir_all(crate_dir.join("src")).unwrap();

    // Minimal consumer manifest. The pyo3 feature set matches the workspace pin so a
    // `cargo check` reuses already-cached crate sources and can run offline.
    let pyo3_version = env!("PYO3_BINDGEN_PYO3_VERSION");
    std::fs::write(
        crate_dir.join("Cargo.toml"),
        format!(
            r#"[package]
name = "pyo3_bindgen_enum_fallback_fixture"
version = "0.0.0"
edition = "2021"
publish = false

[workspace]

[dependencies]
pyo3 = {{ version = "={pyo3_version}", default-features = false, features = ["py-clone"] }}
"#
        ),
    )
    .unwrap();
    std::fs::write(crate_dir.join("src/lib.rs"), source).unwrap();

    let mut command = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned()));
    command
        .arg("check")
        .arg("--quiet")
        .arg("--offline")
        .current_dir(&crate_dir)
        .env("CARGO_TARGET_DIR", crate_dir.join("target"));
    let output = match guard.run(&mut command, CARGO_COMMAND_TIMEOUT) {
        Ok(output) => output,
        Err(error) => panic!(
            "generated enum-fallback crate failed before exit\n{error}\nstdout:\n{}\nstderr:\n{}\n\n--- generated lib.rs ---\n{source}",
            error.stdout(),
            error.stderr(),
        ),
    };

    assert!(
        output.status.success(),
        "generated enum-fallback crate failed to compile\nstatus: {}\nstdout:\n{}\nstderr:\n{}\n\n--- generated lib.rs ---\n{source}",
        output.status,
        output.stdout,
        output.stderr,
    );
}

/// Write `source` into a throwaway crate that depends on the pinned pyo3 and run
/// its bounded runtime test suite with Python auto-initialization. `tag` disambiguates
/// the temp dir; `--offline` keeps the consumer test independent of the network.
fn assert_ir_source_runs(source: &str, tag: &str) {
    let mut guard = OwnedTempDir::allocate(tag).unwrap_or_else(|error| {
        panic!("failed to allocate IR runtime fixture directory for `{tag}`: {error}")
    });
    let crate_dir = guard.path().to_owned();
    std::fs::create_dir_all(crate_dir.join("src")).unwrap();

    let pyo3_version = env!("PYO3_BINDGEN_PYO3_VERSION");
    std::fs::write(
        crate_dir.join("Cargo.toml"),
        format!(
            r#"[package]
name = "pyo3_bindgen_{tag}_fixture"
version = "0.0.0"
edition = "2021"
publish = false

[workspace]

[dependencies]
pyo3 = {{ version = "={pyo3_version}", features = ["auto-initialize"] }}
"#
        ),
    )
    .unwrap();
    std::fs::write(crate_dir.join("src/lib.rs"), source).unwrap();

    let mut command = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned()));
    command
        .arg("test")
        .arg("--quiet")
        .arg("--offline")
        .current_dir(&crate_dir)
        .env("CARGO_TARGET_DIR", crate_dir.join("target"));
    let output = match guard.run(&mut command, CARGO_COMMAND_TIMEOUT) {
        Ok(output) => output,
        Err(error) => panic!(
            "generated collection runtime crate failed before exit\n{error}\nstdout:\n{}\nstderr:\n{}\n\n--- generated lib.rs ---\n{source}",
            error.stdout(),
            error.stderr(),
        ),
    };

    assert!(
        output.status.success(),
        "generated collection runtime crate failed\nstatus: {}\nstdout:\n{}\nstderr:\n{}\n\n--- generated lib.rs ---\n{source}",
        output.status,
        output.stdout,
        output.stderr,
    );
}

#[test]
fn bounded_consumer_timeout_is_proven_by_exact_child_marker() {
    bounded_consumer::assert_timeout_regression(
        "bounded_consumer_timeout_is_proven_by_exact_child_marker",
    );
}

/// Default config: a non-representable enum still aborts the whole generation loudly, exactly
/// as before this flag existed (this is the `Err(err) => return Err(err)` arm).
#[test]
fn ir_enum_variant_collision_aborts_by_default() {
    let mut module = empty_module("pkg", false);
    module.enums = vec![colliding_color_enum("pkg.Color")];

    let error = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .expect("external IR registration must defer schema validation")
        .generate()
        .expect_err("a colliding-member enum must abort generation under the default config");

    assert!(
        error
            .to_string()
            .contains("Rust enum variant name collision"),
        "unexpected error: {error}"
    );
}

/// With `enum_ir_class_fallback`, the same collision degrades to an owning newtype plus a
/// distinct `enum_ir_mirror_fallback` warning, instead of aborting.
#[test]
fn ir_enum_variant_collision_falls_back_to_class() {
    let mut module = empty_module("pkg", false);
    module.enums = vec![colliding_color_enum("pkg.Color")];

    let cfg = Config::builder().enum_ir_class_fallback(true).build();
    let generated = Codegen::new(cfg)
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .expect("the fallback flag must turn the abort into a class wrapper")
        .generate_with_report()
        .expect("fallback generation must succeed");

    // A distinct IR-path code, so report consumers can tell it apart from the live path's
    // `enum_mirror_fallback`.
    assert!(
        generated.report.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "enum_ir_mirror_fallback"
                && diagnostic.path.as_deref() == Some("pkg.Color")
        }),
        "expected an `enum_ir_mirror_fallback` warning at `pkg.Color`: {:?}",
        generated.report.diagnostics
    );

    let tokens = generated.tokens.to_string();
    assert!(
        tokens.contains("pub struct Color") && !tokens.contains("pub enum Color"),
        "the fallen-back enum must emit an owning newtype, not a Rust enum:\n{tokens}"
    );
}

/// End-to-end compile oracle: a property typed with the fallen-back enum lowers to the owning
/// newtype, `set[E]` degrades from `HashSet<E>` to `Bound<PySet>` (a newtype derives only
/// `Debug`, so `HashSet<E>` would not compile), and the whole crate compiles with no dangling
/// reference to the dropped enum.
#[test]
fn ir_enum_fallback_reference_lowers_and_compiles() {
    let mut module = empty_module("m", true);
    module.enums = vec![colliding_color_enum("m.Color")];
    module.classes = vec![ClassIr {
        name: "m.Holder".to_owned(),
        base_classes: vec![],
        docstring: None,
        methods: vec![],
        properties: vec![
            PropertyIr {
                name: "m.Holder.tone".to_owned(),
                py_type: "m.Color".to_owned(),
                setter_type: None,
                readonly: true,
                docstring: None,
                setter_docstring: None,
            },
            PropertyIr {
                name: "m.Holder.tones".to_owned(),
                py_type: "set[m.Color]".to_owned(),
                setter_type: None,
                readonly: true,
                docstring: None,
                setter_docstring: None,
            },
        ],
    }];

    let cfg = Config::builder().enum_ir_class_fallback(true).build();
    let generated = Codegen::new(cfg)
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .expect("fallback IR must build")
        .generate()
        .expect("fallback IR must generate")
        .to_string();

    // The fallen-back enum is now a class newtype...
    assert!(
        generated.contains("pub struct Color") && !generated.contains("pub enum Color"),
        "fallen-back enum must lower to a newtype:\n{generated}"
    );
    // ...so `set[Color]` degrades to `Bound<PySet>`, never a typed `HashSet<Color>`.
    assert!(
        generated.contains("PySet") && !generated.contains("HashSet < Color >"),
        "set[Color] over a fallen-back enum must degrade to Bound<PySet>:\n{generated}"
    );

    assert_ir_source_compiles(&generated, "reference");
}

/// The remaining `classify_variants` error sites (no members, an alias targeting a missing
/// canonical member, an unrepresentable member ident) all warn and fall back rather than
/// aborting when the flag is on.
#[test]
fn ir_enum_fallback_covers_classify_variants_error_sites() {
    let build = |py_enum: EnumIr| {
        let name = py_enum.name.clone();
        let mut module = empty_module("pkg", false);
        module.enums = vec![py_enum];
        let cfg = Config::builder().enum_ir_class_fallback(true).build();
        let generated = Codegen::new(cfg)
            .modules_from_ir(SchemaIr {
                modules: vec![module],
            })
            .unwrap_or_else(|error| panic!("fallback must not abort for {name}: {error}"))
            .generate_with_report()
            .unwrap_or_else(|error| panic!("fallback must generate for {name}: {error}"));
        assert!(
            generated.report.diagnostics.iter().any(|diagnostic| {
                diagnostic.code == "enum_ir_mirror_fallback"
                    && diagnostic.path.as_deref() == Some(name.as_str())
            }),
            "expected an `enum_ir_mirror_fallback` warning for {name}: {:?}",
            generated.report.diagnostics
        );
        let tokens = generated.tokens.to_string();
        assert!(
            tokens.contains("pub struct") && !tokens.contains("pub enum"),
            "{name} must fall back to a newtype:\n{tokens}"
        );
    };

    // No members at all.
    build(EnumIr {
        name: "pkg.Empty".to_owned(),
        kind: EnumKindIr::Enum,
        variants: vec![],
        aliases: vec![],
        docstring: None,
        is_flag: false,
        is_open: false,
        string_backed: false,
        plain_mirror: false,
    });
    // Alias pointing at a canonical member that does not exist.
    build(EnumIr {
        name: "pkg.Dangling".to_owned(),
        kind: EnumKindIr::Enum,
        variants: vec![EnumVariantIr {
            py_name: "REAL".to_owned(),
            value_repr: "1".to_owned(),
        }],
        aliases: vec![EnumAliasIr {
            py_name: "ALIAS".to_owned(),
            target_py_name: "MISSING".to_owned(),
            value_repr: "1".to_owned(),
        }],
        docstring: None,
        is_flag: false,
        is_open: false,
        string_backed: false,
        plain_mirror: false,
    });
    // A member whose only character is alphanumeric-but-non-XID (`\u{b2}`, superscript two),
    // so `enum_variant_ident` returns `None`.
    build(EnumIr {
        name: "pkg.Unrepresentable".to_owned(),
        kind: EnumKindIr::Enum,
        variants: vec![EnumVariantIr {
            py_name: "\u{b2}".to_owned(),
            value_repr: "1".to_owned(),
        }],
        aliases: vec![],
        docstring: None,
        is_flag: false,
        is_open: false,
        string_backed: false,
        plain_mirror: false,
    });
}

/// Byte-neutrality: when every enum in the schema is representable, the fallback path is
/// unreachable, so toggling the flag must not perturb a single output byte.
#[test]
fn ir_enum_class_fallback_is_byte_neutral_for_representable_enums() {
    let off = Codegen::new(Config::default())
        .modules_from_ir(sample_schema())
        .unwrap()
        .generate()
        .unwrap()
        .to_string();
    let on = Codegen::new(Config::builder().enum_ir_class_fallback(true).build())
        .modules_from_ir(sample_schema())
        .unwrap()
        .generate()
        .unwrap()
        .to_string();

    assert_eq!(
        off, on,
        "enabling `enum_ir_class_fallback` must be byte-neutral when no enum fails to lower"
    );
}
