pub(crate) fn parse_pyo3_version(manifest: &str) -> Option<&str> {
    let mut in_pyo3_dependency = false;
    for raw_line in manifest.lines() {
        let line = raw_line.trim();
        if line.starts_with('[') {
            in_pyo3_dependency = line == "[dependencies.pyo3]";
            continue;
        }
        if line.starts_with("pyo3 =") {
            if let Some(version) = parse_version_value(line) {
                return Some(version);
            }
        } else if in_pyo3_dependency && line.starts_with("version =") {
            return parse_version_value(line);
        }
    }
    None
}

fn parse_version_value(line: &str) -> Option<&str> {
    let version_start = line.find("version = \"")? + "version = \"".len();
    let version_end = line[version_start..].find('"')?;
    let requirement = &line[version_start..version_start + version_end];
    let version = requirement.strip_prefix('=')?;
    let mut components = version.split('.');
    if components.clone().count() == 3
        && components.all(|component| {
            !component.is_empty() && component.bytes().all(|byte| byte.is_ascii_digit())
        })
    {
        Some(version)
    } else {
        None
    }
}
