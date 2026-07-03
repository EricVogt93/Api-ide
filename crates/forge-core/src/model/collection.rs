use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::AuthConfig;

/// `collection.json` — metadata at the root of a collection directory.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectionMeta {
    #[serde(default = "super::default_format")]
    pub format: u32,
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub variables: BTreeMap<String, String>,
    #[serde(default)]
    pub auth: AuthConfig,
    /// Explicit ordering of child entries (file / directory names).
    /// Entries missing from the list are appended alphabetically.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub order: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub openapi: Option<OpenApiBinding>,
}

impl CollectionMeta {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            format: crate::FORMAT_VERSION,
            name: name.into(),
            description: String::new(),
            variables: BTreeMap::new(),
            auth: AuthConfig::None,
            order: Vec::new(),
            openapi: None,
        }
    }
}

/// `folder.json` — metadata of a sub-folder inside a collection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct FolderMeta {
    #[serde(default = "super::default_format")]
    pub format: u32,
    /// Display name; defaults to the directory name when empty.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub variables: BTreeMap<String, String>,
    #[serde(default)]
    pub auth: AuthConfig,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub order: Vec<String>,
}

/// Binding of a collection to an imported OpenAPI spec.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenApiBinding {
    /// Spec file path relative to the workspace root (e.g. `specs/api.yaml`).
    pub spec_path: String,
    /// request file path (relative to the collection dir) → operationId.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub operations: BTreeMap<String, String>,
}
