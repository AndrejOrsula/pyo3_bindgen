//! Private normalized-domain atoms shared by syntax planning and type lowering.
//!
//! This module owns the immutable name, path, annotation, and local-type values that cross
//! between the syntax and typing phases.  Python acquisition and token-producing operations
//! remain in their existing phase modules; this owner contains only the shared domain state
//! and the local lookup/planning invariants.

use itertools::Itertools;
use rustc_hash::FxHashMap as HashMap;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize)]
pub(crate) struct Ident {
    /// The Rust-side spelling: a valid Rust identifier (a `r#`-escaped raw
    /// identifier if the Python name collides with a Rust keyword) used for the
    /// actual generated token / binding name.
    rs: String,
    /// Overrides the Python-facing spelling (`as_py()`/`Display`) independently of
    /// `rs`, for the rare case where the two must diverge by more than a
    /// `r#` prefix -- e.g. a real Python parameter literally named `_` whose Rust
    /// binding must become a distinct synthetic identifier (`_arg0`) while calls
    /// back into Python must still use the real name `_`. `None` means `as_py()`
    /// is derived from `rs` as usual (the ordinary `r#`-prefix-stripping case).
    py_override: Option<String>,
}

impl Ident {
    pub(crate) fn from_rs(value: &str) -> Self {
        debug_assert!(!value.is_empty());
        Self {
            rs: value.to_owned(),
            py_override: None,
        }
    }

    pub(crate) fn from_py(value: &str) -> Self {
        debug_assert!(!value.is_empty());
        let rs = Self::py_to_rs(value);
        let py_override = (Self::rs_as_py(&rs) != value).then(|| value.to_owned());
        Self { rs, py_override }
    }

    pub(crate) fn try_from_py(
        value: &str,
        empty_error: impl FnOnce() -> String,
    ) -> crate::Result<Self> {
        if value.is_empty() {
            Err(crate::PyBindgenError::CodegenError(empty_error()))
        } else {
            Ok(Self::from_py(value))
        }
    }

    /// Construct an identifier whose Rust-side spelling (`as_rs()`) and Python-side spelling
    /// (`as_py()`) are independent strings, unlike the ordinary `r#`-prefix divergence.
    pub(crate) fn with_rs_override(py_name: &str, rs_name: &str) -> Self {
        debug_assert!(!py_name.is_empty());
        debug_assert!(!rs_name.is_empty());
        Self {
            rs: rs_name.to_owned(),
            py_override: Some(py_name.to_owned()),
        }
    }

    pub(crate) fn into_rs(self) -> String {
        self.rs
    }

    pub(crate) fn as_rs(&self) -> &str {
        &self.rs
    }

    pub(crate) fn as_py(&self) -> &str {
        self.py_override
            .as_deref()
            .unwrap_or_else(|| Self::rs_as_py(&self.rs))
    }

    fn rs_as_py(value: &str) -> &str {
        value.strip_prefix("r#").unwrap_or(value)
    }

    fn py_to_rs(value: &str) -> String {
        // Fast path: a plain ASCII identifier that is neither a Rust keyword nor the
        // reserved wildcard `_` is accepted verbatim by `syn`, so decide it directly and
        // skip the comparatively costly `syn::parse_str` token parse.
        if is_plain_ascii_ident(value) {
            return if value == "_" || is_rust_keyword(value) {
                format!("r#{value}")
            } else {
                value.to_owned()
            };
        }
        if syn::parse_str::<syn::Ident>(value).is_ok() {
            value.to_owned()
        } else {
            format!("r#{value}")
        }
    }

