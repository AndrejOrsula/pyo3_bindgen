use super::{Ident, ImportMode, Path};
use crate::{
    report::{DomainDiagnostic, EnumAliasReport, EnumReport, EnumVariantReport},
    runtime_sentinels::{self, StdlibModule},
    Config, Result,
};
use pyo3::prelude::*;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize)]
pub(crate) struct PyEnum {
    pub(crate) name: Path,
    kind: PyEnumKind,
    variants: Vec<PyEnumVariant>,
    aliases: Vec<PyEnumAlias>,
    docstring: Option<String>,
    /// Whether Python represents members as bare strings instead of a real enum
    /// class. Captured only by external-IR construction; live enums are always
    /// class-backed regardless of the IR-only config switch.
    #[serde(skip)]
    string_backed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
enum PyEnumKind {
    Enum,
    IntEnum,
    StrEnum,
    /// A closed `enum.Flag` class whose members are all non-negative integers fitting
    /// `u64`. Mirrored as a value-newtype struct instead of a Rust `enum`, because flag
    /// combinations are values rather than a fixed set of names.
    Flag,
    /// A closed `enum.IntFlag` class under the same value constraints as
    /// [`PyEnumKind::Flag`].
    IntFlag,
}

impl PyEnumKind {
    /// Map a generic [`crate::schema::EnumKindIr`] to the internal kind.
    fn from_ir(kind: crate::schema::EnumKindIr) -> Self {
        use crate::schema::EnumKindIr;
        match kind {
            EnumKindIr::Enum => Self::Enum,
            EnumKindIr::IntEnum => Self::IntEnum,
            EnumKindIr::StrEnum => Self::StrEnum,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize)]
struct PyEnumVariant {
    py_name: String,
    rust_ident: Ident,
    value_repr: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize)]
struct PyEnumAlias {
    py_name: String,
    rust_ident: Ident,
    target_py_name: String,
    target_rust_ident: Ident,
    value_repr: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PyEnumParse {
    NotEnum,
    Unsupported,
    Supported,
}

impl PyEnum {
    pub(crate) fn resource_shape(&self) -> crate::resource::StructuralResources {
        let nodes = 1 + self.variants.len() + self.aliases.len();
        crate::resource::StructuralResources {
            definitions: nodes,
            recursion_depth: 1,
            model_nodes: nodes,
        }
    }

    pub(crate) fn classify(class: &pyo3::Bound<pyo3::types::PyType>) -> Result<PyEnumParse> {
        let py = class.py();
        let enum_type = runtime_sentinels::attr(py, StdlibModule::Enum, "Enum")?;
        if !class.is_subclass(enum_type.as_any())? {
            return Ok(PyEnumParse::NotEnum);
        }

        // A flag class is mirrored as a value newtype only when every member value is a
        // plain non-negative Python `int` that fits `u64` (the newtype's bit storage).
        // Anything else -- negative bits, values above `u64::MAX`, non-`int` values, or a
        // class whose members cannot be read at all -- fails toward the owning wrapper.
        for flag_kind in ["Flag", "IntFlag"] {
            if let Ok(flag_type) = runtime_sentinels::attr(py, StdlibModule::Enum, flag_kind) {
                if class.is_subclass(flag_type.as_any())? {
                    return Ok(if Self::flag_member_bits_fit_u64(class) {
                        PyEnumParse::Supported
                    } else {
                        PyEnumParse::Unsupported
                    });
                }
            }
        }

        Ok(PyEnumParse::Supported)
    }

    /// Whether every member of a live flag class carries a plain non-negative `int` value
    /// that fits `u64`. Any introspection failure (an open/dynamic class whose
    /// `__members__` cannot be read or iterated) counts as "does not fit", so the caller
    /// keeps the owning-wrapper fallback rather than emitting a lossy mirror.
    fn flag_member_bits_fit_u64(class: &pyo3::Bound<pyo3::types::PyType>) -> bool {
        let py = class.py();
        let Ok(members) = class
            .getattr(pyo3::intern!(py, "__members__"))
            .and_then(|members| members.call_method0(pyo3::intern!(py, "values")))
        else {
            return false;
        };
        let Ok(members) = members.try_iter() else {
            return false;
        };
        let mut any_member = false;
        for member in members {
            any_member = true;
            let Ok(value) = member.and_then(|member| member.getattr(pyo3::intern!(py, "value")))
            else {
                return false;
            };
            // Exactly `int`: a `bool` (an `int` subclass) or any other custom value type
            // would not round-trip through the newtype's decimal value repr.
            if !value.is_exact_instance_of::<pyo3::types::PyInt>() {
                return false;
            }
            if value.extract::<u64>().is_err() {
                return false;
            }
        }
        any_member
    }

