//! Compile-in-tests proof for external-IR class hierarchies.
//!
//! The correctness of the inherent-forwarder mechanism (a class-hierarchy binding on a
//! concrete generated newtype must COMPILE for every caller import combination, with no
//! `E0034`/`E0050`/`E0599`) cannot be established by string-matching generated tokens.
//! Generic `T: *Methods` receivers do not see the concrete newtype's inherent forwarders
//! and use explicit UFCS for diamond names. The regression that motivated this mechanism
//! shipped precisely because no test ever compiled a generated IR class hierarchy. This
//! test builds a hierarchy exercising every collision shape, generates the bindings,
//! writes them plus a consumer that imports several `*Methods` traits together and calls
//! the methods, and runs `cargo check`.

use pyo3_bindgen_engine::{
    ClassIr, Codegen, Config, EnumIr, EnumKindIr, EnumVariantIr, FunctionIr, FunctionKindIr,
    ImportIr, MethodKindIr, ModuleIr, ParameterIr, ParameterKindIr, PropertyIr, SchemaIr,
};

#[path = "../test_support/bounded_consumer.rs"]
mod bounded_consumer;

/// One `self`-receiver instance method `name(self, <params>) -> ret`.
fn instance_method(qualified_name: &str, params: &[&str], ret: &str) -> FunctionIr {
    let mut ir_params = vec![ParameterIr {
        name: "self".to_owned(),
        kind: ParameterKindIr::PositionalOrKeyword,
        annotation: String::new(),
        has_default: false,
    }];
    ir_params.extend(params.iter().map(|param| ParameterIr {
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
        params: ir_params,
        return_type: ret.to_owned(),
        docstring: None,
    }
}

/// One `self`-receiver instance method with per-parameter annotations:
/// `name(self, <p0>: <ann0>, ...) -> ret`. Unlike `instance_method`, each parameter
/// carries its own annotation string so a dict/enum-keyed signature can be exercised.
fn method_with_params(qualified_name: &str, params: &[(&str, &str)], ret: &str) -> FunctionIr {
    let mut ir_params = vec![ParameterIr {
        name: "self".to_owned(),
        kind: ParameterKindIr::PositionalOrKeyword,
        annotation: String::new(),
        has_default: false,
    }];
    ir_params.extend(params.iter().map(|(name, annotation)| ParameterIr {
        name: (*name).to_owned(),
        kind: ParameterKindIr::PositionalOrKeyword,
        annotation: (*annotation).to_owned(),
        has_default: false,
    }));
    FunctionIr {
        name: qualified_name.to_owned(),
        kind: FunctionKindIr::Method {
            method_type: MethodKindIr::Instance,
        },
        params: ir_params,
        return_type: ret.to_owned(),
        docstring: None,
    }
}

/// One constructor (`__init__`) with a `self` receiver and the given extra params.
fn constructor(qualified_name: &str, params: &[&str]) -> FunctionIr {
    FunctionIr {
        kind: FunctionKindIr::Method {
            method_type: MethodKindIr::Constructor,
        },
        ..instance_method(qualified_name, params, "")
    }
}

fn class_with(
    name: &str,
    bases: &[&str],
    methods: Vec<FunctionIr>,
    properties: Vec<PropertyIr>,
) -> ClassIr {
    ClassIr {
        name: name.to_owned(),
        base_classes: bases.iter().map(|base| (*base).to_owned()).collect(),
        docstring: None,
        methods,
        properties,
    }
}

/// The hierarchy under test, all in one module so the whole crate must compile:
/// - `A.foo(self, a) -> int`
/// - `B(A).foo(self, a) -> int`            same-signature override (defect 3)
/// - `D(A).foo(self, a, b) -> int`         signature-changing override (defects 1+2)
/// - `G(D)` no own `foo`                   linear grandchild, `foo` declared by D and A (defect 4 shape)
/// - `P1.bar(self, a) -> int`
/// - `P2.bar(self, a) -> int`
/// - `J(P1, P2)` no own `bar`              true diamond (defect 4)
/// - `A.value` mutable property; `C(A).value` override      property override
fn hierarchy_module() -> ModuleIr {
    let a = class_with(
        "m.A",
        &[],
        vec![instance_method("m.A.foo", &["a"], "int")],
        vec![PropertyIr {
            name: "m.A.value".to_owned(),
            py_type: "int".to_owned(),
            setter_type: Some("int".to_owned()),
            readonly: false,
            docstring: None,
            setter_docstring: None,
        }],
    );
    let b = class_with(
        "m.B",
        &["m.A"],
        vec![instance_method("m.B.foo", &["a"], "int")],
        vec![],
    );
    let d = class_with(
        "m.D",
        &["m.A"],
        vec![instance_method("m.D.foo", &["a", "b"], "int")],
        vec![],
    );
    let g = class_with("m.G", &["m.D"], vec![], vec![]);
    let p1 = class_with(
        "m.P1",
        &[],
        vec![instance_method("m.P1.bar", &["a"], "int")],
        vec![],
    );
    let p2 = class_with(
        "m.P2",
        &[],
        vec![instance_method("m.P2.bar", &["a"], "int")],
        vec![],
    );
    let j = class_with("m.J", &["m.P1", "m.P2"], vec![], vec![]);
    let c = class_with(
        "m.C",
        &["m.A"],
        vec![],
        vec![PropertyIr {
            name: "m.C.value".to_owned(),
            py_type: "int".to_owned(),
            setter_type: Some("int".to_owned()),
            readonly: false,
            docstring: None,
            setter_docstring: None,
        }],
    );

    // E0201 regression: a constructor (`__init__` -> inherent `pub fn new`) whose class
    // also inherits a method literally named `new` from two ancestors. The rule-(b)
    // diamond forwarder must not emit a second `pub fn new`.
    let n1 = class_with(
        "m.N1",
        &[],
        vec![instance_method("m.N1.new", &["a"], "int")],
        vec![],
    );
    let n2 = class_with(
        "m.N2",
        &[],
        vec![instance_method("m.N2.new", &["a"], "int")],
        vec![],
    );
    let nc = class_with(
        "m.NC",
        &["m.N1", "m.N2"],
        vec![constructor("m.NC.__init__", &[])],
        vec![],
    );

    // Root/marker-trait collision regression: a member literally named `downcast` must
    // be renamed (to `py_downcast`) so it does not shadow the generic downcast API.
    let down = class_with(
        "m.Down",
        &[],
        vec![instance_method("m.Down.downcast", &["a"], "int")],
        vec![],
    );

    // Non-linear diamond (C3 vs DFS): XD(XB, XC), XB(X), XC(X) overriding `foo`. C3
    // MRO(XD) = [XD, XB, XC, X], so `foo` must bind to XC, not the farther X. The two
    // `foo`s take DIFFERENT arities (X: 1 arg, XC: 2 args) so this discriminates C3 from
    // a plain DFS pre-order: DFS would give MRO [XB, X, XC], bind to the 1-arg X, and the
    // 2-arg consumer call below would fail E0061 -- i.e. a silent regression to DFS breaks
    // the build, not just this test's intent.
    let x = class_with(
        "m.X",
        &[],
        vec![instance_method("m.X.foo", &["a"], "int")],
        vec![],
    );
    let xb = class_with("m.XB", &["m.X"], vec![], vec![]);
    let xc = class_with(
        "m.XC",
        &["m.X"],
        vec![instance_method("m.XC.foo", &["a", "b"], "int")],
        vec![],
    );
    let xd = class_with("m.XD", &["m.XB", "m.XC"], vec![], vec![]);

    // Enum regression: an IntEnum member above i64::MAX must compile via #[repr(u64)].
    let flags = EnumIr {
        name: "m.Flags".to_owned(),
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
    };

    ModuleIr {
        name: "m".to_owned(),
        docstring: None,
        is_package: true,
        classes: vec![a, b, d, g, p1, p2, j, c, n1, n2, nc, down, x, xb, xc, xd],
        enums: vec![flags],
        functions: vec![],
        properties: vec![],
        submodules: vec![],
        prelude: vec![],
        imports: vec![],
        type_vars: vec![],
    }
}

/// A consumer that imports several `*Methods` traits together — the crate's own
/// documented pattern, and the exact situation that yielded `E0034` before the
/// inherent forwarders. Every call must resolve to a single inherent method. The
/// functions only need to type-check (`cargo check` never runs them).
const CONSUMER: &str = r#"
pub mod __consumer_probes {
    use super::m;

    // Same-signature override: both traits declare `foo`; the inherent method wins.
    pub fn same_sig(b: &m::B, py: ::pyo3::marker::Python) -> ::pyo3::PyResult<i64> {
        #[allow(unused_imports)]
        use m::{AMethods as _, BMethods as _};
        b.foo(py, 1)
    }

    // Signature-changing override: `d.foo` must be callable with D's own arity.
    pub fn changed_sig(d: &m::D, py: ::pyo3::marker::Python) -> ::pyo3::PyResult<i64> {
        #[allow(unused_imports)]
        use m::{AMethods as _, DMethods as _};
        d.foo(py, 1, 2)
    }

    // Linear grandchild: G owns no `foo`, inherited from D (2-arg) and A (1-arg).
    pub fn grandchild(g: &m::G, py: ::pyo3::marker::Python) -> ::pyo3::PyResult<i64> {
        #[allow(unused_imports)]
        use m::{AMethods as _, DMethods as _};
        g.foo(py, 1, 2)
    }

    // True diamond: J owns no `bar`, declared by both P1 and P2.
    pub fn diamond(j: &m::J, py: ::pyo3::marker::Python) -> ::pyo3::PyResult<i64> {
        #[allow(unused_imports)]
        use m::{JMethods as _, P1Methods as _, P2Methods as _};
        j.bar(py, 1)
    }

    // Generic receivers cannot use J's inherent forwarder. Name the C3 winner's
    // supertrait explicitly; a dot call on `j` would remain E0034.
    pub fn generic_diamond<T: m::JMethods>(
        j: &T,
        py: ::pyo3::marker::Python,
    ) -> ::pyo3::PyResult<i64> {
        <T as m::P1Methods>::bar(j, py, 1)
    }

    // Property override: getter and setter both resolve unambiguously.
    pub fn property(c: &m::C, py: ::pyo3::marker::Python) -> ::pyo3::PyResult<()> {
        #[allow(unused_imports)]
        use m::{AMethods as _, CMethods as _};
        let _current: i64 = c.value(py)?;
        c.set_value(py, 5)
    }

    // The ancestor's original remains reachable via UFCS after the override.
    pub fn ancestor_still_reachable(b: &m::B, py: ::pyo3::marker::Python) -> ::pyo3::PyResult<i64> {
        <m::B as m::AMethods>::foo(b, py, 1)
    }

    // Constructor + inherited `new`: NC compiles (the constructor `new` wins; the
    // inherited `new` is reached through the ancestor trait, not a second inherent fn).
    pub fn constructor_and_inherited_new(
        nc: &m::NC,
        py: ::pyo3::marker::Python,
    ) -> ::pyo3::PyResult<i64> {
        let _built = m::NC::new(py)?;
        <m::NC as m::N1Methods>::new(nc, py, 1)
    }

    // A member named `downcast` is renamed to `py_downcast`, off the root-trait API.
    pub fn renamed_root_trait_member(
        d: &m::Down,
        py: ::pyo3::marker::Python,
    ) -> ::pyo3::PyResult<i64> {
        d.py_downcast(py, 1)
    }

    // Non-linear diamond: `xd.foo` resolves unambiguously (binds to XC via C3) even with
    // both ancestor method traits in scope. The call uses XC's 2-arg arity; a DFS winner
    // (X, 1 arg) would fail E0061 here, so this locks C3 in at compile time.
    pub fn nonlinear_diamond(xd: &m::XD, py: ::pyo3::marker::Python) -> ::pyo3::PyResult<i64> {
        #[allow(unused_imports)]
        use m::{XBMethods as _, XCMethods as _};
        xd.foo(py, 1, 2)
    }

    // An IntEnum member above i64::MAX compiles (via #[repr(u64)]) and orders by value.
    pub fn big_int_enum_orders() -> bool {
        m::Flags::Small < m::Flags::Big
    }
}
"#;

/// The trait-only caller pattern: with the inherent forwarders suppressed
/// (`generate_inherent_forwarders = false`), own and inherited members are reached
/// only through the `*Methods` traits, so every call names its trait explicitly (UFCS)
/// and no `E0034` can arise. Constructors, upcasts, and enums are independent of the
/// forwarders and must still be emitted.
const NO_FORWARDER_CONSUMER: &str = r#"
pub mod __consumer_probes {
    use super::m;

    // Own member through its own trait.
    pub fn own_via_trait(b: &m::B, py: ::pyo3::marker::Python) -> ::pyo3::PyResult<i64> {
        <m::B as m::BMethods>::foo(b, py, 1)
    }

    // Inherited member reached through the ancestor trait the subtype impls.
    pub fn inherited_via_trait(g: &m::G, py: ::pyo3::marker::Python) -> ::pyo3::PyResult<i64> {
        <m::G as m::DMethods>::foo(g, py, 1, 2)
    }

    // Diamond: name one declaring trait explicitly (no inherent forwarder to auto-resolve).
    pub fn diamond_via_trait(j: &m::J, py: ::pyo3::marker::Python) -> ::pyo3::PyResult<i64> {
        <m::J as m::P1Methods>::bar(j, py, 1)
    }

    // Property getter/setter through the owning trait.
    pub fn property_via_trait(c: &m::C, py: ::pyo3::marker::Python) -> ::pyo3::PyResult<()> {
        let _current: i64 = <m::C as m::CMethods>::value(c, py)?;
        <m::C as m::CMethods>::set_value(c, py, 5)
    }

    // Zero-cost upcast is independent of the forwarders and still emitted.
    pub fn upcast(b: &m::B) -> &m::A {
        b.as_ref()
    }

    // Constructors live on the inherent impl, not a forwarder, so they survive.
    pub fn constructor(py: ::pyo3::marker::Python) -> ::pyo3::PyResult<m::NC> {
        m::NC::new(py)
    }

    // Enums are unaffected.
    pub fn enum_orders() -> bool {
        m::Flags::Small < m::Flags::Big
    }
}
"#;

/// Write `source` into a throwaway crate that depends on the pinned pyo3 and run
/// `cargo check --offline`, asserting it compiles. `tag` disambiguates the temp dir.
fn assert_source_compiles(source: &str, tag: &str) {
    // Minimal consumer manifest. The pyo3 feature set matches the workspace pin so a
    // `cargo check` reuses already-cached crate sources and can run offline.
    let pyo3_version = env!("PYO3_BINDGEN_PYO3_VERSION");
    let manifest = format!(
        r#"[package]
name = "pyo3_bindgen_ir_hierarchy_fixture"
version = "0.0.0"
edition = "2021"
publish = false

[workspace]

[dependencies]
pyo3 = {{ version = "={pyo3_version}", default-features = false, features = ["py-clone"] }}
"#
    );
    bounded_consumer::assert_consumer_compiles(
        source,
        tag,
        bounded_consumer::ConsumerManifest::Inline(&manifest),
        &["--quiet", "--offline"],
    );
}

#[test]
fn bounded_consumer_timeout_is_proven_by_exact_child_marker() {
    bounded_consumer::assert_timeout_regression(
        "bounded_consumer_timeout_is_proven_by_exact_child_marker",
    );
}

#[test]
fn external_ir_class_hierarchy_compiles_for_every_caller_import() {
    let generated = Codegen::new(Config::default())
        .modules_from_ir(SchemaIr {
            modules: vec![hierarchy_module()],
        })
        .expect("hierarchy IR must build")
        .generate()
        .expect("hierarchy must generate")
        .to_string();

    assert_source_compiles(&format!("{generated}\n{CONSUMER}"), "default");
}

#[test]
fn external_ir_hierarchy_without_inherent_forwarders_compiles_and_shrinks() {
    let build = |cfg: Config| {
        Codegen::new(cfg)
            .modules_from_ir(SchemaIr {
                modules: vec![hierarchy_module()],
            })
            .expect("hierarchy IR must build")
            .generate()
            .expect("hierarchy must generate")
            .to_string()
    };

    let with_forwarders = build(Config::default());
    let without_forwarders = build(
        Config::builder()
            .generate_inherent_forwarders(false)
            .build(),
    );

    // Suppressing the forwarders removes one `pub fn` per forwarded name -- a strict
    // reduction for this forwarder-heavy hierarchy.
    let count_pub_fn = |source: &str| source.matches("pub fn ").count();
    assert!(
        count_pub_fn(&without_forwarders) < count_pub_fn(&with_forwarders),
        "suppressing inherent forwarders must reduce the `pub fn` count: {} (off) vs {} (on)",
        count_pub_fn(&without_forwarders),
        count_pub_fn(&with_forwarders),
    );

    // And the trait-only object model still compiles for this caller pattern (every
    // inherited call named through its `*Methods` trait, plus surviving constructors,
    // upcasts, and enums).
    assert_source_compiles(
        &format!("{without_forwarders}\n{NO_FORWARDER_CONSUMER}"),
        "no_forwarders",
    );
}

/// One vocabulary-free module exercising downstream external-IR lowering shapes, so
/// a regression in any of them fails in THIS repo's CI rather than being discovered
/// downstream. Covers a small inheritance chain, a read-only vs mutable property, a
/// `set[Enum]` property (typed `HashSet<Enum>`), a reference to a type omitted from the IR
/// (must degrade to `Bound<PyAny>`, never dangle), and a collection property
/// (`PyCollection<T>`). No Blender names — honors the zero-vocabulary invariant.
fn contract_module() -> ModuleIr {
    let color = EnumIr {
        name: "m.Color".to_owned(),
        kind: EnumKindIr::Enum,
        variants: vec![
            EnumVariantIr {
                py_name: "RED".to_owned(),
                value_repr: "1".to_owned(),
            },
            EnumVariantIr {
                py_name: "BLUE".to_owned(),
                value_repr: "2".to_owned(),
            },
        ],
        aliases: vec![],
        docstring: None,
        is_flag: false,
        is_open: false,
        string_backed: false,
        plain_mirror: false,
    };
    let base = class_with(
        "m.Base",
        &[],
        vec![instance_method("m.Base.foo", &["a"], "int")],
        vec![],
    );
    let derived = class_with("m.Derived", &["m.Base"], vec![], vec![]);
    let property = |name: &str, py_type: &str, readonly: bool| PropertyIr {
        name: name.to_owned(),
        py_type: py_type.to_owned(),
        setter_type: None,
        readonly,
        docstring: None,
        setter_docstring: None,
    };
    let holder = ClassIr {
        name: "m.Holder".to_owned(),
        base_classes: vec![],
        docstring: None,
        methods: vec![],
        properties: vec![
            property("m.Holder.count", "int", false),
            property("m.Holder.label", "str", true),
            property("m.Holder.colors", "set[m.Color]", false),
            property("m.Holder.opaque", "m.NotInThisIr", true),
            property("m.Holder.children", "Collection[m.Base]", true),
            property("m.Holder.matrix", "Array[float; 2, 3]", true),
        ],
    };
    ModuleIr {
        name: "m".to_owned(),
        docstring: None,
        is_package: true,
        classes: vec![base, derived, holder],
        enums: vec![color],
        functions: vec![],
        properties: vec![],
        submodules: vec![],
        prelude: vec![],
        imports: vec![],
        type_vars: vec![],
    }
}

/// The external-IR lowering contract (see `docs/EXTERNAL_IR_LOWERING_CONTRACT.md`): each IR
/// shape lowers to a documented Rust shape, the output is byte-stable across runs, and the
/// whole module compiles (proving no dangling reference to an omitted type). Guards the
/// three regressions this consumer hit downstream (forwarder bloat, `set[Enum]` → `PySet`,
/// rejected RNA IR) in this repo's own suite.
#[test]
fn external_ir_contract_shapes_lower_as_documented() {
    let build = |cfg: Config| {
        Codegen::new(cfg)
            .modules_from_ir(SchemaIr {
                modules: vec![contract_module()],
            })
            .expect("contract IR must build")
            .generate()
            .expect("contract IR must generate")
            .to_string()
    };

    let generated = build(Config::default());

    // Read-only property: getter only, no setter.
    assert!(
        generated.contains("fn label") && !generated.contains("fn set_label"),
        "a read-only property must emit a getter and no setter:\n{generated}"
    );
    // Mutable property: getter and setter.
    assert!(
        generated.contains("fn count") && generated.contains("fn set_count"),
        "a mutable property must emit a getter and a setter:\n{generated}"
    );
    // `set[Enum]` lowers to a typed `HashSet<Enum>` (general, not lenient-gated) — never `PySet`.
    assert!(
        generated.contains("HashSet < Color >") && !generated.contains("PySet"),
        "set[Color] must lower to HashSet<Color>, not Bound<PySet>:\n{generated}"
    );
    // A reference to a type omitted from the IR degrades to `Bound<PyAny>` and never leaves a
    // dangling reference to the missing name.
    assert!(
        !generated.contains("NotInThisIr"),
        "an omitted-type reference must degrade to PyAny, not dangle:\n{generated}"
    );
    // A collection property lowers to the `PyCollection<T>` wrapper.
    assert!(
        generated.contains("PyCollection"),
        "Collection[...] must lower to the PyCollection wrapper:\n{generated}"
    );
    // CR7: a fixed-shape `Array[float; 2, 3]` lowers to a row-major nested fixed array
    // `[[f64; 3]; 2]` -- guards the dimension-nesting (`.rev()`) in `into_rs`.
    let compact = generated.replace(' ', "");
    assert!(
        compact.contains("[[f64;3];2]") || compact.contains("[[f64;3usize];2usize]"),
        "Array[float; 2, 3] must lower to a nested fixed array [[f64; 3]; 2]:\n{generated}"
    );

    // Byte-determinism: the same IR regenerates byte-identically (no HashMap/HashSet
    // iteration order leaking into the output).
    assert_eq!(
        generated,
        build(Config::default()),
        "generation must be byte-stable across runs for the same IR"
    );

    // The whole module compiles under the default and downstream flag sets — proving
    // every lowering above is valid Rust with no dangling reference.
    assert_source_compiles(&generated, "contract_default");
    let lenient_flat = build(
        Config::builder()
            .lenient_external_ir(true)
            .generate_inherent_forwarders(false)
            .build(),
    );
    assert_source_compiles(&lenient_flat, "contract_lenient_flat");

    // The full downstream flag set additionally treats mirror enums as string-backed (RNA enum
    // properties are the member string, with no backing Python enum class to import). The
    // string-backed `into_py_any`/`extract` bodies must compile in every real call site,
    // including the `HashSet<Color>` round-trip of the `set[m.Color]` property -- a
    // receiver-type mismatch there (e.g. extracting off the borrowed object) only a
    // `cargo check` catches.
    let string_backed = build(
        Config::builder()
            .lenient_external_ir(true)
            .generate_inherent_forwarders(false)
            .enum_ir_string_backed(true)
            .build(),
    );
    assert!(
        !string_backed.contains("expected Python enum member of"),
        "string-backed enums must not emit the class-import is_instance guard:\n{string_backed}"
    );
    assert_source_compiles(&string_backed, "contract_string_backed");
}

/// A flag-enum set-valued property (`set[Flags]`) must lower, under `--lenient-ir`, to a
/// typed `HashSet<Flags>` — matching the pinned generator — not a raw `Bound<PySet>`.
/// This downstream regression arose because the resolver was enum-kind-blind and the
/// original `cargo check` was run without `--all-targets`, so no test compiled the typed
/// set. This oracle both string-checks the lowering and compiles a `HashSet<MirrorEnum>`
/// consumer, so a regression to `PySet` fails to build here.
#[test]
fn lenient_ir_flag_enum_set_property_lowers_to_typed_hashset() {
    let flags = EnumIr {
        name: "m.Flags".to_owned(),
        kind: EnumKindIr::Enum,
        variants: vec![
            EnumVariantIr {
                py_name: "A".to_owned(),
                value_repr: "1".to_owned(),
            },
            EnumVariantIr {
                py_name: "B".to_owned(),
                value_repr: "2".to_owned(),
            },
        ],
        aliases: vec![],
        docstring: None,
        is_flag: true,
        is_open: false,
        string_backed: false,
        plain_mirror: false,
    };
    let holder = ClassIr {
        name: "m.Holder".to_owned(),
        base_classes: vec![],
        docstring: None,
        methods: vec![],
        properties: vec![PropertyIr {
            name: "m.Holder.opts".to_owned(),
            py_type: "set[m.Flags]".to_owned(),
            setter_type: None,
            readonly: false,
            docstring: None,
            setter_docstring: None,
        }],
    };
    let module = ModuleIr {
        name: "m".to_owned(),
        docstring: None,
        is_package: true,
        classes: vec![holder],
        enums: vec![flags],
        functions: vec![],
        properties: vec![],
        submodules: vec![],
        prelude: vec![],
        imports: vec![],
        type_vars: vec![],
    };

    let generated = Codegen::new(Config::builder().lenient_external_ir(true).build())
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .expect("lenient flag-enum IR must build")
        .generate()
        .expect("lenient flag-enum IR must generate")
        .to_string();

    assert!(
        generated.contains("HashSet < Flags >") && !generated.contains("PySet"),
        "a flag-enum set property must lower to HashSet<Flags> (not Bound<PySet>) under --lenient-ir:\n{generated}"
    );

    // A consumer that reads and writes the property through `HashSet<Flags>` must compile —
    // the exact downstream regression shape. If the property degraded to
    // `Bound<PySet>`, this fails E0308.
    const FLAGSET_CONSUMER: &str = r#"
pub mod __flagset_probe {
    use super::m;
    use std::collections::HashSet;

    pub fn roundtrip(h: &m::Holder, py: ::pyo3::marker::Python) -> ::pyo3::PyResult<()> {
        let current: HashSet<m::Flags> = h.opts(py)?;
        h.set_opts(py, &current)
    }

    pub fn build() -> HashSet<m::Flags> {
        let mut set = HashSet::new();
        set.insert(m::Flags::A);
        set.insert(m::Flags::B);
        set
    }
}
"#;
    assert_source_compiles(
        &format!("{generated}\n{FLAGSET_CONSUMER}"),
        "lenient_flagset",
    );
}

/// The same `set[Flags]` lowering promised by `docs/EXTERNAL_IR_LOWERING_CONTRACT.md`, but
/// reached through STRICT external IR via the per-enum `plain_mirror` opt-in instead of the
/// blanket `--lenient-ir` escape hatch. A producer that signs off on dropping flag semantics
/// must get the identical `HashSet<Flags>` property, not a raw `Bound<PySet>`.
#[test]
fn strict_plain_mirror_flag_enum_set_property_lowers_to_typed_hashset() {
    let flags = EnumIr {
        name: "m.Flags".to_owned(),
        kind: EnumKindIr::Enum,
        variants: vec![
            EnumVariantIr {
                py_name: "A".to_owned(),
                value_repr: "1".to_owned(),
            },
            EnumVariantIr {
                py_name: "B".to_owned(),
                value_repr: "2".to_owned(),
            },
        ],
        aliases: vec![],
        docstring: None,
        is_flag: true,
        is_open: false,
        string_backed: false,
        plain_mirror: true,
    };
    let holder = ClassIr {
        name: "m.Holder".to_owned(),
        base_classes: vec![],
        docstring: None,
        methods: vec![],
        properties: vec![PropertyIr {
            name: "m.Holder.opts".to_owned(),
            py_type: "set[m.Flags]".to_owned(),
            setter_type: None,
            readonly: false,
            docstring: None,
            setter_docstring: None,
        }],
    };
    let module = ModuleIr {
        name: "m".to_owned(),
        docstring: None,
        is_package: true,
        classes: vec![holder],
        enums: vec![flags],
        functions: vec![],
        properties: vec![],
        submodules: vec![],
        prelude: vec![],
        imports: vec![],
        type_vars: vec![],
    };

    let generated = Codegen::new(Config::default())
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .expect("plain_mirror flag-enum IR must build under strict validation")
        .generate()
        .expect("plain_mirror flag-enum IR must generate")
        .to_string();

    assert!(
        generated.contains("HashSet < Flags >") && !generated.contains("PySet"),
        "a plain_mirror flag-enum set property must lower to HashSet<Flags> under strict IR:\n{generated}"
    );

    const FLAGSET_CONSUMER: &str = r#"
pub mod __strict_flagset_probe {
    use super::m;
    use std::collections::HashSet;

    pub fn roundtrip(h: &m::Holder, py: ::pyo3::marker::Python) -> ::pyo3::PyResult<()> {
        let current: HashSet<m::Flags> = h.opts(py)?;
        h.set_opts(py, &current)
    }

    pub fn build() -> HashSet<m::Flags> {
        let mut set = HashSet::new();
        set.insert(m::Flags::A);
        set.insert(m::Flags::B);
        set
    }
}
"#;
    assert_source_compiles(
        &format!("{generated}\n{FLAGSET_CONSUMER}"),
        "strict_plain_mirror_flagset",
    );
}

/// A two-variant mirror enum used as a `dict`/`set` key. Vocabulary-free.
fn color_enum() -> EnumIr {
    EnumIr {
        name: "m.Color".to_owned(),
        kind: EnumKindIr::Enum,
        variants: vec![
            EnumVariantIr {
                py_name: "RED".to_owned(),
                value_repr: "1".to_owned(),
            },
            EnumVariantIr {
                py_name: "BLUE".to_owned(),
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

/// A class exposing `dict[Color, int]` in both argument and return position, plus an
/// `Optional[dict[Color, int]]` argument. The enum key resolves to a mirror `enum`
/// (`Eq + Hash`), so both dicts must lower to the typed `HashMap<Color, i64>` rather
/// than the untyped `Bound<PyDict>`.
fn dict_enum_key_module() -> ModuleIr {
    let cls = class_with(
        "m.Cls",
        &[],
        vec![
            method_with_params(
                "m.Cls.roundtrip",
                &[("d", "dict[m.Color, int]")],
                "dict[m.Color, int]",
            ),
            method_with_params(
                "m.Cls.opt_arg",
                &[("d", "Optional[dict[m.Color, int]]")],
                "int",
            ),
        ],
        vec![],
    );
    ModuleIr {
        name: "m".to_owned(),
        docstring: None,
        is_package: true,
        classes: vec![cls],
        enums: vec![color_enum()],
        functions: vec![],
        properties: vec![],
        submodules: vec![],
        prelude: vec![],
        imports: vec![],
        type_vars: vec![],
    }
}

/// Dicts that MUST keep degrading to `Bound<PyDict>`: a class-newtype key (derives only
/// `Debug`, not `Eq + Hash`), a `dict[Enum, Any]` value, and a `dict[Enum, <omitted>]`
/// value (both `contains_py_any`). This locks the preserved value-position and
/// non-enum-key behavior — the enum-key widening must not spill onto these.
fn dict_degraded_module() -> ModuleIr {
    let other = class_with("m.Other", &[], vec![], vec![]);
    let cls = class_with(
        "m.Deg",
        &[],
        vec![
            method_with_params("m.Deg.class_key", &[("d", "dict[m.Other, int]")], "int"),
            method_with_params("m.Deg.any_val", &[("d", "dict[m.Color, Any]")], "int"),
            method_with_params(
                "m.Deg.unmapped_val",
                &[("d", "dict[m.Color, m.NotInThisIr]")],
                "int",
            ),
        ],
        vec![],
    );
    ModuleIr {
        name: "m".to_owned(),
        docstring: None,
        is_package: true,
        classes: vec![other, cls],
        enums: vec![color_enum()],
        functions: vec![],
        properties: vec![],
        submodules: vec![],
        prelude: vec![],
        imports: vec![],
        type_vars: vec![],
    }
}

/// A consumer that exercises the typed `dict[Enum, V]` lowering in every position:
/// `&HashMap<Enum, V>` argument, `HashMap<Enum, V>` return, `Option<HashMap<Enum, V>>`
/// argument, and a `HashMap<Enum, V>` literal built from mirror-enum keys. If either the
/// return (`K: FromPyObjectOwned + Eq + Hash`) or argument (`&K: IntoPyObject + Eq + Hash`)
/// bound failed to resolve for the generated enum under pyo3 0.29, this fails to compile.
const DICT_ENUM_CONSUMER: &str = r#"
pub mod __dict_enum_probe {
    use super::m;
    use std::collections::HashMap;

    // Argument position (&HashMap<Enum, V>) and return position (HashMap<Enum, V>).
    pub fn roundtrip(
        c: &m::Cls,
        py: ::pyo3::marker::Python,
        d: &HashMap<m::Color, i64>,
    ) -> ::pyo3::PyResult<HashMap<m::Color, i64>> {
        #[allow(unused_imports)]
        use m::ClsMethods as _;
        c.roundtrip(py, d)
    }

    // Optional argument: by-value Option<HashMap<Enum, V>> with no preconversion.
    pub fn opt_arg(
        c: &m::Cls,
        py: ::pyo3::marker::Python,
        d: Option<HashMap<m::Color, i64>>,
    ) -> ::pyo3::PyResult<i64> {
        #[allow(unused_imports)]
        use m::ClsMethods as _;
        c.opt_arg(py, d)
    }

    // A typed map keyed on the mirror enum (proves the Eq + Hash derive).
    pub fn build() -> HashMap<m::Color, i64> {
        let mut map = HashMap::new();
        map.insert(m::Color::Red, 1);
        map.insert(m::Color::Blue, 2);
        map
    }
}
"#;

/// `dict[Enum, V]` lowers to the typed `HashMap<Enum, V>` in both argument and return
/// position (mirroring the shipped `set[Enum]` → `HashSet<Enum>` fix). The typed argument
/// path drops the `IntoPyDict::into_py_dict` preconversion, and the compile oracle proves
/// the pyo3 0.29 conversion bounds resolve for the generated enum in both directions.
#[test]
fn external_ir_dict_enum_key_lowers_to_hashmap_both_positions() {
    let generated = Codegen::new(Config::default())
        .modules_from_ir(SchemaIr {
            modules: vec![dict_enum_key_module()],
        })
        .expect("dict-enum IR must build")
        .generate()
        .expect("dict-enum IR must generate")
        .to_string();

    // Both positions lower to the typed HashMap<Color, i64>.
    assert!(
        generated.contains("HashMap < Color , i64 >"),
        "dict[Color, int] must lower to HashMap<Color, i64> in both positions:\n{generated}"
    );
    // No degradation: neither the untyped `Bound<PyDict>` handle nor the `IntoPyDict`
    // borrowed type may appear for a fully-typed enum-keyed dict.
    assert!(
        !generated.contains("PyDict"),
        "a typed enum-keyed dict must not fall back to Bound<PyDict>/IntoPyDict:\n{generated}"
    );
    // Typed dict arguments (including the Optional wrapper) drop the preconversion.
    assert!(
        !generated.contains("into_py_dict"),
        "a typed enum-keyed dict argument must not emit an into_py_dict preconversion:\n{generated}"
    );

    // Compile oracle: prove the generated bindings + a HashMap<Color, i64> consumer
    // cargo-check under the pinned pyo3, in argument, return, and Optional positions.
    assert_source_compiles(&format!("{generated}\n{DICT_ENUM_CONSUMER}"), "dict_enum");
}

/// The enum-key widening must not spill: a class-newtype key stays `Bound<PyDict>`, and a
/// `dict[Enum, Any]` / `dict[Enum, <omitted>]` value still degrades the whole dict (the
/// `contains_py_any` value-position guard is preserved). None of these may lower to a typed
/// `HashMap`, and the degraded argument path keeps its `into_py_dict` preconversion.
#[test]
fn external_ir_dict_degrades_on_class_key_and_pyany_value() {
    let generated = Codegen::new(Config::default())
        .modules_from_ir(SchemaIr {
            modules: vec![dict_degraded_module()],
        })
        .expect("degraded-dict IR must build")
        .generate()
        .expect("degraded-dict IR must generate")
        .to_string();

    // All three shapes keep the untyped `Bound<PyDict>` handle.
    assert!(
        generated.contains("PyDict"),
        "class-key and PyAny-value dicts must stay Bound<PyDict>:\n{generated}"
    );
    // None of them lower to a typed HashMap.
    assert!(
        !generated.contains("HashMap"),
        "a class-key or PyAny-value dict must not lower to a typed HashMap:\n{generated}"
    );
    // The degraded argument path keeps the IntoPyDict::into_py_dict preconversion.
    assert!(
        generated.contains("into_py_dict"),
        "a degraded dict argument must still materialize a PyDict via into_py_dict:\n{generated}"
    );
}

/// One static method `name(<params>) -> ret` (no `self` receiver). Static methods live on
/// the inherent `impl`, not the `*Methods` trait, so a diamond whose MRO-nearest declarer
/// carries the colliding name as a static method has no trait method to forward to.
fn static_method(qualified_name: &str, params: &[&str], ret: &str) -> FunctionIr {
    FunctionIr {
        kind: FunctionKindIr::Method {
            method_type: MethodKindIr::Static,
        },
        params: params
            .iter()
            .map(|param| ParameterIr {
                name: (*param).to_owned(),
                kind: ParameterKindIr::PositionalOrKeyword,
                annotation: "int".to_owned(),
                has_default: false,
            })
            .collect(),
        ..instance_method(qualified_name, &[], ret)
    }
}

/// A leaf submodule (`is_package = false`) carrying just the given classes. The explicit
/// `is_package: false` is required by the IR shape.
fn ir_submodule(name: &str, classes: Vec<ClassIr>) -> ModuleIr {
    ModuleIr {
        name: name.to_owned(),
        docstring: None,
        is_package: false,
        classes,
        enums: vec![],
        functions: vec![],
        properties: vec![],
        submodules: vec![],
        prelude: vec![],
        imports: vec![],
        type_vars: vec![],
    }
}

/// A package (`is_package = true`) with no own members, wrapping the given submodules.
fn ir_package(name: &str, submodules: Vec<ModuleIr>) -> ModuleIr {
    ModuleIr {
        name: name.to_owned(),
        docstring: None,
        is_package: true,
        classes: vec![],
        enums: vec![],
        functions: vec![],
        properties: vec![],
        submodules,
        prelude: vec![],
        imports: vec![],
        type_vars: vec![],
    }
}

/// The cross-module diamond: package `x` whose submodules place each colliding ancestor in
/// a DIFFERENT module. `x.a.A.dup` and `x.b.B.dup` are both instance methods; `x.c.J`
/// inherits from both and declares no `dup` of its own. C3 MRO(J) = [J, A, B], so the
/// nearest declarer is A (in `x.a`). The generated crate alone compiles either way; only a
/// consumer importing BOTH `AMethods` and `BMethods` and calling `j.dup(py, 1)` detects the
/// missing forwarder (E0034 before the global-registry fix).
fn cross_module_diamond_package() -> ModuleIr {
    let a = class_with(
        "x.a.A",
        &[],
        vec![instance_method("x.a.A.dup", &["a"], "int")],
        vec![],
    );
    let b = class_with(
        "x.b.B",
        &[],
        vec![instance_method("x.b.B.dup", &["a"], "int")],
        vec![],
    );
    let j = class_with("x.c.J", &["x.a.A", "x.b.B"], vec![], vec![]);
    ir_package(
        "x",
        vec![
            ir_submodule("x.a", vec![a]),
            ir_submodule("x.b", vec![b]),
            ir_submodule("x.c", vec![j]),
        ],
    )
}

/// The load-bearing consumer: it imports both ancestor traits from different modules and
/// calls the inherited method on the concrete receiver. This is the exact `E0034` situation
/// module-local resolution left unresolved; the global-registry forwarder fixes it.
const CROSS_MODULE_CONSUMER: &str = r#"
pub mod __cross_module_probe {
    use super::x;

    pub fn cross_module_diamond(j: &x::c::J, py: ::pyo3::marker::Python) -> ::pyo3::PyResult<i64> {
        #[allow(unused_imports)]
        use x::a::AMethods as _;
        #[allow(unused_imports)]
        use x::b::BMethods as _;
        j.dup(py, 1)
    }
}
"#;

/// The suppressed-forwarder counterpart: with no inherent forwarders, the inherited
/// name is reached only through an explicitly named ancestor trait (UFCS), so no `E0034`
/// can arise and the trait-only object model must still compile.
const CROSS_MODULE_NO_FORWARDER_CONSUMER: &str = r#"
pub mod __cross_module_ufcs_probe {
    use super::x;

    pub fn via_trait(j: &x::c::J, py: ::pyo3::marker::Python) -> ::pyo3::PyResult<i64> {
        <x::c::J as x::a::AMethods>::dup(j, py, 1)
    }
}
"#;

/// Avenue A load-bearing oracle: a cross-module diamond compiles for a consumer that
/// imports BOTH ancestor traits and calls the inherited method on the concrete receiver.
/// Before rule (b) consulted the global class registry, the remote ancestors were invisible
/// to `x.c`'s module-local lookup, no inherent forwarder was emitted, and this consumer
/// failed with `E0034`. The generated crate alone compiled either way, so only this
/// two-trait consumer proves the fix.
#[test]
fn external_ir_cross_module_diamond_compiles_for_every_caller_import() {
    let generated = Codegen::new(Config::default())
        .modules_from_ir(SchemaIr {
            modules: vec![cross_module_diamond_package()],
        })
        .expect("cross-module diamond IR must build")
        .generate()
        .expect("cross-module diamond must generate")
        .to_string();

    assert_source_compiles(
        &format!("{generated}\n{CROSS_MODULE_CONSUMER}"),
        "cross_module",
    );
}

#[test]
fn external_ir_omitted_direct_base_promotes_visible_grandparent_trait() {
    let base = class_with(
        "pkg.Base",
        &[],
        vec![instance_method("pkg.Base.value", &[], "int")],
        vec![],
    );
    let middle = class_with("pkg.hidden.Middle", &["pkg.Base"], vec![], vec![]);
    let derived = class_with("pkg.Derived", &["pkg.hidden.Middle"], vec![], vec![]);
    let package = ModuleIr {
        name: "pkg".to_owned(),
        docstring: None,
        is_package: true,
        classes: vec![base, derived],
        enums: vec![],
        functions: vec![],
        properties: vec![],
        submodules: vec![ir_submodule("pkg.hidden", vec![middle])],
        prelude: vec![],
        imports: vec![],
        type_vars: vec![],
    };
    let generated = Codegen::new(Config::builder().traverse_submodules(false).build())
        .modules_from_ir(SchemaIr {
            modules: vec![package],
        })
        .expect("valid hierarchy must build even when traversal omits its direct base")
        .generate()
        .expect("visible grandparent promotion must generate")
        .to_string();
    let consumer = r#"
pub mod __omitted_base_probe {
    #[allow(unused_imports)]
    use super::pkg::{BaseMethods as _, DerivedMethods};

    pub fn inherited<T: DerivedMethods>(
        value: &T,
        py: ::pyo3::marker::Python,
    ) -> ::pyo3::PyResult<i64> {
        value.value(py)
    }
}
"#;

    assert!(
        !generated.contains("pub mod hidden"),
        "disabled traversal must not emit the hidden direct base: {generated}",
    );
    assert_source_compiles(&format!("{generated}\n{consumer}"), "omitted_direct_base");
}

#[test]
fn external_ir_canonical_collision_between_own_and_inherited_members_fails_soft() {
    let nfd_name = "e\u{301}";
    let nfc_name = "\u{e9}";
    let a = class_with(
        "pkg.A",
        &[],
        vec![instance_method(&format!("pkg.A.{nfd_name}"), &[], "int")],
        vec![],
    );
    let b = class_with(
        "pkg.B",
        &[],
        vec![instance_method(&format!("pkg.B.{nfd_name}"), &[], "int")],
        vec![],
    );
    let d = class_with(
        "pkg.D",
        &["pkg.A", "pkg.B"],
        vec![instance_method(&format!("pkg.D.{nfc_name}"), &[], "int")],
        vec![],
    );
    let package = ModuleIr {
        name: "pkg".to_owned(),
        docstring: None,
        is_package: true,
        classes: vec![a, b, d],
        enums: vec![],
        functions: vec![],
        properties: vec![],
        submodules: vec![],
        prelude: vec![],
        imports: vec![],
        type_vars: vec![],
    };
    let generated = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![package],
        })
        .expect("canonically equivalent inherited identifiers must validate")
        .generate_with_report()
        .expect("an own inherent method can disambiguate the inherited surface");
    assert!(
        generated
            .report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "class_forwarder_ident_conflict"),
        "the shadowed inherited member must produce a diagnostic: {:?}",
        generated.report.diagnostics,
    );
    assert_source_compiles(
        &generated.tokens.to_string(),
        "canonical_inherited_forwarder",
    );
}

#[test]
fn external_ir_rejects_canonical_collision_across_inherited_traits() {
    let nfd_name = "e\u{301}";
    let nfc_name = "\u{e9}";
    let a = class_with(
        "pkg.A",
        &[],
        vec![instance_method(&format!("pkg.A.{nfd_name}"), &[], "int")],
        vec![],
    );
    let b = class_with(
        "pkg.B",
        &[],
        vec![instance_method(&format!("pkg.B.{nfc_name}"), &[], "int")],
        vec![],
    );
    let d = class_with("pkg.D", &["pkg.A", "pkg.B"], vec![], vec![]);
    let package = ModuleIr {
        name: "pkg".to_owned(),
        docstring: None,
        is_package: true,
        classes: vec![a, b, d],
        enums: vec![],
        functions: vec![],
        properties: vec![],
        submodules: vec![],
        prelude: vec![],
        imports: vec![],
        type_vars: vec![],
    };

    let error = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![package],
        })
        .expect("the Python hierarchy itself is valid")
        .generate()
        .expect_err("canonically colliding inherited trait methods must be rejected");
    let message = error.to_string();
    assert!(
        message.contains("same canonical Rust method identifier")
            && message.contains("pkg.A")
            && message.contains("pkg.B"),
        "unexpected collision error: {message}",
    );
}

#[test]
fn external_ir_rejects_inherited_method_property_setter_ident_collision() {
    let a = class_with(
        "pkg.A",
        &[],
        vec![instance_method("pkg.A.set_x", &["value"], "int")],
        vec![],
    );
    let b = class_with(
        "pkg.B",
        &[],
        vec![],
        vec![PropertyIr {
            name: "pkg.B.x".to_owned(),
            py_type: "int".to_owned(),
            setter_type: Some("int".to_owned()),
            readonly: false,
            docstring: None,
            setter_docstring: None,
        }],
    );
    let d = class_with("pkg.D", &["pkg.A", "pkg.B"], vec![], vec![]);
    let package = ModuleIr {
        name: "pkg".to_owned(),
        docstring: None,
        is_package: true,
        classes: vec![a, b, d],
        enums: vec![],
        functions: vec![],
        properties: vec![],
        submodules: vec![],
        prelude: vec![],
        imports: vec![],
        type_vars: vec![],
    };

    let error = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![package],
        })
        .expect("the Python hierarchy itself is valid")
        .generate()
        .expect_err("distinct inherited members emitting `set_x` must be rejected");
    let message = error.to_string();
    assert!(
        message.contains("same canonical Rust method identifier 'set_x'")
            && message.contains("'set_x'")
            && message.contains("'x'"),
        "validator must compare emitted forwarder identifiers: {message}",
    );
}

