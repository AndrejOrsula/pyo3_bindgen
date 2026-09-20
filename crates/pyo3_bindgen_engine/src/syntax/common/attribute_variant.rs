use crate::{
    runtime_sentinels::{self, StdlibModule},
    syntax::Path,
    Result,
};
use pyo3::prelude::*;

#[derive(Clone, Copy)]
pub(crate) enum AttributeVariant {
    Import,
    Module,
    Class,
    Function,
    Method,
    Closure,
    TypeVar,
    Property,
}

impl AttributeVariant {
    pub(crate) fn determine(
        py: pyo3::prelude::Python,
        attr: &pyo3::Bound<pyo3::prelude::PyAny>,
        attr_type: &pyo3::Bound<pyo3::types::PyType>,
        attr_module: &Path,
        owner_name: &Path,
        consider_import: bool,
    ) -> Result<Self> {
        // Cheapest signals first: C-level subclass-flag checks, no Python-level call.
        let is_submodule = attr_type
            .is_subclass_of::<pyo3::types::PyModule>()
            .unwrap_or(false);
        let is_class = attr_type
            .is_subclass_of::<pyo3::types::PyType>()
            .unwrap_or(false);
        let is_builtin_function = attr_type
            .is_subclass_of::<pyo3::types::PyCFunction>()
            .unwrap_or(false);
        let is_builtin_method_descriptor =
            !consider_import && is_builtin_method_descriptor(py, attr)?;

        // `inspect.isfunction`/`ismethod` are GIL round-trips; evaluate each at most once,
        // and only when the decision actually reaches for it -- a submodule or class is
        // settled by the cheap flags above without ever calling `inspect`.
        let mut function_flag: Option<bool> = None;
        let mut method_flag: Option<bool> = None;

        // Import classification (module path only), ordered cheapest-first: the module
        // comparison and the C-level flags gate the `inspect` calls, and `str(attr)` --
        // which can be arbitrarily expensive for a heavy `__repr__` and is never consulted
        // off the import path -- is evaluated last, only once everything else has matched.
        // Boolean `&&`/`||` are commutative in value, so this yields exactly the original
        // `consider_import && is_external && (is_submodule || is_class || is_function ||
        // is_method)` (with `is_external = !is_in_locals && attr_module != owner_name`).
        let is_import = consider_import
            && attr_module != owner_name
            && (is_submodule
                || is_class
                || cached_flag(&mut function_flag, || is_function(py, attr))?
                || cached_flag(&mut method_flag, || is_method(py, attr))?)
            && !attr.to_string().contains("<locals>");
        if is_import {
            return Ok(Self::Import);
        }
        if is_submodule {
            return Ok(Self::Module);
        }
        if is_class {
            return Ok(Self::Class);
        }
        if is_builtin_function || cached_flag(&mut function_flag, || is_function(py, attr))? {
            return Ok(Self::Function);
        }
        if is_builtin_method_descriptor || cached_flag(&mut method_flag, || is_method(py, attr))? {
            return Ok(Self::Method);
        }

        // Closure and type-variable classification need the attribute type's defining
        // module and name. Fetch them lazily here -- the common submodule/class/function/
        // method attributes returned above never consult them.
        // `attr_type.name()` can be empty/absent for a pathological metaclass; keep it a raw
        // string (an empty one is harmless here) rather than `Ident::from_py`, which
        // debug-asserts on empty input. It is only compared against the literal "partial".
        let attr_type_name = attr_type.name().map(|n| n.to_string()).unwrap_or_default();
        let attr_type_module = Path::from_py(
            &attr_type
                .getattr(pyo3::intern!(py, "__module__"))
                .map(|a| a.to_string())
                .unwrap_or_default(),
        );

        let attr_type_module = attr_type_module.to_py();
        let is_closure = attr_type_module.as_str() == "functools"
            && attr_type_name == "partial"
            && Self::is_exact_functools_partial(py, attr)?;
        if is_closure {
            return Ok(Self::Closure);
        }

        // Type-variable-like attributes are the annotation machinery a module re-exports:
        // `TypeVar`/`ParamSpec`/`NewType` instances, subscripted generic aliases, special
        // forms, and PEP 604 unions. `typing` and `typing_extensions` export essentially
        // nothing but that machinery, so the defining module of the attribute's type is a
        // sound signal there. This includes backported metadata such as `Self` and
        // `TypeAliasType`. `types` is NOT: it is mostly ordinary runtime objects (a plain
        // `config = types.SimpleNamespace(...)` must stay a property), so its type-like
        // members are matched positively by instance -- `types.UnionType` (`int | str`) and
        // `types.GenericAlias` (`list[int]`, PEP 585 generics) -- via `is_union_type` /
        // `is_generic_alias`.
        let is_type = matches!(attr_type_module.as_str(), "typing" | "typing_extensions")
            || is_union_type(py, attr)
            || is_generic_alias(py, attr);
        if is_type {
            return Ok(Self::TypeVar);
        }

        Ok(Self::Property)
    }