    pub(crate) fn parse(
        cfg: &Config,
        class: &pyo3::Bound<pyo3::types::PyType>,
        name: Path,
    ) -> Result<Self> {
        let py = class.py();
        // Flag classes are checked first: `IntFlag` is neither a `StrEnum` nor an
        // `IntEnum` subclass, but it must never fall through to the plain-mirror kinds.
        let kind = if let Some(flag_kind) = Self::classify_flag_enum(class)? {
            flag_kind
        } else if let Some(str_enum_type) =
            runtime_sentinels::optional_attr(py, StdlibModule::Enum, "StrEnum")?
        {
            if class.is_subclass(str_enum_type.as_any())? {
                PyEnumKind::StrEnum
            } else {
                Self::classify_int_enum(class)?
            }
        } else {
            Self::classify_int_enum(class)?
        };

        let member_items = class
            .getattr(pyo3::intern!(py, "__members__"))?
            .call_method0(pyo3::intern!(py, "items"))?;

        // Extract the raw (py_name, canonical_py_name, value_repr) triples, then run
        // the shared (pyo3-free) classification so the IR path stays byte-identical.
        let mut items = Vec::new();
        let mut canonical_value_reprs: HashMap<String, String> = HashMap::default();
        for item in member_items.try_iter()? {
            let item = item?;
            let item = item
                .cast_into::<pyo3::types::PyTuple>()
                .map_err(pyo3::PyErr::from)?;
            let py_name = item.get_item(0)?.extract::<String>()?;
            let member = item.get_item(1)?;
            let canonical_py_name = member
                .getattr(pyo3::intern!(py, "name"))?
                .extract::<String>()?;
            let value_repr = match canonical_value_reprs.get(&canonical_py_name) {
                Some(value_repr) => value_repr.clone(),
                None => {
                    let value = member.getattr(pyo3::intern!(py, "value"))?;
                    let value_repr = stable_live_value_repr(&value)?;
                    canonical_value_reprs.insert(canonical_py_name.clone(), value_repr.clone());
                    value_repr
                }
            };
            items.push((py_name, canonical_py_name, value_repr));
        }
        let (variants, aliases) = Self::classify_variants(&name, items)?;

        // Second gate on the flag bits (the first is `classify`, on the live values): the
        // newtype mirror stores `u64`, so a value repr that does not parse as one must
        // fall back to the owning wrapper instead of reaching emission.
        if matches!(kind, PyEnumKind::Flag | PyEnumKind::IntFlag) && flag_bits(&variants).is_none()
        {
            return Err(crate::PyBindgenError::CodegenError(format!(
                "Cannot generate a Rust flag mirror for '{name}' because its member values are not all non-negative integers fitting u64"
            )));
        }

        let docstring = if cfg.generate_docs {
            let docstring = class.getattr(pyo3::intern!(py, "__doc__"))?.to_string();
            if docstring.is_empty() || docstring == "None" {
                None
            } else {
                Some(docstring)
            }
        } else {
            None
        };

        Ok(Self {
            name,
            kind,
            variants,
            aliases,
            docstring,
            string_backed: false,
        })
    }

    /// Construct a [`PyEnum`] from the generic IR with zero `pyo3` usage.
    ///
    /// Builds the same struct as [`Self::parse`] but from strings: no
    /// `py.import("enum")`, no `__members__`. Reuses the shared (pyo3-free)
    /// [`Self::classify_variants`] so output matches the live path. The schema validator establishes
    /// the required strict per-enum opt-in (or global leniency) before flag/open IR reaches this seam;
    /// this constructor then emits the documented plain-mirror lowering.
    pub(crate) fn from_ir(cfg: &Config, ir: &crate::schema::EnumIr) -> Result<Self> {
        let name = Path::from_py(&ir.name);
        let kind = PyEnumKind::from_ir(ir.kind);

        let items = ir
            .variants
            .iter()
            .map(|variant| {
                (
                    variant.py_name.clone(),
                    variant.py_name.clone(),
                    variant.value_repr.clone(),
                )
            })
            .chain(ir.aliases.iter().map(|alias| {
                (
                    alias.py_name.clone(),
                    alias.target_py_name.clone(),
                    alias.value_repr.clone(),
                )
            }))
            .collect::<Vec<_>>();
        let (variants, aliases) = Self::classify_variants(&name, items)?;

        let docstring = if cfg.generate_docs {
            ir.docstring.clone()
        } else {
            None
        };

        Ok(Self {
            name,
            kind,
            variants,
            aliases,
            docstring,
            string_backed: cfg.enum_ir_string_backed || ir.string_backed,
        })
    }