    /// Build the `syn::Ident` this identifier denotes without running the tokenizer.
    ///
    /// `None` means the spelling is not one `syn::parse_str::<syn::Ident>` accepts (a
    /// keyword, `_`, or a non-ASCII spelling), so the caller must fall back to the parser
    /// and keep the existing error behaviour. Spans are irrelevant here: every consumer
    /// renders these idents, and rendering ignores span provenance.
    fn syn_ident_fast(&self) -> Option<syn::Ident> {
        let span = proc_macro2::Span::call_site();
        match self.rs.strip_prefix("r#") {
            Some(rest) => (is_plain_ascii_ident(rest)
                && !matches!(rest, "_" | "crate" | "self" | "Self" | "super"))
            .then(|| syn::Ident::new_raw(rest, span)),
            None => {
                (self.rs != "_" && is_plain_ascii_ident(&self.rs) && !is_rust_keyword(&self.rs))
                    .then(|| syn::Ident::new(&self.rs, span))
            }
        }
    }

    /// Build this identifier as one `syn` path segment without running the tokenizer.
    ///
    /// Only valid when [`Self::is_syn_parseable_segment`] holds, which is exactly the set
    /// `proc_macro2::Ident` accepts for the chosen (raw or plain) form. Unlike
    /// [`Self::syn_ident_fast`] a bare keyword is legal here, because `self`, `Self`,
    /// `crate`, and `super` are ordinary path segments.
    fn syn_segment_ident(&self) -> syn::Ident {
        let span = proc_macro2::Span::call_site();
        match self.rs.strip_prefix("r#") {
            Some(rest) => syn::Ident::new_raw(rest, span),
            None => syn::Ident::new(&self.rs, span),
        }
    }

    /// Whether this identifier's Rust spelling is guaranteed to parse as one `syn` path
    /// segment.  A `false` result is conservative and requires the caller to parse eagerly.
    pub(crate) fn is_syn_parseable_segment(&self) -> bool {
        match self.rs.strip_prefix("r#") {
            Some(rest) => {
                is_plain_ascii_ident(rest)
                    && !matches!(rest, "_" | "crate" | "self" | "Self" | "super")
            }
            None => {
                self.rs != "_"
                    && is_plain_ascii_ident(&self.rs)
                    && (!is_rust_keyword(&self.rs)
                        || matches!(self.rs.as_str(), "super" | "self" | "Self" | "crate"))
            }
        }
    }
}

fn is_plain_ascii_ident(value: &str) -> bool {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) if first == '_' || first.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

fn is_rust_keyword(value: &str) -> bool {
    matches!(
        value,
        "as" | "break"
            | "const"
            | "continue"
            | "crate"
            | "dyn"
            | "else"
            | "enum"
            | "extern"
            | "false"
            | "fn"
            | "for"
            | "if"
            | "impl"
            | "in"
            | "let"
            | "loop"
            | "match"
            | "mod"
            | "move"
            | "mut"
            | "pub"
            | "ref"
            | "return"
            | "self"
            | "Self"
            | "static"
            | "struct"
            | "super"
            | "trait"
            | "true"
            | "type"
            | "unsafe"
            | "use"
            | "where"
            | "while"
            | "async"
            | "await"
            | "abstract"
            | "become"
            | "box"
            | "do"
            | "final"
            | "macro"
            | "override"
            | "priv"
            | "typeof"
            | "unsized"
            | "virtual"
            | "yield"
            | "try"
            | "gen"
    )
}

impl TryFrom<Ident> for syn::Ident {
    type Error = syn::Error;

    fn try_from(value: Ident) -> Result<Self, Self::Error> {
        match value.syn_ident_fast() {
            Some(ident) => Ok(ident),
            None => syn::parse_str::<syn::Ident>(&value.into_rs()),
        }
    }
}

impl TryFrom<&Ident> for syn::Ident {
    type Error = syn::Error;

    fn try_from(value: &Ident) -> Result<Self, Self::Error> {
        match value.syn_ident_fast() {
            Some(ident) => Ok(ident),
            None => syn::parse_str::<syn::Ident>(value.as_rs()),
        }
    }
}

impl std::ops::Deref for Ident {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.rs
    }
}

