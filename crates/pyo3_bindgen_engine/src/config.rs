use crate::syntax::{Ident, Path};

/// Array of forbidden type names
pub const FORBIDDEN_TYPE_NAMES: [&str; 7] = [
    "_collections._tuplegetter",
    "AsyncState",
    "getset_descriptor",
    "member_descriptor",
    "method_descriptor",
    "property",
    "py",
];

/// Default array of blocklisted attribute names
const DEFAULT_BLOCKLIST_ATTRIBUTE_NAMES: [&str; 4] = ["builtins", "testing", "tests", "test"];

/// Configuration for `Codegen` engine.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, typed_builder::TypedBuilder)]
pub struct Config {
    /// Flag that determines whether to recursively generate code for all submodules of the target modules.
    #[builder(default = true)]
    pub traverse_submodules: bool,

    /// Flag that determines whether to generate code for prelude modules (Python `__all__` attribute).
    #[builder(default = true)]
    pub generate_preludes: bool,
    /// Flag that determines whether to generate code for imports.
    #[builder(default = true)]
    pub generate_imports: bool,
    /// Flag that determines whether to generate code for classes.
    #[builder(default = true)]
    pub generate_classes: bool,
    /// Flag that determines whether to generate enum-derived bindings.
    ///
    /// When disabled, neither Rust mirror enums nor owning-class fallbacks are emitted in
    /// live-introspection or external-IR mode. Ordinary classes remain enabled according to
    /// [`Config::generate_classes`].
    #[builder(default = true)]
    pub generate_enums: bool,
    /// Exact Python enum paths that should keep using owning Python object wrappers instead of Rust mirror enums.
    ///
    /// Only applies to the live-introspection path. On the external-IR path each enabled enum
    /// in the supplied schema is emitted as a Rust mirror enum; to keep an owning wrapper there,
    /// model the type as a `ClassIr` rather than an `EnumIr` (`Module::from_ir` does not consult
    /// this list). To keep a *non-representable* external-IR enum from aborting generation, see
    /// `enum_ir_class_fallback`.
    #[builder(default)]
    pub enum_raw_fallback_paths: Vec<String>,
    /// External-IR path only: when an `EnumIr` cannot be represented as a Rust mirror enum
    /// (unrepresentable or colliding member/alias idents, a dangling alias target, or no
    /// members), emit an owning Python object wrapper for it instead of aborting the whole
    /// generation. Mirrors the live path's enum fallback (`Module::parse`), but is reported
    /// under the distinct `enum_ir_mirror_fallback` code so consumers can tell the two apart.
    ///
    /// Off by default: a mirror enum silently degrading to an opaque newtype is a public-API
    /// change (every signature that named the enum lowers to the newtype, and `set[E]`
    /// degrades from `HashSet<E>` to `Bound<PySet>`) that must be requested explicitly.
    /// Deliberately NOT folded into `lenient_external_ir`. Has no effect on the
    /// live-introspection path, which already falls back rather than aborting.
    #[builder(default = false)]
    pub enum_ir_class_fallback: bool,
    /// External-IR path only: treat mirror enums whose Python representation is the member
    /// STRING (e.g. Blender RNA enum properties) rather than a Python `enum.Enum` class.
    ///
    /// RNA-derived "enums" are plain string-valued `bpy` properties -- `obj.rotation_mode`
    /// is the `str` `'XYZ'`, and there is no backing `bpy.types.Object_rotation_mode` class
    /// to import. When set, mirror enums round-trip via their string value
    /// (`as_py_name`/`from_py_name`) instead of importing a Python enum class and reading its
    /// `.name`, so the generated runtime conversion does not fail with an `AttributeError`
    /// for a class that does not exist.
    ///
    /// This is the coarse, whole-run override: it flips every mirror enum in the document at
    /// once. The first-class mechanism is the per-enum [`crate::schema::EnumIr::string_backed`]
    /// field on the IR document itself, which lets a producer declare it enum-by-enum; the two
    /// are combined with a logical OR (either source enables string-backing for a given enum).
    ///
    /// Off by default: every other consumer's enums are real Python enum classes, so the
    /// class-import mirror is correct there and the default preserves it byte-for-byte. Has
    /// no effect on the live-introspection path.
    #[builder(default = false)]
    pub enum_ir_string_backed: bool,
    /// Flag that determines whether to generate code for type variables.
    #[builder(default = true)]
    pub generate_type_vars: bool,
    /// Flag that determines whether to generate code for module functions and class methods.
    #[builder(default = true)]
    pub generate_functions: bool,
    /// Flag that determines whether to generate code for module and class properties.
    #[builder(default = true)]
    pub generate_properties: bool,
    /// Flag that determines whether to documentation for the generate code.
    /// The documentation is based on Python docstrings.
    #[builder(default = true)]
    pub generate_docs: bool,
    /// Flag that determines whether to embed lightweight provenance metadata in generated modules.
    #[builder(default = true)]
    pub generate_provenance: bool,

    /// List of blocklisted attribute names that are skipped during the code generation.
    ///
    /// Only applies to the live-introspection path; the external-IR path emits exactly
    /// the members present in the supplied schema.
    #[builder(default = DEFAULT_BLOCKLIST_ATTRIBUTE_NAMES.iter().map(|&s| s.to_string()).collect())]
    pub blocklist_names: Vec<String>,
    /// Flag that determines whether private attributes are considered while parsing the Python code.
    ///
    /// Only applies to the live-introspection path; the external-IR path emits exactly
    /// the members present in the supplied schema.
    #[builder(default = false)]
    pub include_private: bool,

