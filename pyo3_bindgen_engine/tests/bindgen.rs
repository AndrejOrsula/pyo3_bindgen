macro_rules! test_bindgen {
    {
        $(#[$meta:meta])*
        $test_name:ident                       $(,)?
        $(py)?$(python)?$(:)? $code_py:literal $(,)?
        $(rs)?$(rust)?$(:)?   $code_rs:literal $(,)?
    } => {
        #[test]
        $(#[$meta])*
        fn $test_name() {
            // Arrange
            const CODE_PY: &str = indoc::indoc! { $code_py };
            const CODE_RS: &str = indoc::indoc! { $code_rs };

            // Act
            let bindings = pyo3_bindgen_engine::Codegen::default()
                .module_from_str(CODE_PY, concat!("mod_", stringify!($test_name)))
                .unwrap()
                .generate()
                .unwrap();

            // Assert
            fn format_code(input: &str) -> String {
                prettyplease::unparse(&syn::parse_str(input).unwrap())
            }
            let generated_code = format_code(&bindings.to_string());
            let target_code = format_code(CODE_RS);
            assert_eq!(
                generated_code, target_code,
                "\nGenerated:\n\n{generated_code}"
            );
        }
    };
}

test_bindgen! {
    bindgen_property

    py: r#"
        my_property: float = 0.42
    "#

    rs: r#"
        #[allow(
            clippy::all,
            clippy::nursery,
            clippy::pedantic,
            non_camel_case_types,
            non_snake_case,
            non_upper_case_globals,
            unused
        )]
        pub mod mod_bindgen_property {
            pub fn my_property<'py>(py: ::pyo3::marker::Python<'py>) -> ::pyo3::PyResult<f64> {
                use ::pyo3::types::PyAnyMethods;
                py.import_bound(::pyo3::intern!(py, "mod_bindgen_property"))?
                    .getattr(::pyo3::intern!(py, "my_property"))?
                    .extract()
            }
            pub fn set_my_property<'py>(
                py: ::pyo3::marker::Python<'py>,
                p_value: f64,
            ) -> ::pyo3::PyResult<()> {
                use ::pyo3::types::PyAnyMethods;
                py.import_bound(::pyo3::intern!(py, "mod_bindgen_property"))?
                    .setattr(::pyo3::intern!(py, "my_property"), p_value)
            }
        }
    "#
}

test_bindgen! {
    bindgen_function

    py: r#"
        def my_function(my_arg1: str) -> int:
            """My docstring for `my_function`"""
            ...
    "#

    rs: r#"
        #[allow(
            clippy::all,
            clippy::nursery,
            clippy::pedantic,
            non_camel_case_types,
            non_snake_case,
            non_upper_case_globals,
            unused
        )]
        pub mod mod_bindgen_function {
            /// My docstring for `my_function`
            pub fn my_function<'py>(
                py: ::pyo3::marker::Python<'py>,
                p_my_arg1: &str,
            ) -> ::pyo3::PyResult<i64> {
                use ::pyo3::types::PyAnyMethods;
                py.import_bound(::pyo3::intern!(py, "mod_bindgen_function"))?
                    .call_method1(
                        ::pyo3::intern!(py, "my_function"),
                        ::pyo3::types::PyTuple::new_bound(
                            py,
                            [::pyo3::ToPyObject::to_object(&p_my_arg1, py)],
                        ),
                    )?
                    .extract()
            }
        }
    "#
}

