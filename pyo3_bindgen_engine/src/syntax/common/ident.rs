#[repr(transparent)]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Ident(String);

impl Ident {
    pub fn from_rs(value: &str) -> Self {
        debug_assert!(!value.is_empty());
        Self(value.to_owned())
    }

    pub fn from_py(value: &str) -> Self {
        debug_assert!(!value.is_empty());
        Self(Self::py_to_rs(value))
    }

    pub fn into_rs(self) -> String {
        self.0
    }

    pub fn as_rs(&self) -> &str {
        &self.0
    }

    pub fn as_py(&self) -> &str {
        Self::rs_as_py(&self.0)
    }

    fn rs_as_py(value: &str) -> &str {
        value.strip_prefix("r#").unwrap_or(value)
    }

    fn py_to_rs(value: &str) -> String {
        if syn::parse_str::<syn::Ident>(value).is_ok() {
            value.to_owned()
        } else {
            format!("r#{value}")
        }
    }
}

impl TryFrom<Ident> for syn::Ident {
    type Error = syn::Error;
    fn try_from(value: Ident) -> Result<Self, Self::Error> {
        syn::parse_str::<syn::Ident>(&value.into_rs())
    }
}

impl TryFrom<&Ident> for syn::Ident {
    type Error = syn::Error;
    fn try_from(value: &Ident) -> Result<Self, Self::Error> {
        syn::parse_str::<syn::Ident>(value.as_rs())
    }
}

impl std::cmp::PartialOrd for Ident {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl std::cmp::Ord for Ident {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.as_py().cmp(other.as_py())
    }
}

impl std::ops::Deref for Ident {
    type Target = str;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::fmt::Display for Ident {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self.as_py())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_rs() {
        let ident = Ident::from_rs("ident");
        assert_eq!(ident.as_rs(), "ident");
        assert_eq!(ident.as_py(), "ident");
        assert_eq!(ident.into_rs(), "ident");
    }

    #[test]
    fn test_from_py() {
        let ident = Ident::from_py("ident");
        assert_eq!(ident.as_rs(), "ident");
        assert_eq!(ident.as_py(), "ident");
    }

    #[test]
    fn test_from_py_keyword() {
        let ident = Ident::from_py("struct");
        assert_eq!(ident.as_rs(), "r#struct");
        assert_eq!(ident.as_py(), "struct");
    }

    #[test]
    fn test_into_syn() {
        let ident = Ident::from_rs("ident");
        let _syn_ident: syn::Ident = (&ident).try_into().unwrap();
        let _syn_ident: syn::Ident = ident.try_into().unwrap();
    }
}