    /// Shared (pyo3-free) classification of raw `(py_name, canonical_py_name,
    /// value_repr)` triples into canonical variants and aliases, with the same
    /// ident sanitization and collision checks used by both [`Self::parse`] and
    /// [`Self::from_ir`].
    fn classify_variants(
        name: &Path,
        items: impl IntoIterator<Item = (String, String, String)>,
    ) -> Result<(Vec<PyEnumVariant>, Vec<PyEnumAlias>)> {
        let items = items.into_iter().collect::<Vec<_>>();
        let canonical_python_names = items
            .iter()
            .filter(|(py_name, canonical_py_name, _)| py_name == canonical_py_name)
            .map(|(py_name, _, _)| py_name.clone())
            .collect::<HashSet<_>>();
        let mut variants = Vec::new();
        let mut aliases = Vec::new();
        let mut used_rust_idents = HashSet::default();

        for (py_name, canonical_py_name, value_repr) in items {
            if py_name == canonical_py_name {
                let rust_ident = super::naming::enum_variant_ident(&py_name).ok_or_else(|| {
                    crate::PyBindgenError::CodegenError(format!(
                        "Cannot generate Rust enum variant for Python member '{name}.{py_name}'"
                    ))
                })?;
                if !used_rust_idents.insert(rust_ident.clone()) {
                    return Err(crate::PyBindgenError::CodegenError(format!(
                        "Rust enum variant name collision while generating '{name}.{py_name}'"
                    )));
                }
                variants.push(PyEnumVariant {
                    py_name,
                    rust_ident,
                    value_repr,
                });
            } else {
                if !canonical_python_names.contains(&canonical_py_name) {
                    return Err(crate::PyBindgenError::CodegenError(format!(
                        "Cannot generate Rust enum for '{name}' because alias '{py_name}' targets missing canonical member '{canonical_py_name}'"
                    )));
                }
                let rust_ident = super::naming::enum_variant_ident(&py_name).ok_or_else(|| {
                    crate::PyBindgenError::CodegenError(format!(
                        "Cannot generate Rust enum alias for Python member '{name}.{py_name}'"
                    ))
                })?;
                let target_rust_ident =
                    super::naming::enum_variant_ident(&canonical_py_name).ok_or_else(|| {
                        crate::PyBindgenError::CodegenError(format!(
                            "Cannot generate Rust enum alias target for Python member '{name}.{py_name}'"
                        ))
                    })?;
                if !used_rust_idents.insert(rust_ident.clone()) {
                    return Err(crate::PyBindgenError::CodegenError(format!(
                        "Rust enum alias name collision while generating '{name}.{py_name}'"
                    )));
                }
                aliases.push(PyEnumAlias {
                    py_name,
                    rust_ident,
                    target_py_name: canonical_py_name,
                    target_rust_ident,
                    value_repr,
                });
            }
        }

        if variants.is_empty() {
            return Err(crate::PyBindgenError::CodegenError(format!(
                "Cannot generate Rust enum for '{name}' because it has no members"
            )));
        }
        Ok((variants, aliases))
    }

