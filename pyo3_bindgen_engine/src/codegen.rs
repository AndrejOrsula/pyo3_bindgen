use crate::traits::{Canonicalize, Generate};
use crate::{syntax::Module, Config, Result};

#[derive(Debug, Default, Clone)]
pub struct Codegen {
    pub cfg: Config,
    pub modules: Vec<Module>,
}

impl Codegen {
    pub fn new(cfg: Config) -> Result<Self> {
        Ok(Self {
            cfg,
            ..Default::default()
        })
    }

    pub fn module(mut self, module: &pyo3::types::PyModule) -> Result<Self> {
        crate::io_utils::with_suppressed_python_output(
            module.py(),
            self.cfg.suppress_python_stdout,
            self.cfg.suppress_python_stderr,
            || {
                self.modules.push(Module::parse(&self.cfg, module)?);
                Ok(())
            },
        )?;
        Ok(self)
    }

    pub fn modules(mut self, modules: &[&pyo3::types::PyModule]) -> Result<Self> {
        self.modules.reserve(modules.len());
        for module in modules {
            self = self.module(module)?;
        }
        Ok(self)
    }

    pub fn generate(mut self) -> Result<proc_macro2::TokenStream> {
        let mut tokens = proc_macro2::TokenStream::new();

        pyo3::Python::with_gil(|py| {
            crate::io_utils::with_suppressed_python_output(
                py,
                self.cfg.suppress_python_stdout,
                self.cfg.suppress_python_stderr,
                || {
                    // Parse external modules (if enabled)
                    if self.cfg.generate_dependencies {
                        self.parse_dependencies()?;
                    }

                    // Canonicalize the module tree
                    self.canonicalize();

                    // Generate the bindings for all modules
                    for module in &self.modules {
                        tokens.extend(module.generate(&self.cfg)?);
                    }
                    Ok(())
                },
            )
        })?;

        Ok(tokens)
    }

    pub fn build(self, output_path: impl AsRef<std::path::Path>) -> Result<()> {
        std::fs::write(output_path, self.generate()?.to_string())?;
        Ok(())
    }

    fn parse_dependencies(&mut self) -> Result<()> {
        // TODO: Parse modules of dependencies
        todo!()
    }
}

impl Canonicalize for Codegen {
    fn canonicalize(&mut self) {
        todo!();
        for module in &mut self.modules {
            module.canonicalize();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_codegen() {
        pyo3::prepare_freethreaded_python();
        pyo3::Python::with_gil(|py| {
            let cfg = Config::default();
            let module = py.import("gymnasium").unwrap();
            let _codegen = Codegen::new(cfg).unwrap().module(module).unwrap();
        });
    }
}
