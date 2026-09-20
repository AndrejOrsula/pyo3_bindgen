#[path = "../test_support/bounded_consumer.rs"]
mod bounded_consumer;

#[test]
fn module_level_bound_method_generates_function_binding() {
    let generated = pyo3_bindgen_engine::Codegen::default()
        .module_from_str(
            indoc::indoc! { r#"
                class Counter:
                    def increment(self, value: int) -> int:
                        return value + 1

                counter = Counter()
                bound_increment = counter.increment
            "# },
            "mod_bindgen_bound_method",
        )
        .unwrap()
        .generate_with_report()
        .unwrap();

    let generated_code =
        prettyplease::unparse(&syn::parse_str(&generated.tokens.to_string()).unwrap());
    assert!(
        generated_code.contains("pub fn bound_increment"),
        "generated bindings did not expose the bound method:\n{generated_code}"
    );
}

#[test]
fn malformed_live_function_annotations_warn_and_fall_back_to_unknown() {
    let generated = pyo3_bindgen_engine::Codegen::default()
        .module_from_str(
            indoc::indoc! { r#"
                def malformed_parameter(value):
                    return value

                malformed_parameter.__annotations__["value"] = "list[int"

                def malformed_return(value: int):
                    return value

                malformed_return.__annotations__["return"] = "dict[str"
                if hasattr(malformed_return, "__annotate__"):
                    # Python 3.14's STRING-format annotation retrieval calls this
                    # producer instead of consulting the cached mutable dictionary.
                    malformed_return.__annotate__ = lambda format: {
                        "value": "int",
                        "return": "dict[str",
                    }
            "# },
            "mod_malformed_live_function_annotations",
        )
        .expect("malformed runtime annotations must not abort live introspection")
        .generate_with_report()
        .expect("malformed runtime annotations must degrade to untyped bindings");

    let source = prettyplease::unparse(
        &syn::parse2(generated.tokens).expect("generated bindings must remain valid Rust"),
    );
    assert!(
        source.contains("pub fn malformed_parameter") && source.contains("pub fn malformed_return"),
        "functions with malformed annotations must remain in the bindings:\n{source}",
    );
    let fallbacks = generated
        .report
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code == "live_annotation_parse_fallback")
        .count();
    assert_eq!(
        fallbacks, 2,
        "both malformed annotations must produce an observable fallback: {:?}",
        generated.report.diagnostics,
    );
}

#[test]
fn stdlib_random_bound_methods_generate_function_bindings() {
    let generated = pyo3_bindgen_engine::Codegen::default()
        .module_name("random")
        .unwrap()
        .generate()
        .unwrap();
    let generated_code = prettyplease::unparse(&syn::parse_str(&generated.to_string()).unwrap());

    for function_name in ["choice", "seed", "shuffle"] {
        assert!(
            generated_code.contains(&format!("pub fn {function_name}")),
            "generated random bindings did not expose `{function_name}`"
        );
    }
}

#[test]
fn external_ir_module_method_is_rejected_at_terminal_validation() {
    use pyo3_bindgen_engine::{
        FunctionIr, FunctionKindIr, MethodKindIr, ModuleIr, ParameterIr, ParameterKindIr, SchemaIr,
    };

    let error = pyo3_bindgen_engine::Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![ModuleIr {
                name: "unsupported".to_owned(),
                docstring: None,
                is_package: false,
                classes: vec![],
                enums: vec![],
                functions: vec![FunctionIr {
                    name: "unsupported.bound".to_owned(),
                    kind: FunctionKindIr::Method {
                        method_type: MethodKindIr::Instance,
                    },
                    params: vec![ParameterIr {
                        name: "self".to_owned(),
                        kind: ParameterKindIr::PositionalOrKeyword,
                        annotation: String::new(),
                        has_default: false,
                    }],
                    return_type: String::new(),
                    docstring: None,
                }],
                properties: vec![],
                submodules: vec![],
                prelude: vec![],
                imports: vec![],
                type_vars: vec![],
            }],
        })
        .expect("registration defers whole-graph semantic validation")
        .generate()
        .expect_err("module functions cannot carry method semantics");

    assert!(
        error.to_string().contains(
            "external IR module function 'unsupported.bound' cannot use FunctionKindIr::Method"
        ),
        "unexpected validation error: {error}"
    );
}