/// Token/emission lock: the cross-module diamond synthesizes an inherent forwarder for the
/// inherited `dup`, delegating to the C3-nearest declarer A's trait, resolved
/// module-relatively from `x.c` as `super::a::AMethods`. This pins both the fix (a forwarder
/// is emitted) and the winner choice / C3 nearest-first order (A, not B).
#[test]
fn external_ir_cross_module_diamond_emits_forwarder_to_c3_winner() {
    let generated = Codegen::new(Config::default())
        .modules_from_ir(SchemaIr {
            modules: vec![cross_module_diamond_package()],
        })
        .expect("cross-module diamond IR must build")
        .generate()
        .expect("cross-module diamond must generate")
        .to_string();

    // The forwarder body names the C3-nearest declarer A's trait through the module-relative
    // path from `x.c` (`super::a::AMethods`). This exact string is emitted only by J's
    // cross-module rule-(b) forwarder -- A's and B's own rule-(a) forwarders name their trait
    // directly (`AMethods` / `BMethods`, no `super`), so it uniquely proves the fix.
    assert!(
        generated.contains("< Self as super :: a :: AMethods > :: dup"),
        "J must forward the inherited `dup` to the C3-nearest declarer A (super::a::AMethods):\n{generated}"
    );
}

/// The mixed diamond: one colliding ancestor local, one remote. `y.c.MJ` inherits `dup`
/// from `y.c.MB` (same module) and `y.a.MA` (sibling module). Before the global registry
/// the remote MA under-counted the collision to a single declarer, silently suppressing the
/// forwarder; now both are seen and MB (nearest) wins.
fn mixed_diamond_package() -> ModuleIr {
    let ma = class_with(
        "y.a.MA",
        &[],
        vec![instance_method("y.a.MA.dup", &["a"], "int")],
        vec![],
    );
    let mb = class_with(
        "y.c.MB",
        &[],
        vec![instance_method("y.c.MB.dup", &["a"], "int")],
        vec![],
    );
    let mj = class_with("y.c.MJ", &["y.c.MB", "y.a.MA"], vec![], vec![]);
    ir_package(
        "y",
        vec![
            ir_submodule("y.a", vec![ma]),
            ir_submodule("y.c", vec![mb, mj]),
        ],
    )
}

