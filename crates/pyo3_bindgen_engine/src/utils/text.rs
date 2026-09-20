/// Byte length of `docstring` after [`format_docstring`] -- the size it actually occupies
/// once emitted (whitespace / backtick / tilde runs collapsed).
pub fn formatted_docstring_len(docstring: &str) -> usize {
    let mut formatted = docstring.to_owned();
    format_docstring(&mut formatted);
    formatted.len()
}

/// Emitted byte length of an optional member docstring under `cfg`: the
/// [`formatted_docstring_len`] when documentation generation is enabled, else `0` (the flat
/// model omits the `#[doc]` entirely). The deduplication cost model must weigh the bytes
/// actually emitted -- not the raw introspected docstring -- or it would over-credit the
/// saving from dropping a documented member and could *enlarge* a docstring-heavy or
/// docs-disabled build instead of shrinking it.
pub fn emitted_docstring_len(cfg: &crate::Config, docstring: Option<&str>) -> i64 {
    if !cfg.generate_docs {
        return 0;
    }
    docstring.map_or(0, |doc| formatted_docstring_len(doc) as i64)
}

/// Sanitize and format the given docstring.
pub fn format_docstring(docstring: &mut String) {
    // Remove leading and trailing whitespace for each line
    *docstring = docstring
        .lines()
        .map(str::trim)
        .collect::<Vec<_>>()
        .join("\n");

    // NOTE: a trailing `/` is deliberately preserved. Docstrings routinely end in a
    // URL or a path whose final slash is meaningful, and it is safe in every form
    // the docstring is rendered as: both `/// text/` and `/** text/\n*/` terminate
    // correctly, and a docstring containing `*/` never becomes a block comment in
    // the first place.

    // Remove duplicate whitespace characters (except line breaks)
    conditioned_dedup(docstring, |c| c.is_whitespace() && c != '\n');

    // Remove duplicate backticks to avoid potential doctests
    conditioned_dedup(docstring, |c| c == '`');

    // Likewise collapse tilde runs: rustdoc treats a `~~~` fence as a code block and
    // would compile it as a doctest in the generated crate, exactly like a ``` fence.
    conditioned_dedup(docstring, |c| c == '~');

    // If the docstring has multiple lines, make sure it is properly formatted
    if docstring.contains('\n') {
        // Make sure the first line is not empty
        while docstring.starts_with('\n') {
            docstring.remove(0);
        }
        // Make sure it ends with a single newline
        if docstring.ends_with('\n') {
            while docstring.ends_with("\n\n") {
                docstring.pop();
            }
        } else {
            docstring.push('\n');
        }
    }
    // Pad the docstring with a leading whitespace (looks better in the generated code)
    docstring.insert(0, ' ');
}

/// Remove duplicate characters from the input string that satisfy the given predicate.
fn conditioned_dedup(input: &mut String, mut predicate: impl FnMut(char) -> bool) {
    let mut previous = None;
    input.retain(|c| {
        if predicate(c) {
            Some(c) != previous.replace(c)
        } else {
            previous = None;
            true
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn docstring_keeps_a_meaningful_trailing_slash() {
        let mut docstring = "See https://example.com/docs/".to_owned();
        format_docstring(&mut docstring);
        assert_eq!(docstring, " See https://example.com/docs/");
    }

    #[test]
    fn multiline_docstring_keeps_a_meaningful_trailing_slash() {
        let mut docstring = "Summary.\n\nSee https://example.com/docs/".to_owned();
        format_docstring(&mut docstring);
        assert_eq!(docstring, " Summary.\n\nSee https://example.com/docs/\n");
    }

    #[test]
    fn docstring_trims_each_line_and_dedups_spaces() {
        let mut docstring = "  Summary   with   spaces  \n\t indented  ".to_owned();
        format_docstring(&mut docstring);
        assert_eq!(docstring, " Summary with spaces\nindented\n");
    }

    #[test]
    fn docstring_dedups_backticks_to_avoid_doctests() {
        let mut docstring = "Use ```python``` here".to_owned();
        format_docstring(&mut docstring);
        assert_eq!(docstring, " Use `python` here");
    }

    #[test]
    fn docstring_dedups_tildes_to_avoid_doctests() {
        let mut docstring = "Use ~~~python~~~ here".to_owned();
        format_docstring(&mut docstring);
        assert_eq!(docstring, " Use ~python~ here");
    }
}