#[test]
fn generated_module_bound_methods_compile_and_run() {
    let generated = pyo3_bindgen_engine::Codegen::default()
        .module_from_str(
            indoc::indoc! { r#"
                class Counter:
                    def increment(self, value: int) -> int:
                        return value + 1

                counter = Counter()
                bound_increment = counter.increment
            "# },
            "mod_bindgen_bound_method_runtime",
        )
        .unwrap()
        .module_name("random")
        .unwrap()
        .generate()
        .unwrap();
    let generated_code = prettyplease::unparse(&syn::parse2(generated).unwrap());

    let mut working_directory =
        bounded_consumer::OwnedTempDir::allocate("module-methods-bound-method-runtime")
            .expect("runtime consumer directory must be allocated exclusively");
    let crate_dir = working_directory.path().to_owned();
    std::fs::create_dir(crate_dir.join("src")).unwrap();

    let pyo3_version = env!("PYO3_BINDGEN_PYO3_VERSION");
    std::fs::write(
        crate_dir.join("Cargo.toml"),
        format!(
            indoc::indoc! { r#"
            [package]
            name = "pyo3_bindgen_module_methods_runtime"
            version = "0.0.0"
            edition = "2021"
            publish = false

            [dependencies]
            pyo3 = {{ version = "={pyo3_version}", features = ["auto-initialize"] }}
        "# },
            pyo3_version = pyo3_version
        ),
    )
    .unwrap();
    std::fs::write(
        crate_dir.join("src/lib.rs"),
        format!(
            r#"{generated_code}

#[cfg(test)]
mod tests {{
    use pyo3::types::PyAnyMethods;

    #[test]
    fn bound_methods_run() -> pyo3::PyResult<()> {{
        pyo3::Python::attach(|py| {{
            crate::mod_bindgen_bound_method_runtime::pyo3_embed_python_source_code(py)?;
            assert_eq!(
                crate::mod_bindgen_bound_method_runtime::bound_increment(py, 41)?,
                42,
            );

            let values = pyo3::types::PyList::new(py, [1_i64, 2, 3])?;
            assert!(crate::random::seed(py, 1234_i64, 2_i64)?.is_none());
            let choice = crate::random::choice(py, &values)?.extract::<i64>()?;
            assert!([1_i64, 2, 3].contains(&choice));
            Ok(())
        }})
    }}
}}
"#
        ),
    )
    .unwrap();

    let mut command =
        std::process::Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned()));
    command
        .args(["test", "--quiet", "--offline"])
        .current_dir(&crate_dir)
        .env("CARGO_TARGET_DIR", crate_dir.join("target"));
    let output = working_directory
        .run(&mut command, std::time::Duration::from_secs(900))
        .unwrap_or_else(|error| {
            panic!(
                "generated module-method runtime smoke command failed: {error}\nstdout:\n{}\nstderr:\n{}\ngenerated:\n{}",
                error.stdout(),
                error.stderr(),
                generated_code,
            )
        });

    assert!(
        output.status.success(),
        "generated module-method runtime smoke failed\nstatus: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        output.stdout,
        output.stderr
    );
}

#[test]
fn partial_callables_use_annotation_source_globals() {
    let generated = pyo3_bindgen_engine::Codegen::default()
        .module_from_str(
            indoc::indoc! { r#"
                from __future__ import annotations

                import functools

                class int:
                    pass

                def make_custom() -> int:
                    return int()

                partial_custom = functools.partial(make_custom)

                class PartialAnnotation:
                    def make_method_custom(self) -> int:
                        return int()

                    custom = functools.partialmethod(make_method_custom)
            "# },
            "mod_partial_annotation_globals",
        )
        .expect("partial callables with postponed annotations must be inspectable")
        .generate()
        .expect("partial callable annotations must generate");
    let generated_code = prettyplease::unparse(
        &syn::parse2(generated).expect("generated bindings must remain valid Rust"),
    );
    let partial_signature = generated_code
        .split("pub fn partial_custom<'py>(")
        .nth(1)
        .and_then(|rest| rest.split('{').next())
        .unwrap_or_else(|| panic!("missing generated partial function:\n{generated_code}"));
    assert!(
        partial_signature.contains("PyResult<int>"),
        "the partial return must resolve through the source function's shadowed `int`:\n\
         {partial_signature}",
    );

    let mut working_directory =
        bounded_consumer::OwnedTempDir::allocate("module-methods-partial-annotations-runtime")
            .expect("runtime consumer directory must be allocated exclusively");
    let crate_dir = working_directory.path().to_owned();
    std::fs::create_dir(crate_dir.join("src")).unwrap();

    let pyo3_version = env!("PYO3_BINDGEN_PYO3_VERSION");
    std::fs::write(
        crate_dir.join("Cargo.toml"),
        format!(
            indoc::indoc! { r#"
            [package]
            name = "pyo3_bindgen_partial_annotations_runtime"
            version = "0.0.0"
            edition = "2021"
            publish = false

            [dependencies]
            pyo3 = {{ version = "={pyo3_version}", features = ["auto-initialize"] }}
        "# },
            pyo3_version = pyo3_version
        ),
    )
    .unwrap();
    std::fs::write(
        crate_dir.join("src/lib.rs"),
        format!(
            r#"{generated_code}

#[cfg(test)]
mod tests {{
    use crate::mod_partial_annotation_globals::PartialAnnotationMethods;
    use pyo3::types::{{PyAnyMethods, PyTypeMethods}};

    #[test]
    fn partial_annotations_run() -> pyo3::PyResult<()> {{
        pyo3::Python::attach(|py| {{
            crate::mod_partial_annotation_globals::pyo3_embed_python_source_code(py)?;
            let module = py.import(pyo3::intern!(py, "mod_partial_annotation_globals"))?;

            let custom = crate::mod_partial_annotation_globals::partial_custom(py)?;
            assert_eq!(custom.as_any(py).get_type().name()?, "int");

            let instance =
                crate::mod_partial_annotation_globals::PartialAnnotation::from_py_any(
                    module
                        .getattr(pyo3::intern!(py, "PartialAnnotation"))?
                        .call0()?,
                )?;
            let method_custom = instance.custom(py)?;
            assert_eq!(method_custom.as_any(py).get_type().name()?, "int");
            Ok(())
        }})
    }}
}}
"#
        ),
    )
    .unwrap();

    let mut command =
        std::process::Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned()));
    command
        .args(["test", "--quiet", "--offline"])
        .current_dir(&crate_dir)
        .env("CARGO_TARGET_DIR", crate_dir.join("target"));
    let output = working_directory
        .run(&mut command, std::time::Duration::from_secs(900))
        .unwrap_or_else(|error| {
            panic!(
                "generated partial-annotation runtime smoke command failed: {error}\nstdout:\n{}\nstderr:\n{}\ngenerated:\n{}",
                error.stdout(),
                error.stderr(),
                generated_code,
            )
        });

    assert!(
        output.status.success(),
        "generated partial-annotation runtime smoke failed\nstatus: {}\nstdout:\n{}\nstderr:\n{}\n\
         generated:\n{generated_code}",
        output.status,
        output.stdout,
        output.stderr,
    );
}

