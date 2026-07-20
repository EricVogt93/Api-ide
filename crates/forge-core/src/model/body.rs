use serde::{Deserialize, Serialize};

use super::KeyValue;

/// Request body variants.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum BodyDef {
    #[default]
    None,
    /// Free-form text with an editor language hint.
    Raw {
        text: String,
        #[serde(default)]
        language: RawLanguage,
    },
    Json {
        text: String,
    },
    Xml {
        text: String,
    },
    #[serde(rename_all = "camelCase")]
    FormUrlencoded {
        fields: Vec<KeyValue>,
    },
    Multipart {
        parts: Vec<MultipartPart>,
    },
    #[serde(rename_all = "camelCase")]
    GraphQl {
        query: String,
        /// JSON object with GraphQL variables (kept as text for editing).
        #[serde(default, skip_serializing_if = "String::is_empty")]
        variables: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        operation_name: Option<String>,
    },
    Binary {
        /// Path relative to the workspace root (or absolute).
        path: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum RawLanguage {
    #[default]
    Text,
    Json,
    Xml,
    Html,
    Yaml,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MultipartPart {
    pub name: String,
    #[serde(flatten)]
    pub content: PartContent,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    #[serde(
        default = "super::default_true",
        skip_serializing_if = "super::is_true"
    )]
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum PartContent {
    Text { value: String },
    File { path: String },
}

impl BodyDef {
    /// The text the body editor should show, if this body kind is text-based.
    pub fn editor_text(&self) -> Option<&str> {
        match self {
            BodyDef::Raw { text, .. } | BodyDef::Json { text } | BodyDef::Xml { text } => {
                Some(text)
            }
            BodyDef::GraphQl { query, .. } => Some(query),
            _ => None,
        }
    }
}