    /// When enabled (the default), inherited members are deduplicated across the class
    /// hierarchy on the live-introspection path: each class that participates in in-scope
    /// inheritance carries only its own members and inherits the rest through `*Methods`
    /// supertrait bounds (the compact external-IR object model), while classes that cannot
    /// benefit stay on the flat model. Deduplication is therefore a strict size win -- it
    /// never enlarges the output and shrinks it dramatically for deep hierarchies. When
    /// disabled, every inherited member is redeclared on each subclass (the historical
    /// behavior).
    ///
    /// Only applies to the live-introspection path; the external-IR path always emits the
    /// deduplicated model.
    ///
    /// Same-named methods in multiple ancestors are resolved through the global class
    /// registry, including cross-module diamonds, and receive an inherent forwarder to
    /// Python's C3-nearest winner when that winner is a callable generated trait member.
    #[builder(default = true)]
    pub deduplicate_inherited_members: bool,

    /// When enabled (the default), each newtype on the deduplicated object model carries an
    /// inherent `pub fn` forwarder per callable own/inherited trait method, so the name resolves
    /// to a single inherent item and stays directly callable (`obj.name(py)`) without importing
    /// the owning `*Methods` trait, and diamonds bind unambiguously across modules for concrete
    /// generated-newtype receivers. Generic code whose receiver is only `T: *Methods` cannot
    /// use the concrete type's inherent forwarder; same-named supertrait methods must be selected
    /// explicitly with UFCS (`<T as WinnerMethods>::name(...)`).
    ///
    /// When disabled, those forwarders are omitted: own and inherited members are reachable only
    /// through the `*Methods` traits (bring the trait into scope, then call `obj.name(py)` or
    /// `<T as CMethods>::name(&obj, py)`). This drops one `pub fn` per method -- a large size
    /// reduction for wide hierarchies -- in exchange for the direct-inherent-call ergonomics and
    /// the automatic diamond disambiguation.
    ///
    /// Applies wherever the deduplicated object model is emitted: the external-IR path (always)
    /// and the deduplicated live-introspection path. A flat live class has no `*Methods` trait
    /// indirection and is unaffected.
    #[builder(default = true)]
    pub generate_inherent_forwarders: bool,

    /// External-IR path only. When enabled, the schema validator accepts a flag (`is_flag`) or open
    /// (`is_open`) enum without a per-enum `plain_mirror` opt-in. Such an enum is emitted as a plain
    /// Rust mirror enum with the flag/open semantics dropped: a single member is representable,
    /// while bitwise combinations and unknown future values are not.
    ///
    /// Disabled by default: the strict path refuses that lossy shape. A required positional
    /// parameter after a defaulted one is accepted in both modes because defaults do not affect the
    /// emitted Rust signature. Has no effect on the live-introspection path.
    #[builder(default = false)]
    pub lenient_external_ir: bool,

    /// Flag that determines whether to also generate code for the dependencies of the target
    /// modules. The dependent modules are discovered from the (external) imports of the targets.
    ///
    /// Best-effort: a dependency whose module cannot be imported is skipped rather than aborting
    /// generation, and cross-package re-export coverage is incomplete — treat generated dependency
    /// bindings as a convenience, not a guarantee. Only applies to the live-introspection path.
    #[builder(default = false)]
    pub generate_dependencies: bool,

    /// Flag that makes live generation fail when submodule or dependency discovery is
    /// incomplete. Best-effort discovery remains the default; this option has no effect on
    /// the external-IR path.
    #[builder(default = false)]
    pub strict_completeness: bool,

    /// Flag that suppresses the generation of Python STDOUT while parsing the Python code.
    #[builder(default = true)]
    pub suppress_python_stdout: bool,
    /// Flag that suppresses the generation of Python STDERR while parsing the Python code.
    #[builder(default = true)]
    pub suppress_python_stderr: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self::builder().build()
    }
}

impl Config {
    pub(crate) fn is_attr_allowed(
        &self,
        attr_name: &Ident,
        attr_module: &Path,
        _attr_type: &pyo3::Bound<pyo3::types::PyType>,
    ) -> bool {
        self.is_name_allowed(attr_name) && self.is_module_allowed(attr_module)
    }

    /// Name-only half of [`Config::is_attr_allowed`]: the rejections that depend solely on
    /// the attribute's own name -- a private `_` prefix (when `include_private` is disabled)
    /// or a blocklisted name. Split out so a caller can discard an attribute -- the majority
    /// on a class, dominated by dunders -- by name alone, before paying for the
    /// `__module__` lookup and type object the module-path checks would otherwise require.
    pub(crate) fn is_name_allowed(&self, attr_name: &Ident) -> bool {
        if !self.include_private && attr_name.as_py().starts_with('_') {
            return false;
        }
        !self
            .blocklist_names
            .iter()
            .any(|blocklist_match| attr_name.as_py() == blocklist_match)
    }

    /// Module-path half of [`Config::is_attr_allowed`]: the rejections that depend on the
    /// attribute type's defining module -- a private segment (when `include_private` is
    /// disabled), or a `__future__`/`typing` segment. Apply only after [`Config::is_name_allowed`].
    pub(crate) fn is_module_allowed(&self, attr_module: &Path) -> bool {
        if !self.include_private
            && attr_module
                .iter()
                .any(|segment| segment.as_py().starts_with('_'))
        {
            return false;
        }
        !attr_module
            .iter()
            .any(|segment| segment.as_py() == "__future__" || segment.as_py() == "typing")
    }
}
