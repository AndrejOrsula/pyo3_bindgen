use super::{FunctionImplementation, Ident, Path, TraitMethod};
use crate::{typing::Type, Config, Result};
use pyo3::prelude::*;
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
        property: &pyo3::Bound<pyo3::types::PyAny>,
        name: Path,
        owner: PropertyOwner,
    ) -> Result<Self> {
        let py = property.py();

        // Extract the type of the property
        let typ = property.get_type();

        // Do not extract the docstring of the property, because it would point to the docstring of the type/class itself, not this property
        let mut docstring = None;

        // Determine the mutability and type of the property
        let (is_mutable, annotation, setter_annotation);
        let mut setter_docstring = None;
        match owner {
            PropertyOwner::Module => {
                is_mutable = true;
                annotation = Type::try_from(typ)?;
                setter_annotation = annotation.clone();
                docstring.clone_from(&setter_docstring);
            }
            PropertyOwner::Class => {
                let signature = py
                    .import_bound(pyo3::intern!(py, "inspect"))?
                    .getattr(pyo3::intern!(py, "signature"))?;

                if let Ok(getter) = property.getattr(pyo3::intern!(py, "fget")) {
                    // Extract the annotation from the return of the function (if available)
                    if let Ok(function_signature) = signature.call1((&getter,)) {
                        annotation = {
                            let return_annotation = function_signature
                                .getattr(pyo3::intern!(py, "return_annotation"))?;
                            if return_annotation
                                .is(&function_signature.getattr(pyo3::intern!(py, "empty"))?)
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
                        if let Ok(function_signature) = signature.call1((&setter,)) {
                            setter_annotation = {
                                let param = function_signature
                                    .getattr(pyo3::intern!(py, "parameters"))?
                                    .call_method0(pyo3::intern!(py, "values"))?
                                    .iter()?
                                    .nth(1)
                                    .unwrap()?;
                                let annotation = param.getattr(pyo3::intern!(py, "annotation"))?;
                                if annotation.is(&param.getattr(pyo3::intern!(py, "empty"))?) {
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
                            docstring.clone_from(&setter_docstring);
                        } else if setter_docstring.is_none() {
                            // Otherwise, update the setter docstring to match the getter docstring if it is still empty
                            setter_docstring.clone_from(&docstring);
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
    ) -> Result<FunctionImplementation> {
        Ok(match self.owner {
            PropertyOwner::Module => {
                let mut functions = proc_macro2::TokenStream::new();

                // Getter
                let impl_fn = self
                    .generate_getter(cfg, scoped_function_idents, local_types)?
                    .impl_fn;
                functions.extend(quote::quote! { pub #impl_fn });

                // Setter (if mutable)
                if self.is_mutable {
                    let impl_fn = self
                        .generate_setter(cfg, scoped_function_idents, local_types)?
                        .impl_fn;
                    functions.extend(quote::quote! { pub #impl_fn });
                }

                FunctionImplementation::Function(functions)
            }
            PropertyOwner::Class => {
                let mut trait_fn = proc_macro2::TokenStream::new();
                let mut impl_fn = proc_macro2::TokenStream::new();

                // Getter
                let getter = self.generate_getter(cfg, scoped_function_idents, local_types)?;
                trait_fn.extend(getter.trait_fn);
                impl_fn.extend(getter.impl_fn);

                // Setter (if mutable)
                if self.is_mutable {
                    let setter = self.generate_setter(cfg, scoped_function_idents, local_types)?;
                    trait_fn.extend(setter.trait_fn);
                    impl_fn.extend(setter.impl_fn);
                }

                FunctionImplementation::Method(TraitMethod { trait_fn, impl_fn })
            }
        })
    }

    pub fn generate_getter(
        &self,
        cfg: &Config,
        scoped_function_idents: &[&Ident],
        local_types: &HashMap<Path, Path>,
    ) -> Result<TraitMethod> {
        let mut trait_fn = proc_macro2::TokenStream::new();
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
                        return Ok(TraitMethod::empty());
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
                    return Ok(TraitMethod::empty());
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
                impl_fn.extend(quote::quote! {
                    fn #function_ident<'py>(
                        py: ::pyo3::marker::Python<'py>,
                    ) -> ::pyo3::PyResult<#param_type> {
                        ::pyo3::types::PyAnyMethods::extract(
                            &::pyo3::types::PyAnyMethods::getattr(#import.as_any(), ::pyo3::intern!(py, #param_name))?
                        )
                    }
                });
            }
            PropertyOwner::Class => {
                let param_name = self.name.name().as_py();

                trait_fn.extend(quote::quote! {
                    fn #function_ident<'py>(
                        &'py self,
                    ) -> ::pyo3::PyResult<#param_type>;
                });
                impl_fn.extend(quote::quote! {
                    fn #function_ident<'py>(
                        &'py self,
                    ) -> ::pyo3::PyResult<#param_type> {
                        ::pyo3::types::PyAnyMethods::extract(
                            &::pyo3::types::PyAnyMethods::getattr(self.as_any(), ::pyo3::intern!(self.py(), #param_name))?
                        )
                    }
                });
            }
        }

        Ok(TraitMethod { trait_fn, impl_fn })
    }

    pub fn generate_setter(
        &self,
        cfg: &Config,
        scoped_function_idents: &[&Ident],
        local_types: &HashMap<Path, Path>,
    ) -> Result<TraitMethod> {
        let mut trait_fn = proc_macro2::TokenStream::new();
        let mut impl_fn = proc_macro2::TokenStream::new();

        // Documentation
        if cfg.generate_docs {
            if let Some(mut docstring) = self.setter_docstring.clone() {
                crate::utils::text::format_docstring(&mut docstring);
                impl_fn.extend(quote::quote! {
                    #[doc = #docstring]
                });
            }
        }

        // Function
        let function_ident: syn::Ident = {
            let setter_name = Ident::from_py(&format!("set_{}", self.name.name().as_py()));
            if scoped_function_idents.contains(&&setter_name)
                || crate::config::FORBIDDEN_FUNCTION_NAMES.contains(&setter_name.as_py())
            {
                return Ok(TraitMethod::empty());
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
                impl_fn.extend(quote::quote! {
                    fn #function_ident<'py>(
                        py: ::pyo3::marker::Python<'py>,
                        p_value: #param_type,
                    ) -> ::pyo3::PyResult<()> {
                        #param_preprocessing
                        ::pyo3::types::PyAnyMethods::setattr(#import.as_any(), ::pyo3::intern!(py, #param_name), p_value)
                    }
                });
            }
            PropertyOwner::Class => {
                trait_fn.extend(quote::quote! {
                    fn #function_ident<'py>(
                        &'py self,
                        p_value: #param_type,
                    ) -> ::pyo3::PyResult<()>;
                });
                impl_fn.extend(quote::quote! {
                    fn #function_ident<'py>(
                        &'py self,
                        p_value: #param_type,
                    ) -> ::pyo3::PyResult<()> {
                        let py = self.py();
                        #param_preprocessing
                        ::pyo3::types::PyAnyMethods::setattr(self.as_any(), ::pyo3::intern!(py, #param_name), p_value)
                    }
                });
            }
        }

        Ok(TraitMethod { trait_fn, impl_fn })
    }
}
