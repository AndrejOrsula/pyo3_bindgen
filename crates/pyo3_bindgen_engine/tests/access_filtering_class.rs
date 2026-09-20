use indoc::indoc;
use pyo3::types::PyAnyMethods;

#[test]
fn class_name_filter_precedes_hostile_member_access() {
    let generated = pyo3_bindgen_engine::Codegen::default()
        .module_from_str(
            indoc! { r#"
                access_log = []

                class ExplodingDescriptor:
                    def __init__(self, name):
                        self.name = name

                    def __get__(self, instance, owner):
                        access_log.append("descriptor:" + self.name)
                        raise RuntimeError("hostile descriptor was accessed")

                def _dynamic_method(self, value: int) -> int:
                    return value

                class HostileMeta(type):
                    def __dir__(cls):
                        return [
                            "_private",
                            "tests",
                            "plain_value",
                            "allowed_dynamic",
                            "allowed_broken",
                            "__init__",
                            "__call__",
                        ]

                    def __getattribute__(cls, name):
                        if name in {"_private", "tests", "plain_value", "allowed_dynamic", "allowed_broken"}:
                            access_log.append("metaclass:" + name)
                        if name == "allowed_dynamic":
                            return _dynamic_method
                        return type.__getattribute__(cls, name)

                class Hostile(metaclass=HostileMeta):
                    _private = ExplodingDescriptor("_private")
                    tests = ExplodingDescriptor("tests")
                    plain_value: int = 42
                    allowed_broken = ExplodingDescriptor("allowed_broken")

                    def __init__(self, value: int):
                        self.value = value

                    def __call__(self, value: int) -> int:
                        return value
            "#},
            "mod_access_filtering_class",
        )
        .expect("hostile class fixture must import")
        .generate_with_report()
        .expect("hostile class fixture must remain recoverable");

    let generated_code = generated.tokens.to_string();
    assert!(
        generated_code
            .split("impl Hostile {")
            .any(|block| block.contains("pub fn new")),
        "the private-name policy must not discard __init__:\n{generated_code}"
    );
    let hostile_methods = generated_code
        .split("pub trait HostileMethods")
        .nth(1)
        .and_then(|rest| rest.split("impl HostileMethods").next())
        .unwrap_or_default();
    assert!(
        hostile_methods.contains("fn call"),
        "the private-name policy must not discard __call__:\n{generated_code}"
    );
    assert!(
        hostile_methods.contains("fn plain_value"),
        "a static-safe annotated class member must remain generated:\n{generated_code}"
    );
    assert!(
        hostile_methods.contains("fn allowed_dynamic"),
        "an allowed member missing from static lookup must fall back to class access:\n{generated_code}"
    );
    assert!(
        generated
            .report
            .diagnostics
            .iter()
            .filter(|diagnostic| {
                diagnostic.code == "class_attribute_inaccessible"
                    && diagnostic.path.as_deref()
                        == Some("mod_access_filtering_class.Hostile.allowed_broken")
            })
            .count()
            == 1,
        "an allowed failing descriptor must retain its path-qualified diagnostic: {:?}",
        generated.report.diagnostics,
    );

    pyo3::Python::attach(|py| {
        let module = py
            .import(pyo3::intern!(py, "mod_access_filtering_class"))
            .expect("hostile class module must remain imported");
        let hostile_class = module
            .getattr(pyo3::intern!(py, "Hostile"))
            .expect("hostile class must be present");
        let inspect = py
            .import(pyo3::intern!(py, "inspect"))
            .expect("inspect must remain importable");
        assert!(
            inspect
                .call_method1(
                    pyo3::intern!(py, "getattr_static"),
                    (&hostile_class, "allowed_dynamic"),
                )
                .is_err(),
            "the dynamic fixture member must be absent from static lookup"
        );
        let access_log = module
            .as_any()
            .getattr(pyo3::intern!(py, "access_log"))
            .expect("fixture access log must be present")
            .extract::<Vec<String>>()
            .expect("fixture access log must contain strings");

        for name in ["_private", "tests"] {
            assert!(
                !access_log.iter().any(|event| {
                    event == &format!("metaclass:{name}") || event == &format!("descriptor:{name}")
                }),
                "filtered class member `{name}` was dynamically accessed: {access_log:?}"
            );
        }
        assert!(
            !access_log
                .iter()
                .any(|event| event == "metaclass:plain_value"),
            "static-safe allowed class member used dynamic metaclass access: {access_log:?}"
        );
        assert!(
            access_log
                .iter()
                .any(|event| event == "metaclass:allowed_dynamic"),
            "a static lookup miss must fall back to the allowed member's real class access: {access_log:?}"
        );
        assert!(
            access_log
                .iter()
                .any(|event| event == "metaclass:allowed_broken"),
            "binding-required allowed descriptor must use class access: {access_log:?}"
        );
        assert!(
            access_log
                .iter()
                .any(|event| event == "descriptor:allowed_broken"),
            "allowed class member must still be dynamically inspected: {access_log:?}"
        );
    });
}

#[test]
fn class_dir_failure_is_propagated() {
    let error = pyo3_bindgen_engine::Codegen::default()
        .module_from_str(
            indoc! { r#"
                class BrokenDirMeta(type):
                    def __dir__(cls):
                        raise RuntimeError("hostile class __dir__ failure")

                class Broken(metaclass=BrokenDirMeta):
                    pass
            "#},
            "mod_class_dir_failure",
        )
        .expect_err("a failing class __dir__ must reject live introspection");

    match error {
        pyo3_bindgen_engine::PyBindgenError::PyError(error) => assert!(
            error.to_string().contains("hostile class __dir__ failure"),
            "the class __dir__ failure must remain visible: {error}"
        ),
        error => panic!("a hostile class __dir__ must remain a Python error: {error}"),
    }
}

#[test]
fn duplicate_class_dir_names_are_emitted_once() {
    let generated = pyo3_bindgen_engine::Codegen::default()
        .module_from_str(
            indoc! { r#"
                class DuplicateDirMeta(type):
                    def __dir__(cls):
                        return ["value", "value", "value"]

                class Duplicate(metaclass=DuplicateDirMeta):
                    value: int = 42
            "#},
            "mod_duplicate_class_dir",
        )
        .expect("duplicate class __dir__ names must remain parseable")
        .generate_with_report()
        .expect("duplicate class __dir__ names must not fail generation");

    let generated_code = generated.tokens.to_string();
    let duplicate_methods = generated_code
        .split("pub trait DuplicateMethods")
        .nth(1)
        .and_then(|rest| rest.split("impl DuplicateMethods").next())
        .unwrap_or_default();
    assert_eq!(
        duplicate_methods.matches("fn value").count(),
        1,
        "duplicate class __dir__ names must emit one member: {}",
        generated_code
    );
}

#[test]
fn empty_class_dir_name_keeps_the_public_codegen_error() {
    let error = pyo3_bindgen_engine::Codegen::default()
        .module_from_str(
            indoc! { r#"
                class EmptyDirMeta(type):
                    def __dir__(cls):
                        return ["", "value"]

                class Broken(metaclass=EmptyDirMeta):
                    value: int = 42
            "#},
            "mod_empty_class_dir",
        )
        .expect_err("an empty class __dir__ name must reject live introspection");

    match error {
        pyo3_bindgen_engine::PyBindgenError::CodegenError(detail) => assert_eq!(
            detail,
            "class 'mod_empty_class_dir.Broken' __dir__ returned an empty attribute name"
        ),
        error => panic!("an empty class __dir__ name must remain a codegen error: {error}"),
    }
}
