use crate::types::Type;

/// Generate Rust bindings to a Python attribute. The attribute can be a standalone
/// attribute or a property of a class.
pub fn bind_attribute<S: ::std::hash::BuildHasher>(
    py: pyo3::Python,
    module_name: &str,
    is_class: bool,
    name: &str,
    attr: &pyo3::PyAny,
    attr_type: &pyo3::PyAny,
    all_types: &std::collections::HashSet<String, S>,
) -> Result<proc_macro2::TokenStream, pyo3::PyErr> {
    let mut token_stream = proc_macro2::TokenStream::new();

    let mut has_setter = true;
    let mut getter_type = attr_type;
    let mut setter_type = attr_type;
    let getter_doc = py.None();
    let mut getter_doc = getter_doc.as_ref(py);
    let setter_doc = py.None();
    let mut setter_doc = setter_doc.as_ref(py);

    // Check if the attribute has a getter and setter (is a property)
    if let Ok(getter) = attr.getattr("fget") {
        let inspect = py.import("inspect")?;
        let signature = inspect.call_method1("signature", (getter,))?;
        let empty_return_annotation = signature.getattr("empty")?;
        let return_annotation = signature.getattr("return_annotation")?;
        if !return_annotation.is(empty_return_annotation) {
            getter_type = return_annotation;
        }
        if let Ok(doc) = getter.getattr("__doc__") {
            getter_doc = doc;
        }
        has_setter = false;
    }
    if let Ok(setter) = attr.getattr("fset") {
        if !setter.is_none() {
            let inspect = py.import("inspect")?;
            let signature = inspect.call_method1("signature", (setter,))?;
            let empty_return_annotation = signature.getattr("empty")?;
            let value_annotation = signature
                .getattr("parameters")?
                .call_method0("values")?
                .iter()?
                .last()
                .unwrap()?
                .getattr("annotation")?;
            if !value_annotation.is(empty_return_annotation) {
                setter_type = value_annotation;
            }
            if let Ok(doc) = setter.getattr("__doc__") {
                setter_doc = doc;
            }
            has_setter = true;
        }
    }

    let mut getter_doc = getter_doc.to_string();
    if getter_doc == "None" || getter_doc.is_empty() {
        getter_doc = format!("Getter for the `{name}` attribute");
    };

    let mut setter_doc = setter_doc.to_string();
    if setter_doc == "None" || setter_doc.is_empty() {
        setter_doc = format!("Setter for the `{name}` attribute");
    };

    let getter_ident = if syn::parse_str::<syn::Ident>(name).is_ok() {
        quote::format_ident!("{}", name)
    } else {
        quote::format_ident!("r#{}", name)
    };
    let setter_ident = quote::format_ident!("set_{}", name);

    let getter_type = Type::try_from(getter_type)?.into_rs_owned(module_name, all_types);
    let setter_type = Type::try_from(setter_type)?.into_rs_borrowed(module_name, all_types);

    if is_class {
        token_stream.extend(quote::quote! {
            #[doc = #getter_doc]
            pub fn #getter_ident<'py>(
                &'py self,
                py: ::pyo3::marker::Python<'py>,
            ) -> ::pyo3::PyResult<#getter_type> {
                self.getattr(::pyo3::intern!(py, #name))?
                .extract()
            }
        });
        if has_setter {
            token_stream.extend(quote::quote! {
                #[doc = #setter_doc]
                pub fn #setter_ident<'py>(
                    &'py self,
                    py: ::pyo3::marker::Python<'py>,
                    value: #setter_type,
                ) -> ::pyo3::PyResult<()> {
                    self.setattr(::pyo3::intern!(py, #name), value)?;
                    Ok(())
                }
            });
        }
    } else {
        token_stream.extend(quote::quote! {
            #[doc = #getter_doc]
            pub fn #getter_ident<'py>(
                py: ::pyo3::marker::Python<'py>,
            ) -> ::pyo3::PyResult<#getter_type> {
                py.import(::pyo3::intern!(py, #module_name))?
                .getattr(::pyo3::intern!(py, #name))?
                .extract()
            }
        });
        if has_setter {
            token_stream.extend(quote::quote! {
                #[doc = #setter_doc]
                pub fn #setter_ident<'py>(
                    py: ::pyo3::marker::Python<'py>,
                    value: #setter_type,
                ) -> ::pyo3::PyResult<()> {
                    py.import(::pyo3::intern!(py, #module_name))?
                    .setattr(::pyo3::intern!(py, #name), value)?;
                    Ok(())
                }
            });
        }
    }

    Ok(token_stream)
}