test_bindgen! {
    bindgen_class

    py: r#"
        from typing import Dict, Optional
        class MyClass:
            """My docstring for `MyClass`"""
            def __init__(self, my_arg1: str, my_arg2: Optional[int] = None):
                """My docstring for __init__"""
                ...
            def my_method(self, my_arg1: Dict[str, int], **kwargs):
                """My docstring for `my_method`"""
                ...
            @property
            def my_property(self) -> int:
                ...
            @my_property.setter
            def my_property(self, value: int):
                ...

        def my_function_with_class_param(my_arg1: MyClass):
            ...

        def my_function_with_class_return() -> MyClass:
            ...
    "#

    rs: r#"
        #[allow(
            clippy::all,
            clippy::nursery,
            clippy::pedantic,
            non_camel_case_types,
            non_snake_case,
            non_upper_case_globals,
            unused
        )]
        pub mod mod_bindgen_class {
            /// My docstring for `MyClass`
            #[repr(transparent)]
            pub struct MyClass(::pyo3::PyAny);
            ::pyo3::pyobject_native_type_named!(MyClass);
            ::pyo3::pyobject_native_type_info!(
                MyClass,
                ::pyo3::pyobject_native_static_type_object!(::pyo3::ffi::PyBaseObject_Type),
                ::std::option::Option::Some("mod_bindgen_class.MyClass")
            );
            ::pyo3::pyobject_native_type_extract!(MyClass);
            #[automatically_derived]
            impl MyClass {
                /// My docstring for __init__
                pub fn new<'py>(
                    py: ::pyo3::marker::Python<'py>,
                    p_my_arg1: &str,
                    p_my_arg2: ::std::option::Option<i64>,
                ) -> ::pyo3::PyResult<::pyo3::Bound<'py, Self>> {
                    use ::pyo3::types::PyAnyMethods;
                    py.import_bound(::pyo3::intern!(py, "mod_bindgen_class"))?
                        .getattr(::pyo3::intern!(py, "MyClass"))?
                        .call1(
                            ::pyo3::types::PyTuple::new_bound(
                                py,
                                [
                                    ::pyo3::ToPyObject::to_object(&p_my_arg1, py),
                                    ::pyo3::ToPyObject::to_object(&p_my_arg2, py),
                                ],
                            ),
                        )?
                        .extract()
                }
                /// My docstring for `my_method`
                pub fn my_method<'py>(
                    slf: &::pyo3::Bound<'py, Self>,
                    py: ::pyo3::marker::Python<'py>,
                    p_my_arg1: &::std::collections::HashMap<::std::string::String, i64>,
                    p_kwargs: ::std::option::Option<::pyo3::Bound<'py, ::pyo3::types::PyDict>>,
                ) -> ::pyo3::PyResult<::pyo3::Bound<'py, ::pyo3::types::PyAny>> {
                    use ::pyo3::types::PyAnyMethods;
                    let p_kwargs = if let Some(p_kwargs) = p_kwargs {
                        ::pyo3::types::IntoPyDict::into_py_dict_bound(p_kwargs, py)
                    } else {
                        ::pyo3::types::PyDict::new_bound(py)
                    };
                    slf.call_method(
                            ::pyo3::intern!(py, "my_method"),
                            ::pyo3::types::PyTuple::new_bound(
                                py,
                                [::pyo3::ToPyObject::to_object(&p_my_arg1, py)],
                            ),
                            Some(&p_kwargs),
                        )?
                        .extract()
                }
                pub fn my_property<'py>(
                    slf: &::pyo3::Bound<'py, Self>,
                    py: ::pyo3::marker::Python<'py>,
                ) -> ::pyo3::PyResult<i64> {
                    use ::pyo3::types::PyAnyMethods;
                    slf.getattr(::pyo3::intern!(py, "my_property"))?.extract()
                }
                pub fn set_my_property<'py>(
                    slf: &::pyo3::Bound<'py, Self>,
                    py: ::pyo3::marker::Python<'py>,
                    p_value: i64,
                ) -> ::pyo3::PyResult<()> {
                    use ::pyo3::types::PyAnyMethods;
                    slf.setattr(::pyo3::intern!(py, "my_property"), p_value)
                }
            }
            pub fn my_function_with_class_param<'py>(
                py: ::pyo3::marker::Python<'py>,
                p_my_arg1: &::pyo3::Bound<'py, MyClass>,
            ) -> ::pyo3::PyResult<::pyo3::Bound<'py, ::pyo3::types::PyAny>> {
                use ::pyo3::types::PyAnyMethods;
                py.import_bound(::pyo3::intern!(py, "mod_bindgen_class"))?
                    .call_method1(
                        ::pyo3::intern!(py, "my_function_with_class_param"),
                        ::pyo3::types::PyTuple::new_bound(
                            py,
                            [::pyo3::ToPyObject::to_object(&p_my_arg1, py)],
                        ),
                    )?
                    .extract()
            }
            pub fn my_function_with_class_return<'py>(
                py: ::pyo3::marker::Python<'py>,
            ) -> ::pyo3::PyResult<::pyo3::Bound<'py, MyClass>> {
                use ::pyo3::types::PyAnyMethods;
                py.import_bound(::pyo3::intern!(py, "mod_bindgen_class"))?
                    .call_method0(::pyo3::intern!(py, "my_function_with_class_return"))?
                    .extract()
            }
        }
    "#
}