const MIXED_CONSUMER: &str = r#"
pub mod __mixed_probe {
    use super::y;

    pub fn mixed_diamond(j: &y::c::MJ, py: ::pyo3::marker::Python) -> ::pyo3::PyResult<i64> {
        #[allow(unused_imports)]
        use y::a::MAMethods as _;
        #[allow(unused_imports)]
        use y::c::MBMethods as _;
        j.dup(py, 1)
    }
}
"#;

/// Mixed-diamond regression: one ancestor same-module, one sibling-module. Today the remote
/// ancestor was invisible, so `bases.len() == 1` silently suppressed the forwarder; a
/// consumer importing both ancestor traits hit `E0034`. With the global registry the
/// forwarder is emitted (winner MB, the nearest declarer) and the consumer compiles.
#[test]
fn external_ir_mixed_module_diamond_compiles() {
    let generated = Codegen::new(Config::default())
        .modules_from_ir(SchemaIr {
            modules: vec![mixed_diamond_package()],
        })
        .expect("mixed diamond IR must build")
        .generate()
        .expect("mixed diamond must generate")
        .to_string();

    assert_source_compiles(&format!("{generated}\n{MIXED_CONSUMER}"), "mixed_module");
}

/// A cross-module multi-collision whose MRO-nearest owner contributes differently typed
/// method/property forwarders through two owner-local aliases. The child deliberately binds
/// the same short aliases to different classes; replaying the winner in the child's namespace
/// therefore makes the UFCS body ill-typed. The second ancestor uses primitive types so the
/// winner choice is also visible at the consumer boundary.
fn multi_collision_owner_alias_package() -> ModuleIr {
    let owner_payload = class_with("x.types_a.Payload", &[], vec![], vec![]);
    let owner_state = class_with("x.types_a.State", &[], vec![], vec![]);
    let child_payload = class_with("x.types_c.Payload", &[], vec![], vec![]);
    let child_state = class_with("x.types_c.State", &[], vec![], vec![]);

    let owner = class_with(
        "x.a.A",
        &[],
        vec![method_with_params(
            "x.a.A.shared_method",
            &[("value", "Payload")],
            "Payload",
        )],
        vec![PropertyIr {
            name: "x.a.A.shared_property".to_owned(),
            py_type: "State".to_owned(),
            setter_type: Some("State".to_owned()),
            readonly: false,
            docstring: None,
            setter_docstring: None,
        }],
    );
    let other = class_with(
        "x.b.B",
        &[],
        vec![method_with_params(
            "x.b.B.shared_method",
            &[("value", "int")],
            "int",
        )],
        vec![PropertyIr {
            name: "x.b.B.shared_property".to_owned(),
            py_type: "str".to_owned(),
            setter_type: Some("str".to_owned()),
            readonly: false,
            docstring: None,
            setter_docstring: None,
        }],
    );
    let child = class_with("x.c.C", &["x.a.A", "x.b.B"], vec![], vec![]);

    let mut owner_module = ir_submodule("x.a", vec![owner]);
    owner_module.imports = vec![
        ImportIr {
            origin: "x.types_a.Payload".to_owned(),
            target: "x.a.Payload".to_owned(),
        },
        ImportIr {
            origin: "x.types_a.State".to_owned(),
            target: "x.a.State".to_owned(),
        },
    ];
    let mut child_module = ir_submodule("x.c", vec![child]);
    child_module.imports = vec![
        ImportIr {
            origin: "x.types_c.Payload".to_owned(),
            target: "x.c.Payload".to_owned(),
        },
        ImportIr {
            origin: "x.types_c.State".to_owned(),
            target: "x.c.State".to_owned(),
        },
    ];

    ir_package(
        "x",
        vec![
            ir_submodule("x.types_a", vec![owner_payload, owner_state]),
            ir_submodule("x.types_c", vec![child_payload, child_state]),
            owner_module,
            ir_submodule("x.b", vec![other]),
            child_module,
        ],
    )
}