#[test]
fn async_handles_and_signatureless_method_kinds_compile_and_run() {
    let codegen = pyo3_bindgen_engine::Codegen::default()
        .module_from_str(
            indoc::indoc! { r#"
                import asyncio
                import builtins
                import functools
                import inspect

                _real_classmethod = builtins.classmethod
                _real_staticmethod = builtins.staticmethod
                _real_partial = functools.partial
                _real_partialmethod = functools.partialmethod
                _inspect_names = (
                    "signature",
                    "getattr_static",
                    "isfunction",
                    "ismethod",
                    "iscoroutinefunction",
                    "isasyncgenfunction",
                    "isroutine",
                )
                _real_inspect = {
                    name: getattr(inspect, name) for name in _inspect_names
                }

                def _restore_inspect():
                    inspect.__dict__.update(_real_inspect)

                async def compute(value: int) -> int:
                    return value + 1

                async def wrapped_compute(value: int) -> int:
                    return value + 2

                @functools.wraps(wrapped_compute)
                def wrapped_compute_proxy(*args, **kwargs):
                    return wrapped_compute(*args, **kwargs)

                @functools.wraps(wrapped_compute)
                def explicit_signature_proxy(*args, **kwargs):
                    return wrapped_compute(*args, **kwargs)
                explicit_signature_proxy.__signature__ = inspect.signature(wrapped_compute)

                def wrapped_sync_base(value: int) -> int:
                    return value + 3

                @functools.wraps(wrapped_sync_base)
                async def wrapped_async_middle(*args, **kwargs):
                    return wrapped_sync_base(*args, **kwargs)

                @functools.wraps(wrapped_async_middle)
                def wrapped_async_outer(*args, **kwargs):
                    return wrapped_async_middle(*args, **kwargs)

                async def numbers(limit: int) -> int:
                    for value in range(limit):
                        yield value

                def collect_async_generator(generator):
                    async def collect():
                        return [value async for value in generator]
                    return asyncio.run(collect())

                def unrelated_partial_source(bound: int, value: int) -> int:
                    return bound + value

                class AsyncUnaryCallable:
                    async def __call__(self, value: int) -> int:
                        return value + 1

                _async_unary_callable = AsyncUnaryCallable()
                callable_partial = functools.partial(_async_unary_callable, 41)

                def callable_wrapped_proxy(value: int) -> int:
                    return _async_unary_callable(value)
                callable_wrapped_proxy.__wrapped__ = _async_unary_callable

                class Signatureless(dict):
                    async def async_instance(self, value: int) -> int:
                        return value + 1

                    @staticmethod
                    async def async_static(value: int) -> int:
                        return value + 2

                    @classmethod
                    async def async_class(cls, value: int) -> str:
                        return f"{cls.__name__}:{value}"

                    def instance_call(self, *args, **kwargs):
                        return args[0] + kwargs["bonus"]

                    @staticmethod
                    def static_call(*args, **kwargs):
                        return args[0] + kwargs["bonus"]

                    @classmethod
                    def class_call(cls, *args, **kwargs):
                        return cls.__name__ + args[0] + kwargs["suffix"]

                class PartialAsync:
                    async def base(self, value: int) -> int:
                        return value + 1

                    via_partial = functools.partialmethod(base, 41)

                class AsyncMethodCallable:
                    async def __call__(
                        self, owner, bound: int, value: int
                    ) -> str:
                        return f"{type(owner).__name__}:callable:{bound + value}"

                class PartialCallable:
                    via_callable = functools.partialmethod(
                        AsyncMethodCallable(), 40
                    )

                class PartialKinds:
                    arbitrary_partial = functools.partial(unrelated_partial_source, 40)

                    def sync_instance_base(self, bound: int, value: int) -> str:
                        return f"instance:{bound + value}"

                    sync_instance = functools.partialmethod(sync_instance_base, 40)

                    @staticmethod
                    def sync_static_base(bound: int, value: int) -> str:
                        return f"static:{bound + value}"

                    sync_static = functools.partialmethod(sync_static_base, 40)

                    @classmethod
                    def sync_class_base(cls, bound: int, value: int) -> str:
                        return f"{cls.__name__}:sync:{bound + value}"

                    sync_class = functools.partialmethod(sync_class_base, 40)

                    async def async_instance_base(self, bound: int, value: int) -> str:
                        return f"instance-async:{bound + value}"

                    async_instance = functools.partialmethod(async_instance_base, 40)

                    @staticmethod
                    async def async_static_base(bound: int, value: int) -> str:
                        return f"static-async:{bound + value}"

                    async_static = functools.partialmethod(async_static_base, 40)

                    @classmethod
                    async def async_class_base(cls, bound: int, value: int) -> str:
                        return f"{cls.__name__}:async:{bound + value}"

                    async_class = functools.partialmethod(async_class_base, 40)

                # Force inspect.signature failure while leaving descriptor kind intact.
                Signatureless.instance_call.__signature__ = "invalid"
                Signatureless.static_call.__signature__ = "invalid"
                Signatureless.class_call.__func__.__signature__ = "invalid"

                # Introspection must classify from actual binding semantics, not
                # these mutable builtin module attributes.
                builtins.classmethod = object
                builtins.staticmethod = object
                functools.partial = object
                functools.partialmethod = object
                for _inspect_name in _inspect_names:
                    setattr(inspect, _inspect_name, lambda *args, **kwargs: False)
            "# },
            "mod_async_signatureless_runtime",
        )
        .unwrap();
    let generated_result = codegen.generate_with_report();
    let generated = generated_result.unwrap();

    assert!(
        !generated.report.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "method_unknown_type_skipped"
                && diagnostic
                    .path
                    .as_deref()
                    .is_some_and(|path| path.contains("Signatureless"))
        }),
        "signature-less methods must retain descriptor-derived method kinds: {:?}",
        generated.report.diagnostics
    );
    let partialmethod_names = [
        "sync_instance",
        "sync_static",
        "sync_class",
        "async_instance",
        "async_static",
        "async_class",
    ];
    assert!(
        !generated.report.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "class_closure_skipped"
                && diagnostic.path.as_deref().is_some_and(|path| {
                    partialmethod_names
                        .iter()
                        .any(|name| path.ends_with(&format!(".{name}")))
                })
        }),
        "exact class-owned partialmethod descriptors must not be dropped when static/class \
         binding exposes a functools.partial: {:?}",
        generated.report.diagnostics,
    );
    assert!(
        generated.report.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "class_closure_skipped"
                && diagnostic.path.as_deref()
                    == Some("mod_async_signatureless_runtime.PartialKinds.arbitrary_partial")
        }),
        "an arbitrary functools.partial class attribute must remain an unsupported closure: {:?}",
        generated.report.diagnostics,
    );

    let generated_code = prettyplease::unparse(&syn::parse2(generated.tokens).unwrap());
    assert!(
        generated_code.contains("PyResult<::pyo3::Bound<'py, ::pyo3::types::PyAny>>"),
        "async callables must return their live coroutine/generator handle: {generated_code}"
    );
    let signatureless_methods = generated_code
        .split("pub trait SignaturelessMethods")
        .nth(1)
        .expect("SignaturelessMethods must be emitted");
    for method in ["instance_call", "update"] {
        let signature = signatureless_methods
            .split(&format!("fn {method}<'py>("))
            .nth(1)
            .and_then(|rest| rest.split(") ->").next())
            .unwrap_or_else(|| panic!("missing signature-less instance method `{method}`"));
        assert!(
            signature.contains("&self")
                && signature.contains("p_args")
                && signature.contains("p_kwargs"),
            "fallback variadics must survive receiver binding for `{method}`: {signature}"
        );
    }

    let mut working_directory =
        bounded_consumer::OwnedTempDir::allocate("module-methods-async-signatureless-runtime")
            .expect("runtime consumer directory must be allocated exclusively");
    let crate_dir = working_directory.path().to_owned();
    std::fs::create_dir(crate_dir.join("src")).unwrap();

    let pyo3_version = env!("PYO3_BINDGEN_PYO3_VERSION");
    std::fs::write(
        crate_dir.join("Cargo.toml"),
        format!(
            indoc::indoc! { r#"
            [package]
            name = "pyo3_bindgen_async_signatureless_runtime"
            version = "0.0.0"
            edition = "2021"
            publish = false

            [dependencies]
            pyo3 = {{ version = "={pyo3_version}", features = ["auto-initialize"] }}
        "# },
            pyo3_version = pyo3_version
        ),
    )
    .unwrap();
    std::fs::write(
        crate_dir.join("src/lib.rs"),
        format!(
            r#"{generated_code}

#[cfg(test)]
mod tests {{
    use crate::mod_async_signatureless_runtime::{{
        PartialAsyncMethods, PartialCallableMethods, PartialKindsMethods,
        SignaturelessMethods,
    }};
    use pyo3::types::{{PyAnyMethods, PyDictMethods}};

    #[test]
    fn async_and_signatureless_calls_run() -> pyo3::PyResult<()> {{
        pyo3::Python::attach(|py| {{
            crate::mod_async_signatureless_runtime::pyo3_embed_python_source_code(py)?;

            let module =
                py.import(pyo3::intern!(py, "mod_async_signatureless_runtime"))?;
            let builtins = py.import(pyo3::intern!(py, "builtins"))?;
            builtins.setattr(
                pyo3::intern!(py, "classmethod"),
                module.getattr(pyo3::intern!(py, "_real_classmethod"))?,
            )?;
            builtins.setattr(
                pyo3::intern!(py, "staticmethod"),
                module.getattr(pyo3::intern!(py, "_real_staticmethod"))?,
            )?;
            let functools = py.import(pyo3::intern!(py, "functools"))?;
            functools.setattr(
                pyo3::intern!(py, "partial"),
                module.getattr(pyo3::intern!(py, "_real_partial"))?,
            )?;
            functools.setattr(
                pyo3::intern!(py, "partialmethod"),
                module.getattr(pyo3::intern!(py, "_real_partialmethod"))?,
            )?;
            module.call_method0(pyo3::intern!(py, "_restore_inspect"))?;

            let coroutine = crate::mod_async_signatureless_runtime::compute(py, 41)?;
            let inspect = py.import(pyo3::intern!(py, "inspect"))?;
            assert!(
                inspect
                    .call_method1(pyo3::intern!(py, "iscoroutine"), (&coroutine,))?
                    .is_truthy()?
            );
            let answer = py
                .import(pyo3::intern!(py, "asyncio"))?
                .call_method1(pyo3::intern!(py, "run"), (coroutine,))?
                .extract::<i64>()?;
            assert_eq!(answer, 42);

            let callable_partial =
                crate::mod_async_signatureless_runtime::callable_partial(py)?;
            assert!(
                inspect
                    .call_method1(
                        pyo3::intern!(py, "iscoroutine"),
                        (&callable_partial,),
                    )?
                    .is_truthy()?
            );
            assert_eq!(
                py.import(pyo3::intern!(py, "asyncio"))?
                    .call_method1(pyo3::intern!(py, "run"), (callable_partial,))?
                    .extract::<i64>()?,
                42,
            );

            let callable_wrapped =
                crate::mod_async_signatureless_runtime::callable_wrapped_proxy(py, 41)?;
            assert!(
                inspect
                    .call_method1(
                        pyo3::intern!(py, "iscoroutine"),
                        (&callable_wrapped,),
                    )?
                    .is_truthy()?
            );
            assert_eq!(
                py.import(pyo3::intern!(py, "asyncio"))?
                    .call_method1(pyo3::intern!(py, "run"), (callable_wrapped,))?
                    .extract::<i64>()?,
                42,
            );

            let wrapped =
                crate::mod_async_signatureless_runtime::wrapped_compute_proxy(py, 40)?;
            assert!(
                inspect
                    .call_method1(pyo3::intern!(py, "iscoroutine"), (&wrapped,))?
                    .is_truthy()?
            );
            assert_eq!(
                py.import(pyo3::intern!(py, "asyncio"))?
                    .call_method1(pyo3::intern!(py, "run"), (wrapped,))?
                    .extract::<i64>()?,
                42,
            );

            let explicit_signature =
                crate::mod_async_signatureless_runtime::explicit_signature_proxy(py, 40)?;
            assert!(
                inspect
                    .call_method1(
                        pyo3::intern!(py, "iscoroutine"),
                        (&explicit_signature,),
                    )?
                    .is_truthy()?
            );
            assert_eq!(
                py.import(pyo3::intern!(py, "asyncio"))?
                    .call_method1(
                        pyo3::intern!(py, "run"),
                        (explicit_signature,),
                    )?
                    .extract::<i64>()?,
                42,
            );

            let nested_wrapped =
                crate::mod_async_signatureless_runtime::wrapped_async_outer(py, 39)?;
            assert!(
                inspect
                    .call_method1(pyo3::intern!(py, "iscoroutine"), (&nested_wrapped,))?
                    .is_truthy()?
            );
            assert_eq!(
                py.import(pyo3::intern!(py, "asyncio"))?
                    .call_method1(pyo3::intern!(py, "run"), (nested_wrapped,))?
                    .extract::<i64>()?,
                42,
            );

            let generator = crate::mod_async_signatureless_runtime::numbers(py, 3)?;
            assert!(
                inspect
                    .call_method1(pyo3::intern!(py, "isasyncgen"), (&generator,))?
                    .is_truthy()?
            );
            let values = py
                .import(pyo3::intern!(py, "mod_async_signatureless_runtime"))?
                .call_method1(
                    pyo3::intern!(py, "collect_async_generator"),
                    (generator,),
                )?
                .extract::<Vec<i64>>()?;
            assert_eq!(values, vec![0, 1, 2]);

            let partial_kinds =
                crate::mod_async_signatureless_runtime::PartialKinds::from_py_any(
                    module
                        .getattr(pyo3::intern!(py, "PartialKinds"))?
                        .call0()?,
                )?;
            assert_eq!(partial_kinds.sync_instance(py, 2_i64)?, "instance:42");
            assert_eq!(
                crate::mod_async_signatureless_runtime::PartialKinds::sync_static(
                    py, 2_i64,
                )?,
                "static:42",
            );
            assert_eq!(
                crate::mod_async_signatureless_runtime::PartialKinds::sync_class(
                    py, 2_i64,
                )?,
                "PartialKinds:sync:42",
            );

            let partial_instance_async = partial_kinds.async_instance(py, 2_i64)?;
            assert_eq!(
                py.import(pyo3::intern!(py, "asyncio"))?
                    .call_method1(
                        pyo3::intern!(py, "run"),
                        (partial_instance_async,),
                    )?
                    .extract::<String>()?,
                "instance-async:42",
            );
            let partial_static_async =
                crate::mod_async_signatureless_runtime::PartialKinds::async_static(
                    py, 2_i64,
                )?;
            assert_eq!(
                py.import(pyo3::intern!(py, "asyncio"))?
                    .call_method1(
                        pyo3::intern!(py, "run"),
                        (partial_static_async,),
                    )?
                    .extract::<String>()?,
                "static-async:42",
            );
            let partial_class_async =
                crate::mod_async_signatureless_runtime::PartialKinds::async_class(
                    py, 2_i64,
                )?;
            assert_eq!(
                py.import(pyo3::intern!(py, "asyncio"))?
                    .call_method1(
                        pyo3::intern!(py, "run"),
                        (partial_class_async,),
                    )?
                    .extract::<String>()?,
                "PartialKinds:async:42",
            );

            let partial_async =
                crate::mod_async_signatureless_runtime::PartialAsync::from_py_any(
                    module
                        .getattr(pyo3::intern!(py, "PartialAsync"))?
                        .call0()?,
                )?;
            let partial_coroutine = partial_async.via_partial(py)?;
            assert!(
                inspect
                    .call_method1(
                        pyo3::intern!(py, "iscoroutine"),
                        (&partial_coroutine,),
                    )?
                    .is_truthy()?
            );
            assert_eq!(
                py.import(pyo3::intern!(py, "asyncio"))?
                    .call_method1(
                        pyo3::intern!(py, "run"),
                        (partial_coroutine,),
                    )?
                    .extract::<i64>()?,
                42,
            );

            let partial_callable =
                crate::mod_async_signatureless_runtime::PartialCallable::from_py_any(
                    module
                        .getattr(pyo3::intern!(py, "PartialCallable"))?
                        .call0()?,
                )?;
            let callable_method_coroutine = partial_callable.via_callable(py, 2_i64)?;
            assert_eq!(
                py.import(pyo3::intern!(py, "asyncio"))?
                    .call_method1(
                        pyo3::intern!(py, "run"),
                        (callable_method_coroutine,),
                    )?
                    .extract::<String>()?,
                "PartialCallable:callable:42",
            );

            let instance =
                crate::mod_async_signatureless_runtime::Signatureless::from_py_any(
                    module
                        .getattr(pyo3::intern!(py, "Signatureless"))?
                        .call0()?,
                )?;

            let async_instance = instance.async_instance(py, 10_i64)?;
            assert_eq!(
                py.import(pyo3::intern!(py, "asyncio"))?
                    .call_method1(pyo3::intern!(py, "run"), (async_instance,))?
                    .extract::<i64>()?,
                11,
            );
            let async_static =
                crate::mod_async_signatureless_runtime::Signatureless::async_static(
                    py, 10_i64,
                )?;
            assert_eq!(
                py.import(pyo3::intern!(py, "asyncio"))?
                    .call_method1(pyo3::intern!(py, "run"), (async_static,))?
                    .extract::<i64>()?,
                12,
            );
            let async_class =
                crate::mod_async_signatureless_runtime::Signatureless::async_class(
                    py, 10_i64,
                )?;
            assert_eq!(
                py.import(pyo3::intern!(py, "asyncio"))?
                    .call_method1(pyo3::intern!(py, "run"), (async_class,))?
                    .extract::<String>()?,
                "Signatureless:10",
            );

            let args = pyo3::types::PyTuple::new(py, [7_i64])?;
            let kwargs = pyo3::types::PyDict::new(py);
            kwargs.set_item(pyo3::intern!(py, "bonus"), 5_i64)?;
            kwargs.set_item(pyo3::intern!(py, "suffix"), "!")?;

            assert_eq!(
                instance
                    .instance_call(py, &args, Some(kwargs.clone()))?
                    .extract::<i64>()?,
                12,
            );
            assert_eq!(
                crate::mod_async_signatureless_runtime::Signatureless::static_call(
                    py,
                    &args,
                    Some(kwargs.clone()),
                )?
                .extract::<i64>()?,
                12,
            );

            let class_args = pyo3::types::PyTuple::new(py, ["ok"])?;
            assert_eq!(
                crate::mod_async_signatureless_runtime::Signatureless::class_call(
                    py,
                    &class_args,
                    Some(kwargs.clone()),
                )?
                .extract::<String>()?,
                "Signaturelessok!",
            );

            // `dict.update` is a real portable C method descriptor for which
            // inspect.signature raises on CPython. Its positional mapping argument
            // proves the fallback `*args` was not stripped as a fake receiver.
            let mapping = pyo3::types::PyDict::new(py);
            mapping.set_item(pyo3::intern!(py, "updated"), 99_i64)?;
            let update_args = pyo3::types::PyTuple::new(py, [mapping])?;
            instance.update(
                py,
                &update_args,
                Some(pyo3::types::PyDict::new(py)),
            )?;
            assert_eq!(
                instance
                    .bind(py)
                    .get_item(pyo3::intern!(py, "updated"))?
                    .extract::<i64>()?,
                99,
            );

            Ok(())
        }})
    }}
}}
"#
        ),
    )
    .unwrap();

    let mut command =
        std::process::Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned()));
    command
        .args(["test", "--quiet", "--offline"])
        .current_dir(&crate_dir)
        .env("CARGO_TARGET_DIR", crate_dir.join("target"));
    let output = working_directory
        .run(&mut command, std::time::Duration::from_secs(900))
        .unwrap_or_else(|error| {
            panic!(
                "generated async/signature-less runtime smoke command failed: {error}\nstdout:\n{}\nstderr:\n{}\ngenerated:\n{}",
                error.stdout(),
                error.stderr(),
                generated_code,
            )
        });

    assert!(
        output.status.success(),
        "generated async/signature-less runtime smoke failed\nstatus: {}\nstdout:\n{}\nstderr:\n{}\ngenerated:\n{}",
        output.status,
        output.stdout,
        output.stderr,
        generated_code,
    );
}