    /// Exact runtime identity for the only closure object supported by generation.
    ///
    /// Both `type.__name__` and `type.__module__` are writable on user-defined classes;
    /// they remain a cheap candidate filter in [`Self::determine`], but cannot authorize
    /// closure lowering. This cached canonical type object is shared with function
    /// signature/async traversal.
    pub(crate) fn is_exact_functools_partial(
        py: Python<'_>,
        attr: &Bound<'_, PyAny>,
    ) -> PyResult<bool> {
        let partial = crate::runtime_sentinels::functools_partial(py)?;
        Ok(attr.get_type().as_any().is(&partial))
    }

    pub(crate) fn is_exact_functools_partialmethod(
        py: Python<'_>,
        attr: &Bound<'_, PyAny>,
    ) -> PyResult<bool> {
        let partialmethod = crate::runtime_sentinels::functools_partialmethod(py)?;
        Ok(attr.get_type().as_any().is(&partialmethod))
    }
}

/// Return a memoized boolean, computing it once on first request. The backing signals are
/// independent `inspect` GIL round-trips, so this avoids both recomputation and -- when the
/// classification returns before the flag is ever needed -- any evaluation at all.
fn cached_flag(slot: &mut Option<bool>, compute: impl FnOnce() -> Result<bool>) -> Result<bool> {
    match slot {
        Some(value) => Ok(*value),
        None => {
            let value = compute()?;
            *slot = Some(value);
            Ok(value)
        }
    }
}

fn is_function(py: Python<'_>, attr: &Bound<PyAny>) -> Result<bool> {
    Ok(crate::runtime_sentinels::inspect_isfunction(py)?
        .call1((attr,))?
        .is_truthy()?)
}

fn is_method(py: Python<'_>, attr: &Bound<PyAny>) -> Result<bool> {
    Ok(crate::runtime_sentinels::inspect_ismethod(py)?
        .call1((attr,))?
        .is_truthy()?)
}

/// Class-level C methods such as `list.append` and slot wrappers such as
/// `object.__init__` are descriptors, not `inspect.ismethod`/`inspect.isfunction`
/// objects. Classify their exact runtime types as methods before they fall through to
/// properties. The corresponding `types.*DescriptorType` module attributes are
/// writable, so classify against CPython's canonical type objects.
fn is_builtin_method_descriptor(_py: Python<'_>, attr: &Bound<PyAny>) -> Result<bool> {
    Ok(unsafe {
        pyo3::ffi::PyObject_TypeCheck(
            attr.as_ptr(),
            std::ptr::addr_of_mut!(pyo3::ffi::PyMethodDescr_Type),
        ) != 0
            || pyo3::ffi::PyObject_TypeCheck(
                attr.as_ptr(),
                std::ptr::addr_of_mut!(pyo3::ffi::PyWrapperDescr_Type),
            ) != 0
            || pyo3::ffi::PyObject_TypeCheck(
                attr.as_ptr(),
                std::ptr::addr_of_mut!(pyo3::ffi::PyClassMethodDescr_Type),
            ) != 0
    })
}

/// Whether `attr` is a PEP 604 union value (`int | str`), matched by *identity* against the
/// pre-target `types.UnionType`. Identity, not `isinstance`: Python 3.14 aliases
/// `types.UnionType` to `typing.Union`, and `isinstance(x, typing.Union)` raises
/// `TypeError` (which would silently degrade to `false`); `UnionType` has no subclasses, so
/// identity equals `isinstance` on every earlier version. A missing `types.UnionType`
/// (older interpreters) degrades to `false`, exactly as the previous inline code did.
fn is_union_type(py: Python<'_>, attr: &Bound<PyAny>) -> bool {
    runtime_sentinels::optional_attr(py, StdlibModule::Types, "UnionType").is_ok_and(|union_type| {
        union_type.is_some_and(|union_type| attr.get_type().is(&union_type))
    })
}