const MULTI_COLLISION_CONSUMER: &str = r#"
pub mod __multi_collision_probe {
    use super::x;

    pub fn method(
        c: &x::c::C,
        py: ::pyo3::marker::Python,
        value: &x::types_a::Payload,
    ) -> ::pyo3::PyResult<x::types_a::Payload> {
        #[allow(unused_imports)]
        use x::a::AMethods as _;
        #[allow(unused_imports)]
        use x::b::BMethods as _;
        c.shared_method(py, value)
    }

    pub fn property(
        c: &x::c::C,
        py: ::pyo3::marker::Python,
        value: &x::types_a::State,
    ) -> ::pyo3::PyResult<()> {
        #[allow(unused_imports)]
        use x::a::AMethods as _;
        #[allow(unused_imports)]
        use x::b::BMethods as _;
        let _: x::types_a::State = c.shared_property(py)?;
        c.set_shared_property(py, value)
    }
}
"#;

const MULTI_COLLISION_NO_FORWARDER_CONSUMER: &str = r#"
pub mod __multi_collision_ufcs_probe {
    use super::x;

    pub fn method(
        c: &x::c::C,
        py: ::pyo3::marker::Python,
        value: &x::types_a::Payload,
    ) -> ::pyo3::PyResult<x::types_a::Payload> {
        <x::c::C as x::a::AMethods>::shared_method(c, py, value)
    }