    pub(crate) fn generate(
        &self,
        cfg: &Config,
        import_mode: &ImportMode,
        diagnostics: &mut Vec<DomainDiagnostic>,
    ) -> Result<proc_macro2::TokenStream> {
        let name = self.name.name();
        let enum_ident: syn::Ident = match super::naming::rust_ident(
            name,
            super::naming::SymbolKind::Enum,
        ) {
            Some(ident) => {
                if TryInto::<syn::Ident>::try_into(name).is_err() {
                    crate::report::warn(
                        diagnostics,
                        "enum_invalid_ident_renamed",
                        self.name.to_py(),
                        format!(
                            "Enum '{}' is an invalid Rust ident for an enum name. Renamed to '{ident}'.",
                            self.name,
                        ),
                    );
                }
                ident
            }
            None => {
                crate::report::warn(
                    diagnostics,
                    "enum_invalid_ident_skipped",
                    self.name.to_py(),
                    format!(
                        "Enum '{}' is an invalid Rust ident for an enum name. Renaming failed. Bindings will not be generated.",
                        self.name
                    ),
                );
                return Ok(proc_macro2::TokenStream::new());
            }
        };
        let object_name = self.name.to_py();
        let object_import = import_mode.resolve(&self.name)?;

        let mut output = proc_macro2::TokenStream::new();
        if cfg.generate_docs {
            let enum_doc = format!(" Python enum mirror for `{object_name}`.");
            output.extend(quote::quote! {
                #[doc = #enum_doc]
            });
            if let Some(mut docstring) = self.docstring.clone() {
                crate::utils::text::format_docstring(&mut docstring);
                output.extend(quote::quote! {
                    #[doc = #docstring]
                });
            }
        }

        // A closed flag class is a VALUE type, not a fixed set of names: any combination of
        // member bits is a legal Python member-or-composite. It therefore mirrors as a
        // newtype over the bits with member constants and set operators, not as a Rust
        // `enum`.
        if matches!(self.kind, PyEnumKind::Flag | PyEnumKind::IntFlag) {
            output.extend(self.generate_flag(&enum_ident, &object_name, &object_import)?);
            return Ok(output);
        }

        // Explicit integer discriminants (and the derived `Ord` that relies on them)
        // are sound only when EVERY canonical member has a parseable AND UNIQUE integer
        // value. A missing / non-numeric / unsupported-range value would force Rust to
        // auto-assign a colliding discriminant, and a duplicate value is itself a
        // repeated discriminant -- both are E0081. Equal-valued live members fold into
        // aliases rather than repeated variants, but Python integers may exceed Rust's
        // supported enum reprs; external IR may additionally omit/malform values. Keep
        // both paths all-or-nothing.
        let int_values: Option<Vec<i128>> = matches!(self.kind, PyEnumKind::IntEnum)
            .then(|| {
                self.variants
                    .iter()
                    .map(|variant| variant.value_repr.parse::<i128>().ok())
                    .collect::<Option<Vec<_>>>()
            })
            .flatten()
            .filter(|values| {
                // Reject duplicates: equal explicit discriminants are E0081.
                let mut seen = rustc_hash::FxHashSet::default();
                values.iter().all(|value| seen.insert(*value))
            })
            .filter(|values| {
                // Rust 1.83 does not support `repr(i128)`/`repr(u128)` on enums.
                // Preserve explicit value ordering whenever one stable 64-bit repr can
                // represent the complete set; mixed negative + above-i64 values degrade
                // rather than violating the workspace MSRV.
                values.iter().all(|value| i64::try_from(*value).is_ok())
                    || values.iter().all(|value| u64::try_from(*value).is_ok())
            });
        // Only an `IntEnum` with a full, unique value set gets a derived `Ord`: an
        // explicit discriminant then mirrors Python's integer ordering exactly. Without
        // the discriminants (a `StrEnum`, or an `IntEnum` whose values are not all
        // parseable/unique), a derived `Ord` would silently fall back to declaration
        // order and disagree with Python, so it is omitted.
        let derive_ord = int_values
            .is_some()
            .then(|| quote::quote! { PartialOrd, Ord, });
        // Pick one stable repr for the complete value set. Prefer signed when possible;
        // an all-nonnegative set may use `u64` to preserve bitmask values above i64::MAX.
        let repr_attr = int_values.as_ref().map(|values| {
            if values.iter().all(|value| i64::try_from(*value).is_ok()) {
                quote::quote! { #[repr(i64)] }
            } else {
                quote::quote! { #[repr(u64)] }
            }
        });
        // Surface the otherwise-silent Ord/repr loss: an `IntEnum` whose members are not all
        // parseable as unique values fitting one stable `i64`/`u64` repr degrades to a plain
        // mirror (no explicit repr/discriminants and no derived `Ord`), so callers lose
        // value-ordered comparison. Warn so it is observable.
        if matches!(self.kind, PyEnumKind::IntEnum) && int_values.is_none() {
            crate::report::warn(
                diagnostics,
                "enum_intenum_ord_dropped",
                self.name.to_py(),
                format!(
                    "IntEnum '{}' has members whose integer values are missing, non-numeric, \
                     duplicated, or cannot all fit one stable i64/u64 enum representation; \
                     emitting a plain mirror without explicit discriminants or a derived Ord.",
                    self.name
                ),
            );
        }
        let variant_defs = self.variants.iter().enumerate().map(|(index, variant)| {
            let variant_ident: syn::Ident = (&variant.rust_ident).try_into()?;
            let py_name = &variant.py_name;
            let doc = format!(
                " Python enum member `{object_name}.{}` with value `{}`.",
                doc_span(&variant.py_name),
                doc_span(&variant.value_repr)
            );
            // Only emit the alias when it differs from the variant identifier (rustc rejects
            // `#[doc(alias = X)]` when X equals the item's own name) AND is a legal alias
            // payload -- a raw member name with a quote/whitespace would compile-error, so a
            // validated IR must still yield a buildable crate: drop the alias, keep the member.
            let doc_alias = (py_name != &variant_ident.to_string() && is_valid_doc_alias(py_name))
                .then(|| quote::quote! { #[doc(alias = #py_name)] });
            // Mirror the real Python value as an explicit discriminant so the derived
            // `Ord` above matches Python's int ordering instead of declaration order.
            // Emit the CANONICAL decimal of the parsed integer (not the raw `value_repr`)
            // so a `"+5"` or `"007"` repr cannot yield an invalid discriminant token.
            let discriminant = int_values.as_ref().map(|values| {
                let value = proc_macro2::Literal::i128_unsuffixed(values[index]);
                quote::quote! { = #value }
            });
            Result::Ok(quote::quote! {
                #[doc = #doc]
                #doc_alias
                #variant_ident #discriminant,
            })
        });
        let variant_defs = variant_defs.collect::<Result<proc_macro2::TokenStream>>()?;

        let alias_consts = self.aliases.iter().map(|alias| {
            let alias_ident: syn::Ident = (&alias.rust_ident).try_into()?;
            let target_ident: syn::Ident = (&alias.target_rust_ident).try_into()?;
            let py_name = &alias.py_name;
            let doc = format!(
                " Python enum alias `{object_name}.{}` for `{object_name}.{}` with value `{}`.",
                doc_span(&alias.py_name),
                doc_span(&alias.target_py_name),
                doc_span(&alias.value_repr)
            );
            let doc_alias = (py_name != &alias_ident.to_string() && is_valid_doc_alias(py_name))
                .then(|| quote::quote! { #[doc(alias = #py_name)] });
            Result::Ok(quote::quote! {
                #[doc = #doc]
                #doc_alias
                pub const #alias_ident: Self = Self::#target_ident;
            })
        });
        let alias_consts = alias_consts.collect::<Result<proc_macro2::TokenStream>>()?;

        let name_match_arms = self.variants.iter().map(|variant| {
            let variant_ident: syn::Ident = (&variant.rust_ident).try_into()?;
            let py_name = &variant.py_name;
            Result::Ok(quote::quote! {
                Self::#variant_ident => #py_name,
            })
        });
        let name_match_arms = name_match_arms.collect::<Result<proc_macro2::TokenStream>>()?;

        let from_name_match_arms = self
            .variants
            .iter()
            .map(|variant| {
                let variant_ident: syn::Ident = (&variant.rust_ident).try_into()?;
                let py_name = &variant.py_name;
                Result::Ok(quote::quote! {
                    #py_name => ::std::result::Result::Ok(Self::#variant_ident),
                })
            })
            .chain(self.aliases.iter().map(|alias| {
                let target_ident: syn::Ident = (&alias.target_rust_ident).try_into()?;
                let py_name = &alias.py_name;
                Result::Ok(quote::quote! {
                    #py_name => ::std::result::Result::Ok(Self::#target_ident),
                })
            }));
        let from_name_match_arms =
            from_name_match_arms.collect::<Result<proc_macro2::TokenStream>>()?;

        // The class-import mirror assumes the enum maps to a real Python `enum.Enum` class
        // that can be imported and whose members carry a `.name`. External-IR enums whose
        // Python representation is the member STRING (e.g. Blender RNA enum properties) have
        // no such class, so under `enum_ir_string_backed` the round-trip goes through the
        // member string (`as_py_name`/`from_py_name`) with no `#object_import`. The two
        // branches share everything except the `into_py_any`/`extract` bodies; the default
        // (false) branch is byte-identical to the historical output.
        let (into_py_any_body, extract_body) = if self.string_backed {
            (
                quote::quote! {
                    // String-backed external-IR enum: the Python representation IS the member
                    // string (there is no backing Python enum class to import).
                    ::std::result::Result::Ok(
                        ::pyo3::types::PyString::new(py, self.as_py_name()).into_any(),
                    )
                },
                quote::quote! {
                    // String-backed external-IR enum: the Python value is the member string,
                    // so extract it directly and map it back through `from_py_name` (no
                    // Python enum class to import, no `is_instance`).
                    let member_name = ::pyo3::types::PyAnyMethods::extract::<
                        ::std::string::String,
                    >(obj.as_any())?;
                    Self::from_py_name(&member_name)
                },
            )
        } else {
            (
                quote::quote! {
                    // A mirrored enum represents the real Python enum class. Import/member
                    // failures must remain visible instead of silently changing the argument
                    // type to `str`.
                    let enum_type = #object_import;
                    ::pyo3::types::PyAnyMethods::getattr(enum_type.as_any(), self.as_py_name())
                },
                quote::quote! {
                    let py = obj.py();
                    let expected_type = #object_import;
                    if !::pyo3::types::PyAnyMethods::is_instance(
                        obj.as_any(),
                        expected_type.as_any(),
                    )? {
                        return ::std::result::Result::Err(
                            ::pyo3::exceptions::PyTypeError::new_err(
                                ::std::format!("expected Python enum member of '{}'", #object_name),
                            ),
                        );
                    }
                    let name_attr = ::pyo3::types::PyAnyMethods::getattr(
                        obj.as_any(),
                        ::pyo3::intern!(py, "name"),
                    )?;
                    let member_name = ::pyo3::types::PyAnyMethods::extract::<
                        ::std::string::String,
                    >(&name_attr)?;
                    Self::from_py_name(&member_name)
                },
            )
        };

        output.extend(quote::quote! {
            #repr_attr
            #[derive(Debug, Clone, Copy, PartialEq, Eq, #derive_ord Hash)]
            pub enum #enum_ident {
                #variant_defs
            }

            #[automatically_derived]
            impl #enum_ident {
                #alias_consts

                pub fn as_py_name(self) -> &'static str {
                    match self {
                        #name_match_arms
                    }
                }

                pub fn from_py_name(name: &str) -> ::pyo3::PyResult<Self> {
                    match name {
                        #from_name_match_arms
                        _ => ::std::result::Result::Err(::pyo3::exceptions::PyValueError::new_err(
                            ::std::format!(
                                "unknown Python enum member '{}.{}'",
                                #object_name,
                                name
                            ),
                        )),
                    }
                }

                pub fn into_py_any<'py>(
                    self,
                    py: ::pyo3::marker::Python<'py>,
                ) -> ::pyo3::PyResult<::pyo3::Bound<'py, ::pyo3::types::PyAny>> {
                    #into_py_any_body
                }
            }

            #[automatically_derived]
            impl<'py> ::pyo3::FromPyObject<'_, 'py> for #enum_ident {
                type Error = ::pyo3::PyErr;

                fn extract(
                    obj: ::pyo3::Borrowed<'_, 'py, ::pyo3::types::PyAny>,
                ) -> ::std::result::Result<Self, ::pyo3::PyErr> {
                    #extract_body
                }
            }

            #[automatically_derived]
            impl<'py> ::pyo3::IntoPyObject<'py> for #enum_ident {
                type Target = ::pyo3::types::PyAny;
                type Output = ::pyo3::Bound<'py, ::pyo3::types::PyAny>;
                type Error = ::pyo3::PyErr;

                fn into_pyobject(
                    self,
                    py: ::pyo3::marker::Python<'py>,
                ) -> ::std::result::Result<::pyo3::Bound<'py, ::pyo3::types::PyAny>, ::pyo3::PyErr> {
                    self.into_py_any(py)
                }
            }

            #[automatically_derived]
            impl<'a, 'py> ::pyo3::IntoPyObject<'py> for &'a #enum_ident {
                type Target = ::pyo3::types::PyAny;
                type Output = ::pyo3::Bound<'py, ::pyo3::types::PyAny>;
                type Error = ::pyo3::PyErr;

                fn into_pyobject(
                    self,
                    py: ::pyo3::marker::Python<'py>,
                ) -> ::std::result::Result<::pyo3::Bound<'py, ::pyo3::types::PyAny>, ::pyo3::PyErr> {
                    (*self).into_py_any(py)
                }
            }
        });

