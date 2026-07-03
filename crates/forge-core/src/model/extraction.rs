use serde::{Deserialize, Serialize};

/// Extracts a value from a response into a variable for later requests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Extractor {
    #[serde(flatten)]
    pub source: ExtractorSource,
    /// Target variable name (without braces).
    pub var: String,
    #[serde(default)]
    pub scope: ExtractScope,
    #[serde(default = "super::default_true", skip_serializing_if = "super::is_true")]
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "camelCase")]
pub enum ExtractorSource {
    JsonPath {
        expr: String,
    },
    Header {
        name: String,
    },
    Regex {
        pattern: String,
        /// Capture group index; 0 = whole match.
        #[serde(default)]
        group: usize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum ExtractScope {
    /// Lives only for the current run / session.
    #[default]
    Runtime,
    /// Persisted into the active environment file.
    Environment,
}
