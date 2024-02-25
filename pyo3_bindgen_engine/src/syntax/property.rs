use super::Path;
use crate::{
    traits::{Canonicalize, Generate},
    types::Type,
    Config, Result,
};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Property {
    pub name: Path,
    pub owner: PropertyOwner,
    pub is_mutable: bool,
    pub annotation: Type,
    pub setter_annotation: Option<Type>,
    pub docstring: Option<String>,
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

        // Extract the docstring of the property
        let mut docstring = {
            let docstring = property.getattr(pyo3::intern!(py, "__doc__"))?.to_string();
            if docstring.is_empty() || docstring == "None" {
                None
            } else {
                Some(docstring)
            }
        };

        // Determine the mutability and type of the property
        let (is_mutable, annotation, setter_annotation);
        match owner {
            PropertyOwner::Module(_) => {
                is_mutable = true;
                annotation = Type::try_from(typ)?;
                setter_annotation = None;
            }
            PropertyOwner::Class(_) => {
                let signature = py
                    .import(pyo3::intern!(py, "inspect"))
                    .unwrap()
                    .getattr(pyo3::intern!(py, "signature"))
                    .unwrap();

                if let Ok(getter) = property.getattr(pyo3::intern!(py, "fget")) {
                    // Extract the signature of the function
                    let function_signature = signature.call1((getter,)).unwrap();

                    // Extract the annotation from the return of the function
                    annotation = {
                        let return_annotation =
                            function_signature.getattr(pyo3::intern!(py, "return_annotation"))?;
                        if return_annotation.is(function_signature
                            .getattr(pyo3::intern!(py, "empty"))
                            .unwrap())
                        {
                            None
                        } else {
                            Some(return_annotation)
                        }
                    }
                    .try_into()?;

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
                        // Extract the signature of the function
                        let function_signature = signature.call1((setter,)).unwrap();

                        // Extract the annotation from the parameter of the function
                        setter_annotation = Some(
                            {
                                let param = function_signature
                                    .getattr(pyo3::intern!(py, "parameters"))
                                    .unwrap()
                                    .call_method0(pyo3::intern!(py, "values"))
                                    .unwrap()
                                    .iter()
                                    .unwrap()
                                    .nth(1)
                                    .unwrap()
                                    .unwrap();
                                let annotation = param.getattr(pyo3::intern!(py, "annotation"))?;
                                if annotation.is(param.getattr(pyo3::intern!(py, "empty")).unwrap())
                                {
                                    None
                                } else {
                                    Some(annotation)
                                }
                            }
                            .try_into()?,
                        );
                        is_mutable = true;

                        // Update the docstring if it is still empty
                        if docstring.is_none() {
                            docstring = {
                                let docstring =
                                    setter.getattr(pyo3::intern!(py, "__doc__"))?.to_string();
                                if docstring.is_empty() || docstring == "None" {
                                    None
                                } else {
                                    Some(docstring)
                                }
                            };
                        }
                    }
                    _ => {
                        setter_annotation = None;
                        is_mutable = false;
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
        })
    }
}

impl Generate for Property {
    fn generate(&self, _cfg: &Config) -> Result<proc_macro2::TokenStream> {
        todo!()
    }
}

impl Canonicalize for Property {
    fn canonicalize(&mut self) {
        todo!()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PropertyOwner {
    Module(Path),
    Class(Path),
}
