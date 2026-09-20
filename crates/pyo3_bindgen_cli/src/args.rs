use clap::Parser;

pub(crate) const DEFAULT_MAX_IR_BYTES: usize = 64 * 1024 * 1024;

/// Process-level CLI arguments layered over the reusable generation arguments.
#[doc(hidden)]
#[derive(Debug, Parser)]
#[command(
    author,
    version,
    about,
    after_help = "Producer utility: pyo3_bindgen name-plan reads a versioned JSON batch from stdin and writes the canonical enum-variant name plan to stdout."
)]
pub struct CliArgs {
    #[command(flatten)]
    args: Args,
    #[arg(long, default_value_t = DEFAULT_MAX_IR_BYTES)]
    /// Maximum raw external-IR input size in bytes
    max_ir_bytes: usize,
    #[arg(long, conflicts_with = "ir")]
    /// Fail when optional live submodule or dependency discovery is incomplete
    strict_completeness: bool,
}

impl CliArgs {
    pub(crate) fn into_parts(self) -> (Args, usize, bool) {
        (self.args, self.max_ir_bytes, self.strict_completeness)
    }
}

/// Arguments for the CLI tool
#[derive(Debug, Parser)]
#[command(author, version, about)]
pub struct Args {
    #[arg(short='m', long="module-name", required_unless_present="ir", num_args=1..)]
    /// Name of the Python module for which to generate the bindings
    pub module_names: Vec<String>,
    #[arg(long, conflicts_with = "module_names")]
    /// Path to a JSON SchemaIr file to drive generation instead of live introspection
    pub ir: Option<std::path::PathBuf>,
    #[arg(short, long, conflicts_with = "output_dir")]
    /// Name of the output file to which to write the bindings [default: STDOUT]
    pub output: Option<std::path::PathBuf>,
    #[arg(long, conflicts_with = "output")]
    /// Directory in which to write bindings.rs
    pub output_dir: Option<std::path::PathBuf>,
    #[arg(long)]
    /// Name of the JSON report file to write
    pub report: Option<std::path::PathBuf>,
    #[arg(long)]
    /// Fail if generated output differs from the existing output file
    pub check: bool,
    #[arg(long)]
    /// Fail if generation emits any warning
    pub deny_warnings: bool,
    #[arg(long)]
    /// Fail if generation emits more warnings than this budget
    pub warning_budget: Option<usize>,
    #[arg(long, conflicts_with = "ir")]
    /// Include private Python attributes during introspection (live-introspection only;
    /// the --ir path emits exactly the members present in the schema)
    pub include_private: bool,
    #[arg(long = "no-traverse-submodules")]
    /// Do not recursively traverse Python submodules
    pub no_traverse_submodules: bool,
    #[arg(long = "blocklist-name", conflicts_with = "ir")]
    /// Replace the default blocklisted Python attribute names (live-introspection only;
    /// the --ir path emits exactly the members present in the schema)
    pub blocklist_names: Vec<String>,
    #[arg(long = "no-deduplicate-inherited-members", conflicts_with = "ir")]
    /// Disable deduplication of inherited members: redeclare every inherited member on
    /// each subclass (the historical, larger output). By default, inherited members are
    /// shared through `*Methods` supertrait bounds wherever that shrinks the output
    /// (live-introspection only; the --ir path always emits the deduplicated model)
    pub no_deduplicate_inherited_members: bool,
    #[arg(long = "no-inherent-forwarders")]
    /// Omit the inherent `pub fn` forwarders on deduplicated-model newtypes: own and
    /// inherited members are then reached only through the `*Methods` traits (bring the
    /// trait into scope and call `obj.name(py)`). A large size reduction for wide
    /// hierarchies; applies to the --ir path and the deduplicated live-introspection path
    pub no_inherent_forwarders: bool,
    #[arg(long = "lenient-ir", conflicts_with = "module_names")]
    /// Accept flag/open external-IR enums without a per-enum `plain_mirror` opt-in,
    /// emitting plain mirror enums with the flag/open semantics dropped. External-IR path only
    pub lenient_ir: bool,
    #[arg(long = "enum-class-fallback", conflicts_with = "module_names")]
    /// Emit an owning Python object wrapper for any external-IR enum that cannot be
    /// represented as a Rust mirror enum (colliding/unrepresentable member idents, a
    /// dangling alias target, or no members), instead of aborting generation. References
    /// to the enum then lower to the owning newtype and `set[E]` lowers to `Bound<PySet>`
    /// rather than `HashSet<E>`. External-IR path only
    pub enum_class_fallback: bool,
    #[arg(long = "enum-string-backed", conflicts_with = "module_names")]
    /// Treat external-IR mirror enums as string-backed: their Python representation is the
    /// member string (e.g. Blender RNA enum properties like `obj.rotation_mode == 'XYZ'`),
    /// not a Python `enum.Enum` class. Enums then round-trip via their string value instead
    /// of importing a Python enum class that does not exist. External-IR path only
    pub enum_string_backed: bool,
}

