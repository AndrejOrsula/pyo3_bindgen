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
  pyo3::Python::attach(|py| {
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

> [!NOTE]
> Version `0.6` of `pyo3_bindgen` targets the `pyo3` `0.29` API, including `pyo3::Bound<'py, T>`, `IntoPyObject`, and `Python::attach`. Use version `0.4` of `pyo3_bindgen` if you require the old "GIL Refs" API.

## Overview

Publishable packages live under [`crates/`](crates/README.md); runnable,
non-publishable usage examples live under [`examples/`](examples/):

- **[pyo3_bindgen](https://github.com/AndrejOrsula/pyo3_bindgen/tree/main/crates/pyo3_bindgen):** Public API for generation of bindings (in `build.rs` or via procedural macros)
- **[pyo3_bindgen_cli](https://github.com/AndrejOrsula/pyo3_bindgen/tree/main/crates/pyo3_bindgen_cli):** CLI tool for generation of bindings via `pyo3_bindgen` executable
- **[pyo3_bindgen_engine](https://github.com/AndrejOrsula/pyo3_bindgen/tree/main/crates/pyo3_bindgen_engine):** The underlying engine for generation of bindings
- **[pyo3_bindgen_macros](https://github.com/AndrejOrsula/pyo3_bindgen/tree/main/crates/pyo3_bindgen_macros):** Procedural macros for in-place generation

Contracts and maintainer guidance are indexed under [`docs/`](docs/README.md).
The [release plan](docs/release_checklist.md) owns all unfinished work and publication gates.

Features of `pyo3_bindgen`:

- **`macros` \[experimental\]:** Enables `import_python!` macro from `pyo3_bindgen_macros` crate
- **`numpy` \[experimental\]:** Maps Python [`numpy::ndarray`](https://numpy.org/doc/stable/reference/generated/numpy.ndarray.html) annotations of unknown dtype to Rust [`numpy::PyUntypedArray`](https://docs.rs/numpy/latest/numpy/struct.PyUntypedArray.html)
- **`num-complex` \[experimental\]:** Maps Python `complex` annotations to [`num_complex::Complex64`](https://docs.rs/num-complex/latest/num_complex/type.Complex64.html)

These features change the generated Rust types. The crate compiling the generated
bindings must therefore declare the corresponding runtime dependencies directly:

```toml
[dependencies]
pyo3 = { version = "0.29", features = ["auto-initialize", "num-complex"] }
numpy = "0.29"       # required by generated `numpy` mappings
num-complex = "0.4"  # required by generated `num-complex` mappings
```

Without these generator features, `complex` remains a `Bound<PyComplex>` and an
`ndarray` annotation falls back to `PyAny`.

## Instructions

> [!WARNING]
> Live generation imports and introspects the requested Python modules. Imports,
> descriptors, annotations, and package initialization can execute arbitrary code with the
> generator process's privileges. Only introspect trusted Python environments;
> use a reviewed external `SchemaIr` for untrusted or isolated inputs. Temporary
> restoration of known standard-library sentinels handles synchronous callbacks,
> but is not a transaction: cyclic or deferred finalizers can run later and retain
> their ordinary Python side effects.

Python 3.14 deferred annotations are requested through public
`annotationlib.Format.STRING` introspection. This preserves callable shape and
typed lowering but does not guarantee side-effect-free metadata access: Python
may execute annotation expressions while producing strings. The generator does
not evaluate captured strings. See Python's
[annotationlib security guidance](https://docs.python.org/3.14/library/annotationlib.html#security-implications-of-introspecting-annotations).

Live APIs support one main CPython interpreter identity for the lifetime of a process. Do not
finalize or reinitialize that interpreter, switch interpreter identities, or use subinterpreters
with live generation. Use a fresh process when interpreter-lifecycle isolation is required.
Multiple callers and threads are supported only while attached to that same interpreter identity.
Process-static `Py<T>` caches are not keyed by interpreter identity.
The engine's process-wide coordination protects known engine-managed globals such as output streams
and restored standard-library sentinels; it is not a transaction or sandbox for arbitrary Python
imports, descriptors, callbacks, finalizers, or other Python effects.

### <a href="#-option-1-build-script"><img src="https://rustacean.net/assets/rustacean-flat-noshadow.svg" width="16" height="16"></a> Option 1: Build script

First, add `pyo3_bindgen` as a **build dependency** to your [`Cargo.toml`](https://doc.rust-lang.org/cargo/reference/manifest.html) manifest. To compile the generated bindings, you must also add `pyo3` as a regular dependency because generated paths refer to the `pyo3` crate directly.

```toml
[build-dependencies]
pyo3_bindgen = { version = "0.6" }

[dependencies]
pyo3 = { version = "0.29", features = ["auto-initialize"] }
```

Then, create a [`build.rs`](https://doc.rust-lang.org/cargo/reference/build-scripts.html) script in the root of your crate that generates bindings to the selected Python modules. In this example, the bindings are simultaneously generated for the "os", "posixpath", and "sys" Python modules. At the end of the generation process, the Rust bindings are written to `${OUT_DIR}/bindings.rs`.

> [!TIP]
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

Afterwards, you can include the generated Rust code via the `include!` macro anywhere in your crate and use the generated bindings as regular Rust modules. The bindings take a `pyo3::Python<'_>` token and can be called from a `pyo3::Python::attach` closure.

```rs
//! src/main.rs
include!(concat!(env!("OUT_DIR"), "/bindings.rs"));

fn main() -> pyo3::PyResult<()> {
    pyo3::Python::attach(|py| {
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

As an alternative to build scripts, you can use procedural macros to generate the bindings in-place. First, add `pyo3_bindgen` as a **regular dependency** to your [`Cargo.toml`](https://doc.rust-lang.org/cargo/reference/manifest.html) manifest and enable the `macros` feature. Add `pyo3` directly because the generated paths refer to that crate.

```toml
[dependencies]
pyo3 = { version = "0.29", features = ["auto-initialize"] }
pyo3_bindgen = { version = "0.6", features = ["macros"] }
```

Subsequently, the `import_python!` macro can be used to generate Rust bindings for the selected Python modules anywhere in your crate. As demonstrated in the example below, Rust bindings are generated for the "math" Python module and can directly be used in the same scope. Similar to the previous approach, call the generated bindings while attached to Python via `pyo3::Python::attach`.

> [!NOTE]
> As opposed to using build scripts, this approach does not offer the same level of customization via `pyo3_bindgen::Config`. Furthermore, the procedural macro is quite experimental and might not work in all cases.

> [!WARNING]
> `import_python!` imports and introspects Python during compile time. Module
> initialization, descriptors, package hooks, and annotations can execute arbitrary
> code with the compiler process's privileges. Use this macro only with trusted
> Python environments; use a reviewed external `SchemaIr` when compile-time Python
> execution is not acceptable.

```rs
use pyo3_bindgen::import_python;
import_python!("math");

// Which Pi do you prefer?
// a) 🐍 Pi from Python "math" module
// b) 🦀 Pi from Rust standard library
// c) 🥧 Pi from your favourite bakery
pyo3::Python::attach(|py| {
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

Install the CLI with mapping features when those generated types are desired:

```bash
cargo install --locked pyo3_bindgen_cli --features numpy,num-complex
```

Afterwards, run the `pyo3_bindgen` executable to generate Rust bindings for the selected Python modules. The generated bindings are printed to STDOUT by default, but they can also be written to a file via the `-o` option (see `pyo3_bindgen --help` for more options).

```bash
pyo3_bindgen -m os sys numpy -o bindings.rs
```

Recursive live submodule discovery is best-effort by default. When `--report` is used, each
attempted origin and final emitted/skipped/failed/truncated outcome is path-qualified and the report
includes deterministic totals. Use `--strict-completeness` to fail before replacing output whenever
optional live discovery is incomplete. Library callers select the same policy with
`Config::strict_completeness`; it does not apply to `--ir`.

The CLI process contract uses stable exit classes: `0` means complete success, `1` means failure
without a visible or uncertain primary publication, `2` means command-line usage rejection, and
`3` means output/report publication was visible, uncertain, or not proven durable. Machine-readable
failures are written as `ERROR[stable_code]: ...` on stderr; human-readable prose is not part of the
contract. When a distinct report target is independently safe, its version-1 diagnostics also use
stable report-level codes; those finer diagnostic categories are not required to equal the stderr
operation code.

Reusable command integrations can use the facade's `publication::{Request, Session, Outcome}`
API. It owns command delivery state while the engine retains path identity, locks, atomic writes,
and CLI durability policy. Callers supply generation, diagnostics, and any standard-output writer.
Each session permits one primary output/check attempt; report recovery remains repeatable.
Finalization rejects unfinished requested deliveries. Check errors retain typed operation codes;
report publication and rejection adopt caller-provided operation identifiers.
This does not change the direct artifact writers or the `Codegen::build` compatibility policy.

> [!TIP]
> Instead of introspecting a live Python module, the CLI can generate bindings from a pre-captured external schema (a JSON `SchemaIr`) via `pyo3_bindgen --ir schema.json -o bindings.rs`. This decouples generation from a Python runtime, which is useful for reproducible, offline, or cross-environment builds where the target module is not importable at generation time.

The built-in external-IR path does not initialize, attach to, or execute a Python interpreter. The
generator itself still builds on PyO3 and may retain the platform's normal `libpython` link/load
requirement. Custom `BindingSource` adapters acquire and materialize their input outside this
execution-free guarantee; their transport, parser, trust, and side effects remain the adapter's
responsibility. Release packaging uses a shared, linkable CPython because static-only interpreters
can fail while linking the procedural macro before generation begins.

The CLI emits structured lifecycle, source, generation, lock, report, and artifact
events through `tracing`. Engine warnings remain visible by default. Set `RUST_LOG`
to inspect phase timings and fields without mixing telemetry into generated STDOUT:

```bash
RUST_LOG=pyo3_bindgen_cli=info,pyo3_bindgen_engine=debug \
  pyo3_bindgen --ir schema.json -o bindings.rs
```

Library consumers receive the same engine events but retain control of subscriber
installation and formatting.

## Status

This project is a mature pre-1.0 generator. The API of the generated bindings is not yet stable.

- Generation runs during builds; generated-code compilation can dominate build time.
  Benchmark sources live in [the engine](crates/pyo3_bindgen_engine/benches/).
- The generation of bindings should never panic as long as the target Python module can be successfully imported. If it does, please [report](https://github.com/AndrejOrsula/pyo3_bindgen/issues/new) this as a bug.
- The generated bindings should always be compilable and usable in Rust. If you encounter any issues, consider manually fixing the problematic parts of the bindings and please [report](https://github.com/AndrejOrsula/pyo3_bindgen/issues/new) this as a bug.
- However, the generated bindings are based on the introspection of the target Python module. Therefore, the completeness and correctness of the generated bindings are directly dependent on the quality of the module structure, type annotations and docstrings in the target Python module. Ideally, the generated bindings should be considered unsafe and serve as a starting point for safe and idiomatic Rust APIs. If you find that something in the generated bindings is incorrect or missing, please [report](https://github.com/AndrejOrsula/pyo3_bindgen/issues/new) this as well.
- Not all Python types are mapped to their Rust equivalents yet. For this reason, some additional type-casting might be required when using the generated bindings (e.g. `let typed_value: MyType = any_value.extract()?;`).
- Although implemented, the procedural macro might not work in many cases. Therefore, it is recommended that the build scripts be used wherever possible.

## License

This project is dual-licensed to be compatible with the Rust project, under either the [MIT](https://github.com/AndrejOrsula/pyo3_bindgen/blob/v0.6.0/LICENSE-MIT) or [Apache 2.0](https://github.com/AndrejOrsula/pyo3_bindgen/blob/v0.6.0/LICENSE-APACHE) licenses.

## Contributing

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