        Ok(output)
    }

    /// Emit the value-newtype mirror of a closed `Flag`/`IntFlag` class.
    ///
    /// Unlike the plain mirror this type has no `as_py_name`/`from_py_name`: a flag VALUE
    /// (`R|W`) need not correspond to any single named Python member, so there is no
    /// canonical name to expose. `core::ops::Not` is likewise not emitted: the complement
    /// of a flag set depends on the Python class's boundary policy (`STRICT`, `KEEP`,
    /// `CONFORM`), which only Python can apply. Conversions therefore delegate to the real
    /// class -- `into_py_any` CALLS it with the value and lets Python enforce the boundary
    /// -- instead of fabricating members on the Rust side.
    fn generate_flag(
        &self,
        enum_ident: &syn::Ident,
        object_name: &str,
        object_import: &proc_macro2::TokenStream,
    ) -> Result<proc_macro2::TokenStream> {
        let bits = flag_bits(&self.variants).ok_or_else(|| {
            crate::PyBindgenError::CodegenError(format!(
                "Cannot generate a Rust flag mirror for '{}' because its member values are not all non-negative integers fitting u64",
                self.name
            ))
        })?;

        let variant_consts = self
            .variants
            .iter()
            .zip(bits.iter())
            .map(|(variant, value)| {
                let variant_ident: syn::Ident = (&variant.rust_ident).try_into()?;
                let py_name = &variant.py_name;
                let doc = format!(
                    " Python flag member `{object_name}.{}` with value `{}`.",
                    doc_span(&variant.py_name),
                    doc_span(&variant.value_repr)
                );
                let doc_alias = (py_name != &variant_ident.to_string()
                    && is_valid_doc_alias(py_name))
                .then(|| quote::quote! { #[doc(alias = #py_name)] });
                let value = proc_macro2::Literal::u64_suffixed(*value);
                Result::Ok(quote::quote! {
                    #[doc = #doc]
                    #doc_alias
                    pub const #variant_ident: #enum_ident = #enum_ident(#value);
                })
            })
            .collect::<Result<proc_macro2::TokenStream>>()?;

        let alias_consts = self
            .aliases
            .iter()
            .map(|alias| {
                let alias_ident: syn::Ident = (&alias.rust_ident).try_into()?;
                let target_ident: syn::Ident = (&alias.target_rust_ident).try_into()?;
                let py_name = &alias.py_name;
                let doc = format!(
                    " Python flag alias `{object_name}.{}` for `{object_name}.{}` with value `{}`.",
                    doc_span(&alias.py_name),
                    doc_span(&alias.target_py_name),
                    doc_span(&alias.value_repr)
                );
                let doc_alias = (py_name != &alias_ident.to_string()
                    && is_valid_doc_alias(py_name))
                .then(|| quote::quote! { #[doc(alias = #py_name)] });
                Result::Ok(quote::quote! {
                    #[doc = #doc]
                    #doc_alias
                    pub const #alias_ident: #enum_ident = #enum_ident::#target_ident;
                })
            })
            .collect::<Result<proc_macro2::TokenStream>>()?;

        let value_doc = " The raw Python `value` bits of this flag set.";
        let contains_doc =
            " Whether every bit of `other` is set (the empty set is always contained).";
        let is_empty_doc = " Whether no bit is set (Python's falsy flag value).";
        let into_py_any_doc = concat!(
            " Convert into the Python member for these bits by CALLING the mirrored class.\n",
            "\n",
            " Python applies the class's own boundary policy, so out-of-boundary bits raise",
            " instead of fabricating a member that the class does not define.",
        );

        Ok(quote::quote! {
            #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
            pub struct #enum_ident(u64);

            #[automatically_derived]
            impl #enum_ident {
                #variant_consts
                #alias_consts

                #[doc = #value_doc]
                pub const fn value(self) -> u64 {
                    self.0
                }

                #[doc = #contains_doc]
                pub const fn contains(self, other: #enum_ident) -> bool {
                    self.0 & other.0 == other.0
                }

                #[doc = #is_empty_doc]
                pub const fn is_empty(self) -> bool {
                    self.0 == 0
                }

                #[doc = #into_py_any_doc]
                pub fn into_py_any<'py>(
                    self,
                    py: ::pyo3::marker::Python<'py>,
                ) -> ::pyo3::PyResult<::pyo3::Bound<'py, ::pyo3::types::PyAny>> {
                    let enum_type = #object_import;
                    ::pyo3::types::PyAnyMethods::call1(enum_type.as_any(), (self.0,))
                }
            }

            #[automatically_derived]
            impl ::core::ops::BitOr for #enum_ident {
                type Output = Self;

                fn bitor(self, rhs: Self) -> Self {
                    Self(self.0 | rhs.0)
                }
            }

            #[automatically_derived]
            impl ::core::ops::BitAnd for #enum_ident {
                type Output = Self;

                fn bitand(self, rhs: Self) -> Self {
                    Self(self.0 & rhs.0)
                }
            }

            #[automatically_derived]
            impl ::core::ops::BitXor for #enum_ident {
                type Output = Self;

                fn bitxor(self, rhs: Self) -> Self {
                    Self(self.0 ^ rhs.0)
                }
            }

            #[automatically_derived]
            impl<'py> ::pyo3::FromPyObject<'_, 'py> for #enum_ident {
                type Error = ::pyo3::PyErr;

                fn extract(
                    obj: ::pyo3::Borrowed<'_, 'py, ::pyo3::types::PyAny>,
                ) -> ::std::result::Result<Self, ::pyo3::PyErr> {
                    let py = obj.py();
                    let expected_type = #object_import;
                    if !::pyo3::types::PyAnyMethods::is_instance(
                        obj.as_any(),
                        expected_type.as_any(),
                    )? {
                        return ::std::result::Result::Err(
                            ::pyo3::exceptions::PyTypeError::new_err(
                                ::std::format!("expected Python flag member of '{}'", #object_name),
                            ),
                        );
                    }
                    let value_attr = ::pyo3::types::PyAnyMethods::getattr(
                        obj.as_any(),
                        ::pyo3::intern!(py, "value"),
                    )?;
                    let value = ::pyo3::types::PyAnyMethods::extract::<u64>(&value_attr)?;
                    ::std::result::Result::Ok(Self(value))
                }
            }

            #[automatically_derived]
            impl<'py> ::pyo3::IntoPyObject<'py> for #enum_ident {
                type Target = ::pyo3::types::PyAny;
                type Output = ::pyo3::Bound<'py, ::pyo3::types::PyAny>;
                type Error = ::pyo3::PyErr;

                fn into_pyobject(
                    self,
                    py: ::pyo3::marker::Python<'py>,
                ) -> ::std::result::Result<::pyo3::Bound<'py, ::pyo3::types::PyAny>, ::pyo3::PyErr> {
                    self.into_py_any(py)
                }
            }

            #[automatically_derived]
            impl<'a, 'py> ::pyo3::IntoPyObject<'py> for &'a #enum_ident {
                type Target = ::pyo3::types::PyAny;
                type Output = ::pyo3::Bound<'py, ::pyo3::types::PyAny>;
                type Error = ::pyo3::PyErr;

                fn into_pyobject(
                    self,
                    py: ::pyo3::marker::Python<'py>,
                ) -> ::std::result::Result<::pyo3::Bound<'py, ::pyo3::types::PyAny>, ::pyo3::PyErr> {
                    (*self).into_py_any(py)
                }
            }
        })
    }

    pub(crate) fn report(&self) -> EnumReport {
        EnumReport {
            name: self.name.to_py(),
            kind: match self.kind {
                PyEnumKind::Enum => "Enum",
                PyEnumKind::IntEnum => "IntEnum",
                PyEnumKind::StrEnum => "StrEnum",
                // Additive backing kinds (report schema version stays 1): a consumer that
                // only knows the three plain kinds still parses the report; it simply sees
                // a `kind` string it does not recognize.
                PyEnumKind::Flag => "Flag",
                PyEnumKind::IntFlag => "IntFlag",
            }
            .to_owned(),
            variants: self
                .variants
                .iter()
                .map(|variant| EnumVariantReport {
                    python_name: variant.py_name.clone(),
                    rust_name: variant.rust_ident.as_rs().to_owned(),
                    value_repr: variant.value_repr.clone(),
                })
                .collect(),
            aliases: self
                .aliases
                .iter()
                .map(|alias| EnumAliasReport {
                    python_name: alias.py_name.clone(),
                    rust_name: alias.rust_ident.as_rs().to_owned(),
                    target_python_name: alias.target_py_name.clone(),
                    target_rust_name: alias.target_rust_ident.as_rs().to_owned(),
                    value_repr: alias.value_repr.clone(),
                })
                .collect(),
        }
    }

    /// The flag kind of `class`, or [`None`] when it is not a flag class at all.
    /// `IntFlag` is checked before `Flag` because it is a `Flag` subclass.
    fn classify_flag_enum(class: &pyo3::Bound<pyo3::types::PyType>) -> Result<Option<PyEnumKind>> {
        let py = class.py();
        for (attr, kind) in [("IntFlag", PyEnumKind::IntFlag), ("Flag", PyEnumKind::Flag)] {
            if let Some(flag_type) = runtime_sentinels::optional_attr(py, StdlibModule::Enum, attr)?
            {
                if class.is_subclass(flag_type.as_any())? {
                    return Ok(Some(kind));
                }
            }
        }
        Ok(None)
    }

    fn classify_int_enum(class: &pyo3::Bound<pyo3::types::PyType>) -> Result<PyEnumKind> {
        let int_enum_type = runtime_sentinels::attr(class.py(), StdlibModule::Enum, "IntEnum")?;
        if class.is_subclass(int_enum_type.as_any())? {
            Ok(PyEnumKind::IntEnum)
        } else {
            Ok(PyEnumKind::Enum)
        }
    }
}

