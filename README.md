# pyo3_bindgen

<p align="left">
  <a href="https://crates.io/crates/pyo3_bindgen">                                   <img alt="crates.io"  src="https://img.shields.io/crates/v/pyo3_bindgen.svg"></a>
  <a href="https://docs.rs/pyo3_bindgen">                                            <img alt="docs.rs"    src="https://docs.rs/pyo3_bindgen/badge.svg"></a>
  <a href="https://github.com/AndrejOrsula/pyo3_bindgen/actions/workflows/rust.yml"> <img alt="Rust"       src="https://github.com/AndrejOrsula/pyo3_bindgen/actions/workflows/rust.yml/badge.svg"></a>
  <a href="https://deps.rs/repo/github/AndrejOrsula/pyo3_bindgen">                   <img alt="deps.rs"    src="https://deps.rs/repo/github/AndrejOrsula/pyo3_bindgen/status.svg"></a>
  <a href="https://codecov.io/gh/AndrejOrsula/pyo3_bindgen">                         <img alt="codecov.io" src="https://codecov.io/gh/AndrejOrsula/pyo3_bindgen/branch/main/graph/badge.svg"></a>
</p>

Automatic generation of Rust FFI bindings to Python modules via [PyO3](https://pyo3.rs). Python modules are analyzed recursively to generate Rust bindings with an identical structure for all public classes, functions, properties, and constants. Any available docstrings and type annotations are also preserved in their Rust equivalents.

An example of a generated Rust function signature and its intended usage is shown below. Of course, manually wrapping parts of the generated bindings in a more idiomatic Rust API might be beneficial in most cases.

<table>
<tr><th><img src="https://www.svgrepo.com/show/354238/python.svg" width="12" height="12"></a> Source code (Python) <img src="https://www.svgrepo.com/show/354238/python.svg" width="12" height="12"></a></th><th><img src="https://rustacean.net/assets/rustacean-flat-noshadow.svg" width="12" height="12"> Generated code (Rust) <img src="https://rustacean.net/assets/rustacean-flat-noshadow.svg" width="12" height="12"></th></tr>
<tr>
<td>

```py
 
def answer_to(question: str) -> int:
  """Returns answer to question."""

  return 42

 
```

______________________________________________________________________

```py
 
def main():
  assert answer_to("life") == 42


if __name__ == "__main__":
  main()
 
```

</td>
<td>

```rs
/// Returns answer to question.
pub fn answer_to<'py>(
  py: ::pyo3::marker::Python<'py>,
  question: &str,
) -> ::pyo3::PyResult<i64> {
  ... // Calls function via `pyo3`
}
```

______________________________________________________________________

```rs
pub fn main() -> pyo3::PyResult<()> {
  pyo3::Python::with_gil(|py| {
    assert_eq!(
      answer_to(py, "universe")?, 42
    );
    Ok(())
  })
}
```

</td>
</tr>
</table>

This project is intended to simplify the integration or transition of existing Python codebases into Rust. You, as a developer, gain immediate access to the Rust type system and countless other benefits of modern compiled languages with the generated bindings. Furthermore, the entire stock of high-quality crates from [crates.io](https://crates.io) becomes at your disposal.

On its own, the generated Rust code does not provide any performance benefits over using the Python code (it might actually be slower — yet to be benchmarked). However, it can be used as a starting point for further optimization if you decide to rewrite performance-critical parts of your codebase in pure Rust.

## Overview

The workspace contains these packages:

- **[pyo3_bindgen](pyo3_bindgen):** Public API for generation of bindings (in `build.rs` or via procedural macros)
- **[pyo3_bindgen_cli](pyo3_bindgen_cli):** CLI tool for generation of bindings via `pyo3_bindgen` executable
- **[pyo3_bindgen_engine](pyo3_bindgen_engine):** The underlying engine for generation of bindings
- **[pyo3_bindgen_macros](pyo3_bindgen_macros):** \[Experimental\] Procedural macros for in-place generation

## Instructions

Add `pyo3` as a dependency and `pyo3_bindgen` as a build dependency to your [`Cargo.toml`](https://doc.rust-lang.org/cargo/reference/manifest.html) manifest (`auto-initialize` feature of `pyo3` is optional and shown here for your convenience).

```toml
[dependencies]
pyo3 = { version = "0.20", features = ["auto-initialize"] }

[build-dependencies]
pyo3_bindgen = { version = "0.1" }
```

### <a href="#-option-1-build-script"><img src="https://rustacean.net/assets/rustacean-flat-noshadow.svg" width="16" height="16"></a> Option 1: Build script

Create a [`build.rs`](https://doc.rust-lang.org/cargo/reference/build-scripts.html) script in the root of your crate that generates bindings to the `target_module` Python module.

```rs
// build.rs

fn main() {
    // Generate Rust bindings to the Python module
    pyo3_bindgen::build_bindings(
        "target_module",
        std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("bindings.rs"),
    )
    .unwrap();
}
```

Afterwards, include the generated bindings anywhere in your crate.

```rs
#[allow(non_camel_case_types, non_snake_case, non_upper_case_globals)]
pub mod target_module {
    include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
}
```

### <a href="#-option-2-cli-tool"><img src="https://www.svgrepo.com/show/353478/bash-icon.svg" width="16" height="16"></a> Option 2: CLI tool

Install the `pyo3_bindgen` executable with `cargo`.

```bash
cargo install --locked pyo3_bindgen_cli
```

Afterwards, run the `pyo3_bindgen` executable while passing the name of the target Python module.

```bash
# Pass `--help` to show the usage and available options
pyo3_bindgen -m target_module -o bindings.rs
```

### <a href="#-option-3-experimental-procedural-macros"><img src="https://www.svgrepo.com/show/269868/lab.svg" width="16" height="16"></a> Option 3 \[Experimental\]: Procedural macros

> **Note:** This feature is experimental and will probably fail in many cases. It is recommended to use build scripts instead.

Enable the `macros` feature of `pyo3_bindgen`.

```toml
[build-dependencies]
pyo3_bindgen = { version = "0.1", features = ["macros"] }
```

Then, you can call the `import_python!` macro anywhere in your crate.

```rs
#[allow(non_camel_case_types, non_snake_case, non_upper_case_globals)]
pub mod target_module {
    pyo3_bindgen::import_python!("target_module");
}
```

## Status

This project is in early development, and as such, the API of the generated bindings is not yet stable.

- Not all Python types are mapped to their Rust equivalents yet. Especially the support for mapping types of module-wide classes for which bindings are generated is also still missing. For this reason, a lot of boilerplate might be currently required when using the generated bindings (e.g. `let typed_value: target_module::Class = any_value.extract()?;`).
- The binding generation is primarily designed to be used inside build scripts or via procedural macros. Therefore, the performance of the codegen process is [benchmarked](./pyo3_bindgen_engine/benches/bindgen.rs) to understand the potential impact on build times. Surprisingly, even the initial unoptimized version of the engine is able to process the entire `numpy` 1.26.3 in ~300 ms on a *modern* laptop while generating 166k lines of formatted Rust code (line count includes documentation). Adding more features might increase this time, but there is also plenty of room for optimization in the current naive implementation.
- The generation of bindings should never panic as long as the target Python module can be successfully imported. If it does, it is a bug resulting from an unexpected edge-case Python module structure or an unforeseen combination of enabled PyO3 features.
- Although implemented, the procedural macros might not work in all cases - especially when some PyO3 features are enabled. In most cases, PyO3 fails to import the target Python module when used from within a `proc_macro` crate. Therefore, it is recommended to use build scripts instead for now.
- The code will be refactored and cleaned up in the upcoming releases. The current implementation is a result of a very quick prototype that was built to test the feasibility of the idea.

Please [report](https://github.com/AndrejOrsula/pyo3_bindgen/issues/new) any issues that you might encounter. Contributions are more than welcome! If you are looking for a place to start, consider searching for `TODO` comments in the codebase.

## License

This project is dual-licensed to be compatible with the Rust project, under either the [MIT](LICENSE-MIT) or [Apache 2.0](LICENSE-APACHE) licenses.

## Contributing

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
