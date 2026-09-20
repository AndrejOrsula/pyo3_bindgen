//! Generic, language-neutral intermediate representation (IR) for driving Rust
//! binding generation from a pre-built schema instead of live Python introspection.
//!
//! These types are deliberately **generic**: they use only `String`/`bool`/`Vec`/
//! `Option` primitives plus a handful of small `enum`s. They carry **zero** knowledge
//! of any particular Python package, framework, or domain — a caller translates its
//! own source of truth into this IR and feeds it to [`crate::Codegen::modules_from_ir`].
//!
//! This module is the serialized input contract, not the code-generation domain
//! model. Validation and lowering convert these wire-facing values into the private
//! `syntax` model before generation. Source selection and loading live separately in
//! [`crate::BindingSource`].
//!
//! Type annotations travel as plain Python-style strings (e.g. `"list[int]"`,
//! `"pkg.Class"`, `"X | None"`) and are parsed at the `from_ir` boundary via
//! [`std::str::FromStr`] for [`crate::typing`]'s `Type`, reusing the exact same
//! fallback the live introspection path already relies on.

use serde::{Deserialize, Serialize};

/// Root wrapper for an externally supplied schema.
///
/// First-seen module and member order is preserved. Dotted top-level names synthesize
/// missing parent packages, and siblings sharing a dotted root merge into one emitted
/// parent while retaining caller order within that merged tree.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SchemaIr {
    /// Top-level modules, in caller-supplied order.
    pub modules: Vec<ModuleIr>,
}

/// A Python-style module (or package) to generate bindings for.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ModuleIr {
    /// Fully-qualified dotted module name (e.g. `"pkg.sub"`).
    pub name: String,
    /// Optional module docstring.
    #[serde(default)]
    pub docstring: Option<String>,
    /// Whether the module is a package that can contain submodules.
    pub is_package: bool,
    /// Classes defined directly in this module.
    #[serde(default)]
    pub classes: Vec<ClassIr>,
    /// Enums defined directly in this module.
    #[serde(default)]
    pub enums: Vec<EnumIr>,
    /// Free functions defined directly in this module.
    #[serde(default)]
    pub functions: Vec<FunctionIr>,
    /// Module-level properties (attributes).
    #[serde(default)]
    pub properties: Vec<PropertyIr>,
    /// Nested submodules.
    #[serde(default)]
    pub submodules: Vec<ModuleIr>,
    /// Optional prelude re-export identifiers. Entries must be non-empty,
    /// non-dotted, and unique. When prelude generation is enabled, each entry must
    /// resolve to an enabled direct member of this module, including an emitted
    /// [`ImportIr`] target.
    #[serde(default)]
    pub prelude: Vec<String>,
    /// Optional explicit imports (cross-module re-exports).
    #[serde(default)]
    pub imports: Vec<ImportIr>,
    /// Optional type-variable names.
    #[serde(default)]
    pub type_vars: Vec<String>,
}

/// A class to emit as a Python-object newtype wrapper.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ClassIr {
    /// Fully-qualified dotted class name (e.g. `"pkg.sub.MyClass"`).
    pub name: String,
    /// Direct base-class dotted names, in Python declaration order (empty for a root).
    ///
    /// Every entry must resolve to another `ClassIr` in scope. The engine rejects
    /// cycles and non-class targets, then expands these direct edges into the full
    /// nearest-first transitive closure used by supertraits, ancestor impls, and
    /// zero-cost upcasts.
    #[serde(default)]
    pub base_classes: Vec<String>,
    /// Optional class docstring.
    #[serde(default)]
    pub docstring: Option<String>,
    /// Methods of the class. Each method's owning class path is supplied
    /// automatically from this class — the IR author need not repeat it.
    #[serde(default)]
    pub methods: Vec<FunctionIr>,
    /// Instance/class properties.
    #[serde(default)]
    pub properties: Vec<PropertyIr>,
}

/// A function or method.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FunctionIr {
    /// Fully-qualified dotted function name.
    pub name: String,
    /// What kind of callable this is.
    pub kind: FunctionKindIr,
    /// Parameters, in Python declaration order. Names must be non-empty, unique,
    /// and representable as generated Rust parameters. Kinds must follow
    /// `inspect.Parameter` ordering with at most one `*args` and one `**kwargs`;
    /// variadics cannot have defaults. Instance methods, constructors, and callable
    /// methods must include a leading positional `self` or `cls`; that receiver is
    /// validated and stripped. Class methods use the signature exposed by Python's
    /// already-bound descriptor: their implicit class receiver must be omitted, and
    /// every declared parameter remains part of the generated Rust ABI. This includes
    /// a first declared parameter literally named `self` or `cls`, which may be an
    /// ordinary argument in a valid bound signature. Static methods likewise have no
    /// implicit receiver, so `self`/`cls` are permitted as ordinary parameter names
    /// and are never stripped.
    #[serde(default)]
    pub params: Vec<ParameterIr>,
    /// Python-style return annotation string (fed to `Type::from_str`).
    #[serde(default)]
    pub return_type: String,
    /// Optional docstring.
    #[serde(default)]
    pub docstring: Option<String>,
}

/// Discriminates the kind of a [`FunctionIr`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FunctionKindIr {
    /// A free function.
    Function,
    /// A method bound to a class.
    Method {
        /// The method's binding kind.
        method_type: MethodKindIr,
    },
    /// A closure-like callable.
    Closure,
}

