use pyo3::types::{PyAnyMethods, PyModule, PyModuleMethods};
use pyo3_bindgen_engine::{Codegen, Config};

const HOSTILE_MODULE_SOURCE: &str = r#"
import types

class HostileModule(types.ModuleType):
    def __dir__(self):
        return ["_private", "testing", "dict_backed", "dynamic_failure"]

    def __getattribute__(self, name):
        if name in {"__all__", "_private", "testing", "dict_backed", "dynamic_failure", "all_only_missing"}:
            accesses = object.__getattribute__(self, "_accesses")
            accesses.append(name)
            raise RuntimeError(f"hostile attribute accessed: {name}")
        return types.ModuleType.__getattribute__(self, name)

hostile = HostileModule("pyo3_bindgen_access_filtering")
hostile._accesses = []
hostile.dict_backed = 42
hostile.__all__ = ["dynamic_failure", "all_only_missing", "testing", "dict_backed", "_private"]
"#;

const BROKEN_ALL_ACCESS_SOURCE: &str = r#"
import types

class BrokenAllAccess(types.ModuleType):
    def __dir__(self):
        return []

    def __getattribute__(self, name):
        if name == "__all__":
            raise RuntimeError("broken __all__ access")
        return types.ModuleType.__getattribute__(self, name)

broken = BrokenAllAccess("pyo3_bindgen_broken_all_access")
"#;

const BROKEN_ALL_ITERATION_SOURCE: &str = r#"
import types

class BrokenAllIteration:
    def __iter__(self):
        yield "present"
        raise RuntimeError("broken __all__ iteration")

broken = types.ModuleType("pyo3_bindgen_broken_all_iteration")
broken.present = 42
broken.__all__ = BrokenAllIteration()
"#;

const BROKEN_DIR_SOURCE: &str = r#"
import types

class BrokenDir(types.ModuleType):
    def __dir__(self):
        raise RuntimeError("intentional hostile __dir__ failure")

broken = BrokenDir("pyo3_bindgen_broken_dir")
broken.value = 42
"#;

const DUPLICATE_DIR_SOURCE: &str = r#"
import types

class DuplicateDir(types.ModuleType):
    def __dir__(self):
        return ["value", "value", "value"]

duplicate = DuplicateDir("pyo3_bindgen_duplicate_dir")
duplicate.value = 42
"#;

const MALFORMED_DIR_SOURCE: &str = r#"
import types

class MalformedDir(types.ModuleType):
    def __dir__(self):
        return ["", "value"]

malformed = MalformedDir("pyo3_bindgen_malformed_dir")
malformed.value = 42
"#;

fn module_from_factory<'py>(
    py: pyo3::Python<'py>,
    source: &str,
    file_name: &std::ffi::CStr,
    factory_name: &std::ffi::CStr,
    attr_name: &str,
) -> pyo3::PyResult<pyo3::Bound<'py, PyModule>> {
    let source = std::ffi::CString::new(source).unwrap();
    let factory = PyModule::from_code(py, source.as_c_str(), file_name, factory_name)?;
    factory
        .getattr(attr_name)?
        .cast_into::<PyModule>()
        .map_err(Into::into)
}