/// The `u64` bits of every canonical flag member, or [`None`] when any value repr is not a
/// plain non-negative integer fitting `u64`. Aliases are omitted deliberately: an alias
/// reuses its canonical member's value repr, so gating the canonical members covers both.
fn flag_bits(variants: &[PyEnumVariant]) -> Option<Vec<u64>> {
    variants
        .iter()
        .map(|variant| variant.value_repr.parse::<u64>().ok())
        .collect()
}

fn stable_live_value_repr(value: &pyo3::Bound<pyo3::types::PyAny>) -> Result<String> {
    if value.is_exact_instance_of::<pyo3::types::PyNone>()
        || value.is_exact_instance_of::<pyo3::types::PyBool>()
        || value.is_exact_instance_of::<pyo3::types::PyInt>()
        || value.is_exact_instance_of::<pyo3::types::PyFloat>()
        || value.is_exact_instance_of::<pyo3::types::PyComplex>()
        || value.is_exact_instance_of::<pyo3::types::PyString>()
        || value.is_exact_instance_of::<pyo3::types::PyBytes>()
        || value.is_exact_instance_of::<pyo3::types::PyEllipsis>()
        || value.is_exact_instance_of::<pyo3::types::PyNotImplemented>()
    {
        return Ok(value.repr()?.to_string());
    }

    if value.is_exact_instance_of::<pyo3::types::PyTuple>() {
        let tuple = value
            .cast::<pyo3::types::PyTuple>()
            .map_err(pyo3::PyErr::from)?;
        let values = tuple
            .iter()
            .map(|item| stable_live_value_repr(&item))
            .collect::<Result<Vec<_>>>()?;
        return Ok(match values.as_slice() {
            [value] => format!("({value},)"),
            values => format!("({})", values.join(", ")),
        });
    }

    if value.is_exact_instance_of::<pyo3::types::PyFrozenSet>() {
        let frozen_set = value
            .cast::<pyo3::types::PyFrozenSet>()
            .map_err(pyo3::PyErr::from)?;
        let mut values = frozen_set
            .iter()
            .map(|item| stable_live_value_repr(&item))
            .collect::<Result<Vec<_>>>()?;
        values.sort();
        return Ok(if values.is_empty() {
            "frozenset()".to_owned()
        } else {
            format!("frozenset({{{}}})", values.join(", "))
        });
    }

    Ok(format!("<{} instance>", stable_live_type_label(value)))
}

