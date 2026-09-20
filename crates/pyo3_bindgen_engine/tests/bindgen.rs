#[path = "../test_support/bounded_consumer.rs"]
mod bounded_consumer;

fn generate_bindings(code_py: &str, module_name: &str) -> String {
    generate_bindings_with_config(code_py, module_name, pyo3_bindgen_engine::Config::default())
}

fn generate_bindings_with_config(
    code_py: &str,
    module_name: &str,
    mut cfg: pyo3_bindgen_engine::Config,
) -> String {
    cfg.generate_provenance = false;
    let bindings = pyo3_bindgen_engine::Codegen::new(cfg)
        .module_from_str(code_py, module_name)
        .unwrap()
        .generate()
        .unwrap();

    prettyplease::unparse(&syn::parse_str(&bindings.to_string()).unwrap())
}

fn assert_bindgen_eq(code_py: &str, module_name: &str, code_rs: &str) {
    let generated_code = generate_bindings(code_py, module_name);
    let target_code = prettyplease::unparse(&syn::parse_str(code_rs).unwrap());
    assert_eq!(
        generated_code, target_code,
        "\nGenerated:\n\n{generated_code}"
    );
}

#[test]
fn live_static_method_keeps_parameter_named_self() {
    let generated = generate_bindings(
        indoc::indoc! { r#"
            class Widget:
                @staticmethod
                def keep(self: int) -> int:
                    return self
        "# },
        "mod_static_self",
    );

    assert!(
        generated.contains("pub fn keep<'py>(\n            py:"),
        "descriptor-proven static method must remain associated: {generated}"
    );
    assert!(
        generated.contains("p_self: i64"),
        "static method must keep its legal 'self' argument: {generated}"
    );
    assert!(
        !generated.contains("fn keep<'py>(\n            &self,"),
        "static method must not become an instance method: {generated}"
    );
}

#[test]
fn live_inferred_method_keeps_non_positional_self_parameters() {
    let generated = generate_bindings(
        indoc::indoc! { r#"
            class Widget:
                def collect_star(*self):
                    return self

                def collect_kwargs(**self):
                    return self
        "# },
        "mod_live_variadic_self",
    );

    for method in ["collect_star", "collect_kwargs"] {
        let signature = generated
            .split(&format!("fn {method}"))
            .nth(1)
            .and_then(|rest| rest.split(") ->").next())
            .unwrap_or_else(|| panic!("missing generated {method} signature: {generated}"));
        assert!(
            signature.contains("p_self:"),
            "live non-positional self must remain caller-visible: {signature}"
        );
        assert!(
            !signature.contains("&self"),
            "a non-positional first parameter must not become a stripped receiver: {signature}"
        );
    }
}

#[test]
fn builtin_classmethod_descriptor_keeps_bound_signature() {
    let generated = pyo3_bindgen_engine::Codegen::default()
        .module_from_str(
            "class DictSubclass(dict):\n    pass\n",
            "mod_builtin_classmethod_descriptor",
        )
        .unwrap()
        .generate_with_report()
        .unwrap();
    let diagnostics = generated.report.diagnostics;
    let generated = prettyplease::unparse(&syn::parse2(generated.tokens).unwrap());
    assert!(
        generated.contains("fromkeys"),
        "generated subclass bindings omitted inherited `dict.fromkeys`: {generated}"
    );
    let fromkeys = generated
        .split("pub fn fromkeys")
        .nth(1)
        .and_then(|rest| rest.split("pub fn ").next())
        .unwrap_or_else(|| {
            panic!("generated `dict.fromkeys` binding should exist; diagnostics: {diagnostics:?}")
        });
    let signature = fromkeys
        .split(") ->")
        .next()
        .expect("generated `dict.fromkeys` signature should terminate");

    assert!(
        signature.contains("py: ::pyo3::marker::Python<'py>"),
        "built-in classmethod must be an associated function: {signature}"
    );
    assert!(
        signature.contains("p_iterable:") && signature.contains("p_value:"),
        "bound classmethod signature must retain every explicit parameter: {signature}"
    );
    assert!(
        !signature.contains("&self"),
        "built-in classmethod must not become an instance method: {signature}"
    );
}

#[test]
fn live_builtin_subclass_annotations_keep_custom_identity() {
    let generated = generate_bindings(
        indoc::indoc! { r#"
            class UserId(int):
                pass

            class UserList(list):
                pass

            def echo_id(value: UserId) -> UserId:
                return value

            def echo_list(value: UserList) -> UserList:
                return value
        "# },
        "mod_custom_builtin_subclasses",
    );

    assert!(
        generated.contains("pub struct UserId {") && generated.contains("pub struct UserList {"),
        "custom builtin subclasses must emit their own wrappers: {generated}"
    );
    assert!(
        generated.contains("p_value: &UserId") && generated.contains("p_value: &UserList"),
        "custom builtin subclass annotations must not flatten to primitive/container Rust types: {generated}"
    );
}

#[cfg(not(Py_LIMITED_API))]
#[test]
fn live_datetime_annotation_is_not_flattened_to_date() {
    let generated = generate_bindings(
        indoc::indoc! { r#"
            import datetime

            def identity(value: datetime.datetime) -> datetime.datetime:
                return value
        "# },
        "mod_datetime_annotation",
    );

    assert!(
        generated.contains("::pyo3::types::PyDateTime"),
        "datetime.datetime must lower to PyDateTime: {generated}"
    );
    assert!(
        !generated.contains("::pyo3::types::PyDate>"),
        "datetime.datetime must not lower to PyDate: {generated}"
    );
}

#[test]
fn live_c_descriptors_remain_methods_and_plain_classes_keep_constructor() {
    let generated = generate_bindings(
        indoc::indoc! { r#"
            class Plain:
                pass

            class ListSubclass(list):
                pass
        "# },
        "mod_c_descriptors",
    );

    let list_methods = generated
        .split("pub trait ListSubclassMethods")
        .nth(1)
        .expect("ListSubclassMethods must be emitted");
    assert!(
        list_methods.contains("fn append<'py>(") && list_methods.contains("p_object:"),
        "inherited C method descriptors must retain callable parameters: {generated}"
    );

    assert!(
        generated.contains("#[automatically_derived]\n    impl Plain {")
            && generated
                .split("#[automatically_derived]\n    impl Plain {")
                .skip(1)
                .any(|plain_impl| plain_impl.contains("pub fn new<'py>(")),
        "object.__init__ wrapper descriptor must seed a constructor for plain classes: {generated}"
    );
}

#[test]
fn live_slots_member_descriptor_emits_setter() {
    let generated = generate_bindings(
        indoc::indoc! { r#"
            class Slotted:
                __slots__ = ("value",)
        "# },
        "mod_writable_slot",
    );

    assert!(
        generated.contains("fn set_value<'py>("),
        "writable __slots__ member descriptor must emit setter: {generated}"
    );
}

#[test]
fn live_literal_raw_identifier_keeps_python_lookup_name() {
    let generated = generate_bindings("globals()['r#type'] = 7\n", "mod_literal_raw_identifier");

    assert!(
        generated.contains("::pyo3::intern!(py, \"r#type\")"),
        "literal Python name r#type must not be looked up as type: {generated}"
    );
}

#[test]
fn live_zero_receiver_fallback_keeps_owning_class_path() {
    let generated = generate_bindings(
        indoc::indoc! { r#"
            import inspect

            class Zero:
                def method(self):
                    return 1

            Zero.method.__signature__ = inspect.Signature([])
        "# },
        "mod_zero_receiver",
    );

    assert!(
        generated.contains("::pyo3::intern!(py, \"mod_zero_receiver\")"),
        "fallback dispatcher must import owning module: {generated}"
    );
    assert!(
        !generated.contains("::pyo3::intern!(py, \"mod_zero_receiver.Zero.method\")")
            && !generated.contains("::pyo3::intern!(py, \"mod_zero_receiver.Zero\")"),
        "fallback must not treat full method path as class import path: {generated}"
    );
}

#[test]
fn live_non_runtime_protocol_falls_back_to_untyped_alias() {
    let generated = generate_bindings(
        indoc::indoc! { r#"
            from typing import Protocol

            class SupportsRun(Protocol):
                def run(self) -> int: ...

            def consume(value: SupportsRun) -> int:
                return value.run()
        "# },
        "mod_protocol",
    );

    assert!(
        generated.contains("pub type SupportsRun = ::pyo3::types::PyAny;"),
        "non-runtime protocol must emit safe untyped alias: {generated}"
    );
    assert!(
        !generated.contains("pub struct SupportsRun"),
        "non-runtime protocol must not emit nominal wrapper with isinstance guard: {generated}"
    );
}

#[test]
fn live_quoted_and_forward_ref_annotations_resolve_local_class() {
    let generated = generate_bindings(
        indoc::indoc! { r#"
            from __future__ import annotations
            import typing
            from typing import ForwardRef, Optional

            class User:
                pass

            def quoted(value: 'User') -> 'User':
                return value

            def forwarded(value):
                return value

            forwarded.__annotations__ = {
                "value": ForwardRef("User"),
                "return": ForwardRef("User"),
            }

            def textual(value: ForwardRef("User")) -> ForwardRef("User"):
                return value

            def nested(value: Optional[ForwardRef("User")]) -> Optional[ForwardRef("User")]:
                return value

            if hasattr(typing, "Annotated"):
                Annotated = typing.Annotated

                def annotated(value: Annotated[User, "metadata"]) -> Annotated[User, "metadata"]:
                    return value
            else:
                # `typing.Annotated` was added in CPython 3.9. Keep the workspace's
                # CPython 3.8 compatibility lane meaningful without requiring the
                # optional `typing_extensions` package.
                def annotated(value: User) -> User:
                    return value
        "# },
        "mod_forward_refs",
    );

    for function in ["quoted", "forwarded", "textual", "annotated"] {
        let signature = generated
            .split(&format!("pub fn {function}<'py>("))
            .nth(1)
            .and_then(|rest| rest.split('{').next())
            .unwrap_or_else(|| panic!("missing {function} signature: {generated}"));
        assert!(
            signature.contains("p_value: &User") && signature.contains("PyResult<User>"),
            "{function} forward reference must resolve to local User wrapper: {signature}"
        );
    }

    let nested = generated
        .split("pub fn nested<'py>(")
        .nth(1)
        .and_then(|rest| rest.split('{').next())
        .unwrap_or_else(|| panic!("missing nested signature: {generated}"));
    assert!(
        nested.contains("p_value: ::std::option::Option<User>")
            && nested.contains("PyResult<::std::option::Option<User>>"),
        "nested textual ForwardRef must resolve inside Optional: {nested}"
    );
}

#[test]
fn postponed_annotations_follow_wrapped_function_globals() {
    let generated = generate_bindings(
        indoc::indoc! { r#"
            from __future__ import annotations
            import functools

            _decorator_globals = {
                "__name__": "foreign_decorators",
                "functools": functools,
            }
            exec(
                """
            def decorate(function):
                @functools.wraps(function)
                def wrapper(*args, **kwargs):
                    return function(*args, **kwargs)
                return wrapper
            """,
                _decorator_globals,
            )
            _decorate = _decorator_globals["decorate"]

            class int:
                pass

            @_decorate
            def echo(value: int) -> int:
                return value
        "# },
        "mod_cross_module_wrapper",
    );

    let signature = generated
        .split("pub fn echo<'py>(")
        .nth(1)
        .and_then(|rest| rest.split('{').next())
        .unwrap_or_else(|| panic!("missing wrapped function signature: {generated}"));
    assert!(
        signature.contains("p_value: &int") && signature.contains("PyResult<int>"),
        "`inspect.signature` and annotation globals must follow the same wrapped function: \
         {signature}"
    );
}

#[test]
fn postponed_builtin_type_aliases_keep_bound_runtime_semantics() {
    let generated = generate_bindings(
        indoc::indoc! { r#"
            from __future__ import annotations

            List = list
            Dict = dict
            Sequence = list
            Type = type

            def lists(value: List[int]) -> Sequence[str]:
                return [str(item) for item in value]

            def dictionaries(value: Dict[str, int]) -> Dict[str, int]:
                return value

            def types(value: Type[int]) -> Type[int]:
                return value
        "# },
        "mod_builtin_type_aliases",
    );

    let lists = generated
        .split("pub fn lists<'py>(")
        .nth(1)
        .and_then(|rest| rest.split('{').next())
        .unwrap_or_else(|| panic!("missing aliased-list signature: {generated}"));
    assert!(
        lists.contains("p_value: &[i64]") && lists.contains("PyResult<Vec<::std::string::String>>"),
        "aliases to `list` must retain concrete list semantics: {lists}"
    );

    let dictionaries = generated
        .split("pub fn dictionaries<'py>(")
        .nth(1)
        .and_then(|rest| rest.split('{').next())
        .unwrap_or_else(|| panic!("missing aliased-dict signature: {generated}"));
    assert!(
        dictionaries.contains("p_value: &::std::collections::HashMap<::std::string::String, i64>",)
            && dictionaries
                .contains("PyResult<::std::collections::HashMap<::std::string::String, i64>>",),
        "aliases to `dict` must retain dictionary semantics: {dictionaries}"
    );

    let types = generated
        .split("pub fn types<'py>(")
        .nth(1)
        .and_then(|rest| rest.split('{').next())
        .unwrap_or_else(|| panic!("missing aliased-type signature: {generated}"));
    assert!(
        types.contains("p_value: &::pyo3::Bound<'py, ::pyo3::types::PyType>")
            && types.contains("PyResult<::pyo3::Bound<'py, ::pyo3::types::PyType>>"),
        "aliases to `type` must retain type-object semantics: {types}"
    );
}

#[test]
fn typing_extensions_metadata_is_an_alias_not_a_mutable_property() {
    let generated = generate_bindings(
        indoc::indoc! { r#"
            try:
                from typing_extensions import Self as BackportSelf
            except ImportError:
                class _SelfMetadata:
                    pass
                _SelfMetadata.__module__ = "typing_extensions"
                BackportSelf = _SelfMetadata()

            try:
                from typing_extensions import TypeAliasType as _TypeAliasType
                BackportAlias = _TypeAliasType("BackportAlias", int)
            except ImportError:
                class _AliasMetadata:
                    pass
                _AliasMetadata.__module__ = "typing_extensions"
                BackportAlias = _AliasMetadata()
        "# },
        "mod_typing_extensions_metadata",
    );

    assert!(
        generated.contains("pub type BackportSelf = ::pyo3::types::PyAny;")
            && generated.contains("pub type BackportAlias = ::pyo3::types::PyAny;"),
        "typing_extensions metadata must be classified as an annotation alias: {generated}"
    );
    assert!(
        !generated.contains("pub fn Backport") && !generated.contains("set_Backport"),
        "typing_extensions metadata must not expose mutable property accessors: {generated}"
    );
}

#[test]
fn ir_string_backed_enum_config_does_not_change_live_enums() {
    let generated = generate_bindings_with_config(
        indoc::indoc! { r#"
            from enum import Enum

            class Color(Enum):
                RED = "red"
        "# },
        "mod_live_enum_ir_config",
        pyo3_bindgen_engine::Config {
            enum_ir_string_backed: true,
            ..Default::default()
        },
    );

    assert!(
        generated.contains("let enum_type =")
            && !generated.contains("PyString::new(py, self.as_py_name())"),
        "IR-only string-backed switch must not change live enum runtime conversion: {generated}"
    );
}

#[test]
fn bindgen_report_tracks_modules_outputs_and_warnings() {
    let cfg = pyo3_bindgen_engine::Config {
        generate_provenance: false,
        ..Default::default()
    };
    let generated = pyo3_bindgen_engine::Codegen::new(cfg)
        .module_from_str(
            indoc::indoc! { r#"
                from enum import Enum, Flag, auto

                class Color(Enum):
                    RED = 1
                    SCARLET = 1

                class Permission(Flag):
                    READ = auto()
                    WRITE = auto()

                class LegacyPermission(Flag):
                    ALL = -1
            "# },
            "mod_bindgen_report",
        )
        .unwrap()
        .generate_with_report()
        .unwrap();

    let report = generated.report;
    assert_eq!(report.schema_version, 1);
    assert_eq!(report.generator.name, "pyo3_bindgen");
    assert_eq!(
        report.generator.pyo3_version.as_deref(),
        Some(env!("PYO3_BINDGEN_PYO3_VERSION")),
        "report must retain the exact PyO3 version selected by build metadata"
    );
    assert!(report.python.is_some());
    assert_eq!(report.modules.len(), 1);
    assert_eq!(report.modules[0].name, "mod_bindgen_report");
    assert_eq!(report.modules[0].input_kind, "embedded_source");
    assert!(report.modules[0].source_hash.is_some());
    // The closed `Flag` class mirrors (backing kind `Flag`); only the negative-valued one
    // keeps the wrapper fallback and is therefore absent from the enum details.
    assert_eq!(report.modules[0].enums.len(), 2);
    assert_eq!(report.modules[0].enums[0].name, "mod_bindgen_report.Color");
    assert_eq!(report.modules[0].enums[0].kind, "Enum");
    assert_eq!(report.modules[0].enums[0].variants[0].python_name, "RED");
    assert_eq!(report.modules[0].enums[0].aliases[0].python_name, "SCARLET");
    assert_eq!(
        report.modules[0].enums[0].aliases[0].target_python_name,
        "RED"
    );
    assert_eq!(
        report.modules[0].enums[1].name,
        "mod_bindgen_report.Permission"
    );
    assert_eq!(report.modules[0].enums[1].kind, "Flag");
    assert_eq!(report.outputs.len(), 1);
    assert_eq!(report.outputs[0].kind, "token_stream");
    assert!(report
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.code == "enum_flag_fallback"
            && diagnostic.path.as_deref() == Some("mod_bindgen_report.LegacyPermission")));
    assert!(
        !report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "enum_flag_fallback"
                && diagnostic.path.as_deref() == Some("mod_bindgen_report.Permission")),
        "a closed int-valued Flag class must no longer warn: {:?}",
        report.diagnostics
    );
}

#[test]
fn bindgen_generated_output_contains_matching_provenance() {
    let generated = pyo3_bindgen_engine::Codegen::default()
        .module_from_str("my_property: int = 1", "mod_bindgen_provenance")
        .unwrap()
        .generate_with_report()
        .unwrap();

    let generated_code =
        prettyplease::unparse(&syn::parse_str(&generated.tokens.to_string()).unwrap());
    let module_report = generated
        .report
        .modules
        .iter()
        .find(|module| module.name == "mod_bindgen_provenance")
        .unwrap();
    assert!(generated_code.contains("pub const PYO3_BINDGEN_GENERATOR_VERSION"));
    assert!(generated_code
        .contains("pub const PYO3_BINDGEN_MODULE_NAME: &str = \"mod_bindgen_provenance\""));
    assert!(generated_code.contains(&format!(
        "pub const PYO3_BINDGEN_INPUT_HASH: &str = \"{}\"",
        module_report.input_hash
    )));
}

#[test]
fn bindgen_provenance_qualifies_str_when_module_shadows_primitive() {
    // A module that emits its own module-scope `str` type (like `builtins`) shadows the
    // primitive, so the provenance consts must be qualified to `&::core::primitive::str`
    // or they fail to compile (E0308). The sibling test above pins the unqualified `&str`
    // for a normal module; this pins the qualified form for a shadowing one, so both
    // directions of the `str_shadowed` branch have a fast, non-ignored guard (the only
    // other coverage is the `#[ignore]`d `builtins` compile sweep).
    let generated = pyo3_bindgen_engine::Codegen::default()
        .module_from_str("class str:\n    pass\n", "mod_bindgen_shadow_str")
        .unwrap()
        .generate()
        .unwrap();

    let generated_code = prettyplease::unparse(&syn::parse_str(&generated.to_string()).unwrap());
    assert!(
        generated_code.contains("pub const PYO3_BINDGEN_MODULE_NAME: &::core::primitive::str ="),
        "a module declaring `str` must qualify provenance consts out of the shadowing newtype"
    );
    assert!(
        !generated_code.contains("pub const PYO3_BINDGEN_MODULE_NAME: &str ="),
        "the bare `&str` form would bind to the module-scope `str` newtype (E0308)"
    );
}

#[test]
fn imported_module_input_hash_tracks_inspected_api_shape() {
    pyo3::Python::initialize();
    let ((first_hash, first_tokens), (second_hash, second_tokens)) = pyo3::Python::attach(|py| {
        pyo3_bindgen_engine::Codegen::prime_live_runtime(py).unwrap();
        let first = pyo3::types::PyModule::from_code(
            py,
            pyo3::ffi::c_str!("def alpha(value: int) -> int:\n    return value\n"),
            pyo3::ffi::c_str!("hash_probe.py"),
            pyo3::ffi::c_str!("hash_probe"),
        )
        .unwrap();
        let hash = |module: &pyo3::Bound<'_, pyo3::types::PyModule>| {
            let generated = pyo3_bindgen_engine::Codegen::default()
                .module(module)
                .unwrap()
                .generate_with_report()
                .unwrap();
            (
                generated.report.modules[0].input_hash.clone(),
                generated.tokens.to_string(),
            )
        };
        let first = hash(&first);
        let second = pyo3::types::PyModule::from_code(
            py,
            pyo3::ffi::c_str!("def beta(value: str) -> str:\n    return value\n"),
            pyo3::ffi::c_str!("hash_probe.py"),
            pyo3::ffi::c_str!("hash_probe"),
        )
        .unwrap();
        (first, hash(&second))
    });

    assert_ne!(first_tokens, second_tokens, "test modules must differ");

    assert_ne!(
        first_hash, second_hash,
        "imported-module drift hashes must change when the inspected API changes",
    );
}

#[test]
fn generation_report_counts_only_enabled_outputs() {
    let cfg = pyo3_bindgen_engine::Config {
        generate_functions: false,
        generate_enums: false,
        ..Default::default()
    };
    let generated = pyo3_bindgen_engine::Codegen::new(cfg)
        .module_from_str(
            indoc::indoc! { r#"
                from enum import Enum

                class Color(Enum):
                    RED = 1

                def hidden() -> int:
                    return 1
            "# },
            "report_enabled_outputs",
        )
        .unwrap()
        .generate_with_report()
        .unwrap();

    let module = &generated.report.modules[0];
    assert_eq!(module.counts.functions, 0);
    assert_eq!(module.counts.enums, 0);
    assert!(module.enums.is_empty());
}

#[test]
fn bindgen_report_tracks_generation_time_renames() {
    let cfg = pyo3_bindgen_engine::Config {
        generate_provenance: false,
        ..Default::default()
    };
    let generated = pyo3_bindgen_engine::Codegen::new(cfg)
        .module_from_str(
            indoc::indoc! { r#"
                import sys

                def generated_function():
                    pass

                generated_function.__name__ = "bad-function"
                setattr(sys.modules[__name__], "bad-function", generated_function)
                setattr(sys.modules[__name__], "bad-class", type("bad-class", (), {}))
            "# },
            "mod_bindgen_generation_report",
        )
        .unwrap()
        .generate_with_report()
        .unwrap();

    assert!(generated
        .report
        .diagnostics
        .iter()
        .any(
            |diagnostic| diagnostic.code == "function_invalid_ident_renamed"
                && diagnostic.path.as_deref() == Some("mod_bindgen_generation_report.bad-function")
        ));
    assert!(generated
        .report
        .diagnostics
        .iter()
        .any(
            |diagnostic| diagnostic.code == "class_invalid_ident_renamed"
                && diagnostic.path.as_deref() == Some("mod_bindgen_generation_report.bad-class")
        ));

    let generated_code =
        prettyplease::unparse(&syn::parse_str(&generated.tokens.to_string()).unwrap());
    assert!(generated_code.contains("pub fn f_bad_function"));
    assert!(generated_code.contains("pub struct s_bad_class"));
}

#[test]
fn bindgen_property() {
    assert_bindgen_eq(
        indoc::indoc! { r#"
            my_property: float = 0.42
        "# },
        "mod_bindgen_property",
        indoc::indoc! { r#"
            #[allow(
                clippy::all,
                clippy::nursery,
                clippy::pedantic,
                non_camel_case_types,
                non_snake_case,
                non_upper_case_globals,
                unused
            )]
            pub mod mod_bindgen_property {
                /// Embed the Python source code of the module into the Python interpreter
                /// in order to enable the use of the generated Rust bindings.
                pub fn pyo3_embed_python_source_code<'py>(
                    py: ::pyo3::marker::Python<'py>,
                ) -> ::pyo3::PyResult<()> {
                    pyo3::types::PyAnyMethods::set_item(
                        &pyo3::types::PyAnyMethods::getattr(
                            py.import(pyo3::intern!(py, "sys"))?.as_any(),
                            pyo3::intern!(py, "modules"),
                        )?,
                        "mod_bindgen_property",
                        pyo3::types::PyModule::from_code(
                            py,
                            ::pyo3::ffi::c_str!("my_property: float = 0.42\n"),
                            ::pyo3::ffi::c_str!("mod_bindgen_property/__init__.py"),
                            ::pyo3::ffi::c_str!("mod_bindgen_property"),
                        )?,
                    )
                }
                pub fn my_property<'py>(
                    py: ::pyo3::marker::Python<'py>,
                ) -> ::pyo3::PyResult<f64> {
                    ::std::result::Result::Ok(::pyo3::types::PyAnyMethods::extract(
                        &::pyo3::types::PyAnyMethods::getattr(
                            py.import(::pyo3::intern!(py, "mod_bindgen_property"))?.as_any(),
                            ::pyo3::intern!(py, "my_property"),
                        )?,
                    )?)
                }
                pub fn set_my_property<'py>(
                    py: ::pyo3::marker::Python<'py>,
                    p_value: f64,
                ) -> ::pyo3::PyResult<()> {
                    ::pyo3::types::PyAnyMethods::setattr(
                        py.import(::pyo3::intern!(py, "mod_bindgen_property"))?.as_any(),
                        ::pyo3::intern!(py, "my_property"),
                        p_value,
                    )
                }
            }
        "# },
    );
}

#[test]
fn bindgen_function() {
    assert_bindgen_eq(
        indoc::indoc! { r#"
            def my_function(my_arg1: str) -> int:
                """My docstring for `my_function`"""
                ...
        "# },
        "mod_bindgen_function",
        indoc::indoc! { r#"
            #[allow(
                clippy::all,
                clippy::nursery,
                clippy::pedantic,
                non_camel_case_types,
                non_snake_case,
                non_upper_case_globals,
                unused
            )]
            pub mod mod_bindgen_function {
                /// Embed the Python source code of the module into the Python interpreter
                /// in order to enable the use of the generated Rust bindings.
                pub fn pyo3_embed_python_source_code<'py>(
                    py: ::pyo3::marker::Python<'py>,
                ) -> ::pyo3::PyResult<()> {
                    pyo3::types::PyAnyMethods::set_item(
                        &pyo3::types::PyAnyMethods::getattr(
                            py.import(pyo3::intern!(py, "sys"))?.as_any(),
                            pyo3::intern!(py, "modules"),
                        )?,
                        "mod_bindgen_function",
                        pyo3::types::PyModule::from_code(
                            py,
                            ::pyo3::ffi::c_str!(
                                "def my_function(my_arg1: str) -> int:\n    \"\"\"My docstring for `my_function`\"\"\"\n    ...\n"
                            ),
                            ::pyo3::ffi::c_str!("mod_bindgen_function/__init__.py"),
                            ::pyo3::ffi::c_str!("mod_bindgen_function"),
                        )?,
                    )
                }
                /// My docstring for `my_function`
                pub fn my_function<'py>(
                    py: ::pyo3::marker::Python<'py>,
                    p_my_arg1: &str,
                ) -> ::pyo3::PyResult<i64> {
                    ::std::result::Result::Ok(::pyo3::types::PyAnyMethods::extract(
                        &::pyo3::types::PyAnyMethods::call_method1(
                            py.import(::pyo3::intern!(py, "mod_bindgen_function"))?.as_any(),
                            ::pyo3::intern!(py, "my_function"),
                            ::pyo3::types::PyTuple::new(
                                py,
                                [::pyo3::IntoPyObjectExt::into_bound_py_any(p_my_arg1, py)?],
                            )?,
                        )?,
                    )?)
                }
            }
        "# },
    );
}

#[test]
fn bindgen_class_uses_owning_wrapper_contract() {
    let generated_code = generate_bindings(
        indoc::indoc! { r#"
            from typing import Dict, Optional
            class MyClass:
                """My docstring for `MyClass`"""
                def __init__(self, my_arg1: str, my_arg2: Optional[int] = None):
                    """My docstring for __init__"""
                    ...
                def my_method(self, my_arg1: Dict[str, int], **kwargs):
                    """My docstring for `my_method`"""
                    ...
                @property
                def my_property(self) -> int:
                    ...
                @my_property.setter
                def my_property(self, value: int):
                    ...

            def my_function_with_class_param(my_arg1: MyClass):
                ...

            def my_function_with_class_return() -> MyClass:
                ...
        "# },
        "mod_bindgen_class",
    );

    let wrapper_start = generated_code
        .find("    /// My docstring for `MyClass`\n    #[derive(Debug)]")
        .unwrap();
    let constructor_start = generated_code
        .find("    #[automatically_derived]\n    impl MyClass {\n        /// My docstring for __init__")
        .unwrap();
    let wrapper_block = generated_code[wrapper_start..constructor_start]
        .lines()
        .map(|line| line.strip_prefix("    ").unwrap_or(line))
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(
        wrapper_block,
        indoc::indoc! { r#"
            /// My docstring for `MyClass`
            #[derive(Debug)]
            pub struct MyClass {
                obj: ::pyo3::Py<::pyo3::types::PyAny>,
            }
            #[automatically_derived]
            impl MyClass {
                pub fn from_py_any<'py>(
                    obj: ::pyo3::Bound<'py, ::pyo3::types::PyAny>,
                ) -> ::pyo3::PyResult<Self> {
                    let py = obj.py();
                    let expected_type = ::pyo3::types::PyAnyMethods::getattr(
                        py.import(::pyo3::intern!(py, "mod_bindgen_class"))?.as_any(),
                        ::pyo3::intern!(py, "MyClass"),
                    )?;
                    if !::pyo3::types::PyAnyMethods::is_instance(
                        obj.as_any(),
                        expected_type.as_any(),
                    )? {
                        return ::std::result::Result::Err(
                            ::pyo3::exceptions::PyTypeError::new_err(
                                ::std::format!(
                                    "expected Python instance of '{}'",
                                    "mod_bindgen_class.MyClass"
                                ),
                            ),
                        );
                    }
                    ::std::result::Result::Ok(Self { obj: obj.unbind() })
                }
                #[inline]
                pub fn bind<'py>(
                    &self,
                    py: ::pyo3::marker::Python<'py>,
                ) -> &::pyo3::Bound<'py, ::pyo3::types::PyAny> {
                    self.obj.bind(py)
                }
                #[inline]
                pub fn as_any<'py>(
                    &self,
                    py: ::pyo3::marker::Python<'py>,
                ) -> &::pyo3::Bound<'py, ::pyo3::types::PyAny> {
                    self.bind(py)
                }
                #[inline]
                pub fn clone_ref(&self, py: ::pyo3::marker::Python<'_>) -> Self {
                    Self {
                        obj: self.obj.clone_ref(py),
                    }
                }
                #[inline]
                pub fn into_py_any(self) -> ::pyo3::Py<::pyo3::types::PyAny> {
                    self.obj
                }
            }
            #[automatically_derived]
            impl<'py> ::pyo3::FromPyObject<'_, 'py> for MyClass {
                type Error = ::pyo3::PyErr;
                fn extract(
                    obj: ::pyo3::Borrowed<'_, 'py, ::pyo3::types::PyAny>,
                ) -> ::std::result::Result<Self, Self::Error> {
                    Self::from_py_any(obj.to_owned())
                }
            }
            #[automatically_derived]
            impl<'py> ::pyo3::IntoPyObject<'py> for MyClass {
                type Target = ::pyo3::types::PyAny;
                type Output = ::pyo3::Bound<'py, Self::Target>;
                type Error = ::std::convert::Infallible;
                fn into_pyobject(
                    self,
                    py: ::pyo3::marker::Python<'py>,
                ) -> ::std::result::Result<Self::Output, Self::Error> {
                    ::std::result::Result::Ok(self.obj.into_bound(py))
                }
            }
            #[automatically_derived]
            impl<'a, 'py> ::pyo3::IntoPyObject<'py> for &'a MyClass {
                type Target = ::pyo3::types::PyAny;
                type Output = ::pyo3::Borrowed<'a, 'py, Self::Target>;
                type Error = ::std::convert::Infallible;
                fn into_pyobject(
                    self,
                    py: ::pyo3::marker::Python<'py>,
                ) -> ::std::result::Result<Self::Output, Self::Error> {
                    ::std::result::Result::Ok(self.obj.bind_borrowed(py))
                }
            }
        "# }
        .trim_end()
    );

    for expected in [
        "pub struct MyClass {\n        obj: ::pyo3::Py<::pyo3::types::PyAny>,\n    }",
        "impl<'py> ::pyo3::FromPyObject<'_, 'py> for MyClass",
        "impl<'py> ::pyo3::IntoPyObject<'py> for MyClass",
        "pub fn bind<'py>(\n            &self,\n            py: ::pyo3::marker::Python<'py>,",
        "pub fn as_any<'py>(\n            &self,\n            py: ::pyo3::marker::Python<'py>,",
        "pub fn clone_ref(&self, py: ::pyo3::marker::Python<'_>) -> Self",
        "pub fn into_py_any(self) -> ::pyo3::Py<::pyo3::types::PyAny>",
        "pub fn new<'py>(\n            py: ::pyo3::marker::Python<'py>,",
        "fn my_method<'py>(\n            &self,\n            py: ::pyo3::marker::Python<'py>,",
        "fn set_my_property<'py>(\n            &self,\n            py: ::pyo3::marker::Python<'py>,",
        "pub fn my_function_with_class_param<'py>(\n        py: ::pyo3::marker::Python<'py>,\n        p_my_arg1: &MyClass,",
        "pub fn my_function_with_class_return<'py>(\n        py: ::pyo3::marker::Python<'py>,\n    ) -> ::pyo3::PyResult<MyClass>",
    ] {
        assert!(
            generated_code.contains(expected),
            "missing expected snippet:\n{expected}\n\nGenerated:\n{generated_code}"
        );
    }

    assert!(!generated_code.contains("pyobject_native_type_info"));
    assert!(!generated_code.contains("derive(Debug, Clone)"));
    assert!(!generated_code.contains("ToPyObject"));
    assert!(!generated_code.contains("Python::with_gil"));
}

#[test]
fn bindgen_enum_mirror_contract() {
    let generated_code = generate_bindings(
        indoc::indoc! { r#"
            from enum import Enum, IntEnum

            class Color(Enum):
                """Paint colors."""
                RED = "red"
                GREEN = "green"
                SCARLET = RED

            class Mode(IntEnum):
                FAST = 1
                SLOW = 2

            def paint(color: Color) -> Color:
                return color

            def mode_value(mode: Mode) -> int:
                return int(mode)
        "# },
        "mod_bindgen_enum",
    );

    for expected in [
        "Python enum mirror for `mod_bindgen_enum.Color`.",
        "pub enum Color",
        "Python enum member `mod_bindgen_enum.Color.RED` with value `'red'`.",
        "#[doc(alias = \"RED\")]\n        Red,",
        "#[doc(alias = \"GREEN\")]\n        Green,",
        "Python enum alias `mod_bindgen_enum.Color.SCARLET` for `mod_bindgen_enum.Color.RED` with value `'red'`.",
        "pub const Scarlet: Self = Self::Red;",
        "pub fn as_py_name(self) -> &'static str",
        "pub fn from_py_name(name: &str) -> ::pyo3::PyResult<Self>",
        "impl<'py> ::pyo3::FromPyObject<'_, 'py> for Color",
        "impl<'py> ::pyo3::IntoPyObject<'py> for Color",
        "impl<'a, 'py> ::pyo3::IntoPyObject<'py> for &'a Color",
        "pub enum Mode",
        "p_color: &Color,",
        ") -> ::pyo3::PyResult<Color>",
        "p_mode: &Mode,",
    ] {
        assert!(
            generated_code.contains(expected),
            "missing expected snippet:\n{expected}\n\nGenerated:\n{generated_code}"
        );
    }

    assert!(!generated_code.contains("pub struct Color"));
    assert!(!generated_code.contains("pub struct Mode"));
}

#[test]
fn bindgen_int_enum_variants_get_explicit_value_discriminants() {
    // `HIGH` is declared before `LOW`/`MEDIUM`, so the derived `Ord` must not
    // be allowed to fall back to declaration order: each variant needs its
    // real Python int value as an explicit Rust discriminant, or `Ord` would
    // silently disagree with Python (Python: `LOW < MEDIUM < HIGH`).
    let generated_code = generate_bindings(
        indoc::indoc! { r#"
            from enum import IntEnum

            class Priority(IntEnum):
                HIGH = 3
                LOW = 1
                MEDIUM = 2
        "# },
        "mod_bindgen_int_enum_discriminant",
    );

    for expected in ["High = 3,", "Low = 1,", "Medium = 2,"] {
        assert!(
            generated_code.contains(expected),
            "missing expected discriminant:\n{expected}\n\nGenerated:\n{generated_code}"
        );
    }
    assert!(generated_code.contains("PartialOrd, Ord"));
}

#[test]
fn bindgen_live_enum_value_repr_preserves_stable_immutable_shapes() {
    let generated = pyo3_bindgen_engine::Codegen::default()
        .module_from_str(
            indoc::indoc! { r#"
                from enum import Enum

                class Marker:
                    pass

                class Value(Enum):
                    STABLE = ("text", 7, True, None, b"bytes", ("only",), frozenset({"z", "a"}), Marker())
                    SAME = STABLE
            "# },
            "mod_bindgen_enum_repr",
        )
        .unwrap()
        .generate_with_report()
        .unwrap();
    let enum_report = generated
        .report
        .modules
        .iter()
        .find(|module| module.name == "mod_bindgen_enum_repr")
        .unwrap()
        .enums
        .iter()
        .find(|enumeration| enumeration.name == "mod_bindgen_enum_repr.Value")
        .unwrap();
    let expected = "('text', 7, True, None, b'bytes', ('only',), frozenset({'a', 'z'}), <mod_bindgen_enum_repr.Marker instance>)";

    assert_eq!(enum_report.variants[0].value_repr, expected);
    assert_eq!(enum_report.aliases[0].value_repr, expected);
    assert!(generated.tokens.to_string().contains(expected));
}

#[test]
fn bindgen_live_enum_value_repr_handles_type_without_module() {
    let generate = || {
        pyo3_bindgen_engine::Codegen::default()
            .module_from_str(
                indoc::indoc! { r#"
                    from enum import Enum

                    class Marker:
                        __module__ = None

                    class Value(Enum):
                        MARKER = Marker()
                "# },
                "mod_bindgen_enum_repr_missing_module",
            )
            .unwrap()
            .generate_with_report()
            .unwrap()
    };
    let generated = generate();
    let regenerated = generate();

    assert_eq!(generated.tokens.to_string(), regenerated.tokens.to_string());
    let value_repr = &generated
        .report
        .modules
        .iter()
        .find(|module| module.name == "mod_bindgen_enum_repr_missing_module")
        .unwrap()
        .enums
        .iter()
        .find(|enumeration| enumeration.name == "mod_bindgen_enum_repr_missing_module.Value")
        .unwrap()
        .variants[0]
        .value_repr;

    assert_eq!(value_repr, "<Marker instance>");
    assert!(generated.tokens.to_string().contains("<Marker instance>"));
}

#[test]
fn bindgen_unsupported_flag_enum_falls_back_to_class_wrapper() {
    // A negative member cannot be stored in the `u64` flag newtype, so this class keeps the
    // owning Python-object wrapper (a named-field struct, not the newtype mirror).
    let generated_code = generate_bindings(
        indoc::indoc! { r#"
            from enum import Flag

            class Permission(Flag):
                ALL = -1

            def default_permission() -> Permission:
                return Permission.ALL
        "# },
        "mod_bindgen_flag",
    );

    assert!(!generated_code.contains("pub enum Permission"));
    assert!(!generated_code.contains("pub struct Permission(u64);"));
    assert!(generated_code.contains("pub struct Permission {"));
    assert!(generated_code.contains(") -> ::pyo3::PyResult<Permission>"));
}

#[test]
fn bindgen_enum_name_collision_falls_back_to_class_wrapper() {
    let generated_code = generate_bindings(
        indoc::indoc! { r#"
            from enum import Enum

            class Weird(Enum):
                A_B = 1
                A__B = 2
        "# },
        "mod_bindgen_enum_collision",
    );

    assert!(!generated_code.contains("pub enum Weird"));
    assert!(generated_code.contains("pub struct Weird"));
}

#[test]
fn bindgen_enum_generation_can_be_disabled() {
    let generated_code = generate_bindings_with_config(
        indoc::indoc! { r#"
            from enum import Enum

            class Color(Enum):
                RED = "red"

            class Widget:
                pass
        "# },
        "mod_bindgen_enum_disabled",
        pyo3_bindgen_engine::Config::builder()
            .generate_enums(false)
            .build(),
    );

    assert!(!generated_code.contains("pub enum Color"));
    assert!(
        !generated_code.contains("pub struct Color"),
        "disabling enum generation must not emit an enum-derived fallback class: {generated_code}"
    );
    assert!(
        generated_code.contains("pub struct Widget"),
        "disabling enum generation must preserve unrelated classes: {generated_code}"
    );
}

#[test]
fn external_ir_enum_generation_can_be_disabled_without_fallback_class() {
    let module = pyo3_bindgen_engine::ModuleIr {
        name: "mod_external_enum_disabled".to_owned(),
        docstring: None,
        is_package: false,
        classes: vec![pyo3_bindgen_engine::ClassIr {
            name: "mod_external_enum_disabled.Widget".to_owned(),
            base_classes: vec![],
            docstring: None,
            methods: vec![],
            properties: vec![],
        }],
        enums: vec![pyo3_bindgen_engine::EnumIr {
            name: "mod_external_enum_disabled.Color".to_owned(),
            kind: pyo3_bindgen_engine::EnumKindIr::Enum,
            variants: vec![],
            aliases: vec![],
            docstring: None,
            is_flag: false,
            is_open: false,
            string_backed: false,
            plain_mirror: false,
        }],
        functions: vec![],
        properties: vec![],
        submodules: vec![],
        prelude: vec![],
        imports: vec![],
        type_vars: vec![],
    };
    let generated_code = pyo3_bindgen_engine::Codegen::new(
        pyo3_bindgen_engine::Config::builder()
            .generate_enums(false)
            .enum_ir_class_fallback(true)
            .build(),
    )
    .module_from_ir(module)
    .unwrap()
    .generate()
    .unwrap();
    let generated_code = prettyplease::unparse(&syn::parse2(generated_code).unwrap());

    assert!(!generated_code.contains("pub enum Color"));
    assert!(
        !generated_code.contains("pub struct Color"),
        "disabling enum generation must not emit an external-IR fallback class: {generated_code}"
    );
    assert!(
        generated_code.contains("pub struct Widget"),
        "disabling enum generation must preserve unrelated external-IR classes: {generated_code}"
    );
}

#[test]
fn bindgen_enum_generation_can_be_disabled_by_exact_path() {
    let generated_code = generate_bindings_with_config(
        indoc::indoc! { r#"
            from enum import Enum

            class Color(Enum):
                RED = "red"
        "# },
        "mod_bindgen_enum_path_fallback",
        pyo3_bindgen_engine::Config::builder()
            .enum_raw_fallback_paths(vec!["mod_bindgen_enum_path_fallback.Color".to_owned()])
            .build(),
    );

    assert!(!generated_code.contains("pub enum Color"));
    assert!(generated_code.contains("pub struct Color"));
}

#[test]
fn generated_fixture_compiles_and_runs() {
    let code_py = indoc::indoc! { r#"
        class Widget:
            def __init__(self, value: int):
                if value < 0:
                    raise ValueError("bad widget")
                self._value = value

            @property
            def value(self) -> int:
                return self._value

            @value.setter
            def value(self, value: int):
                self._value = value

            @property
            def read_only(self) -> int:
                return self._value + 1

            @property
            def explode(self) -> int:
                raise ValueError("bad property")

            def bump(self, delta: int) -> int:
                self._value += delta
                return self._value

            def fail_method(self):
                raise ValueError("bad method")

        class DictSubclass(dict):
            pass

        def make_widget(value: int) -> Widget:
            return Widget(value)

        def raw_value():
            return 7

        def fail():
            raise ValueError("boom")
    "# };
    let generated_code = generate_bindings(code_py, "mod_bindgen_runtime");
    assert!(!generated_code.contains("set_read_only"));

    let mut working_directory = bounded_consumer::OwnedTempDir::allocate("bindgen_runtime")
        .expect("generated runtime consumer directory must be allocated exclusively");
    let crate_dir = working_directory.path().to_owned();
    std::fs::create_dir(crate_dir.join("src")).unwrap();

    let pyo3_version = env!("PYO3_BINDGEN_PYO3_VERSION");
    std::fs::write(
        crate_dir.join("Cargo.toml"),
        format!(
            r#"[package]
name = "pyo3_bindgen_runtime_smoke"
version = "0.0.0"
edition = "2021"
publish = false

[dependencies]
pyo3 = {{ version = "={pyo3_version}", features = ["auto-initialize"] }}
"#
        ),
    )
    .unwrap();

    std::fs::write(
        crate_dir.join("src/lib.rs"),
        format!(
            r#"{generated_code}

#[cfg(test)]
mod tests {{
    use super::mod_bindgen_runtime::{{self, DictSubclass, Widget, WidgetMethods}};
    use pyo3::types::PyAnyMethods;

    #[test]
    fn generated_bindings_run() -> pyo3::PyResult<()> {{
        pyo3::Python::attach(|py| {{
            mod_bindgen_runtime::pyo3_embed_python_source_code(py)?;

            let widget = Widget::new(py, 41)?;
            assert_eq!(widget.value(py)?, 41);
            widget.set_value(py, 100)?;
            assert_eq!(widget.value(py)?, 100);
            assert_eq!(widget.read_only(py)?, 101);
            assert_eq!(widget.bump(py, 1)?, 101);
            assert_eq!(widget.read_only(py)?, 102);

            let widget_ref = widget.clone_ref(py);
            widget.set_value(py, 7)?;
            assert_eq!(widget_ref.value(py)?, 7);

            let class_name: String = widget
                .as_any(py)
                .getattr(pyo3::intern!(py, "__class__"))?
                .getattr(pyo3::intern!(py, "__name__"))?
                .extract()?;
            assert_eq!(class_name, "Widget");

            let made = mod_bindgen_runtime::make_widget(py, 5)?;
            assert_eq!(made.value(py)?, 5);
            let made_raw = made.clone_ref(py).into_py_any();
            assert_eq!(made_raw.bind(py).getattr(pyo3::intern!(py, "value"))?.extract::<i64>()?, 5);

            let raw = mod_bindgen_runtime::raw_value(py)?;
            assert_eq!(raw.extract::<i64>()?, 7);

            let fromkeys = DictSubclass::fromkeys(py, ["a", "b"], 7_i64)?;
            assert_eq!(
                fromkeys.extract::<std::collections::HashMap<String, i64>>()?,
                std::collections::HashMap::from([
                    ("a".to_owned(), 7_i64),
                    ("b".to_owned(), 7_i64),
                ]),
            );

            let err = Widget::new(py, -1).unwrap_err();
            assert!(err.is_instance_of::<pyo3::exceptions::PyValueError>(py));

            let err = widget.fail_method(py).unwrap_err();
            assert!(err.is_instance_of::<pyo3::exceptions::PyValueError>(py));

            let err = widget.explode(py).unwrap_err();
            assert!(err.is_instance_of::<pyo3::exceptions::PyValueError>(py));

            let err = mod_bindgen_runtime::fail(py).unwrap_err();
            assert!(err.is_instance_of::<pyo3::exceptions::PyValueError>(py));
            Ok(())
        }})
    }}
}}
"#
        ),
    )
    .unwrap();

    let mut command =
        std::process::Command::new(std::env::var("CARGO").unwrap_or("cargo".to_owned()));
    command
        .arg("test")
        .arg("--quiet")
        .current_dir(&crate_dir)
        .env("CARGO_TARGET_DIR", crate_dir.join("target"));
    let output = working_directory
        .run(&mut command, bounded_consumer::CARGO_COMMAND_TIMEOUT)
        .unwrap_or_else(|error| {
            panic!(
                "bounded generated runtime smoke failed: {error}\nstdout:\n{}\nstderr:\n{}",
                error.stdout(),
                error.stderr()
            )
        });

    assert!(
        output.status.success(),
        "generated runtime smoke failed\nstatus: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        output.stdout,
        output.stderr
    );
}

#[test]
fn generated_enum_fixture_compiles_and_runs() {
    let code_py = indoc::indoc! { r#"
        from enum import Enum, Flag, IntEnum, auto

        class Color(Enum):
            RED = "red"
            GREEN = "green"
            SCARLET = RED

        class OtherColor(Enum):
            RED = "other-red"

        class Mode(IntEnum):
            FAST = 1
            SLOW = 2

        class Permission(Flag):
            READ = auto()
            WRITE = auto()

        def paint(color: Color) -> Color:
            return color

        def describe(color: Color) -> str:
            return color.name

        def mode_value(mode: Mode) -> int:
            return int(mode)

        def default_permission() -> Permission:
            return Permission.READ
    "# };
    let generated_code = generate_bindings(code_py, "mod_bindgen_enum_runtime");

    let mut working_directory = bounded_consumer::OwnedTempDir::allocate("bindgen_enum_runtime")
        .expect("generated enum runtime consumer directory must be allocated exclusively");
    let crate_dir = working_directory.path().to_owned();
    std::fs::create_dir(crate_dir.join("src")).unwrap();

    let pyo3_version = env!("PYO3_BINDGEN_PYO3_VERSION");
    std::fs::write(
        crate_dir.join("Cargo.toml"),
        format!(
            r#"[package]
name = "pyo3_bindgen_enum_runtime_smoke"
version = "0.0.0"
edition = "2021"
publish = false

[dependencies]
pyo3 = {{ version = "={pyo3_version}", features = ["auto-initialize"] }}
"#
        ),
    )
    .unwrap();

    std::fs::write(
        crate_dir.join("src/lib.rs"),
        format!(
            r#"{generated_code}

#[cfg(test)]
mod tests {{
    use super::mod_bindgen_enum_runtime::{{self, Color, Mode, Permission}};
    use pyo3::{{IntoPyObject, types::PyAnyMethods}};

    #[test]
    fn generated_enum_bindings_run() -> pyo3::PyResult<()> {{
        pyo3::Python::attach(|py| {{
            mod_bindgen_enum_runtime::pyo3_embed_python_source_code(py)?;

            assert_eq!(Color::Red.as_py_name(), "RED");
            assert_eq!(Color::Scarlet, Color::Red);
            assert_eq!(Color::from_py_name("SCARLET")?, Color::Red);
            assert_eq!(mod_bindgen_enum_runtime::paint(py, &Color::Red)?, Color::Red);
            assert_eq!(mod_bindgen_enum_runtime::describe(py, &Color::Green)?, "GREEN");

            let py_alias = Color::Scarlet.into_pyobject(py)?;
            assert_eq!(py_alias.getattr(pyo3::intern!(py, "name"))?.extract::<String>()?, "RED");
            assert_eq!(py_alias.extract::<Color>()?, Color::Red);

            let wrong_enum_member = py
                .import("mod_bindgen_enum_runtime")?
                .getattr("OtherColor")?
                .getattr("RED")?;
            assert!(
                wrong_enum_member.extract::<Color>().is_err(),
                "a member of a different Python enum must not extract as Color",
            );
            assert!(
                pyo3::types::PyString::new(py, "RED").extract::<Color>().is_err(),
                "a bare member-name string must not extract as Color",
            );

            assert_eq!(Mode::from_py_name("SLOW")?, Mode::Slow);
            assert_eq!(mod_bindgen_enum_runtime::mode_value(py, &Mode::Fast)?, 1);

            let permission = mod_bindgen_enum_runtime::default_permission(py)?;
            assert_eq!(permission, Permission::Read);
            assert_eq!(permission.value(), 1);

            py.import("mod_bindgen_enum_runtime")?.delattr("Color")?;
            assert!(
                Color::Red.into_pyobject(py).is_err(),
                "failure to resolve the mirrored Python enum must remain an error",
            );
            Ok(())
        }})
    }}
}}
"#
        ),
    )
    .unwrap();

    let mut command =
        std::process::Command::new(std::env::var("CARGO").unwrap_or("cargo".to_owned()));
    command
        .arg("test")
        .arg("--quiet")
        .current_dir(&crate_dir)
        .env("CARGO_TARGET_DIR", crate_dir.join("target"));
    let output = working_directory
        .run(&mut command, bounded_consumer::CARGO_COMMAND_TIMEOUT)
        .unwrap_or_else(|error| {
            panic!(
                "bounded generated enum runtime smoke failed: {error}\nstdout:\n{}\nstderr:\n{}",
                error.stdout(),
                error.stderr()
            )
        });

    assert!(
        output.status.success(),
        "generated enum runtime smoke failed\nstatus: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        output.stdout,
        output.stderr
    );
}

/// Runtime oracle for the closed-`Flag`/`IntFlag` value-newtype mirror: the generated
/// structs must compile AND round-trip through the real Python classes -- Rust set
/// operations produce the Python member with the same `.value`, extraction recovers the
/// same bits, and a value that is not an instance of the mirrored class surfaces as a
/// `PyResult` error rather than a panic or a fabricated member.
#[test]
fn generated_flag_enum_fixture_compiles_and_runs() {
    let code_py = indoc::indoc! { r#"
        import enum

        class Perm(enum.Flag):
            R = 1
            W = 2
            X = 4
            RW = 3

        class Mode(enum.IntFlag):
            A = 1
            B = 2
            ALIAS_A = 1

        def echo_perm(perm: Perm) -> Perm:
            return perm

        def perm_value(perm: Perm) -> int:
            return perm.value
    "# };
    let generated_code = generate_bindings(code_py, "mod_flag_runtime");

    let mut working_directory = bounded_consumer::OwnedTempDir::allocate("bindgen_flag_runtime")
        .expect("generated flag runtime consumer directory must be allocated exclusively");
    let crate_dir = working_directory.path().to_owned();
    std::fs::create_dir(crate_dir.join("src")).unwrap();

    let pyo3_version = env!("PYO3_BINDGEN_PYO3_VERSION");
    std::fs::write(
        crate_dir.join("Cargo.toml"),
        format!(
            r#"[package]
name = "pyo3_bindgen_flag_runtime_smoke"
version = "0.0.0"
edition = "2021"
publish = false

[dependencies]
pyo3 = {{ version = "={pyo3_version}", features = ["auto-initialize"] }}
"#
        ),
    )
    .unwrap();

    std::fs::write(
        crate_dir.join("src/lib.rs"),
        format!(
            r#"{generated_code}

#[cfg(test)]
mod tests {{
    use super::mod_flag_runtime::{{self, Mode, Perm}};
    use pyo3::{{IntoPyObject, types::PyAnyMethods}};

    #[test]
    fn generated_flag_bindings_run() -> pyo3::PyResult<()> {{
        pyo3::Python::attach(|py| {{
            mod_flag_runtime::pyo3_embed_python_source_code(py)?;

            // Set operations are pure Rust bit math over the member consts.
            let rw = Perm::R | Perm::W;
            assert_eq!(rw, Perm::Rw);
            assert_eq!(rw.value(), 3);
            assert!(rw.contains(Perm::R) && rw.contains(Perm::W));
            assert!(!rw.contains(Perm::X));
            assert!(!rw.is_empty());
            assert!((Perm::R & Perm::W).is_empty());
            assert_eq!(Perm::R & Perm::Rw, Perm::R);
            assert_eq!(Perm::R ^ Perm::Rw, Perm::W);

            // Into Python: the real class is CALLED with the bits, so Python decides which
            // member (or composite) the value denotes.
            let py_rw = rw.into_pyobject(py)?;
            assert_eq!(py_rw.getattr(pyo3::intern!(py, "value"))?.extract::<u64>()?, 3);
            let class_name: String = py_rw
                .getattr(pyo3::intern!(py, "__class__"))?
                .getattr(pyo3::intern!(py, "__name__"))?
                .extract()?;
            assert_eq!(class_name, "Perm");

            // Back-conversion recovers the same bits.
            assert_eq!(py_rw.extract::<Perm>()?, Perm::Rw);
            assert_eq!(mod_flag_runtime::echo_perm(py, &rw)?, Perm::Rw);
            assert_eq!(mod_flag_runtime::perm_value(py, &rw)?, 3);

            // The empty set survives the round trip without fabricating a member.
            let empty = Perm::R ^ Perm::R;
            assert!(empty.is_empty());
            assert_eq!(empty.into_pyobject(py)?.getattr(pyo3::intern!(py, "value"))?.extract::<u64>()?, 0);

            // IntFlag mirrors the same way, and an alias shares its target's bits.
            assert_eq!(Mode::AliasA, Mode::A);
            assert_eq!((Mode::A | Mode::B).into_pyobject(py)?.extract::<Mode>()?.value(), 3);

            // An out-of-class value is a `PyResult` error, never a panic.
            let three = 3_i64.into_pyobject(py)?.into_any();
            let err = three.extract::<Perm>().unwrap_err();
            assert!(err.is_instance_of::<pyo3::exceptions::PyTypeError>(py));
            let other_member = py
                .import("mod_flag_runtime")?
                .getattr("Mode")?
                .getattr("A")?;
            assert!(
                other_member.extract::<Perm>().is_err(),
                "a member of a different Python flag class must not extract as Perm",
            );

            // Losing the mirrored Python class stays an error too.
            py.import("mod_flag_runtime")?.delattr("Perm")?;
            assert!(
                rw.into_pyobject(py).is_err(),
                "failure to resolve the mirrored Python flag class must remain an error",
            );
            Ok(())
        }})
    }}
}}
"#
        ),
    )
    .unwrap();

    let mut command =
        std::process::Command::new(std::env::var("CARGO").unwrap_or("cargo".to_owned()));
    command
        .arg("test")
        .arg("--quiet")
        .current_dir(&crate_dir)
        .env("CARGO_TARGET_DIR", crate_dir.join("target"));
    let output = working_directory
        .run(&mut command, bounded_consumer::CARGO_COMMAND_TIMEOUT)
        .unwrap_or_else(|error| {
            panic!(
                "bounded generated flag runtime smoke failed: {error}\nstdout:\n{}\nstderr:\n{}",
                error.stdout(),
                error.stderr()
            )
        });

    assert!(
        output.status.success(),
        "generated flag runtime smoke failed\nstatus: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        output.stdout,
        output.stderr
    );
}

fn config_without_dedup() -> pyo3_bindgen_engine::Config {
    pyo3_bindgen_engine::Config::builder()
        .deduplicate_inherited_members(false)
        .build()
}

/// A block of `count` distinct instance methods, each with parameters and a docstring, so a
/// hierarchy sharing them clears the deduplication cost model's break-even by a wide margin
/// -- keeping the dedup tests robust to exact cost-constant tuning.
fn documented_methods(count: usize) -> String {
    (0..count)
        .map(|i| {
            format!(
                "    def m{i}(self, a: int, b: str, c: float) -> int:\n\
                \x20       \"\"\"Documentation for method m{i}: a reasonably long description so \
                the emitted method is substantial.\"\"\"\n\
                \x20       ...\n"
            )
        })
        .collect()
}

#[test]
fn live_deduplicates_inherited_members_via_supertraits() {
    // A deep hierarchy: `Base` carries the bulk of the members (enough to clear the cost
    // model's root-support break-even), inherited by `Mid` and `Leaf`. Deduplication should
    // share them through `*Methods` supertrait bounds rather than redeclaring them on
    // every subclass.
    let base_methods = documented_methods(20);
    let code = format!(
        "class Base:\n\
        {base_methods}\
        class Mid(Base):\n\
        \x20   def mid_only(self): ...\n\
        class Leaf(Mid):\n\
        \x20   def leaf_only(self): ...\n"
    );
    let dedup = generate_bindings(&code, "mod_dedup_deep");
    let flat = generate_bindings_with_config(&code, "mod_dedup_deep", config_without_dedup());

    // Deduplication gives subclasses a supertrait bound; the flat model leaves the trait
    // standalone.
    assert!(
        dedup.contains("pub trait MidMethods: "),
        "deduplicated Mid must inherit a supertrait:\n{dedup}"
    );
    assert!(
        dedup.contains("pub trait LeafMethods: "),
        "deduplicated Leaf must inherit a supertrait:\n{dedup}"
    );
    assert!(
        flat.contains("pub trait LeafMethods {") && flat.contains("pub trait MidMethods {"),
        "the flat model must leave every subclass trait standalone:\n{flat}"
    );
    assert!(
        !dedup.contains("pub trait LeafMethods {") && !dedup.contains("pub trait MidMethods {"),
        "deduplication must not leave a subclass trait standalone:\n{dedup}"
    );

    // Nothing is lost: `Leaf` implements its ancestors' traits, so an inherited member
    // like `m0` stays callable through them.
    assert!(
        dedup.contains("BaseMethods for Leaf"),
        "Leaf must implement BaseMethods so inherited members stay callable:\n{dedup}"
    );

    // Per-member reduplication factor: an inherited method is emitted only on its owner
    // under dedup (its `*Methods` declaration plus the owner's own inherent forwarder),
    // reached by every subclass through the supertrait -- but redeclared on all three
    // classes under the flat model. So the flat count is exactly 3x the dedup count. The
    // aggregate size check below would miss a *partial* drop (one subclass keeps the member)
    // that still nets smaller; this pins it exactly.
    let dedup_m0 = dedup.matches("fn m0<'py>").count();
    let flat_m0 = flat.matches("fn m0<'py>").count();
    assert!(
        dedup_m0 > 0,
        "m0 must still be emitted on its owner under dedup:\n{dedup}"
    );
    assert_eq!(
        flat_m0,
        3 * dedup_m0,
        "the flat model must redeclare m0 on all 3 classes, dedup only on its owner \
         (dedup {dedup_m0} vs flat {flat_m0})"
    );

    // Strict size win for a deep hierarchy.
    assert!(
        dedup.len() < flat.len(),
        "deduplicated output ({}) must be smaller than the flat output ({})",
        dedup.len(),
        flat.len()
    );
}

#[test]
fn live_cross_module_collision_wrappers_compile_with_owner_local_annotations() {
    // `A` stores postponed, unqualified `Token` annotations that resolve through module
    // `a`'s import. Module `c` declares a *different* Token. Replaying A's signature in
    // C's ordinary local namespace therefore binds the wrong nominal wrapper and fails
    // when delegated through AMethods. Conversely, A's unbound `ChildOnly` annotation
    // must remain PyAny even though C declares that name: Python resolves annotations in
    // the defining function's globals, never the subclass module's globals. Exercise a
    // method plus mutable-property getter/setter so all inherited forwarder shapes stay
    // covered.
    pyo3::Python::initialize();
    let generated = pyo3::Python::attach(|py| {
        use pyo3::types::{PyAnyMethods, PyDict, PyDictMethods, PyList, PyModule, PyModuleMethods};

        pyo3_bindgen_engine::Codegen::prime_live_runtime(py).unwrap();

        let package_name = "mod_live_owner_local_forwarder";
        let package = PyModule::new(py, package_name).unwrap();
        package.setattr("__path__", PyList::empty(py)).unwrap();
        let types = PyModule::new(py, &format!("{package_name}.types")).unwrap();
        let a = PyModule::new(py, &format!("{package_name}.a")).unwrap();
        let b = PyModule::new(py, &format!("{package_name}.b")).unwrap();
        let c = PyModule::new(py, &format!("{package_name}.c")).unwrap();
        let sys_modules = py
            .import("sys")
            .unwrap()
            .getattr("modules")
            .unwrap()
            .cast_into::<PyDict>()
            .unwrap();
        for (name, module) in [
            (package_name.to_owned(), &package),
            (format!("{package_name}.types"), &types),
            (format!("{package_name}.a"), &a),
            (format!("{package_name}.b"), &b),
            (format!("{package_name}.c"), &c),
        ] {
            sys_modules.set_item(name, module).unwrap();
        }
        package.setattr("types", &types).unwrap();
        package.setattr("a", &a).unwrap();
        package.setattr("b", &b).unwrap();
        package.setattr("c", &c).unwrap();

        py.run(
            pyo3::ffi::c_str!("class Token:\n    pass"),
            Some(&types.dict()),
            None,
        )
        .unwrap();
        let a_source = std::ffi::CString::new(format!(
            "from __future__ import annotations\n\
             from {package_name}.types import Token\n\
             class A:\n\
             \x20   def collide(self, value: Token) -> Token:\n\
             \x20       return value\n\
             \x20   def child_only(self) -> ChildOnly:\n\
             \x20       return None\n\
             \x20   @property\n\
             \x20   def item(self) -> Token:\n\
             \x20       return Token()\n\
             \x20   @item.setter\n\
             \x20   def item(self, value: Token) -> None:\n\
             \x20       pass\n\
             {}",
            documented_methods(20),
        ))
        .unwrap();
        py.run(a_source.as_c_str(), Some(&a.dict()), None).unwrap();
        py.run(
            pyo3::ffi::c_str!(
                "class B:\n\
                 \x20   def collide(self, value: int) -> int:\n\
                 \x20       return value\n\
                 \x20   def child_only(self) -> int:\n\
                 \x20       return 0\n\
                 \x20   @property\n\
                 \x20   def item(self) -> int:\n\
                 \x20       return 0\n\
                 \x20   @item.setter\n\
                 \x20   def item(self, value: int) -> None:\n\
                 \x20       pass"
            ),
            Some(&b.dict()),
            None,
        )
        .unwrap();
        let c_source = std::ffi::CString::new(format!(
            "from {package_name}.a import A\n\
             from {package_name}.b import B\n\
             class Token:\n\
             \x20   pass\n\
             class ChildOnly:\n\
             \x20   pass\n\
             class C(A, B):\n\
             \x20   pass"
        ))
        .unwrap();
        py.run(c_source.as_c_str(), Some(&c.dict()), None).unwrap();

        let tokens = pyo3_bindgen_engine::Codegen::new(pyo3_bindgen_engine::Config {
            generate_provenance: false,
            ..Default::default()
        })
        .modules([
            package.clone(),
            types.clone(),
            a.clone(),
            b.clone(),
            c.clone(),
        ])
        .unwrap()
        .generate()
        .unwrap();

        for name in [
            format!("{package_name}.c"),
            format!("{package_name}.b"),
            format!("{package_name}.a"),
            format!("{package_name}.types"),
            package_name.to_owned(),
        ] {
            sys_modules.del_item(name).unwrap();
        }
        prettyplease::unparse(&syn::parse2(tokens).unwrap())
    });

    assert!(
        generated.contains("pub trait CMethods: "),
        "fixture must select the deduplicated model:\n{generated}"
    );
    let c_impl = generated
        .rsplit("impl C {")
        .next()
        .and_then(|rest| rest.split("\n        }").next())
        .expect("C's inherent collision-forwarder impl must be generated");
    assert!(
        c_impl.contains("pub fn collide")
            && c_impl.contains("pub fn child_only")
            && c_impl.contains("pub fn item")
            && c_impl.contains("pub fn set_item")
            && c_impl.contains("<Self as super::a::AMethods>::collide")
            && c_impl.contains("p_value: &super::types::Token")
            && c_impl.contains("::pyo3::PyResult<super::types::Token>")
            && c_impl.contains("<Self as super::a::AMethods>::child_only")
            && !c_impl.contains("::pyo3::PyResult<ChildOnly>"),
        "inherited collision wrappers must resolve annotations in A's namespace while \
         spelling their Rust paths relative to C:\n{c_impl}"
    );

    let mut working_directory =
        bounded_consumer::OwnedTempDir::allocate("live_owner_local_forwarder")
            .expect("generated forwarder consumer directory must be allocated exclusively");
    let crate_dir = working_directory.path().to_owned();
    std::fs::create_dir(crate_dir.join("src")).unwrap();
    let pyo3_version = env!("PYO3_BINDGEN_PYO3_VERSION");
    std::fs::write(
        crate_dir.join("Cargo.toml"),
        format!(
            r#"[package]
name = "pyo3_bindgen_live_owner_local_forwarder"
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
    std::fs::write(crate_dir.join("src/lib.rs"), generated).unwrap();
    let mut command =
        std::process::Command::new(std::env::var("CARGO").unwrap_or("cargo".to_owned()));
    command
        .arg("check")
        .arg("--quiet")
        .arg("--offline")
        .current_dir(&crate_dir)
        .env("CARGO_TARGET_DIR", crate_dir.join("target"));
    let output = working_directory
        .run(&mut command, bounded_consumer::CARGO_COMMAND_TIMEOUT)
        .unwrap_or_else(|error| {
            panic!(
                "bounded cross-module live inherited forwarder check failed: {error}\nstdout:\n{}\nstderr:\n{}",
                error.stdout(),
                error.stderr()
            )
        });

    assert!(
        output.status.success(),
        "cross-module live inherited forwarders failed to compile\nstatus: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        output.stdout,
        output.stderr,
    );
}

#[test]
fn live_dedup_falls_back_when_exact_output_would_grow() {
    let base_name = format!("Base{}", "A".repeat(5_000));
    let methods = documented_methods(20);
    let code = format!(
        "class {base_name}:\n\
        {methods}\
        class Derived({base_name}):\n\
        \x20   def own(self): ...\n"
    );

    let adaptive = generate_bindings(&code, "mod_dedup_long_path");
    let flat = generate_bindings_with_config(&code, "mod_dedup_long_path", config_without_dedup());

    assert_eq!(
        adaptive, flat,
        "adaptive live dedup must choose the flat candidate when long emitted paths make \
         the deduplicated candidate larger"
    );
}

#[test]
fn live_dedup_preserves_subclass_field_reannotations() {
    let methods = documented_methods(20);
    let code = format!(
        "class Base:\n\
        \x20   value: int = 0\n\
        {methods}\
        class Derived(Base):\n\
        \x20   value: str\n"
    );
    let dedup = generate_bindings(&code, "mod_dedup_reannotated_field");
    let derived_methods = dedup
        .split("pub trait DerivedMethods")
        .nth(1)
        .and_then(|rest| rest.split("impl DerivedMethods").next())
        .expect("DerivedMethods trait must be generated");

    assert!(
        dedup.contains("pub trait DerivedMethods: "),
        "fixture must select the deduplicated model:\n{dedup}"
    );
    assert!(
        derived_methods.contains("fn value<'py>(")
            && derived_methods.contains("::std::string::String")
            && derived_methods.contains("p_value: &str"),
        "a subclass field re-annotation must remain an own typed property instead of \
         inheriting the base property's Rust type:\n{derived_methods}"
    );
}

#[test]
fn live_dedup_preserves_out_of_scope_intermediate_field_reannotations() {
    let methods = documented_methods(20);
    let code = format!(
        "class Base:\n\
        \x20   value: int = 0\n\
        {methods}\
        class _Middle(Base):\n\
        \x20   value: str\n\
        class Leaf(_Middle):\n\
        \x20   pass\n"
    );
    let dedup = generate_bindings(&code, "mod_dedup_intermediate_reannotation");
    let leaf_methods = dedup
        .split("pub trait LeafMethods")
        .nth(1)
        .and_then(|rest| rest.split("impl LeafMethods").next())
        .expect("LeafMethods trait must be generated");

    assert!(
        dedup.contains("pub trait LeafMethods: "),
        "fixture must select the deduplicated model:\n{dedup}"
    );
    let leaf_trait_header = dedup
        .split("pub trait LeafMethods:")
        .nth(1)
        .and_then(|rest| rest.split('{').next())
        .expect("LeafMethods supertrait header must be generated");
    assert!(
        leaf_trait_header.contains("BaseMethods"),
        "an in-scope grandparent must be promoted across an out-of-scope direct base:\n\
         {leaf_trait_header}"
    );
    assert!(
        leaf_methods.contains("fn value<'py>(")
            && leaf_methods.contains("::std::string::String")
            && leaf_methods.contains("p_value: &str"),
        "a field re-annotation on an out-of-scope intermediate base must remain on the \
         generated leaf instead of inheriting an older base type:\n{leaf_methods}"
    );
}

#[test]
fn live_dedup_does_not_mutate_imported_module_fingerprints() {
    pyo3::Python::initialize();
    pyo3::Python::attach(|py| {
        use pyo3::types::PyModule;

        pyo3_bindgen_engine::Codegen::prime_live_runtime(py).unwrap();

        let source = std::ffi::CString::new(format!(
            "class Base:\n{}class Derived(Base):\n    def own(self): ...\n",
            documented_methods(20)
        ))
        .unwrap();
        let module = PyModule::from_code(
            py,
            source.as_c_str(),
            pyo3::ffi::c_str!("mod_dedup_fingerprint.py"),
            pyo3::ffi::c_str!("mod_dedup_fingerprint"),
        )
        .unwrap();
        let generate = |deduplicate_inherited_members| {
            pyo3_bindgen_engine::Codegen::new(pyo3_bindgen_engine::Config {
                deduplicate_inherited_members,
                generate_provenance: true,
                ..Default::default()
            })
            .module(&module)
            .unwrap()
            .generate_with_report()
            .unwrap()
        };

        let adaptive = generate(true);
        let flat = generate(false);
        let adaptive_hash = &adaptive.report.modules[0].input_hash;
        let flat_hash = &flat.report.modules[0].input_hash;
        assert_eq!(
            adaptive_hash, flat_hash,
            "an imported-module fingerprint must not depend on emission deduplication"
        );
        let adaptive_source = prettyplease::unparse(&syn::parse2(adaptive.tokens).unwrap());
        assert!(
            adaptive_source.contains("pub trait DerivedMethods: "),
            "fixture must select the deduplicated model:\n{adaptive_source}"
        );
        assert!(
            adaptive_source.contains(&format!(
                "PYO3_BINDGEN_INPUT_HASH: &str = \"{adaptive_hash}\""
            )),
            "provenance token and report must use the same pre-dedup fingerprint"
        );
    });
}

#[test]
fn live_dedup_ignores_submodules_disabled_for_emission() {
    pyo3::Python::initialize();
    pyo3::Python::attach(|py| {
        use pyo3::types::{PyAnyMethods, PyDict, PyDictMethods, PyModule, PyModuleMethods};

        pyo3_bindgen_engine::Codegen::prime_live_runtime(py).unwrap();

        let package = PyModule::new(py, "mod_dedup_pkg").unwrap();
        let submodule = PyModule::new(py, "mod_dedup_pkg.sub").unwrap();
        let modules = py
            .import("sys")
            .unwrap()
            .getattr("modules")
            .unwrap()
            .cast_into::<PyDict>()
            .unwrap();
        modules.set_item("mod_dedup_pkg", &package).unwrap();
        modules.set_item("mod_dedup_pkg.sub", &submodule).unwrap();
        let base_source =
            std::ffi::CString::new(format!("class Base:\n{}", documented_methods(20))).unwrap();
        py.run(base_source.as_c_str(), Some(&submodule.dict()), None)
            .unwrap();
        package
            .setattr("Base", submodule.getattr("Base").unwrap())
            .unwrap();
        py.run(
            pyo3::ffi::c_str!(
                "class Derived(Base):\n    def own(self, value: int) -> int:\n        return value"
            ),
            Some(&package.dict()),
            None,
        )
        .unwrap();

        let generate = |deduplicate_inherited_members| {
            let config = pyo3_bindgen_engine::Config {
                traverse_submodules: false,
                deduplicate_inherited_members,
                generate_provenance: false,
                ..Default::default()
            };
            let tokens = pyo3_bindgen_engine::Codegen::new(config)
                .modules([package.clone(), submodule.clone()])
                .unwrap()
                .generate()
                .unwrap();
            prettyplease::unparse(&syn::parse2(tokens).unwrap())
        };

        let adaptive = generate(true);
        let flat = generate(false);
        assert_eq!(
            adaptive, flat,
            "a traversal-disabled base cannot participate in deduplication because its \
             trait/newtype is not emitted"
        );
        assert!(
            adaptive.contains("fn m0<'py>") && !adaptive.contains("BaseMethods for Derived"),
            "Derived must retain inherited members on the flat model: {adaptive}"
        );
        modules.del_item("mod_dedup_pkg.sub").unwrap();
        modules.del_item("mod_dedup_pkg").unwrap();
    });
}

#[test]
fn live_keeps_an_isolated_class_on_the_flat_model() {
    // A class with no in-scope base and no in-scope subclass cannot exploit the
    // deduplicated object model, so it stays flat even with deduplication enabled -- and
    // the whole module is byte-identical to the non-deduplicated build (zero overhead).
    let code = indoc::indoc! { r#"
        class Standalone:
            def solo(self): ...
            def only(self): ...
    "# };
    let dedup = generate_bindings(code, "mod_dedup_isolated");
    let flat = generate_bindings_with_config(code, "mod_dedup_isolated", config_without_dedup());

    assert!(
        dedup.contains("pub trait StandaloneMethods {"),
        "an isolated class must stay on the flat model:\n{dedup}"
    );
    assert_eq!(
        dedup, flat,
        "an all-isolated module must be byte-identical with deduplication on or off"
    );
}

#[test]
fn live_dedup_keeps_inherited_static_class_and_constructor_members() {
    // A deduplicated subclass inherits INSTANCE methods through the `*Methods` supertrait,
    // but static methods, class methods and the constructor (`__init__`) lower to INHERENT
    // items that Rust does not inherit across newtypes. They must therefore stay
    // redeclared on the subclass -- dropping them would silently make them uncallable
    // (e.g. a subclass with no own `__init__` would lose its constructor). Regression test
    // for that member-loss bug.
    let instance_methods = documented_methods(20);
    let code = format!(
        "class Base:\n\
        \x20   def __init__(self, x: int): ...\n\
        \x20   @staticmethod\n\
        \x20   def make() -> int: ...\n\
        \x20   @classmethod\n\
        \x20   def from_thing(cls, t: int): ...\n\
        {instance_methods}\
        class Derived(Base):\n\
        \x20   def own(self): ...\n\
        class Derived2(Base):\n\
        \x20   def own2(self): ...\n"
    );
    let dedup = generate_bindings(&code, "mod_dedup_ctor");

    // The subclasses are deduplicated (their instance methods come through the supertrait).
    assert!(
        dedup.contains("pub trait DerivedMethods: "),
        "Derived must be deduplicated for this test to be meaningful:\n{dedup}"
    );
    assert!(
        dedup.contains("BaseMethods for Derived"),
        "Derived must inherit Base's instance methods via the supertrait:\n{dedup}"
    );

    // The inherited constructor / static / class methods must appear on Base AND on both
    // deduplicated subclasses (3 occurrences each) -- they are redeclared, never dropped,
    // because they lower to inherent items Rust does not inherit across newtypes.
    for (name, kind) in [
        ("new", "constructor"),
        ("make", "static method"),
        ("from_thing", "class method"),
    ] {
        let count = dedup.matches(&format!("pub fn {name}<'py>")).count();
        assert_eq!(
            count, 3,
            "inherited {kind} `{name}` must stay on both deduplicated subclasses \
             (Base + Derived + Derived2 = 3 occurrences), found {count}:\n{dedup}"
        );
    }
}

/// `count` no-parameter methods each carrying a large docstring, so documentation dominates
/// their emitted size. A hierarchy sharing them shrinks dramatically with docs on but barely
/// at all with docs off -- the case the cost model must not misjudge into a size increase.
fn big_doc_methods(count: usize) -> String {
    let doc = "x".repeat(2000);
    (0..count)
        .map(|i| {
            format!("    def m{i}(self) -> int:\n        \"\"\"{doc}\"\"\"\n        return {i}\n")
        })
        .collect()
}

#[test]
fn live_dedup_never_grows_a_docs_disabled_build() {
    // Regression for the strict-win violation: `estimated_flat_size` used the RAW docstring
    // length and ignored `generate_docs`, so with documentation disabled the cost model
    // over-credited phantom docstring bytes the flat model never emits and could deduplicate
    // a hierarchy whose real (doc-less) saving does not clear the root-support block --
    // enlarging the output. These big-docstring, no-parameter methods shrink under dedup
    // with docs on, but with docs off dedup must decline rather than grow.
    let methods = big_doc_methods(20);
    let code = format!("class Base:\n{methods}class Sub(Base):\n\x20   def sub_only(self): ...\n");

    // Docs on: the hierarchy still deduplicates and shrinks.
    let dedup_docs = generate_bindings(&code, "mod_bigdoc");
    let flat_docs = generate_bindings_with_config(&code, "mod_bigdoc", config_without_dedup());
    assert!(
        dedup_docs.len() < flat_docs.len(),
        "with docs on, the big-docstring hierarchy must shrink under dedup (dedup {} vs flat {})",
        dedup_docs.len(),
        flat_docs.len()
    );

    // Docs off: the real saving is tiny, so dedup must never enlarge the output.
    let docs_off_dedup = pyo3_bindgen_engine::Config::builder()
        .generate_docs(false)
        .build();
    let docs_off_flat = pyo3_bindgen_engine::Config::builder()
        .generate_docs(false)
        .deduplicate_inherited_members(false)
        .build();
    let dedup = generate_bindings_with_config(&code, "mod_bigdoc", docs_off_dedup);
    let flat = generate_bindings_with_config(&code, "mod_bigdoc", docs_off_flat);
    assert!(
        dedup.len() <= flat.len(),
        "with documentation disabled, deduplication must never enlarge the output \
         (dedup {} vs flat {} bytes)",
        dedup.len(),
        flat.len()
    );
}

#[test]
fn live_dedup_does_not_overcount_whitespace_collapsed_docstrings() {
    // The other half of the strict-win fix (and the originally reproduced case): docstrings
    // dominated by whitespace runs, which `format_docstring` collapses to almost nothing
    // before emission. The cost model must credit the *emitted* (collapsed) size, not the
    // raw docstring length -- else it over-credits the saving and deduplicates a hierarchy
    // whose real emission is tiny, enlarging the output.
    let pad = " ".repeat(4000);
    let methods: String = (0..4)
        .map(|i| format!("    def m{i}(self):\n        \"\"\"A{pad}B\"\"\"\n        ...\n"))
        .collect();
    let code = format!("class Base:\n{methods}class Sub(Base):\n\x20   def sub_only(self): ...\n");
    let dedup = generate_bindings(&code, "mod_ws_doc");
    let flat = generate_bindings_with_config(&code, "mod_ws_doc", config_without_dedup());
    assert!(
        dedup.len() <= flat.len(),
        "whitespace-collapsed docstrings must not be over-counted into a size increase \
         (dedup {} vs flat {})",
        dedup.len(),
        flat.len()
    );
}

#[test]
fn live_dedup_declines_a_marginal_hierarchy_and_stays_flat() {
    // An in-scope hierarchy whose members are trivial (no docstrings, no parameters) saves
    // too little to clear the one-time root-support block, so the strict-win gate must
    // decline to deduplicate it, leaving the output byte-identical to the non-deduplicated
    // build. Guards the global `total_beneficial > ROOT_SUPPORT_COST` threshold: deleting it
    // would let this marginal hierarchy grow by the root block.
    let code = indoc::indoc! { r#"
        class Base:
            def a(self): ...
            def b(self): ...
        class Sub(Base):
            def c(self): ...
    "# };
    let dedup = generate_bindings(code, "mod_marginal");
    let flat = generate_bindings_with_config(code, "mod_marginal", config_without_dedup());
    assert_eq!(
        dedup, flat,
        "a marginal (trivial-member) hierarchy must decline dedup and stay byte-identical"
    );
}

#[test]
fn dedup_model_hoists_wrapper_boilerplate_to_root_trait_defaults() {
    // (D3) `bind`/`as_any`/`clone_ref` have bodies identical modulo `Self`. On the
    // deduplicated model every newtype already implements the root/marker traits, so those
    // three are emitted ONCE as `#[inline]` default methods there instead of on each
    // newtype's inherent `impl`. `from_py_any` names the concrete class and its runtime
    // import, and `into_py_any` collides with pyo3's `IntoPyObjectExt` blanket impl unless
    // it stays inherent (E0034), so both stay per class. A flat class implements neither
    // trait and therefore keeps all its inherent copies.
    let methods = documented_methods(20);
    let code = format!("class Base:\n{methods}class Sub(Base):\n\x20   def sub_only(self): ...\n");
    let dedup = generate_bindings(&code, "mod_d3_hoist");
    let flat = generate_bindings_with_config(&code, "mod_d3_hoist", config_without_dedup());

    assert!(
        dedup.contains("pub trait PyObjectBase"),
        "the hierarchy must deduplicate and emit root support:\n{dedup}"
    );
    // The shared declaration-plus-body lives on the traits, exactly once each.
    for declaration in [
        "fn bind<'py>(\n        &self,\n        py: ::pyo3::marker::Python<'py>,\n    ) -> &::pyo3::Bound<'py, ::pyo3::types::PyAny> {\n        self.__pyo3_obj().bind(py)\n    }",
        "fn as_any<'py>(\n        &self,\n        py: ::pyo3::marker::Python<'py>,\n    ) -> &::pyo3::Bound<'py, ::pyo3::types::PyAny> {\n        self.bind(py)\n    }",
        "fn clone_ref(&self, py: ::pyo3::marker::Python<'_>) -> Self {\n        Self::__from_obj(self.__pyo3_obj().clone_ref(py))\n    }",
    ] {
        assert_eq!(
            dedup.matches(declaration).count(),
            1,
            "the hoisted default method must be emitted exactly once:\n{declaration}\n\nGenerated:\n{dedup}"
        );
    }
    // `PyTypeMarker` gained the root supertrait so `clone_ref` can reach the handle while
    // staying `Sized`.
    assert!(
        dedup.contains("pub trait PyTypeMarker: ::core::marker::Sized + PyObjectBase {"),
        "the marker trait must require the root trait:\n{dedup}"
    );
    // Nothing per class anymore, except `from_py_any` and `into_py_any`.
    for hoisted in [
        "pub fn bind<'py>(",
        "pub fn as_any<'py>(",
        "pub fn clone_ref(&self, py: ::pyo3::marker::Python<'_>) -> Self",
    ] {
        assert!(
            !dedup.contains(hoisted),
            "the deduplicated model must not re-emit '{hoisted}' on a newtype:\n{dedup}"
        );
    }
    // `into_py_any` must NOT be hoisted: pyo3's `IntoPyObjectExt` blanket impl supplies the
    // same name, and an inherent method is what wins resolution against it (E0034 guard).
    assert!(
        !dedup.contains("fn into_py_any(self) -> ::pyo3::Py<::pyo3::types::PyAny> {\n        self.__into_obj()\n    }"),
        "`into_py_any` must not be a trait default method (E0034 vs pyo3's IntoPyObjectExt):\n{dedup}"
    );
    assert_eq!(
        dedup
            .matches("pub fn into_py_any(self) -> ::pyo3::Py<::pyo3::types::PyAny>")
            .count(),
        2,
        "`into_py_any` stays a per-class inherent method (Base + Sub):\n{dedup}"
    );
    assert_eq!(
        dedup.matches("pub fn from_py_any<'py>(").count(),
        2,
        "`from_py_any` stays per class (Base + Sub):\n{dedup}"
    );

    // The flat model is untouched: it has no root trait to carry the defaults.
    assert!(
        !flat.contains("trait PyObjectBase"),
        "the flat model must not emit root support:\n{flat}"
    );
    assert_eq!(
        flat.matches("pub fn bind<'py>(").count(),
        2,
        "a flat class keeps its inherent `bind`:\n{flat}"
    );
    assert_eq!(
        flat.matches("pub fn into_py_any(self) -> ::pyo3::Py<::pyo3::types::PyAny>")
            .count(),
        2,
        "a flat class keeps its inherent `into_py_any`:\n{flat}"
    );
}

#[test]
fn dedup_wrapper_api_compiles_and_runs_with_into_py_object_ext_in_scope() {
    // Run oracle for the D3 hoist on a DEDUPLICATED hierarchy: a consumer brings the shared
    // root/marker traits into scope AND `pyo3::conversion::IntoPyObjectExt` (whose blanket
    // `impl<T: IntoPyObject> IntoPyObjectExt for T` supplies a same-named `into_py_any`),
    // then calls all four wrapper-boilerplate methods. This locks the fix for the E0034
    // ambiguity: `into_py_any` must stay INHERENT on the newtype, because an inherent method
    // is what takes resolution precedence over the blanket-impl trait method. If it were
    // hoisted back onto `PyTypeMarker` this test fails to compile with
    // `error[E0034]: multiple applicable items in scope`.
    let methods = documented_methods(20);
    let code = format!(
        "class Base:\n\
        \x20   def __init__(self, value: int):\n\
        \x20       self._value = value\n\
        {methods}\
        class Sub(Base):\n\
        \x20   def __init__(self, value: int):\n\
        \x20       super().__init__(value)\n\
        \x20   def sub_only(self) -> int:\n\
        \x20       return self._value\n"
    );
    let generated_code = generate_bindings(&code, "mod_d3_oracle");
    assert!(
        generated_code.contains("pub trait PyObjectBase"),
        "the oracle hierarchy must actually deduplicate:\n{generated_code}"
    );

    let mut working_directory = bounded_consumer::OwnedTempDir::allocate("d3_oracle")
        .expect("generated D3 consumer directory must be allocated exclusively");
    let crate_dir = working_directory.path().to_owned();
    std::fs::create_dir(crate_dir.join("src")).unwrap();

    let pyo3_version = env!("PYO3_BINDGEN_PYO3_VERSION");
    std::fs::write(
        crate_dir.join("Cargo.toml"),
        format!(
            r#"[package]
name = "pyo3_bindgen_d3_oracle"
version = "0.0.0"
edition = "2021"
publish = false

[dependencies]
pyo3 = {{ version = "={pyo3_version}", features = ["auto-initialize"] }}
"#
        ),
    )
    .unwrap();

    std::fs::write(
        crate_dir.join("src/lib.rs"),
        format!(
            r#"{generated_code}

#[cfg(test)]
mod tests {{
    use super::mod_d3_oracle::{{self, Base, Sub}};
    // The shared root/marker traits live at the generated root, not inside the module.
    use super::{{PyObjectBase as _, PyTypeMarker as _}};
    // The blanket-impl trait that made the hoisted `into_py_any` ambiguous.
    use pyo3::conversion::IntoPyObjectExt;
    use pyo3::types::PyAnyMethods;

    #[test]
    fn hoisted_wrapper_boilerplate_is_callable() -> pyo3::PyResult<()> {{
        pyo3::Python::attach(|py| {{
            mod_d3_oracle::pyo3_embed_python_source_code(py)?;

            let sub = Sub::new(py, 7)?;
            // `PyObjectBase::bind` / `PyObjectBase::as_any` (hoisted trait defaults).
            assert_eq!(
                sub.bind(py)
                    .getattr(pyo3::intern!(py, "_value"))?
                    .extract::<i64>()?,
                7
            );
            let class_name: String = sub
                .as_any(py)
                .getattr(pyo3::intern!(py, "__class__"))?
                .getattr(pyo3::intern!(py, "__name__"))?
                .extract()?;
            assert_eq!(class_name, "Sub");

            // `PyTypeMarker::clone_ref` (hoisted trait default): shares the handle.
            let shared = sub.clone_ref(py);
            sub.bind(py).setattr(pyo3::intern!(py, "_value"), 11)?;
            assert_eq!(
                shared
                    .bind(py)
                    .getattr(pyo3::intern!(py, "_value"))?
                    .extract::<i64>()?,
                11
            );

            // Inherent `into_py_any`: unambiguous even with `IntoPyObjectExt` imported.
            let raw = shared.into_py_any();
            assert_eq!(
                raw.bind(py)
                    .getattr(pyo3::intern!(py, "_value"))?
                    .extract::<i64>()?,
                11
            );

            // Same four calls on the base newtype of the deduplicated hierarchy.
            let base = Base::new(py, 3)?;
            assert!(base.as_any(py).hasattr(pyo3::intern!(py, "m0"))?);
            let base_raw = base.clone_ref(py).into_py_any();
            assert_eq!(
                base_raw
                    .bind(py)
                    .getattr(pyo3::intern!(py, "_value"))?
                    .extract::<i64>()?,
                3
            );

            // `IntoPyObjectExt`'s own `into_py_any` still resolves for non-wrapper types,
            // proving the import is live (and not silently unused).
            let boxed = 5_i64.into_py_any(py)?;
            assert_eq!(boxed.bind(py).extract::<i64>()?, 5);
            Ok(())
        }})
    }}
}}
"#
        ),
    )
    .unwrap();

    let mut command =
        std::process::Command::new(std::env::var("CARGO").unwrap_or("cargo".to_owned()));
    command
        .arg("test")
        .arg("--quiet")
        .current_dir(&crate_dir)
        .env("CARGO_TARGET_DIR", crate_dir.join("target"));
    let output = working_directory
        .run(&mut command, bounded_consumer::CARGO_COMMAND_TIMEOUT)
        .unwrap_or_else(|error| {
            panic!(
                "bounded D3 dedup wrapper-API check failed: {error}\nstdout:\n{}\nstderr:\n{}",
                error.stdout(),
                error.stderr()
            )
        });

    assert!(
        output.status.success(),
        "D3 dedup wrapper-API oracle failed\nstatus: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        output.stdout,
        output.stderr
    );
}

#[test]
fn live_dedup_emits_no_root_support_when_classes_disabled() {
    // With class generation disabled the deduplication pass must not run: it would mark
    // participants and emit the (now dead) root-support block, growing the output over the
    // classes-disabled baseline.
    let methods = documented_methods(20);
    let code = format!("class Base:\n{methods}class Sub(Base):\n\x20   def sub_only(self): ...\n");
    let cfg = pyo3_bindgen_engine::Config::builder()
        .generate_classes(false)
        .build();
    let generated = generate_bindings_with_config(&code, "mod_no_classes", cfg);
    assert!(
        !generated.contains("trait PyObjectBase"),
        "a classes-disabled build must not emit the root-support trait:\n{generated}"
    );
    assert!(
        !generated.contains("pub struct Base"),
        "a classes-disabled build must not emit any class:\n{generated}"
    );
}

#[test]
fn live_dedup_falls_back_for_a_module_named_like_the_root_trait() {
    // A top-level module named `PyObjectBase` collides with the synthetic root trait emitted
    // for a deduplicating hierarchy. Adaptive live generation can avoid that optional
    // optimization, so it must fall back to the valid flat model rather than emitting a
    // collision or rejecting an input the historical model supports.
    let methods = documented_methods(20);
    let code = format!("class Base:\n{methods}class Sub(Base):\n\x20   def sub_only(self): ...\n");
    let generated = pyo3_bindgen_engine::Codegen::new(pyo3_bindgen_engine::Config::default())
        .module_from_str(&code, "PyObjectBase")
        .unwrap()
        .generate()
        .expect("a dedup-only root-support collision must fall back to the flat model");
    let generated = prettyplease::unparse(&syn::parse2(generated).unwrap());
    assert!(
        generated.contains("pub mod PyObjectBase") && !generated.contains("trait PyObjectBase"),
        "fallback must emit only the user module, without colliding root support:\n{generated}"
    );
}

#[test]
fn live_dedup_hierarchy_compiles_and_runs() {
    // The live deduplicated object model (member drops derived from `__mro__`) is otherwise
    // only string-matched. Compile and RUN a Base -> Mid -> Leaf hierarchy and call an
    // inherited instance method on a `Leaf`, proving the dropped members stay reachable
    // through the `*Methods` supertraits (a wrong drop would be an E0599 / missing method).
    let padding = documented_methods(20);
    let code_py = format!(
        "class Base:\n\
        \x20   def __init__(self, start: int):\n\
        \x20       self._n = start\n\
        \x20   def base_value(self) -> int:\n\
        \x20       return self._n\n\
        {padding}\
        class _Middle(Base):\n\
        \x20   def mid_tag(self) -> str:\n\
        \x20       return \"mid\"\n\
        class Leaf(_Middle):\n\
        \x20   def leaf_tag(self) -> str:\n\
        \x20       return \"leaf\"\n"
    );
    let generated_code = generate_bindings(&code_py, "mod_dedup_rt");
    // Sanity: the hierarchy actually deduplicated (else it would not exercise inheritance).
    assert!(
        generated_code.contains("BaseMethods for Leaf"),
        "Leaf must inherit BaseMethods for this test to be meaningful:\n{generated_code}"
    );

    let mut working_directory = bounded_consumer::OwnedTempDir::allocate("dedup_runtime")
        .expect("generated dedup runtime consumer directory must be allocated exclusively");
    let crate_dir = working_directory.path().to_owned();
    std::fs::create_dir(crate_dir.join("src")).unwrap();
    let pyo3_version = env!("PYO3_BINDGEN_PYO3_VERSION");
    std::fs::write(
        crate_dir.join("Cargo.toml"),
        format!(
            r#"[package]
name = "pyo3_bindgen_dedup_rt_smoke"
version = "0.0.0"
edition = "2021"
publish = false

[dependencies]
pyo3 = {{ version = "={pyo3_version}", features = ["auto-initialize"] }}
"#
        ),
    )
    .unwrap();
    std::fs::write(
        crate_dir.join("src/lib.rs"),
        format!(
            r#"{generated_code}

#[cfg(test)]
mod tests {{
    use super::mod_dedup_rt::{{self, BaseMethods, Leaf, LeafMethods}};

    fn inherited_through_leaf_trait<T: LeafMethods>(
        value: &T,
        py: pyo3::Python<'_>,
    ) -> pyo3::PyResult<i64> {{
        value.base_value(py)
    }}

    #[test]
    fn deduplicated_hierarchy_runs() -> pyo3::PyResult<()> {{
        pyo3::Python::attach(|py| {{
            mod_dedup_rt::pyo3_embed_python_source_code(py)?;
            let leaf = Leaf::new(py, 10)?;
            // Inherited from Base (dropped from Leaf, reached via the BaseMethods supertrait).
            assert_eq!(leaf.base_value(py)?, 10);
            // Generic LeafMethods bounds must inherit BaseMethods even though the direct
            // Python base is filtered from generated bindings.
            assert_eq!(inherited_through_leaf_trait(&leaf, py)?, 10);
            // Owned by the omitted intermediate, so retained directly on Leaf.
            assert_eq!(leaf.mid_tag(py)?, "mid");
            // Leaf's own method (inherent forwarder).
            assert_eq!(leaf.leaf_tag(py)?, "leaf");
            Ok(())
        }})
    }}
}}
"#
        ),
    )
    .unwrap();

    let mut command =
        std::process::Command::new(std::env::var("CARGO").unwrap_or("cargo".to_owned()));
    command
        .arg("test")
        .arg("--quiet")
        .current_dir(&crate_dir)
        .env("CARGO_TARGET_DIR", crate_dir.join("target"));
    let output = working_directory
        .run(&mut command, bounded_consumer::CARGO_COMMAND_TIMEOUT)
        .unwrap_or_else(|error| {
            panic!(
                "bounded deduplicated hierarchy runtime check failed: {error}\nstdout:\n{}\nstderr:\n{}",
                error.stdout(),
                error.stderr()
            )
        });
    assert!(
        output.status.success(),
        "deduplicated hierarchy runtime smoke failed\nstatus: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        output.stdout,
        output.stderr
    );
}

/// Python source with enough shape variety (classes, inheritance, enums, defaults,
/// docstrings with escapes and non-ASCII) that its token stream exercises every rendering
/// path the streamed hash must reproduce.
const NONTRIVIAL_SOURCE: &str = indoc::indoc! { r#"
    from __future__ import annotations
    from enum import Enum

    class Color(Enum):
        """A palette with "quotes", a backslash \\ and non-ASCII: αβγ."""
        RED = 1
        GREEN = 2

    class Base:
        def describe(self, verbose: bool = False) -> str: ...

    class Derived(Base):
        tag: str = "derived"
        def scale(self, factor: float = 1.5, *args: int, **kwargs: str) -> list[float]: ...

    def free_function(values: list[int], lookup: dict[str, float]) -> tuple[int, ...]: ...
"# };

/// The `token_stream` output metadata is produced by streaming the token stream's
/// `Display` rendering through the hasher rather than materializing it. Pin that path to
/// the semantics it replaced -- `stable_hash(tokens.to_string().as_bytes())` paired with
/// `tokens.to_string().len()` -- so a future `proc_macro2` whose rendering (or whose
/// `Formatter` usage) drifts cannot silently change reported hashes.
#[test]
fn token_stream_output_hash_matches_materialized_rendering() {
    let generated = pyo3_bindgen_engine::Codegen::default()
        .module_from_str(NONTRIVIAL_SOURCE, "mod_bindgen_streamed_hash")
        .unwrap()
        .generate_with_report()
        .unwrap();

    let materialized = generated.tokens.to_string();
    // Guard against the oracle degenerating into a trivial stream.
    assert!(
        materialized.len() > 1000,
        "expected a nontrivial token stream, got {} bytes",
        materialized.len()
    );

    assert_eq!(generated.report.outputs.len(), 1);
    let output = &generated.report.outputs[0];
    assert_eq!(output.kind, "token_stream");
    assert_eq!(
        output.hash,
        pyo3_bindgen_engine::stable_hash(materialized.as_bytes()),
        "streamed hash must equal the hash of the materialized rendering"
    );
    assert_eq!(
        output.bytes,
        materialized.len(),
        "reported byte count must equal the materialized rendering's length"
    );
}

/// `generate` skips the token-stream output metadata that `generate_with_report` collects.
/// That knob is report-only, so it must not reach emission: both entry points must produce
/// byte-identical bindings for the same input.
#[test]
fn generate_matches_generate_with_report_tokens() {
    let without_report = pyo3_bindgen_engine::Codegen::default()
        .module_from_str(NONTRIVIAL_SOURCE, "mod_bindgen_report_knob")
        .unwrap()
        .generate()
        .unwrap();
    let with_report = pyo3_bindgen_engine::Codegen::default()
        .module_from_str(NONTRIVIAL_SOURCE, "mod_bindgen_report_knob")
        .unwrap()
        .generate_with_report()
        .unwrap();

    assert_eq!(without_report.to_string(), with_report.tokens.to_string());
    // The report-only difference the knob is allowed to make.
    assert_eq!(with_report.report.outputs.len(), 1);
}
