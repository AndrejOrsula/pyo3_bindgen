use super::{
    AttributeVariant, Forwarder, FunctionImplementation, Ident, ImportMode, Path, TraitMethod,
};
use crate::{planning::Type, report::DomainDiagnostic, Config, Result};
use itertools::Itertools;
use proc_macro2::TokenStream;
use pyo3::{
    exceptions::PyAttributeError,
    prelude::*,
    types::{
        PyDict, PyDictMethods, PyFunction, PyModule, PyModuleMethods, PyString, PyTuple, PyType,
    },
    PyTypeInfo,
};
use std::str::FromStr;

const IR_FORCED_CUSTOM_PREFIX: &str = "\0pyo3_bindgen.ir_custom\0";

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize)]
pub(crate) struct Function {
    pub(crate) name: Path,
    pub(crate) typ: FunctionType,
    parameters: Vec<Parameter>,
    return_annotation: Type,
    docstring: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize)]
pub(crate) enum FunctionType {
    Function,
    Method { class_path: Path, typ: MethodType },
    Closure,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize)]
pub(crate) enum MethodType {
    InstanceMethod,
    ClassMethod,
    StaticMethod,
    Constructor,
    Callable,
    Unknown,
}

/// The safe suffix for a parameter's `p_`-prefixed local Rust binding: the
/// parameter's Rust-side spelling (`Ident::as_rs`) with any `r#` keyword-escape
/// prefix stripped. Always derived from `as_rs()`, never `as_py()`, so a
/// diverging Python-name override (see `Ident::with_rs_override`, used for a
/// keyword-only parameter literally named `_`) cannot leak into -- or break the
/// collision-avoidance uniqueness of -- the generated local variable name.
fn param_binding_suffix(name: &Ident) -> &str {
    name.as_rs().strip_prefix("r#").unwrap_or(name.as_rs())
}

impl Function {
    pub(crate) fn resource_shape(&self) -> crate::resource::StructuralResources {
        let (return_nodes, return_depth) = self.return_annotation.resource_shape();
        let mut resources = crate::resource::StructuralResources {
            definitions: 1,
            recursion_depth: return_depth,
            model_nodes: 1 + return_nodes,
        };
        for parameter in &self.parameters {
            let (nodes, depth) = parameter.annotation.resource_shape();
            resources.model_nodes += 1 + nodes;
            resources.recursion_depth = resources.recursion_depth.max(depth);
        }
        resources
    }

    /// Python object path resolved when this callable is emitted.
    pub(crate) fn import_path(&self) -> Path {
        match &self.typ {
            FunctionType::Function | FunctionType::Closure => {
                self.name.parent().unwrap_or_else(|| unreachable!())
            }
            FunctionType::Method { class_path, .. } => class_path.clone(),
        }
    }

    /// Rough byte size this method contributes when re-emitted as a trait method under the
    /// flat model: a fixed base for the signature and body plus its docstring, which
    /// dominates the variance (a documented method is many times larger than a trivial
    /// one). The deduplication cost model sums this over the members a subclass would
    /// otherwise redeclare, so a hierarchy of heavily-documented methods correctly reads
    /// as a large saving while a handful of trivial accessors does not.
    pub(crate) fn estimated_flat_size(&self, cfg: &Config) -> i64 {
        // A method emits nothing under the flat model when class-method generation is
        // disabled, so deduplicating it saves nothing.
        if !cfg.generate_functions {
            return 0;
        }
        const BASE: i64 = 200;
        // Each parameter widens the signature and adds body preprocessing (tuple/dict
        // packing) -- material for wide APIs like `argparse`'s `add_argument`.
        const PER_PARAM: i64 = 90;
        BASE + PER_PARAM * self.parameters.len() as i64
            + crate::utils::text::emitted_docstring_len(cfg, self.docstring.as_deref())
    }

    pub(crate) fn parse(
        _cfg: &Config,
        function: &pyo3::Bound<pyo3::types::PyAny>,
        name: Path,
        mut typ: FunctionType,
        diagnostics: &mut Vec<DomainDiagnostic>,
    ) -> Result<Self> {
        let py = function.py();

        // Extract the docstring of the function
        let docstring = {
            let docstring = function.getattr(pyo3::intern!(py, "__doc__"))?.to_string();
            if docstring.is_empty() || docstring == "None" {
                None
            } else {
                Some(docstring)
            }
        };

        let inspect = crate::runtime_sentinels::inspect_module(py)?;
        let signature_chain = Self::signature_annotation_chain(&inspect, function, false);

        // Method kind is descriptor metadata, independent of whether Python can expose a
        // call signature. Resolve it first so a signature-less C descriptor still emits
        // the correct instance/static/class API instead of remaining `Unknown` and being
        // dropped during generation.
        let inferred_live_instance_method =
            Self::infer_live_method_type(py, &inspect, function, &name, &mut typ)?;

        // Calling an async function returns a coroutine/async-generator object, not the
        // function's annotated awaited/yield type. Keep that live object as `PyAny`; a
        // caller can hand it to an event loop or iterate it asynchronously.
        let returns_async_handle = Self::signature_returns_async_handle(&inspect, function)?;

        // Extract the signature of the function
        if let Ok(function_signature) = Self::inspect_signature(&inspect, function) {
            // Extract the parameters of the function
            let mut parameters = function_signature
                .getattr(pyo3::intern!(py, "parameters"))?
                .call_method0(pyo3::intern!(py, "values"))?
                .try_iter()?
                .map(|param| {
                    let param = param?;

                    // The parameter name is taken verbatim, with no normalization and no
                    // duplicate check, because `inspect` is only NEARLY a guarantee of
                    // both: a source-defined parameter is NFKC by construction (CPython
                    // normalizes identifiers at compile time, PEP 3131), so two source
                    // parameters can never differ by normalization alone -- but a
                    // hand-assigned `__signature__` can carry such a pair, since
                    // `inspect.Parameter` validates its name with `str.isidentifier()`
                    // alone, which does not normalize. The canonical-binding pass at the
                    // end of `finalize_parameters` is the backstop for that case.
                    let parameter_name = Ident::try_from_py(
                        &param.getattr(pyo3::intern!(py, "name"))?.to_string(),
                        || format!("function '{name}' has a parameter with an empty name"),
                    )?;
                    let kind = ParameterKind::try_from(
                        param.getattr(pyo3::intern!(py, "kind"))?.extract::<u8>()?,
                    )?;
                    let annotation = match kind {
                        ParameterKind::VarPositional => Type::PyTuple(vec![Type::Unknown]),
                        ParameterKind::VarKeyword => Type::Optional(Box::new(Type::PyDict {
                            key_type: Box::new(Type::Unknown),
                            value_type: Box::new(Type::Unknown),
                        })),
                        _ => {
                            let annotation = param.getattr(pyo3::intern!(py, "annotation"))?;
                            if annotation.is(&param.getattr(pyo3::intern!(py, "empty"))?) {
                                Type::Unknown
                            } else {
                                Self::parse_signature_annotation_or_unknown(
                                    annotation,
                                    signature_chain.as_deref(),
                                    diagnostics,
                                    &name,
                                    format!(
                                        "Parameter '{}' annotation of function '{}'",
                                        parameter_name.as_py(),
                                        name
                                    ),
                                )
                            }
                        }
                    };

                    let has_default = {
                        let default = param.getattr(pyo3::intern!(py, "default"))?;
                        !default.is(&param.getattr(pyo3::intern!(py, "empty"))?)
                    };

                    Result::Ok(Parameter {
                        name: parameter_name,
                        kind,
                        annotation,
                        has_default,
                    })
                })
                .collect::<Result<Vec<_>>>()?;

            // Extract the return annotation of the function
            let return_annotation = if returns_async_handle {
                Type::PyAny
            } else {
                let return_annotation =
                    function_signature.getattr(pyo3::intern!(py, "return_annotation"))?;
                if return_annotation.is(&function_signature.getattr(pyo3::intern!(py, "empty"))?) {
                    Type::Unknown
                } else {
                    Self::parse_signature_annotation_or_unknown(
                        return_annotation,
                        signature_chain.as_deref(),
                        diagnostics,
                        &name,
                        format!("Return annotation of function '{name}'"),
                    )
                }
            };

            // A raw ordinary-function descriptor normally proves an instance method, but a
            // non-positional first parameter cannot be stripped as its receiver: `*self`
            // and `**self` are caller-visible variadics. Fall back to the same static shape
            // used by unresolved external IR so finalization preserves the whole parameter.
            if inferred_live_instance_method
                && parameters.first().is_some_and(|parameter| {
                    !matches!(
                        parameter.kind,
                        ParameterKind::PositionalOnly | ParameterKind::PositionalOrKeyword
                    )
                })
            {
                let class_path = match &typ {
                    FunctionType::Method { class_path, .. } => class_path.clone(),
                    _ => unreachable!("live instance inference always produces a method"),
                };
                typ = FunctionType::Method {
                    class_path,
                    typ: MethodType::StaticMethod,
                };
            }

            // Run the shared post-extraction parameter finalize/validate logic
            // (self/cls stripping, '_' filtering, non-first-'self' handling). This is
            // factored so the IR path (`from_ir`) produces byte-identical output.
            Self::finalize_parameters(&mut parameters, &mut typ, &name, diagnostics);

            Ok(Self {
                name,
                typ,
                parameters,
                return_annotation,
                docstring,
            })
        } else {
            Ok(Self {
                name,
                typ,
                // The generated fallback invokes a receiver/class-bound dispatcher.
                // These synthetic wildcards therefore model only caller-visible
                // arguments, after any implicit `self`/`cls`; do not run
                // receiver-stripping finalization over them.
                parameters: Self::variadic_fallback_parameters(),
                return_annotation: Type::Unknown,
                docstring,
            })
        }
    }