impl std::fmt::Display for Ident {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self.as_py())
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Hash, serde::Serialize)]
pub(crate) struct Path {
    pub(crate) leading_colon: bool,
    segments: Vec<Ident>,
}

impl Path {
    pub(crate) fn from_rs(value: &str) -> Self {
        if value.is_empty() {
            return Self::default();
        }
        debug_assert!(!value.contains('.'), "Invalid Rust path: {value}");
        Self {
            leading_colon: value.starts_with("::"),
            segments: value
                .split("::")
                .filter(|s| !s.is_empty())
                .map(Ident::from_rs)
                .collect(),
        }
    }

    pub(crate) fn from_py(value: &str) -> Self {
        if value.is_empty() {
            return Self::default();
        }
        debug_assert!(!value.contains("::"), "Invalid Python path: {value}");
        Self {
            leading_colon: false,
            segments: std::iter::repeat_n(
                Ident::from_rs("super"),
                value.chars().take_while(|&c| c == '.').count(),
            )
            .chain(
                value
                    .split('.')
                    .filter(|s| !s.is_empty())
                    .map(Ident::from_py),
            )
            .collect_vec(),
        }
    }

    pub(crate) fn from_live_module(value: &str) -> crate::Result<Self> {
        if let Some(offset) = value.find('\0') {
            return Err(crate::PyBindgenError::CodegenError(format!(
                "Python module name contains a NUL byte at offset {offset}"
            )));
        }
        if value.contains("::") || value.split('.').any(str::is_empty) {
            return Err(crate::PyBindgenError::CodegenError(format!(
                "invalid live module path '{value}'"
            )));
        }
        let path = Self::from_py(value);
        if path.is_empty() || path.to_py() != value {
            Err(crate::PyBindgenError::CodegenError(format!(
                "invalid live module path '{value}'"
            )))
        } else {
            Ok(path)
        }
    }

    pub(crate) fn into_rs(self) -> String {
        std::iter::repeat_n(String::new(), usize::from(self.leading_colon))
            .chain(self.segments.into_iter().map(Ident::into_rs))
            .collect_vec()
            .join("::")
    }

    pub(crate) fn to_rs(&self) -> String {
        std::iter::repeat_n("", usize::from(self.leading_colon))
            .chain(self.segments.iter().map(Ident::as_rs))
            .collect_vec()
            .join("::")
    }

    /// Build this path as a `syn::Path` without running the tokenizer.
    ///
    /// `None` falls back to the parser, so an unrepresentable path keeps its current
    /// error. Every accepted segment is one `proc_macro2::Ident` accepts, which
    /// [`Ident::is_syn_parseable_segment`] already guarantees is a strict subset of what
    /// `syn` parses.
    fn syn_path_fast(&self) -> Option<syn::Path> {
        if !self.is_syn_parseable() {
            return None;
        }
        let segments = self
            .segments
            .iter()
            .map(|segment| syn::PathSegment::from(segment.syn_segment_ident()))
            .collect();
        Some(syn::Path {
            leading_colon: self.leading_colon.then(<syn::Token![::]>::default),
            segments,
        })
    }

    /// Whether [`Self::to_rs`] is guaranteed to be accepted by `syn::Path` without invoking
    /// the tokenizer.  A `false` result requires the caller to parse eagerly.
    pub(crate) fn is_syn_parseable(&self) -> bool {
        !self.segments.is_empty() && self.segments.iter().all(Ident::is_syn_parseable_segment)
    }

    pub(crate) fn to_py(&self) -> String {
        self.segments
            .iter()
            .map(|ident| {
                if ident.as_rs() == "super" {
                    ""
                } else {
                    ident.as_py()
                }
            })
            .collect_vec()
            .join(".")
    }

    pub(crate) fn join(&self, other: &Path) -> Self {
        assert!(
            !other.leading_colon,
            "Leading colon is not allowed in the second path when joining"
        );
        Self {
            leading_colon: self.leading_colon,
            segments: self
                .segments
                .iter()
                .cloned()
                .chain(other.iter().cloned())
                .collect(),
        }
    }

