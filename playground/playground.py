"""The playground's Python side, run by Pyodide inside the page's web worker.

`worker.js` calls these functions and passes their results to the page. The
bindings come from `pyo3_bindgen_playground`, the real generator compiled for
Pyodide; this file only installs packages and packs the starter crate.
"""

import annotationlib
import enum
import importlib
import importlib.metadata
import importlib.util
import inspect
import io
import json
import keyword
import re
import sys
import zipfile

import micropip
import pyo3_bindgen_playground as engine

# Python module names that also work as Rust paths in the starter crate.
MODULE_NAME = re.compile(r"[A-Za-z_][A-Za-z0-9_]*(\.[A-Za-z_][A-Za-z0-9_]*)*")
RUST_KEYWORDS = frozenset(
    "as async await break const continue crate dyn else enum extern false fn for gen if impl in "
    "let loop match mod move mut pub ref return self Self static struct super trait true type "
    "unsafe use where while abstract become box do final macro override priv try typeof unsized "
    "virtual yield _".split()
)
REQUIREMENT = re.compile(
    r"[A-Za-z0-9][A-Za-z0-9._-]*(\[[A-Za-z0-9._, -]+\])?(\s*[=<>!~]=?\s*[A-Za-z0-9.*+!_-]+)?"
)
# Modules created from pasted code, which a new paste may replace.
PASTED = set()
REPOSITORY = "https://github.com/AndrejOrsula/pyo3_bindgen"
# The public commit whose generator this page runs, when its sources were
# unmodified; the starter crate's README can then pin it.
GENERATOR_REVISION = ""


def configure(generator_revision):
    """Records the build's details that the starter crate refers to."""
    global GENERATOR_REVISION
    GENERATOR_REVISION = (
        generator_revision if re.fullmatch(r"[0-9a-f]{40}", generator_revision) else ""
    )


SKIPPED_TOP_LEVEL = {"tests", "test", "testing", "docs", "examples", "benchmarks"}


def check_module_name(name):
    """Rejects names that cannot be both a Python module and a Rust path."""
    if not MODULE_NAME.fullmatch(name):
        raise ValueError(f"{name!r} is not a Python module name")
    for part in name.split("."):
        if keyword.iskeyword(part) or part in RUST_KEYWORDS:
            raise ValueError(
                f"{name!r} uses the keyword {part!r}, which Rust cannot name"
            )


def check_options(options):
    """Rejects skipped names that are not Python names. A shared link sets them,
    and the starter crate's README and `build.rs` quote them as they are."""
    for name in options.get("blocklist_names") or []:
        if isinstance(name, str) and not name.isidentifier():
            raise ValueError(f"{name!r} is not a Python name, so no attribute has it")


def shadows_module(name):
    """Whether `name` is a module that Python could import other than pasted code."""
    if name in PASTED:
        return False
    if name in sys.modules or name in sys.stdlib_module_names:
        return True
    try:
        return importlib.util.find_spec(name) is not None
    except (ImportError, ValueError):
        return False


async def install(requirement):
    """Installs a PyPI requirement and returns its importable top-level modules."""
    requirement = requirement.strip()
    if not REQUIREMENT.fullmatch(requirement):
        raise ValueError(
            f"{requirement!r} is not a package name, such as attrs or attrs==25.3.0"
        )
    await micropip.install(requirement)
    name = re.match(r"[A-Za-z0-9._-]+", requirement).group(0)
    distribution = importlib.metadata.distribution(name)
    return {
        "name": distribution.metadata["Name"],
        "version": distribution.version,
        "modules": top_level_modules(distribution),
    }


def top_level_modules(distribution):
    """The importable top-level modules of an installed distribution."""
    declared = distribution.read_text("top_level.txt")
    if declared:
        names = set(declared.split())
    else:
        names = set()
        for path in distribution.files or ():
            parts = path.parts
            if len(parts) > 1 and parts[1] == "__init__.py":
                names.add(parts[0])
            elif len(parts) == 1 and parts[0].endswith(".py"):
                names.add(parts[0][:-3])
    usable = []
    for name in sorted(names):
        if name.startswith("_") or name in SKIPPED_TOP_LEVEL:
            continue
        try:
            check_module_name(name)
        except ValueError:
            continue
        usable.append(name)
    return usable


def default_options():
    """The generator's defaults for the options that the page offers."""
    return dict(engine.default_options())