    pub fn property(
        c: &x::c::C,
        py: ::pyo3::marker::Python,
        value: &x::types_a::State,
    ) -> ::pyo3::PyResult<()> {
        let _: x::types_a::State = <x::c::C as x::a::AMethods>::shared_property(c, py)?;
        <x::c::C as x::a::AMethods>::set_shared_property(c, py, value)
    }
}
"#;

#[test]
fn external_ir_multi_collision_winner_reuses_owner_aliases_and_orders_suppressed_diagnostics() {
    let build = |cfg: Config| {
        Codegen::new(cfg)
            .modules_from_ir(SchemaIr {
                modules: vec![multi_collision_owner_alias_package()],
            })
            .expect("multi-collision IR must build")
            .generate_with_report()
            .expect("multi-collision IR must generate")
    };

    let enabled = build(Config::default());
    let enabled_source = enabled.tokens.to_string();
    assert!(
        enabled_source.contains("< Self as super :: a :: AMethods > :: shared_method")
            && enabled_source.contains("< Self as super :: a :: AMethods > :: shared_property")
            && enabled_source.contains("< Self as super :: a :: AMethods > :: set_shared_property"),
        "enabled multi-collision forwarders must target the nearest cross-module owner for every member: {enabled_source}"
    );
    assert!(
        enabled.report.diagnostics.is_empty(),
        "enabled forwarders must not emit suppressed-collision diagnostics: {:?}",
        enabled.report.diagnostics,
    );
    assert_source_compiles(
        &format!("{enabled_source}\n{MULTI_COLLISION_CONSUMER}"),
        "multi_collision_enabled",
    );

    let suppressed = build(
        Config::builder()
            .generate_inherent_forwarders(false)
            .build(),
    );
    let suppressed_source = suppressed.tokens.to_string();
    assert!(
        !suppressed_source.contains("pub fn shared_method")
            && !suppressed_source.contains("pub fn shared_property")
            && !suppressed_source.contains("pub fn set_shared_property"),
        "suppressed forwarders must not emit inherent multi-collision methods: {suppressed_source}"
    );
    assert_eq!(
        suppressed
            .report
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code.as_str())
            .collect::<Vec<_>>(),
        vec![
            "class_ambiguous_without_forwarders",
            "class_ambiguous_without_forwarders",
        ],
        "suppressed diagnostics must preserve collision order: {:?}",
        suppressed.report.diagnostics,
    );
    assert_eq!(
        suppressed
            .report
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.path.as_deref())
            .collect::<Vec<_>>(),
        vec![Some("x.c.C"), Some("x.c.C")],
        "suppressed diagnostics must identify the generated child: {:?}",
        suppressed.report.diagnostics,
    );
    assert!(
        suppressed.report.diagnostics[0]
            .message
            .contains("shared_method")
            && suppressed.report.diagnostics[1]
                .message
                .contains("shared_property"),
        "suppressed diagnostics must list method before property: {:?}",
        suppressed.report.diagnostics,
    );
    assert_source_compiles(
        &format!("{suppressed_source}\n{MULTI_COLLISION_NO_FORWARDER_CONSUMER}"),
        "multi_collision_suppressed",
    );
}

