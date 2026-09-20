use super::{Ident, Path};
use crate::{PyBindgenError, Result};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};
use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;

/// Canonical spelling rustc uses when comparing Unicode identifiers.
pub(crate) fn canonical_rust_name(name: &str) -> String {
    // A raw identifier (`r#foo`) names the SAME symbol as its bare spelling `foo` to
    // rustc, so strip a leading `r#` before comparison. Without this, the `*Methods`
    // trait of a class whose name is a raw-escapable keyword (`match` -> `r#match` ->
    // trait `r#matchMethods`) and a sibling type literally named `matchMethods` land in
    // different symbol-table buckets yet collide at compile time (E0428) -- a silent
    // miscompile the string comparison misses. `#` is not a valid identifier character
    // (Python XID or generated), so the only strings reaching here with an `r#` prefix
    // are genuine raw-keyword idents; stripping it can never merge two legitimately
    // distinct symbols.
    let name = name.strip_prefix("r#").unwrap_or(name);
    // An ASCII string is already in NFC (no character decomposes or recomposes), so skip
    // building the normalization iterator on the overwhelmingly common ASCII-identifier
    // case -- this runs on every symbol-table operation (reserve/claim/allocate/lookup).
    if name.is_ascii() {
        name.to_owned()
    } else {
        name.nfc().collect()
    }
}

/// Rust item role used to select the stable fallback prefix for an invalid Python name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SymbolKind {
    Module,
    Class,
    Enum,
    Function,
    TypeVar,
}

impl SymbolKind {
    pub(crate) const fn is_value(self) -> bool {
        matches!(self, Self::Function)
    }
}

impl SymbolKind {
    const fn fallback_prefix(self) -> &'static str {
        match self {
            Self::Module => "m_",
            Self::Class => "s_",
            Self::Enum => "e_",
            Self::Function => "f_",
            Self::TypeVar => "t_",
        }
    }
}

/// Convert one Python identifier into the canonical Rust identifier for its role.
///
/// Definitions, references, and collision validation must all cross this seam so a
/// fallback rename cannot drift between them.
pub(crate) fn rust_ident(name: &Ident, kind: SymbolKind) -> Option<syn::Ident> {
    if let Ok(ident) = name.try_into() {
        return Some(ident);
    }

    let sanitized = name
        .as_py()
        .replace(|character: char| !character.is_alphanumeric(), "_");
    Ident::from_py(&format!("{}{sanitized}", kind.fallback_prefix()))
        .try_into()
        .ok()
}

/// Resolve a Python path to the path emitted in Rust.
///
/// Every prefix segment names a generated module; only the final segment uses the
/// caller-provided item kind. Keeping definition and reference lowering behind this
/// seam prevents a sanitized definition (for example, `pkg.Self` -> `pkg::s_Self`)
/// from being referenced under its raw, invalid Python spelling.
pub(crate) fn rust_path(path: &Path, final_kind: SymbolKind) -> Option<Path> {
    let final_index = path.len().checked_sub(1)?;
    let segments = path
        .iter()
        .enumerate()
        .map(|(index, segment)| {
            let kind = if index == final_index {
                final_kind
            } else {
                SymbolKind::Module
            };
            rust_ident(segment, kind).map(|ident| Ident::from_rs(&ident.to_string()))
        })
        .collect::<Option<Vec<_>>>()?;
    Some(Path::from(segments.as_slice()))
}

/// Wrapper-boilerplate method names carried by every generated class wrapper.
///
/// On the flat model all five are inherent methods on the newtype. On the
/// deduplicated model (D3) only `from_py_any` and `into_py_any` stay inherent (the
/// latter because pyo3's `IntoPyObjectExt` blanket impl supplies the same name, so a
/// trait-level copy would be ambiguous); the other three are `#[inline]` default
/// methods on the shared root/marker traits the newtype implements. Either way the
/// name is taken, so the reservation is unconditional --
/// a Python member with one of these names is renamed under both models.
pub(crate) const INHERENT_CLASS_METHODS: [&str; 5] =
    ["as_any", "bind", "clone_ref", "from_py_any", "into_py_any"];