def generate_modules(modules, options):
    """Bindings for importable modules, with the starter crate's files."""
    modules = [name.strip() for name in modules if name.strip()]
    if not modules:
        raise ValueError("Name at least one module, such as math")
    for name in modules:
        check_module_name(name)
    options = dict(options)
    check_options(options)
    code, report, analysis = engine.generate_modules(modules, options)
    return result(code, report, analysis, modules, None, options)


def generate_source(source, module, options):
    """Bindings for a module given by its source code, with the starter crate's files."""
    module = module.strip()
    check_module_name(module)
    if "." in module:
        raise ValueError("Name pasted code as a top-level module, such as orbit")
    # Code that took a module's name would replace that module for every later
    # import in this tab, including the standard library's.
    if shadows_module(module):
        raise ValueError(
            f"{module} is already a Python module here; name the pasted code differently"
        )
    options = dict(options)
    check_options(options)
    # Executing the code again replaces the previous version of the module.
    sys.modules.pop(module, None)
    PASTED.add(module)
    code, report, analysis = engine.generate_source(source, module, options)
    return result(code, report, analysis, [module], source, options)


def result(code, report, analysis, modules, source, options):
    """The page's view of a generation; the analysis stays JSON text for the page to parse."""
    report = json.loads(report)
    return {
        "code": code,
        "report": report,
        "analysis": analysis,
        "python_api": python_api(module["name"] for module in report["modules"]),
        "files": crate_files(code, report, modules, source, options),
    }


# The page's overview shows at most this many public Python items, and
# signatures of at most this length.
PYTHON_API_LIMIT = 5000
SIGNATURE_LIMIT = 400


def signature_of(target):
    """The signature of a callable; annotations that name what Python cannot find
    stay as written."""
    try:
        return inspect.signature(target)
    except (TypeError, ValueError):
        return None
    except Exception:
        try:
            return inspect.signature(
                target, annotation_format=annotationlib.Format.STRING
            )
        except Exception:
            return None


def python_signature(target, module_name):
    """The signature of a callable as Python prints it, without the module's own prefix."""
    signature = signature_of(target)
    if signature is None:
        return None
    text = signature.format(quote_annotation_strings=False)
    text = without_addresses(text.replace(f"{module_name}.", ""))
    return text if len(text) <= SIGNATURE_LIMIT else text[: SIGNATURE_LIMIT - 1] + "…"


def without_addresses(text):
    """A repr without the memory addresses that some print, which change every run."""
    return re.sub(r" at 0x[0-9a-fA-F]+>", ">", text)


def first_line(target):
    doc = inspect.getdoc(target)
    return doc.strip().splitlines()[0] if doc and doc.strip() else None


def python_api(module_names):
    """What the page shows beside each binding: kind, signature, and summary of the
    module-level and class-level Python items of the generated modules."""
    api = {}
    for module_name in module_names:
        module = sys.modules.get(module_name)
        if module is None:
            try:
                module = importlib.import_module(module_name)
            except Exception:
                continue
        api[module_name] = {"kind": "module", "doc": first_line(module)}
        annotations = class_annotations(module)
        for name, value in list(vars(module).items()):
            if name.startswith("_") or inspect.ismodule(value):
                continue
            path = f"{module_name}.{name}"
            try:
                entries = python_item(path, value, module_name, annotations.get(name))
            except Exception:
                # The overview is a convenience: an item that Python cannot
                # describe is left out rather than failing the generation.
                continue
            for key, entry in entries.items():
                if len(api) >= PYTHON_API_LIMIT:
                    return api
                api[key] = entry
    return api


def python_item(path, value, module_name, annotation):
    """The entries of a module-level item and, for a class, of its members."""
    if not inspect.isclass(value):
        if callable(value):
            return {
                path: {
                    "kind": "function",
                    "signature": python_signature(value, module_name),
                    "doc": first_line(value),
                }
            }
        return {
            path: {
                "kind": "value",
                "type": annotation_text(annotation, module_name)
                or type(value).__name__,
            }
        }
    entries = {path: python_class(value, module_name)}
    for member, item in list(vars(value).items()):
        entry = None if member.startswith("_") else python_member(item, module_name)
        if entry:
            entries[f"{path}.{member}"] = entry
    # Annotated class attributes, such as dataclass fields.
    for member, member_annotation in class_annotations(value).items():
        if not member.startswith("_"):
            entries.setdefault(
                f"{path}.{member}",
                {
                    "kind": "attribute",
                    "type": annotation_text(member_annotation, module_name),
                },
            )
    return entries