/// F3 (`--no-inherent-forwarders` mode): the ambiguity is now DETECTED and warned even
/// though no forwarder can be emitted in that mode, and the detection pass emits no tokens so
/// the output stays byte-identical. The trait-only model still compiles under
/// UFCS.
#[test]
fn external_ir_cross_module_diamond_without_forwarders_warns_but_emits_nothing() {
    let bindings = Codegen::new(
        Config::builder()
            .generate_inherent_forwarders(false)
            .build(),
    )
    .modules_from_ir(SchemaIr {
        modules: vec![cross_module_diamond_package()],
    })
    .expect("cross-module diamond IR must build")
    .generate_with_report()
    .expect("cross-module diamond must generate");

    // The ambiguity is surfaced (F3 detection-only diagnostic).
    assert!(
        bindings
            .report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "class_ambiguous_without_forwarders"),
        "suppressed-forwarder mode must warn about the ambiguous inherited `dup`: {:?}",
        bindings.report.diagnostics,
    );

    // Byte-neutral: the detection pass emits no inherent forwarder. No `pub fn` for the
    // inherited name appears anywhere (in this mode there are no inherent `impl` blocks at
    // all), proving the F3 branch leaves the token stream untouched, as required by
    // downstream generated-byte drift checks.
    let generated = bindings.tokens.to_string();
    assert!(
        !generated.contains("pub fn dup"),
        "the detection-only F3 pass must not emit any inherent forwarder:\n{generated}"
    );

    assert_source_compiles(
        &format!("{generated}\n{CROSS_MODULE_NO_FORWARDER_CONSUMER}"),
        "cross_module_no_fwd",
    );
}

