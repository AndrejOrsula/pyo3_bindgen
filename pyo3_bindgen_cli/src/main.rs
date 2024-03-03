//! CLI tool for automatic generation of Rust FFI bindings to Python modules.

use clap::Parser;
use std::io::Write;

fn main() {
    // Parse the CLI arguments
    let args = Args::parse();

    // Generate the bindings for the module specified by the `--module-name` argument
    let bindings = args
        .module_names
        .iter()
        .fold(pyo3_bindgen::Codegen::default(), |codegen, module_name| {
            codegen.module_name(module_name).unwrap_or_else(|err| {
                panic!("Failed to parse the content of '{module_name}' Python module:\n{err}")
            })
        })
        .generate()
        .unwrap_or_else(|err| panic!("Failed to generate bindings for Python modules:\n{err}"));

    // Format the bindings with prettyplease
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
        std::io::stdout().write_all(bindings.as_bytes()).unwrap();
    }
}

/// Arguments for the CLI tool
#[derive(Parser)]
#[command(author, version, about)]
struct Args {
    #[arg(short='m', long="module-name", required=true, num_args=1..)]
    /// Name of the Python module for which to generate the bindings
    pub module_names: Vec<String>,
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
        let input = ["", "-m", "os", "--output", "bindings.rs"];

        // Act
        let args = Args::parse_from(input);

        // Assert
        assert_eq!(args.module_names, ["os"]);
        assert_eq!(args.output, Some("bindings.rs".into()));
    }

    #[test]
    fn test_parser_short() {
        // Arrange
        let input = ["", "-m", "sys"];

        // Act
        let args = Args::parse_from(input);

        // Assert
        assert_eq!(args.module_names, ["sys"]);
    }

    #[test]
    fn test_parser_long() {
        // Arrange
        let input = ["", "--module-name", "io"];

        // Act
        let args = Args::parse_from(input);

        // Assert
        assert_eq!(args.module_names, ["io"]);
    }

    #[test]
    fn test_parser_multiple() {
        // Arrange
        let input = ["", "-m", "os", "sys", "--module-name", "io"];

        // Act
        let args = Args::parse_from(input);

        // Assert
        assert_eq!(args.module_names, ["os", "sys", "io"]);
    }
}