/// Whether `attr` is a PEP 585 subscripted generic (`list[int]`, `dict[str, int]`),
/// matched by `isinstance` against the pre-target `types.GenericAlias`. Deliberately
/// classified type-like even though it is not a subclass of `type`, so a builtin generic is
/// treated the same as the equivalent `typing.List[int]`. A missing sentinel degrades to
/// `false`, as the previous inline code did.
fn is_generic_alias(py: Python<'_>, attr: &Bound<PyAny>) -> bool {
    runtime_sentinels::attr(py, StdlibModule::Types, "GenericAlias")
        .is_ok_and(|generic_alias| attr.is_instance(&generic_alias).unwrap_or(false))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pyo3::types::{PyAnyMethods, PyDict, PyDictMethods};

    #[test]
    fn closure_classification_requires_exact_functools_partial_type() {
        pyo3::Python::initialize();
        pyo3::Python::attach(|py| {
            let globals = PyDict::new(py);
            py.run(
                pyo3::ffi::c_str!(
                    "import functools\n\
                     real = functools.partial(lambda: None)\n\
                     partial = type('partial', (), {'__module__': 'functools'})\n\
                     spoof = partial()\n"
                ),
                Some(&globals),
                None,
            )
            .unwrap();

            let functools = Path::from_py("functools");
            let owner = Path::from_py("fixture");
            let determine = |name: &str| {
                let value = globals.get_item(name).unwrap().unwrap();
                AttributeVariant::determine(
                    py,
                    &value,
                    &value.get_type(),
                    &functools,
                    &owner,
                    false,
                )
                .unwrap()
            };
            assert!(
                matches!(determine("real"), AttributeVariant::Closure),
                "an exact functools.partial must remain a generated closure",
            );
            assert!(
                matches!(determine("spoof"), AttributeVariant::Property),
                "writable type name/module metadata must not impersonate functools.partial",
            );
        });
    }

    #[test]
    fn builtin_descriptor_classification_ignores_mutated_types_sentinels() {
        pyo3::Python::initialize();
        pyo3::Python::attach(|py| {
            crate::io_utils::with_suppressed_python_output(py, false, false, || {
                crate::runtime_sentinels::prime(py)?;
                let types = py.import(pyo3::intern!(py, "types"))?;
                let names = [
                    "MethodDescriptorType",
                    "WrapperDescriptorType",
                    "ClassMethodDescriptorType",
                ];
                let _restore = names
                    .iter()
                    .map(|name| {
                        crate::runtime_sentinels::TestAttributeRestore::new(py, &types, name)
                    })
                    .collect::<PyResult<Vec<_>>>()?;
                for name in names {
                    types.setattr(name, py.get_type::<pyo3::types::PyAny>())?;
                }

                let inspect = py.import(pyo3::intern!(py, "inspect"))?;
                let method = inspect.call_method1(
                    pyo3::intern!(py, "getattr_static"),
                    (py.get_type::<pyo3::types::PyList>(), "append"),
                )?;
                let wrapper = inspect.call_method1(
                    pyo3::intern!(py, "getattr_static"),
                    (py.get_type::<pyo3::types::PyAny>(), "__init__"),
                )?;
                let class_method = inspect.call_method1(
                    pyo3::intern!(py, "getattr_static"),
                    (py.get_type::<pyo3::types::PyDict>(), "fromkeys"),
                )?;
                let results = [&method, &wrapper, &class_method]
                    .map(|descriptor| is_builtin_method_descriptor(py, descriptor).unwrap());

                assert!(
                    results.into_iter().all(std::convert::identity),
                    "canonical C descriptor types must survive mutable `types` aliases",
                );
                Ok(())
            })
            .unwrap();
        });
    }

    #[test]
    fn type_like_classification_ignores_mutated_types_sentinels() {
        pyo3::Python::initialize();
        pyo3::Python::attach(|py| {
            crate::io_utils::with_suppressed_python_output(py, false, false, || {
                crate::runtime_sentinels::prime(py)?;
                let types = py.import(pyo3::intern!(py, "types"))?;
                let _restore = ["GenericAlias", "UnionType"]
                    .iter()
                    .map(|name| {
                        crate::runtime_sentinels::TestAttributeRestore::new(py, &types, name)
                    })
                    .collect::<PyResult<Vec<_>>>()?;
                let generic_type = types.getattr("GenericAlias").ok().map(Bound::unbind);
                let union_type = types.getattr("UnionType").ok().map(Bound::unbind);
                let generic = generic_type.as_ref().map(|_| {
                    py.eval(pyo3::ffi::c_str!("list[int]"), None, None)
                        .unwrap()
                        .unbind()
                });
                let union = union_type.as_ref().map(|_| {
                    py.eval(pyo3::ffi::c_str!("int | str"), None, None)
                        .unwrap()
                        .unbind()
                });
                let poison = py.get_type::<PyAny>();
                if generic_type.is_some() {
                    types.setattr("GenericAlias", &poison)?;
                }
                if union_type.is_some() {
                    types.setattr("UnionType", &poison)?;
                }

                let classified: PyResult<_> = crate::runtime_sentinels::with_restored(py, || {
                    Ok((
                        generic
                            .as_ref()
                            .map(|value| is_generic_alias(py, value.bind(py))),
                        union
                            .as_ref()
                            .map(|value| is_union_type(py, value.bind(py))),
                    ))
                });
                let generic_poison_restored = generic_type
                    .as_ref()
                    .is_none_or(|_| types.getattr("GenericAlias").unwrap().is(poison.as_any()));
                let union_poison_restored = union_type
                    .as_ref()
                    .is_none_or(|_| types.getattr("UnionType").unwrap().is(poison.as_any()));

                let (generic_classified, union_classified) = classified?;
                assert_ne!(
                    generic_classified,
                    Some(false),
                    "mutated types.GenericAlias changed generic-alias classification",
                );
                assert_ne!(
                    union_classified,
                    Some(false),
                    "mutated types.UnionType changed union classification",
                );
                assert!(
                    generic_poison_restored && union_poison_restored,
                    "canonical-global guard did not restore target types mutations",
                );
                Ok(())
            })
            .unwrap();
        });
    }
}