def annotation_text(annotation, module_name):
    if annotation is None or annotation is inspect.Parameter.empty:
        return None
    text = (
        annotation
        if isinstance(annotation, str)
        else inspect.formatannotation(annotation)
    )
    return text.replace(f"{module_name}.", "")


def python_class(cls, module_name):
    bases = [base.__name__ for base in cls.__bases__ if base is not object]
    entry = {
        "kind": "enum" if issubclass(cls, enum.Enum) else "class",
        "bases": bases,
        "doc": first_line(cls),
    }
    if issubclass(cls, enum.Enum):
        # `__members__` also holds aliases, which the bindings include.
        entry["members"] = [
            without_addresses(f"{name} = {member.value!r}")[:80]
            for name, member in cls.__members__.items()
        ]
    else:
        signature = python_signature(cls, module_name)
        entry["signature"] = signature.removesuffix(" -> None") if signature else None
    return entry


def class_annotations(cls):
    """The annotations of a class or module, as written if Python cannot evaluate them."""
    for annotation_format in (
        annotationlib.Format.VALUE,
        annotationlib.Format.STRING,
    ):
        try:
            return annotationlib.get_annotations(cls, format=annotation_format)
        except Exception:
            continue
    return {}


def python_member(item, module_name):
    if isinstance(item, property):
        signature = signature_of(item.fget) if item.fget else None
        returns = signature.return_annotation if signature else None
        return {
            "kind": "property",
            "type": annotation_text(returns, module_name),
            "doc": first_line(item),
        }
    if isinstance(item, (staticmethod, classmethod)):
        function = item.__func__
        kind = "staticmethod" if isinstance(item, staticmethod) else "classmethod"
        return {
            "kind": kind,
            "signature": python_signature(function, module_name),
            "doc": first_line(function),
        }
    if inspect.isfunction(item) or inspect.ismethoddescriptor(item):
        return {
            "kind": "method",
            "signature": python_signature(item, module_name),
            "doc": first_line(item),
        }
    return None


def crate_name(modules):
    root = (
        re.sub(r"[^a-z0-9_]", "_", modules[0].split(".")[0].lower()).strip("_")
        or "python"
    )
    return f"{root}_bindings"


def example_call(code, module):
    """A commented call to the first top-level function of `module`, if it has one."""
    path = module.split(".")
    indent = " " * (4 * len(path))
    pattern = re.compile(
        rf"^{indent}pub fn (?!pyo3_embed_python_source_code\b)(\w+)(?:<[^>]*>)?\(\s*py: [^,)]+,?(.*?)\)\s*->",
        re.MULTILINE | re.DOTALL,
    )
    start = code.find(f"pub mod {path[-1]} {{")
    match = pattern.search(code, start if start >= 0 else 0)
    if match is None:
        return None
    arguments = [
        part.split(":")[0].strip()
        for part in re.split(r",\s*(?![^<]*>)", match.group(2))
    ]
    # Generated parameters carry a `p_` prefix; the hint names the Python ones.
    arguments = [
        name.removeprefix("p_") for name in arguments if re.fullmatch(r"(r#)?\w+", name)
    ]
    call = ", ".join(["py", *arguments])
    return f"{'::'.join(path)}::{match.group(1)}({call})?"


# Options of the page that the command-line tool spells as flags; the others
# need a build script. The analysis only changes what the page shows.
CLI_FLAGS = {
    ("include_private", True): "--include-private",
    ("traverse_submodules", False): "--no-traverse-submodules",
    ("deduplicate_inherited_members", False): "--no-deduplicate-inherited-members",
    ("generate_inherent_forwarders", False): "--no-inherent-forwarders",
    ("strict_completeness", True): "--strict-completeness",
    ("call_builders", True): "--call-builders",
}
PAGE_ONLY_OPTIONS = {"analysis", "max_output_bytes"}


def changed_options(options):
    """The options that differ from the generator's defaults, in a stable order."""
    defaults = default_options()
    return {
        key: options[key]
        for key in sorted(options)
        if key not in PAGE_ONLY_OPTIONS
        and key in defaults
        and options[key] != defaults[key]
    }


def cli_command(modules, changed):
    """The `pyo3_bindgen` command for these options, or None if a flag is missing."""
    flags = []
    for key, value in changed.items():
        if key == "blocklist_names":
            if not value:
                return None
            flags += [f"--blocklist-name {name}" for name in value]
        elif (key, value) in CLI_FLAGS:
            flags.append(CLI_FLAGS[key, value])
        else:
            return None
    return " ".join(["pyo3_bindgen", "-m", *modules, *flags, "-o", "src/bindings.rs"])


