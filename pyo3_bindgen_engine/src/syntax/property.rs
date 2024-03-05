use super::{Ident, Path};
use crate::{typing::Type, Config, Result};
use rustc_hash::FxHashMap as HashMap;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Property {
    pub name: Path,
    owner: PropertyOwner,
    is_mutable: bool,
    annotation: Type,
    setter_annotation: Type,
    docstring: Option<String>,
    setter_docstring: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PropertyOwner {
    Module,
    Class,
}

impl Property {
    pub fn parse(
        _cfg: &Config,
        property: &pyo3::types::PyAny,
        name: Path,
        owner: PropertyOwner,
    ) -> Result<Self> {
        let py = property.py();

        // Extract the type of the property
        let typ = property.get_type();

        // Do not extract the docstring of the property, because it would point to the docstring of the type/class itself, not this property
        let mut docstring = None;

        // Determine the mutability and type of the property
        let (is_mutable, annotation, setter_annotation, mut setter_docstring);
        match owner {
            PropertyOwner::Module => {
                is_mutable = true;
                annotation = Type::try_from(typ)?;
                setter_annotation = annotation.clone();
                setter_docstring = docstring.clone();
            }
            PropertyOwner::Class => {
                let signature = py
                    .import(pyo3::intern!(py, "inspect"))?
                    .getattr(pyo3::intern!(py, "signature"))?;

                if let Ok(getter) = property.getattr(pyo3::intern!(py, "fget")) {
                    // Extract the annotation from the return of the function (if available)
                    if let Ok(function_signature) = signature.call1((getter,)) {
                        annotation = {
                            let return_annotation = function_signature
                                .getattr(pyo3::intern!(py, "return_annotation"))?;
                            if return_annotation
                                .is(function_signature.getattr(pyo3::intern!(py, "empty"))?)
                            {
                                Type::Unknown
                            } else {
                                return_annotation.try_into()?
                            }
                        };
                    } else {
                        annotation = Type::try_from(typ)?;
                    }

                    // Update the docstring if it is empty
                    if docstring.is_none() {
                        docstring = {
                            let docstring =
                                getter.getattr(pyo3::intern!(py, "__doc__"))?.to_string();
                            if docstring.is_empty() || docstring == "None" {
                                None
                            } else {
                                Some(docstring)
                            }
                        };
                    }
                } else {
                    annotation = Type::try_from(typ)?;
                }

                match property.getattr(pyo3::intern!(py, "fset")) {
                    Ok(setter) if !setter.is_none() => {
                        is_mutable = true;

                        // Extract the annotation from the parameter of the function (if available)
                        if let Ok(function_signature) = signature.call1((setter,)) {
                            setter_annotation = {
                                let param = function_signature
                                    .getattr(pyo3::intern!(py, "parameters"))?
                                    .call_method0(pyo3::intern!(py, "values"))?
                                    .iter()?
                                    .nth(1)
                                    .unwrap()?;
                                let annotation = param.getattr(pyo3::intern!(py, "annotation"))?;
                                if annotation.is(param.getattr(pyo3::intern!(py, "empty"))?) {
                                    Type::Unknown
                                } else {
                                    annotation.try_into()?
                                }
                            };
                        } else {
                            setter_annotation = Type::Unknown;
                        }

                        setter_docstring = {
                            let docstring =
                                setter.getattr(pyo3::intern!(py, "__doc__"))?.to_string();
                            if docstring.is_empty() || docstring == "None" {
                                None
                            } else {
                                Some(docstring)
                            }
                        };

                        if docstring.is_none() {
                            // Update the getter docstring to match setter docstring if it is still empty
                            docstring = setter_docstring.clone();
                        } else if setter_docstring.is_none() {
                            // Otherwise, update the setter docstring to match the getter docstring if it is still empty
                            setter_docstring = docstring.clone();
                        }
                    }
                    _ => {
                        is_mutable = false;
                        setter_annotation = Type::Unknown;
                        setter_docstring = None;
                    }
                }
            }
        }

        Ok(Self {
            name,
            owner,
            is_mutable,
            annotation,
            setter_annotation,
            docstring,
            setter_docstring,
        })
    }

    pub fn generate(
        &self,
        cfg: &Config,
        scoped_function_idents: &[&Ident],
        local_types: &HashMap<Path, Path>,
    ) -> Result<proc_macro2::TokenStream> {
        let mut output = proc_macro2::TokenStream::new();

        // Getter
        output.extend(self.generate_getter(cfg, scoped_function_idents, local_types)?);

        // Setter (if mutable)
        if self.is_mutable {
            output.extend(self.generate_setter(cfg, scoped_function_idents, local_types)?);
        }

        Ok(output)
    }

    pub fn generate_getter(
        &self,
        cfg: &Config,
        scoped_function_idents: &[&Ident],
        local_types: &HashMap<Path, Path>,
    ) -> Result<proc_macro2::TokenStream> {
        let mut output = proc_macro2::TokenStream::new();

        // Documentation
        if cfg.generate_docs {
            if let Some(docstring) = &self.docstring {
                // Trim the docstring and add a leading whitespace (looks better in the generated code)
                let mut docstring = docstring.trim().trim_end_matches('/').to_owned();
                docstring.insert(0, ' ');
                // Replace all double quotes with single quotes
                docstring = docstring.replace('"', "'");

                output.extend(quote::quote! {
                    #[doc = #docstring]
                });
            }
        }

        // Function
        let function_ident: syn::Ident = {
            let name = self.name.name();
            if let Ok(ident) = name.try_into() {
                if scoped_function_idents.contains(&name)
                    || crate::config::FORBIDDEN_FUNCTION_NAMES.contains(&name.as_py())
                {
                    let getter_name = Ident::from_py(&format!("get_{}", name.as_py()));
                    if scoped_function_idents.contains(&&getter_name)
                        || crate::config::FORBIDDEN_FUNCTION_NAMES.contains(&getter_name.as_py())
                    {
                        return Ok(proc_macro2::TokenStream::new());
                    } else {
                        getter_name.try_into()?
                    }
                } else {
                    ident
                }
            } else {
                let getter_name = Ident::from_py(&format!("get_{}", name.as_py()));
                if scoped_function_idents.contains(&&getter_name)
                    || crate::config::FORBIDDEN_FUNCTION_NAMES.contains(&getter_name.as_py())
                {
                    return Ok(proc_macro2::TokenStream::new());
                } else {
                    getter_name.try_into()?
                }
            }
        };
        let param_name = self.name.name().as_py();
        let param_type = self.annotation.clone().into_rs_owned(local_types);
        match &self.owner {
            PropertyOwner::Module => {
                let import = pyo3::Python::with_gil(|py| {
                    self.name
                        .parent()
                        .unwrap_or_else(|| unreachable!())
                        .import_quote(py)
                });
                output.extend(quote::quote! {
                    pub fn #function_ident<'py>(
                        py: ::pyo3::marker::Python<'py>,
                    ) -> ::pyo3::PyResult<#param_type> {
                        ::pyo3::FromPyObject::extract(
                            #import.getattr(::pyo3::intern!(py, #param_name))?
                        )
                    }
                });
            }
            PropertyOwner::Class => {
                let param_name = self.name.name().as_py();

                output.extend(quote::quote! {
                    pub fn #function_ident<'py>(
                        &'py self,
                        py: ::pyo3::marker::Python<'py>,
                    ) -> ::pyo3::PyResult<#param_type> {
                        self.0.getattr(::pyo3::intern!(py, #param_name))?
                        .extract()
                    }
                });
            }
        }

        Ok(output)
    }

    pub fn generate_setter(
        &self,
        _cfg: &Config,
        scoped_function_idents: &[&Ident],
        local_types: &HashMap<Path, Path>,
    ) -> Result<proc_macro2::TokenStream> {
        let mut output = proc_macro2::TokenStream::new();

        // Documentation
        if let Some(docstring) = &self.setter_docstring {
            // Trim the docstring and add a leading whitespace (looks better in the generated code)
            let mut docstring = docstring.trim().trim_end_matches('/').to_owned();
            docstring.insert(0, ' ');
            // Replace all double quotes with single quotes
            docstring = docstring.replace('"', "'");

            output.extend(quote::quote! {
                #[doc = #docstring]
            });
        }

        // Function
        let function_ident: syn::Ident = {
            let setter_name = Ident::from_py(&format!("set_{}", self.name.name().as_py()));
            if scoped_function_idents.contains(&&setter_name)
                || crate::config::FORBIDDEN_FUNCTION_NAMES.contains(&setter_name.as_py())
            {
                return Ok(proc_macro2::TokenStream::new());
            } else {
                setter_name.try_into()?
            }
        };
        let param_name = self.name.name().as_py();
        let param_preprocessing = self.annotation.preprocess_borrowed(
            &syn::Ident::new("p_value", proc_macro2::Span::call_site()),
            local_types,
        );
        let param_type = self.annotation.clone().into_rs_borrowed(local_types);
        match &self.owner {
            PropertyOwner::Module => {
                let import = pyo3::Python::with_gil(|py| {
                    self.name
                        .parent()
                        .unwrap_or_else(|| unreachable!())
                        .import_quote(py)
                });
                output.extend(quote::quote! {
                    pub fn #function_ident<'py>(
                        py: ::pyo3::marker::Python<'py>,
                        p_value: #param_type,
                    ) -> ::pyo3::PyResult<()> {
                        #param_preprocessing
                        #import.setattr(::pyo3::intern!(py, #param_name), p_value)
                    }
                });
            }
            PropertyOwner::Class => {
                output.extend(quote::quote! {
                    pub fn #function_ident<'py>(
                        &'py self,
                        py: ::pyo3::marker::Python<'py>,
                        p_value: #param_type,
                    ) -> ::pyo3::PyResult<()> {
                        #param_preprocessing
                        self.0.setattr(::pyo3::intern!(py, #param_name), p_value)
                    }
                });
            }
        }

        Ok(output)
    }
}
