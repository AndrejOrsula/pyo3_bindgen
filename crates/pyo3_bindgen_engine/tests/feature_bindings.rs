#![cfg(any(feature = "num-complex", feature = "numpy"))]

#[cfg(feature = "numpy")]
use pyo3_bindgen_engine::ClassIr;
use pyo3_bindgen_engine::{
    Codegen, Config, EnumIr, EnumKindIr, EnumVariantIr, FunctionIr, FunctionKindIr, ImportIr,
    MethodKindIr, ModuleIr, ParameterIr, ParameterKindIr, PropertyIr, SchemaIr,
};

#[path = "../test_support/bounded_consumer.rs"]
mod bounded_consumer;

fn generated_function(annotation: &str) -> String {
    let module = ModuleIr {
        name: "feature_types".to_owned(),
        docstring: None,
        is_package: false,
        classes: vec![],
        enums: vec![],
        functions: vec![FunctionIr {
            name: "feature_types.echo".to_owned(),
            kind: FunctionKindIr::Function,
            params: vec![ParameterIr {
                name: "value".to_owned(),
                kind: ParameterKindIr::PositionalOrKeyword,
                annotation: annotation.to_owned(),
                has_default: false,
            }],
            return_type: annotation.to_owned(),
            docstring: None,
        }],
        properties: vec![],
        submodules: vec![],
        prelude: vec![],
        imports: vec![],
        type_vars: vec![],
    };
    let tokens = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .unwrap()
        .generate()
        .unwrap();
    prettyplease::unparse(&syn::parse2(tokens).unwrap())
}

#[test]
fn exact_import_alias_type_references_compile_for_consumers() {
    let child = ModuleIr {
        name: "alias_types.sub".to_owned(),
        docstring: None,
        is_package: false,
        classes: vec![pyo3_bindgen_engine::ClassIr {
            name: "alias_types.sub.Widget".to_owned(),
            base_classes: vec![],
            docstring: None,
            methods: vec![],
            properties: vec![],
        }],
        enums: vec![],
        functions: vec![],
        properties: vec![],
        submodules: vec![],
        prelude: vec![],
        imports: vec![],
        type_vars: vec![],
    };
    let module = ModuleIr {
        name: "alias_types".to_owned(),
        docstring: None,
        is_package: true,
        classes: vec![],
        enums: vec![],
        functions: vec![
            FunctionIr {
                name: "alias_types.make".to_owned(),
                kind: FunctionKindIr::Function,
                params: vec![],
                return_type: "Alias".to_owned(),
                docstring: None,
            },
            FunctionIr {
                name: "alias_types.make_qualified".to_owned(),
                kind: FunctionKindIr::Function,
                params: vec![],
                return_type: "alias_types.Alias".to_owned(),
                docstring: None,
            },
        ],
        properties: vec![],
        submodules: vec![child],
        prelude: vec![],
        imports: vec![ImportIr {
            origin: "alias_types.sub.Widget".to_owned(),
            target: "alias_types.Alias".to_owned(),
        }],
        type_vars: vec![],
    };

    let tokens = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .unwrap()
        .generate()
        .unwrap();
    let generated = prettyplease::unparse(&syn::parse2(tokens).unwrap());

    assert!(
        generated.contains("pub use self::sub::Widget as Alias;"),
        "{generated}"
    );
    assert_eq!(
        generated.matches("::pyo3::PyResult<Alias>").count(),
        2,
        "{generated}"
    );
    assert_generated_crate_checks(
        "pyo3_bindgen_exact_import_alias",
        "num_complex_consumer",
        &generated,
    );
}

#[test]
fn exact_import_alias_uses_sanitized_origin_path() {
    let child = ModuleIr {
        name: "sanitized_alias.sub".to_owned(),
        docstring: None,
        is_package: false,
        classes: vec![pyo3_bindgen_engine::ClassIr {
            name: "sanitized_alias.sub.Self".to_owned(),
            base_classes: vec![],
            docstring: None,
            methods: vec![],
            properties: vec![],
        }],
        enums: vec![],
        functions: vec![],
        properties: vec![],
        submodules: vec![],
        prelude: vec![],
        imports: vec![],
        type_vars: vec![],
    };
    let module = ModuleIr {
        name: "sanitized_alias".to_owned(),
        docstring: None,
        is_package: true,
        classes: vec![],
        enums: vec![],
        functions: vec![FunctionIr {
            name: "sanitized_alias.make".to_owned(),
            kind: FunctionKindIr::Function,
            params: vec![],
            return_type: "Alias".to_owned(),
            docstring: None,
        }],
        properties: vec![],
        submodules: vec![child],
        prelude: vec![],
        imports: vec![ImportIr {
            origin: "sanitized_alias.sub.Self".to_owned(),
            target: "sanitized_alias.Alias".to_owned(),
        }],
        type_vars: vec![],
    };

    let tokens = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .unwrap()
        .generate()
        .unwrap();
    let generated = prettyplease::unparse(&syn::parse2(tokens).unwrap());

    assert!(
        generated.contains("pub use self::sub::s_Self as Alias;"),
        "{generated}"
    );
    assert_generated_crate_checks(
        "pyo3_bindgen_sanitized_exact_import_alias",
        "num_complex_consumer",
        &generated,
    );
}

