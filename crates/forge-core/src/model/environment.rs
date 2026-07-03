use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// `<name>.env.json` — a committed environment definition.
///
/// Secret variables are only *declared* here (`"secret": true`, no value);
/// their values live in the sibling gitignored `<name>.secrets.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Environment {
    #[serde(default = "super::default_format")]
    pub format: u32,
    pub name: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub variables: BTreeMap<String, EnvVar>,
}

impl Environment {
    pub fn new(name: impl Into<String>) -> Self {
        Self { format: crate::FORMAT_VERSION, name: name.into(), variables: BTreeMap::new() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct EnvVar {
    /// Plain value; must be `None` when `secret` is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default, skip_serializing_if = "super::is_false")]
    pub secret: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}

impl EnvVar {
    pub fn plain(value: impl Into<String>) -> Self {
        Self { value: Some(value.into()), secret: false, description: String::new() }
    }

    pub fn secret() -> Self {
        Self { value: None, secret: true, description: String::new() }
    }
}

/// Contents of a `<name>.secrets.json` file: variable name → secret value.
pub type SecretValues = BTreeMap<String, String>;
