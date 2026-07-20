use serde::{Deserialize, Serialize};

/// A single row in a key/value table (headers, query params, form fields).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyValue {
    pub key: String,
    #[serde(default)]
    pub value: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(
        default = "super::default_true",
        skip_serializing_if = "super::is_true"
    )]
    pub enabled: bool,
}

impl KeyValue {
    pub fn new(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            value: value.into(),
            description: String::new(),
            enabled: true,
        }
    }

    pub fn is_active(&self) -> bool {
        self.enabled && !self.key.is_empty()
    }
}