def rust_string(text):
    return json.dumps(text, ensure_ascii=False)


def build_script(modules, source, changed):
    """A `build.rs` that generates the same bindings at build time."""
    first = modules[0]
    config_changes = {
        key: value for key, value in changed.items() if key != "call_builders"
    }
    setters = []
    for key, value in config_changes.items():
        if key == "blocklist_names":
            names = ", ".join(f"{rust_string(name)}.to_owned()" for name in value)
            setters.append(f"        .blocklist_names(vec![{names}])")
        else:
            setters.append(f"        .{key}({str(value).lower()})")
    if source is not None:
        watch = f'    println!("cargo:rerun-if-changed={first}.py");'
        add_source = f'        .module_from_str(&std::fs::read_to_string("{first}.py")?, "{first}")?'
    else:
        watch = None
        names = ", ".join(rust_string(name) for name in modules)
        add_source = f"        .module_names([{names}])?"
    builders = changed.get("call_builders", False)
    lines = [
        "//! build.rs",
        "use pyo3_bindgen::{Codegen, Config, GenerationOptions};"
        if builders
        else "use pyo3_bindgen::{Codegen, Config};",
        "use std::path::PathBuf;",
        "",
        "fn main() -> Result<(), Box<dyn std::error::Error>> {",
    ]
    if watch:
        lines.append(watch)
    lines += [
        '    for name in ["PYO3_PYTHON", "PYTHONPATH", "PYTHONHOME", "VIRTUAL_ENV"] {',
        '        println!("cargo:rerun-if-env-changed={name}");',
        "    }",
        '    let output = PathBuf::from(std::env::var_os("OUT_DIR").ok_or("OUT_DIR is missing")?)',
        '        .join("bindings.rs");',
        "    let config = Config::builder()",
        *setters,
        "        .build();",
        "    Codegen::new(config)",
    ]
    if builders:
        lines.append(
            "        .with_generation_options(GenerationOptions::new().with_call_builders(true))"
        )
    lines += [add_source, "        .build_if_changed(output)?;", "    Ok(())", "}"]
    return "\n".join(lines)


