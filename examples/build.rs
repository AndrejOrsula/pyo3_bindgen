use pyo3_bindgen::Codegen;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    Codegen::default()
        .module_names(["os", "posixpath", "sys"])?
        .build(format!("{}/bindings.rs", std::env::var("OUT_DIR")?))?;
    Ok(())
}