impl Args {
    pub(crate) fn output_target(&self) -> Result<Option<std::path::PathBuf>, String> {
        match (&self.output, &self.output_dir) {
            (Some(output), None) => Ok(Some(output.clone())),
            (None, Some(output_dir)) => Ok(Some(output_dir.join("bindings.rs"))),
            (None, None) => Ok(None),
            (Some(_), Some(_)) => {
                Err("--output and --output-dir cannot be used together".to_owned())
            }
        }
    }
    pub(crate) fn config(&self) -> pyo3_bindgen::Config {
        let mut cfg = pyo3_bindgen::Config::default();
        if self.include_private {
            cfg.include_private = true;
        }
        if self.no_traverse_submodules {
            cfg.traverse_submodules = false;
        }
        if !self.blocklist_names.is_empty() {
            cfg.blocklist_names = self.blocklist_names.clone();
        }
        if self.no_deduplicate_inherited_members {
            cfg.deduplicate_inherited_members = false;
        }
        if self.no_inherent_forwarders {
            cfg.generate_inherent_forwarders = false;
        }
        if self.lenient_ir {
            cfg.lenient_external_ir = true;
        }
        if self.enum_class_fallback {
            cfg.enum_ir_class_fallback = true;
        }
        if self.enum_string_backed {
            cfg.enum_ir_string_backed = true;
        }
        cfg
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_ir_byte_limit_has_conservative_default_and_override() {
        let default = CliArgs::try_parse_from(["pyo3_bindgen", "--ir", "schema.json"]).unwrap();
        assert_eq!(default.max_ir_bytes, DEFAULT_MAX_IR_BYTES);

        let explicit = CliArgs::try_parse_from([
            "pyo3_bindgen",
            "--ir",
            "schema.json",
            "--max-ir-bytes",
            "4096",
        ])
        .unwrap();
        assert_eq!(explicit.max_ir_bytes, 4096);
    }

    #[test]
    fn strict_completeness_is_private_live_cli_policy() {
        let default = CliArgs::try_parse_from(["pyo3_bindgen", "--module-name", "math"])
            .expect("default live arguments parse");
        assert!(!default.strict_completeness);

        let strict = CliArgs::try_parse_from([
            "pyo3_bindgen",
            "--module-name",
            "math",
            "--strict-completeness",
        ])
        .expect("strict live arguments parse");
        assert!(strict.strict_completeness);

        assert!(CliArgs::try_parse_from([
            "pyo3_bindgen",
            "--ir",
            "schema.json",
            "--strict-completeness",
        ])
        .is_err());
    }

    #[test]
    fn reusable_args_struct_literal_keeps_its_existing_public_shape() {
        let args = Args {
            module_names: vec!["math".to_owned()],
            ir: None,
            output: None,
            output_dir: None,
            report: None,
            check: false,
            deny_warnings: false,
            warning_budget: None,
            include_private: false,
            no_traverse_submodules: false,
            blocklist_names: Vec::new(),
            no_deduplicate_inherited_members: false,
            no_inherent_forwarders: false,
            lenient_ir: false,
            enum_class_fallback: false,
            enum_string_backed: false,
        };

        assert_eq!(args.module_names, ["math"]);
    }
}
