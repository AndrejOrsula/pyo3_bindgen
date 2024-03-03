//! Parsing of procedural macro arguments.

use syn::{
    parse::{Parse, ParseStream, Result},
    LitStr,
};

/// Arguments for the `import_python` procedural macro.
pub struct Args {
    /// Name of the Python module for which to generate the bindings.
    pub module_name: String,
}

impl Parse for Args {
    fn parse(input: ParseStream) -> Result<Self> {
        // Python module name might contain dots, so it is parsed as a string literal
        let module_name = input.parse::<LitStr>()?.value();
        Ok(Args { module_name })
    }
}
