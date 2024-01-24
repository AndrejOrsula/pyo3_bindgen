//! CLI tool for automatic generation of Rust FFI bindings to Python modules.

use clap::Parser;

fn main() {
    // Parse the CLI arguments
    let args = Args::parse();

    // Generate the bindings for the module specified by the `--module-name` argument
    let bindings = pyo3_bindgen::generate_bindings(&args.module_name).unwrap_or_else(|err| {
        panic!(
            "Failed to generate bindings for module: {}\n{err}",
            args.module_name
        )
    });

    let bindings = prettyplease::unparse(&syn::parse2(bindings).unwrap());

    if let Some(output) = args.output {
        // Write the bindings to a file if the `--output` argument is provided
        if let Some(parent) = output.parent() {
            std::fs::create_dir_all(parent).unwrap_or_else(|_| {
                panic!("Failed to create output directory: {}", parent.display())
            });
        }
        std::fs::write(&output, &bindings)
            .unwrap_or_else(|_| panic!("Failed to write to file: {}", output.display()));
    } else {
        // Otherwise, print the bindings to STDOUT
        println!("{bindings}");
    }
}

/// Arguments for the CLI tool
#[derive(Parser)]
#[command(author, version, about)]
struct Args {
    #[arg(short, long)]
    /// Name of the Python module for which to generate the bindings
    pub module_name: String,
    #[arg(short, long)]
    /// Name of the output file to which to write the bindings [default: STDOUT]
    pub output: Option<std::path::PathBuf>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parser_all() {
        // Arrange
        let input = ["", "-m", "pip", "--output", "bindings.rs"];

        // Act
        let args = Args::parse_from(input);

        // Assert
        assert_eq!(args.module_name, "pip");
        assert_eq!(args.output, Some("bindings.rs".into()));
    }

    #[test]
    fn test_parser_short() {
        // Arrange
        let input = ["", "-m", "numpy"];

        // Act
        let args = Args::parse_from(input);

        // Assert
        assert_eq!(args.module_name, "numpy");
    }

    #[test]
    fn test_parser_long() {
        // Arrange
        let input = ["", "--module-name", "setuptools"];

        // Act
        let args = Args::parse_from(input);

        // Assert
        assert_eq!(args.module_name, "setuptools");
    }
}
