use super::Ident;
use itertools::Itertools;

#[derive(Debug, Default, Clone, PartialEq, Eq, Hash)]
pub struct Path {
    pub leading_colon: bool,
    segments: Vec<Ident>,
}

impl Path {
    pub fn from_rs(value: &str) -> Self {
        if value.is_empty() {
            return Self::default();
        }
        debug_assert!(!value.contains('.'), "Invalid Rust path: {value}");
        Self {
            leading_colon: value.starts_with("::"),
            segments: value
                .split("::")
                .filter(|s| !s.is_empty())
                .map(Ident::from_rs)
                .collect(),
        }
    }

    pub fn from_py(value: &str) -> Self {
        if value.is_empty() {
            return Self::default();
        }
        debug_assert!(!value.contains("::"), "Invalid Python path: {value}");
        Self {
            leading_colon: false,
            segments: std::iter::repeat(Ident::from_rs("super"))
                .take(value.chars().take_while(|&c| c == '.').count())
                .chain(
                    value
                        .split('.')
                        .filter(|s| !s.is_empty())
                        .map(Ident::from_py),
                )
                .collect_vec(),
        }
    }

    pub fn into_rs(self) -> String {
        std::iter::repeat(String::new())
            .take(usize::from(self.leading_colon))
            .chain(self.segments.into_iter().map(Ident::into_rs))
            .collect_vec()
            .join("::")
    }

    pub fn to_rs(&self) -> String {
        std::iter::repeat("")
            .take(usize::from(self.leading_colon))
            .chain(self.segments.iter().map(Ident::as_rs))
            .collect_vec()
            .join("::")
    }

    pub fn to_py(&self) -> String {
        self.segments
            .iter()
            .map(Ident::as_py)
            .map(|s| if s == "super" { "" } else { s })
            .collect_vec()
            .join(".")
    }

    pub fn join(&self, other: &Path) -> Self {
        assert!(
            !other.leading_colon,
            "Leading colon is not allowed in the second path when joining"
        );
        Self {
            leading_colon: self.leading_colon,
            segments: self
                .segments
                .iter()
                .cloned()
                .chain(other.iter().cloned())
                .collect(),
        }
    }

    pub fn concat(&self, other: &Path) -> Self {
        assert!(
            !other.leading_colon,
            "Leading colon is not allowed in the second path when concatenating"
        );
        Self {
            leading_colon: self.leading_colon,
            segments: self
                .segments
                .iter()
                .chain(&other.segments)
                .cloned()
                .collect(),
        }
    }

    pub fn name(&self) -> &Ident {
        self.segments.last().unwrap()
    }

    pub fn root(&self) -> Option<Self> {
        if self.segments.is_empty() {
            None
        } else {
            Some(Self {
                leading_colon: self.leading_colon,
                segments: vec![self.segments[0].clone()],
            })
        }
    }

    pub fn parent(&self) -> Option<Self> {
        if self.segments.len() > 1 {
            Some(Self {
                leading_colon: self.leading_colon,
                segments: self.segments[..self.segments.len() - 1].to_vec(),
            })
        } else {
            None
        }
    }

    /// Define a fully qualified path from self to target.
    /// Use self if they start at the same point.
    /// Use super to go up the hierarchy.
    /// If they do not share any common prefix, use super until the nothing is reached
    pub fn relative_to(&self, target: &Path, fully_unambiguous: bool) -> Self {
        if self == target {
            return if fully_unambiguous {
                Path {
                    leading_colon: false,
                    segments: vec![Ident::from_rs("super"), target.name().clone()],
                }
            } else {
                Path {
                    leading_colon: false,
                    segments: vec![Ident::from_rs("self")],
                }
            };
        }

        // Find the length of the common prefix
        let common_prefix_length = self
            .segments
            .iter()
            .zip(target.segments.iter())
            .take_while(|(a, b)| a == b)
            .count();

        // Determine the relative path
        let mut relative_segments = if fully_unambiguous {
            match common_prefix_length {
                n if n < self.segments.len() => std::iter::repeat(Ident::from_rs("super"))
                    .take(self.segments.len() - n)
                    .chain(target.segments.iter().skip(n).cloned())
                    .collect_vec(),
                n if n == self.segments.len() => std::iter::once(Ident::from_rs("self"))
                    .chain(target.segments.iter().skip(n).cloned())
                    .collect_vec(),
                _ => {
                    unreachable!()
                }
            }
        } else {
            match common_prefix_length {
                n if n < self.segments.len() => std::iter::repeat(Ident::from_rs("super"))
                    .take(self.segments.len() - n)
                    .chain(target.segments.iter().skip(n).cloned())
                    .collect_vec(),
                n if n == self.segments.len() => {
                    target.segments.iter().skip(n).cloned().collect_vec()
                }
                _ => {
                    unreachable!()
                }
            }
        };

        if fully_unambiguous {
            // If the relative segment ends with "super", fully specify the path by adding another "super" and the name of the target
            if relative_segments.last().map(Ident::as_rs) == Some("super") {
                relative_segments.extend([Ident::from_rs("super"), target.name().clone()]);
            }
        }

        Path {
            leading_colon: false,
            segments: relative_segments,
        }
    }

