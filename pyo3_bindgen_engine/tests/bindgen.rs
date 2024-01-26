macro_rules! test_bindgen {
    {
        $(#[$meta:meta])*
        $test_name:ident                        $(,)?
        $(py)?$(python)? $(:)? $code_py:literal $(,)?
        $(rs)?$(rust)?   $(:)? $code_rs:literal $(,)?
    } => {
        #[test]
        $(#[$meta])*
        fn $test_name() {
            // Arrange
            const CODE_PY: &str = indoc::indoc! { $code_py };
            const CODE_RS: &str = indoc::indoc! { $code_rs };

            // Act
            let bindings = pyo3_bindgen_engine::generate_bindings_from_str(
                CODE_PY,
                concat!("t_mod_", stringify!($test_name)),
            )
            .unwrap();

            // Assert
            let generated_code = format_code(&bindings.to_string());
            let target_code = format_code(CODE_RS);
            assert_eq!(
                generated_code, target_code,
                "\nGenerated:\n\n{generated_code}"
            );
        }
    };
}

fn format_code(input: &str) -> String {
    prettyplease::unparse(&syn::parse_str(input).unwrap())
}

test_bindgen! {
    test_bindgen_attribute

    py:r#"
        t_const_float: float = 0.42
    "#

    rs:r#"
        ///
        #[allow(
            clippy::all,
            clippy::nursery,
            clippy::pedantic,
            non_camel_case_types,
            non_snake_case,
            non_upper_case_globals,
            unused
        )]
        mod t_mod_test_bindgen_attribute {
            ///Getter for the `t_const_float` attribute
            pub fn t_const_float<'py>(py: ::pyo3::marker::Python<'py>) -> ::pyo3::PyResult<f64> {
                py.import(::pyo3::intern!(py, "t_mod_test_bindgen_attribute"))?
                    .getattr(::pyo3::intern!(py, "t_const_float"))?
                    .extract()
            }
            ///Setter for the `t_const_float` attribute
            pub fn set_t_const_float<'py>(
                py: ::pyo3::marker::Python<'py>,
                value: f64,
            ) -> ::pyo3::PyResult<()> {
                py.import(::pyo3::intern!(py, "t_mod_test_bindgen_attribute"))?
                    .setattr(::pyo3::intern!(py, "t_const_float"), value)?;
                Ok(())
            }
        }
    "#
}

test_bindgen! {
    test_bindgen_function

    py:r#"
        def t_fn(t_arg1: str) -> int:
            """t_docs"""
            ...
    "#

    rs:r#"
        ///
        #[allow(
            clippy::all,
                clippy::nursery,
                clippy::pedantic,
                non_camel_case_types,
                non_snake_case,
                non_upper_case_globals,
                unused
        )]
        mod t_mod_test_bindgen_function {
            ///t_docs
            pub fn t_fn<'py>(
                py: ::pyo3::marker::Python<'py>,
                t_arg1: &str,
            ) -> ::pyo3::PyResult<i64> {
                let __internal_args = ();
                let __internal_kwargs = ::pyo3::types::PyDict::new(py);
                __internal_kwargs.set_item(::pyo3::intern!(py, "t_arg1"), t_arg1)?;
                py.import(::pyo3::intern!(py, "t_mod_test_bindgen_function"))?
                    .call_method(
                        ::pyo3::intern!(py, "t_fn"),
                        __internal_args,
                        Some(__internal_kwargs),
                    )?
                    .extract()
            }
        }
    "#
}

test_bindgen! {
    test_bindgen_class

    py:r#"
        from typing import Dict, Optional
        class t_class:
            """t_docs"""
            def __init__(self, t_arg1: str, t_arg2: Optional[int] = None):
                """t_docs_init"""
                ...
            def t_method(self, t_arg1: Dict[str, int], **kwargs):
                """t_docs_method"""
                ...
            @property
            def t_prop(self) -> int:
                ...
            @t_prop.setter
            def t_prop(self, value: int):
                ...
    "#

    rs:r#"
        ///
        #[allow(
            clippy::all,
            clippy::nursery,
            clippy::pedantic,
            non_camel_case_types,
            non_snake_case,
            non_upper_case_globals,
            unused
        )]
        mod t_mod_test_bindgen_class {
            ///t_docs
            #[repr(transparent)]
            pub struct t_class(::pyo3::PyAny);
            ::pyo3::pyobject_native_type_named!(t_class);
            ::pyo3::pyobject_native_type_info!(
                t_class,
                ::pyo3::pyobject_native_static_type_object!(::pyo3::ffi::PyBaseObject_Type),
                ::std::option::Option::Some("t_mod_test_bindgen_class.t_class")
            );
            ::pyo3::pyobject_native_type_extract!(t_class);
            #[automatically_derived]
            impl t_class {
                ///t_docs_init
                pub fn new<'py>(
                    py: ::pyo3::marker::Python<'py>,
                    t_arg1: &str,
                    t_arg2: ::std::option::Option<i64>,
                ) -> ::pyo3::PyResult<&'py Self> {
                    let __internal_args = ();
                    let __internal_kwargs = ::pyo3::types::PyDict::new(py);
                    __internal_kwargs.set_item(::pyo3::intern!(py, "t_arg1"), t_arg1)?;
                    __internal_kwargs.set_item(::pyo3::intern!(py, "t_arg2"), t_arg2)?;
                    py.import(::pyo3::intern!(py, "t_mod_test_bindgen_class"))?
                        .getattr(::pyo3::intern!(py, "t_class"))?
                        .call(__internal_args, Some(__internal_kwargs))?
                        .extract()
                }
                ///Call self as a function.
                pub fn call<'py>(
                    &'py self,
                    py: ::pyo3::marker::Python<'py>,
                    args: impl ::pyo3::IntoPy<::pyo3::Py<::pyo3::types::PyTuple>>,
                    kwargs: &'py ::pyo3::types::PyDict,
                ) -> ::pyo3::PyResult<&'py ::pyo3::types::PyAny> {
                    let __internal_args = args;
                    self.call_method1(::pyo3::intern!(py, "__call__"), __internal_args)?
                        .extract()
                }
                ///t_docs_method
                pub fn t_method<'py>(
                    &'py self,
                    py: ::pyo3::marker::Python<'py>,
                    t_arg1: &::std::collections::HashMap<::std::string::String, i64>,
                    kwargs: &'py ::pyo3::types::PyDict,
                ) -> ::pyo3::PyResult<&'py ::pyo3::types::PyAny> {
                    let __internal_args = ();
                    let __internal_kwargs = kwargs;
                    __internal_kwargs.set_item(::pyo3::intern!(py, "t_arg1"), t_arg1)?;
                    self.call_method(
                            ::pyo3::intern!(py, "t_method"),
                            __internal_args,
                            Some(__internal_kwargs),
                        )?
                        .extract()
                }
                ///Getter for the `t_prop` attribute
                pub fn t_prop<'py>(
                    &'py self,
                    py: ::pyo3::marker::Python<'py>,
                ) -> ::pyo3::PyResult<i64> {
                    self.getattr(::pyo3::intern!(py, "t_prop"))?.extract()
                }
                ///Setter for the `t_prop` attribute
                pub fn set_t_prop<'py>(
                    &'py self,
                    py: ::pyo3::marker::Python<'py>,
                    value: i64,
                ) -> ::pyo3::PyResult<()> {
                    self.setattr(::pyo3::intern!(py, "t_prop"), value)?;
                    Ok(())
                }
            }
        }
    "#
}
