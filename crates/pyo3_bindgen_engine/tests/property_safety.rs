use pyo3::types::PyAnyMethods;

#[path = "../test_support/bounded_consumer.rs"]
mod bounded_consumer;

#[test]
fn malformed_property_setter_signature_falls_back_to_unknown_annotation() {
    let generated = pyo3_bindgen_engine::Codegen::default()
        .module_from_str(
            indoc::indoc! { r#"
                class Widget:
                    @property
                    def value(self) -> int:
                        return 1

                    @value.setter
                    def value(self):
                        pass
            "# },
            "mod_malformed_property_setter",
        )
        .expect("an importable unusual setter must not panic or fail introspection")
        .generate()
        .expect("an unusual setter must still generate safe bindings");

    let generated = prettyplease::unparse(
        &syn::parse2(generated).expect("generated bindings must remain valid Rust syntax"),
    );
    assert!(generated.contains("fn set_value<'py>("));
    assert!(
        generated.contains("p_value: impl ::pyo3::IntoPyObject<'py>"),
        "missing unknown-annotation setter fallback:\n{generated}"
    );
}

#[test]
fn malformed_live_property_annotations_warn_and_fall_back_to_unknown() {
    let generated = pyo3_bindgen_engine::Codegen::default()
        .module_from_str(
            indoc::indoc! { r#"
                class Widget:
                    @property
                    def value(self) -> int:
                        return 1

                    @value.setter
                    def value(self, next_value: int):
                        pass

                Widget.value.fget.__annotations__["return"] = "list[int"
                Widget.value.fset.__annotations__["next_value"] = "dict[str"
                if hasattr(Widget.value.fget, "__annotate__"):
                    Widget.value.fget.__annotate__ = lambda format: {"return": "list[int"}
                    Widget.value.fset.__annotate__ = lambda format: {"next_value": "dict[str"}
            "# },
            "mod_malformed_live_property_annotations",
        )
        .expect("malformed accessor annotations must not abort live introspection")
        .generate_with_report()
        .expect("malformed accessor annotations must degrade to untyped bindings");

    let source = prettyplease::unparse(
        &syn::parse2(generated.tokens).expect("generated bindings must remain valid Rust"),
    );
    assert!(
        source.contains("fn value<'py>(") && source.contains("fn set_value<'py>("),
        "property accessors with malformed annotations must remain available:\n{source}",
    );
    let fallbacks = generated
        .report
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code == "live_annotation_parse_fallback")
        .count();
    assert_eq!(
        fallbacks, 2,
        "both malformed accessor annotations must warn: {:?}",
        generated.report.diagnostics,
    );
}

#[test]
fn reentrant_module_dir_stdlib_poison_does_not_corrupt_function_signature() {
    let generated = pyo3_bindgen_engine::Codegen::default()
        .module_from_str(
            indoc::indoc! { r#"
                import inspect

                def victim(value: int) -> int:
                    return value

                def __dir__():
                    inspect.Signature = object
                    return ["victim"]
            "# },
            "mod_reentrant_stdlib_poison",
        )
        .expect("a reentrant stdlib mutation must not abort live introspection")
        .generate()
        .expect("the canonical inspect globals must remain usable");
    let source = prettyplease::unparse(
        &syn::parse2(generated).expect("generated bindings must remain valid Rust syntax"),
    );

    assert!(
        source.contains("p_value: i64") && source.contains("PyResult<i64>"),
        "module.__dir__ stdlib poison corrupted the later function signature:\n{source}",
    );
}

#[test]
fn parallel_live_parsing_scopes_target_stdlib_mutations() {
    pyo3::Python::initialize();
    let canonical_property = pyo3::Python::attach(|py| {
        py.import(pyo3::intern!(py, "builtins"))
            .unwrap()
            .getattr(pyo3::intern!(py, "property"))
            .unwrap()
            .unbind()
    });
    let poison = std::thread::spawn(move || {
        pyo3_bindgen_engine::Codegen::default()
            .module_from_str(
                indoc::indoc! { r#"
                    import builtins
                    import time

                    class FakeProperty:
                        pass

                    builtins.property = FakeProperty
                    time.sleep(0.2)
                "# },
                "mod_parallel_stdlib_poison",
            )
            .and_then(pyo3_bindgen_engine::Codegen::generate)
            .map(|_| ())
            .map_err(|error| error.to_string())
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let poison_visible = pyo3::Python::attach(|py| {
            !py.import(pyo3::intern!(py, "builtins"))
                .unwrap()
                .getattr(pyo3::intern!(py, "property"))
                .unwrap()
                .is(canonical_property.bind(py))
        });
        if poison_visible {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "poison target did not reach its concurrent mutation window",
        );
        std::thread::yield_now();
    }
    let victim = std::thread::spawn(move || {
        pyo3_bindgen_engine::Codegen::default()
            .module_from_str(
                indoc::indoc! { r#"
                    class Widget:
                        @property
                        def value(self) -> int:
                            return 1
                "# },
                "mod_parallel_stdlib_victim",
            )
            .and_then(pyo3_bindgen_engine::Codegen::generate)
            .map(|_| ())
            .map_err(|error| error.to_string())
    });

    poison
        .join()
        .expect("poison generation thread must not panic")
        .expect("poison target must remain inspectable");
    victim
        .join()
        .expect("victim generation thread must not panic")
        .expect("parallel target must not observe another target's stdlib poison");
}

#[test]
fn wrapped_and_async_property_getters_use_the_exposed_callable_semantics() {
    let generated = pyo3_bindgen_engine::Codegen::default()
        .module_from_str(
            indoc::indoc! { r#"
                from __future__ import annotations

                import functools
                import inspect

                base_globals = {"__name__": "mod_property_callable_base"}
                exec("""
                from __future__ import annotations

                class int:
                    pass

                def sync_value(self) -> int:
                    return int()

                async def async_value(self) -> int:
                    return int()

                async def async_stream(self) -> int:
                    yield int()
                """, base_globals)

                @functools.wraps(base_globals["sync_value"])
                def sync_proxy(*args, **kwargs):
                    return base_globals["sync_value"](*args, **kwargs)

                @functools.wraps(base_globals["async_value"])
                def async_proxy(*args, **kwargs):
                    return base_globals["async_value"](*args, **kwargs)
                async_proxy.__signature__ = inspect.signature(base_globals["async_value"])

                class Widget:
                    sync_value = property(sync_proxy)
                    async_value = property(async_proxy)
                    async_stream = property(base_globals["async_stream"])

                    @property
                    def normal_value(self) -> int:
                        return 1
            "# },
            "mod_property_callable_semantics",
        )
        .expect("wrapped and async properties must be inspectable")
        .generate()
        .expect("wrapped and async properties must generate");
    let source = prettyplease::unparse(
        &syn::parse2(generated).expect("generated bindings must remain valid Rust"),
    );
    let methods = source
        .split("pub trait WidgetMethods")
        .nth(1)
        .and_then(|rest| rest.split("impl WidgetMethods").next())
        .unwrap_or_else(|| panic!("missing WidgetMethods trait:\n{source}"));
    let signature = |name: &str| {
        methods
            .split(&format!("fn {name}<'py>("))
            .nth(1)
            .and_then(|rest| rest.split(';').next())
            .unwrap_or_else(|| panic!("missing property getter `{name}`:\n{methods}"))
    };

    for name in ["sync_value", "async_value", "async_stream"] {
        let signature = signature(name);
        assert!(
            signature.contains("PyResult<::pyo3::Bound<'py, ::pyo3::types::PyAny>>"),
            "`{name}` must return its live object instead of extracting the annotated \
             awaited value or resolving annotations in wrapper globals:\n{signature}",
        );
    }
    assert!(
        signature("normal_value").contains("PyResult<i64>"),
        "an ordinary synchronous property remains typed:\n{methods}",
    );
}

#[test]
fn async_property_accessors_use_callable_object_semantics() {
    let generated = pyo3_bindgen_engine::Codegen::default()
        .module_from_str(
            indoc::indoc! { r#"
                import functools
                import gc
                import inspect
                import warnings

                async def _reported_async(*args, **kwargs) -> int:
                    return 0

                def _reported_sync(*args, **kwargs) -> int:
                    return 0

                class _ReportsAsyncMeta(type):
                    def __getattribute__(cls, name):
                        if name == "__call__":
                            return _reported_async
                        return type.__getattribute__(cls, name)

                class _ReportsSyncMeta(type):
                    def __getattribute__(cls, name):
                        if name == "__call__":
                            return _reported_sync
                        return type.__getattribute__(cls, name)

                class AsyncGetter:
                    async def __call__(self, owner) -> int:
                        return owner._value

                class AsyncSetter:
                    async def __call__(self, owner, next_value: int):
                        owner.async_callable_setter_calls += 1
                        owner._value = next_value

                class NestedAsyncGetterCall:
                    async def __call__(self, owner) -> int:
                        return owner._value

                class NestedAsyncGetter:
                    __call__ = NestedAsyncGetterCall()

                class NestedAsyncSetterCall:
                    async def __call__(self, owner, next_value: int):
                        owner.async_callable_setter_calls += 1
                        owner._value = next_value

                class NestedAsyncSetter:
                    __call__ = NestedAsyncSetterCall()

                class SpoofedSyncGetter(metaclass=_ReportsAsyncMeta):
                    def __call__(self, owner) -> int:
                        return owner._value

                class SpoofedSyncSetter(metaclass=_ReportsAsyncMeta):
                    def __call__(self, owner, next_value: int):
                        owner.spoofed_sync_setter_calls += 1
                        owner._value = next_value

                class HiddenAsyncGetter(metaclass=_ReportsSyncMeta):
                    async def __call__(self, owner) -> int:
                        return owner._value

                class HiddenAsyncSetter(metaclass=_ReportsSyncMeta):
                    async def __call__(self, owner, next_value: int):
                        owner.async_callable_setter_calls += 1
                        owner._value = next_value

                class SyncGetter:
                    def __call__(self, owner) -> int:
                        return owner._value

                class SyncSetter:
                    def __call__(self, owner, next_value: int):
                        owner.sync_callable_setter_calls += 1
                        owner._value = next_value

                def wrapped_proxy(target):
                    def proxy(*args, **kwargs):
                        return target(*args, **kwargs)
                    proxy.__wrapped__ = target
                    return proxy

                class Widget:
                    def __init__(self):
                        self._value = 0
                        self.async_function_setter_calls = 0
                        self.async_callable_setter_calls = 0
                        self.sync_callable_setter_calls = 0
                        self.spoofed_sync_setter_calls = 0

                    @property
                    def value(self) -> int:
                        return self._value

                    @value.setter
                    async def value(self, next_value: int):
                        self.async_function_setter_calls += 1
                        self._value = next_value

                    bare_callable_value = property(AsyncGetter(), AsyncSetter())
                    partial_callable_value = property(
                        functools.partial(AsyncGetter()),
                        functools.partial(AsyncSetter()),
                    )
                    nested_callable_value = property(
                        NestedAsyncGetter(),
                        NestedAsyncSetter(),
                    )
                    hidden_async_value = property(
                        HiddenAsyncGetter(),
                        HiddenAsyncSetter(),
                    )
                    spoofed_sync_value = property(
                        SpoofedSyncGetter(),
                        SpoofedSyncSetter(),
                    )
                    wrapped_callable_value = property(
                        wrapped_proxy(AsyncGetter()),
                        wrapped_proxy(AsyncSetter()),
                    )
                    sync_callable_value = property(SyncGetter(), SyncSetter())

                    @property
                    def ordinary(self) -> int:
                        return self._value

                    @ordinary.setter
                    def ordinary(self, next_value: int):
                        self._value = next_value

                probe = Widget()
                with warnings.catch_warnings(record=True) as observed:
                    warnings.simplefilter("always")
                    probe.value = 7
                    probe.bare_callable_value = 8
                    probe.partial_callable_value = 9
                    probe.nested_callable_value = 10
                    probe.hidden_async_value = 11
                    probe.wrapped_callable_value = 12
                    probe.spoofed_sync_value = 13
                    gc.collect()
                    callable_getter_results = [
                        probe.bare_callable_value,
                        probe.partial_callable_value,
                        probe.nested_callable_value,
                        probe.hidden_async_value,
                        probe.wrapped_callable_value,
                    ]
                callable_getters_are_coroutines = all(
                    inspect.iscoroutine(result) for result in callable_getter_results
                )
                for result in callable_getter_results:
                    result.close()
                spoofed_sync_roundtrip = probe.spoofed_sync_value == 13
                probe.sync_callable_value = 14
                discarded_async_accessors = (
                    probe.async_function_setter_calls == 0
                    and probe.async_callable_setter_calls == 0
                    and probe.sync_callable_setter_calls == 1
                    and probe.spoofed_sync_setter_calls == 1
                    and callable_getters_are_coroutines
                    and spoofed_sync_roundtrip
                    and probe.sync_callable_value == 14
                    and sum(
                        "was never awaited" in str(warning.message)
                        for warning in observed
                    ) >= 6
                )
            "# },
            "mod_async_property_setter",
        )
        .expect("an async property setter must not abort live introspection")
        .generate_with_report()
        .expect("an async property setter must degrade to a getter-only binding");

    let source = prettyplease::unparse(
        &syn::parse2(generated.tokens).expect("generated bindings must remain valid Rust"),
    );
    let methods = source
        .split("pub trait WidgetMethods")
        .nth(1)
        .and_then(|rest| rest.split("impl WidgetMethods").next())
        .unwrap_or_else(|| panic!("missing WidgetMethods trait:\n{source}"));
    assert!(
        methods.contains("fn value<'py>("),
        "the readable half of an async-setter property must remain:\n{methods}",
    );
    assert!(
        !methods.contains("fn set_value<'py>("),
        "assignment discards an async setter's coroutine, so no setter may be exposed:\n\
         {methods}",
    );
    for name in [
        "bare_callable_value",
        "hidden_async_value",
        "nested_callable_value",
        "partial_callable_value",
        "wrapped_callable_value",
    ] {
        let callable_getter = methods
            .split(&format!("fn {name}<'py>("))
            .nth(1)
            .and_then(|rest| rest.split(';').next())
            .unwrap_or_else(|| panic!("missing async callable-object getter `{name}`:\n{methods}"));
        assert!(
            callable_getter.contains("PyResult<::pyo3::Bound<'py, ::pyo3::types::PyAny>>"),
            "async callable-object getter `{name}` must return its live coroutine:\n\
             {callable_getter}",
        );
        assert!(
            !methods.contains(&format!("fn set_{name}<'py>(")),
            "async callable-object setter `{name}` must not advertise discarded mutation:\n\
             {methods}",
        );
    }
    let sync_callable_getter = methods
        .split("fn sync_callable_value<'py>(")
        .nth(1)
        .and_then(|rest| rest.split(';').next())
        .unwrap_or_else(|| panic!("missing sync callable-object getter:\n{methods}"));
    assert!(
        sync_callable_getter.contains("PyResult<i64>")
            && methods.contains("fn set_sync_callable_value<'py>("),
        "sync callable-object accessors must remain typed and mutable:\n{methods}",
    );
    let spoofed_sync_getter = methods
        .split("fn spoofed_sync_value<'py>(")
        .nth(1)
        .and_then(|rest| rest.split(';').next())
        .unwrap_or_else(|| panic!("missing metaclass-spoofed sync getter:\n{methods}"));
    let modern_callable_signature_lookup = pyo3::Python::attach(|py| py.version_info() >= (3, 11));
    assert!(
        methods.contains("fn set_spoofed_sync_value<'py>(")
            && if modern_callable_signature_lookup {
                spoofed_sync_getter.contains("PyResult<i64>")
            } else {
                spoofed_sync_getter.contains("PyResult<::pyo3::Bound<'py, ::pyo3::types::PyAny>>")
            },
        "metaclass attribute spoofing must not turn actual sync accessors async; pre-3.11 \
         signature provenance remains conservatively untyped:\n{methods}",
    );
    assert!(
        methods.contains("fn set_ordinary<'py>("),
        "ordinary synchronous property setters must remain available:\n{methods}",
    );
    let diagnostics = generated
        .report
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code == "async_property_setter_skipped")
        .collect::<Vec<_>>();
    assert_eq!(
        diagnostics.len(),
        6,
        "each skipped async setter must produce one stable diagnostic: {:?}",
        generated.report.diagnostics,
    );
    assert!(
        [
            "mod_async_property_setter.Widget.bare_callable_value",
            "mod_async_property_setter.Widget.hidden_async_value",
            "mod_async_property_setter.Widget.nested_callable_value",
            "mod_async_property_setter.Widget.partial_callable_value",
            "mod_async_property_setter.Widget.value",
            "mod_async_property_setter.Widget.wrapped_callable_value",
        ]
        .iter()
        .all(|expected| {
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.path.as_deref() == Some(*expected))
        }),
        "diagnostics must identify both async setters: {diagnostics:?}",
    );

    pyo3::Python::attach(|py| {
        let module = py
            .import(pyo3::intern!(py, "mod_async_property_setter"))
            .unwrap();
        let discarded = pyo3::types::PyAnyMethods::getattr(
            module.as_any(),
            pyo3::intern!(py, "discarded_async_accessors"),
        )
        .unwrap();
        assert!(
            pyo3::types::PyAnyMethods::is_truthy(&discarded).unwrap(),
            "runtime source must prove async callable accessors return discarded/live \
             coroutines while sync callable accessors execute normally",
        );
    });
}

#[test]
fn property_callable_descriptors_and_func_spoofs_follow_runtime_binding() {
    let generated = pyo3_bindgen_engine::Codegen::default()
        .module_from_str(
            indoc::indoc! { r#"
                import gc
                import inspect
                import warnings

                async def _reported_async(*args, **kwargs) -> int:
                    return 0

                def _reported_sync(*args, **kwargs) -> int:
                    return 0

                def _report_async_get(raw, instance, owner):
                    return _reported_async

                def _report_sync_get(raw, instance, owner):
                    return _reported_sync

                class _SwitchingCall:
                    def __init__(self, class_target, instance_target, reported_get):
                        object.__setattr__(self, "class_target", class_target)
                        object.__setattr__(self, "instance_target", instance_target)
                        object.__setattr__(self, "reported_get", reported_get)

                    def __getattribute__(self, name):
                        if name == "__get__":
                            return object.__getattribute__(self, "reported_get")
                        return object.__getattribute__(self, name)

                    def __get__(self, instance, owner):
                        target = object.__getattribute__(
                            self,
                            "class_target" if instance is None else "instance_target",
                        )
                        if instance is None:
                            return target
                        return target.__get__(instance, owner)

                async def _async_get(accessor, owner) -> int:
                    return owner._value

                async def _async_set(accessor, owner, next_value: int):
                    owner.async_setter_calls += 1
                    owner._value = next_value

                def _sync_get(accessor, owner) -> int:
                    return owner._value

                def _sync_set(accessor, owner, next_value: int):
                    owner.sync_setter_calls += 1
                    owner._value = next_value

                class _RuntimeAsyncGetter:
                    __call__ = _SwitchingCall(
                        _sync_get, _async_get, _report_sync_get
                    )

                class _RuntimeAsyncSetter:
                    __call__ = _SwitchingCall(
                        _sync_set, _async_set, _report_sync_get
                    )

                class _RuntimeSyncGetter:
                    __call__ = _SwitchingCall(
                        _async_get, _sync_get, _report_async_get
                    )

                class _RuntimeSyncSetter:
                    __call__ = _SwitchingCall(
                        _async_set, _sync_set, _report_async_get
                    )

                class _RaisingCall:
                    def __get__(self, instance, owner):
                        raise RuntimeError("hostile descriptor binding")

                class _BrokenGetter:
                    __call__ = _RaisingCall()

                def _spoofed_get(owner) -> int:
                    return owner._value

                def _spoofed_set(owner, next_value: int):
                    owner.spoofed_setter_calls += 1
                    owner._value = next_value

                _spoofed_get.__func__ = _reported_async
                _spoofed_get.__self__ = object()
                _spoofed_set.__func__ = _reported_async
                _spoofed_set.__self__ = object()

                class Widget:
                    def __init__(self):
                        self._value = 0
                        self.async_setter_calls = 0
                        self.sync_setter_calls = 0
                        self.spoofed_setter_calls = 0

                    descriptor_async = property(
                        _RuntimeAsyncGetter(), _RuntimeAsyncSetter()
                    )
                    descriptor_sync = property(
                        _RuntimeSyncGetter(), _RuntimeSyncSetter()
                    )
                    spoofed_func = property(_spoofed_get, _spoofed_set)
                    broken = property(_BrokenGetter())

                probe = Widget()
                with warnings.catch_warnings(record=True) as observed:
                    warnings.simplefilter("always")
                    probe.descriptor_async = 1
                    gc.collect()
                    async_result = probe.descriptor_async
                async_getter_is_coroutine = inspect.iscoroutine(async_result)
                async_result.close()
                probe.descriptor_sync = 2
                probe.spoofed_func = 3
                runtime_binding_controls = (
                    probe.async_setter_calls == 0
                    and probe.sync_setter_calls == 1
                    and probe.spoofed_setter_calls == 1
                    and async_getter_is_coroutine
                    and probe.descriptor_sync == 3
                    and probe.spoofed_func == 3
                    and any(
                        "was never awaited" in str(warning.message)
                        for warning in observed
                    )
                )
            "# },
            "mod_property_descriptor_binding",
        )
        .expect("descriptor-bound property accessors must remain inspectable")
        .generate_with_report()
        .expect("hostile descriptor binding must fail conservative, not abort generation");

    let source = prettyplease::unparse(
        &syn::parse2(generated.tokens).expect("generated bindings must remain valid Rust"),
    );
    let methods = source
        .split("pub trait WidgetMethods")
        .nth(1)
        .and_then(|rest| rest.split("impl WidgetMethods").next())
        .unwrap_or_else(|| panic!("missing WidgetMethods trait:\n{source}"));
    let getter = |name: &str| {
        methods
            .split(&format!("fn {name}<'py>("))
            .nth(1)
            .and_then(|rest| rest.split(';').next())
            .unwrap_or_else(|| panic!("missing property getter `{name}`:\n{methods}"))
    };
    assert!(
        getter("descriptor_async").contains("PyResult<::pyo3::Bound<'py, ::pyo3::types::PyAny>>")
            && !methods.contains("fn set_descriptor_async<'py>("),
        "runtime-bound async descriptor accessors must stay live/getter-only:\n{methods}",
    );
    assert!(
        getter("descriptor_sync").contains("PyResult<::pyo3::Bound<'py, ::pyo3::types::PyAny>>")
            && methods.contains("fn set_descriptor_sync<'py>("),
        "runtime-sync custom descriptors must remain mutable while their dynamic signature \
         provenance stays untyped:\n{methods}",
    );
    assert!(
        getter("spoofed_func").contains("PyResult<i64>")
            && methods.contains("fn set_spoofed_func<'py>("),
        "writable __func__ spoofing must not affect a plain sync function:\n{methods}",
    );
    assert!(
        getter("broken").contains("PyResult<::pyo3::Bound<'py, ::pyo3::types::PyAny>>"),
        "raising descriptor binding must degrade to a live Python object:\n{methods}",
    );
    assert!(
        generated.report.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "async_property_setter_skipped"
                && diagnostic.path.as_deref()
                    == Some("mod_property_descriptor_binding.Widget.descriptor_async")
        }),
        "the runtime-async descriptor setter needs its stable diagnostic: {:?}",
        generated.report.diagnostics,
    );
    pyo3::Python::attach(|py| {
        let module = py
            .import(pyo3::intern!(py, "mod_property_descriptor_binding"))
            .unwrap();
        let controls = pyo3::types::PyAnyMethods::getattr(
            module.as_any(),
            pyo3::intern!(py, "runtime_binding_controls"),
        )
        .unwrap();
        assert!(
            pyo3::types::PyAnyMethods::is_truthy(&controls).unwrap(),
            "Python runtime controls must prove descriptor and spoof behavior",
        );
    });
}

#[test]
fn property_callable_annotation_globals_are_exact_or_untyped() {
    let generated = pyo3_bindgen_engine::Codegen::default()
        .module_from_str(
            indoc::indoc! { r#"
                from __future__ import annotations

                import inspect
                import typing

                class int:
                    pass

                class Getter:
                    def __call__(self, owner) -> int:
                        return owner._value

                class Setter:
                    def __call__(self, owner, next_value: int):
                        owner._value = next_value

                class NoneSignatureGetter:
                    __signature__ = None

                    def __call__(self, owner) -> int:
                        return owner._value

                class ExplicitSignatureGetter:
                    __signature__ = inspect.Signature(return_annotation="int")

                    def __call__(self, owner) -> int:
                        return owner._value

                class ForwardRefSignatureGetter:
                    __signature__ = inspect.Signature(
                        return_annotation=typing.ForwardRef("int")
                    )

                    def __call__(self, owner) -> int:
                        return owner._value

                class NestedForwardRefSignatureGetter:
                    __signature__ = inspect.Signature(
                        return_annotation=typing.List[typing.ForwardRef("int")]
                    )

                    def __call__(self, owner) -> int:
                        return owner._value

                def _dynamic_target(owner) -> int:
                    return owner._value

                class DynamicWrappedGetter:
                    def __getattribute__(self, name):
                        if name == "__wrapped__":
                            return _dynamic_target
                        return object.__getattribute__(self, name)

                    def __call__(self, owner) -> int:
                        return owner._value

                class Widget:
                    def __init__(self):
                        self._value = int()

                    value = property(Getter(), Setter())
                    none_signature = property(NoneSignatureGetter())
                    explicit_signature = property(ExplicitSignatureGetter())
                    forward_ref_signature = property(ForwardRefSignatureGetter())
                    nested_forward_ref_signature = property(
                        NestedForwardRefSignatureGetter()
                    )
                    dynamic_wrapped = property(DynamicWrappedGetter())

                probe = Widget()
                next_value = int()
                probe.value = next_value
                annotation_runtime_controls = (
                    probe.value is next_value
                    and isinstance(probe.none_signature, int)
                    and isinstance(probe.explicit_signature, int)
                    and isinstance(probe.forward_ref_signature, int)
                    and isinstance(probe.nested_forward_ref_signature, int)
                    and isinstance(probe.dynamic_wrapped, int)
                )
            "# },
            "mod_property_callable_globals",
        )
        .expect("callable-object property annotations must be inspectable")
        .generate_with_report()
        .expect("untrusted annotation provenance must degrade instead of abort");

    let source = prettyplease::unparse(
        &syn::parse2(generated.tokens).expect("generated bindings must remain valid Rust"),
    );
    let methods = source
        .split("pub trait WidgetMethods")
        .nth(1)
        .and_then(|rest| rest.split("impl WidgetMethods").next())
        .unwrap_or_else(|| panic!("missing WidgetMethods trait:\n{source}"));
    assert!(
        methods
            .split("fn value<'py>(")
            .nth(1)
            .and_then(|rest| rest.split(';').next())
            .is_some_and(|signature| signature.contains("PyResult<int>"))
            && methods.contains("fn set_value<'py>(")
            && methods.contains("p_value: &int"),
        "ordinary callable-object getter/setter annotations must use __call__ globals:\n{methods}",
    );
    assert!(
        methods
            .split("fn none_signature<'py>(")
            .nth(1)
            .and_then(|rest| rest.split(';').next())
            .is_some_and(|signature| signature.contains("PyResult<int>")),
        "explicit __signature__ = None must continue natural callable dispatch:\n{methods}",
    );
    for name in [
        "dynamic_wrapped",
        "explicit_signature",
        "forward_ref_signature",
        "nested_forward_ref_signature",
    ] {
        assert!(
            methods
                .split(&format!("fn {name}<'py>("))
                .nth(1)
                .and_then(|rest| rest.split(';').next())
                .is_some_and(|signature| {
                    signature.contains("PyResult<::pyo3::Bound<'py, ::pyo3::types::PyAny>>")
                }),
            "untrusted string provenance for `{name}` must remain untyped:\n{methods}",
        );
    }
    let untrusted_paths = generated
        .report
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code == "signature_annotation_source_untrusted")
        .filter_map(|diagnostic| diagnostic.path.as_deref())
        .collect::<Vec<_>>();
    assert!(
        [
            "mod_property_callable_globals.Widget.dynamic_wrapped",
            "mod_property_callable_globals.Widget.explicit_signature",
            "mod_property_callable_globals.Widget.forward_ref_signature",
            "mod_property_callable_globals.Widget.nested_forward_ref_signature",
        ]
        .iter()
        .all(|path| untrusted_paths.contains(path)),
        "untrusted property annotation sources need stable paths: {untrusted_paths:?}",
    );
    pyo3::Python::attach(|py| {
        let module = py
            .import(pyo3::intern!(py, "mod_property_callable_globals"))
            .unwrap();
        let controls = pyo3::types::PyAnyMethods::getattr(
            module.as_any(),
            pyo3::intern!(py, "annotation_runtime_controls"),
        )
        .unwrap();
        assert!(
            pyo3::types::PyAnyMethods::is_truthy(&controls).unwrap(),
            "Python runtime controls must prove shadowed callable annotations",
        );
    });
}

#[test]
fn mutable_builtins_property_does_not_change_property_semantics() {
    let source = indoc::indoc! { r#"
                import builtins
                import datetime
                import inspect

                _real_property = builtins.property

                class Widget:
                    def __init__(self):
                        self._value = 0

                    @_real_property
                    def value(self) -> int:
                        return self._value

                    @value.setter
                    def value(self, next_value: int):
                        self._value = next_value

                    @_real_property
                    def async_value(self) -> int:
                        return self._value

                    @async_value.setter
                    async def async_value(self, next_value: int):
                        self._value = next_value

                class RealPropertySubclass(_real_property):
                    pass

                def _subclass_get(owner) -> int:
                    return owner._value

                def _subclass_set(owner, next_value: int):
                    owner._value = next_value

                Widget.subclassed = RealPropertySubclass(
                    _subclass_get, _subclass_set
                )

                class _FakePropertyMeta(type):
                    def __instancecheck__(cls, instance):
                        return False

                class _FakeProperty(metaclass=_FakePropertyMeta):
                    pass

                builtins.property = _FakeProperty
            "# };
    let codegen = pyo3_bindgen_engine::Codegen::default()
        .module_from_str(source, "mod_mutated_builtin_property")
        .expect("module with a saved real property type must import");
    let generated_result = codegen.generate_with_report();
    let generated = generated_result
        .expect("mutable builtins.property must not alter canonical descriptor recognition");
    let source = prettyplease::unparse(
        &syn::parse2(generated.tokens).expect("generated bindings must remain valid Rust"),
    );
    let methods = source
        .split("pub trait WidgetMethods")
        .nth(1)
        .and_then(|rest| rest.split("impl WidgetMethods").next())
        .unwrap_or_else(|| panic!("missing WidgetMethods trait:\n{source}"));
    for name in ["value", "subclassed"] {
        assert!(
            methods.contains(&format!("fn {name}<'py>("))
                && methods.contains(&format!("fn set_{name}<'py>(")),
            "real property `{name}` must remain typed/mutable under builtin mutation:\n{methods}",
        );
    }
    assert!(
        methods.contains("fn async_value<'py>(") && !methods.contains("fn set_async_value<'py>("),
        "real async property setter must remain suppressed under builtin mutation:\n{methods}",
    );
    assert!(
        generated.report.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "async_property_setter_skipped"
                && diagnostic.path.as_deref()
                    == Some("mod_mutated_builtin_property.Widget.async_value")
        }),
        "async setter diagnostic must survive builtins.property replacement: {:?}",
        generated.report.diagnostics,
    );
}

#[test]
fn postponed_pseudo_field_spelling_respects_shadowed_module_roots() {
    let generated = pyo3_bindgen_engine::Codegen::default()
        .module_from_str(
            indoc::indoc! { r#"
                from __future__ import annotations

                class FakeTyping:
                    ClassVar = int

                typing = FakeTyping

                class Shadowed:
                    value: typing.ClassVar[int]

                    def __init__(self, value):
                        self.value = value
            "# },
            "mod_shadowed_pseudo_field",
        )
        .expect("shadowed pseudo-field module must parse")
        .generate()
        .expect("shadowed pseudo-field bindings must generate");
    let source = prettyplease::unparse(
        &syn::parse2(generated).expect("generated bindings must remain valid Rust syntax"),
    );
    let methods = source
        .split("pub trait ShadowedMethods")
        .nth(1)
        .and_then(|rest| rest.split("impl ShadowedMethods").next())
        .expect("ShadowedMethods trait must be generated");
    assert!(
        methods.contains("fn value<'py>(") && methods.contains("fn set_value<'py>("),
        "a shadowed `typing.ClassVar` spelling is an ordinary instance field:\n{methods}"
    );
}

fn generate_live_module(module_name: &str) -> String {
    let config = pyo3_bindgen_engine::Config {
        generate_provenance: false,
        ..Default::default()
    };
    let generated = pyo3_bindgen_engine::Codegen::new(config)
        .module_names([module_name])
        .expect("stdlib module must be importable")
        .generate()
        .expect("stdlib bindings must generate");
    prettyplease::unparse(
        &syn::parse2(generated).expect("generated bindings must remain valid Rust syntax"),
    )
}

#[test]
fn readonly_c_member_descriptors_do_not_emit_setters() {
    let datetime = generate_live_module("datetime");
    for setter in ["set_days", "set_seconds", "set_microseconds"] {
        assert!(
            !datetime.contains(&format!("fn {setter}<'py>(")),
            "readonly datetime.timedelta member emitted `{setter}`:\n{datetime}"
        );
    }

    let builtins = generate_live_module("builtins");
    for setter in ["set_start", "set_stop", "set_step"] {
        assert!(
            !builtins.contains(&format!("fn {setter}<'py>(")),
            "readonly slice member emitted `{setter}`:\n{builtins}"
        );
    }
}

#[test]
fn declared_python_slot_member_descriptor_still_emits_setter() {
    let generated = pyo3_bindgen_engine::Codegen::default()
        .module_from_str(
            indoc::indoc! { r#"
                class Slotted:
                    __slots__ = ("value",)
            "# },
            "mod_writable_slot_property_safety",
        )
        .expect("slotted module must be importable")
        .generate()
        .expect("slotted bindings must generate");
    let generated = prettyplease::unparse(
        &syn::parse2(generated).expect("generated bindings must remain valid Rust syntax"),
    );

    assert!(
        generated.contains("fn set_value<'py>("),
        "declared Python slot must retain its setter:\n{generated}"
    );
}

#[test]
fn mutable_member_descriptor_alias_does_not_drop_slot_setter() {
    let source = indoc::indoc! { r#"
                import inspect
                import types

                _real_member_descriptor_type = types.MemberDescriptorType

                class Slotted:
                    __slots__ = ("value",)

                types.MemberDescriptorType = object
            "# };
    let codegen = pyo3_bindgen_engine::Codegen::default()
        .module_from_str(source, "mod_mutated_member_descriptor")
        .expect("module with mutated types.MemberDescriptorType must import");
    let generated_result = codegen.generate();
    pyo3::Python::attach(|py| {
        let module = py
            .import(pyo3::intern!(py, "mod_mutated_member_descriptor"))
            .unwrap();
        let types = py.import(pyo3::intern!(py, "types")).unwrap();
        let saved = pyo3::types::PyAnyMethods::getattr(
            module.as_any(),
            pyo3::intern!(py, "_real_member_descriptor_type"),
        )
        .unwrap();
        let current = pyo3::types::PyAnyMethods::getattr(
            types.as_any(),
            pyo3::intern!(py, "MemberDescriptorType"),
        )
        .unwrap();
        assert!(
            pyo3::types::PyAnyMethods::is(&current, &saved),
            "live parsing must restore the pre-target types.MemberDescriptorType",
        );
    });
    let generated = generated_result
        .expect("canonical member descriptor recognition must ignore the mutable alias");
    let source = prettyplease::unparse(
        &syn::parse2(generated).expect("generated bindings must remain valid Rust"),
    );
    assert!(
        source.contains("fn set_value<'py>("),
        "declared Python slot must retain its setter after alias mutation:\n{source}",
    );
}

const PEP649_ANNOTATION_SOURCE: &str = indoc::indoc! { r#"
                import typing

                evaluations = 0

                def metadata():
                    global evaluations
                    evaluations += 1
                    return "metadata"

                class Lazy:
                    field: typing.Annotated[int, metadata()]

                    def __init__(self, field: int):
                        self.field = field
                        self._value = field

                    @property
                    def value(self) -> typing.Annotated[int, metadata()]:
                        return self._value

                    @value.setter
                    def value(
                        self,
                        value: typing.Annotated[int, metadata()],
                    ):
                        self._value = value

                def lazy_function(
                    value: typing.Annotated[int, metadata()],
                ) -> typing.Annotated[str, metadata()]:
                    return str(value)

                def unresolved(value: Missing) -> Missing:
                    return value
            "# };

const PEP649_STRING_ORACLE_SUFFIX: &str = indoc::indoc! { r#"
                import annotationlib
                import inspect

                _string_format = annotationlib.Format.STRING
                annotationlib.get_annotations(Lazy, format=_string_format)
                _property = Lazy.value
                inspect.signature(_property.fget, annotation_format=_string_format)
                inspect.signature(_property.fset, annotation_format=_string_format)
                inspect.signature(lazy_function, annotation_format=_string_format)
                inspect.signature(unresolved, annotation_format=_string_format)
                oracle_evaluations = evaluations
            "# };

#[test]
fn pep649_annotations_preserve_shape_and_types_with_string_introspection() {
    pyo3::Python::initialize();
    if !pyo3::Python::attach(|py| py.version_info() >= (3, 14)) {
        return;
    }

    let oracle_module = "mod_pep649_annotations_string_oracle";
    let oracle_source = format!("{PEP649_ANNOTATION_SOURCE}\n{PEP649_STRING_ORACLE_SUFFIX}");
    pyo3_bindgen_engine::Codegen::default()
        .module_from_str(&oracle_source, oracle_module)
        .expect("the independent public STRING oracle fixture must import");
    let expected_evaluations = pyo3::Python::attach(|py| {
        py.import(oracle_module)
            .expect("the independent public STRING oracle fixture must remain importable")
            .getattr(pyo3::intern!(py, "oracle_evaluations"))
            .expect("the public STRING oracle must record its evaluation count")
            .extract::<usize>()
            .expect("the public STRING oracle evaluation count must be an integer")
    });

    let generated = pyo3_bindgen_engine::Codegen::default()
        .module_from_str(PEP649_ANNOTATION_SOURCE, "mod_pep649_annotations")
        .expect("Python 3.14 deferred annotations must be inspectable in STRING format")
        .generate()
        .expect("deferred annotations must generate");
    let source = prettyplease::unparse(
        &syn::parse2(generated).expect("generated PEP 649 bindings must be valid Rust"),
    );

    let lazy_methods = source
        .split("pub trait LazyMethods")
        .nth(1)
        .and_then(|rest| rest.split("impl LazyMethods").next())
        .expect("LazyMethods trait must be generated");
    for accessor in ["field", "value"] {
        assert!(
            lazy_methods.contains(&format!("fn {accessor}<'py>("))
                && lazy_methods.contains(&format!("fn set_{accessor}<'py>(")),
            "deferred class/property annotation lost `{accessor}` accessors:\n{lazy_methods}",
        );
    }
    assert!(
        lazy_methods.matches("PyResult<i64>").count() >= 2
            && lazy_methods.matches("p_value: i64").count() >= 2,
        "Annotated[int, ...] class/property types must remain exact:\n{lazy_methods}",
    );
    let lazy_function = source
        .split("pub fn lazy_function")
        .nth(1)
        .and_then(|rest| rest.split('{').next())
        .expect("lazy_function signature must be generated");
    assert!(
        lazy_function.contains("p_value: i64")
            && lazy_function.contains("PyResult<::std::string::String>"),
        "deferred function annotations must retain exact parameter/return types:\n\
         {lazy_function}",
    );
    let unresolved = source
        .split("pub fn unresolved")
        .nth(1)
        .and_then(|rest| rest.split('{').next())
        .expect("unresolved signature must be generated");
    assert!(
        unresolved.contains("p_value:")
            && !unresolved.contains("p_args:")
            && !unresolved.contains("p_kwargs:"),
        "an unresolved deferred annotation must not erase callable arity:\n{unresolved}",
    );
    pyo3::Python::attach(|py| {
        let module = py
            .import(pyo3::intern!(py, "mod_pep649_annotations"))
            .unwrap();
        assert_eq!(
            pyo3::types::PyAnyMethods::extract::<usize>(
                &pyo3::types::PyAnyMethods::getattr(
                    module.as_any(),
                    pyo3::intern!(py, "evaluations"),
                )
                .unwrap(),
            )
            .unwrap(),
            expected_evaluations,
            "annotation metadata effects must match independent public STRING introspection",
        );
    });
}

const LIVE_ANNOTATED_FIELDS_SOURCE: &str = indoc::indoc! { r#"
    from __future__ import annotations
    import dataclasses
    import datetime
    import typing
    import typing as t
    import dataclasses as dc
    from dataclasses import InitVar, dataclass
    from typing import ClassVar

    CVInt = ClassVar[int]
    IVInt = InitVar[int]

    class Plain:
        value: int
        declared: int = None
        runtime_class_var: ClassVar[int] = 11
        postponed_class_var: "typing.ClassVar[str]" = "skip"
        aliased_class_var: t.ClassVar[bytes] = b"skip"
        parameterized_alias_class_var: CVInt = 12
        _private_value: int
        test: int

        def __init__(self, value: int):
            self.value = value
            self._private_value = value
            self.test = value

    class DerivedPlain(Plain):
        pass

    class Any:
        pass

    class ShadowedFields:
        value: Any

        def __init__(self, value: Any):
            self.value = value

        @property
        def alias(self) -> Any:
            return self.value

        @alias.setter
        def alias(self, value: Any):
            self.value = value

    class WriteOnly:
        def __init__(self, value: int):
            self._value = value

        def _set_value(self, value: int):
            self._value = value

        value = property(fset=_set_value)

    @dataclass
    class Data:
        score: int

    def default_callback(value: int) -> int:
        return value + 1

    class CallableBase:
        def value(self) -> int:
            return -1

    @dataclass
    class CallableFields(CallableBase):
        value: int
        callback: typing.Callable[[int], int] = default_callback

    class DerivedCallableFields(CallableFields):
        pass

    class OverriddenCallableField(CallableFields):
        def callback(self) -> str:
            return "method override"

    class AnnotatedMethod:
        method: typing.Callable[[int], int]

        def method(self, value: int) -> int:
            return value + 1

    class ClassVarAnnotatedMethod:
        work: ClassVar[typing.Callable[[int], int]]

        def work(self, value: int) -> int:
            return value + 2

    class ClassVarMethodBase:
        inherited_work: ClassVar[typing.Callable[[int], int]]

    class ClassVarMethodOverride(ClassVarMethodBase):
        def inherited_work(self, value: int) -> int:
            return value + 3

    class RuntimeMethodBase:
        def inherited_runtime(self, value: int) -> int:
            return value + 4

    class PseudoAnnotatedMethod(RuntimeMethodBase):
        inherited_runtime: ClassVar[typing.Callable[[int], int]]

    class RuntimePropertyBase:
        @property
        def inherited_property(self) -> int:
            return 9

    class PseudoAnnotatedProperty(RuntimePropertyBase):
        inherited_property: ClassVar[int]

    if hasattr(dataclasses, "KW_ONLY"):
        @dataclass
        class PseudoFields:
            value: int
            separator: dc.KW_ONLY
            temporary: dc.InitVar[int]
            stored: int = 0

            def __post_init__(self, temporary: int):
                self.stored = temporary
    else:
        # `dataclasses.KW_ONLY` was added in CPython 3.10. The older compatibility
        # lane still exercises InitVar filtering without manufacturing a false field.
        @dataclass
        class PseudoFields:
            value: int
            temporary: dc.InitVar[int]
            stored: int = 0

            def __post_init__(self, temporary: int):
                self.stored = temporary

    @dataclass
    class AliasedPseudoFields:
        value: int
        temporary: IVInt

        def __post_init__(self, temporary: int):
            self.value += temporary

    class PlainInitVarAnnotation:
        temporary: InitVar[int]

        def __init__(self, temporary: int):
            self.temporary = temporary

    class Slotted:
        __slots__ = ("amount",)
        amount: int

        def __init__(self, amount: int):
            self.amount = amount

    class ReadonlyAnnotatedDelta(datetime.timedelta):
        days: int

    class _RejectingDescriptor:
        def __get__(self, instance, owner):
            return 1

        def __set__(self, instance, value):
            raise AttributeError("readonly")

    class ReadonlyCustomDescriptor:
        blocked: int = _RejectingDescriptor()

    class _DeleteOnlyDescriptor:
        def __get__(self, instance, owner):
            return 1

        def __delete__(self, instance):
            pass

    class DeleteOnlyHost:
        delete_only: int = _DeleteOnlyDescriptor()

    @dataclass(frozen=True)
    class Frozen:
        locked: int

    @dataclass(frozen=True)
    class FrozenSlots:
        __slots__ = ("locked_slot",)
        locked_slot: int

    @dataclass(frozen=True)
    class FrozenProperty:
        _value: int

        @property
        def value(self) -> int:
            return self._value

        @value.setter
        def value(self, value: int):
            object.__setattr__(self, "_value", value)

    @dataclass(frozen=True)
    class FrozenBase:
        base: int

    class MutableFrozenChild(FrozenBase):
        base: str
        value: int

        def __init__(self, base: str, value: int):
            super().__init__(base)
            self.value = value

    @dataclass(frozen=True)
    class FrozenLeft:
        left: int

    @dataclass(frozen=True)
    class FrozenRight:
        right: int

    class MutableFrozenPair(FrozenLeft, FrozenRight):
        right: str
        fresh: int

    class CustomSetattr:
        denied: int

        def __setattr__(self, name, value):
            raise AttributeError("readonly")

    class Descriptor:
        value: int

        def __init__(self, value: str):
            self._value = value

        @property
        def value(self) -> str:
            return self._value

        @value.setter
        def value(self, value: str):
            self._value = value

    class Malformed:
        broken: "tuple[int], str]"

        def __init__(self):
            self.broken = object()

    class HostileAnnotations:
        pass

    class _ExplodingAnnotations:
        def items(self):
            raise RuntimeError("annotation mapping must be fail-soft")

    HostileAnnotations.__annotations__ = _ExplodingAnnotations()
"# };

fn assert_generated_annotation_bindings_run(generated: &str) {
    let mut working_directory = bounded_consumer::OwnedTempDir::allocate("property-safety")
        .expect("temporary crate directory must be allocated exclusively");
    let crate_dir = working_directory.path().to_owned();
    std::fs::create_dir(crate_dir.join("src")).expect("temporary crate must be creatable");
    let pyo3_version = env!("PYO3_BINDGEN_PYO3_VERSION");
    std::fs::write(
        crate_dir.join("Cargo.toml"),
        format!(
            r#"[package]
name = "pyo3_bindgen_property_safety_fixture"
version = "0.0.0"
edition = "2021"
publish = false

[workspace]

[dependencies]
pyo3 = {{ version = "={pyo3_version}", features = ["auto-initialize"] }}
"#
        ),
    )
    .expect("temporary manifest must be writable");
    std::fs::write(
        crate_dir.join("src/lib.rs"),
        format!(
            r#"{generated}

#[cfg(test)]
mod tests {{
    use super::mod_live_annotated_fields::{{
        self, AnnotatedMethod, AnnotatedMethodMethods, Any, CallableFields,
        CallableFieldsMethods, ClassVarAnnotatedMethod, ClassVarAnnotatedMethodMethods,
        ClassVarMethodOverride, ClassVarMethodOverrideMethods, Data, DataMethods,
        DerivedCallableFields, DerivedCallableFieldsMethods, DerivedPlain, DerivedPlainMethods,
        Descriptor, DescriptorMethods, FrozenProperty, FrozenPropertyMethods, FrozenSlots,
        FrozenSlotsMethods, MutableFrozenChild, MutableFrozenChildMethods, Plain,
        PlainInitVarAnnotation, PlainInitVarAnnotationMethods, PlainMethods, ShadowedFields,
        ShadowedFieldsMethods, Slotted, SlottedMethods, PseudoAnnotatedMethod,
        PseudoAnnotatedMethodMethods, PseudoAnnotatedProperty,
        PseudoAnnotatedPropertyMethods, WriteOnly, WriteOnlyMethods,
    }};
    use pyo3::types::PyAnyMethods;

    #[test]
    fn generated_annotation_bindings_run() -> ::pyo3::PyResult<()> {{
        ::pyo3::Python::attach(|py| {{
            mod_live_annotated_fields::pyo3_embed_python_source_code(py)?;

            let plain = Plain::new(py, 3)?;
            let _: i64 = plain.value(py)?;
            plain.set_value(py, 5)?;
            assert_eq!(plain.value(py)?, 5);
            plain.set_declared(py, 17)?;
            let _: i64 = plain.declared(py)?;
            assert_eq!(plain.declared(py)?, 17);

            let derived = DerivedPlain::new(py, 19)?;
            let _: i64 = derived.value(py)?;
            derived.set_value(py, 23)?;
            assert_eq!(derived.value(py)?, 23);

            let data = Data::new(py, 7)?;
            let _: i64 = data.score(py)?;
            data.set_score(py, 9)?;
            assert_eq!(data.score(py)?, 9);

            let module = py.import(pyo3::intern!(py, "mod_live_annotated_fields"))?;
            let callable_fields = CallableFields::from_py_any(
                module
                    .getattr(pyo3::intern!(py, "CallableFields"))?
                    .call1((41,))?,
            )?;
            assert_eq!(callable_fields.value(py)?, 41);
            callable_fields.set_value(py, 43)?;
            assert_eq!(callable_fields.value(py)?, 43);
            assert_eq!(
                callable_fields
                    .callback(py)?
                    .call1((1,))?
                    .extract::<i64>()?,
                2,
            );
            let derived_callable_fields = DerivedCallableFields::from_py_any(
                module
                    .getattr(pyo3::intern!(py, "DerivedCallableFields"))?
                    .call1((47,))?,
            )?;
            assert_eq!(derived_callable_fields.value(py)?, 47);
            assert_eq!(
                derived_callable_fields
                    .callback(py)?
                    .call1((2,))?
                    .extract::<i64>()?,
                3,
            );
            let annotated_method = AnnotatedMethod::from_py_any(
                module
                    .getattr(pyo3::intern!(py, "AnnotatedMethod"))?
                    .call0()?,
            )?;
            assert_eq!(annotated_method.method(py, 5)?, 6);
            let class_var_method = ClassVarAnnotatedMethod::from_py_any(
                module
                    .getattr(pyo3::intern!(py, "ClassVarAnnotatedMethod"))?
                    .call0()?,
            )?;
            assert_eq!(class_var_method.work(py, 5)?, 7);
            let inherited_class_var_method = ClassVarMethodOverride::from_py_any(
                module
                    .getattr(pyo3::intern!(py, "ClassVarMethodOverride"))?
                    .call0()?,
            )?;
            assert_eq!(inherited_class_var_method.inherited_work(py, 5)?, 8);
            let pseudo_annotated_method = PseudoAnnotatedMethod::from_py_any(
                module
                    .getattr(pyo3::intern!(py, "PseudoAnnotatedMethod"))?
                    .call0()?,
            )?;
            assert_eq!(pseudo_annotated_method.inherited_runtime(py, 5)?, 9);
            let pseudo_annotated_property = PseudoAnnotatedProperty::from_py_any(
                module
                    .getattr(pyo3::intern!(py, "PseudoAnnotatedProperty"))?
                    .call0()?,
            )?;
            assert_eq!(pseudo_annotated_property.inherited_property(py)?, 9);
            let mutable_frozen_child = MutableFrozenChild::new(py, "base", 2)?;
            assert_eq!(mutable_frozen_child.value(py)?, 2);
            mutable_frozen_child.set_value(py, 3)?;
            assert_eq!(mutable_frozen_child.value(py)?, 3);

            let frozen_slots = FrozenSlots::new(py, 31)?;
            assert_eq!(frozen_slots.locked_slot(py)?, 31);
            let frozen_property = FrozenProperty::new(py, 37)?;
            assert_eq!(frozen_property.value(py)?, 37);

            let plain_init_var = PlainInitVarAnnotation::new(py, 8)?;
            assert_eq!(plain_init_var.temporary(py)?.extract::<i64>()?, 8);
            plain_init_var.set_temporary(py, 10)?;
            assert_eq!(plain_init_var.temporary(py)?.extract::<i64>()?, 10);

            let slotted = Slotted::new(py, 11)?;
            let _: i64 = slotted.amount(py)?;
            slotted.set_amount(py, 13)?;
            assert_eq!(slotted.amount(py)?, 13);

            let descriptor = Descriptor::new(py, "before")?;
            let _: String = descriptor.value(py)?;
            descriptor.set_value(py, "after")?;
            assert_eq!(descriptor.value(py)?, "after");

            let any = Any::from_py_any(
                module
                    .getattr(pyo3::intern!(py, "Any"))?
                    .call0()?,
            )?;
            let shadowed = ShadowedFields::new(py, &any)?;
            let _: Any = shadowed.value(py)?;
            let _: Any = shadowed.alias(py)?;
            shadowed.set_value(py, &any)?;
            shadowed.set_alias(py, &any)?;

            let write_only = WriteOnly::new(py, 31)?;
            write_only.set_value(py, 37)?;
            assert_eq!(
                write_only
                    .bind(py)
                    .getattr(pyo3::intern!(py, "_value"))?
                    .extract::<i64>()?,
                37,
            );

            Ok(())
        }})
    }}
}}
"#
        ),
    )
    .expect("temporary Rust source must be writable");

    let shared_target_directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find(|directory| directory.join("Cargo.lock").is_file())
        .expect("engine crate must belong to a locked workspace")
        .join("target/property-safety-runtime");
    let mut command =
        std::process::Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned()));
    command
        .args(["test", "--quiet", "--offline"])
        .current_dir(&crate_dir)
        .env("CARGO_TARGET_DIR", shared_target_directory);
    let output = working_directory
        .run(&mut command, std::time::Duration::from_secs(900))
        .unwrap_or_else(|error| {
            panic!(
                "generated annotation bindings command failed: {error}\nstdout:\n{}\nstderr:\n{}\n\n--- generated lib.rs ---\n{}",
                error.stdout(),
                error.stderr(),
                generated,
            )
        });
    assert!(
        output.status.success(),
        "generated annotation bindings failed compile/runtime smoke\nstatus: {}\nstdout:\n{}\nstderr:\n{}\n\n--- generated lib.rs ---\n{generated}",
        output.status,
        output.stdout,
        output.stderr,
    );
}

#[test]
fn live_class_annotations_generate_typed_runtime_fields_safely() {
    let generated = pyo3_bindgen_engine::Codegen::default()
        .module_from_str(LIVE_ANNOTATED_FIELDS_SOURCE, "mod_live_annotated_fields")
        .expect("annotated source must parse without evaluating its annotations")
        .generate_with_report()
        .expect("malformed annotations must fail soft");
    let diagnostic_codes = generated
        .report
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code.as_str())
        .collect::<Vec<_>>();
    assert!(
        diagnostic_codes.contains(&"class_annotation_parse_fallback"),
        "malformed annotation must produce a fallback diagnostic: {diagnostic_codes:?}"
    );
    assert!(
        diagnostic_codes.contains(&"class_annotations_unreadable"),
        "hostile annotation mapping must produce a fail-soft diagnostic: {diagnostic_codes:?}"
    );
    let source = prettyplease::unparse(
        &syn::parse2(generated.tokens).expect("generated bindings must remain valid Rust syntax"),
    );
    let shadowed_methods = source
        .split("pub trait ShadowedFieldsMethods")
        .nth(1)
        .and_then(|rest| rest.split("impl ShadowedFieldsMethods").next())
        .expect("ShadowedFieldsMethods trait must be generated");
    for signature in ["PyResult<Any>", "p_value: &Any"] {
        assert!(
            shadowed_methods.contains(signature),
            "postponed class/property annotation must resolve local `Any`: missing `{signature}` in\n{shadowed_methods}"
        );
    }
    let write_only_methods = source
        .split("pub trait WriteOnlyMethods")
        .nth(1)
        .and_then(|rest| rest.split("impl WriteOnlyMethods").next())
        .expect("WriteOnlyMethods trait must be generated");
    assert!(
        write_only_methods.contains("fn set_value<'py>(")
            && !write_only_methods.contains("fn value<'py>("),
        "write-only Python property must expose only its setter:\n{write_only_methods}"
    );
    let callable_field_methods = source
        .split("pub trait CallableFieldsMethods")
        .nth(1)
        .and_then(|rest| rest.split("impl CallableFieldsMethods").next())
        .expect("CallableFieldsMethods trait must be generated");
    for field in ["value", "callback"] {
        assert!(
            callable_field_methods.contains(&format!("fn {field}<'py>("))
                && callable_field_methods.contains(&format!("fn set_{field}<'py>(")),
            "annotated dataclass field `{field}` was misclassified as its callable \
             class/default value:\n{callable_field_methods}"
        );
    }
    let derived_callable_field_methods = source
        .split("pub trait DerivedCallableFieldsMethods")
        .nth(1)
        .and_then(|rest| rest.split("impl DerivedCallableFieldsMethods").next())
        .expect("DerivedCallableFieldsMethods trait must be generated");
    assert!(
        derived_callable_field_methods.contains("fn callback<'py>(")
            && derived_callable_field_methods.contains("fn set_callback<'py>("),
        "an inherited callable-valued field must remain a property on a flat subclass:\n\
         {derived_callable_field_methods}"
    );
    let overridden_callable_methods = source
        .split("pub trait OverriddenCallableFieldMethods")
        .nth(1)
        .and_then(|rest| rest.split("impl OverriddenCallableFieldMethods").next())
        .expect("OverriddenCallableFieldMethods trait must be generated");
    assert!(
        overridden_callable_methods.contains("fn callback<'py>(")
            && !overridden_callable_methods.contains("fn set_callback<'py>("),
        "a nearer subclass method must override an inherited field annotation:\n\
        {overridden_callable_methods}"
    );
    for (class_name, method_name) in [
        ("AnnotatedMethod", "method"),
        ("ClassVarAnnotatedMethod", "work"),
        ("ClassVarMethodOverride", "inherited_work"),
        ("PseudoAnnotatedMethod", "inherited_runtime"),
    ] {
        let methods = source
            .split(&format!("pub trait {class_name}Methods"))
            .nth(1)
            .and_then(|rest| rest.split(&format!("impl {class_name}Methods")).next())
            .unwrap_or_else(|| panic!("{class_name}Methods trait must be generated"));
        assert!(
            methods.contains(&format!("fn {method_name}<'py>("))
                && !methods.contains(&format!("fn set_{method_name}<'py>(")),
            "a runtime method was suppressed or converted into an annotated field:\n{methods}"
        );
    }
    let pseudo_annotated_property_methods = source
        .split("pub trait PseudoAnnotatedPropertyMethods")
        .nth(1)
        .and_then(|rest| rest.split("impl PseudoAnnotatedPropertyMethods").next())
        .expect("PseudoAnnotatedPropertyMethods trait must be generated");
    assert!(
        pseudo_annotated_property_methods.contains("fn inherited_property<'py>(")
            && !pseudo_annotated_property_methods.contains("fn set_inherited_property<'py>("),
        "a nearer pseudo annotation must not hide an inherited runtime property:\n\
         {pseudo_annotated_property_methods}"
    );
    let pseudo_field_methods = source
        .split("pub trait PseudoFieldsMethods")
        .nth(1)
        .and_then(|rest| rest.split("impl PseudoFieldsMethods").next())
        .expect("PseudoFieldsMethods trait must be generated");
    for omitted in ["separator", "temporary"] {
        assert!(
            !pseudo_field_methods.contains(&format!("fn {omitted}<'py>("))
                && !pseudo_field_methods.contains(&format!("fn set_{omitted}<'py>(")),
            "dataclass pseudo-field `{omitted}` leaked into instance API:\n{pseudo_field_methods}"
        );
    }
    let aliased_pseudo_field_methods = source
        .split("pub trait AliasedPseudoFieldsMethods")
        .nth(1)
        .and_then(|rest| rest.split("impl AliasedPseudoFieldsMethods").next())
        .expect("AliasedPseudoFieldsMethods trait must be generated");
    assert!(
        !aliased_pseudo_field_methods.contains("fn temporary<'py>(")
            && !aliased_pseudo_field_methods.contains("fn set_temporary<'py>("),
        "a parameterized InitVar alias leaked into the instance API:\n\
         {aliased_pseudo_field_methods}"
    );
    let mutable_frozen_child_methods = source
        .split("pub trait MutableFrozenChildMethods")
        .nth(1)
        .and_then(|rest| rest.split("impl MutableFrozenChildMethods").next())
        .expect("MutableFrozenChildMethods trait must be generated");
    assert!(
        mutable_frozen_child_methods.contains("fn set_value<'py>(")
            && !mutable_frozen_child_methods.contains("fn set_base<'py>("),
        "a non-dataclass subclass field must remain writable without making an inherited \
         frozen field writable when the subclass reannotates its name:\n\
         {mutable_frozen_child_methods}"
    );
    for class_name in ["FrozenSlots", "FrozenProperty"] {
        let methods = source
            .split(&format!("pub trait {class_name}Methods"))
            .nth(1)
            .and_then(|rest| rest.split(&format!("impl {class_name}Methods")).next())
            .unwrap_or_else(|| panic!("{class_name}Methods trait must be generated"));
        assert!(
            !methods.contains("fn set_locked_slot<'py>(")
                && !methods.contains("fn set_value<'py>("),
            "a setter blocked by an exact frozen dataclass leaked from `{class_name}`:\n\
             {methods}"
        );
    }
    let mutable_frozen_pair_methods = source
        .split("pub trait MutableFrozenPairMethods")
        .nth(1)
        .and_then(|rest| rest.split("impl MutableFrozenPairMethods").next())
        .expect("MutableFrozenPairMethods trait must be generated");
    assert!(
        mutable_frozen_pair_methods.contains("fn set_fresh<'py>(")
            && !mutable_frozen_pair_methods.contains("fn set_right<'py>("),
        "all frozen setters in a multiple-inheritance chain must reject their own field \
         names while permitting a new subclass field:\n{mutable_frozen_pair_methods}"
    );
    let plain_init_var_methods = source
        .split("pub trait PlainInitVarAnnotationMethods")
        .nth(1)
        .and_then(|rest| rest.split("impl PlainInitVarAnnotationMethods").next())
        .expect("PlainInitVarAnnotationMethods trait must be generated");
    assert!(
        plain_init_var_methods.contains("fn temporary<'py>(")
            && plain_init_var_methods.contains("fn set_temporary<'py>("),
        "InitVar is a pseudo-field only on dataclasses:\n{plain_init_var_methods}"
    );

    for omitted in [
        "runtime_class_var",
        "postponed_class_var",
        "aliased_class_var",
        "parameterized_alias_class_var",
        "_private_value",
        "test",
    ] {
        assert!(
            !source.contains(&format!("fn {omitted}<'py>("))
                && !source.contains(&format!("fn set_{omitted}<'py>(")),
            "filtered or ClassVar annotation leaked into instance API as `{omitted}`:\n{source}"
        );
    }
    let flat_tokens = pyo3_bindgen_engine::Codegen::new(pyo3_bindgen_engine::Config {
        deduplicate_inherited_members: false,
        generate_provenance: false,
        ..Default::default()
    })
    .module_from_str(
        LIVE_ANNOTATED_FIELDS_SOURCE,
        "mod_live_annotated_fields_flat",
    )
    .expect("annotated source must parse under the flat inheritance model")
    .generate()
    .expect("flat inheritance generation must remain fail-soft");
    let flat_source = prettyplease::unparse(
        &syn::parse2(flat_tokens).expect("flat generated bindings must remain valid Rust syntax"),
    );
    for class_var in ["runtime_class_var", "postponed_class_var"] {
        assert!(
            !flat_source.contains(&format!("fn {class_var}"))
                && !flat_source.contains(&format!("fn set_{class_var}")),
            "inherited ClassVar leaked under flat generation as `{class_var}`:\n{flat_source}"
        );
    }
    let flat_derived_methods = flat_source
        .split("pub trait DerivedPlainMethods")
        .nth(1)
        .and_then(|rest| rest.split("impl DerivedPlainMethods").next())
        .expect("flat DerivedPlainMethods trait must be generated");
    for (class_name, member_name) in [
        ("PseudoAnnotatedMethod", "inherited_runtime"),
        ("PseudoAnnotatedProperty", "inherited_property"),
    ] {
        let methods = flat_source
            .split(&format!("pub trait {class_name}Methods"))
            .nth(1)
            .and_then(|rest| rest.split(&format!("impl {class_name}Methods")).next())
            .unwrap_or_else(|| panic!("flat {class_name}Methods trait must be generated"));
        assert!(
            methods.contains(&format!("fn {member_name}<'py>(")),
            "a pseudo annotation must not hide inherited runtime member `{member_name}` \
             under flat generation:\n{methods}"
        );
    }
    assert!(
        flat_derived_methods.contains("fn value<'py>(")
            && flat_derived_methods.contains("fn set_value<'py>("),
        "flat subclasses must retain inherited annotation-only fields:\n{flat_derived_methods}"
    );
    assert!(
        source.contains("fn broken<'py>("),
        "malformed field annotation must fall back without dropping its field:\n{source}"
    );
    for readonly in ["days", "blocked", "delete_only", "locked", "denied"] {
        assert!(
            !source.contains(&format!("fn set_{readonly}<'py>(")),
            "an annotation made readonly field `{readonly}` writable:\n{source}"
        );
    }
    assert_generated_annotation_bindings_run(&source);
}