def crate_files(code, report, modules, source, options):
    """The files of a starter crate around the generated bindings."""
    name = crate_name(modules)
    generator = report["generator"]
    python = report.get("python") or {}
    # The interpreter reports its version followed by build details.
    python_version = (python.get("version") or "").split(" ")[0]
    pyo3_version = generator.get("pyo3_version") or "0.29.2"
    first = modules[0]
    changed = changed_options(options)

    lines = [
        f"//! Calls the Python module `{first}` from Rust through the bindings in",
        "//! `bindings.rs`.",
        "",
        'include!("bindings.rs");',
        "",
        "fn main() -> pyo3::PyResult<()> {",
        "    pyo3::Python::attach(|py| {",
    ]
    if source is not None:
        lines += [
            "        // Load the embedded Python source before calling into it.",
            f"        {first}::pyo3_embed_python_source_code(py)?;",
        ]
    lines += [
        f'        let module = py.import("{first}")?;',
        '        println!("Python module ready: {module}");',
    ]
    call = example_call(code, first)
    if call:
        lines += [
            "        // Uncomment to call a binding:",
            f"        // let value = {call};",
        ]
    lines += ["        Ok(())", "    })", "}", ""]
    main_rs = "\n".join(lines)

    cargo_toml = "\n".join(
        [
            "[package]",
            f'name = "{name}"',
            'version = "0.1.0"',
            'edition = "2024"',
            "publish = false",
            "",
            "[dependencies]",
            f'pyo3 = {{ version = "={pyo3_version}", features = ["auto-initialize"] }}',
            "",
        ]
    )

    # A module's distribution can have another name, such as `attr` from `attrs`.
    # Pyodide installs parts of the standard library, such as `sqlite3`, as
    # packages; those need no installation elsewhere.
    roots = sorted(
        {module.split(".")[0] for module in modules} - sys.stdlib_module_names
    )
    providers = importlib.metadata.packages_distributions()
    distributions = sorted(
        {name for root in roots for name in providers.get(root, ())}, key=str.lower
    )
    installed = [
        f"{name}=={importlib.metadata.version(name)}" for name in distributions
    ]
    elsewhere = [root for root in roots if root not in providers]
    if source is not None:
        setup = (
            f"The bindings embed the source of `{first}` (also in `{first}.py`), so "
            "`main.rs` loads it into the interpreter first and no installation is needed."
        )
    elif installed or elsewhere:
        steps = []
        if installed:
            steps.append(
                "Install the package for the Python interpreter that PyO3 uses:\n\n"
                f"```sh\npython -m pip install {' '.join(installed)}\n```"
            )
        if elsewhere:
            steps.append(
                "Make "
                + ", ".join(f"`{root}`" for root in elsewhere)
                + " importable by the Python interpreter that PyO3 uses, for example "
                "through `PYTHONPATH`."
            )
        setup = "\n\n".join(steps)
    else:
        setup = "The module is part of the Python standard library, so no installation is needed."
    usage = (
        "`src/main.rs` shows how to call a binding, and"
        if call
        else "`src/main.rs` imports the module; "
    )
    readme = [
        f"# {name}",
        "",
        f"Rust bindings to the Python module{'s' if len(modules) > 1 else ''} "
        + ", ".join(f"`{m}`" for m in modules)
        + f", generated by [pyo3_bindgen](https://github.com/AndrejOrsula/pyo3_bindgen) "
        f"{generator['version']} in its browser playground.",
        "",
        "## Run",
        "",
        setup,
        "",
        "Then build and run the crate. PyO3 links to the Python found on `PATH`; set "
        "`PYO3_PYTHON` to choose another interpreter.",
        "",
        "```sh",
        "cargo run",
        "```",
        "",
        "It imports the module through the bindings and prints it, which confirms the "
        f"setup. {usage} `cargo doc --open` lists every generated function and type.",
        "",
        "## Regenerate",
        "",
    ]
    if changed:
        readme += [
            "The playground generated these bindings with these options changed from "
            "their defaults: "
            + ", ".join(
                f"`{key} = {json.dumps(value)}`" for key, value in changed.items()
            )
            + ".",
            "",
        ]
    version = generator["version"]
    command = cli_command(modules, changed) if source is None else None
    if command:
        readme += [
            "With the command-line tool at the same version:",
            "",
            "```sh",
            f"cargo install --locked pyo3_bindgen_cli@{version}",
            command,
            "```",
            "",
            "Or generate them at build time instead: add",
        ]
    else:
        readme += ["To generate them at build time instead, add"]
    readme[-1] += (
        f' `pyo3_bindgen = "={version}"` under `[build-dependencies]` in '
        "`Cargo.toml`, save this as `build.rs`, and include "
        '`concat!(env!("OUT_DIR"), "/bindings.rs")` in `src/main.rs` instead of `bindings.rs`:'
    )
    readme += ["", "```rust", build_script(modules, source, changed), "```", ""]
    if GENERATOR_REVISION:
        pinned = f'{{ git = "{REPOSITORY}", rev = "{GENERATOR_REVISION}" }}'
        readme += [
            f"If version {version} is not on crates.io yet, use the exact commit that this "
            f"playground runs instead: `pyo3_bindgen = {pinned}`"
            + (
                f", and `cargo install --locked --git {REPOSITORY} --rev {GENERATOR_REVISION} "
                "pyo3_bindgen_cli` for the command-line tool."
                if command
                else "."
            ),
            "",
        ]
    else:
        readme += [
            f"Use these regeneration steps when version {version} is available on "
            "crates.io. If it is not available yet, regenerate in the browser and "
            "download a fresh starter. The included `src/bindings.rs` builds without "
            "installing the generator.",
            "",
        ]
    readme += [
        "## Notes",
        "",
        f"- The playground ran {python.get('implementation', 'CPython')} {python_version} "
        "in the browser (Pyodide). A module that differs by Python version or platform may "
        "have a different API on your machine.",
        "",
    ]

    # The page shows the files in this order.
    files = {
        "src/bindings.rs": code,
        "src/main.rs": main_rs,
        "Cargo.toml": cargo_toml,
        "README.md": "\n".join(readme),
    }
    if source is not None:
        files[f"{first}.py"] = source if source.endswith("\n") else source + "\n"
    files[".gitignore"] = "/target\n"
    return files


def crate_zip(files, name):
    """Packs the starter crate into a zip archive, under one top-level folder."""
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w", zipfile.ZIP_DEFLATED) as archive:
        for path in sorted(files):
            info = zipfile.ZipInfo(f"{name}/{path}", date_time=(1980, 1, 1, 0, 0, 0))
            info.compress_type = zipfile.ZIP_DEFLATED
            info.external_attr = 0o644 << 16
            archive.writestr(info, files[path])
    return buffer.getvalue()
