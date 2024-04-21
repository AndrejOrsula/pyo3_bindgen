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
  """Returns answer to a question."""

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
/// Returns answer to a question.
pub fn answer_to<'py>(
  py: ::pyo3::Python<'py>,
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

On its own, the generated Rust code does not provide any performance benefits over using the Python code. However, it can be used as a starting point for further optimization if you decide to rewrite performance-critical parts of your codebase in pure Rust.

## Overview

The workspace contains these packages:

- **[pyo3_bindgen](pyo3_bindgen):** Public API for generation of bindings (in `build.rs` or via procedural macros)
- **[pyo3_bindgen_cli](pyo3_bindgen_cli):** CLI tool for generation of bindings via `pyo3_bindgen` executable
- **[pyo3_bindgen_engine](pyo3_bindgen_engine):** The underlying engine for generation of bindings
- **[pyo3_bindgen_macros](pyo3_bindgen_macros):** Procedural macros for in-place generation

## Instructions

### <a href="#-option-1-build-script"><img src="https://rustacean.net/assets/rustacean-flat-noshadow.svg" width="16" height="16"></a> Option 1: Build script

First, add `pyo3_bindgen` as a **build dependency** to your [`Cargo.toml`](https://doc.rust-lang.org/cargo/reference/manifest.html) manifest. To actually use the generated bindings, you will also need to add `pyo3` as a regular dependency (or use the re-exported `pyo3_bindgen::pyo3` module).

```toml
[build-dependencies]
pyo3_bindgen = { version = "0.4" }

[dependencies]
pyo3 = { version = "0.20", features = ["auto-initialize"] }
```

Then, create a [`build.rs`](https://doc.rust-lang.org/cargo/reference/build-scripts.html) script in the root of your crate that generates bindings to the selected Python modules. In this example, the bindings are simultaneously generated for the "os", "posixpath", and "sys" Python modules. At the end of the generation process, the Rust bindings are written to `${OUT_DIR}/bindings.rs`.

> \[!TIP\]
> With this approach, you can also customize the generation process via [`pyo3_bindgen::Config`](https://docs.rs/pyo3_bindgen/latest/pyo3_bindgen/struct.Config.html) that can be passed to the constructor, e.g. `Codegen::new(Config::builder().include_private(true).build())`.

```rs
//! build.rs
use pyo3_bindgen::Codegen;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    Codegen::default()
        .module_names(["os", "posixpath", "sys"])?
        .build(format!("{}/bindings.rs", std::env::var("OUT_DIR")?))?;
    Ok(())
}
```

Afterwards, you can include the generated Rust code via the `include!` macro anywhere in your crate and use the generated bindings as regular Rust modules. However, the bindings must be used within the `pyo3::Python::with_gil` closure to ensure that Python [GIL](https://wiki.python.org/moin/GlobalInterpreterLock) is held.

```rs
//! src/main.rs
include!(concat!(env!("OUT_DIR"), "/bindings.rs"));

fn main() -> pyo3::PyResult<()> {
    pyo3::Python::with_gil(|py| {
        // Get the path to the Python executable via "sys" Python module
        let python_exe_path = sys::executable(py)?;
        // Get the current working directory via "os" Python module
        let current_dir = os::getcwd(py)?;
        // Get the relative path to the Python executable via "posixpath" Python module
        let relpath_to_python_exe = posixpath::relpath(py, python_exe_path, current_dir)?;

        println!("Relative path to Python executable: '{relpath_to_python_exe}'");
        Ok(())
    })
}
```

### <a href="#-option-2-procedural-macros-experimental"><img src="https://www.svgrepo.com/show/269868/lab.svg" width="16" height="16"></a> Option 2: Procedural macros (experimental)

As an alternative to build scripts, you can use procedural macros to generate the bindings in-place. First, add `pyo3_bindgen_macros` as a **regular dependency** to your [`Cargo.toml`](https://doc.rust-lang.org/cargo/reference/manifest.html) manifest.

```toml
[dependencies]
pyo3_bindgen = { version = "0.4" }
```

Subsequently, the `import_python!` macro can be used to generate Rust bindings for the selected Python modules anywhere in your crate. As demonstrated in the example below, Rust bindings are generated for the "math" Python module and can directly be used in the same scope. Similar to the previous approach, the generated bindings must be used within the `pyo3::Python::with_gil` closure to ensure that Python [GIL](https://wiki.python.org/moin/GlobalInterpreterLock) is held.

> \[!NOTE\]
> As opposed to using build scripts, this approach does not offer the same level of customization via `pyo3_bindgen::Config`. Furthermore, the procedural macro is quite experimental and might not work in all cases.

```rs
use pyo3_bindgen::import_python;
import_python!("math");

// Which Pi do you prefer?
// a) 🐍 Pi from Python "math" module
// b) 🦀 Pi from Rust standard library
// c) 🥧 Pi from your favourite bakery
pyo3::Python::with_gil(|py| {
    let python_pi = math::pi(py).unwrap();
    let rust_pi = std::f64::consts::PI;
    assert_eq!(python_pi, rust_pi);
})
```

### <a href="#-option-3-cli-tool"><img src="https://www.svgrepo.com/show/353478/bash-icon.svg" width="16" height="16"></a> Option 3: CLI tool

For a quick start and testing purposes, you can use the `pyo3_bindgen` executable to generate and inspect bindings for the selected Python modules. The executable is available as a standalone package and can be installed via `cargo`.

```bash
cargo install --locked pyo3_bindgen_cli
```

Afterwards, run the `pyo3_bindgen` executable to generate Rust bindings for the selected Python modules. The generated bindings are printed to STDOUT by default, but they can also be written to a file via the `-o` option (see `pyo3_bindgen --help` for more options).

```bash
pyo3_bindgen -m os sys numpy -o bindings.rs
```

## Status

This project is in early development, and as such, the API of the generated bindings is not yet stable.

- The binding generation is primarily designed to be used inside build scripts or via procedural macros. Therefore, the performance of the codegen process is [benchmarked](./pyo3_bindgen_engine/benches/bindgen.rs) to understand the potential impact on build times. Here are some preliminary results for version `0.3` with the default configuration (measured: parsing IO & codegen | not measured: compilation of the generated bindings, which takes much longer):
  - `sys`: 1.24 ms (0.66k total LoC)
  - `os`: 8.38 ms (3.88k total LoC)
  - `numpy`: 1.02 s (294k total LoC)
  - `torch`: 7.05 s (1.08M total LoC)
- The generation of bindings should never panic as long as the target Python module can be successfully imported. If it does, please [report](https://github.com/AndrejOrsula/pyo3_bindgen/issues/new) this as a bug.
- The generated bindings should always be compilable and usable in Rust. If you encounter any issues, consider manually fixing the problematic parts of the bindings and please [report](https://github.com/AndrejOrsula/pyo3_bindgen/issues/new) this as a bug.
- However, the generated bindings are based on the introspection of the target Python module. Therefore, the completeness and correctness of the generated bindings are directly dependent on the quality of the module structure, type annotations and docstrings in the target Python module. Ideally, the generated bindings should be considered unsafe and serve as a starting point for safe and idiomatic Rust APIs. If you find that something in the generated bindings is incorrect or missing, please [report](https://github.com/AndrejOrsula/pyo3_bindgen/issues/new) this as well.
- Not all Python types are mapped to their Rust equivalents yet. For this reason, some additional type-casting might be required when using the generated bindings (e.g. `let typed_value: MyType = any_value.extract()?;`).
- Although implemented, the procedural macro might not work in many cases. Therefore, it is recommended that the build scripts be used wherever possible.

## License

This project is dual-licensed to be compatible with the Rust project, under either the [MIT](LICENSE-MIT) or [Apache 2.0](LICENSE-APACHE) licenses.

## Contributing

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
