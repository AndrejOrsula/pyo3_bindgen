//! Regression test for a module-level property with a docstring reached via the
//! external IR (`Property::from_ir`) path. Unlike live introspection (which always
//! clears the docstring for `PropertyOwner::Module`), the IR path carries the
//! docstring through unconditionally, so this is the only way to exercise a
//! doc-commented module-level getter/setter.

use pyo3_bindgen_engine::{Codegen, ModuleIr, PropertyIr, SchemaIr};

#[test]
fn module_property_with_docstring_generates_valid_rust() {
    let module = ModuleIr {
        name: "pkg".to_owned(),
        docstring: None,
        is_package: false,
        classes: vec![],
        enums: vec![],
        functions: vec![],
        properties: vec![PropertyIr {
            name: "pkg.my_property".to_owned(),
            py_type: "int".to_owned(),
            setter_type: None,
            readonly: false,
            docstring: Some("A documented module-level property.".to_owned()),
            setter_docstring: Some("A documented module-level setter.".to_owned()),
        }],
        submodules: vec![],
        prelude: vec![],
        imports: vec![],
        type_vars: vec![],
    };

    let tokens = Codegen::default()
        .modules_from_ir(SchemaIr {
            modules: vec![module],
        })
        .unwrap()
        .generate()
        .unwrap();

    // The doc attribute must precede `pub fn`, not follow it, or this fails to parse.
    let generated = prettyplease::unparse(
        &syn::parse2(tokens).expect("generated bindings must remain valid Rust syntax"),
    );

    assert!(
        generated.contains("pub fn my_property"),
        "missing module property getter:\n{generated}"
    );
    assert!(
        generated.contains("pub fn set_my_property"),
        "missing module property setter:\n{generated}"
    );
    assert!(
        generated.contains("A documented module-level property."),
        "missing getter docstring:\n{generated}"
    );
    assert!(
        generated.contains("A documented module-level setter."),
        "missing setter docstring:\n{generated}"
    );
}
