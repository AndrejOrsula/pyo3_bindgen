#[test]
fn public_stdlib_replacements_do_not_change_live_api_classification() {
    let generated = pyo3_bindgen_engine::Codegen::default()
        .module_from_str(
            indoc::indoc! { r#"
                import dataclasses
                import enum
                import types
                import typing

                _Enum = enum.Enum
                _ClassVar = typing.ClassVar
                _InitVar = dataclasses.InitVar
                _KW_ONLY = getattr(dataclasses, "KW_ONLY", None)

                class Shade(_Enum):
                    LIGHT = 1
                    DARK = 2

                _Alias = eval("list[int]") if hasattr(types, "GenericAlias") else typing.List[int]
                _Maybe = eval("int | None") if hasattr(types, "UnionType") else typing.Optional[int]
                Alias = _Alias
                Maybe = _Maybe

                def typed(values, maybe):
                    return values

                typed.__annotations__ = {
                    "values": _Alias,
                    "maybe": _Maybe,
                    "return": _Alias,
                }

                @dataclasses.dataclass
                class Fields:
                    shared: _ClassVar[int] = 1
                    incoming: _InitVar[int] = 2
                    value: int = 3

                if _KW_ONLY is not None:
                    @dataclasses.dataclass
                    class KeywordFields:
                        marker: _KW_ONLY
                        keyword_value: int = 4

                @dataclasses.dataclass(frozen=True)
                class FrozenBase:
                    frozen_value: int

                class FrozenChild(FrozenBase):
                    pass

                # Mutate the public spellings only after every genuine object exists.
                enum.Enum = object
                enum.Flag = object
                enum.IntFlag = object
                enum.IntEnum = object
                if hasattr(enum, "StrEnum"):
                    enum.StrEnum = object

                typing.ClassVar = object()
                if hasattr(typing, "_GenericAlias"):
                    typing._GenericAlias = object

                dataclasses.InitVar = object
                if hasattr(dataclasses, "KW_ONLY"):
                    dataclasses.KW_ONLY = object
                dataclasses.fields = lambda owner: ()

                if hasattr(types, "GenericAlias"):
                    types.GenericAlias = int
                if hasattr(types, "UnionType"):
                    types.UnionType = int
            "# },
            "mod_runtime_sentinel_safety",
        )
        .expect("public stdlib replacements must not abort live introspection")
        .generate_with_report()
        .expect("snapshotted identities must preserve the authored API");

    let module = &generated.report.modules[0];
    let shade = module
        .enums
        .iter()
        .find(|enumeration| enumeration.name.ends_with(".Shade"))
        .expect("the genuine enum must remain an enum mirror");
    assert_eq!(shade.kind, "Enum");
    assert_eq!(
        shade
            .variants
            .iter()
            .map(|variant| variant.python_name.as_str())
            .collect::<Vec<_>>(),
        ["LIGHT", "DARK"],
    );

    let source = prettyplease::unparse(
        &syn::parse2(generated.tokens).expect("generated bindings must remain valid Rust"),
    );
    let fields = source
        .split("pub trait FieldsMethods")
        .nth(1)
        .and_then(|rest| rest.split("impl FieldsMethods").next())
        .unwrap_or_else(|| panic!("missing FieldsMethods trait:\n{source}"));
    assert!(
        fields.contains("fn value<'py>(") && fields.contains("fn set_value<'py>("),
        "ordinary dataclass fields must retain accessors:\n{fields}",
    );
    for pseudo in ["shared", "incoming", "marker"] {
        assert!(
            !fields.contains(&format!("fn {pseudo}<'py>("))
                && !fields.contains(&format!("fn set_{pseudo}<'py>(")),
            "pseudo-field `{pseudo}` leaked into the instance API:\n{fields}",
        );
    }

    if let Some(keyword_fields) = source
        .split("pub trait KeywordFieldsMethods")
        .nth(1)
        .and_then(|rest| rest.split("impl KeywordFieldsMethods").next())
    {
        assert!(
            keyword_fields.contains("fn keyword_value<'py>(")
                && !keyword_fields.contains("fn marker<'py>(")
                && !keyword_fields.contains("fn set_marker<'py>("),
            "KW_ONLY must stay metadata while the real field remains:\n{keyword_fields}",
        );
    }

    let frozen_child = source
        .split("pub trait FrozenChildMethods")
        .nth(1)
        .and_then(|rest| rest.split("impl FrozenChildMethods").next())
        .unwrap_or_else(|| panic!("missing FrozenChildMethods trait:\n{source}"));
    assert!(
        !frozen_child.contains("fn set_frozen_value<'py>("),
        "a frozen-base field must not gain an invalid subclass setter:\n{frozen_child}",
    );

    let typed = source
        .split("pub fn typed<'py>(")
        .nth(1)
        .and_then(|rest| rest.split('{').next())
        .unwrap_or_else(|| panic!("missing typed function:\n{source}"));
    assert!(
        typed.contains("p_values: &[i64]")
            && typed.contains("p_maybe: ::std::option::Option<i64>")
            && typed.contains("PyResult<Vec<i64>>"),
        "GenericAlias/UnionType replacements corrupted the function signature:\n{typed}",
    );
}

#[test]
fn operation_added_stdlib_globals_cannot_hijack_snapshotted_helpers() {
    let generated = pyo3_bindgen_engine::Codegen::default()
        .module_from_str(
            indoc::indoc! { r#"
                import inspect

                # `inspect` normally resolves this name from builtins. Adding a module
                # global shadows that lookup inside its otherwise-canonical functions.
                inspect.isinstance = lambda *args: False

                def typed(value: int) -> int:
                    return value
            "# },
            "mod_added_inspect_global",
        )
        .expect("an added stdlib global must not corrupt live introspection")
        .generate_with_report()
        .expect("the authored callable must still generate");

    let leaked = pyo3::Python::attach(|py| {
        use pyo3::types::PyAnyMethods;

        let inspect = py.import("inspect").unwrap();
        let leaked = inspect.hasattr("isinstance").unwrap();
        // Keep this regression hermetic even when run against a broken implementation.
        if leaked {
            inspect.delattr("isinstance").unwrap();
        }
        leaked
    });
    assert!(
        !leaked,
        "an operation-added inspect global survived live introspection",
    );

    let source = prettyplease::unparse(
        &syn::parse2(generated.tokens).expect("generated bindings must remain valid Rust"),
    );
    let typed = source
        .split("pub fn typed<'py>(")
        .nth(1)
        .and_then(|rest| rest.split('{').next())
        .unwrap_or_else(|| panic!("typed was misclassified as a property:\n{source}"));
    assert!(
        typed.contains("p_value: i64") && typed.contains("PyResult<i64>"),
        "an added inspect global corrupted the callable signature:\n{typed}",
    );
}