pub(crate) const PROVENANCE_VALUES: [&str; 3] = [
    "PYO3_BINDGEN_GENERATOR_VERSION",
    "PYO3_BINDGEN_MODULE_NAME",
    "PYO3_BINDGEN_INPUT_HASH",
];

pub(crate) const EMBED_SOURCE_FUNCTION: &str = "pyo3_embed_python_source_code";

pub(crate) fn conflicts_with_inherent_class_method(name: &Ident) -> bool {
    INHERENT_CLASS_METHODS.contains(&name.as_py())
}

/// Method/const names carried by the always-implemented root and marker traits
/// (`PyObjectBase` / `PyTypeMarker`) on the external-IR path. A Python member with one
/// of these names would shadow or be ambiguous with the generic handle/downcast API for
/// a concrete newtype, so on that path they are reserved and renamed exactly like
/// [`INHERENT_CLASS_METHODS`]. These traits are not emitted on the live path, so the
/// reservation is applied there only.
pub(crate) const RESERVED_ROOT_TRAIT_METHODS: [&str; 6] = [
    "__pyo3_obj",
    "try_downcast",
    "downcast",
    "__from_obj",
    "__into_obj",
    "PY_CLASS",
];

pub(crate) fn conflicts_with_root_trait_method(name: &Ident) -> bool {
    RESERVED_ROOT_TRAIT_METHODS.contains(&name.as_py())
}

/// Convert a Python enum member into the canonical UpperCamelCase Rust variant.
///
/// Python identifiers are Unicode (PEP 3131) and so are Rust's, so the word
/// characters kept here are the Unicode-alphanumeric ones -- the same class
/// [`rust_ident`] sanitizes against. Restricting them to ASCII would silently drop
/// the non-ASCII letters of a member such as `café` (yielding a truncated variant)
/// or erase an entirely non-ASCII member down to nothing. Anything the case
/// mapping still leaves invalid as a Rust identifier is rejected by the `syn` parse
/// below, exactly as before.
pub(crate) fn enum_variant_ident(python_name: &str) -> Option<Ident> {
    let mut rust_name = String::new();
    let mut uppercase_next = true;
    // Normalize to NFC *before* filtering. A decomposed member (e.g. `e` + U+0301)
    // carries its accent as a separate combining mark, which is not alphanumeric and
    // would be dropped by the filter below -- silently turning `café` into `Cafe` and
    // colliding it with a distinct `cafe` member. Recomposing first keeps the accent
    // on its base letter, so composed and decomposed spellings yield one variant.
    for character in python_name.nfc() {
        if character.is_alphanumeric() {
            if rust_name.is_empty() && character.is_numeric() {
                // A Rust identifier may not start with a digit.
                rust_name.push('V');
            }
            if uppercase_next {
                rust_name.extend(character.to_uppercase());
                uppercase_next = false;
            } else {
                rust_name.extend(character.to_lowercase());
            }
        } else {
            uppercase_next = true;
        }
    }

    if rust_name.is_empty() {
        return None;
    }

    // Emit the spelling rustc itself would canonicalize to, so that two members
    // differing only in Unicode composition are one and the same variant name here
    // -- and are therefore caught by the caller's collision check rather than by
    // rustc, after the fact, as a duplicate variant. (Case mapping a Unicode letter
    // can denormalize, so this NFC pass is still needed after the loop.)
    let ident = Ident::from_py(&canonical_rust_name(&rust_name));
    if syn::parse_str::<syn::Ident>(ident.as_rs()).is_ok() {
        return Some(ident);
    }

    // The one non-empty, case-mapped spelling `syn` still rejects is the reserved
    // word `Self` (a member literally named `self`/`Self`/`SELF`), which has no raw
    // identifier form. Prefix a disambiguator so such a member yields a renamed
    // variant instead of aborting the whole enum. Genuinely unrepresentable names
    // (alphanumeric-but-non-XID such as `²`) still return `None`, as before.
    let prefixed = Ident::from_py(&canonical_rust_name(&format!("V{rust_name}")));
    syn::parse_str::<syn::Ident>(prefixed.as_rs()).ok()?;
    Some(prefixed)
}