    /// Inspect a callable with Python 3.14+ deferred annotations in public STRING format.
    ///
    /// PEP 649 makes `inspect.signature`'s default VALUE format execute annotation
    /// expressions. STRING preserves the exact callable shape and gives our parser textual
    /// annotations without evaluating the captured strings. The public API may still execute
    /// annotation metadata while producing those strings, so STRING is not a no-evaluation
    /// guarantee.
    pub(crate) fn inspect_signature<'py>(
        _inspect: &pyo3::Bound<'py, pyo3::types::PyModule>,
        callable: &pyo3::Bound<'py, pyo3::types::PyAny>,
    ) -> PyResult<pyo3::Bound<'py, pyo3::types::PyAny>> {
        let py = callable.py();
        if py.version_info() >= (3, 14) {
            let kwargs = pyo3::types::PyDict::new(py);
            kwargs.set_item(
                pyo3::intern!(py, "annotation_format"),
                Self::annotationlib_string_format(py)?,
            )?;
            crate::runtime_sentinels::inspect_signature(py)?.call((callable,), Some(&kwargs))
        } else {
            crate::runtime_sentinels::inspect_signature(py)?.call1((callable,))
        }
    }

    pub(crate) fn annotationlib_string_format(
        py: Python<'_>,
    ) -> PyResult<pyo3::Bound<'_, pyo3::types::PyAny>> {
        crate::runtime_sentinels::attr(
            py,
            crate::runtime_sentinels::StdlibModule::Annotationlib,
            "Format",
        )?
        .getattr(pyo3::intern!(py, "STRING"))
    }

    /// Infer an unresolved live method from its raw class descriptor.
    ///
    /// This deliberately runs before `inspect.signature`: many portable C descriptors
    /// (`dict.update`, `str.maketrans`, ...) have no inspectable signature, but their
    /// descriptor kind remains authoritative.
    fn infer_live_method_type(
        py: Python<'_>,
        _inspect: &Bound<'_, pyo3::types::PyModule>,
        function: &Bound<'_, PyAny>,
        name: &Path,
        typ: &mut FunctionType,
    ) -> Result<bool> {
        let class_path = match &*typ {
            FunctionType::Method {
                class_path,
                typ: MethodType::Unknown,
            } => class_path.clone(),
            _ => return Ok(false),
        };

        // Resolve the owning class, then inspect its raw descriptor without invoking
        // `__get__`. Importing only the path root gives the module, not the class, and
        // loses static/class method evidence.
        let owner_and_raw = class_path
            .parent()
            .and_then(|module_path| py.import(module_path.to_py().as_str()).ok())
            .and_then(|module| module.getattr(class_path.name().as_py()).ok())
            .and_then(|class| {
                crate::runtime_sentinels::inspect_getattr_static(py)
                    .ok()?
                    .call1((&class, name.name().as_py()))
                    .ok()
                    .map(|raw| (class, raw))
            });

        let mut inferred_instance_method = false;
        if let Some((owner, raw)) = owner_and_raw {
            // Infer from actual class-access binding rather than mutable
            // `builtins.classmethod`/`staticmethod` spellings or subclass checks.
            // Ordinary instance descriptors are returned unchanged. Class-bound
            // descriptors expose the exact owner as `__self__`; remaining changed
            // values are static results.
            let method_type = if raw.is(function) {
                MethodType::InstanceMethod
            } else if function
                .getattr(pyo3::intern!(py, "__self__"))
                .is_ok_and(|bound_self| bound_self.is(&owner))
            {
                MethodType::ClassMethod
            } else {
                MethodType::StaticMethod
            };
            inferred_instance_method = method_type == MethodType::InstanceMethod;
            *typ = FunctionType::Method {
                class_path,
                typ: method_type,
            };
        }

        Ok(inferred_instance_method)
    }

    fn variadic_fallback_parameters() -> Vec<Parameter> {
        vec![
            Parameter {
                name: Ident::from_rs("args"),
                kind: ParameterKind::VarPositional,
                annotation: Type::PyTuple(vec![Type::Unknown]),
                has_default: false,
            },
            Parameter {
                name: Ident::from_rs("kwargs"),
                kind: ParameterKind::VarKeyword,
                annotation: Type::Optional(Box::new(Type::PyDict {
                    key_type: Box::new(Type::Unknown),
                    value_type: Box::new(Type::Unknown),
                })),
                has_default: false,
            },
        ]
    }

    pub(crate) fn exact_bound_method_function<'py>(
        callable: &Bound<'py, pyo3::types::PyAny>,
    ) -> Option<Bound<'py, PyFunction>> {
        let py = callable.py();
        let function = callable
            .getattr(pyo3::intern!(py, "__func__"))
            .ok()?
            .cast_into::<PyFunction>()
            .ok()?;
        let bound_self = callable.getattr(pyo3::intern!(py, "__self__")).ok()?;
        let owner = bound_self
            .clone()
            .cast_into::<PyType>()
            .unwrap_or_else(|_| bound_self.get_type());
        let canonical_method = function
            .call_method1(pyo3::intern!(py, "__get__"), (&bound_self, &owner))
            .ok()?;
        callable
            .get_type()
            .as_any()
            .is(canonical_method.get_type().as_any())
            .then_some(function)
    }

    /// Obtain the exact globals dictionary for a Python function or bound method.
    ///
    /// Both `__globals__` and `__func__` are writable names on arbitrary callable
    /// objects. Only an exact Python function, or the exact Python function owned by
    /// a genuine bound method, may authorize a globals dictionary for postponed
    /// annotation resolution.
    pub(crate) fn callable_globals<'py>(
        function: &Bound<'py, pyo3::types::PyAny>,
    ) -> Option<Bound<'py, PyDict>> {
        let py = function.py();
        let function = function
            .clone()
            .cast_into::<PyFunction>()
            .ok()
            .or_else(|| Self::exact_bound_method_function(function))?;
        function
            .getattr(pyo3::intern!(py, "__globals__"))
            .ok()
            .and_then(|globals| globals.cast_into::<PyDict>().ok())
    }

    /// Resolve the function wrapped by CPython's unbound `partialmethod` helper.
    ///
    /// Python 3.14 exposes the descriptor as `__partialmethod__`; older supported
    /// versions use the private `_partialmethod` spelling. Both objects expose the
    /// annotation and async source through their `func` attribute.
    fn partialmethod_source<'py>(
        callable: &Bound<'py, pyo3::types::PyAny>,
    ) -> Option<Bound<'py, pyo3::types::PyAny>> {
        let py = callable.py();
        let exact_source = |metadata_name: &str| {
            let metadata = callable.getattr(metadata_name).ok()?;
            if !AttributeVariant::is_exact_functools_partialmethod(py, &metadata).ok()? {
                return None;
            }
            metadata.getattr(pyo3::intern!(py, "func")).ok()
        };
        exact_source("__partialmethod__").or_else(|| exact_source("_partialmethod"))
    }

    /// Source callable of an exact `functools.partial`.
    ///
    /// Keep the type-identity check shared between signature-global and async-kind
    /// traversal. In particular, Python 3.8 recognizes a partial of a plain coroutine
    /// function but not a partial of a bound coroutine classmethod; following `.func`
    /// supplies the portable classification without treating arbitrary callable objects
    /// with a user-defined `func` attribute as transparent wrappers.
    fn functools_partial_source<'py>(
        callable: &Bound<'py, pyo3::types::PyAny>,
    ) -> Option<Bound<'py, pyo3::types::PyAny>> {
        let py = callable.py();
        AttributeVariant::is_exact_functools_partial(py, callable)
            .ok()?
            .then(|| callable.getattr(pyo3::intern!(py, "func")).ok())
            .flatten()
    }

    fn is_exact_python_function(callable: &Bound<'_, PyAny>) -> bool {
        callable
            .get_type()
            .as_any()
            .is(PyFunction::type_object(callable.py()).as_any())
    }

    /// Invoke immutable builtin `type.__getattribute__`, bypassing a custom
    /// metaclass hook and any mutable spelling of `builtins.type`.
    fn canonical_type_getattribute<'py>(
        class: &Bound<'py, PyType>,
        name: &str,
    ) -> PyResult<Bound<'py, PyAny>> {
        static TYPE_GETATTRIBUTE: pyo3::sync::PyOnceLock<Py<PyAny>> = pyo3::sync::PyOnceLock::new();

        let py = class.py();
        let type_getattribute = TYPE_GETATTRIBUTE
            .get_or_try_init(py, || {
                PyType::new::<PyType>(py)
                    .getattr(pyo3::intern!(py, "__getattribute__"))
                    .map(Bound::unbind)
            })?
            .bind(py);
        type_getattribute.call1((class, name))
    }

    /// Find an unbound attribute exactly where Python special-method lookup does:
    /// the first raw class dictionary in the real type MRO.
    fn raw_type_mro_attribute<'py>(
        class: &Bound<'py, PyType>,
        name: &str,
    ) -> PyResult<Option<Bound<'py, PyAny>>> {
        let mro = Self::canonical_type_getattribute(class, "__mro__")?.cast_into::<PyTuple>()?;
        for base in mro.iter() {
            let base = base.cast_into::<PyType>()?;
            let dictionary = Self::canonical_type_getattribute(&base, "__dict__")?;
            if dictionary.contains(name)? {
                return dictionary.get_item(name).map(Some);
            }
        }
        Ok(None)
    }

    /// Whether ordinary instance metadata reads are stable under the held GIL.
    ///
    /// A custom `__getattribute__` or `__getattr__` can return a different
    /// `__wrapped__`/`__signature__` object on each access, producing a signature
    /// and globals source from different callables.
    fn has_default_instance_attribute_lookup(callable: &Bound<'_, PyAny>) -> bool {
        let py = callable.py();
        let owner = callable.get_type();
        let object_type = PyType::new::<PyAny>(py);
        let actual = Self::raw_type_mro_attribute(&owner, "__getattribute__")
            .ok()
            .flatten();
        let canonical = Self::raw_type_mro_attribute(&object_type, "__getattribute__")
            .ok()
            .flatten();
        actual
            .zip(canonical)
            .is_some_and(|(actual, canonical)| actual.is(&canonical))
            && Self::raw_type_mro_attribute(&owner, "__getattr__")
                .is_ok_and(|value| value.is_none())
    }

    /// Callable-object target whose signature provenance is stable enough to
    /// authorize postponed-annotation globals.
    fn trusted_signature_call_target<'py>(
        callable: &Bound<'py, PyAny>,
    ) -> Option<Bound<'py, PyAny>> {
        let py = callable.py();
        let owner = callable.get_type();
        let raw_call = Self::raw_type_mro_attribute(&owner, "__call__")
            .ok()
            .flatten()?;
        if !Self::is_exact_python_function(&raw_call) {
            return None;
        }

        if py.version_info() >= (3, 11) {
            Self::type_owned_call(callable).ok().flatten()
        } else if owner
            .get_type()
            .as_any()
            .is(PyType::new::<PyType>(py).as_any())
        {
            owner.getattr(pyo3::intern!(py, "__call__")).ok()
        } else {
            None
        }
    }

    /// Canonical instance-bound `__call__` used by Python special-method dispatch.
    ///
    /// Class access binds a custom descriptor with `instance=None`, which can return
    /// a different callable from the one used at runtime. Find the raw descriptor in
    /// the real MRO, then invoke the descriptor type's exact `__get__` with the
    /// callable instance. Looking up `__get__` through builtin `type` also bypasses a
    /// descriptor object's custom `__getattribute__`.
    fn type_owned_call<'py>(
        callable: &Bound<'py, pyo3::types::PyAny>,
    ) -> PyResult<Option<Bound<'py, pyo3::types::PyAny>>> {
        let py = callable.py();
        let owner = callable.get_type();
        let Some(raw_call) = Self::raw_type_mro_attribute(&owner, "__call__")? else {
            return Ok(None);
        };
        let descriptor_get =
            match Self::canonical_type_getattribute(&raw_call.get_type(), "__get__") {
                Ok(descriptor_get) => descriptor_get,
                Err(error) if error.is_instance_of::<PyAttributeError>(py) => {
                    return Ok(Some(raw_call));
                }
                Err(error) => return Err(error),
            };
        descriptor_get
            .call1((&raw_call, callable, &owner))
            .map(Some)
    }

    /// Follow the ordinary `__wrapped__` chain used by `inspect.signature`, stopping
    /// at a callable with an explicit `__signature__`.
    ///
    /// `functools.wraps` can place a wrapper and its copied string annotations in a
    /// different module from the wrapped function. `inspect.signature` reports the
    /// wrapped signature, so postponed annotations must use that same function's
    /// globals rather than the outer wrapper's globals.
    pub(crate) fn signature_annotation_chain<'py>(
        _inspect: &Bound<'py, PyModule>,
        function: &Bound<'py, pyo3::types::PyAny>,
        project_callable_object: bool,
    ) -> Option<Vec<Bound<'py, pyo3::types::PyAny>>> {
        let py = function.py();
        let mut current = Self::exact_bound_method_function(function)
            .map(Bound::into_any)
            .unwrap_or_else(|| function.clone());
        let mut may_project_call = project_callable_object;
        let mut seen = std::collections::HashSet::new();
        let mut chain = Vec::new();
        const MAX_WRAPPER_DEPTH: usize = 256;
        for _ in 0..MAX_WRAPPER_DEPTH {
            if let Some(function) = Self::exact_bound_method_function(&current) {
                current = function.into_any();
            }
            if !seen.insert((current.as_ptr(), may_project_call)) {
                return None;
            }
            chain.push(current.clone());

            let stable_wrapper_metadata = Self::is_exact_python_function(&current)
                || AttributeVariant::is_exact_functools_partial(py, &current).unwrap_or(false);
            if !stable_wrapper_metadata && !Self::has_default_instance_attribute_lookup(&current) {
                return None;
            }

            // A non-None explicit Signature has annotation objects but no
            // trustworthy globals provenance. Even a non-string ForwardRef (or
            // nested generic containing one) can resolve against the wrong module,
            // so all such annotations degrade. Explicit None disables only
            // `__wrapped__`; inspect then continues natural partial/callable
            // dispatch.
            let allow_wrapped = match current.getattr(pyo3::intern!(py, "__signature__")) {
                Ok(signature) if !signature.is_none() => return None,
                Ok(_) => false,
                Err(error) if error.is_instance_of::<PyAttributeError>(py) => true,
                Err(_) => return None,
            };
            if allow_wrapped {
                if let Ok(wrapped) = current.getattr(pyo3::intern!(py, "__wrapped__")) {
                    if !stable_wrapper_metadata {
                        return None;
                    }
                    current = wrapped;
                    may_project_call = true;
                    continue;
                }
            }

            // `inspect.signature` exposes the wrapped callable's annotations for an
            // unbound `functools.partialmethod` helper, but CPython stores that source
            // on partialmethod metadata rather than `__wrapped__`.
            if let Some(func) = Self::partialmethod_source(&current) {
                if !stable_wrapper_metadata {
                    return None;
                }
                current = func;
                may_project_call = true;
                continue;
            }

            // Exact `functools.partial` objects likewise expose their `.func`
            // signature. Match the same exact runtime metadata used by attribute
            // classification rather than following an arbitrary callable's `func`
            // attribute.
            if let Some(func) = Self::functools_partial_source(&current) {
                current = func;
                may_project_call = true;
                continue;
            }

            if may_project_call
                && !crate::runtime_sentinels::inspect_isroutine(py)
                    .and_then(|isroutine| isroutine.call1((&current,)))
                    .and_then(|result| result.is_truthy())
                    .unwrap_or(true)
            {
                if let Some(type_call) = Self::trusted_signature_call_target(&current) {
                    current = type_call;
                    may_project_call = true;
                    continue;
                }
                return None;
            }

            return Some(chain);
        }

        // A dynamic `__wrapped__` can manufacture a fresh object forever. With
        // no trustworthy terminal callable, expose no globals rather than resolve
        // annotations in an arbitrary intermediate namespace.
        None
    }

    /// Globals used by the terminal callable whose signature `inspect.signature` exposes.
    pub(crate) fn signature_annotation_globals<'py>(
        signature_chain: &[Bound<'py, pyo3::types::PyAny>],
    ) -> Option<Bound<'py, PyDict>> {
        signature_chain.last().and_then(Self::callable_globals)
    }

    /// Whether calling the callable, or an ordinary wrapper in its `__wrapped__`
    /// chain, returns an async handle instead of the annotated awaited/yield value.
    ///
    /// This traversal is deliberately separate from
    /// [`Self::signature_annotation_chain`]. An explicit `__signature__` stops
    /// `inspect.signature` from unwrapping annotation metadata, but it does not change
    /// what a transparent wrapper returns. Continue through `__wrapped__` here so a
    /// synchronous `functools.wraps(async_fn)` proxy cannot advertise the async
    /// function's awaited annotation as its immediate Rust return type.
    pub(crate) fn signature_returns_async_handle(
        inspect: &Bound<'_, PyModule>,
        function: &Bound<'_, pyo3::types::PyAny>,
    ) -> PyResult<bool> {
        Self::callable_graph_returns_async_handle(inspect, function, false)
    }

    /// Property-accessor async classification, including callable objects.
    ///
    /// `inspect.iscoroutinefunction(instance)` is false even when the special method
    /// which calling that instance dispatches (`type(instance).__call__`) is async.
    /// Project that type-owned method only for non-routine nodes in the accessor's
    /// ordinary wrapper/partial graph. Type-owned call targets can themselves be
    /// callable objects, so projection remains enabled; pointer/mode cycle detection
    /// and the shared depth cap bound recursive callable-object chains.
    pub(crate) fn property_accessor_returns_async_handle(
        inspect: &Bound<'_, PyModule>,
        accessor: &Bound<'_, pyo3::types::PyAny>,
    ) -> PyResult<bool> {
        Self::callable_graph_returns_async_handle(inspect, accessor, true)
    }

    fn callable_graph_returns_async_handle(
        _inspect: &Bound<'_, PyModule>,
        function: &Bound<'_, pyo3::types::PyAny>,
        project_callable_object: bool,
    ) -> PyResult<bool> {
        let py = function.py();
        let is_async_callable = |callable: &Bound<'_, PyAny>| -> PyResult<bool> {
            Ok(crate::runtime_sentinels::inspect_iscoroutinefunction(py)?
                .call1((callable,))?
                .is_truthy()?
                || crate::runtime_sentinels::inspect_isasyncgenfunction(py)?
                    .call1((callable,))?
                    .is_truthy()?)
        };

        let current = Self::exact_bound_method_function(function)
            .map(Bound::into_any)
            .unwrap_or_else(|| function.clone());
        let mut seen = std::collections::HashSet::new();
        // Retain visited nodes while pointer identity is used as the cycle key.
        // Dynamic wrapper properties may otherwise free a node and let CPython
        // recycle its address for a distinct later node.
        let mut retained = Vec::new();
        let mut pending = vec![(current, project_callable_object)];
        const MAX_WRAPPER_DEPTH: usize = 256;
        for _ in 0..MAX_WRAPPER_DEPTH {
            let Some((current, may_project_call)) = pending.pop() else {
                return Ok(false);
            };
            if !seen.insert((current.as_ptr(), may_project_call)) {
                continue;
            }
            retained.push(current.clone());
            if is_async_callable(&current)? {
                return Ok(true);
            }

            if let Ok(wrapped) = current.getattr(pyo3::intern!(py, "__wrapped__")) {
                pending.push((wrapped, true));
            }

            // Looking up a `functools.partialmethod` on its class can produce a
            // synchronous helper function. Its version-specific partialmethod
            // metadata retains the actual async callable.
            if let Some(func) = Self::partialmethod_source(&current) {
                pending.push((func, true));
            }

            // Python 3.8's `inspect.iscoroutinefunction` recognizes partials of plain
            // coroutine functions, but not a partial whose source is a bound coroutine
            // classmethod. Exact `functools.partial` traversal closes that version gap.
            if let Some(func) = Self::functools_partial_source(&current) {
                pending.push((func, true));
            }

            if may_project_call
                && !crate::runtime_sentinels::inspect_isroutine(py)?
                    .call1((&current,))?
                    .is_truthy()?
            {
                // Python special-method dispatch is type-owned and ignores both an
                // instance's ordinary `__call__` attribute and custom metaclass
                // `__getattribute__` reports. Function/method targets are routines and
                // therefore stop projection naturally; nested callable objects remain
                // bounded by the shared cycle/depth guards.
                match Self::type_owned_call(&current) {
                    Ok(Some(type_call)) => pending.push((type_call, true)),
                    Ok(None) => {}
                    Err(_) => {
                        // A hostile descriptor can raise while binding `__call__`.
                        // Preserve the live Python result instead of aborting the
                        // entire module or trusting an awaited annotation.
                        return Ok(true);
                    }
                }
            }
        }

        // A dynamically-produced or pathologically deep wrapper graph cannot be
        // classified soundly. Preserve the live Python object rather than risk
        // extracting it as an annotated awaited value.
        Ok(true)
    }

    /// Parse a live annotation, honoring module-local classes that shadow textual
    /// builtin/typing spellings under `from __future__ import annotations`.
    pub(crate) fn parse_signature_annotation_or_unknown(
        annotation: Bound<'_, pyo3::types::PyAny>,
        signature_chain: Option<&[Bound<'_, pyo3::types::PyAny>]>,
        diagnostics: &mut Vec<DomainDiagnostic>,
        diagnostic_path: &Path,
        subject: String,
    ) -> Type {
        let Some(signature_chain) = signature_chain else {
            crate::report::warn(
                diagnostics,
                "signature_annotation_source_untrusted",
                diagnostic_path.to_py(),
                format!(
                    "{subject} has no trustworthy terminal callable after dynamic, cyclic, or \
                     excessive wrapper traversal. Falling back to an untyped Python object."
                ),
            );
            return Type::Unknown;
        };
        let globals = Self::signature_annotation_globals(signature_chain);
        if annotation.is_instance_of::<PyString>() && globals.is_none() {
            crate::report::warn(
                diagnostics,
                "signature_annotation_source_untrusted",
                diagnostic_path.to_py(),
                format!(
                    "{subject} is a string annotation without exact Python-function globals. \
                     Falling back to an untyped Python object."
                ),
            );
            return Type::Unknown;
        }
        Self::parse_live_annotation_or_unknown(
            annotation,
            globals.as_ref(),
            diagnostics,
            diagnostic_path,
            subject,
        )
    }

    /// Parse a live annotation, honoring module-local classes that shadow textual
    /// builtin/typing spellings under `from __future__ import annotations`.
    pub(crate) fn parse_live_annotation_or_unknown(
        annotation: Bound<'_, pyo3::types::PyAny>,
        globals: Option<&Bound<'_, PyDict>>,
        diagnostics: &mut Vec<DomainDiagnostic>,
        diagnostic_path: &Path,
        subject: String,
    ) -> Type {
        match Self::parse_live_annotation(annotation, globals) {
            Ok(annotation) => annotation,
            Err(error) => {
                crate::report::warn(
                    diagnostics,
                    "live_annotation_parse_fallback",
                    diagnostic_path.to_py(),
                    format!(
                        "{subject} could not be parsed: {error}. Falling back to an untyped \
                         Python object."
                    ),
                );
                Type::Unknown
            }
        }
    }

    /// Strict annotation parser shared with external-IR lowering. Live callers should
    /// normally use [`Self::parse_live_annotation_or_unknown`] so hostile runtime
    /// metadata cannot abort an otherwise inspectable module.
    pub(crate) fn parse_live_annotation(
        annotation: Bound<'_, pyo3::types::PyAny>,
        globals: Option<&Bound<'_, PyDict>>,
    ) -> Result<Type> {
        if let Some(globals) = globals {
            if let Ok(annotation_string) = annotation.cast::<PyString>() {
                if let Some(rewritten) = Self::rewrite_shadowed_builtin_annotation(
                    annotation.py(),
                    annotation_string.to_str()?,
                    globals,
                )? {
                    return Type::from_str(&rewritten);
                }
            }
        }
        annotation.try_into()
    }

    /// Resolve parser-reserved identifier tokens whose globals-dict binding is a class.
    ///
    /// This is deliberately a lexical rewrite over the already-produced annotation
    /// string. Resolution performs exact `dict` lookups and type-identity checks; typing
    /// classes additionally use their stable module/name metadata. No `eval` or
    /// `get_type_hints` is involved. A token following `.` is qualified
    /// (`builtins.int`, `pkg.int`) and is left intact. Rewriting tokens recursively also
    /// covers simple generic forms such as `list[int]`, `Optional[int]`, and
    /// `dict[str, int]`.
    fn rewrite_shadowed_builtin_annotation(
        py: Python<'_>,
        annotation: &str,
        globals: &Bound<'_, PyDict>,
    ) -> Result<Option<String>> {
        let Some(module_name) = globals
            .get_item(pyo3::intern!(py, "__name__"))?
            .and_then(|name| name.extract::<String>().ok())
        else {
            return Ok(None);
        };

        let bytes = annotation.as_bytes();
        let mut rewritten = String::with_capacity(annotation.len() + module_name.len());
        let mut index = 0;
        let mut previous_character = None;
        let mut previous_non_whitespace = None;
        let mut changed = false;
        let continues_identifier = |character: char| {
            character == '_' || character.is_alphanumeric() || !character.is_ascii()
        };
        while index < bytes.len() {
            if bytes[index].is_ascii_alphabetic() || bytes[index] == b'_' {
                let start = index;
                index += 1;
                while index < bytes.len()
                    && (bytes[index].is_ascii_alphanumeric() || bytes[index] == b'_')
                {
                    index += 1;
                }
                let token = &annotation[start..index];
                let joined_to_left_identifier =
                    previous_character.is_some_and(continues_identifier);
                let joined_to_right_identifier = annotation[index..]
                    .chars()
                    .next()
                    .is_some_and(continues_identifier);
                let followed_by_qualification = annotation[index..]
                    .chars()
                    .find(|character| !character.is_whitespace())
                    == Some('.');
                let is_unqualified_token = previous_non_whitespace != Some('.')
                    && !followed_by_qualification
                    && !joined_to_left_identifier
                    && !joined_to_right_identifier;
                // Rewrite only spellings which the string parser would otherwise consume
                // as builtins or typing forms. An ordinary class name such as `User`
                // already parses as a custom type and must stay untouched; importantly,
                // this also preserves wrapper syntax such as `ForwardRef("User")` when
                // `ForwardRef` is the imported typing class.
                let replacement = if is_unqualified_token
                    && Self::is_shadowable_annotation_name(token)
                {
                    globals.get_item(token)?.and_then(|binding| {
                        if let Some(path) = Self::exact_typing_annotation_path(token, &binding) {
                            return Some(path);
                        }
                        if !binding.is_instance_of::<PyType>() {
                            return None;
                        }
                        Some(
                            Self::exact_builtin_annotation_name(&binding)
                                .map(str::to_owned)
                                .unwrap_or_else(|| format!("{module_name}.{token}")),
                        )
                    })
                } else {
                    None
                };
                if let Some(replacement) = replacement.filter(|replacement| replacement != token) {
                    rewritten.push_str(&replacement);
                    changed = true;
                } else {
                    rewritten.push_str(token);
                }
                previous_character = token.chars().last();
                previous_non_whitespace = token.chars().last();
            } else {
                let character = annotation[index..]
                    .chars()
                    .next()
                    .unwrap_or_else(|| unreachable!());
                rewritten.push(character);
                index += character.len_utf8();
                previous_character = Some(character);
                if !character.is_whitespace() {
                    previous_non_whitespace = Some(character);
                }
            }
        }

        Ok(changed.then_some(rewritten))
    }

    /// Names with special meaning to [`Type::from_str`] when unqualified.
    ///
    /// Capitalized builtin aliases are included only in their subscripted form by that
    /// parser, but rewriting the bare token here is harmless and lets the same lexical
    /// pass handle both `List` and `List[T]`.
    fn is_shadowable_annotation_name(name: &str) -> bool {
        matches!(
            name,
            "Any"
                | "Annotated"
                | "Array"
                | "Callable"
                | "Collection"
                | "Dict"
                | "Ellipsis"
                | "FrozenSet"
                | "Iterable"
                | "Iterator"
                | "List"
                | "Mapping"
                | "NoneType"
                | "Optional"
                | "Sequence"
                | "Set"
                | "Tuple"
                | "Type"
                | "Union"
                | "bool"
                | "bytearray"
                | "bytes"
                | "callable"
                | "capsule"
                | "cfunction"
                | "code"
                | "complex"
                | "dict"
                | "float"
                | "frame"
                | "frozenset"
                | "function"
                | "int"
                | "list"
                | "module"
                | "set"
                | "slice"
                | "str"
                | "super"
                | "timedelta"
                | "traceback"
                | "tuple"
                | "type"
        ) || crate::config::FORBIDDEN_TYPE_NAMES.contains(&name)
    }

    /// Return the parser's canonical spelling for a known exact Python type.
    ///
    /// The spelling is derived from object identity rather than the global's name, so
    /// aliases such as `List = list` and `Sequence = list` retain the actual `list[T]`
    /// semantics instead of being mistaken for either a typing alias or a local class.
    fn exact_builtin_annotation_name(
        value: &Bound<'_, pyo3::types::PyAny>,
    ) -> Option<&'static str> {
        let value = value.cast::<PyType>().ok()?;
        let py = value.py();
        macro_rules! exact {
            ($typ:ty, $name:literal) => {
                if value.is(PyType::new::<$typ>(py)) {
                    return Some($name);
                }
            };
        }
        exact!(pyo3::types::PyBool, "bool");
        exact!(pyo3::types::PyByteArray, "bytearray");
        exact!(pyo3::types::PyBytes, "bytes");
        exact!(pyo3::types::PyComplex, "complex");
        exact!(pyo3::types::PyDict, "dict");
        exact!(pyo3::types::PyEllipsis, "Ellipsis");
        exact!(pyo3::types::PyFloat, "float");
        exact!(pyo3::types::PyFrozenSet, "frozenset");
        exact!(pyo3::types::PyInt, "int");
        exact!(pyo3::types::PyList, "list");
        exact!(pyo3::types::PyNone, "NoneType");
        exact!(pyo3::types::PySet, "set");
        exact!(pyo3::types::PySlice, "slice");
        exact!(pyo3::types::PyString, "str");
        exact!(pyo3::types::PyTuple, "tuple");
        exact!(pyo3::types::PyType, "type");
        exact!(pyo3::types::PyCapsule, "capsule");
        exact!(pyo3::types::PyCFunction, "cfunction");
        #[cfg(not(Py_LIMITED_API))]
        exact!(pyo3::types::PyCode, "code");
        #[cfg(all(not(Py_LIMITED_API), not(any(PyPy, GraalPy))))]
        exact!(pyo3::types::PyFrame, "frame");
        #[cfg(all(not(Py_LIMITED_API), not(PyPy)))]
        exact!(pyo3::types::PyFunction, "function");
        exact!(pyo3::types::PyModule, "module");
        #[cfg(not(PyPy))]
        exact!(pyo3::types::PySuper, "super");
        exact!(pyo3::types::PyTraceback, "traceback");
        #[cfg(not(Py_LIMITED_API))]
        exact!(pyo3::types::PyDelta, "timedelta");
        None
    }

    /// Return the stable qualified spelling for a genuine same-named typing export.
    fn exact_typing_annotation_path(
        name: &str,
        value: &Bound<'_, pyo3::types::PyAny>,
    ) -> Option<String> {
        let module = value
            .getattr(pyo3::intern!(value.py(), "__module__"))
            .ok()?
            .extract::<String>()
            .ok()?;
        let qualname = value
            .getattr(pyo3::intern!(value.py(), "__qualname__"))
            .ok()
            .and_then(|qualname| qualname.extract::<String>().ok());
        // Python 3.8's canonical typing aliases expose `__module__` but not
        // `__qualname__`. When a qualified name is present, it must still agree with
        // the globals-dict spelling before the identity check below can succeed.
        if !matches!(module.as_str(), "typing" | "typing_extensions")
            || qualname.as_deref().is_some_and(|qualname| qualname != name)
        {
            return None;
        }

        // Metadata alone is writable on arbitrary Python objects. Require identity with
        // the same-named export in the already-loaded canonical module too. Combining
        // both checks rejects a local object merely installed into `typing` as well as a
        // local object that only spoofs `__module__`/`__qualname__`.
        let py = value.py();
        // SAFETY: `PyImport_GetModuleDict` returns the interpreter-owned, non-null
        // borrowed modules dictionary while `py` is attached. `Bound` acquires its own
        // reference, and the checked casts protect both concrete wrappers.
        let modules = unsafe { Bound::from_borrowed_ptr(py, pyo3::ffi::PyImport_GetModuleDict()) };
        let modules = modules.cast_into::<PyDict>().ok()?;
        let canonical_module = modules.get_item(&module).ok().flatten()?;
        let canonical_module = canonical_module.cast_into::<PyModule>().ok()?;
        canonical_module
            .dict()
            .get_item(name)
            .ok()
            .flatten()
            .filter(|canonical| value.is(canonical))
            .map(|_| format!("{module}.{name}"))
    }

    /// Construct a [`Function`] from the generic IR with zero `pyo3` usage.
    ///
    /// `typ` is the already-resolved [`FunctionType`] (the caller knows the owning
    /// class, if any). Parameters are converted purely from strings and then run
    /// through the exact same [`Self::finalize_parameters`] logic as [`Self::parse`],
    /// so the output is byte-identical to the live path for an equivalent signature.
    pub(crate) fn from_ir(
        _cfg: &Config,
        ir: &crate::schema::FunctionIr,
        mut typ: FunctionType,
        diagnostics: &mut Vec<DomainDiagnostic>,
        local_annotation_types: &super::IrAnnotationTypes,
    ) -> Result<Self> {
        let name = Path::from_py(&ir.name);
        let docstring = ir.docstring.clone();

        // Reject malformed method IR before shared finalization can strip an ordinary
        // argument. Instance methods, constructors, and callables carry one explicit
        // positional `self`/`cls` receiver in IR. Class methods instead mirror Python's
        // bound signature, where the implicit class receiver is already absent.
        if let FunctionType::Method {
            typ: MethodType::InstanceMethod | MethodType::Constructor | MethodType::Callable,
            ..
        } = &typ
        {
            match ir.params.first() {
                None => {
                    return Err(crate::PyBindgenError::CodegenError(format!(
                        "Method '{}' binds a receiver but declares no parameters; its leading \
                         'self'/'cls' parameter is missing from the IR",
                        ir.name
                    )));
                }
                Some(receiver) if !matches!(receiver.name.as_str(), "self" | "cls") => {
                    return Err(crate::PyBindgenError::CodegenError(format!(
                        "Method '{}' binds a receiver and requires a leading receiver named \
                         'self' or 'cls', found '{}'",
                        ir.name, receiver.name
                    )));
                }
                Some(receiver)
                    if !matches!(
                        receiver.kind,
                        crate::schema::ParameterKindIr::PositionalOnly
                            | crate::schema::ParameterKindIr::PositionalOrKeyword
                    ) =>
                {
                    return Err(crate::PyBindgenError::CodegenError(format!(
                        "Method '{}' binds a receiver; receiver must be positional-only or \
                         positional-or-keyword, found {:?}",
                        ir.name, receiver.kind
                    )));
                }
                Some(_) => {}
            }
        }
        let mut parameters = ir
            .params
            .iter()
            .map(|param| {
                let kind = ParameterKind::from_ir(param.kind);
                // Variadic parameters carry a kind-fixed synthetic annotation in
                // `Self::parse`; force the same here so the IR path stays byte-identical
                // (an omitted `*args`/`**kwargs` annotation must not fall through to the
                // string parser, which would yield a non-`PyTuple`/`PyDict` type).
                let annotation = match kind {
                    ParameterKind::VarPositional => Type::PyTuple(vec![Type::Unknown]),
                    ParameterKind::VarKeyword => Type::Optional(Box::new(Type::PyDict {
                        key_type: Box::new(Type::Unknown),
                        value_type: Box::new(Type::Unknown),
                    })),
                    _ => Self::parse_ir_annotation(&param.annotation, local_annotation_types)?,
                };
                Result::Ok(Parameter {
                    name: Ident::from_py(&param.name),
                    kind,
                    annotation,
                    has_default: param.has_default,
                })
            })
            .collect::<Result<Vec<_>>>()?;

        let return_annotation = Self::parse_ir_annotation(&ir.return_type, local_annotation_types)?;

        // Run the SAME post-extraction parameter finalize/validate logic as `parse` and
        // retain its diagnostics in the owning module's generation report.
        Self::finalize_parameters(&mut parameters, &mut typ, &name, diagnostics);

        Ok(Self {
            name,
            typ,
            parameters,
            return_annotation,
            docstring,
        })
    }

    /// Parse an external-IR annotation after qualifying direct local types that collide
    /// with parser-reserved builtin/typing spellings.
    ///
    /// A schema module can define `pkg.Type` and annotate `Type[int, str]`; that is a
    /// parameterized local class, not malformed `typing.Type`. The live path resolves
    /// the same ambiguity from the function's globals. External IR instead provides a
    /// deterministic name map assembled by [`super::Module::from_ir_in_schema`].
    pub(crate) fn forced_custom_ir_annotation(path: &str) -> String {
        format!("{IR_FORCED_CUSTOM_PREFIX}{path}")
    }

    fn restore_forced_custom_ir_annotations(annotation: Type) -> Type {
        match annotation {
            Type::Other(name) => Type::Other(
                name.strip_prefix(IR_FORCED_CUSTOM_PREFIX)
                    .unwrap_or(&name)
                    .to_owned(),
            ),
            Type::Optional(inner) => {
                Type::Optional(Box::new(Self::restore_forced_custom_ir_annotations(*inner)))
            }
            Type::Union(types) => Type::Union(
                types
                    .into_iter()
                    .map(Self::restore_forced_custom_ir_annotations)
                    .collect(),
            ),
            Type::Collection(inner) => {
                Type::Collection(Box::new(Self::restore_forced_custom_ir_annotations(*inner)))
            }
            Type::FixedArray(inner, dimensions) => Type::FixedArray(
                Box::new(Self::restore_forced_custom_ir_annotations(*inner)),
                dimensions,
            ),
            Type::PyDict {
                key_type,
                value_type,
            } => Type::PyDict {
                key_type: Box::new(Self::restore_forced_custom_ir_annotations(*key_type)),
                value_type: Box::new(Self::restore_forced_custom_ir_annotations(*value_type)),
            },
            Type::PyFrozenSet(inner) => {
                Type::PyFrozenSet(Box::new(Self::restore_forced_custom_ir_annotations(*inner)))
            }
            Type::PyList(inner) => {
                Type::PyList(Box::new(Self::restore_forced_custom_ir_annotations(*inner)))
            }
            Type::PySet(inner) => {
                Type::PySet(Box::new(Self::restore_forced_custom_ir_annotations(*inner)))
            }
            Type::PyTuple(types) => Type::PyTuple(
                types
                    .into_iter()
                    .map(Self::restore_forced_custom_ir_annotations)
                    .collect(),
            ),
            Type::PyFunction {
                param_types,
                return_annotation,
            } => Type::PyFunction {
                param_types: param_types
                    .into_iter()
                    .map(Self::restore_forced_custom_ir_annotations)
                    .collect(),
                return_annotation: Box::new(Self::restore_forced_custom_ir_annotations(
                    *return_annotation,
                )),
            },
            annotation => annotation,
        }
    }

    pub(crate) fn parse_ir_annotation(
        annotation: &str,
        local_annotation_types: &super::IrAnnotationTypes,
    ) -> Result<Type> {
        let bytes = annotation.as_bytes();
        let mut rewritten = String::with_capacity(annotation.len());
        let mut index = 0;
        let mut previous_character = None;
        let mut previous_non_whitespace = None;
        let mut changed = false;
        let continues_identifier = |character: char| {
            character == '_' || character.is_alphanumeric() || !character.is_ascii()
        };
        let continues_qualified_path =
            |character: char| character == '.' || continues_identifier(character);
        while index < bytes.len() {
            // Only candidates whose own first byte matches can apply here, so the
            // prepared table hands back exactly that bucket, still longest-first.
            let mapped = local_annotation_types
                .candidates_starting_with(bytes[index])
                .iter()
                .find(|(path, _)| {
                    if !annotation[index..].starts_with(path.as_str())
                        || previous_character.is_some_and(continues_qualified_path)
                    {
                        return false;
                    }
                    let remaining = &annotation[index + path.len()..];
                    if path.ends_with('.') {
                        remaining.chars().next().is_some_and(continues_identifier)
                    } else {
                        !remaining
                            .chars()
                            .next()
                            .is_some_and(continues_qualified_path)
                    }
                });
            if let Some((path, replacement)) = mapped {
                rewritten.push_str(replacement);
                changed = true;
                index += path.len();
                previous_character = path.chars().last();
                previous_non_whitespace = path.chars().last();
                continue;
            }

            if bytes[index].is_ascii_alphabetic() || bytes[index] == b'_' {
                let start = index;
                index += 1;
                while index < bytes.len()
                    && (bytes[index].is_ascii_alphanumeric() || bytes[index] == b'_')
                {
                    index += 1;
                }
                let token = &annotation[start..index];
                let joined_to_left_identifier =
                    previous_character.is_some_and(continues_identifier);
                let joined_to_right_identifier = annotation[index..]
                    .chars()
                    .next()
                    .is_some_and(continues_identifier);
                let followed_by_qualification = annotation[index..]
                    .chars()
                    .find(|character| !character.is_whitespace())
                    == Some('.');
                let is_unqualified_token = previous_non_whitespace != Some('.')
                    && !followed_by_qualification
                    && !joined_to_left_identifier
                    && !joined_to_right_identifier;
                let replacement = is_unqualified_token
                    .then(|| local_annotation_types.get(token))
                    .flatten();
                if let Some(replacement) = replacement {
                    rewritten.push_str(replacement);
                    changed = true;
                } else {
                    rewritten.push_str(token);
                }
                previous_character = token.chars().last();
                previous_non_whitespace = token.chars().last();
            } else {
                let character = annotation[index..]
                    .chars()
                    .next()
                    .unwrap_or_else(|| unreachable!());
                rewritten.push(character);
                index += character.len_utf8();
                previous_character = Some(character);
                if !character.is_whitespace() {
                    previous_non_whitespace = Some(character);
                }
            }
        }

        Type::from_str(if changed { &rewritten } else { annotation })
            .map(Self::restore_forced_custom_ir_annotations)
            .map_err(|error| {
                if !changed {
                    return error;
                }
                let detail = match error {
                    crate::PyBindgenError::CodegenError(detail) => detail,
                    other => other.to_string(),
                }
                .replace(IR_FORCED_CUSTOM_PREFIX, "")
                .replace('\0', "\\0");
                crate::PyBindgenError::CodegenError(format!(
                    "Invalid external-IR annotation {annotation:?}: {detail}"
                ))
            })
    }

    /// Shared post-extraction parameter finalize/validate logic used by both
    /// [`Self::parse`] (live introspection) and [`Self::from_ir`] (external IR).
    ///
    /// This is pure string/struct logic with zero `pyo3` usage; the live-only
    /// method-type *inference* (which probes Python) stays in [`Self::parse`].
    fn finalize_parameters(
        parameters: &mut Vec<Parameter>,
        typ: &mut FunctionType,
        name: &Path,
        diagnostics: &mut Vec<DomainDiagnostic>,
    ) {
        // (#20) A parameter literally named `_` must be RETAINED, not dropped: dropping
        // it before the receiver strip and positional emission shifts every subsequent
        // parameter's index, breaking positional call arity. Give each a distinct
        // synthetic name (`_arg{N}`) so it occupies its real position while remaining a
        // valid, non-clashing Rust identifier. Renaming (rather than stripping) here also
        // keeps the leading-receiver logic below correct, since the receiver is never a
        // `_` parameter.
        // Names already used by real (non-`_`) parameters, so a synthetic name can
        // never collide with one — e.g. a signature mixing `_` with a real parameter
        // literally named `_arg1` (both would otherwise become `_arg1` → E0415).
        let mut taken: rustc_hash::FxHashSet<String> = parameters
            .iter()
            .filter(|p| p.name.as_rs() != "r#_")
            .map(|p| p.name.as_rs().to_string())
            .collect();
        for (index, param) in parameters.iter_mut().enumerate() {
            if param.name.as_rs() == "r#_" {
                let mut candidate = format!("_arg{index}");
                while !taken.insert(candidate.clone()) {
                    candidate.push('_');
                }
                // Keep the real Python name (`_`) reachable via `as_py()` -- it is the
                // actual keyword Python expects at the call site for a keyword-only
                // parameter -- while only the Rust-side binding identifier becomes the
                // synthetic, collision-free name. `Ident::from_rs` would clobber both
                // spellings with the synthetic name, so a keyword-only `_` parameter
                // would be sent to Python under the wrong keyword (raising
                // `TypeError: unexpected keyword argument`).
                let original_py_name = param.name.as_py().to_owned();
                param.name = Ident::with_rs_override(&original_py_name, &candidate);
            }
        }

        // Infer only genuinely unresolved methods. Explicit static/class methods may
        // legally name their first ordinary parameter `self`; descriptor inspection or
        // external IR already proved their method kind, so the spelling must not
        // override that stronger evidence.
        match &*typ {
            FunctionType::Method {
                typ: MethodType::InstanceMethod | MethodType::Constructor | MethodType::Callable,
                ..
            } => {}
            FunctionType::Method {
                class_path,
                typ: MethodType::Unknown,
            } => {
                let class_path = class_path.clone();
                let first_parameter_is_positional = parameters.first().is_some_and(|parameter| {
                    matches!(
                        parameter.kind,
                        ParameterKind::PositionalOnly | ParameterKind::PositionalOrKeyword
                    )
                });
                match parameters.first().map(|p| p.name.as_rs()) {
                    // A leading `self` receiver -> instance method; the shared
                    // receiver-strip below removes it.
                    Some("r#self") if first_parameter_is_positional => {
                        *typ = FunctionType::Method {
                            class_path,
                            typ: MethodType::InstanceMethod,
                        };
                    }
                    // A leading class/metaclass receiver -> class method by convention.
                    // `cls` is the ordinary spelling; `mcs`/`mcls`/`metacls` are the
                    // conventional metaclass-method receivers (e.g. stdlib
                    // `enum.EnumMeta`). Unlike a genuine `ClassMethod` (whose bound
                    // signature already omits the implicit class receiver), this one
                    // carries an explicit receiver parameter that must be stripped here.
                    // Classifying it as a static method instead -- the previous behavior
                    // -- would forward the receiver to Python as an ordinary positional
                    // argument, so every call would raise `TypeError` from the duplicated
                    // class receiver.
                    Some("cls" | "mcs" | "mcls" | "metacls") if first_parameter_is_positional => {
                        parameters.remove(0);
                        *typ = FunctionType::Method {
                            class_path,
                            typ: MethodType::ClassMethod,
                        };
                    }
                    _ => {
                        *typ = FunctionType::Method {
                            class_path,
                            typ: MethodType::StaticMethod,
                        };
                    }
                }
            }
            FunctionType::Method { .. } => {}
            FunctionType::Function | FunctionType::Closure => {
                if parameters.first().map(|p| p.name.as_rs()) == Some("r#self") {
                    if [
                        ParameterKind::PositionalOnly,
                        ParameterKind::PositionalOrKeyword,
                    ]
                    .contains(&parameters[0].kind)
                    {
                        crate::report::warn(
                            diagnostics,
                            "function_self_parameter_renamed",
                            name.to_py(),
                            format!(
                                "Function '{name}' has the first parameter named 'self', but is not marked as a method. The parameter is renamed to '__unknown_self__'."
                            ),
                        );
                        // Route the rename through the same collision-avoiding
                        // allocation as the `_` -> `_arg{N}` branch above. A parameter
                        // literally named `__unknown_self__` alongside this renamed
                        // receiver would otherwise share the binding `p___unknown_self__`
                        // (E0415: identifier bound more than once). `taken` already holds
                        // every other parameter's Rust name; the un-collided common case
                        // keeps the name `__unknown_self__` verbatim.
                        let mut candidate = String::from("__unknown_self__");
                        while !taken.insert(candidate.clone()) {
                            candidate.push('_');
                        }
                        parameters[0].name = Ident::from_rs(&candidate);
                        parameters[0].annotation = Type::Unknown;
                    } else {
                        crate::report::warn(
                            diagnostics,
                            "function_self_parameters_replaced",
                            name.to_py(),
                            format!(
                                "Function '{name}' has the first parameter named 'self', but is not marked as a method. All parameters are replaced with '*args' and '**kwargs'."
                            ),
                        );
                        *parameters = vec![
                            Parameter {
                                name: Ident::from_rs("args"),
                                kind: ParameterKind::VarPositional,
                                annotation: Type::PyTuple(vec![Type::Unknown]),
                                has_default: false,
                            },
                            Parameter {
                                name: Ident::from_rs("kwargs"),
                                kind: ParameterKind::VarKeyword,
                                annotation: Type::Optional(Box::new(Type::PyDict {
                                    key_type: Box::new(Type::Unknown),
                                    value_type: Box::new(Type::Unknown),
                                })),
                                has_default: false,
                            },
                        ];
                    }
                }
            }
        }

        // Hack: Reassign InstanceMethod with no parameter to StaticMethod
        // This should not be necessary as every InstanceMethod should have at least one parameter (self), but it does for certain complex Python modules
        if let FunctionType::Method {
            class_path,
            typ: MethodType::InstanceMethod,
        } = &*typ
        {
            if parameters.is_empty() {
                let class_path = class_path.clone();
                crate::report::warn(
                    diagnostics,
                    "method_static_fallback",
                    name.to_py(),
                    format!(
                        "Method '{name}' is marked as an instance method, but has no parameters. Changed to static method.",
                    ),
                );
                *typ = FunctionType::Method {
                    class_path,
                    typ: MethodType::StaticMethod,
                };
            }
        }

        // Skip the first parameter if it's an instance method (or `__init__`/`__call__`)
        if let FunctionType::Method {
            typ: MethodType::InstanceMethod | MethodType::Constructor | MethodType::Callable,
            ..
        } = &*typ
        {
            // Guard against malformed IR: a constructor/callable declared with no
            // parameters (the live path always includes the receiver) must not panic on
            // `Vec::remove(0)`.
            if !parameters.is_empty() {
                parameters.remove(0);
            }
        }

        // A remaining `self` is ambiguous only after stripping a receiver. Static and
        // class methods may legally use that spelling for an ordinary argument; their
        // generated Rust argument is prefixed with `p_` and remains valid.
        if matches!(
            &*typ,
            FunctionType::Method {
                typ: MethodType::InstanceMethod | MethodType::Constructor | MethodType::Callable,
                ..
            }
        ) && parameters
            .iter()
            .any(|param| param.name.as_rs() == "r#self")
        {
            crate::report::warn(
                diagnostics,
                "method_self_parameters_replaced",
                name.to_py(),
                format!(
                    "Method '{name}' has a non-first parameter named 'self'. All parameters are replaced with '*args' and '**kwargs'.",
                ),
            );
            *parameters = vec![
                Parameter {
                    name: Ident::from_rs("args"),
                    kind: ParameterKind::VarPositional,
                    annotation: Type::PyTuple(vec![Type::Unknown]),
                    has_default: false,
                },
                Parameter {
                    name: Ident::from_rs("kwargs"),
                    kind: ParameterKind::VarKeyword,
                    annotation: Type::Optional(Box::new(Type::PyDict {
                        key_type: Box::new(Type::Unknown),
                        value_type: Box::new(Type::Unknown),
                    })),
                    has_default: false,
                },
            ];
        }

        // Final pass, deliberately last: it must observe the parameter list exactly as
        // emitted, i.e. after the `_` -> `_arg{N}` renames, the `__unknown_self__`
        // rename, the leading-receiver strip and both wholesale `*args`/`**kwargs`
        // replacements above.
        //
        // rustc canonicalizes identifiers to NFC, so two parameters whose Rust spellings
        // differ only by Unicode normalization (`é` U+00E9 vs `e` + U+0301) bind the one
        // identifier `p_é` twice -> E0415 "identifier bound more than once". Real Python
        // source cannot produce such a pair (CPython NFKC-normalizes identifiers at
        // compile time, PEP 3131) and the external-IR path already rejects it in
        // `schema_validation::validate_function_parameters`, but a hand-assigned
        // `__signature__` can: `inspect.Parameter` validates its name with
        // `str.isidentifier()` alone, which does not normalize. Rename the later member
        // of each canonical collision so every binding stays unique.
        //
        // `canonical_rust_name` is the identity on ASCII, so an ASCII-only signature --
        // every real target -- allocates the same names it always did and the emitted
        // bytes are unchanged. The set is kept SEPARATE from `taken` above on purpose:
        // `canonical_rust_name` also strips `r#`, which would merge `r#self` into `self`
        // and perturb the existing `_arg{N}`/`__unknown_self__` allocation.
        let mut canonical_bindings: rustc_hash::FxHashSet<String> =
            rustc_hash::FxHashSet::default();
        for param in parameters.iter_mut() {
            let mut candidate = param_binding_suffix(&param.name).to_owned();
            let mut renamed = false;
            // Append to the `r#`-STRIPPED suffix, never to `as_rs()`: `r#self` + `_`
            // would yield `r#self_`, a raw spelling of a non-keyword, which is a
            // different identifier again and merely relocates the collision.
            while !canonical_bindings.insert(super::naming::canonical_rust_name(&format!(
                "p_{candidate}"
            ))) {
                candidate.push('_');
                renamed = true;
            }
            if renamed {
                crate::report::warn(
                    diagnostics,
                    "function_parameter_binding_collision",
                    name.to_py(),
                    format!(
                        "Function '{name}' has parameters whose Rust bindings collide after Unicode normalization; parameter '{}' is bound as 'p_{candidate}'.",
                        param.name.as_py(),
                    ),
                );
                // Only the Rust-side spelling changes: `as_py()` must keep the exact
                // Python name, which is the keyword actually sent at the call site for a
                // keyword-only parameter (the same reason the `_` -> `_arg{N}` rename
                // above uses `with_rs_override` rather than `Ident::from_rs`).
                let original_py_name = param.name.as_py().to_owned();
                param.name = Ident::with_rs_override(&original_py_name, &candidate);
            }
        }
    }

    /// Whether this function's signature (parameters or return) references a
    /// collection-protocol type that lowers to the generic `PyCollection<T>` wrapper.
    pub(crate) fn mentions_collection(&self) -> bool {
        self.return_annotation.mentions_collection()
            || self
                .parameters
                .iter()
                .any(|param| param.annotation.mentions_collection())
    }

    pub(crate) fn generate(
        &self,
        cfg: &Config,
        name_allocator: &mut super::naming::NameAllocator,
        local_types: &super::LocalTypes,
        diagnostics: &mut Vec<DomainDiagnostic>,
        import_mode: &ImportMode,
    ) -> Result<FunctionImplementation> {
        let mut impl_fn = proc_macro2::TokenStream::new();

        // Documentation
        if cfg.generate_docs {
            if let Some(mut docstring) = self.docstring.clone() {
                crate::utils::text::format_docstring(&mut docstring);
                impl_fn.extend(quote::quote! {
                    #[doc = #docstring]
                });
            }
        }

        // Function signature
        let function_ident: syn::Ident = {
            let name = self.name.name();
            if let Some(ident) =
                super::naming::rust_ident(name, super::naming::SymbolKind::Function)
            {
                if matches!(self.typ, FunctionType::Function | FunctionType::Closure)
                    && name_allocator.is_generated(name)
                {
                    let aliased_ident = name_allocator.allocate(&format!("py_{}", name.as_py()));
                    crate::report::warn(
                        diagnostics,
                        "function_generated_name_renamed",
                        self.name.to_py(),
                        format!(
                            "Function '{}' conflicts with generated module support. Renamed to '{aliased_ident}'.",
                            self.name
                        ),
                    );
                    aliased_ident
                } else if matches!(self.typ, FunctionType::Method { .. })
                    && (super::naming::conflicts_with_inherent_class_method(name)
                        || (import_mode.uses_dedup_model()
                            && super::naming::conflicts_with_root_trait_method(name)))
                {
                    let aliased_ident = name_allocator.allocate(&format!("py_{}", name.as_py()));
                    crate::report::warn(
                        diagnostics,
                        "method_inherent_name_renamed",
                        self.name.to_py(),
                        format!(
                            "Method '{}' conflicts with the generated inherent class API. Renamed to '{aliased_ident}'.",
                            self.name
                        ),
                    );
                    aliased_ident
                } else {
                    if TryInto::<syn::Ident>::try_into(name).is_err() {
                        crate::report::warn(
                            diagnostics,
                            "function_invalid_ident_renamed",
                            self.name.to_py(),
                            format!(
                                "Function '{}' is an invalid Rust ident for a function name. Renamed to '{}'.",
                                self.name,
                                self.name
                                    .parent()
                                    .unwrap_or_default()
                                    .join(&Ident::from_rs(&ident.to_string()).into())
                            ),
                        );
                    }
                    ident
                }
            } else {
                crate::report::warn(
                    diagnostics,
                    "function_invalid_ident_skipped",
                    self.name.to_py(),
                    format!(
                        "Function '{}' is an invalid Rust ident for a function name. Renaming failed. Bindings will not be generated.",
                        self.name
                    ),
                );
                return Ok(FunctionImplementation::empty_function());
            }
        };
        let param_idents: Vec<syn::Ident> = self
            .parameters
            .iter()
            .map(|param| {
                Ok(
                    Ident::from_py(&format!("p_{}", param_binding_suffix(&param.name)))
                        .try_into()?,
                )
            })
            .collect::<Result<Vec<_>>>()?;
        // Pre-process parameters that require it
        let param_preprocessing: proc_macro2::TokenStream = self
            .parameters
            .iter()
            .zip(param_idents.iter())
            .map(|(param, param_ident)| {
                param.annotation.preprocess_borrowed(
                    param_ident,
                    local_types,
                    param.kind == ParameterKind::VarKeyword,
                )
            })
            .collect();
        let param_types: Vec<proc_macro2::TokenStream> = self
            .parameters
            .iter()
            .map(|param| Result::Ok(param.annotation.clone().into_rs_borrowed(local_types)))
            .collect::<Result<Vec<_>>>()?;
        let return_type = self.return_annotation.clone().into_rs_owned(local_types);
        // The actual Rust identifier the method is emitted under. For an instance
        // method it is `function_ident`; a callable is renamed to `call`. Captured
        // here so the external-IR inherent forwarder (below) can name the trait
        // method it delegates to.
        let mut method_ident = function_ident.clone();
        let fn_contract = match &self.typ {
            FunctionType::Method {
                typ: MethodType::InstanceMethod,
                ..
            } => {
                name_allocator.record_resolution(self.name.name(), &function_ident);
                quote::quote! {
                    fn #function_ident<'py>(
                        &self,
                        py: ::pyo3::marker::Python<'py>,
                        #(#param_idents: #param_types),*
                    ) -> ::pyo3::PyResult<#return_type>
                }
            }
            FunctionType::Method {
                typ: MethodType::Callable,
                ..
            } => {
                let call_fn_ident = name_allocator.allocate("call");
                name_allocator.record_resolution(self.name.name(), &call_fn_ident);
                method_ident = call_fn_ident.clone();
                quote::quote! {
                    fn #call_fn_ident<'py>(
                        &self,
                        py: ::pyo3::marker::Python<'py>,
                        #(#param_idents: #param_types),*
                    ) -> ::pyo3::PyResult<#return_type>
                }
            }
            FunctionType::Method {
                typ: MethodType::Constructor,
                ..
            } => {
                // The `new`/`call` slots are allocated on demand here, after any
                // deduplication has already dropped inherited members. A base with an
                // instance method literally named `new` therefore keeps this constructor at
                // `new1` on the flat build but frees the `new` slot on a deduplicated
                // subclass -- cosmetic only: `T::new(..)` binds the constructor and
                // `x.new(..)` the instance method regardless of the allocated name.
                let new_fn_ident = name_allocator.allocate("new");
                name_allocator.record_resolution(self.name.name(), &new_fn_ident);
                quote::quote! {
                    pub fn #new_fn_ident<'py>(
                        py: ::pyo3::marker::Python<'py>,
                        #(#param_idents: #param_types),*
                    ) -> ::pyo3::PyResult<Self>
                }
            }
            _ => {
                if !matches!(
                    self.typ,
                    FunctionType::Method {
                        typ: MethodType::Unknown,
                        ..
                    }
                ) {
                    name_allocator.record_resolution(self.name.name(), &function_ident);
                }
                quote::quote! {
                    pub fn #function_ident<'py>(
                        py: ::pyo3::marker::Python<'py>,
                        #(#param_idents: #param_types),*
                    ) -> ::pyo3::PyResult<#return_type>
                }
            }
        };
        impl_fn.extend(fn_contract.clone());

        // If the function is a method with `self` as a parameter, extract the Python marker from `self`
        let maybe_extract_py = TokenStream::new();

        // Function body (function dispatcher)
        let function_dispatcher = match &self.typ {
            FunctionType::Function | FunctionType::Closure => {
                import_mode.resolve(&self.name.parent().unwrap_or_else(|| unreachable!()))?
            }
            FunctionType::Method {
                class_path,
                typ: MethodType::ClassMethod | MethodType::StaticMethod | MethodType::Constructor,
            } => import_mode.resolve(class_path)?,
            FunctionType::Method {
                typ: MethodType::InstanceMethod | MethodType::Callable,
                ..
            } => {
                // External-IR path: the body becomes a trait *default* method, where
                // `Self` is generic, so reach the handle via the synthetic root trait
                // accessor instead of the concrete inherent `self.bind(py)`.
                if import_mode.uses_dedup_model() {
                    quote::quote! {
                        self.__pyo3_obj().bind(py)
                    }
                } else {
                    quote::quote! {
                        self.bind(py)
                    }
                }
            }
            FunctionType::Method {
                typ: MethodType::Unknown,
                ..
            } => {
                crate::report::warn(
                    diagnostics,
                    "method_unknown_type_skipped",
                    self.name.to_py(),
                    format!(
                        "Method '{}' has an unknown type. Bindings will not be generated.",
                        self.name
                    ),
                );
                return Ok(FunctionImplementation::empty_method());
            }
        };
        // The instance/callable dispatcher already evaluates to `&Bound<PyAny>`
        // (`self.bind(py)` / `self.__pyo3_obj().bind(py)`), so the `.as_any()` the call sites
        // would otherwise append is an identity no-op. Only the module/class/static
        // dispatchers -- an imported module or type handle -- need the widening, so apply it
        // once here rather than emitting a redundant `.as_any()` on every instance call.
        let function_dispatcher = if matches!(
            &self.typ,
            FunctionType::Method {
                typ: MethodType::InstanceMethod | MethodType::Callable,
                ..
            }
        ) {
            function_dispatcher
        } else {
            quote::quote! { #function_dispatcher.as_any() }
        };

        // Function body: positional args
        let positional_args_idents: Vec<syn::Ident> = self
            .parameters
            .iter()
            .filter(|param| {
                [
                    ParameterKind::PositionalOnly,
                    ParameterKind::PositionalOrKeyword,
                ]
                .contains(&param.kind)
            })
            .map(|param| {
                Ok(
                    Ident::from_py(&format!("p_{}", param_binding_suffix(&param.name)))
                        .try_into()?,
                )
            })
            .collect::<Result<_>>()?;
        let var_positional_args_ident: Option<syn::Ident> = self
            .parameters
            .iter()
            .find(|param| param.kind == ParameterKind::VarPositional)
            .and_then(|param| {
                Ident::from_py(&format!("p_{}", param_binding_suffix(&param.name)))
                    .try_into()
                    .ok()
            });
        let has_positional_args =
            !positional_args_idents.is_empty() || var_positional_args_ident.is_some();
        let positional_args = if let Some(var_positional_args_ident) = var_positional_args_ident {
            if positional_args_idents.is_empty() {
                quote::quote! {
                    #var_positional_args_ident
                }
            } else {
                let n_args_fixed = positional_args_idents.len();
                quote::quote! {
                    {
                        let mut __internal__args = Vec::with_capacity(#n_args_fixed + ::pyo3::types::PyTupleMethods::len(#var_positional_args_ident));
                        __internal__args.extend([#(::pyo3::IntoPyObjectExt::into_bound_py_any(#positional_args_idents, py)?),*]);
                        __internal__args.extend(::pyo3::types::PyTupleMethods::iter(#var_positional_args_ident).map(|__internal__arg| __internal__arg.into_any()));
                        ::pyo3::types::PyTuple::new(
                            py,
                            __internal__args,
                        )?
                    }
                }
            }
        } else if positional_args_idents.is_empty() {
            quote::quote! {
                ()
            }
        } else {
            quote::quote! {
                ::pyo3::types::PyTuple::new(
                    py,
                    [#(::pyo3::IntoPyObjectExt::into_bound_py_any(#positional_args_idents, py)?),*],
                )?
            }
        };
        // Function body: keyword args
        let keyword_args: Vec<&Parameter> = self
            .parameters
            .iter()
            .filter(|param| [ParameterKind::KeywordOnly].contains(&param.kind))
            .collect_vec();
        let keyword_args_names: Vec<&str> = keyword_args
            .iter()
            .map(|param| param.name.as_py())
            .collect();
        let keyword_args_idents: Vec<syn::Ident> = keyword_args
            .iter()
            .map(|param| {
                Ok(
                    Ident::from_py(&format!("p_{}", param_binding_suffix(&param.name)))
                        .try_into()?,
                )
            })
            .collect::<Result<_>>()?;
        let var_keyword_args_ident: Option<syn::Ident> = self
            .parameters
            .iter()
            .find(|param| param.kind == ParameterKind::VarKeyword)
            .and_then(|param| {
                Ident::from_py(&format!("p_{}", param_binding_suffix(&param.name)))
                    .try_into()
                    .ok()
            });
        let has_keyword_args = !keyword_args_idents.is_empty() || var_keyword_args_ident.is_some();
        let keyword_args = if let Some(var_keyword_args_ident) = var_keyword_args_ident {
            if keyword_args_idents.is_empty() {
                quote::quote! {
                    #var_keyword_args_ident
                }
            } else {
                quote::quote! {
                    {
                        let __internal__kwargs =
                            ::pyo3::types::PyDictMethods::copy(&#var_keyword_args_ident)?;
                        #(
                            if ::pyo3::types::PyDictMethods::contains(
                                &__internal__kwargs,
                                ::pyo3::intern!(py, #keyword_args_names),
                            )? {
                                return ::std::result::Result::Err(
                                    ::pyo3::exceptions::PyTypeError::new_err(
                                        ::std::format!(
                                            "got multiple values for keyword argument '{}'",
                                            #keyword_args_names,
                                        ),
                                    ),
                                );
                            }
                            ::pyo3::types::PyDictMethods::set_item(&__internal__kwargs, ::pyo3::intern!(py, #keyword_args_names), #keyword_args_idents)?;
                        )*
                        __internal__kwargs
                    }
                }
            }
        } else if keyword_args_idents.is_empty() {
            quote::quote! {
                ::pyo3::types::PyDict::new(py)
            }
        } else {
            quote::quote! {
                {
                    let __internal__kwargs = ::pyo3::types::PyDict::new(py);
                    #(
                        ::pyo3::types::PyDictMethods::set_item(&__internal__kwargs, ::pyo3::intern!(py, #keyword_args_names), #keyword_args_idents)?;
                    )*
                    __internal__kwargs
                }
            }
        };
        // Function body: call
        let call = if let FunctionType::Method {
            typ: MethodType::Constructor | MethodType::Callable,
            ..
        } = &self.typ
        {
            if has_keyword_args {
                quote::quote! {
                    ::pyo3::types::PyAnyMethods::call(#function_dispatcher, #positional_args, Some(&#keyword_args))
                }
            } else if has_positional_args {
                quote::quote! {
                    ::pyo3::types::PyAnyMethods::call1(#function_dispatcher, #positional_args)
                }
            } else {
                quote::quote! {
                    ::pyo3::types::PyAnyMethods::call0(#function_dispatcher)
                }
            }
        } else {
            let method_name = self.name.name().as_py();
            if has_keyword_args {
                quote::quote! {
                    ::pyo3::types::PyAnyMethods::call_method(#function_dispatcher, ::pyo3::intern!(py, #method_name), #positional_args, Some(&#keyword_args))
                }
            } else if has_positional_args {
                quote::quote! {
                    ::pyo3::types::PyAnyMethods::call_method1(#function_dispatcher, ::pyo3::intern!(py, #method_name), #positional_args)
                }
            } else {
                quote::quote! {
                    ::pyo3::types::PyAnyMethods::call_method0(#function_dispatcher, ::pyo3::intern!(py, #method_name))
                }
            }
        };

        // Function body
        impl_fn.extend(quote::quote! {
            {
                #maybe_extract_py
                #param_preprocessing
                ::std::result::Result::Ok(::pyo3::types::PyAnyMethods::extract(
                    &#call?
                )?)
            }
        });

        Ok(match &self.typ {
            FunctionType::Method {
                typ: MethodType::InstanceMethod | MethodType::Callable,
                ..
            } => {
                if import_mode.uses_dedup_model() {
                    // External-IR path: the full method (with body) becomes a
                    // default-bodied trait method; the per-type impl stays empty.
                    // The newtype also gets an inherent forwarder to this trait
                    // method, so a call resolves unambiguously regardless of which
                    // ancestor `*Methods` traits the caller has in scope.
                    FunctionImplementation::Method(TraitMethod {
                        trait_fn: impl_fn,
                        impl_fn: TokenStream::new(),
                        forwarders: vec![Forwarder {
                            ident: method_ident,
                            params: quote::quote! { #(#param_idents: #param_types),* },
                            args: quote::quote! { #(#param_idents),* },
                            ret: return_type,
                        }],
                    })
                } else {
                    FunctionImplementation::Method(TraitMethod {
                        trait_fn: quote::quote! { #fn_contract ; },
                        impl_fn,
                        forwarders: Vec::new(),
                    })
                }
            }
            _ => FunctionImplementation::Function(impl_fn),
        })
    }
}

#[derive(Debug, Clone, serde::Serialize)]
struct Parameter {
    name: Ident,
    kind: ParameterKind,
    annotation: Type,
    has_default: bool,
}

impl PartialEq for Parameter {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
            && self.kind == other.kind
            && self.annotation == other.annotation
            && self.has_default == other.has_default
    }
}

impl Eq for Parameter {}

impl std::hash::Hash for Parameter {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.name.hash(state);
        self.kind.hash(state);
        self.annotation.hash(state);
        self.has_default.hash(state);
    }
}

impl FunctionType {
    /// Resolve a generic [`crate::schema::FunctionKindIr`] into a [`FunctionType`],
    /// supplying the owning class path for methods (ignored for free functions).
    pub(crate) fn from_ir_kind(
        kind: &crate::schema::FunctionKindIr,
        class_path: Option<&Path>,
    ) -> Self {
        use crate::schema::{FunctionKindIr, MethodKindIr};
        match kind {
            FunctionKindIr::Function => Self::Function,
            FunctionKindIr::Closure => Self::Closure,
            FunctionKindIr::Method { method_type } => Self::Method {
                class_path: class_path.cloned().unwrap_or_default(),
                typ: match method_type {
                    MethodKindIr::Instance => MethodType::InstanceMethod,
                    MethodKindIr::Class => MethodType::ClassMethod,
                    MethodKindIr::Static => MethodType::StaticMethod,
                    MethodKindIr::Constructor => MethodType::Constructor,
                    MethodKindIr::Callable => MethodType::Callable,
                    MethodKindIr::Unknown => MethodType::Unknown,
                },
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
enum ParameterKind {
    PositionalOnly,
    PositionalOrKeyword,
    VarPositional,
    KeywordOnly,
    VarKeyword,
}

impl TryFrom<u8> for ParameterKind {
    type Error = crate::PyBindgenError;

    fn try_from(kind: u8) -> std::result::Result<Self, Self::Error> {
        match kind {
            0 => Ok(Self::PositionalOnly),
            1 => Ok(Self::PositionalOrKeyword),
            2 => Ok(Self::VarPositional),
            3 => Ok(Self::KeywordOnly),
            4 => Ok(Self::VarKeyword),
            other => Err(crate::PyBindgenError::CodegenError(format!(
                "unsupported Python parameter kind {other} reported by `inspect.Parameter.kind`; \
                 expected 0-4 (positional-only, positional-or-keyword, var-positional, \
                 keyword-only, var-keyword). This can happen with a custom `__signature__`."
            ))),
        }
    }
}

impl ParameterKind {
    /// Map a generic [`crate::schema::ParameterKindIr`] to the internal kind.
    fn from_ir(kind: crate::schema::ParameterKindIr) -> Self {
        use crate::schema::ParameterKindIr;
        match kind {
            ParameterKindIr::PositionalOnly => Self::PositionalOnly,
            ParameterKindIr::PositionalOrKeyword => Self::PositionalOrKeyword,
            ParameterKindIr::VarPositional => Self::VarPositional,
            ParameterKindIr::KeywordOnly => Self::KeywordOnly,
            ParameterKindIr::VarKeyword => Self::VarKeyword,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a plain positional-or-keyword parameter from its Python spelling.
    fn py_param(name: &str) -> Parameter {
        Parameter {
            name: Ident::from_py(name),
            kind: ParameterKind::PositionalOrKeyword,
            annotation: Type::Unknown,
            has_default: false,
        }
    }

    #[test]
    fn partialmethod_source_supports_legacy_and_dunder_metadata() {
        pyo3::Python::initialize();
        pyo3::Python::attach(|py| {
            let globals = PyDict::new(py);
            py.run(
                pyo3::ffi::c_str!(
                    "import functools\n\
                     exec('async def target():\\n    pass')\n\
                     legacy_helper = lambda: None\n\
                     legacy_helper._partialmethod = functools.partialmethod(target)\n\
                     dunder_helper = lambda: None\n\
                     dunder_helper.__partialmethod__ = functools.partialmethod(target)\n\
                     partialmethod = type(\n\
                         'partialmethod', (), {'__module__': 'functools'}\n\
                     )\n\
                     spoof_metadata = partialmethod()\n\
                     spoof_metadata.func = target\n\
                     fallback_helper = lambda: None\n\
                     fallback_helper.__partialmethod__ = spoof_metadata\n\
                     fallback_helper._partialmethod = functools.partialmethod(target)\n\
                     spoof_helper = lambda: None\n\
                     spoof_helper.__partialmethod__ = spoof_metadata\n"
                ),
                Some(&globals),
                None,
            )
            .unwrap();

            let target = globals.get_item("target").unwrap().unwrap();
            let inspect = py.import(pyo3::intern!(py, "inspect")).unwrap();
            for helper_name in ["legacy_helper", "dunder_helper", "fallback_helper"] {
                let helper = globals.get_item(helper_name).unwrap().unwrap();
                let source = Function::partialmethod_source(&helper)
                    .unwrap_or_else(|| panic!("{helper_name} source was not found"));
                assert!(source.is(&target), "{helper_name} chose the wrong source");

                let chain = Function::signature_annotation_chain(&inspect, &helper, false);
                assert!(
                    chain
                        .as_deref()
                        .and_then(|chain| chain.last())
                        .is_some_and(|source| source.is(&target)),
                    "{helper_name} annotation chain did not reach its source"
                );
                assert!(
                    Function::signature_returns_async_handle(&inspect, &helper).unwrap(),
                    "{helper_name} async traversal did not reach its source"
                );
            }

            let spoof_helper = globals.get_item("spoof_helper").unwrap().unwrap();
            assert!(
                Function::partialmethod_source(&spoof_helper).is_none(),
                "writable type name/module metadata must not impersonate functools.partialmethod",
            );
            assert!(
                !Function::signature_returns_async_handle(&inspect, &spoof_helper).unwrap(),
                "spoofed partialmethod metadata must not affect async classification",
            );
        });
    }

    #[test]
    fn functools_partial_source_requires_exact_type_identity() {
        pyo3::Python::initialize();
        pyo3::Python::attach(|py| {
            let globals = PyDict::new(py);
            py.run(
                pyo3::ffi::c_str!(
                    "import functools\n\
                     target = lambda: None\n\
                     exact = functools.partial(target)\n\
                     partial = type('partial', (), {'__module__': 'functools'})\n\
                     spoof = partial()\n\
                     spoof.func = target\n"
                ),
                Some(&globals),
                None,
            )
            .unwrap();

            let target = globals.get_item("target").unwrap().unwrap();
            let exact = globals.get_item("exact").unwrap().unwrap();
            let spoof = globals.get_item("spoof").unwrap().unwrap();
            assert!(
                Function::functools_partial_source(&exact).is_some_and(|source| source.is(&target)),
                "an exact functools.partial must expose its callable source",
            );
            assert!(
                Function::functools_partial_source(&spoof).is_none(),
                "writable type name/module metadata must not impersonate functools.partial",
            );
        });
    }

    #[test]
    fn bound_method_and_globals_metadata_require_exact_runtime_objects() {
        pyo3::Python::initialize();
        pyo3::Python::attach(|py| {
            let globals = PyDict::new(py);
            let source = std::ffi::CString::new(indoc::indoc! {r#"
                async def async_target(*args):
                    return 1

                def sync_target(self=None):
                    return 1

                class Owner:
                    method = sync_target

                bound = Owner().method
                sync_target.__func__ = async_target
                sync_target.__self__ = Owner()

                class SpoofCallable:
                    __func__ = async_target
                    __self__ = Owner()
                    __globals__ = {"int": object()}

                    def __call__(self):
                        return 1

                spoof_callable = SpoofCallable()
            "#})
            .unwrap();
            py.run(&source, Some(&globals), None).unwrap();
            let inspect = py.import(pyo3::intern!(py, "inspect")).unwrap();
            let sync_target = globals.get_item("sync_target").unwrap().unwrap();
            let bound = globals.get_item("bound").unwrap().unwrap();
            let spoof_callable = globals.get_item("spoof_callable").unwrap().unwrap();

            assert!(
                Function::exact_bound_method_function(&bound)
                    .is_some_and(|function| function.as_any().is(&sync_target)),
                "a genuine Python bound method must expose its exact function",
            );
            assert!(
                Function::exact_bound_method_function(&sync_target).is_none(),
                "writable __func__/__self__ attributes must not turn a plain function into a method",
            );
            assert!(
                !Function::signature_returns_async_handle(&inspect, &sync_target).unwrap(),
                "a spoofed async __func__ must not suppress a synchronous callable",
            );
            assert!(
                Function::callable_globals(&bound).is_some(),
                "a genuine bound method must retain its defining globals",
            );
            assert!(
                Function::callable_globals(&spoof_callable).is_none(),
                "writable __globals__/__func__ attributes on a callable object are untrusted",
            );
        });
    }

    #[test]
    fn callable_projection_binds_descriptors_like_runtime_special_lookup() {
        pyo3::Python::initialize();
        pyo3::Python::attach(|py| {
            let globals = PyDict::new(py);
            let source = std::ffi::CString::new(indoc::indoc! {r#"
                import inspect

                async def async_target(self, *args):
                    return 1

                def sync_target(self, *args):
                    return 1

                class SwitchingDescriptor:
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

                class RuntimeAsync:
                    __call__ = SwitchingDescriptor(sync_target, async_target, sync_target)

                class RuntimeSync:
                    __call__ = SwitchingDescriptor(async_target, sync_target, async_target)

                class RaisingDescriptor:
                    def __get__(self, instance, owner):
                        raise RuntimeError("hostile binding")

                class Broken:
                    __call__ = RaisingDescriptor()

                runtime_async = RuntimeAsync()
                runtime_sync = RuntimeSync()
                broken = Broken()
                async_result = runtime_async()
                runtime_async_is_coroutine = inspect.iscoroutine(async_result)
                async_result.close()
                runtime_sync_result = runtime_sync()
            "#})
            .unwrap();
            py.run(&source, Some(&globals), None).unwrap();
            let inspect = py.import(pyo3::intern!(py, "inspect")).unwrap();
            let runtime_async = globals.get_item("runtime_async").unwrap().unwrap();
            let runtime_sync = globals.get_item("runtime_sync").unwrap().unwrap();
            let broken = globals.get_item("broken").unwrap().unwrap();

            assert!(
                Function::property_accessor_returns_async_handle(&inspect, &runtime_async).unwrap(),
                "instance-bound async descriptor target must classify async",
            );
            assert!(
                !Function::property_accessor_returns_async_handle(&inspect, &runtime_sync).unwrap(),
                "class-view async descriptor target must not override the runtime sync target",
            );
            assert!(
                Function::property_accessor_returns_async_handle(&inspect, &broken).unwrap(),
                "raising descriptor binding must fail conservative instead of aborting generation",
            );
            assert!(
                globals
                    .get_item("runtime_async_is_coroutine")
                    .unwrap()
                    .unwrap()
                    .is_truthy()
                    .unwrap(),
                "runtime descriptor control must actually return a coroutine",
            );
            assert_eq!(
                globals
                    .get_item("runtime_sync_result")
                    .unwrap()
                    .unwrap()
                    .extract::<i64>()
                    .unwrap(),
                1,
            );
        });
    }

    #[test]
    fn transparent_wrappers_project_async_callable_objects_and_cap_fresh_chains() {
        pyo3::Python::initialize();
        pyo3::Python::attach(|py| {
            let _runtime_permit = crate::io_utils::acquire_python_runtime(py).unwrap();
            let globals = PyDict::new(py);
            let source = std::ffi::CString::new(indoc::indoc! {r#"
                import functools

                class AsyncCallable:
                    async def __call__(self, value: int = 1) -> int:
                        return value

                async_callable = AsyncCallable()
                exact_partial = functools.partial(async_callable, 2)

                def proxy(*args, **kwargs):
                    return async_callable(*args, **kwargs)

                proxy.__wrapped__ = async_callable

                class Holder:
                    method = functools.partialmethod(async_callable, 3)

                unbound_partialmethod = Holder.method
                bound_partialmethod = Holder().method

                class Endless:
                    @property
                    def __wrapped__(self):
                        return Endless()

                endless = Endless()
            "#})
            .unwrap();
            py.run(&source, Some(&globals), None).unwrap();
            let inspect = py.import(pyo3::intern!(py, "inspect")).unwrap();

            for name in [
                "exact_partial",
                "proxy",
                "unbound_partialmethod",
                "bound_partialmethod",
            ] {
                let callable = globals.get_item(name).unwrap().unwrap();
                assert!(
                    Function::signature_returns_async_handle(&inspect, &callable).unwrap(),
                    "{name} did not project its transparent async callable-object source",
                );
            }
            let async_callable = globals.get_item("async_callable").unwrap().unwrap();
            assert!(
                !Function::signature_returns_async_handle(&inspect, &async_callable).unwrap(),
                "an arbitrary module-level callable object is not a transparent wrapper root",
            );

            let exact_partial = globals.get_item("exact_partial").unwrap().unwrap();
            let chain =
                Function::signature_annotation_chain(&inspect, &exact_partial, false).unwrap();
            assert!(
                Function::signature_annotation_globals(&chain).is_some(),
                "partial callable-object annotations must reach the type-owned call globals",
            );

            let endless = globals.get_item("endless").unwrap().unwrap();
            assert!(
                Function::signature_annotation_chain(&inspect, &endless, false).is_none(),
                "fresh dynamic wrapper chains must stop at the shared traversal cap",
            );
            let mut diagnostics = Vec::new();
            let annotation = PyString::new(py, "int").into_any();
            assert_eq!(
                Function::parse_signature_annotation_or_unknown(
                    annotation,
                    None,
                    &mut diagnostics,
                    &Path::from_py("mod.endless"),
                    "Endless return annotation".to_owned(),
                ),
                Type::Unknown,
            );
            assert_eq!(
                diagnostics
                    .iter()
                    .filter(|diagnostic| {
                        diagnostic.code.as_str() == "signature_annotation_source_untrusted"
                    })
                    .count(),
                1,
            );
        });
    }

    /// Two parameters that differ only by Unicode normalization bind the same `p_`
    /// identifier to rustc (which canonicalizes to NFC), so the later one must be
    /// renamed -- while keeping its real Python keyword reachable via `as_py()`.
    #[test]
    fn nfc_parameter_binding_collision_is_renamed() {
        let mut parameters = vec![py_param("\u{e9}"), py_param("e\u{301}")];
        let mut typ = FunctionType::Function;
        let mut diagnostics = Vec::new();

        let first_rs = parameters[0].name.as_rs().to_owned();
        let second_rs = parameters[1].name.as_rs().to_owned();

        Function::finalize_parameters(
            &mut parameters,
            &mut typ,
            &Path::from_py("pkg.run"),
            &mut diagnostics,
        );

        // The FIRST occurrence always keeps its binding; only later members of a
        // collision are renamed. This pins the rename to signature order.
        assert_eq!(parameters[0].name.as_rs(), first_rs);
        assert_eq!(parameters[1].name.as_rs(), format!("{second_rs}_"));
        // Only the Rust-side spelling changed: both keyword names are intact.
        assert_eq!(parameters[0].name.as_py(), "\u{e9}");
        assert_eq!(parameters[1].name.as_py(), "e\u{301}");
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(
            diagnostics[0].code.as_str(),
            "function_parameter_binding_collision"
        );
    }

    /// Byte-neutrality guard: `canonical_rust_name` is the identity on ASCII, so an
    /// ASCII-only signature -- every real target -- must round-trip through
    /// `finalize_parameters` with exactly the names (and the zero diagnostics) it
    /// produced before the collision pass existed, including the `r#`-escaped keyword
    /// spellings and the synthetic `_arg{N}` allocation the pass runs after.
    #[test]
    fn ascii_parameters_are_untouched_by_binding_collision_pass() {
        let mut parameters = ["value", "self", "type", "_", "_arg0"]
            .into_iter()
            .map(py_param)
            .collect::<Vec<_>>();
        // A static method keeps every parameter: no receiver strip, no `self` rewrite.
        let mut typ = FunctionType::Method {
            class_path: Path::from_py("pkg.Cls"),
            typ: MethodType::StaticMethod,
        };
        let mut diagnostics = Vec::new();

        Function::finalize_parameters(
            &mut parameters,
            &mut typ,
            &Path::from_py("pkg.Cls.run"),
            &mut diagnostics,
        );

        assert_eq!(
            parameters
                .iter()
                .map(|param| param.name.as_rs())
                .collect::<Vec<_>>(),
            ["value", "r#self", "r#type", "_arg3", "_arg0"]
        );
        assert_eq!(
            parameters
                .iter()
                .map(|param| param.name.as_py())
                .collect::<Vec<_>>(),
            ["value", "self", "type", "_", "_arg0"]
        );
        assert!(diagnostics.is_empty(), "unexpected: {diagnostics:?}");
    }

    #[test]
    fn postponed_builtin_shadow_rewrite_uses_exact_globals_bindings() {
        pyo3::Python::initialize();
        pyo3::Python::attach(|py| {
            let globals = PyDict::new(py);
            globals.set_item("__name__", "fixture").unwrap();

            assert_eq!(
                Function::rewrite_shadowed_builtin_annotation(py, "int", &globals).unwrap(),
                None,
                "an absent local binding must preserve the builtin annotation"
            );

            globals
                .set_item("int", pyo3::types::PyType::new::<pyo3::types::PyInt>(py))
                .unwrap();
            assert_eq!(
                Function::rewrite_shadowed_builtin_annotation(py, "int", &globals).unwrap(),
                None,
                "an exact builtin binding must remain builtin"
            );

            // Any other class object bound under `int` shadows the builtin. The live
            // integration test below supplies a real module-defined `class int`.
            globals
                .set_item("int", pyo3::types::PyType::new::<pyo3::types::PyString>(py))
                .unwrap();
            assert_eq!(
                Function::rewrite_shadowed_builtin_annotation(
                    py,
                    "dict[str, list[int]] | None",
                    &globals,
                )
                .unwrap()
                .as_deref(),
                Some("dict[str, list[str]] | None"),
                "an alias to an exact builtin type must use that type's canonical spelling"
            );
            globals
                .set_item("List", pyo3::types::PyType::new::<pyo3::types::PyList>(py))
                .unwrap();
            globals
                .set_item("int", pyo3::types::PyType::new::<pyo3::types::PyInt>(py))
                .unwrap();
            globals
                .set_item(
                    "Sequence",
                    pyo3::types::PyType::new::<pyo3::types::PyList>(py),
                )
                .unwrap();
            assert_eq!(
                Function::rewrite_shadowed_builtin_annotation(
                    py,
                    "List[int] | Sequence[str]",
                    &globals,
                )
                .unwrap()
                .as_deref(),
                Some("list[int] | list[str]"),
                "capitalized and protocol aliases to a builtin must retain builtin semantics"
            );

            py.run(
                pyo3::ffi::c_str!("class int:\n    pass"),
                Some(&globals),
                None,
            )
            .unwrap();
            assert_eq!(
                Function::rewrite_shadowed_builtin_annotation(py, "list[int]", &globals)
                    .unwrap()
                    .as_deref(),
                Some("list[fixture.int]"),
                "a genuine local class must use the module-qualified binding"
            );
            py.run(
                pyo3::ffi::c_str!("class Annotated:\n    pass\nAnnotated.__module__ = 'typing'"),
                Some(&globals),
                None,
            )
            .unwrap();
            assert_eq!(
                Function::rewrite_shadowed_builtin_annotation(
                    py,
                    "Annotated[int, 'metadata']",
                    &globals,
                )
                .unwrap()
                .as_deref(),
                Some("fixture.Annotated[fixture.int, 'metadata']"),
                "writable type metadata must not impersonate the genuine typing export"
            );
            if let Ok(typing) = py.import(pyo3::intern!(py, "typing")) {
                if let Ok(list) = typing.getattr(pyo3::intern!(py, "List")) {
                    globals.set_item("List", list).unwrap();
                    assert_eq!(
                        Function::rewrite_shadowed_builtin_annotation(py, "List[int]", &globals,)
                            .unwrap()
                            .as_deref(),
                        Some("typing.List[fixture.int]"),
                        "a genuine non-type typing export must retain typing semantics"
                    );
                }
                if let Ok(annotated) = typing.getattr(pyo3::intern!(py, "Annotated")) {
                    globals.set_item("Annotated", annotated).unwrap();
                    assert_eq!(
                        Function::rewrite_shadowed_builtin_annotation(
                            py,
                            "Annotated[int, 'metadata']",
                            &globals,
                        )
                        .unwrap()
                        .as_deref(),
                        Some("typing.Annotated[fixture.int, 'metadata']"),
                        "the genuine typing export must retain Annotated metadata semantics"
                    );
                }
            }
            assert_eq!(
                Function::rewrite_shadowed_builtin_annotation(py, "builtins.int", &globals)
                    .unwrap(),
                None,
                "an explicitly qualified builtin must not resolve through globals"
            );
            assert_eq!(
                Function::rewrite_shadowed_builtin_annotation(py, "'int'", &globals)
                    .unwrap()
                    .as_deref(),
                Some("'fixture.int'"),
                "an explicitly quoted forward reference must resolve too"
            );
            globals
                .set_item(
                    "ForwardRef",
                    pyo3::types::PyType::new::<pyo3::types::PyString>(py),
                )
                .unwrap();
            assert_eq!(
                Function::rewrite_shadowed_builtin_annotation(
                    py,
                    "ForwardRef(\"User\")",
                    &globals,
                )
                .unwrap(),
                None,
                "typing wrapper syntax must not be mistaken for a shadowable type name"
            );
            assert_eq!(
                Function::rewrite_shadowed_builtin_annotation(py, "πint", &globals).unwrap(),
                None,
                "an ASCII builtin suffix inside a Unicode identifier is not a separate token"
            );
        });
    }
}