#[test]
fn non_traversed_import_origin_is_not_reexported_or_mapped() {
    let child = ModuleIr {
        name: "hidden_import.child".to_owned(),
        docstring: None,
        is_package: false,
        classes: vec![pyo3_bindgen_engine::ClassIr {
            name: "hidden_import.child.Widget".to_owned(),
            base_classes: vec![],
            docstring: None,
            methods: vec![],
            properties: vec![],
        }],
        enums: vec![],
        functions: vec![],
        properties: vec![],
        submodules: vec![],
        prelude: vec![],
        imports: vec![],
        type_vars: vec![],
    };
    let module = ModuleIr {
        name: "hidden_import".to_owned(),
        docstring: None,
        is_package: true,
        classes: vec![],
        enums: vec![],
        functions: vec![FunctionIr {
            name: "hidden_import.make".to_owned(),
            kind: FunctionKindIr::Function,
            params: vec![],
            return_type: "Alias".to_owned(),
            docstring: None,
        }],
        properties: vec![],
        submodules: vec![child],
        prelude: vec![],
        imports: vec![ImportIr {
            origin: "hidden_import.child.Widget".to_owned(),
            target: "hidden_import.Alias".to_owned(),
        }],
        type_vars: vec![],
    };

    let tokens = Codegen::new(Config::builder().traverse_submodules(false).build())
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .unwrap()
        .generate()
        .unwrap();
    let generated = prettyplease::unparse(&syn::parse2(tokens).unwrap());

    assert!(!generated.contains("pub use"), "{generated}");
    assert!(!generated.contains("pub mod child"), "{generated}");
    assert_generated_crate_checks(
        "pyo3_bindgen_non_traversed_import",
        "num_complex_consumer",
        &generated,
    );
}

#[test]
fn disabled_class_import_origin_is_not_reexported_or_mapped() {
    let module = ModuleIr {
        name: "disabled_class_import".to_owned(),
        docstring: None,
        is_package: false,
        classes: vec![pyo3_bindgen_engine::ClassIr {
            name: "disabled_class_import.Widget".to_owned(),
            base_classes: vec![],
            docstring: None,
            methods: vec![],
            properties: vec![],
        }],
        enums: vec![],
        functions: vec![FunctionIr {
            name: "disabled_class_import.make".to_owned(),
            kind: FunctionKindIr::Function,
            params: vec![],
            return_type: "Alias".to_owned(),
            docstring: None,
        }],
        properties: vec![],
        submodules: vec![],
        prelude: vec![],
        imports: vec![ImportIr {
            origin: "disabled_class_import.Widget".to_owned(),
            target: "disabled_class_import.Alias".to_owned(),
        }],
        type_vars: vec![],
    };

    let tokens = Codegen::new(Config::builder().generate_classes(false).build())
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .unwrap()
        .generate()
        .unwrap();
    let generated = prettyplease::unparse(&syn::parse2(tokens).unwrap());

    assert!(!generated.contains("pub use"), "{generated}");
    assert!(!generated.contains("pub struct Widget"), "{generated}");
    assert_generated_crate_checks(
        "pyo3_bindgen_disabled_class_import",
        "num_complex_consumer",
        &generated,
    );
}

