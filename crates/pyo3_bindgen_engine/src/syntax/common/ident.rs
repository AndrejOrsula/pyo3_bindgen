pub(crate) use crate::planning::Ident;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_rs() {
        let ident = Ident::from_rs("ident");
        assert_eq!(ident.as_rs(), "ident");
        assert_eq!(ident.as_py(), "ident");
        assert_eq!(ident.into_rs(), "ident");
    }

    #[test]
    fn test_from_py() {
        let ident = Ident::from_py("ident");
        assert_eq!(ident.as_rs(), "ident");
        assert_eq!(ident.as_py(), "ident");
    }

    #[test]
    fn test_from_py_keyword() {
        let ident = Ident::from_py("struct");
        assert_eq!(ident.as_rs(), "r#struct");
        assert_eq!(ident.as_py(), "struct");
    }

    #[test]
    fn live_empty_identifier_is_rejected() {
        let error = Ident::try_from_py("", || "empty live identifier".to_owned()).unwrap_err();
        assert!(error.to_string().contains("empty live identifier"));
    }

    #[test]
    fn test_into_syn() {
        let ident = Ident::from_rs("ident");
        let _syn_ident: syn::Ident = (&ident).try_into().unwrap();
        let _syn_ident: syn::Ident = ident.try_into().unwrap();
    }

    #[test]
    fn test_with_rs_override_diverges_spellings() {
        let ident = Ident::with_rs_override("_", "_arg0");
        assert_eq!(ident.as_rs(), "_arg0");
        assert_eq!(ident.as_py(), "_");
        assert_eq!(ident.to_string(), "_");
    }

    #[test]
    fn python_override_participates_in_identity() {
        let keyword = Ident::from_py("type");
        let literal_raw_name = Ident::from_py("r#type");

        assert_eq!(keyword.as_rs(), literal_raw_name.as_rs());
        assert_ne!(keyword.as_py(), literal_raw_name.as_py());
        assert_ne!(
            keyword, literal_raw_name,
            "distinct Python names must not collapse merely because they need the same Rust raw identifier"
        );
    }

    #[test]
    fn ident_fast_path_matches_syn_parse() {
        fn reference(value: &str) -> String {
            if syn::parse_str::<syn::Ident>(value).is_ok() {
                value.to_owned()
            } else {
                format!("r#{value}")
            }
        }

        let keywords = "as break const continue crate dyn else enum extern false fn for if impl in let loop match mod move mut pub ref return self Self static struct super trait true type unsafe use where while async await abstract become box do final macro override priv typeof unsized virtual yield try";
        let identifiers = concat!(
            "union macro_rules raw dyn2 unions _ __ _x _0 x_ value Value VALUE v1 ",
            "camelCase snake_case T a z9 asx fnn ret",
            "ur selfish Selff boxed café naïve Ελληνικά переменная 変数 αβγ 1abc 3 r#foo",
        );
        let non_identifiers = ["foo-bar", "foo.bar", "foo bar", "a+b"];

        for value in keywords
            .split_whitespace()
            .chain(identifiers.split_whitespace())
            .chain(non_identifiers)
        {
            assert_eq!(
                Ident::from_py(value).as_rs(),
                reference(value),
                "py_to_rs fast path diverged from syn for {value:?}",
            );
        }
    }

    #[test]
    fn gen_keyword_is_raw_escaped_for_edition_2024() {
        assert_eq!(Ident::from_py("gen").as_rs(), "r#gen");
    }
}