    pub(crate) fn concat(&self, other: &Path) -> Self {
        assert!(
            !other.leading_colon,
            "Leading colon is not allowed in the second path when concatenating"
        );
        Self {
            leading_colon: self.leading_colon,
            segments: self
                .segments
                .iter()
                .chain(&other.segments)
                .cloned()
                .collect(),
        }
    }

    pub(crate) fn name(&self) -> &Ident {
        self.segments.last().unwrap()
    }

    pub(crate) fn root(&self) -> Option<Self> {
        if self.segments.is_empty() {
            None
        } else {
            Some(Self {
                leading_colon: self.leading_colon,
                segments: vec![self.segments[0].clone()],
            })
        }
    }

    pub(crate) fn parent(&self) -> Option<Self> {
        if self.segments.len() > 1 {
            Some(Self {
                leading_colon: self.leading_colon,
                segments: self.segments[..self.segments.len() - 1].to_vec(),
            })
        } else {
            None
        }
    }

    /// Define a fully qualified path from self to target.
    pub(crate) fn relative_to(&self, target: &Path, fully_unambiguous: bool) -> Self {
        if self == target {
            return if fully_unambiguous {
                Path {
                    leading_colon: false,
                    segments: vec![Ident::from_rs("super"), target.name().clone()],
                }
            } else {
                Path {
                    leading_colon: false,
                    segments: vec![Ident::from_rs("self")],
                }
            };
        }

        let common_prefix_length = self
            .segments
            .iter()
            .zip(target.segments.iter())
            .take_while(|(a, b)| a == b)
            .count();

        let mut relative_segments = if fully_unambiguous {
            match common_prefix_length {
                n if n < self.segments.len() => {
                    std::iter::repeat_n(Ident::from_rs("super"), self.segments.len() - n)
                        .chain(target.segments.iter().skip(n).cloned())
                        .collect_vec()
                }
                n if n == self.segments.len() => std::iter::once(Ident::from_rs("self"))
                    .chain(target.segments.iter().skip(n).cloned())
                    .collect_vec(),
                _ => unreachable!(),
            }
        } else {
            match common_prefix_length {
                n if n < self.segments.len() => {
                    std::iter::repeat_n(Ident::from_rs("super"), self.segments.len() - n)
                        .chain(target.segments.iter().skip(n).cloned())
                        .collect_vec()
                }
                n if n == self.segments.len() => {
                    target.segments.iter().skip(n).cloned().collect_vec()
                }
                _ => unreachable!(),
            }
        };

        if fully_unambiguous && relative_segments.last().map(Ident::as_rs) == Some("super") {
            relative_segments.extend([Ident::from_rs("super"), target.name().clone()]);
        }

        Path {
            leading_colon: false,
            segments: relative_segments,
        }
    }
}

impl From<Ident> for Path {
    fn from(ident: Ident) -> Self {
        Self {
            leading_colon: false,
            segments: vec![ident],
        }
    }
}

impl From<&[Ident]> for Path {
    fn from(segments: &[Ident]) -> Self {
        Self {
            leading_colon: false,
            segments: segments.to_owned(),
        }
    }
}

impl std::cmp::PartialOrd for Path {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl std::cmp::Ord for Path {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (self.leading_colon, &self.segments).cmp(&(other.leading_colon, &other.segments))
    }
}

impl TryFrom<Path> for syn::Path {
    type Error = syn::Error;

    fn try_from(value: Path) -> Result<Self, Self::Error> {
        match value.syn_path_fast() {
            Some(path) => Ok(path),
            None => syn::parse_str::<syn::Path>(&value.into_rs()),
        }
    }
}

impl TryFrom<&Path> for syn::Path {
    type Error = syn::Error;