/// Mirrors the engine's internal method-binding kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MethodKindIr {
    /// Instance method. IR parameters begin with positional `self` or `cls`.
    Instance,
    /// Bound class method. The implicit class receiver is omitted; all IR parameters
    /// are explicit and preserved regardless of name.
    Class,
    /// Static method. Every IR parameter is explicit, regardless of its name.
    Static,
    /// Constructor (`__init__`). IR parameters begin with positional `self` or `cls`.
    Constructor,
    /// Callable (`__call__`). IR parameters begin with positional `self` or `cls`.
    Callable,
    /// Unknown (let the engine apply its fallback heuristics).
    Unknown,
}

/// A function/method parameter.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ParameterIr {
    /// Parameter name.
    pub name: String,
    /// Parameter passing kind.
    pub kind: ParameterKindIr,
    /// Python-style type annotation string (fed to `Type::from_str`).
    #[serde(default)]
    pub annotation: String,
    /// Whether the parameter has a default value.
    #[serde(default)]
    pub has_default: bool,
}

/// Mirrors the engine's internal parameter kinds (Python `inspect` semantics).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ParameterKindIr {
    /// Positional-only.
    PositionalOnly,
    /// Positional-or-keyword.
    PositionalOrKeyword,
    /// `*args`.
    VarPositional,
    /// Keyword-only.
    KeywordOnly,
    /// `**kwargs`.
    VarKeyword,
}

/// A module-level or class-level property (attribute).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PropertyIr {
    /// Property name.
    pub name: String,
    /// Python-style getter type annotation string (fed to `Type::from_str`).
    pub py_type: String,
    /// Optional setter type annotation; defaults to `py_type` when absent.
    #[serde(default)]
    pub setter_type: Option<String>,
    /// Whether the property is read-only (no setter is emitted).
    pub readonly: bool,
    /// Optional getter docstring.
    #[serde(default)]
    pub docstring: Option<String>,
    /// Optional setter docstring.
    #[serde(default)]
    pub setter_docstring: Option<String>,
}

/// An enum to mirror as a Rust enum.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EnumIr {
    /// Fully-qualified dotted enum name.
    pub name: String,
    /// Enum backing kind.
    pub kind: EnumKindIr,
    /// Canonical variants, in order.
    #[serde(default)]
    pub variants: Vec<EnumVariantIr>,
    /// Aliases pointing to canonical variants.
    #[serde(default)]
    pub aliases: Vec<EnumAliasIr>,
    /// Optional docstring.
    #[serde(default)]
    pub docstring: Option<String>,
    /// Flag-style enum marker (e.g. `Flag`/`IntFlag`). Strict external IR requires
    /// [`EnumIr::plain_mirror`]; lenient IR accepts the same lossy plain-mirror lowering globally.
    #[serde(default)]
    pub is_flag: bool,
    /// Open/runtime-populated enum marker. Strict external IR requires [`EnumIr::plain_mirror`];
    /// lenient IR accepts the same lossy plain-mirror lowering globally.
    #[serde(default)]
    pub is_open: bool,
    /// Mirror this enum's Python representation as its member STRING (e.g. Blender
    /// RNA enum properties) instead of importing a Python enum class. Per-enum
    /// equivalent of [`crate::Config::enum_ir_string_backed`]; either source enables it.
    ///
    /// Skipped when `false` so that a document authored before this field existed
    /// re-serializes byte-for-byte as it did then. `Module::from_ir` derives the
    /// `PYO3_BINDGEN_INPUT_HASH` provenance constant from the re-serialized IR, so an
    /// unconditionally emitted `"string_backed": false` would silently rewrite that hash for
    /// every pre-existing enum-bearing document. A `true` value still perturbs the hash --
    /// correctly, since it changes lowering.
    #[serde(default, skip_serializing_if = "core::ops::Not::not")]
    pub string_backed: bool,
    /// Explicit producer opt-in to accept `is_flag`/`is_open` under strict external-IR
    /// validation and lower the enum as a plain mirror enum with the flag/open semantics
    /// deliberately dropped. Without it, strict validation rejects `is_flag`/`is_open`;
    /// lenient external IR is unaffected either way.
    ///
    /// Skipped when `false` for the same input-hash byte-neutrality reason as
    /// [`EnumIr::string_backed`].
    #[serde(default, skip_serializing_if = "core::ops::Not::not")]
    pub plain_mirror: bool,
}

/// Mirrors the engine's internal enum backing kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EnumKindIr {
    /// Plain `enum.Enum`.
    Enum,
    /// `enum.IntEnum`.
    IntEnum,
    /// `enum.StrEnum`.
    StrEnum,
}

/// A canonical enum variant.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EnumVariantIr {
    /// Python member identifier.
    pub py_name: String,
    /// `repr()` of the member value (used for documentation).
    #[serde(default)]
    pub value_repr: String,
}

/// An enum alias mapping a non-canonical name to a canonical variant.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EnumAliasIr {
    /// Python member identifier of the alias.
    pub py_name: String,
    /// Python member identifier of the canonical variant it targets.
    pub target_py_name: String,
    /// `repr()` of the value (used for documentation).
    #[serde(default)]
    pub value_repr: String,
}

/// An explicit cross-module import / re-export.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ImportIr {
    /// Origin dotted path of the imported item.
    pub origin: String,
    /// Target dotted path where it is re-exported.
    pub target: String,
}
