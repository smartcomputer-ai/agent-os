use alloc::string::String;
use core::fmt;
use core::str::FromStr;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefError {
    InvalidHash { value: String },
    InvalidName { value: String },
}

impl fmt::Display for RefError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidHash { value } => write!(
                f,
                "invalid hash '{value}': must start with 'sha256:' followed by 64 hex chars"
            ),
            Self::InvalidName { value } => {
                write!(f, "invalid name '{value}': expected namespace/name@version")
            }
        }
    }
}

impl core::error::Error for RefError {}

#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct HashRef(String);

impl HashRef {
    pub fn new(value: impl Into<String>) -> Result<Self, RefError> {
        let value = value.into();
        if is_valid_hash(&value) {
            Ok(Self(value))
        } else {
            Err(RefError::InvalidHash { value })
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for HashRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Display for HashRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for HashRef {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for HashRef {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Self::new(s).map_err(serde::de::Error::custom)
    }
}

impl FromStr for HashRef {
    type Err = RefError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct SchemaRef(String);

impl SchemaRef {
    pub fn new(value: impl Into<String>) -> Result<Self, RefError> {
        let value = value.into();
        if is_valid_name(&value) {
            Ok(Self(value))
        } else {
            Err(RefError::InvalidName { value })
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SchemaRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Display for SchemaRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for SchemaRef {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for SchemaRef {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Self::new(s).map_err(serde::de::Error::custom)
    }
}

impl FromStr for SchemaRef {
    type Err = RefError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(deny_unknown_fields)]
pub struct SecretRef {
    pub alias: String,
    pub version: u64,
}

fn is_valid_hash(value: &str) -> bool {
    const PREFIX: &str = "sha256:";
    value.starts_with(PREFIX)
        && value.len() == PREFIX.len() + 64
        && value[PREFIX.len()..].chars().all(|c| c.is_ascii_hexdigit())
}

fn is_valid_name(value: &str) -> bool {
    let (ns, rest) = match value.split_once('/') {
        Some(parts) => parts,
        None => return false,
    };
    if !is_valid_namespace(ns) {
        return false;
    }
    let (name, version) = match rest.split_once('@') {
        Some(parts) => parts,
        None => return false,
    };
    if !is_valid_identifier(name) {
        return false;
    }
    is_valid_version(version)
}

fn is_valid_namespace(ns: &str) -> bool {
    if ns.is_empty() {
        return false;
    }
    let mut chars = ns.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() => {}
        _ => return false,
    }
    chars.all(|c| matches!(c, 'a'..='z' | '0'..='9' | '.' | '_' | '-'))
}

fn is_valid_identifier(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| matches!(c, 'a'..='z' | 'A'..='Z' | '0'..='9' | '.' | '_' | '-'))
}

fn is_valid_version(version: &str) -> bool {
    if version.is_empty() || version.starts_with('0') {
        return false;
    }
    version.chars().all(|c| c.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_validation() {
        assert!(
            HashRef::new("sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef")
                .is_ok()
        );
        assert!(HashRef::new("sha256:zzz").is_err());
    }

    #[test]
    fn schema_validation() {
        assert!(SchemaRef::new("com.acme/foo@1").is_ok());
        assert!(SchemaRef::new("Com/foo@1").is_err());
        assert!(SchemaRef::new("com/foo@01").is_err());
    }
}