    fn try_from(value: &Path) -> Result<Self, Self::Error> {
        match value.syn_path_fast() {
            Some(path) => Ok(path),
            None => syn::parse_str::<syn::Path>(&value.to_rs()),
        }
    }
}

impl std::ops::Deref for Path {
    type Target = [Ident];

    fn deref(&self) -> &Self::Target {
        &self.segments
    }
}

impl std::ops::DerefMut for Path {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.segments
    }
}

impl std::fmt::Display for Path {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self.to_py())
    }
}

/// Precomputed live-interpreter import resolution consumed by token emission.
///
/// Acquisition records the longest importable module prefix for every Python object path.
/// Emission only performs this immutable lookup; it never attaches to Python or probes imports.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ImportPrefixes {
    by_path: HashMap<Path, Path>,
}

impl ImportPrefixes {
    pub(crate) fn new(entries: impl IntoIterator<Item = (Path, Path)>) -> crate::Result<Self> {
        let mut by_path = HashMap::default();
        for (path, package) in entries {
            if path.is_empty() || package.is_empty() || !path.starts_with(&package) {
                return Err(crate::PyBindgenError::CodegenError(format!(
                    "invalid import prefix '{}' for object path '{}'",
                    package, path
                )));
            }
            if let Some(previous) = by_path.insert(path.clone(), package.clone()) {
                if previous != package {
                    return Err(crate::PyBindgenError::CodegenError(format!(
                        "conflicting import prefixes '{}' and '{}' for object path '{}'",
                        previous, package, path
                    )));
                }
            }
        }
        Ok(Self { by_path })
    }

    pub(crate) fn package_for(&self, path: &Path) -> crate::Result<&Path> {
        self.by_path.get(path).ok_or_else(|| {
            crate::PyBindgenError::CodegenError(format!(
                "normalized plan has no live import prefix for object path '{path}'"
            ))
        })
    }
}

/// Enum that maps Python types to Rust types.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize)]
pub(crate) enum Type {
    PyAny,
    Other(String),
    Unknown,

    // Primitives
    PyBool,
    PyByteArray,
    PyBytes,
    PyFloat,
    PyInt,
    PyString,

    // Enums
    Optional(Box<Type>),
    Union(Vec<Type>),
    PyNone,

    // Collections
    Collection(Box<Type>),
    FixedArray(Box<Type>, Vec<usize>),
    PyDict {
        key_type: Box<Type>,
        value_type: Box<Type>,
    },
    PyFrozenSet(Box<Type>),
    PyList(Box<Type>),
    PySet(Box<Type>),
    PyTuple(Vec<Type>),

    // Additional types - std
    IpV4Addr,
    IpV6Addr,
    Path,
    PySlice,

    // Additional types - num-complex
    PyComplex,

    // Additional types - datetime
    #[cfg(not(Py_LIMITED_API))]
    PyDate,
    #[cfg(not(Py_LIMITED_API))]
    PyDateTime,
    PyDelta,
    #[cfg(not(Py_LIMITED_API))]
    PyTime,
    #[cfg(not(Py_LIMITED_API))]
    PyTzInfo,

    // Python-specific types
    PyCapsule,
    PyCFunction,
    #[cfg(not(Py_LIMITED_API))]
    PyCode,
    PyEllipsis,
    #[cfg(all(not(Py_LIMITED_API), not(any(PyPy, GraalPy))))]
    PyFrame,
    PyFunction {
        param_types: Vec<Type>,
        return_annotation: Box<Type>,
    },
    PyModule,
    #[cfg(not(PyPy))]
    PySuper,
    PyTraceback,
    #[allow(clippy::enum_variant_names)]
    PyType,
}