/// Owned, public representation of one planned enum variant.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EnumVariantPlan {
    pub python_name: String,
    pub rust_name: String,
}

pub(crate) fn enum_variant_ident_string(python_name: &str) -> Option<String> {
    enum_variant_ident(python_name).map(|ident| ident.to_string())
}

pub(crate) fn plan_enum_variants(python_names: &[String]) -> Vec<EnumVariantPlan> {
    let mut used_rust_names = HashSet::default();

    python_names
        .iter()
        .filter_map(|python_name| {
            let rust_name = enum_variant_ident_string(python_name)?;
            if !used_rust_names.insert(canonical_rust_name(&rust_name)) {
                return None;
            }
            Some(EnumVariantPlan {
                python_name: python_name.clone(),
                rust_name,
            })
        })
        .collect()
}

/// Collision-checking symbol table for one Rust namespace.
#[derive(Debug)]
pub(crate) struct SymbolTable {
    namespace: &'static str,
    symbols: HashMap<String, String>,
}

impl SymbolTable {
    pub(crate) fn new(namespace: &'static str) -> Self {
        Self {
            namespace,
            symbols: HashMap::default(),
        }
    }

    pub(crate) fn register(&mut self, rust_name: &str, origin: impl Into<String>) -> Result<()> {
        let origin = origin.into();
        let canonical_name = canonical_rust_name(rust_name);
        if let Some(previous) = self.symbols.insert(canonical_name.clone(), origin.clone()) {
            return Err(PyBindgenError::CodegenError(format!(
                "Rust {} symbol collision '{canonical_name}' between '{previous}' and '{origin}'",
                self.namespace
            )));
        }
        Ok(())
    }
}

/// Per-Rust-namespace allocator for generated and aliased names.
///
/// Python-authored names are reserved up front. Synthetic names then receive the first
/// available deterministic suffix (`new`, `new1`, ...), and every allocation is retained
/// so later properties/methods cannot reuse it.
#[derive(Debug, Default)]
pub(crate) struct NameAllocator {
    occupied: HashSet<String>,
    generated: HashSet<String>,
    resolved: HashMap<String, syn::Ident>,
}