fn stable_live_type_label(value: &pyo3::Bound<pyo3::types::PyAny>) -> String {
    let value_type = value.get_type();
    let module = stable_type_string_attr(&value_type, "__module__");
    let qualname = stable_type_string_attr(&value_type, "__qualname__");

    if let (Some(module), Some(qualname)) = (&module, &qualname) {
        return format!("{module}.{qualname}");
    }
    if let Some(qualname) = qualname {
        return qualname;
    }
    stable_type_string_attr(&value_type, "__name__").unwrap_or_else(|| "opaque".to_owned())
}

fn stable_type_string_attr(
    value_type: &pyo3::Bound<pyo3::types::PyType>,
    name: &str,
) -> Option<String> {
    value_type
        .getattr(name)
        .ok()?
        .extract::<String>()
        .ok()
        .filter(|value| !value.is_empty())
}

/// Whether `value` is accepted by rustc as a `#[doc(alias = "...")]` payload. rustc rejects an
/// empty alias, any leading/trailing whitespace, and any `"`/`'` or whitespace-other-than-a-
/// plain-space (tab, newline, ...). A member whose Python name violates this still generates
/// under its sanitized Rust ident; only the searchable alias is dropped, so a validated IR
/// never yields a non-compiling crate.
fn is_valid_doc_alias(value: &str) -> bool {
    !value.is_empty()
        && value.trim() == value
        && !value
            .chars()
            .any(|c| c == '"' || c == '\'' || (c.is_whitespace() && c != ' '))
}

/// Neutralizes control characters (newlines, tabs, ...) in a value interpolated into a
/// generated doc comment, so an IR-authored member name or value repr cannot inject a fenced
/// code block (a rustdoc doctest) or otherwise break the single-line comment.
fn doc_span(value: &str) -> String {
    value
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}
