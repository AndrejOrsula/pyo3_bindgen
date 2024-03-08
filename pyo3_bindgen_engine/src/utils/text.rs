/// Sanitize and format the given docstring.
pub fn format_docstring(docstring: &mut String) {
    // Remove leading and trailing whitespace for each line
    *docstring = docstring
        .lines()
        .map(str::trim)
        .collect::<Vec<_>>()
        .join("\n");

    // Remove trailing slashes
    while docstring.ends_with('/') {
        docstring.pop();
        docstring.truncate(docstring.trim_end().len());
    }

    // Remove duplicate whitespace characters (except line breaks)
    conditioned_dedup(docstring, |c| c.is_whitespace() && c != '\n');

    // Remove duplicate backticks to avoid potential doctests
    conditioned_dedup(docstring, |c| c == '`');

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
            Some(c) != std::mem::replace(&mut previous, Some(c))
        } else {
            previous = None;
            true
        }
    });
}