#[test]
fn postponed_builtin_shadow_annotations_compile_and_run() {
    let generated = pyo3_bindgen_engine::Codegen::default()
        .module_from_str(
            indoc::indoc! { r#"
                from __future__ import annotations
                import builtins

                class int:
                    def __init__(self, value: builtins.int):
                        self.value = value

                class Any:
                    pass

                class Annotated:
                    @classmethod
                    def __class_getitem__(cls, item):
                        return cls

                def echo(value: int) -> int:
                    return value

                def echo_any(value: Any) -> Any:
                    return value

                def echo_annotated(value: Annotated[int]) -> Annotated[int]:
                    return value

                def echo_many(values: list[int]) -> list[int]:
                    return values

                class Holder:
                    def echo(self, value: int) -> int:
                        return value

                    @classmethod
                    def echo_class(cls, value: int) -> int:
                        return value
            "# },
            "mod_postponed_shadow",
        )
        .unwrap()
        .module_from_str(
            indoc::indoc! { r#"
                from __future__ import annotations
                from typing import Any

                def plus_one(value: int) -> int:
                    return value + 1

                def passthrough(value: Any) -> Any:
                    return value
            "# },
            "mod_postponed_builtin_control",
        )
        .unwrap()
        .module_from_str(
            indoc::indoc! { r#"
                from __future__ import annotations
                import builtins

                int = builtins.int

                def plus_two(value: int) -> int:
                    return value + 2
            "# },
            "mod_postponed_builtin_alias_control",
        )
        .unwrap()
        .generate()
        .unwrap();
    let generated_code = prettyplease::unparse(&syn::parse2(generated).unwrap());

    let echo_signature = generated_code
        .split("pub fn echo<'py>(")
        .nth(1)
        .and_then(|rest| rest.split('{').next())
        .unwrap_or_else(|| panic!("missing shadowed free-function signature: {generated_code}"));
    assert!(
        echo_signature.contains("p_value: &int") && echo_signature.contains("PyResult<int>"),
        "postponed bare `int` must resolve to the local wrapper: {echo_signature}"
    );
    let echo_any_signature = generated_code
        .split("pub fn echo_any<'py>(")
        .nth(1)
        .and_then(|rest| rest.split('{').next())
        .unwrap_or_else(|| panic!("missing shadowed Any signature: {generated_code}"));
    assert!(
        echo_any_signature.contains("p_value: &Any")
            && echo_any_signature.contains("PyResult<Any>"),
        "postponed `Any` bound to a local class must not lower as typing.Any: {echo_any_signature}"
    );
    let echo_annotated_signature = generated_code
        .split("pub fn echo_annotated<'py>(")
        .nth(1)
        .and_then(|rest| rest.split('{').next())
        .unwrap_or_else(|| panic!("missing shadowed Annotated signature: {generated_code}"));
    assert!(
        echo_annotated_signature.contains("p_value: &Annotated")
            && echo_annotated_signature.contains("PyResult<Annotated>"),
        "postponed local `Annotated[T]` must not lower as typing.Annotated metadata: \
         {echo_annotated_signature}"
    );
    let echo_many_signature = generated_code
        .split("pub fn echo_many<'py>(")
        .nth(1)
        .and_then(|rest| rest.split('{').next())
        .unwrap_or_else(|| panic!("missing nested shadow signature: {generated_code}"));
    assert!(
        echo_many_signature.contains("p_values: &[int]")
            && echo_many_signature.contains("PyResult<Vec<int>>"),
        "local shadowing must recurse through simple list generics: {echo_many_signature}"
    );
    let control_signature = generated_code
        .split("pub fn plus_one<'py>(")
        .nth(1)
        .and_then(|rest| rest.split('{').next())
        .unwrap_or_else(|| panic!("missing builtin control signature: {generated_code}"));
    assert!(
        control_signature.contains("p_value: i64") && control_signature.contains("PyResult<i64>"),
        "ordinary postponed `int` must remain the genuine builtin: {control_signature}"
    );
    let typing_any_control_signature = generated_code
        .split("pub fn passthrough<'py>(")
        .nth(1)
        .and_then(|rest| rest.split('{').next())
        .unwrap_or_else(|| panic!("missing typing.Any control signature: {generated_code}"));
    assert!(
        typing_any_control_signature.contains("IntoPyObject<'py>")
            && typing_any_control_signature.contains("Bound<'py, ::pyo3::types::PyAny>"),
        "genuine typing.Any must remain untyped: {typing_any_control_signature}"
    );
    let alias_control_signature = generated_code
        .split("pub fn plus_two<'py>(")
        .nth(1)
        .and_then(|rest| rest.split('{').next())
        .unwrap_or_else(|| panic!("missing builtin-alias control signature: {generated_code}"));
    assert!(
        alias_control_signature.contains("p_value: i64")
            && alias_control_signature.contains("PyResult<i64>"),
        "a globals binding to the exact builtin must remain builtin: {alias_control_signature}"
    );

    let mut working_directory =
        bounded_consumer::OwnedTempDir::allocate("module-methods-postponed-shadow-runtime")
            .expect("runtime consumer directory must be allocated exclusively");
    let crate_dir = working_directory.path().to_owned();
    std::fs::create_dir(crate_dir.join("src")).unwrap();

    let pyo3_version = env!("PYO3_BINDGEN_PYO3_VERSION");
    std::fs::write(
        crate_dir.join("Cargo.toml"),
        format!(
            indoc::indoc! { r#"
            [package]
            name = "pyo3_bindgen_postponed_shadow_runtime"
            version = "0.0.0"
            edition = "2021"
            publish = false

            [dependencies]
            pyo3 = {{ version = "={pyo3_version}", features = ["auto-initialize"] }}
        "# },
            pyo3_version = pyo3_version
        ),
    )
    .unwrap();
    std::fs::write(
        crate_dir.join("src/lib.rs"),
        format!(
            r#"{generated_code}

#[cfg(test)]
mod tests {{
    use crate::mod_postponed_shadow::HolderMethods;
    use pyo3::types::PyAnyMethods;

    #[test]
    fn postponed_shadow_calls_run() -> pyo3::PyResult<()> {{
        pyo3::Python::attach(|py| {{
            crate::mod_postponed_shadow::pyo3_embed_python_source_code(py)?;
            crate::mod_postponed_builtin_control::pyo3_embed_python_source_code(py)?;
            crate::mod_postponed_builtin_alias_control::pyo3_embed_python_source_code(py)?;

            let shadow_module = py.import(pyo3::intern!(py, "mod_postponed_shadow"))?;
            let value = crate::mod_postponed_shadow::int::from_py_any(
                shadow_module
                    .getattr(pyo3::intern!(py, "int"))?
                    .call1((7_i64,))?,
            )?;

            let echoed = crate::mod_postponed_shadow::echo(py, &value)?;
            assert_eq!(
                echoed
                    .bind(py)
                    .getattr(pyo3::intern!(py, "value"))?
                    .extract::<i64>()?,
                7,
            );

            let any_value = crate::mod_postponed_shadow::Any::from_py_any(
                shadow_module
                    .getattr(pyo3::intern!(py, "Any"))?
                    .call0()?,
            )?;
            let echoed_any = crate::mod_postponed_shadow::echo_any(py, &any_value)?;
            assert!(
                echoed_any
                    .bind(py)
                    .is_instance(
                        &shadow_module.getattr(pyo3::intern!(py, "Any"))?
                    )?
            );

            let annotated_value = crate::mod_postponed_shadow::Annotated::from_py_any(
                shadow_module
                    .getattr(pyo3::intern!(py, "Annotated"))?
                    .call0()?,
            )?;
            let echoed_annotated =
                crate::mod_postponed_shadow::echo_annotated(py, &annotated_value)?;
            assert!(
                echoed_annotated
                    .bind(py)
                    .is_instance(
                        &shadow_module.getattr(pyo3::intern!(py, "Annotated"))?
                    )?
            );

            let echoed_many = crate::mod_postponed_shadow::echo_many(
                py,
                &[value.clone_ref(py)],
            )?;
            assert_eq!(echoed_many.len(), 1);
            assert_eq!(
                echoed_many[0]
                    .bind(py)
                    .getattr(pyo3::intern!(py, "value"))?
                    .extract::<i64>()?,
                7,
            );

            let holder = crate::mod_postponed_shadow::Holder::from_py_any(
                shadow_module
                    .getattr(pyo3::intern!(py, "Holder"))?
                    .call0()?,
            )?;
            let method_value = holder.echo(py, &value)?;
            assert_eq!(
                method_value
                    .bind(py)
                    .getattr(pyo3::intern!(py, "value"))?
                    .extract::<i64>()?,
                7,
            );
            let class_value =
                crate::mod_postponed_shadow::Holder::echo_class(py, &value)?;
            assert_eq!(
                class_value
                    .bind(py)
                    .getattr(pyo3::intern!(py, "value"))?
                    .extract::<i64>()?,
                7,
            );

            assert_eq!(
                crate::mod_postponed_builtin_control::plus_one(py, 41_i64)?,
                42,
            );
            assert_eq!(
                crate::mod_postponed_builtin_control::passthrough(py, 42_i64)?
                    .extract::<i64>()?,
                42,
            );
            assert_eq!(
                crate::mod_postponed_builtin_alias_control::plus_two(py, 40_i64)?,
                42,
            );
            Ok(())
        }})
    }}
}}
"#
        ),
    )
    .unwrap();

    let mut command =
        std::process::Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned()));
    command
        .args(["test", "--quiet", "--offline"])
        .current_dir(&crate_dir)
        .env("CARGO_TARGET_DIR", crate_dir.join("target"));
    let output = working_directory
        .run(&mut command, std::time::Duration::from_secs(900))
        .unwrap_or_else(|error| {
            panic!(
                "generated postponed-shadow runtime smoke command failed: {error}\nstdout:\n{}\nstderr:\n{}\ngenerated:\n{}",
                error.stdout(),
                error.stderr(),
                generated_code,
            )
        });

    assert!(
        output.status.success(),
        "generated postponed-shadow runtime smoke failed\nstatus: {}\nstdout:\n{}\nstderr:\n{}\ngenerated:\n{}",
        output.status,
        output.stdout,
        output.stderr,
        generated_code,
    );
}