impl Type {
    /// Iteratively count type nodes and maximum annotation depth.
    pub(crate) fn resource_shape(&self) -> (usize, usize) {
        let mut nodes = 0;
        let mut max_depth = 0;
        let mut pending = vec![(self, 1usize)];
        while let Some((current, depth)) = pending.pop() {
            nodes += 1;
            max_depth = max_depth.max(depth);
            match current {
                Self::Optional(inner)
                | Self::Collection(inner)
                | Self::PyFrozenSet(inner)
                | Self::PyList(inner)
                | Self::PySet(inner) => pending.push((inner, depth + 1)),
                Self::FixedArray(inner, dimensions) => {
                    nodes += dimensions.len();
                    pending.push((inner, depth + 1));
                }
                Self::Union(types) | Self::PyTuple(types) => {
                    pending.extend(types.iter().map(|item| (item, depth + 1)));
                }
                Self::PyDict {
                    key_type,
                    value_type,
                } => {
                    pending.push((key_type, depth + 1));
                    pending.push((value_type, depth + 1));
                }
                Self::PyFunction {
                    param_types,
                    return_annotation,
                } => {
                    pending.extend(param_types.iter().map(|item| (item, depth + 1)));
                    pending.push((return_annotation, depth + 1));
                }
                _ => {}
            }
        }
        (nodes, max_depth)
    }

    pub(crate) fn mentions_collection(&self) -> bool {
        match self {
            Self::Collection(..) => true,
            Self::Optional(inner)
            | Self::PyList(inner)
            | Self::PySet(inner)
            | Self::PyFrozenSet(inner)
            | Self::FixedArray(inner, _) => inner.mentions_collection(),
            Self::Union(types) | Self::PyTuple(types) => {
                types.iter().any(Self::mentions_collection)
            }
            Self::PyDict {
                key_type,
                value_type,
            } => key_type.mentions_collection() || value_type.mentions_collection(),
            Self::PyFunction {
                param_types,
                return_annotation,
            } => {
                return_annotation.mentions_collection()
                    || param_types.iter().any(Self::mentions_collection)
            }
            _ => false,
        }
    }

    pub(crate) fn is_hashable(&self) -> bool {
        matches!(
            self,
            Self::PyBool
                | Self::PyBytes
                | Self::IpV4Addr
                | Self::IpV6Addr
                | Self::Path
                | Self::PyInt
                | Self::PyString
        )
    }
}

/// One local Python type resolved to its internal path representation, alongside a lazily
/// materialized Rust syntax path.
#[derive(Clone)]
pub(crate) struct LocalType {
    pub(crate) path: Path,
    rust_path: std::cell::OnceCell<syn::Path>,
    pub(crate) is_enum: bool,
}

impl LocalType {
    /// Validate the emitted path at construction while deferring the common successful parse.
    pub(crate) fn try_from_path(
        path: Path,
        is_enum: bool,
    ) -> std::result::Result<Self, syn::Error> {
        let rust_path = if path.is_syn_parseable() {
            std::cell::OnceCell::new()
        } else {
            let parsed: syn::Path = (&path).try_into()?;
            std::cell::OnceCell::from(parsed)
        };
        Ok(Self {
            path,
            rust_path,
            is_enum,
        })
    }

    pub(crate) fn rust_path(&self) -> &syn::Path {
        self.rust_path.get_or_init(|| {
            (&self.path)
                .try_into()
                .unwrap_or_else(|_| unreachable!("validated by Path::is_syn_parseable"))
        })
    }
}

#[derive(Default)]
pub(crate) struct LocalTypes {
    local_by_python_path: std::rc::Rc<HashMap<Path, LocalType>>,
    global_by_python_path: std::rc::Rc<HashMap<Path, LocalType>>,
    lookup_scope: Option<Path>,
    collection_wrapper: Option<std::rc::Rc<LocalType>>,
}

impl LocalTypes {
    pub(crate) fn from_maps(
        local_by_python_path: std::rc::Rc<HashMap<Path, LocalType>>,
        global_by_python_path: std::rc::Rc<HashMap<Path, LocalType>>,
        collection_wrapper: Option<std::rc::Rc<LocalType>>,
    ) -> Self {
        Self {
            local_by_python_path,
            global_by_python_path,
            lookup_scope: None,
            collection_wrapper,
        }
    }