fn hostile_module(py: pyo3::Python<'_>) -> pyo3::PyResult<pyo3::Bound<'_, PyModule>> {
    module_from_factory(
        py,
        HOSTILE_MODULE_SOURCE,
        pyo3::ffi::c_str!("access_filtering_module.py"),
        pyo3::ffi::c_str!("access_filtering_module"),
        "hostile",
    )
}

fn assert_hostile_access_filtering(config: Config) {
    pyo3::Python::initialize();
    pyo3::Python::attach(|py| {
        Codegen::prime_live_runtime(py).unwrap();
        let module = hostile_module(py)?;
        assert!(
            module.dict().contains("dict_backed")?,
            "fixture must place the allowed attribute directly in the module dictionary"
        );
        let generated = Codegen::new(config)
            .module(&module)
            .expect("hostile module should remain parseable")
            .generate_with_report()
            .expect("hostile attribute failures should not abort generation");

        let accesses = module.getattr("_accesses")?.extract::<Vec<String>>()?;
        assert!(
            !accesses.iter().any(|name| name == "_private"),
            "RED: private module name was dynamically accessed: {accesses:?}"
        );
        assert!(
            !accesses.iter().any(|name| name == "testing"),
            "RED: blocklisted module name was dynamically accessed: {accesses:?}"
        );
        assert!(
            !accesses.iter().any(|name| name == "dict_backed"),
            "RED: dict-backed allowed module name used dynamic access: {accesses:?}"
        );
        assert!(
            !accesses.iter().any(|name| name == "__all__"),
            "RED: dict-backed __all__ used dynamic access: {accesses:?}"
        );
        assert_eq!(
            accesses
                .iter()
                .filter(|name| name.as_str() == "dynamic_failure")
                .count(),
            1,
            "a name shared by __all__ and __dir__ must be resolved exactly once: {accesses:?}"
        );
        assert_eq!(
            accesses
                .iter()
                .filter(|name| name.as_str() == "all_only_missing")
                .count(),
            1,
            "an allowed __all__-only name must be resolved exactly once: {accesses:?}"
        );
        assert_eq!(
            generated
                .report
                .diagnostics
                .iter()
                .filter(|diagnostic| {
                    diagnostic.code == "attribute_access_failed"
                        && diagnostic.path.as_deref() == Some("pyo3_bindgen_access_filtering")
                        && diagnostic.message.contains("dynamic_failure")
                })
                .count(),
            1,
            "a cached __all__ lookup failure must not be retried or reported by __dir__ traversal"
        );
        assert_eq!(
            generated
                .report
                .diagnostics
                .iter()
                .filter(|diagnostic| {
                    diagnostic.code == "attribute_access_failed"
                        && diagnostic.path.as_deref() == Some("pyo3_bindgen_access_filtering")
                        && diagnostic.message.contains("all_only_missing")
                })
                .count(),
            1,
            "an allowed __all__-only lookup failure needs one path-qualified diagnostic"
        );
        assert_eq!(
            generated.report.modules[0].counts.properties, 1,
            "the dict-backed allowed value must still be classified and emitted"
        );
        Ok::<_, pyo3::PyErr>(())
    })
    .unwrap();
}

#[test]
fn allowed_module_dict_attributes_bypass_dynamic_access() {
    assert_hostile_access_filtering(Config::default());
}

#[test]
fn allowed_module_dict_attributes_bypass_dynamic_access_with_dependencies_enabled() {
    assert_hostile_access_filtering(Config::builder().generate_dependencies(true).build());
}

#[test]
fn missing_all_remains_absent_from_an_ordinary_module() {
    pyo3::Python::initialize();
    pyo3::Python::attach(|py| {
        Codegen::prime_live_runtime(py).unwrap();
        let module = PyModule::new(py, "pyo3_bindgen_missing_all")?;
        assert!(!module.dict().contains("__all__")?);

        Codegen::default()
            .module(&module)
            .expect("a module without __all__ should remain parseable");

        assert!(
            !module.dict().contains("__all__")?,
            "introspection must not create a missing __all__"
        );
        Ok::<_, pyo3::PyErr>(())
    })
    .unwrap();
}

#[test]
fn non_attribute_all_access_failure_is_reported_once() {
    pyo3::Python::initialize();
    pyo3::Python::attach(|py| {
        Codegen::prime_live_runtime(py).unwrap();
        let module = module_from_factory(
            py,
            BROKEN_ALL_ACCESS_SOURCE,
            pyo3::ffi::c_str!("broken_all_access.py"),
            pyo3::ffi::c_str!("broken_all_access_factory"),
            "broken",
        )?;
        let generated = Codegen::default()
            .module(&module)
            .expect("a failing dynamic __all__ should degrade safely")
            .generate_with_report()
            .expect("a failing dynamic __all__ should not abort generation");

        assert_eq!(
            generated
                .report
                .diagnostics
                .iter()
                .filter(|diagnostic| {
                    diagnostic.code == "attribute_access_failed"
                        && diagnostic.path.as_deref() == Some("pyo3_bindgen_broken_all_access")
                        && diagnostic.message.contains("__all__")
                })
                .count(),
            1,
        );
        Ok::<_, pyo3::PyErr>(())
    })
    .unwrap();
}

#[test]
fn non_iterable_all_type_failure_is_reported_once() {
    pyo3::Python::initialize();
    pyo3::Python::attach(|py| {
        Codegen::prime_live_runtime(py).unwrap();
        let module = PyModule::new(py, "pyo3_bindgen_broken_all_type")?;
        module.setattr("__all__", 42)?;
        let generated = Codegen::default()
            .module(&module)
            .expect("a non-iterable __all__ should degrade safely")
            .generate_with_report()
            .expect("a non-iterable __all__ should not abort generation");

        assert_eq!(
            generated
                .report
                .diagnostics
                .iter()
                .filter(|diagnostic| {
                    diagnostic.code == "attribute_access_failed"
                        && diagnostic.path.as_deref() == Some("pyo3_bindgen_broken_all_type")
                        && diagnostic.message.contains("__all__")
                })
                .count(),
            1,
        );
        Ok::<_, pyo3::PyErr>(())
    })
    .unwrap();
}

#[test]
fn all_iteration_failure_is_reported_once() {
    pyo3::Python::initialize();
    pyo3::Python::attach(|py| {
        Codegen::prime_live_runtime(py).unwrap();
        let module = module_from_factory(
            py,
            BROKEN_ALL_ITERATION_SOURCE,
            pyo3::ffi::c_str!("broken_all_iteration.py"),
            pyo3::ffi::c_str!("broken_all_iteration_factory"),
            "broken",
        )?;
        let generated = Codegen::default()
            .module(&module)
            .expect("a failing __all__ iterator should degrade safely")
            .generate_with_report()
            .expect("a failing __all__ iterator should not abort generation");

        assert_eq!(
            generated
                .report
                .diagnostics
                .iter()
                .filter(|diagnostic| {
                    diagnostic.code == "attribute_access_failed"
                        && diagnostic.path.as_deref() == Some("pyo3_bindgen_broken_all_iteration")
                        && diagnostic.message.contains("__all__")
                })
                .count(),
            1,
        );
        Ok::<_, pyo3::PyErr>(())
    })
    .unwrap();
}

#[test]
fn hostile_module_dir_exception_remains_a_public_python_error() {
    pyo3::Python::initialize();
    pyo3::Python::attach(|py| {
        Codegen::prime_live_runtime(py).unwrap();
        let module = module_from_factory(
            py,
            BROKEN_DIR_SOURCE,
            pyo3::ffi::c_str!("broken_dir.py"),
            pyo3::ffi::c_str!("broken_dir_factory"),
            "broken",
        )?;
        let error = Codegen::default()
            .module(&module)
            .expect_err("a failing module __dir__ must reject module registration");

        match error {
            pyo3_bindgen_engine::PyBindgenError::PyError(error) => assert!(
                error
                    .to_string()
                    .contains("intentional hostile __dir__ failure"),
                "the public Python error must retain the __dir__ exception: {error}"
            ),
            error => panic!("a hostile __dir__ must remain a Python error: {error}"),
        }
        Ok::<_, pyo3::PyErr>(())
    })
    .unwrap();
}

#[test]
fn duplicate_module_dir_names_are_emitted_once() {
    pyo3::Python::initialize();
    pyo3::Python::attach(|py| {
        Codegen::prime_live_runtime(py).unwrap();
        let module = module_from_factory(
            py,
            DUPLICATE_DIR_SOURCE,
            pyo3::ffi::c_str!("duplicate_dir.py"),
            pyo3::ffi::c_str!("duplicate_dir_factory"),
            "duplicate",
        )?;
        let generated = Codegen::default()
            .module(&module)
            .expect("duplicate __dir__ names should remain parseable")
            .generate_with_report()
            .expect("duplicate __dir__ names should not fail generation");

        let module_report = generated
            .report
            .modules
            .iter()
            .find(|module| module.name == "pyo3_bindgen_duplicate_dir")
            .expect("the duplicate-name module should be reported");
        assert_eq!(
            module_report.counts.properties, 1,
            "duplicate __dir__ names must not duplicate emitted members"
        );
        Ok::<_, pyo3::PyErr>(())
    })
    .unwrap();
}

#[test]
fn empty_module_dir_name_keeps_the_public_codegen_error() {
    pyo3::Python::initialize();
    pyo3::Python::attach(|py| {
        Codegen::prime_live_runtime(py).unwrap();
        let module = module_from_factory(
            py,
            MALFORMED_DIR_SOURCE,
            pyo3::ffi::c_str!("malformed_dir.py"),
            pyo3::ffi::c_str!("malformed_dir_factory"),
            "malformed",
        )?;
        let error = Codegen::default()
            .module(&module)
            .expect_err("an empty __dir__ name must reject module registration");

        match error {
            pyo3_bindgen_engine::PyBindgenError::CodegenError(detail) => assert_eq!(
                detail,
                "module 'pyo3_bindgen_malformed_dir' __dir__ returned an empty attribute name"
            ),
            error => panic!("an empty __dir__ name must remain a codegen error: {error}"),
        }
        Ok::<_, pyo3::PyErr>(())
    })
    .unwrap();
}
