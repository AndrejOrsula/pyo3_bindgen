//! Parsing of procedural macro arguments.

use syn::{
    parse::{Parse, ParseStream, Result},
    LitStr,
};

/// Arguments for the `import_python` procedural macro.
pub struct Args {
    /// Name of the Python module for which to generate the bindings.
    pub module_name: String,
    /// The module-name string literal, retained so codegen failures can be reported
    /// as a `compile_error!` spanned at the macro call site (not a proc-macro panic).
    pub literal: LitStr,
}

impl Parse for Args {
    fn parse(input: ParseStream) -> Result<Self> {
        // Python module name might contain dots, so it is parsed as a string literal
        let literal = input.parse::<LitStr>()?;
        Ok(Args {
            module_name: literal.value(),
            literal,
        })
    }
}