impl NameAllocator {
    pub(crate) fn from_python_names<'a>(
        names: impl IntoIterator<Item = &'a Ident>,
        kind: SymbolKind,
    ) -> Self {
        let mut allocator = Self::default();
        for name in names {
            if let Some(ident) = rust_ident(name, kind) {
                allocator.reserve(&ident.to_string());
            }
        }
        allocator
    }

    pub(crate) fn reserve_inherent_class_methods(&mut self) {
        for name in INHERENT_CLASS_METHODS {
            self.reserve_generated(name);
        }
    }

    /// Reserve the root/marker trait method names. External-IR path only -- those
    /// traits are not emitted on the live path. See [`RESERVED_ROOT_TRAIT_METHODS`].
    pub(crate) fn reserve_root_trait_methods(&mut self) {
        for name in RESERVED_ROOT_TRAIT_METHODS {
            self.reserve_generated(name);
        }
    }

    pub(crate) fn reserve_generated_module_values(
        &mut self,
        provenance: bool,
        embedded_source: bool,
    ) {
        if provenance {
            for name in PROVENANCE_VALUES {
                self.reserve_generated(name);
            }
        }
        if embedded_source {
            self.reserve_generated(EMBED_SOURCE_FUNCTION);
        }
    }

    pub(crate) fn reserve_ident(&mut self, ident: &syn::Ident) {
        self.reserve(&ident.to_string());
    }

    fn reserve(&mut self, name: &str) {
        self.occupied.insert(canonical_rust_name(name));
    }

    fn reserve_generated(&mut self, name: &str) {
        self.reserve(name);
        self.generated.insert(canonical_rust_name(name));
    }

    pub(crate) fn is_generated(&self, name: &Ident) -> bool {
        self.generated.contains(&canonical_rust_name(name.as_rs()))
    }

    pub(crate) fn record_resolution(&mut self, python_name: &Ident, rust_ident: &syn::Ident) {
        self.resolved
            .insert(python_name.as_py().to_owned(), rust_ident.clone());
    }

    pub(crate) fn resolved(&self, python_name: &Ident) -> Option<syn::Ident> {
        self.resolved.get(python_name.as_py()).cloned()
    }

    /// Every `(Python name, emitted Rust identifier)` resolution recorded so far.
    ///
    /// Keeping the origin name alongside the emitted identifier lets class-hierarchy
    /// validation distinguish an ordinary override of the SAME Python member from two
    /// DISTINCT Python members that rustc normalizes to one method identifier.
    pub(crate) fn resolved_members(&self) -> Vec<(String, String)> {
        let mut resolved = self
            .resolved
            .iter()
            .map(|(python_name, ident)| (python_name.clone(), ident.to_string()))
            .collect::<Vec<_>>();
        resolved.sort_unstable();
        resolved
    }

    pub(crate) fn validate_resolutions(&self) -> Result<()> {
        self.validate_resolutions_with(&[])
    }

    pub(crate) fn validate_resolutions_with(
        &self,
        additional: &[(syn::Ident, String)],
    ) -> Result<()> {
        let mut symbols = SymbolTable::new("value");
        let mut generated = self.generated.iter().collect::<Vec<_>>();
        generated.sort_unstable();
        for name in generated {
            symbols.register(name, "generated support")?;
        }

        let mut resolved = self.resolved.iter().collect::<Vec<_>>();
        resolved.sort_unstable_by_key(|(python_name, _)| *python_name);
        for (python_name, rust_ident) in resolved {
            symbols.register(&rust_ident.to_string(), python_name)?;
        }
        for (rust_ident, origin) in additional {
            symbols.register(&rust_ident.to_string(), origin)?;
        }
        Ok(())
    }

    pub(crate) fn claim(&mut self, ident: &syn::Ident) -> bool {
        self.occupied
            .insert(canonical_rust_name(&ident.to_string()))
    }

    pub(crate) fn allocate_prefixed(
        &mut self,
        prefix: &str,
        python_name: &Ident,
    ) -> Option<syn::Ident> {
        let ident = rust_ident(python_name, SymbolKind::Function)?;
        let fragment = ident.to_string();
        let fragment = fragment.strip_prefix("r#").unwrap_or(&fragment);
        Some(self.allocate(&format!("{prefix}{fragment}")))
    }

    pub(crate) fn allocate(&mut self, preferred: &str) -> syn::Ident {
        let mut suffix = 0usize;
        loop {
            let candidate = if suffix == 0 {
                preferred.to_owned()
            } else {
                format!("{preferred}{suffix}")
            };
            let ident: syn::Ident = Ident::from_py(&candidate)
                .try_into()
                .expect("internally generated Rust identifier must be valid");
            if self
                .occupied
                .insert(canonical_rust_name(&ident.to_string()))
            {
                return ident;
            }
            suffix += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocation_reserves_python_names_and_prior_results() {
        let existing = [Ident::from_py("new"), Ident::from_py("new1")];
        let mut names = NameAllocator::from_python_names(existing.iter(), SymbolKind::Function);

        assert_eq!(names.allocate("new").to_string(), "new2");
        assert_eq!(names.allocate("new").to_string(), "new3");
    }

    #[test]
    fn resolutions_distinguish_literal_raw_prefix_from_keyword_escape() {
        let keyword = Ident::from_py("type");
        let literal_raw_prefix = Ident::from_py("r#type");
        assert_eq!(keyword.as_rs(), literal_raw_prefix.as_rs());
        assert_ne!(keyword.as_py(), literal_raw_prefix.as_py());

        let mut names = NameAllocator::default();
        names.record_resolution(&keyword, &syn::parse_str("get_type").unwrap());
        names.record_resolution(
            &literal_raw_prefix,
            &syn::parse_str("get_literal_raw_type").unwrap(),
        );

        assert_eq!(names.resolved(&keyword).unwrap().to_string(), "get_type");
        assert_eq!(
            names.resolved(&literal_raw_prefix).unwrap().to_string(),
            "get_literal_raw_type"
        );
        names.validate_resolutions().unwrap();
    }

    #[test]
    fn emitted_name_reservations_use_unicode_nfc() {
        let mut symbols = SymbolTable::new("value");
        symbols.register("é", "precomposed").unwrap();
        let error = symbols
            .register("e\u{301}", "decomposed")
            .expect_err("rustc treats canonically equivalent identifiers as equal");
        assert!(error.to_string().contains("symbol collision 'é'"));

        let existing = [Ident::from_py("e\u{301}")];
        let mut names = NameAllocator::from_python_names(existing.iter(), SymbolKind::Function);
        assert_eq!(names.allocate("é").to_string(), "é1");
    }

    #[test]
    fn canonical_sanitization_depends_only_on_symbol_role() {
        let invalid = Ident::from_py("not-valid");

        assert_eq!(
            rust_ident(&invalid, SymbolKind::Module)
                .unwrap()
                .to_string(),
            "m_not_valid"
        );
        assert_eq!(
            rust_ident(&invalid, SymbolKind::Function)
                .unwrap()
                .to_string(),
            "f_not_valid"
        );
    }

    #[test]
    fn enum_variants_use_upper_camel_case() {
        assert_eq!(enum_variant_ident("RED").unwrap().as_rs(), "Red");
        assert_eq!(enum_variant_ident("rgb_array").unwrap().as_rs(), "RgbArray");
        assert_eq!(enum_variant_ident("2d").unwrap().as_rs(), "V2d");
    }

    /// Python identifiers are Unicode (PEP 3131); non-ASCII letters must survive
    /// instead of being silently dropped from the variant name.
    #[test]
    fn enum_variants_keep_unicode_letters() {
        assert_eq!(enum_variant_ident("café").unwrap().as_rs(), "Café");
        assert_eq!(
            enum_variant_ident("café_au_lait").unwrap().as_rs(),
            "CaféAuLait"
        );
        assert_eq!(enum_variant_ident("日本").unwrap().as_rs(), "日本");
        assert_eq!(enum_variant_ident("Ω_max").unwrap().as_rs(), "ΩMax");
    }

    /// Variant idents are emitted in the NFC form rustc canonicalizes to, so the
    /// caller's collision check (which hashes the raw spelling) agrees with the
    /// clash rustc would later flag. Case mapping a Unicode letter can otherwise
    /// yield a non-NFC sequence.
    #[test]
    fn enum_variant_output_is_nfc() {
        for name in ["café", "ΩMax", "日本_mode", "Straße"] {
            let ident = enum_variant_ident(name).unwrap();
            let nfc: String = ident.as_rs().nfc().collect();
            assert_eq!(
                ident.as_rs(),
                nfc,
                "variant ident for '{name}' must be emitted in NFC form",
            );
        }
    }

    /// A member with no identifier-usable characters still has no variant name.
    #[test]
    fn enum_variants_reject_nameless_members() {
        assert!(enum_variant_ident("").is_none());
        assert!(enum_variant_ident("__").is_none());
        assert!(enum_variant_ident("!?").is_none());
    }

    /// Composition-independence: a decomposed member (base letter + combining mark)
    /// must yield the same variant as its composed spelling instead of silently
    /// losing the accent and colliding with a distinct un-accented member.
    #[test]
    fn enum_variants_are_composition_independent() {
        let composed = enum_variant_ident("café").unwrap();
        let decomposed = enum_variant_ident("cafe\u{301}").unwrap();
        assert_eq!(decomposed.as_rs(), composed.as_rs());
        assert_eq!(decomposed.as_rs(), "Café");
    }

    /// A member named `self`/`Self` upper-camel-cases to the reserved word `Self`,
    /// which has no raw-identifier form; it must be renamed, not abort the enum.
    #[test]
    fn enum_variant_self_member_is_renamed_not_rejected() {
        assert_eq!(enum_variant_ident("self").unwrap().as_rs(), "VSelf");
        assert_eq!(enum_variant_ident("Self").unwrap().as_rs(), "VSelf");
        assert_eq!(enum_variant_ident("SELF").unwrap().as_rs(), "VSelf");
    }
}