    pub fn import_quote(&self, py: pyo3::Python) -> proc_macro2::TokenStream {
        // Find the last package and import it via py.import, then get the rest of the path via getattr()
        let mut package_path = self.root().unwrap_or_else(|| unreachable!());
        for i in (1..self.len()).rev() {
            let module_name = Self::from(&self[..i]);
            if py.import(module_name.to_py().as_str()).is_ok() {
                package_path = module_name;
                break;
            }
        }

        // Resolve the remaining path
        let remaining_path = self
            .strip_prefix(package_path.segments.as_slice())
            .unwrap_or_else(|| unreachable!());

        // Convert paths to strings
        let package_path = package_path.to_py();
        let remaining_path = remaining_path
            .iter()
            .map(|ident| ident.as_py().to_owned())
            .collect_vec();

        // Generate the import code
        quote::quote! {
            py.import(::pyo3::intern!(py, #package_path))?#(.getattr(::pyo3::intern!(py, #remaining_path))?)*
        }
    }
}

impl From<Ident> for Path {
    fn from(ident: Ident) -> Self {
        Self {
            leading_colon: false,
            segments: vec![ident],
        }
    }
}

impl From<&[Ident]> for Path {
    fn from(segments: &[Ident]) -> Self {
        Self {
            leading_colon: false,
            segments: segments.to_owned(),
        }
    }
}

impl std::cmp::PartialOrd for Path {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl std::cmp::Ord for Path {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.to_py().cmp(&other.to_py())
    }
}

impl TryFrom<Path> for syn::Path {
    type Error = syn::Error;
    fn try_from(value: Path) -> Result<Self, Self::Error> {
        syn::parse_str::<syn::Path>(&value.into_rs())
    }
}

impl TryFrom<&Path> for syn::Path {
    type Error = syn::Error;
    fn try_from(value: &Path) -> Result<Self, Self::Error> {
        syn::parse_str::<syn::Path>(&value.to_rs())
    }
}

impl std::ops::Deref for Path {
    type Target = [Ident];
    fn deref(&self) -> &Self::Target {
        &self.segments
    }
}

impl std::ops::DerefMut for Path {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.segments
    }
}

impl std::fmt::Display for Path {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_py())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_rs() {
        let path = Path::from_rs("long::path::to");
        assert_eq!(path.to_rs(), "long::path::to");
        assert_eq!(path.to_py(), "long.path.to");
        assert_eq!(path.into_rs(), "long::path::to");
    }

    #[test]
    fn test_from_rs_leading_colon() {
        let path = Path::from_rs("::long::path::to");
        assert_eq!(path.to_rs(), "::long::path::to");
        assert_eq!(path.to_py(), "long.path.to");
    }

    #[test]
    fn test_from_py() {
        let path = Path::from_py("long.path.to");
        assert_eq!(path.to_py(), "long.path.to");
        assert_eq!(path.to_rs(), "long::path::to");
    }

    #[test]
    fn test_from_py_relative() {
        let path = Path::from_py("..long.path.to");
        assert_eq!(path.to_py(), "..long.path.to");
        assert_eq!(path.to_rs(), "super::super::long::path::to");
    }

    #[test]
    fn test_from_py_keyword() {
        let path = Path::from_py("mod.struct");
        assert_eq!(path.to_py(), "mod.struct");
        assert_eq!(path.to_rs(), "r#mod::r#struct");
    }

    #[test]
    fn test_name() {
        let path = Path::from_rs("long::path::to");
        assert_eq!(path.name().as_rs(), "to");
    }

    #[test]
    fn test_root() {
        let path = Path::from_rs("long::path::to");
        assert_eq!(path.root().unwrap().to_rs(), "long");
    }

    #[test]
    fn test_parent() {
        let path = Path::from_rs("long::path::to");
        assert_eq!(path.parent().unwrap().to_rs(), "long::path");
    }

    #[test]
    fn test_into_syn() {
        let path = Path::from_rs("long::path::to");
        let _syn_path: syn::Path = (&path).try_into().unwrap();
        let _syn_path: syn::Path = path.try_into().unwrap();
    }
}
