//! Persisted, ordered request sequences.

use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::model::{FormatVersion, RequestMeta};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SequenceDocument {
    #[serde(rename = "$schema", default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    pub format_version: FormatVersion,
    pub kind: SequenceKind,
    pub meta: RequestMeta,
    pub requests: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SequenceKind {
    Sequence,
}

impl SequenceDocument {
    pub fn parse(text: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(text)
    }

    /// Resolve project-relative request paths without allowing project escape.
    pub fn resolve_files(&self, root: &Path) -> Result<Vec<PathBuf>, String> {
        if self.requests.is_empty() {
            return Err("sequence must contain at least one request".to_string());
        }
        self.requests
            .iter()
            .map(|request| {
                let relative = Path::new(request);
                if relative.is_absolute()
                    || relative
                        .components()
                        .any(|component| matches!(component, Component::ParentDir))
                {
                    return Err(format!(
                        "sequence request must stay inside the project: {request}"
                    ));
                }
                if !request.ends_with(".request.json") {
                    return Err(format!(
                        "sequence entry is not a request document: {request}"
                    ));
                }
                let path = root.join(relative);
                if !path.is_file() {
                    return Err(format!(
                        "sequence request does not exist: {}",
                        path.display()
                    ));
                }
                Ok(path)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_order_and_rejects_escape() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("requests")).unwrap();
        for name in ["a", "b"] {
            std::fs::write(
                dir.path().join(format!("requests/{name}.request.json")),
                "{}",
            )
            .unwrap();
        }
        let mut sequence = SequenceDocument::parse(
            r#"{"formatVersion":1,"kind":"sequence","meta":{"id":"s","name":"S"},
                "requests":["requests/b.request.json","requests/a.request.json"]}"#,
        )
        .unwrap();

        let files = sequence.resolve_files(dir.path()).unwrap();
        assert!(files[0].ends_with("b.request.json"));
        assert!(files[1].ends_with("a.request.json"));

        sequence.requests = vec!["../outside.request.json".to_string()];
        assert!(sequence.resolve_files(dir.path()).is_err());
    }

    #[test]
    fn shipped_schema_accepts_a_sequence() {
        let schema: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(
                Path::new(env!("CARGO_MANIFEST_DIR")).join("../../schemas/sequence-v1.schema.json"),
            )
            .unwrap(),
        )
        .unwrap();
        let validator = jsonschema::validator_for(&schema).unwrap();
        let document = serde_json::json!({
            "formatVersion": 1,
            "kind": "sequence",
            "meta": {"id": "smoke", "name": "Smoke"},
            "requests": ["requests/a.request.json"]
        });

        assert!(validator.is_valid(&document));
    }
}
