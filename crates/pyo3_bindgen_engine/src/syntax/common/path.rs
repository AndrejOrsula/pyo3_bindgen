pub(crate) use crate::planning::Path;

use itertools::Itertools;

impl Path {
    /// Resolve an external-IR import against its normalized module set.
    pub(crate) fn import_quote_static(
        &self,
        known_modules: &rustc_hash::FxHashSet<Path>,
    ) -> proc_macro2::TokenStream {
        // Find the longest prefix that is a known module, then get the rest via getattr()
        let mut package_path = self.root().unwrap_or_else(|| unreachable!());
        for i in (1..self.len()).rev() {
            let module_name = Self::from(&self[..i]);
            if known_modules.contains(&module_name) {
                package_path = module_name;
                break;
            }
        }

        self.import_quote_with_package(&package_path)
    }

    /// Shared, Python-free token emission for pre-resolved import facts.
    fn import_quote_with_package(&self, package_path: &Path) -> proc_macro2::TokenStream {
        let remaining_path = self
            .strip_prefix(&package_path[..])
            .unwrap_or_else(|| unreachable!());

        let package_path = package_path.to_py();
        let remaining_path = remaining_path
            .iter()
            .map(|ident| ident.as_py().to_owned())
            .collect_vec();

        remaining_path.into_iter().fold(
            quote::quote! { py.import(::pyo3::intern!(py, #package_path))? },
            |acc, ident| {
                quote::quote! { ::pyo3::types::PyAnyMethods::getattr(#acc.as_any(), ::pyo3::intern!(py, #ident))? }
            },
        )
    }
}

/// Selects the immutable import facts and object model consumed during emission.
#[derive(Clone)]
pub(crate) enum ImportMode {
    /// Live import prefixes captured before emission. `deduplicated` controls only the emitted
    /// object model; import resolution is identical for both live candidates.
    Live {
        prefixes: crate::planning::ImportPrefixes,
        deduplicated: bool,
    },
    /// Resolve imports against a static set of known module paths (IR path).
    Static(rustc_hash::FxHashSet<Path>),
}

impl ImportMode {
    pub(crate) fn resolve(&self, path: &Path) -> crate::Result<proc_macro2::TokenStream> {
        match self {
            Self::Live { prefixes, .. } => {
                Ok(path.import_quote_with_package(prefixes.package_for(path)?))
            }
            Self::Static(known_modules) => Ok(path.import_quote_static(known_modules)),
        }
    }

    pub(crate) fn uses_dedup_model(&self) -> bool {
        matches!(
            self,
            Self::Static(_)
                | Self::Live {
                    deduplicated: true,
                    ..
                }
        )
    }

    pub(crate) fn is_static(&self) -> bool {
        matches!(self, Self::Static(_))
    }

    pub(crate) fn is_live_deduplicated(&self) -> bool {
        matches!(
            self,
            Self::Live {
                deduplicated: true,
                ..
            }
        )
    }

    pub(crate) fn with_flat_live_model(&self) -> Self {
        match self {
            Self::Live { prefixes, .. } => Self::Live {
                prefixes: prefixes.clone(),
                deduplicated: false,
            },
            Self::Static(modules) => Self::Static(modules.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::Ident;

    #[test]
    fn test_from_rs() {
        let path = Path::from_rs("long::path::to");
        assert_eq!(path.to_rs(), "long::path::to");
        assert_eq!(path.to_py(), "long.path.to");
        assert_eq!(path.into_rs(), "long::path::to");
    }

    #[test]
    fn test_from_rs_leading_colon() {
        let path = Path::from_rs("::long::path::to");
        assert_eq!(path.to_rs(), "::long::path::to");
        assert_eq!(path.to_py(), "long.path.to");
    }

    #[test]
    fn test_from_py() {
        let path = Path::from_py("long.path.to");
        assert_eq!(path.to_py(), "long.path.to");
        assert_eq!(path.to_rs(), "long::path::to");
    }

    #[test]
    fn test_from_py_relative() {
        let path = Path::from_py("..long.path.to");
        assert_eq!(path.to_py(), "..long.path.to");
        assert_eq!(path.to_rs(), "super::super::long::path::to");
    }

    #[test]
    fn test_from_py_keyword() {
        let path = Path::from_py("mod.struct");
        assert_eq!(path.to_py(), "mod.struct");
        assert_eq!(path.to_rs(), "r#mod::r#struct");
    }

    #[test]
    fn live_module_literal_super_segments_roundtrip() {
        for module_name in ["super", "pkg.super"] {
            let path = Path::from_live_module(module_name).unwrap();
            assert_eq!(path.to_py(), module_name);
        }
    }

    #[test]
    fn test_name() {
        let path = Path::from_rs("long::path::to");
        assert_eq!(path.name().as_rs(), "to");
    }

    #[test]
    fn test_root() {
        let path = Path::from_rs("long::path::to");
        assert_eq!(path.root().unwrap().to_rs(), "long");
    }

    #[test]
    fn test_parent() {
        let path = Path::from_rs("long::path::to");
        assert_eq!(path.parent().unwrap().to_rs(), "long::path");
    }

    #[test]
    fn test_into_syn() {
        let path = Path::from_rs("long::path::to");
        let _syn_path: syn::Path = (&path).try_into().unwrap();
        let _syn_path: syn::Path = path.try_into().unwrap();
    }

    /// The load-bearing soundness guarantee behind lazy `LocalType::rust_path`:
    /// `Path::is_syn_parseable` must be a strict subset of what `syn` accepts.
    #[test]
    fn is_syn_parseable_is_a_subset_of_syn_parse() {
        let segments = [
            "a",
            "_a",
            "A9",
            "value",
            "Value",
            "super",
            "self",
            "Self",
            "crate",
            "fn",
            "type",
            "struct",
            "match",
            "as",
            "true",
            "async",
            "_",
            "r#type",
            "r#fn",
            "r#match",
            "r#async",
            "r#union",
            "r#_",
            "r#crate",
            "r#self",
            "r#Self",
            "r#super",
            "union",
            "macro_rules",
            "café",
            "中文",
            "9bad",
            "a-b",
            "a+b",
        ];

        let build = |lc: bool, parts: &[&str]| {
            let segments: Vec<Ident> = parts.iter().map(|s| Ident::from_rs(s)).collect();
            let mut path = Path::from(segments.as_slice());
            path.leading_colon = lc;
            path
        };

        let mut proven_parseable = 0_usize;
        let check = |path: &Path, proven: &mut usize| {
            if path.is_syn_parseable() {
                assert!(
                    syn::parse_str::<syn::Path>(&path.to_rs()).is_ok(),
                    "is_syn_parseable over-accepted {:?} (to_rs = {:?}), which syn rejects",
                    path,
                    path.to_rs(),
                );
                *proven += 1;
            }
        };

        for &s1 in &segments {
            for lc in [false, true] {
                check(&build(lc, &[s1]), &mut proven_parseable);
            }
            for &s2 in &segments {
                for lc in [false, true] {
                    check(&build(lc, &[s1, s2]), &mut proven_parseable);
                    check(
                        &build(lc, &["super", s1, "modx", s2]),
                        &mut proven_parseable,
                    );
                }
            }
        }

        assert!(proven_parseable > 0);
        assert!(Path::from_rs("super::super::modx::Foo").is_syn_parseable());
        assert!(Path::from_rs("self").is_syn_parseable());
        assert!(Path::from_rs("::a::b").is_syn_parseable());
    }

    #[test]
    fn is_syn_parseable_rejects_empty_path() {
        assert!(!Path::default().is_syn_parseable());
        assert!(syn::parse_str::<syn::Path>(&Path::default().to_rs()).is_err());
    }

    /// The tokenizer-free conversion is an optimization, so it must be
    /// indistinguishable from the parser it replaces: same acceptance decision and,
    /// when accepted, the same rendered tokens. Spans are deliberately excluded
    /// because generation only ever renders these values.
    #[test]
    fn syn_conversions_match_the_parser_they_replace() {
        use quote::ToTokens as _;

        let spellings = [
            "a",
            "_a",
            "A9",
            "value",
            "Value",
            "super",
            "self",
            "Self",
            "crate",
            "fn",
            "type",
            "match",
            "true",
            "async",
            "union",
            "macro_rules",
            "_",
            "r#type",
            "r#fn",
            "r#union",
            "r#_",
            "r#crate",
            "r#self",
            "café",
            "中文",
            "9bad",
            "a-b",
        ];

        let mut converted_idents = 0_usize;
        let mut converted_paths = 0_usize;

        for spelling in spellings {
            let ident = Ident::from_rs(spelling);
            let converted = syn::Ident::try_from(&ident);
            let parsed = syn::parse_str::<syn::Ident>(ident.as_rs());
            match (&converted, &parsed) {
                (Ok(converted), Ok(parsed)) => {
                    assert_eq!(
                        converted.to_token_stream().to_string(),
                        parsed.to_token_stream().to_string(),
                        "ident conversion diverged from the parser for {spelling:?}",
                    );
                    converted_idents += 1;
                }
                (Err(_), Err(_)) => {}
                _ => panic!(
                    "ident conversion and the parser disagreed on {spelling:?}: \
                     converted ok = {}, parsed ok = {}",
                    converted.is_ok(),
                    parsed.is_ok(),
                ),
            }
            // Owned and borrowed conversions must agree with each other too.
            assert_eq!(
                syn::Ident::try_from(ident.clone()).is_ok(),
                converted.is_ok(),
                "owned and borrowed ident conversions disagreed on {spelling:?}",
            );

            for leading_colon in [false, true] {
                for parts in [
                    vec![spelling],
                    vec!["super", spelling],
                    vec![spelling, "modx"],
                ] {
                    let segments: Vec<Ident> = parts.iter().map(|s| Ident::from_rs(s)).collect();
                    let mut path = Path::from(segments.as_slice());
                    path.leading_colon = leading_colon;

                    let converted = syn::Path::try_from(&path);
                    let parsed = syn::parse_str::<syn::Path>(&path.to_rs());
                    match (&converted, &parsed) {
                        (Ok(converted), Ok(parsed)) => {
                            assert_eq!(
                                converted.to_token_stream().to_string(),
                                parsed.to_token_stream().to_string(),
                                "path conversion diverged from the parser for {:?}",
                                path.to_rs(),
                            );
                            converted_paths += 1;
                        }
                        (Err(_), Err(_)) => {}
                        _ => panic!(
                            "path conversion and the parser disagreed on {:?}: \
                             converted ok = {}, parsed ok = {}",
                            path.to_rs(),
                            converted.is_ok(),
                            parsed.is_ok(),
                        ),
                    }
                    assert_eq!(
                        syn::Path::try_from(path.clone()).is_ok(),
                        converted.is_ok(),
                        "owned and borrowed path conversions disagreed on {:?}",
                        path.to_rs(),
                    );
                }
            }
        }

        assert!(converted_idents > 0 && converted_paths > 0);
    }
}
