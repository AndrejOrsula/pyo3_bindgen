use pyo3::types::PyAnyMethods;
use pyo3_bindgen_engine::{
    BindingSource, Codegen, Config, LiveModules, ModuleIr, PyBindgenResult, SchemaIr,
};
use std::{
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

fn empty_module(name: &str) -> ModuleIr {
    ModuleIr {
        name: name.to_owned(),
        docstring: None,
        is_package: true,
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
fn built_in_source_adapters_share_one_codegen_seam() {
    let from_ir = Codegen::default()
        .source(SchemaIr {
            modules: vec![empty_module("source_ir")],
        })
        .expect("SchemaIr must implement BindingSource")
        .generate()
        .expect("external source must generate");
    assert!(from_ir.to_string().contains("source_ir"));

    Codegen::new(Config {
        generate_dependencies: false,
        traverse_submodules: false,
        ..Config::default()
    })
    .source(LiveModules::new(["math"]))
    .expect("LiveModules must implement BindingSource")
    .generate()
    .expect("live source must generate");
}

#[test]
fn downstream_source_adapter_can_delegate_without_private_domain_access() {
    struct AuthoredSchema(SchemaIr);

    impl BindingSource for AuthoredSchema {
        fn load(self) -> PyBindgenResult<pyo3_bindgen_engine::SchemaSource> {
            Ok(pyo3_bindgen_engine::SchemaSource::Ir(self.0))
        }
    }

    Codegen::default()
        .source(AuthoredSchema(SchemaIr {
            modules: vec![empty_module("custom_source")],
        }))
        .expect("a downstream adapter must cross the public source seam")
        .generate()
        .expect("custom source must generate");
}

#[test]
fn conflicting_custom_source_is_rejected_before_load_after_live_registration() {
    struct SideEffectingSource {
        loads: Arc<AtomicUsize>,
    }

    impl BindingSource for SideEffectingSource {
        fn load(self) -> PyBindgenResult<pyo3_bindgen_engine::SchemaSource> {
            self.loads.fetch_add(1, Ordering::SeqCst);
            Ok(pyo3_bindgen_engine::SchemaSource::Ir(SchemaIr {
                modules: vec![empty_module("must_not_load")],
            }))
        }
    }

    let loads = Arc::new(AtomicUsize::new(0));
    let codegen = Codegen::default()
        .module_from_str("value = 1", "live_module")
        .expect("live module should parse");

    let error = codegen
        .source(SideEffectingSource {
            loads: Arc::clone(&loads),
        })
        .expect_err("a custom source cannot run after live mode is selected");

    assert!(error.to_string().contains("cannot mix"));
    assert_eq!(loads.load(Ordering::SeqCst), 0, "load must not execute");
}

#[test]
fn conflicting_custom_source_is_rejected_before_load_after_ir_registration() {
    struct SideEffectingSource {
        loads: Arc<AtomicUsize>,
    }

    impl BindingSource for SideEffectingSource {
        fn load(self) -> PyBindgenResult<pyo3_bindgen_engine::SchemaSource> {
            self.loads.fetch_add(1, Ordering::SeqCst);
            Ok(pyo3_bindgen_engine::SchemaSource::Live(vec![
                "must_not_load".to_owned(),
            ]))
        }
    }

    let loads = Arc::new(AtomicUsize::new(0));
    let codegen = Codegen::default()
        .module_from_ir(empty_module("ir_module"))
        .expect("external IR should parse");

    let error = codegen
        .source(SideEffectingSource {
            loads: Arc::clone(&loads),
        })
        .expect_err("a custom source cannot run after external IR mode is selected");

    assert!(error.to_string().contains("cannot mix"));
    assert_eq!(loads.load(Ordering::SeqCst), 0, "load must not execute");
}

#[test]
fn codegen_rejects_external_ir_after_live_introspection() {
    let codegen = Codegen::default()
        .module_from_str("value = 1", "live_module")
        .expect("live module should parse");

    let error = codegen
        .module_from_ir(empty_module("ir_module"))
        .expect_err("mixing live modules and external IR must be rejected");

    assert!(
        error.to_string().contains("cannot mix"),
        "unexpected error: {error}",
    );
}

#[test]
fn codegen_rejects_live_introspection_after_external_ir() {
    let codegen = Codegen::default()
        .module_from_ir(empty_module("ir_module"))
        .expect("external IR should parse");

    let error = codegen
        .module_from_str("raise RuntimeError('must not execute')", "live_module")
        .expect_err("mixing external IR and live modules must be rejected");

    assert!(
        error.to_string().contains("cannot mix"),
        "unexpected error: {error}",
    );
}

#[test]
fn repeated_external_ir_calls_share_one_terminal_validation_scope() {
    let codegen = Codegen::default()
        .module_from_ir(empty_module("pkg"))
        .expect("first external IR module should parse");

    let error = codegen
        .module_from_ir(empty_module("pkg"))
        .expect("external IR registration must defer duplicate validation")
        .generate()
        .expect_err("duplicate modules across builder calls must be rejected at generation");

    assert!(
        error.to_string().contains("duplicate module path 'pkg'"),
        "unexpected error: {error}",
    );
}

#[test]
fn repeated_live_source_name_is_rejected_before_execution() {
    let codegen = Codegen::default()
        .module_from_str("value = 1", "duplicate_live")
        .expect("first live module should parse");

    let error = codegen
        .module_from_str(
            "raise RuntimeError('duplicate source must not execute')",
            "duplicate_live",
        )
        .expect_err("duplicate live modules must be rejected");

    assert!(
        error
            .to_string()
            .contains("duplicate live module path 'duplicate_live'"),
        "unexpected error: {error}",
    );
}

#[test]
fn imported_python_module_alias_uses_its_resolved_name() {
    Codegen::default()
        .module_name("os.path")
        .expect("Python module aliases such as os.path must remain supported");
}

#[test]
fn embedded_source_nul_is_an_error_instead_of_a_panic() {
    let result = catch_unwind(AssertUnwindSafe(|| {
        Codegen::default().module_from_str("value = '\0'", "nul_source")
    }));

    let error = result
        .expect("interior NUL must not panic")
        .expect_err("interior NUL must be rejected");
    assert!(error.to_string().contains("source contains a NUL byte"));
}

#[test]
fn embedded_module_name_nul_is_an_error_instead_of_a_panic() {
    let result = catch_unwind(AssertUnwindSafe(|| {
        Codegen::default().module_from_str("value = 1", "nul\0name")
    }));

    let error = result
        .expect("interior NUL must not panic")
        .expect_err("interior NUL must be rejected");
    assert!(error
        .to_string()
        .contains("module name contains a NUL byte"));
}

#[test]
fn malformed_live_module_name_is_an_error_instead_of_a_panic() {
    pyo3::Python::initialize();
    let result = catch_unwind(AssertUnwindSafe(|| {
        pyo3::Python::attach(|py| {
            Codegen::prime_live_runtime(py).unwrap();
            let module = pyo3::types::PyModule::new(py, "malformed_name").unwrap();
            module.setattr("__name__", py.None()).unwrap();
            Codegen::default().module(&module)
        })
    }));

    let error = result
        .expect("malformed __name__ must not panic")
        .expect_err("malformed __name__ must be rejected");
    assert!(matches!(
        error,
        pyo3_bindgen_engine::PyBindgenError::PyError(_)
    ));
}

#[test]
fn missing_parent_package_is_a_generation_error_instead_of_a_panic() {
    pyo3::Python::initialize();
    let codegen = pyo3::Python::attach(|py| {
        Codegen::prime_live_runtime(py).unwrap();
        let module = pyo3::types::PyModule::new(py, "pyo3_bindgen_missing_parent.child").unwrap();
        Codegen::default().module(&module).unwrap()
    });

    let result = catch_unwind(AssertUnwindSafe(|| codegen.generate()));

    let error = result
        .expect("missing parent import must not panic")
        .expect_err("missing parent import must fail generation");
    assert!(matches!(
        error,
        pyo3_bindgen_engine::PyBindgenError::PyError(_)
    ));
}

#[test]
fn failing_module_dir_is_an_error_instead_of_a_panic() {
    pyo3::Python::initialize();
    let result = catch_unwind(AssertUnwindSafe(|| {
        pyo3::Python::attach(|py| {
            Codegen::prime_live_runtime(py).unwrap();
            let source = std::ffi::CString::new(indoc::indoc! {r#"
                import types
                class BrokenModule(types.ModuleType):
                    def __dir__(self):
                        raise RuntimeError('dir failed')
                broken = BrokenModule('broken_dir')
            "#})
            .unwrap();
            let factory = pyo3::types::PyModule::from_code(
                py,
                source.as_c_str(),
                pyo3::ffi::c_str!("broken_dir_factory.py"),
                pyo3::ffi::c_str!("broken_dir_factory"),
            )
            .unwrap();
            let module = factory
                .getattr("broken")
                .unwrap()
                .cast_into::<pyo3::types::PyModule>()
                .unwrap();
            Codegen::default().module(&module)
        })
    }));

    let error = result
        .expect("failing __dir__ must not panic")
        .expect_err("failing __dir__ must reject introspection");
    assert!(matches!(
        error,
        pyo3_bindgen_engine::PyBindgenError::PyError(_)
    ));
}

#[test]
fn empty_name_from_module_dir_is_an_error_instead_of_a_panic() {
    pyo3::Python::initialize();
    let result = catch_unwind(AssertUnwindSafe(|| {
        pyo3::Python::attach(|py| {
            Codegen::prime_live_runtime(py).unwrap();
            let source = std::ffi::CString::new(indoc::indoc! {r#"
                import types
                class BrokenModule(types.ModuleType):
                    def __dir__(self):
                        return [""]
                broken = BrokenModule('empty_dir_name')
            "#})
            .unwrap();
            let factory = pyo3::types::PyModule::from_code(
                py,
                source.as_c_str(),
                pyo3::ffi::c_str!("empty_dir_name_factory.py"),
                pyo3::ffi::c_str!("empty_dir_name_factory"),
            )
            .unwrap();
            let module = factory
                .getattr("broken")
                .unwrap()
                .cast_into::<pyo3::types::PyModule>()
                .unwrap();
            Codegen::default().module(&module)
        })
    }));

    let error = result
        .expect("empty __dir__ name must not panic")
        .expect_err("empty __dir__ name must reject introspection");
    assert!(
        error.to_string().contains("empty attribute name"),
        "unexpected error: {error}",
    );
}

#[test]
fn empty_name_from_module_all_is_an_error_instead_of_a_panic() {
    let result = catch_unwind(AssertUnwindSafe(|| {
        Codegen::default().module_from_str("__all__ = ['']", "empty_all_name")
    }));

    let error = result
        .expect("empty __all__ name must not panic")
        .expect_err("empty __all__ name must reject introspection");
    assert!(
        error.to_string().contains("__all__") && error.to_string().contains("empty attribute name"),
        "unexpected error: {error}",
    );
}

/// Prelude re-exports are emitted in a canonical (sorted) order, not in `__all__`'s
/// declaration order. Upstream modules such as numpy build `__all__` from a `set`, whose
/// iteration order is randomized per interpreter process (`PYTHONHASHSEED`); echoing it
/// verbatim produced non-reproducible bindings and an unstable input-hash fingerprint that
/// defeated `--check`. Here `__all__` is a strict subset of the public functions (so a
/// prelude is actually emitted) declared in reverse-alphabetical order; the emitted
/// re-export group must nonetheless be sorted.
#[test]
fn prelude_reexport_order_is_canonical_not_all_declaration_order() {
    let source = "def zeta():\n    pass\n\
                  def alpha():\n    pass\n\
                  def mu():\n    pass\n\
                  def sigma():\n    pass\n\
                  __all__ = ['zeta', 'mu', 'alpha']\n";
    let generated = Codegen::default()
        .module_from_str(source, "prelude_order")
        .expect("module must parse")
        .generate()
        .expect("generation must succeed")
        .to_string();

    // Isolate the prelude re-export group: `pub use super :: { <names> } ;`.
    let group = generated
        .split_once("pub use super")
        .expect("a prelude must be emitted")
        .1;
    let open = group.find('{').expect("prelude group open brace");
    let close = group.find('}').expect("prelude group close brace");
    let names: Vec<&str> = group[open + 1..close]
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .collect();

    assert_eq!(
        names,
        ["alpha", "mu", "zeta"],
        "prelude re-exports must be emitted in sorted order regardless of __all__ order",
    );
}

#[test]
fn empty_name_from_class_dir_is_an_error_instead_of_a_panic() {
    let result = catch_unwind(AssertUnwindSafe(|| {
        Codegen::default().module_from_str(
            indoc::indoc! {r#"
                class EmptyDirMeta(type):
                    def __dir__(cls):
                        return [""]

                class Broken(metaclass=EmptyDirMeta):
                    pass
            "#},
            "empty_class_dir_name",
        )
    }));

    let error = result
        .expect("empty class __dir__ name must not panic")
        .expect_err("empty class __dir__ name must reject introspection");
    assert!(
        error.to_string().contains("class") && error.to_string().contains("empty attribute name"),
        "unexpected error: {error}",
    );
}

#[test]
fn noncanonical_live_module_paths_are_rejected_without_panicking() {
    for module_name in ["", ".relative", "trailing.", "double..dot", "rust::path"] {
        let result = catch_unwind(AssertUnwindSafe(|| {
            Codegen::default().module_from_str("value = 1", module_name)
        }));
        let error = result
            .unwrap_or_else(|_| panic!("invalid path '{module_name}' must not panic"))
            .unwrap_err();
        assert!(
            error.to_string().contains("invalid live module path"),
            "unexpected error for '{module_name}': {error}",
        );
    }
}

#[test]
fn embedded_source_cannot_change_registered_module_identity() {
    let error = Codegen::default()
        .module_from_str("__name__ = 'renamed_by_source'", "expected_source_name")
        .expect_err("source-mutated module identity must be rejected");

    assert!(error
        .to_string()
        .contains("live module resolved to 'renamed_by_source', expected 'expected_source_name'"));
}

#[test]
fn recursively_discovered_live_module_cannot_be_registered_again() {
    let codegen = Codegen::new(Config {
        generate_dependencies: false,
        ..Config::default()
    })
    .module_name("html")
    .expect("html package should parse");

    let error = codegen
        .module_name("html.entities")
        .expect_err("discovered submodule must count as registered");

    assert!(error
        .to_string()
        .contains("duplicate live module path 'html.entities'"));
}

#[test]
fn live_package_cannot_rediscover_an_already_registered_child() {
    let codegen = Codegen::new(Config {
        generate_dependencies: false,
        ..Config::default()
    })
    .module_name("html.entities")
    .expect("html.entities should parse");

    let error = codegen
        .module_name("html")
        .expect_err("package must not silently merge a registered child");

    assert!(error
        .to_string()
        .contains("duplicate live module path 'html.entities'"));
}

#[test]
fn dotted_embedded_source_keeps_its_helper_and_report_identity() {
    let generated = Codegen::default()
        .module_from_str("value = 1", "html.pyo3_bindgen_embedded")
        .unwrap()
        .generate_with_report()
        .unwrap();
    let source = pyo3_bindgen_engine::RustArtifact::from_tokens(generated.tokens)
        .unwrap()
        .contents()
        .to_owned();

    assert!(source.contains("pyo3_embed_python_source_code"));
    let module = generated
        .report
        .modules
        .iter()
        .find(|module| module.name == "html.pyo3_bindgen_embedded")
        .unwrap();
    assert_eq!(module.input_kind, "embedded_source");
    assert!(module.source_hash.is_some());
}

#[test]
fn embedded_python_module_key_uses_python_not_rust_spelling() {
    let tokens = Codegen::default()
        .module_from_str("value = 1", "struct")
        .unwrap()
        .generate()
        .unwrap();
    let source = pyo3_bindgen_engine::RustArtifact::from_tokens(tokens)
        .unwrap()
        .contents()
        .to_owned();

    assert!(source.contains("\"struct\""));
    assert!(!source.contains("\"r#struct\""));
}

/// Embedded fixture covering every closed-`Flag` shape the mirror must handle: a plain
/// `Flag` with a composite member (`RW = 3`), an `IntFlag` with a true alias
/// (`ALIAS_A = 1`), and an unsupported flag class (a negative member) that must keep the
/// owning-wrapper fallback.
const FLAG_FIXTURE: &str = indoc::indoc! { r#"
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

    class Weird(enum.Flag):
        NEG = -1

    class Huge(enum.Flag):
        BIG = 1 << 64
"# };

/// Generate the flag fixture under `module_name`, returning the pretty-printed Rust source
/// and the generation report.
fn generate_flag_fixture(module_name: &str) -> (String, pyo3_bindgen_engine::GenerationReport) {
    let generated = Codegen::new(Config {
        generate_provenance: false,
        ..Default::default()
    })
    .module_from_str(FLAG_FIXTURE, module_name)
    .expect("flag fixture must parse")
    .generate_with_report()
    .expect("flag fixture must generate");
    let source = prettyplease::unparse(&syn::parse_str(&generated.tokens.to_string()).unwrap());
    (source, generated.report)
}

#[test]
fn closed_flag_enum_mirrors_as_value_newtype_struct() {
    let (source, _report) = generate_flag_fixture("mod_flag_mirror_struct");

    assert!(
        source.contains("pub struct Perm(u64);"),
        "a closed Flag class must mirror as a value newtype:\n{source}"
    );
    assert!(
        source.contains("pub const R: Perm = Perm(1u64);")
            && source.contains("pub const W: Perm = Perm(2u64);")
            && source.contains("pub const X: Perm = Perm(4u64);"),
        "every canonical member must become a bit-valued const:\n{source}"
    );
    assert!(
        source.contains("pub const Rw: Perm = Perm(3u64);"),
        "a composite member must become a const with its composite bits:\n{source}"
    );
    assert!(
        source.contains("pub const fn value(self) -> u64")
            && source.contains("pub const fn contains(self, other: Perm) -> bool")
            && source.contains("pub const fn is_empty(self) -> bool"),
        "the flag mirror must expose value/contains/is_empty:\n{source}"
    );
    assert!(
        source.contains("::core::ops::BitOr for Perm")
            && source.contains("::core::ops::BitAnd for Perm")
            && source.contains("::core::ops::BitXor for Perm"),
        "the flag mirror must implement the three set operators:\n{source}"
    );
    assert!(
        !source.contains("::core::ops::Not for Perm"),
        "`Not` is deliberately omitted in v1 (Python boundary semantics):\n{source}"
    );
    assert!(
        !source.contains("impl Perm {\n    pub fn as_py_name")
            && !source.contains("fn as_py_name(self) -> &'static str"),
        "flag mirrors must not claim a canonical member name:\n{source}"
    );
    // No owning Python-object wrapper for the supported flag classes.
    assert!(
        !source.contains("pub struct Perm {") && !source.contains("pub trait PermMethods"),
        "a mirrored flag must not also emit the owning wrapper:\n{source}"
    );
    // The conversions import the real class and call it with the bits.
    assert!(
        source.contains("PyAnyMethods::call1(enum_type.as_any(), (self.0,))"),
        "IntoPyObject must call the real Python flag class with the value:\n{source}"
    );
}

#[test]
fn flag_enum_alias_member_becomes_a_const_with_its_target_bits() {
    let (source, _report) = generate_flag_fixture("mod_flag_mirror_alias");

    assert!(
        source.contains("pub struct Mode(u64);"),
        "a closed IntFlag class must mirror as a value newtype:\n{source}"
    );
    assert!(
        source.contains("pub const A: Mode = Mode(1u64);"),
        "canonical IntFlag members must become consts:\n{source}"
    );
    assert!(
        source.contains("pub const AliasA: Mode = Mode::A;"),
        "an alias member must become a const pointing at its canonical target:\n{source}"
    );
}

/// Both rejection arms of the `u64` bit model are covered here: `Weird` holds a NEGATIVE
/// member and `Huge` a member ABOVE `u64::MAX` (`1 << 64`). Each must keep the owning
/// Python-object wrapper and warn; the supported classes in the same fixture must not.
#[test]
fn unsupported_flag_enum_keeps_the_wrapper_fallback_and_warns() {
    let (source, report) = generate_flag_fixture("mod_flag_mirror_unsupported");

    assert!(
        !source.contains("pub struct Weird(u64);"),
        "a flag class with a negative member must not mirror:\n{source}"
    );
    assert!(
        !source.contains("pub struct Huge(u64);"),
        "a flag class with a member above u64::MAX must not mirror:\n{source}"
    );
    assert!(
        source.contains("pub struct Weird {\n        obj: ::pyo3::Py<::pyo3::types::PyAny>,"),
        "an unsupported flag class must keep the owning wrapper:\n{source}"
    );
    assert!(
        source.contains("pub struct Huge {\n        obj: ::pyo3::Py<::pyo3::types::PyAny>,"),
        "an above-u64 flag class must keep the owning wrapper:\n{source}"
    );

    let mut flag_fallbacks = report
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code == "enum_flag_fallback")
        .map(|diagnostic| diagnostic.path.clone().unwrap_or_default())
        .collect::<Vec<_>>();
    flag_fallbacks.sort();
    assert_eq!(
        flag_fallbacks,
        vec![
            "mod_flag_mirror_unsupported.Huge".to_owned(),
            "mod_flag_mirror_unsupported.Weird".to_owned(),
        ],
        "only the unsupported flag classes may warn: {flag_fallbacks:?}"
    );
}

#[test]
fn flag_enum_report_records_the_flag_backing_kind() {
    let (_source, report) = generate_flag_fixture("mod_flag_mirror_report");

    let module = report
        .modules
        .iter()
        .find(|module| module.name == "mod_flag_mirror_report")
        .expect("the fixture module must be reported");
    let kind_of = |name: &str| {
        module
            .enums
            .iter()
            .find(|enumeration| enumeration.name == format!("mod_flag_mirror_report.{name}"))
            .unwrap_or_else(|| panic!("enum {name} must be reported: {:?}", module.enums))
            .kind
            .clone()
    };

    assert_eq!(kind_of("Perm"), "Flag");
    assert_eq!(kind_of("Mode"), "IntFlag");
    assert!(
        !module
            .enums
            .iter()
            .any(|enumeration| enumeration.name.ends_with(".Weird")),
        "an unsupported flag class is not a mirrored enum: {:?}",
        module.enums
    );

    let perm = module
        .enums
        .iter()
        .find(|enumeration| enumeration.name == "mod_flag_mirror_report.Perm")
        .unwrap();
    assert_eq!(
        perm.variants
            .iter()
            .map(|variant| (variant.python_name.as_str(), variant.value_repr.as_str()))
            .collect::<Vec<_>>(),
        vec![("R", "1"), ("W", "2"), ("X", "4"), ("RW", "3")],
        "flag members are reported in deterministic Python declaration order"
    );
}