#[test]
fn disabled_enum_import_origin_is_not_reexported_or_mapped() {
    let module = ModuleIr {
        name: "disabled_enum_import".to_owned(),
        docstring: None,
        is_package: false,
        classes: vec![],
        enums: vec![EnumIr {
            name: "disabled_enum_import.Mode".to_owned(),
            kind: EnumKindIr::Enum,
            variants: vec![EnumVariantIr {
                py_name: "VALUE".to_owned(),
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
            name: "disabled_enum_import.make".to_owned(),
            kind: FunctionKindIr::Function,
            params: vec![],
            return_type: "Alias".to_owned(),
            docstring: None,
        }],
        properties: vec![],
        submodules: vec![],
        prelude: vec![],
        imports: vec![ImportIr {
            origin: "disabled_enum_import.Mode".to_owned(),
            target: "disabled_enum_import.Alias".to_owned(),
        }],
        type_vars: vec![],
    };

    let tokens = Codegen::new(Config::builder().generate_enums(false).build())
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .unwrap()
        .generate()
        .unwrap();
    let generated = prettyplease::unparse(&syn::parse2(tokens).unwrap());

    assert!(!generated.contains("pub use"), "{generated}");
    assert!(!generated.contains("pub enum Mode"), "{generated}");
    assert_generated_crate_checks(
        "pyo3_bindgen_disabled_enum_import",
        "num_complex_consumer",
        &generated,
    );
}

#[test]
fn disabled_enum_does_not_collide_with_enabled_class_import() {
    let child = ModuleIr {
        name: "disabled_enum_collision.child".to_owned(),
        docstring: None,
        is_package: false,
        classes: vec![pyo3_bindgen_engine::ClassIr {
            name: "disabled_enum_collision.child.Widget".to_owned(),
            base_classes: vec![],
            docstring: None,
            methods: vec![],
            properties: vec![],
        }],
        enums: vec![],
        functions: vec![],
        properties: vec![],
        submodules: vec![],
        prelude: vec![],
        imports: vec![],
        type_vars: vec![],
    };
    let module = ModuleIr {
        name: "disabled_enum_collision".to_owned(),
        docstring: None,
        is_package: true,
        classes: vec![],
        enums: vec![EnumIr {
            name: "disabled_enum_collision.Alias".to_owned(),
            kind: EnumKindIr::Enum,
            variants: vec![EnumVariantIr {
                py_name: "VALUE".to_owned(),
                value_repr: "1".to_owned(),
            }],
            aliases: vec![],
            docstring: None,
            is_flag: false,
            is_open: false,
            string_backed: false,
            plain_mirror: false,
        }],
        functions: vec![],
        properties: vec![],
        submodules: vec![child],
        prelude: vec![],
        imports: vec![ImportIr {
            origin: "disabled_enum_collision.child.Widget".to_owned(),
            target: "disabled_enum_collision.Alias".to_owned(),
        }],
        type_vars: vec![],
    };

    let tokens = Codegen::new(Config::builder().generate_enums(false).build())
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .unwrap()
        .generate()
        .unwrap();
    let generated = prettyplease::unparse(&syn::parse2(tokens).unwrap());

    assert!(!generated.contains("pub enum Alias"), "{generated}");
    assert!(
        generated.contains("pub use self::child::Widget as Alias;"),
        "{generated}"
    );
    assert_generated_crate_checks(
        "pyo3_bindgen_disabled_enum_collision",
        "num_complex_consumer",
        &generated,
    );
}

#[test]
fn disabled_import_alias_is_not_mapped_as_a_local_type() {
    let child = ModuleIr {
        name: "disabled_import_alias.child".to_owned(),
        docstring: None,
        is_package: false,
        classes: vec![pyo3_bindgen_engine::ClassIr {
            name: "disabled_import_alias.child.Widget".to_owned(),
            base_classes: vec![],
            docstring: None,
            methods: vec![],
            properties: vec![],
        }],
        enums: vec![],
        functions: vec![],
        properties: vec![],
        submodules: vec![],
        prelude: vec![],
        imports: vec![],
        type_vars: vec![],
    };
    let module = ModuleIr {
        name: "disabled_import_alias".to_owned(),
        docstring: None,
        is_package: true,
        classes: vec![],
        enums: vec![],
        functions: vec![FunctionIr {
            name: "disabled_import_alias.make".to_owned(),
            kind: FunctionKindIr::Function,
            params: vec![],
            return_type: "Alias".to_owned(),
            docstring: None,
        }],
        properties: vec![],
        submodules: vec![child],
        prelude: vec![],
        imports: vec![ImportIr {
            origin: "disabled_import_alias.child.Widget".to_owned(),
            target: "disabled_import_alias.Alias".to_owned(),
        }],
        type_vars: vec![],
    };

    let tokens = Codegen::new(Config::builder().generate_imports(false).build())
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .unwrap()
        .generate()
        .unwrap();
    let generated = prettyplease::unparse(&syn::parse2(tokens).unwrap());

    assert!(!generated.contains("pub use"), "{generated}");
    assert!(generated.contains("pub struct Widget"), "{generated}");
    assert_generated_crate_checks(
        "pyo3_bindgen_disabled_import_alias",
        "num_complex_consumer",
        &generated,
    );
}

#[test]
fn disabled_value_and_type_var_import_origins_are_not_emitted() {
    for (
        case,
        cfg,
        child_functions,
        child_properties,
        child_type_vars,
        parent_functions,
        parent_properties,
    ) in [
        (
            "function",
            Config::builder().generate_functions(false).build(),
            vec![FunctionIr {
                name: "disabled_origins.child.C".to_owned(),
                kind: FunctionKindIr::Function,
                params: vec![],
                return_type: "int".to_owned(),
                docstring: None,
            }],
            vec![],
            vec![],
            vec![],
            vec![PropertyIr {
                name: "disabled_origins.value".to_owned(),
                py_type: "Alias".to_owned(),
                setter_type: None,
                readonly: true,
                docstring: None,
                setter_docstring: None,
            }],
        ),
        (
            "property",
            Config::builder().generate_properties(false).build(),
            vec![],
            vec![PropertyIr {
                name: "disabled_origins.child.C".to_owned(),
                py_type: "int".to_owned(),
                setter_type: None,
                readonly: true,
                docstring: None,
                setter_docstring: None,
            }],
            vec![],
            vec![FunctionIr {
                name: "disabled_origins.make".to_owned(),
                kind: FunctionKindIr::Function,
                params: vec![],
                return_type: "Alias".to_owned(),
                docstring: None,
            }],
            vec![],
        ),
        (
            "type_var",
            Config::builder().generate_type_vars(false).build(),
            vec![],
            vec![],
            vec!["disabled_origins.child.C".to_owned()],
            vec![FunctionIr {
                name: "disabled_origins.make".to_owned(),
                kind: FunctionKindIr::Function,
                params: vec![],
                return_type: "Alias".to_owned(),
                docstring: None,
            }],
            vec![],
        ),
    ] {
        let child = ModuleIr {
            name: "disabled_origins.child".to_owned(),
            docstring: None,
            is_package: false,
            classes: vec![],
            enums: vec![],
            functions: child_functions,
            properties: child_properties,
            submodules: vec![],
            prelude: vec![],
            imports: vec![],
            type_vars: child_type_vars,
        };
        let module = ModuleIr {
            name: "disabled_origins".to_owned(),
            docstring: None,
            is_package: true,
            classes: vec![],
            enums: vec![],
            functions: parent_functions,
            properties: parent_properties,
            submodules: vec![child],
            prelude: vec![],
            imports: vec![ImportIr {
                origin: "disabled_origins.child.C".to_owned(),
                target: "disabled_origins.Alias".to_owned(),
            }],
            type_vars: vec![],
        };

        let tokens = Codegen::new(cfg)
            .modules_from_ir(SchemaIr {
                modules: vec![module],
            })
            .unwrap()
            .generate()
            .unwrap();
        let generated = prettyplease::unparse(&syn::parse2(tokens).unwrap());

        assert!(!generated.contains("pub use"), "case {case}: {generated}");
        assert_generated_crate_checks(
            &format!("pyo3_bindgen_disabled_{case}_origin"),
            "num_complex_consumer",
            &generated,
        );
    }
}

#[test]
fn disabled_class_members_do_not_affect_enabled_name_plans() {
    let class = pyo3_bindgen_engine::ClassIr {
        name: "member_flags.Widget".to_owned(),
        base_classes: vec![],
        docstring: None,
        methods: vec![FunctionIr {
            name: "member_flags.Widget.not-valid".to_owned(),
            kind: FunctionKindIr::Method {
                method_type: MethodKindIr::Static,
            },
            params: vec![],
            return_type: "int".to_owned(),
            docstring: None,
        }],
        properties: vec![PropertyIr {
            name: "f_not_valid".to_owned(),
            py_type: "int".to_owned(),
            setter_type: None,
            readonly: true,
            docstring: None,
            setter_docstring: None,
        }],
    };
    let module = ModuleIr {
        name: "member_flags".to_owned(),
        docstring: None,
        is_package: false,
        classes: vec![class.clone()],
        enums: vec![],
        functions: vec![],
        properties: vec![],
        submodules: vec![],
        prelude: vec![],
        imports: vec![],
        type_vars: vec![],
    };

    let methods_disabled = Codegen::new(Config::builder().generate_functions(false).build())
        .modules_from_ir(SchemaIr {
            modules: vec![module.clone()],
        })
        .unwrap()
        .generate()
        .unwrap();
    let methods_disabled = prettyplease::unparse(&syn::parse2(methods_disabled).unwrap());
    assert!(
        methods_disabled.contains("fn f_not_valid<'py>"),
        "{methods_disabled}"
    );
    assert!(
        !methods_disabled.contains("fn get_f_not_valid<'py>"),
        "{methods_disabled}"
    );
    assert!(
        !methods_disabled.contains("not-valid"),
        "{methods_disabled}"
    );
    assert_generated_crate_checks(
        "pyo3_bindgen_disabled_class_methods",
        "num_complex_consumer",
        &methods_disabled,
    );

    let properties_disabled = Codegen::new(Config::builder().generate_properties(false).build())
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .unwrap()
        .generate()
        .unwrap();
    let properties_disabled = prettyplease::unparse(&syn::parse2(properties_disabled).unwrap());
    assert!(
        properties_disabled.contains("fn f_not_valid<'py>"),
        "{properties_disabled}"
    );
    assert!(
        !properties_disabled.contains("get_f_not_valid"),
        "{properties_disabled}"
    );
    assert_generated_crate_checks(
        "pyo3_bindgen_disabled_class_properties",
        "num_complex_consumer",
        &properties_disabled,
    );
}

#[test]
fn disabled_module_function_does_not_reserve_property_name() {
    let module = ModuleIr {
        name: "module_member_flags".to_owned(),
        docstring: None,
        is_package: false,
        classes: vec![],
        enums: vec![],
        functions: vec![FunctionIr {
            name: "module_member_flags.not-valid".to_owned(),
            kind: FunctionKindIr::Function,
            params: vec![],
            return_type: "int".to_owned(),
            docstring: None,
        }],
        properties: vec![PropertyIr {
            name: "module_member_flags.f_not_valid".to_owned(),
            py_type: "int".to_owned(),
            setter_type: None,
            readonly: true,
            docstring: None,
            setter_docstring: None,
        }],
        submodules: vec![],
        prelude: vec![],
        imports: vec![],
        type_vars: vec![],
    };

    let tokens = Codegen::new(Config::builder().generate_functions(false).build())
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .unwrap()
        .generate()
        .unwrap();
    let generated = prettyplease::unparse(&syn::parse2(tokens).unwrap());

    assert!(generated.contains("fn f_not_valid<'py>"), "{generated}");
    assert!(
        !generated.contains("fn get_f_not_valid<'py>"),
        "{generated}"
    );
    assert!(!generated.contains("not-valid"), "{generated}");
    assert_generated_crate_checks(
        "pyo3_bindgen_disabled_module_function",
        "num_complex_consumer",
        &generated,
    );
}

#[test]
fn property_import_uses_name_plan_without_disabled_functions() {
    let child = ModuleIr {
        name: "property_import.child".to_owned(),
        docstring: None,
        is_package: false,
        classes: vec![],
        enums: vec![],
        functions: vec![FunctionIr {
            name: "property_import.child.not-valid".to_owned(),
            kind: FunctionKindIr::Function,
            params: vec![],
            return_type: "int".to_owned(),
            docstring: None,
        }],
        properties: vec![PropertyIr {
            name: "property_import.child.f_not_valid".to_owned(),
            py_type: "int".to_owned(),
            setter_type: None,
            readonly: true,
            docstring: None,
            setter_docstring: None,
        }],
        submodules: vec![],
        prelude: vec![],
        imports: vec![],
        type_vars: vec![],
    };
    let module = ModuleIr {
        name: "property_import".to_owned(),
        docstring: None,
        is_package: true,
        classes: vec![],
        enums: vec![],
        functions: vec![],
        properties: vec![],
        submodules: vec![child],
        prelude: vec![],
        imports: vec![ImportIr {
            origin: "property_import.child.f_not_valid".to_owned(),
            target: "property_import.AliasProperty".to_owned(),
        }],
        type_vars: vec![],
    };

    let tokens = Codegen::new(Config::builder().generate_functions(false).build())
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .unwrap()
        .generate()
        .unwrap();
    let generated = prettyplease::unparse(&syn::parse2(tokens).unwrap());

    assert!(
        generated.contains("pub use self::child::f_not_valid as AliasProperty;"),
        "{generated}"
    );
    assert!(generated.contains("fn f_not_valid<'py>"), "{generated}");
    assert!(!generated.contains("get_f_not_valid"), "{generated}");
    assert!(!generated.contains("not-valid"), "{generated}");
    assert_generated_crate_checks(
        "pyo3_bindgen_property_import_disabled_function",
        "num_complex_consumer",
        &generated,
    );
}

#[test]
fn prelude_name_reservations_only_include_emitted_items() {
    let disabled_cases = [
        (
            "function",
            Config::builder().generate_functions(false).build(),
        ),
        (
            "property",
            Config::builder().generate_properties(false).build(),
        ),
        ("class", Config::builder().generate_classes(false).build()),
        ("enum", Config::builder().generate_enums(false).build()),
        (
            "submodule",
            Config::builder().traverse_submodules(false).build(),
        ),
        ("import", Config::builder().generate_imports(false).build()),
        (
            "type_var",
            Config::builder().generate_type_vars(false).build(),
        ),
    ];

    for (case, cfg) in disabled_cases {
        let mut module = ModuleIr {
            name: "prelude_flags".to_owned(),
            docstring: None,
            is_package: true,
            classes: vec![],
            enums: vec![],
            functions: vec![],
            properties: vec![],
            submodules: vec![],
            prelude: vec!["Exported".to_owned()],
            imports: vec![],
            type_vars: vec!["prelude_flags.Exported".to_owned()],
        };
        match case {
            "function" => module.functions.push(FunctionIr {
                name: "prelude_flags.prelude".to_owned(),
                kind: FunctionKindIr::Function,
                params: vec![],
                return_type: "int".to_owned(),
                docstring: None,
            }),
            "property" => module.properties.push(PropertyIr {
                name: "prelude_flags.prelude".to_owned(),
                py_type: "int".to_owned(),
                setter_type: None,
                readonly: true,
                docstring: None,
                setter_docstring: None,
            }),
            "class" => module.classes.push(pyo3_bindgen_engine::ClassIr {
                name: "prelude_flags.prelude".to_owned(),
                base_classes: vec![],
                docstring: None,
                methods: vec![],
                properties: vec![],
            }),
            "enum" => module.enums.push(EnumIr {
                name: "prelude_flags.prelude".to_owned(),
                kind: EnumKindIr::Enum,
                variants: vec![EnumVariantIr {
                    py_name: "VALUE".to_owned(),
                    value_repr: "1".to_owned(),
                }],
                aliases: vec![],
                docstring: None,
                is_flag: false,
                is_open: false,
                string_backed: false,
                plain_mirror: false,
            }),
            "submodule" => module.submodules.push(ModuleIr {
                name: "prelude_flags.prelude".to_owned(),
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
            }),
            "import" => {
                module.submodules.push(ModuleIr {
                    name: "prelude_flags.child".to_owned(),
                    docstring: None,
                    is_package: false,
                    classes: vec![pyo3_bindgen_engine::ClassIr {
                        name: "prelude_flags.child.C".to_owned(),
                        base_classes: vec![],
                        docstring: None,
                        methods: vec![],
                        properties: vec![],
                    }],
                    enums: vec![],
                    functions: vec![],
                    properties: vec![],
                    submodules: vec![],
                    prelude: vec![],
                    imports: vec![],
                    type_vars: vec![],
                });
                module.imports.push(ImportIr {
                    origin: "prelude_flags.child.C".to_owned(),
                    target: "prelude_flags.prelude".to_owned(),
                });
            }
            "type_var" => {
                module.type_vars = vec!["prelude_flags.prelude".to_owned()];
                module.functions.push(FunctionIr {
                    name: "prelude_flags.Exported".to_owned(),
                    kind: FunctionKindIr::Function,
                    params: vec![],
                    return_type: "int".to_owned(),
                    docstring: None,
                });
            }
            _ => unreachable!(),
        }

        let tokens = Codegen::new(cfg)
            .modules_from_ir(SchemaIr {
                modules: vec![module],
            })
            .unwrap()
            .generate()
            .unwrap();
        let generated = prettyplease::unparse(&syn::parse2(tokens).unwrap());
        assert!(
            generated.contains("pub mod prelude {"),
            "case {case}: {generated}"
        );
        assert!(
            !generated.contains("pub mod prelude1"),
            "case {case}: {generated}"
        );
    }

    let child = ModuleIr {
        name: "prelude_flags.child".to_owned(),
        docstring: None,
        is_package: false,
        classes: vec![pyo3_bindgen_engine::ClassIr {
            name: "prelude_flags.child.C".to_owned(),
            base_classes: vec![],
            docstring: None,
            methods: vec![],
            properties: vec![],
        }],
        enums: vec![],
        functions: vec![],
        properties: vec![],
        submodules: vec![],
        prelude: vec![],
        imports: vec![],
        type_vars: vec![],
    };
    let module = ModuleIr {
        name: "prelude_flags".to_owned(),
        docstring: None,
        is_package: true,
        classes: vec![],
        enums: vec![],
        functions: vec![],
        properties: vec![],
        submodules: vec![child],
        prelude: vec!["Exported".to_owned()],
        imports: vec![ImportIr {
            origin: "prelude_flags.child.C".to_owned(),
            target: "prelude_flags.prelude".to_owned(),
        }],
        type_vars: vec!["prelude_flags.Exported".to_owned()],
    };
    let tokens = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .unwrap()
        .generate()
        .unwrap();
    let generated = prettyplease::unparse(&syn::parse2(tokens).unwrap());
    assert!(generated.contains("pub mod prelude1 {"), "{generated}");
}

#[test]
fn callable_imports_use_their_final_allocated_names() {
    let child = ModuleIr {
        name: "callable_imports.child".to_owned(),
        docstring: None,
        is_package: false,
        classes: vec![],
        enums: vec![],
        functions: vec![FunctionIr {
            name: "callable_imports.child.PYO3_BINDGEN_MODULE_NAME".to_owned(),
            kind: FunctionKindIr::Function,
            params: vec![],
            return_type: "int".to_owned(),
            docstring: None,
        }],
        properties: vec![PropertyIr {
            name: "callable_imports.child.PYO3_BINDGEN_INPUT_HASH".to_owned(),
            py_type: "int".to_owned(),
            setter_type: None,
            readonly: true,
            docstring: None,
            setter_docstring: None,
        }],
        submodules: vec![],
        prelude: vec![],
        imports: vec![],
        type_vars: vec![],
    };
    let module = ModuleIr {
        name: "callable_imports".to_owned(),
        docstring: None,
        is_package: true,
        classes: vec![],
        enums: vec![],
        functions: vec![],
        properties: vec![],
        submodules: vec![child],
        prelude: vec![],
        imports: vec![
            ImportIr {
                origin: "callable_imports.child.PYO3_BINDGEN_MODULE_NAME".to_owned(),
                target: "callable_imports.Self".to_owned(),
            },
            ImportIr {
                origin: "callable_imports.child.PYO3_BINDGEN_INPUT_HASH".to_owned(),
                target: "callable_imports.AliasProperty".to_owned(),
            },
        ],
        type_vars: vec![],
    };

    let tokens = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .unwrap()
        .generate()
        .unwrap();
    let generated = prettyplease::unparse(&syn::parse2(tokens).unwrap());

    assert!(
        generated.contains("pub use self::child::py_PYO3_BINDGEN_MODULE_NAME as f_Self;"),
        "{generated}"
    );
    assert!(
        generated.contains("pub use self::child::get_PYO3_BINDGEN_INPUT_HASH as AliasProperty;"),
        "{generated}"
    );
    assert_generated_crate_checks(
        "pyo3_bindgen_callable_imports",
        "num_complex_consumer",
        &generated,
    );
}

#[test]
fn same_name_callable_imports_reuse_target_module_allocations() {
    let child = ModuleIr {
        name: "same_callable_imports.child".to_owned(),
        docstring: None,
        is_package: false,
        classes: vec![],
        enums: vec![],
        functions: vec![FunctionIr {
            name: "same_callable_imports.child.PYO3_BINDGEN_MODULE_NAME".to_owned(),
            kind: FunctionKindIr::Function,
            params: vec![],
            return_type: "int".to_owned(),
            docstring: None,
        }],
        properties: vec![PropertyIr {
            name: "same_callable_imports.child.PYO3_BINDGEN_INPUT_HASH".to_owned(),
            py_type: "int".to_owned(),
            setter_type: None,
            readonly: true,
            docstring: None,
            setter_docstring: None,
        }],
        submodules: vec![],
        prelude: vec![],
        imports: vec![],
        type_vars: vec![],
    };
    let module = ModuleIr {
        name: "same_callable_imports".to_owned(),
        docstring: None,
        is_package: true,
        classes: vec![],
        enums: vec![],
        functions: vec![],
        properties: vec![],
        submodules: vec![child],
        prelude: vec![],
        imports: vec![
            ImportIr {
                origin: "same_callable_imports.child.PYO3_BINDGEN_MODULE_NAME".to_owned(),
                target: "same_callable_imports.PYO3_BINDGEN_MODULE_NAME".to_owned(),
            },
            ImportIr {
                origin: "same_callable_imports.child.PYO3_BINDGEN_INPUT_HASH".to_owned(),
                target: "same_callable_imports.PYO3_BINDGEN_INPUT_HASH".to_owned(),
            },
        ],
        type_vars: vec![],
    };

    let tokens = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .unwrap()
        .generate()
        .unwrap();
    let generated = prettyplease::unparse(&syn::parse2(tokens).unwrap());

    assert!(
        generated.contains("pub use self::child::py_PYO3_BINDGEN_MODULE_NAME;"),
        "{generated}"
    );
    assert!(
        generated.contains(
            "pub use self::child::get_PYO3_BINDGEN_INPUT_HASH as py_PYO3_BINDGEN_INPUT_HASH;"
        ),
        "{generated}"
    );
    assert_generated_crate_checks(
        "pyo3_bindgen_same_callable_imports",
        "num_complex_consumer",
        &generated,
    );
}

#[test]
fn module_import_alias_preserves_sanitized_descendant_paths() {
    let child = ModuleIr {
        name: "alias_descendants.sub".to_owned(),
        docstring: None,
        is_package: false,
        classes: vec![pyo3_bindgen_engine::ClassIr {
            name: "alias_descendants.sub.Self".to_owned(),
            base_classes: vec![],
            docstring: None,
            methods: vec![],
            properties: vec![],
        }],
        enums: vec![],
        functions: vec![],
        properties: vec![],
        submodules: vec![],
        prelude: vec![],
        imports: vec![],
        type_vars: vec![],
    };
    let module = ModuleIr {
        name: "alias_descendants".to_owned(),
        docstring: None,
        is_package: true,
        classes: vec![],
        enums: vec![],
        functions: vec![FunctionIr {
            name: "alias_descendants.make".to_owned(),
            kind: FunctionKindIr::Function,
            params: vec![],
            return_type: "renamed.Self".to_owned(),
            docstring: None,
        }],
        properties: vec![],
        submodules: vec![child],
        prelude: vec![],
        imports: vec![ImportIr {
            origin: "alias_descendants.sub".to_owned(),
            target: "alias_descendants.renamed".to_owned(),
        }],
        type_vars: vec![],
    };

    let tokens = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .unwrap()
        .generate()
        .unwrap();
    let generated = prettyplease::unparse(&syn::parse2(tokens).unwrap());

    assert!(
        generated.contains("pub use self::sub as renamed;"),
        "{generated}"
    );
    assert!(generated.contains("renamed::s_Self"), "{generated}");
    assert_generated_crate_checks(
        "pyo3_bindgen_module_import_alias",
        "num_complex_consumer",
        &generated,
    );
}

#[test]
fn sanitized_local_type_references_compile_for_consumers() {
    let echo = |name: &str, annotation: &str| FunctionIr {
        name: format!("sanitized_types.{name}"),
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
    let module = ModuleIr {
        name: "sanitized_types".to_owned(),
        docstring: None,
        is_package: false,
        classes: vec![pyo3_bindgen_engine::ClassIr {
            name: "sanitized_types.Self".to_owned(),
            base_classes: vec![],
            docstring: None,
            methods: vec![],
            properties: vec![],
        }],
        enums: vec![EnumIr {
            name: "sanitized_types.super".to_owned(),
            kind: EnumKindIr::Enum,
            variants: vec![EnumVariantIr {
                py_name: "VALUE".to_owned(),
                value_repr: "1".to_owned(),
            }],
            aliases: vec![],
            docstring: None,
            is_flag: false,
            is_open: false,
            string_backed: false,
            plain_mirror: false,
        }],
        functions: vec![
            echo("echo_class", "sanitized_types.Self"),
            echo("echo_enum", "sanitized_types.super"),
            echo("echo_type_var", "sanitized_types.crate"),
        ],
        properties: vec![],
        submodules: vec![],
        prelude: vec![],
        imports: vec![],
        type_vars: vec!["sanitized_types.crate".to_owned()],
    };

    let tokens = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .unwrap()
        .generate()
        .expect("sanitized local type references must lower without panicking");
    let generated = prettyplease::unparse(&syn::parse2(tokens).unwrap());

    assert!(generated.contains("pub struct s_Self"), "{generated}");
    assert!(generated.contains("pub enum e_super"), "{generated}");
    assert!(generated.contains("pub type t_crate"), "{generated}");
    assert_generated_crate_checks(
        "pyo3_bindgen_sanitized_local_types",
        "num_complex_consumer",
        &generated,
    );
}

#[test]
fn disabled_class_references_fall_back_dynamic_for_consumers() {
    let module = ModuleIr {
        name: "PyObjectBase".to_owned(),
        docstring: None,
        is_package: false,
        classes: vec![pyo3_bindgen_engine::ClassIr {
            name: "PyObjectBase.Widget".to_owned(),
            base_classes: vec![],
            docstring: None,
            methods: vec![],
            properties: vec![],
        }],
        enums: vec![],
        functions: vec![FunctionIr {
            name: "PyObjectBase.echo".to_owned(),
            kind: FunctionKindIr::Function,
            params: vec![ParameterIr {
                name: "value".to_owned(),
                kind: ParameterKindIr::PositionalOrKeyword,
                annotation: "PyObjectBase.Widget".to_owned(),
                has_default: false,
            }],
            return_type: "PyObjectBase.Widget".to_owned(),
            docstring: None,
        }],
        properties: vec![],
        submodules: vec![],
        prelude: vec![],
        imports: vec![],
        type_vars: vec![],
    };
    let tokens = Codegen::new(Config::builder().generate_classes(false).build())
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .unwrap()
        .generate()
        .unwrap();
    let generated = prettyplease::unparse(&syn::parse2(tokens).unwrap());

    assert!(!generated.contains("pub struct Widget"), "{generated}");
    assert!(!generated.contains("trait PyObjectBase"), "{generated}");
    assert_generated_crate_checks(
        "pyo3_bindgen_disabled_class_reference",
        "num_complex_consumer",
        &generated,
    );
}

#[test]
fn disabled_enum_references_fall_back_dynamic_for_consumers() {
    let module = ModuleIr {
        name: "disabled_enum".to_owned(),
        docstring: None,
        is_package: false,
        classes: vec![],
        enums: vec![EnumIr {
            name: "disabled_enum.Mode".to_owned(),
            kind: EnumKindIr::Enum,
            variants: vec![EnumVariantIr {
                py_name: "VALUE".to_owned(),
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
            name: "disabled_enum.echo".to_owned(),
            kind: FunctionKindIr::Function,
            params: vec![ParameterIr {
                name: "value".to_owned(),
                kind: ParameterKindIr::PositionalOrKeyword,
                annotation: "disabled_enum.Mode".to_owned(),
                has_default: false,
            }],
            return_type: "disabled_enum.Mode".to_owned(),
            docstring: None,
        }],
        properties: vec![],
        submodules: vec![],
        prelude: vec![],
        imports: vec![],
        type_vars: vec![],
    };
    let tokens = Codegen::new(Config::builder().generate_enums(false).build())
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .unwrap()
        .generate()
        .unwrap();
    let generated = prettyplease::unparse(&syn::parse2(tokens).unwrap());

    assert!(!generated.contains("pub enum Mode"), "{generated}");
    assert_generated_crate_checks(
        "pyo3_bindgen_disabled_enum_reference",
        "num_complex_consumer",
        &generated,
    );
}

#[test]
fn non_traversed_type_references_fall_back_dynamic_for_consumers() {
    let child = ModuleIr {
        name: "not_traversed.child".to_owned(),
        docstring: None,
        is_package: false,
        classes: vec![pyo3_bindgen_engine::ClassIr {
            name: "not_traversed.child.Hidden".to_owned(),
            base_classes: vec![],
            docstring: None,
            methods: vec![],
            properties: vec![],
        }],
        enums: vec![],
        functions: vec![],
        properties: vec![],
        submodules: vec![],
        prelude: vec![],
        imports: vec![],
        type_vars: vec![],
    };
    let module = ModuleIr {
        name: "not_traversed".to_owned(),
        docstring: None,
        is_package: true,
        classes: vec![],
        enums: vec![],
        functions: vec![FunctionIr {
            name: "not_traversed.echo".to_owned(),
            kind: FunctionKindIr::Function,
            params: vec![ParameterIr {
                name: "value".to_owned(),
                kind: ParameterKindIr::PositionalOrKeyword,
                annotation: "not_traversed.child.Hidden".to_owned(),
                has_default: false,
            }],
            return_type: "not_traversed.child.Hidden".to_owned(),
            docstring: None,
        }],
        properties: vec![],
        submodules: vec![child],
        prelude: vec![],
        imports: vec![],
        type_vars: vec![],
    };
    let tokens = Codegen::new(Config::builder().traverse_submodules(false).build())
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .unwrap()
        .generate()
        .unwrap();
    let generated = prettyplease::unparse(&syn::parse2(tokens).unwrap());

    assert!(!generated.contains("pub mod child"), "{generated}");
    assert!(!generated.contains("trait PyObjectBase"), "{generated}");
    assert_generated_crate_checks(
        "pyo3_bindgen_non_traversed_reference",
        "num_complex_consumer",
        &generated,
    );
}

fn assert_generated_crate_checks(name: &str, fixture: &str, generated: &str) {
    bounded_consumer::assert_consumer_compiles(
        generated,
        name,
        bounded_consumer::ConsumerManifest::Fixture(fixture),
        &["--quiet", "--locked"],
    );
}

#[cfg(feature = "num-complex")]
#[test]
fn generated_num_complex_binding_compiles_for_consumers() {
    let generated = generated_function("complex");
    assert!(generated.contains("::num_complex::Complex64"));
    assert_generated_crate_checks(
        "pyo3_bindgen_num_complex_fixture",
        "num_complex_consumer",
        &generated,
    );
}

#[cfg(feature = "numpy")]
#[test]
fn generated_numpy_binding_compiles_for_consumers() {
    let generated = generated_function("numpy.ndarray");
    assert!(generated.contains("::numpy::PyUntypedArray"));
    assert_generated_crate_checks("pyo3_bindgen_numpy_fixture", "numpy_consumer", &generated);
}

#[cfg(feature = "numpy")]
#[test]
fn local_ndarray_class_wins_over_numpy_adapter() {
    let module = ModuleIr {
        name: "pkg".to_owned(),
        docstring: None,
        is_package: false,
        classes: vec![ClassIr {
            name: "pkg.ndarray".to_owned(),
            base_classes: vec![],
            docstring: None,
            methods: vec![],
            properties: vec![],
        }],
        enums: vec![],
        functions: vec![FunctionIr {
            name: "pkg.echo".to_owned(),
            kind: FunctionKindIr::Function,
            params: vec![ParameterIr {
                name: "value".to_owned(),
                kind: ParameterKindIr::PositionalOrKeyword,
                annotation: "pkg.ndarray".to_owned(),
                has_default: false,
            }],
            return_type: "pkg.ndarray".to_owned(),
            docstring: None,
        }],
        properties: vec![],
        submodules: vec![],
        prelude: vec![],
        imports: vec![],
        type_vars: vec![],
    };

    let tokens = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .unwrap()
        .generate()
        .unwrap()
        .to_string();

    assert!(tokens.contains("p_value : & ndarray"), "{tokens}");
    assert!(!tokens.contains("numpy :: PyUntypedArray"), "{tokens}");
}