/// F2 coverage widening: a cross-module diamond whose C3-nearest declarer carries the
/// colliding name as a STATIC method. There is no trait method to forward to, so no inherent
/// forwarder is emitted and the existing `class_forwarder_winner_not_callable` warning fires
/// -- which it could NOT do cross-module before the global registry, because SA and SB were
/// invisible to SJ's module-local lookup. The generated crate itself stays valid Rust (the
/// residual E0034 only bites a consumer that imports both instance-method traits and calls
/// `.dup`).
fn static_winner_diamond_package() -> ModuleIr {
    let sa = class_with(
        "z.a.SA",
        &[],
        vec![static_method("z.a.SA.dup", &["a"], "int")],
        vec![],
    );
    let sb = class_with(
        "z.b.SB",
        &[],
        vec![instance_method("z.b.SB.dup", &["a"], "int")],
        vec![],
    );
    let sj = class_with("z.c.SJ", &["z.a.SA", "z.b.SB"], vec![], vec![]);
    ir_package(
        "z",
        vec![
            ir_submodule("z.a", vec![sa]),
            ir_submodule("z.b", vec![sb]),
            ir_submodule("z.c", vec![sj]),
        ],
    )
}

#[test]
fn external_ir_cross_module_diamond_static_winner_warns_not_callable() {
    let bindings = Codegen::new(Config::default())
        .modules_from_ir(SchemaIr {
            modules: vec![static_winner_diamond_package()],
        })
        .expect("static-winner diamond IR must build")
        .generate_with_report()
        .expect("static-winner diamond must generate");

    assert!(
        bindings
            .report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "class_forwarder_winner_not_callable"),
        "a cross-module diamond whose C3-nearest declarer is a static method must warn: {:?}",
        bindings.report.diagnostics,
    );

    assert_source_compiles(&bindings.tokens.to_string(), "static_winner");
}