    pub(crate) fn get(&self, path: &Path) -> Option<&LocalType> {
        if let Some(scope) = &self.lookup_scope {
            let scoped_path = scope.concat(path);
            self.global_by_python_path
                .get(&scoped_path)
                .or_else(|| self.global_by_python_path.get(path))
        } else {
            self.local_by_python_path
                .get(path)
                .or_else(|| self.global_by_python_path.get(path))
        }
    }

    pub(crate) fn contains_key(&self, path: &Path) -> bool {
        self.get(path).is_some()
    }

    pub(crate) fn collection_wrapper(&self) -> Option<&LocalType> {
        self.collection_wrapper.as_deref()
    }

    /// View this module's emitted Rust paths through another Python module's annotation
    /// namespace.
    pub(crate) fn scoped_to(&self, scope: Path) -> Self {
        Self {
            local_by_python_path: std::rc::Rc::clone(&self.local_by_python_path),
            global_by_python_path: std::rc::Rc::clone(&self.global_by_python_path),
            lookup_scope: Some(scope),
            collection_wrapper: self.collection_wrapper.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::ToTokens;

    #[test]
    fn path_domain_interface_roundtrips_python_and_rust_names() {
        let path = Path::from_py("pkg.mod.Widget");
        assert_eq!(path.to_py(), "pkg.mod.Widget");
        assert_eq!(path.to_rs(), "pkg::r#mod::Widget");
        assert_eq!(Path::from_rs(&path.to_rs()), path);
    }

    #[test]
    fn import_prefixes_are_total_and_reject_conflicting_facts() {
        let object = Path::from_py("pkg.mod.Widget");
        let prefixes = ImportPrefixes::new([(object.clone(), Path::from_py("pkg.mod"))]).unwrap();
        assert_eq!(prefixes.package_for(&object).unwrap().to_py(), "pkg.mod");
        assert!(prefixes
            .package_for(&Path::from_py("pkg.mod.Other"))
            .unwrap_err()
            .to_string()
            .contains("no live import prefix"));

        let error = ImportPrefixes::new([
            (object.clone(), Path::from_py("pkg")),
            (object, Path::from_py("pkg.mod")),
        ])
        .unwrap_err();
        assert!(error.to_string().contains("conflicting import prefixes"));
    }

    #[test]
    fn type_domain_interface_preserves_recursive_collection_planning() {
        let annotation = Type::Optional(Box::new(Type::PyList(Box::new(Type::Collection(
            Box::new(Type::Unknown),
        )))));
        assert!(annotation.mentions_collection());
        assert!(!Type::PyList(Box::new(Type::Unknown)).mentions_collection());
    }

    #[test]
    fn local_lookup_interface_scopes_global_paths_and_materializes_rust_paths() {
        let mut local = HashMap::default();
        local.insert(
            Path::from_py("Widget"),
            LocalType::try_from_path(Path::from_rs("self::Widget"), false).unwrap(),
        );
        let mut global = HashMap::default();
        global.insert(
            Path::from_py("pkg.Widget"),
            LocalType::try_from_path(Path::from_rs("self::pkg::Widget"), true).unwrap(),
        );
        let local_types =
            LocalTypes::from_maps(std::rc::Rc::new(local), std::rc::Rc::new(global), None);

        assert_eq!(
            local_types
                .get(&Path::from_py("Widget"))
                .unwrap()
                .path
                .to_rs(),
            "self::Widget"
        );
        let scoped = local_types.scoped_to(Path::from_py("pkg"));
        let resolved = scoped.get(&Path::from_py("Widget")).unwrap();
        assert!(resolved.is_enum);
        assert_eq!(
            resolved.rust_path().to_token_stream().to_string(),
            "self :: pkg :: Widget"
        );
    }
}
