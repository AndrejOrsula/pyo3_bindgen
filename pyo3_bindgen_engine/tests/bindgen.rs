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
        /// Embed the Python source code of the module into the Python interpreter
        /// in order to enable the use of the generated Rust bindings.
        pub fn pyo3_embed_python_source_code<'py>(
            py: ::pyo3::marker::Python<'py>,
        ) -> ::pyo3::PyResult<()> {
            const SOURCE_CODE: &str = "my_property: float = 0.42\n";
            pyo3::types::PyAnyMethods::set_item(
                &pyo3::types::PyAnyMethods::getattr(
                    py.import_bound(pyo3::intern!(py, "sys"))?.as_any(),
                    pyo3::intern!(py, "modules"),
                )?,
                "mod_bindgen_property",
                pyo3::types::PyModule::from_code_bound(
                    py,
                    SOURCE_CODE,
                    "mod_bindgen_property/__init__.py",
                    "mod_bindgen_property",
                )?,
            )
        }
        pub fn my_property<'py>(py: ::pyo3::marker::Python<'py>) -> ::pyo3::PyResult<f64> {
            ::pyo3::types::PyAnyMethods::extract(
                &::pyo3::types::PyAnyMethods::getattr(
                    py.import_bound(::pyo3::intern!(py, "mod_bindgen_property"))?.as_any(),
                    ::pyo3::intern!(py, "my_property"),
                )?,
            )
        }
        pub fn set_my_property<'py>(
            py: ::pyo3::marker::Python<'py>,
            p_value: f64,
        ) -> ::pyo3::PyResult<()> {
            ::pyo3::types::PyAnyMethods::setattr(
                py.import_bound(::pyo3::intern!(py, "mod_bindgen_property"))?.as_any(),
                ::pyo3::intern!(py, "my_property"),
                p_value,
            )
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
        /// Embed the Python source code of the module into the Python interpreter
        /// in order to enable the use of the generated Rust bindings.
        pub fn pyo3_embed_python_source_code<'py>(
            py: ::pyo3::marker::Python<'py>,
        ) -> ::pyo3::PyResult<()> {
            const SOURCE_CODE: &str = "def my_function(my_arg1: str) -> int:\n    \"\"\"My docstring for `my_function`\"\"\"\n    ...\n";
            pyo3::types::PyAnyMethods::set_item(
                &pyo3::types::PyAnyMethods::getattr(
                    py.import_bound(pyo3::intern!(py, "sys"))?.as_any(),
                    pyo3::intern!(py, "modules"),
                )?,
                "mod_bindgen_function",
                pyo3::types::PyModule::from_code_bound(
                    py,
                    SOURCE_CODE,
                    "mod_bindgen_function/__init__.py",
                    "mod_bindgen_function",
                )?,
            )
        }
        /// My docstring for `my_function`
        pub fn my_function<'py>(
            py: ::pyo3::marker::Python<'py>,
            p_my_arg1: &str,
        ) -> ::pyo3::PyResult<i64> {
            ::pyo3::types::PyAnyMethods::extract(
                &::pyo3::types::PyAnyMethods::call_method1(
                    py.import_bound(::pyo3::intern!(py, "mod_bindgen_function"))?.as_any(),
                    ::pyo3::intern!(py, "my_function"),
                    ::pyo3::types::PyTuple::new_bound(
                        py,
                        [::pyo3::ToPyObject::to_object(&p_my_arg1, py)],
                    ),
                )?,
            )
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
        /// Embed the Python source code of the module into the Python interpreter
        /// in order to enable the use of the generated Rust bindings.
        pub fn pyo3_embed_python_source_code<'py>(
            py: ::pyo3::marker::Python<'py>,
        ) -> ::pyo3::PyResult<()> {
            const SOURCE_CODE: &str = "from typing import Dict, Optional\nclass MyClass:\n    \"\"\"My docstring for `MyClass`\"\"\"\n    def __init__(self, my_arg1: str, my_arg2: Optional[int] = None):\n        \"\"\"My docstring for __init__\"\"\"\n        ...\n    def my_method(self, my_arg1: Dict[str, int], **kwargs):\n        \"\"\"My docstring for `my_method`\"\"\"\n        ...\n    @property\n    def my_property(self) -> int:\n        ...\n    @my_property.setter\n    def my_property(self, value: int):\n        ...\n\ndef my_function_with_class_param(my_arg1: MyClass):\n    ...\n\ndef my_function_with_class_return() -> MyClass:\n    ...\n";
            pyo3::types::PyAnyMethods::set_item(
                &pyo3::types::PyAnyMethods::getattr(
                    py.import_bound(pyo3::intern!(py, "sys"))?.as_any(),
                    pyo3::intern!(py, "modules"),
                )?,
                "mod_bindgen_class",
                pyo3::types::PyModule::from_code_bound(
                    py,
                    SOURCE_CODE,
                    "mod_bindgen_class/__init__.py",
                    "mod_bindgen_class",
                )?,
            )
        }
        /// My docstring for `MyClass`
        #[repr(transparent)]
        pub struct MyClass(::pyo3::PyAny);
        ::pyo3::pyobject_native_type_named!(MyClass);
        ::pyo3::pyobject_native_type_info!(
            MyClass,
            ::pyo3::pyobject_native_static_type_object!(::pyo3::ffi::PyBaseObject_Type),
            ::std::option::Option::Some("mod_bindgen_class.MyClass")
        );
        #[automatically_derived]
        impl MyClass {
            /// My docstring for __init__
            pub fn new<'py>(
                py: ::pyo3::marker::Python<'py>,
                p_my_arg1: &str,
                p_my_arg2: ::std::option::Option<i64>,
            ) -> ::pyo3::PyResult<::pyo3::Bound<'py, Self>> {
                ::pyo3::types::PyAnyMethods::extract(
                    &::pyo3::types::PyAnyMethods::call1(
                        ::pyo3::types::PyAnyMethods::getattr(
                                py
                                    .import_bound(::pyo3::intern!(py, "mod_bindgen_class"))?
                                    .as_any(),
                                ::pyo3::intern!(py, "MyClass"),
                            )?
                            .as_any(),
                        ::pyo3::types::PyTuple::new_bound(
                            py,
                            [
                                ::pyo3::ToPyObject::to_object(&p_my_arg1, py),
                                ::pyo3::ToPyObject::to_object(&p_my_arg2, py),
                            ],
                        ),
                    )?,
                )
            }
        }
        /// These methods are defined for the `Bound<'py, T>` smart pointer, so to use
        /// method call syntax these methods are separated into a trait, because stable
        /// Rust does not yet support `arbitrary_self_types`.
        #[doc(alias = "MyClass")]
        #[automatically_derived]
        pub trait MyClassMethods {
            fn my_method<'py>(
                &'py self,
                p_my_arg1: &::std::collections::HashMap<::std::string::String, i64>,
                p_kwargs: ::std::option::Option<::pyo3::Bound<'py, ::pyo3::types::PyDict>>,
            ) -> ::pyo3::PyResult<::pyo3::Bound<'py, ::pyo3::types::PyAny>>;
            fn my_property<'py>(&'py self) -> ::pyo3::PyResult<i64>;
            fn set_my_property<'py>(&'py self, p_value: i64) -> ::pyo3::PyResult<()>;
        }
        #[automatically_derived]
        impl MyClassMethods for ::pyo3::Bound<'_, MyClass> {
            /// My docstring for `my_method`
            fn my_method<'py>(
                &'py self,
                p_my_arg1: &::std::collections::HashMap<::std::string::String, i64>,
                p_kwargs: ::std::option::Option<::pyo3::Bound<'py, ::pyo3::types::PyDict>>,
            ) -> ::pyo3::PyResult<::pyo3::Bound<'py, ::pyo3::types::PyAny>> {
                let py = self.py();
                let p_kwargs = if let Some(p_kwargs) = p_kwargs {
                    ::pyo3::types::IntoPyDict::into_py_dict_bound(p_kwargs, py)
                } else {
                    ::pyo3::types::PyDict::new_bound(py)
                };
                ::pyo3::types::PyAnyMethods::extract(
                    &::pyo3::types::PyAnyMethods::call_method(
                        self.as_any(),
                        ::pyo3::intern!(py, "my_method"),
                        ::pyo3::types::PyTuple::new_bound(
                            py,
                            [::pyo3::ToPyObject::to_object(&p_my_arg1, py)],
                        ),
                        Some(&p_kwargs),
                    )?,
                )
            }
            fn my_property<'py>(&'py self) -> ::pyo3::PyResult<i64> {
                ::pyo3::types::PyAnyMethods::extract(
                    &::pyo3::types::PyAnyMethods::getattr(
                        self.as_any(),
                        ::pyo3::intern!(self.py(), "my_property"),
                    )?,
                )
            }
            fn set_my_property<'py>(&'py self, p_value: i64) -> ::pyo3::PyResult<()> {
                let py = self.py();
                ::pyo3::types::PyAnyMethods::setattr(
                    self.as_any(),
                    ::pyo3::intern!(py, "my_property"),
                    p_value,
                )
            }
        }
        pub fn my_function_with_class_param<'py>(
            py: ::pyo3::marker::Python<'py>,
            p_my_arg1: &::pyo3::Bound<'py, MyClass>,
        ) -> ::pyo3::PyResult<::pyo3::Bound<'py, ::pyo3::types::PyAny>> {
            ::pyo3::types::PyAnyMethods::extract(
                &::pyo3::types::PyAnyMethods::call_method1(
                    py.import_bound(::pyo3::intern!(py, "mod_bindgen_class"))?.as_any(),
                    ::pyo3::intern!(py, "my_function_with_class_param"),
                    ::pyo3::types::PyTuple::new_bound(
                        py,
                        [::pyo3::ToPyObject::to_object(&p_my_arg1, py)],
                    ),
                )?,
            )
        }
        pub fn my_function_with_class_return<'py>(
            py: ::pyo3::marker::Python<'py>,
        ) -> ::pyo3::PyResult<::pyo3::Bound<'py, MyClass>> {
            ::pyo3::types::PyAnyMethods::extract(
                &::pyo3::types::PyAnyMethods::call_method0(
                    py.import_bound(::pyo3::intern!(py, "mod_bindgen_class"))?.as_any(),
                    ::pyo3::intern!(py, "my_function_with_class_return"),
                )?,
            )
        }
    }
    "#
}
