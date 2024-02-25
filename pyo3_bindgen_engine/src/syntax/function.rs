use super::{Ident, Path};
use crate::{
    traits::{Canonicalize, Generate},
    types::Type,
    Config, Result,
};

use pyo3::ToPyObject;

#[derive(Debug, Clone)]
pub struct Function {
    pub name: Path,
    pub typ: FunctionType,
    pub parameters: Vec<Parameter>,
    pub return_annotation: Type,
    pub docstring: Option<String>,
}

impl Function {
    pub fn parse(
        _cfg: &Config,
        function: &pyo3::types::PyAny,
        name: Path,
        typ: FunctionType,
    ) -> Result<Self> {
        let py = function.py();

        // Extract the signature of the function
        let function_signature = py
            .import(pyo3::intern!(py, "inspect"))
            .unwrap()
            .call_method1(pyo3::intern!(py, "signature"), (function,))
            .unwrap();

        // Extract the parameters of the function
        let parameters = function_signature
            .getattr(pyo3::intern!(py, "parameters"))
            .unwrap()
            .call_method0(pyo3::intern!(py, "values"))
            .unwrap()
            .iter()
            .unwrap()
            .map(|param| {
                let param = param?;

                let name = Ident::from_py(&param.getattr(pyo3::intern!(py, "name"))?.to_string());
                let kind =
                    ParameterKind::from(param.getattr(pyo3::intern!(py, "kind"))?.extract::<u8>()?);
                let annotation = {
                    let annotation = param.getattr(pyo3::intern!(py, "annotation"))?;
                    if annotation.is(param.getattr(pyo3::intern!(py, "empty")).unwrap()) {
                        None
                    } else {
                        Some(annotation)
                    }
                }
                .try_into()?;
                let default = {
                    let default = param.getattr(pyo3::intern!(py, "default"))?;
                    if default.is(param.getattr(pyo3::intern!(py, "empty")).unwrap()) {
                        None
                    } else {
                        Some(default.to_object(py))
                    }
                };

                Result::Ok(Parameter {
                    name,
                    kind,
                    annotation,
                    default,
                })
            })
            .collect::<Result<Vec<_>>>()?;

        // Extract the return annotation of the function
        let return_annotation = {
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

        // Extract the docstring of the function
        let docstring = {
            let docstring = function.getattr(pyo3::intern!(py, "__doc__"))?.to_string();
            if docstring.is_empty() || docstring == "None" {
                None
            } else {
                Some(docstring)
            }
        };

        Ok(Self {
            name,
            typ,
            parameters,
            return_annotation,
            docstring,
        })
    }
}

impl Generate for Function {
    fn generate(&self, _cfg: &Config) -> Result<proc_macro2::TokenStream> {
        todo!()
    }
}

impl Canonicalize for Function {
    fn canonicalize(&mut self) {
        todo!()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum FunctionType {
    Function,
    Method { class_path: Path, typ: MethodType },
    Closure(Path),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum MethodType {
    Constructor,
    Call,
    Regular,
}

#[derive(Debug, Clone)]
pub struct Parameter {
    pub name: Ident,
    pub kind: ParameterKind,
    pub annotation: Type,
    pub default: Option<pyo3::Py<pyo3::types::PyAny>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ParameterKind {
    PositionalOnly,
    PositionalOrKeyword,
    VarPositional,
    KeywordOnly,
    VarKeyword,
}

impl From<u8> for ParameterKind {
    fn from(kind: u8) -> Self {
        match kind {
            0 => Self::PositionalOnly,
            1 => Self::PositionalOrKeyword,
            2 => Self::VarPositional,
            3 => Self::KeywordOnly,
            4 => Self::VarKeyword,
            _ => unreachable!(),
        }
    }
}
