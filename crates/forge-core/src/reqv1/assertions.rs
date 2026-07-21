//! Request-adjacent assertion documents.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::catalog::{find_builtin, BuiltinIntent};
use super::model::{FormatVersion, PipelineEntry, PipelinePhase, RequestDocument};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssertionDocument {
    #[serde(rename = "$schema", default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    pub format_version: FormatVersion,
    pub kind: AssertionKind,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assertions: Vec<AssertionEntry>,
}

impl Default for AssertionDocument {
    fn default() -> Self {
        Self {
            schema: None,
            format_version: FormatVersion,
            kind: AssertionKind::Assertions,
            assertions: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AssertionKind {
    Assertions,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssertionEntry {
    #[serde(rename = "use")]
    pub uses: String,
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub with: serde_json::Map<String, Value>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

impl AssertionDocument {
    pub fn parse(text: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(text)
    }

    pub fn load_for_request(request: &Path) -> Result<Self, String> {
        let path = assertions_path(request);
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                Self::parse(&text).map_err(|error| format!("invalid {}: {error}", path.display()))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(format!("cannot read {}: {error}", path.display())),
        }
    }

    pub fn save_for_request(&self, request: &Path) -> Result<(), String> {
        let path = assertions_path(request);
        if self.assertions.is_empty() {
            return match std::fs::remove_file(&path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(format!("cannot remove {}: {error}", path.display())),
            };
        }
        let mut text = serde_json::to_string_pretty(self).map_err(|error| error.to_string())?;
        text.push('\n');
        std::fs::write(&path, text)
            .map_err(|error| format!("cannot write {}: {error}", path.display()))
    }

    pub fn take_from_request(request: &mut RequestDocument) -> Self {
        let mut document = Self::default();
        request.pipeline.retain(|entry| {
            if is_assertion(entry) {
                document.push(entry.clone().into());
                false
            } else {
                true
            }
        });
        document
    }

    pub fn extend(&mut self, other: Self) {
        for assertion in other.assertions {
            self.push(assertion);
        }
    }

    pub fn apply_to(&self, request: &mut RequestDocument) {
        for assertion in &self.assertions {
            let entry = PipelineEntry {
                phase: PipelinePhase::AfterResponse,
                uses: assertion.uses.clone(),
                with: assertion.with.clone(),
                enabled: assertion.enabled,
            };
            if !request
                .pipeline
                .iter()
                .any(|existing| same_entry(existing, &entry))
            {
                request.pipeline.push(entry);
            }
        }
    }

    pub fn push(&mut self, assertion: AssertionEntry) {
        if !self.assertions.contains(&assertion) {
            self.assertions.push(assertion);
        }
    }
}

impl From<PipelineEntry> for AssertionEntry {
    fn from(entry: PipelineEntry) -> Self {
        Self {
            uses: entry.uses,
            with: entry.with,
            enabled: entry.enabled,
        }
    }
}

pub fn assertions_path(request: &Path) -> PathBuf {
    let name = request
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("request.request.json");
    let stem = name.strip_suffix(".request.json").unwrap_or(name);
    request.with_file_name(format!("{stem}.assertions.json"))
}

pub fn load_request_document(path: &Path) -> Result<RequestDocument, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let mut request = RequestDocument::parse(&text)
        .map_err(|error| format!("invalid {}: {error}", path.display()))?;
    super::hooks::HookDocument::load_for_request(path)?.apply_to(&mut request);
    AssertionDocument::load_for_request(path)?.apply_to(&mut request);
    Ok(request)
}

fn is_assertion(entry: &PipelineEntry) -> bool {
    if entry.phase != PipelinePhase::AfterResponse {
        return false;
    }
    let builtin = entry
        .uses
        .strip_prefix("builtin:")
        .and_then(|reference| reference.split('@').next())
        .and_then(find_builtin)
        .is_some_and(|definition| definition.intent == BuiltinIntent::Validate);
    let normalized = entry.uses.replace('\\', "/");
    builtin
        || normalized.starts_with("assertions/")
        || normalized.contains("/assertions/")
        || normalized.contains(":assertions/")
}

fn same_entry(left: &PipelineEntry, right: &PipelineEntry) -> bool {
    left.phase == right.phase
        && left.uses == right.uses
        && left.with == right.with
        && left.enabled == right.enabled
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_sidecar_path_and_moves_only_assertions() {
        let mut request = RequestDocument::parse(
            r#"{"formatVersion":1,"kind":"request","meta":{"id":"x","name":"x"},
                "request":{"method":"GET","url":"https://example.test"},
                "pipeline":[
                  {"phase":"afterResponse","use":"builtin:assert-status@1","with":{"expected":200}},
                  {"phase":"afterResponse","use":"builtin:extract-header@1","with":{"name":"X-Id","target":"id"}}
                ]}"#,
        )
        .unwrap();

        let assertions = AssertionDocument::take_from_request(&mut request);

        assert_eq!(
            assertions_path(Path::new("requests/users.get.request.json")),
            Path::new("requests/users.get.assertions.json")
        );
        assert_eq!(assertions.assertions.len(), 1);
        assert_eq!(request.pipeline.len(), 1);
    }

    #[test]
    fn saved_sidecar_is_loaded_into_the_effective_request() {
        let dir = tempfile::tempdir().unwrap();
        let request = dir.path().join("get.request.json");
        std::fs::write(
            &request,
            r#"{"formatVersion":1,"kind":"request","meta":{"id":"x","name":"x"},
                "request":{"method":"GET","url":"https://example.test"}}"#,
        )
        .unwrap();
        AssertionDocument {
            assertions: vec![AssertionEntry {
                uses: "builtin:assert-status@1".to_string(),
                with: serde_json::Map::from_iter([("expected".to_string(), Value::from(200))]),
                enabled: true,
            }],
            ..AssertionDocument::default()
        }
        .save_for_request(&request)
        .unwrap();

        let loaded = load_request_document(&request).unwrap();

        assert_eq!(loaded.pipeline.len(), 1);
        assert_eq!(loaded.pipeline[0].phase, PipelinePhase::AfterResponse);
    }

    #[test]
    fn shipped_schema_accepts_an_assertion_document() {
        let schema: Value = serde_json::from_str(
            &std::fs::read_to_string(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../schemas/assertions-v1.schema.json"),
            )
            .unwrap(),
        )
        .unwrap();
        let validator = jsonschema::validator_for(&schema).unwrap();
        let document = serde_json::json!({
            "formatVersion": 1,
            "kind": "assertions",
            "assertions": [{
                "use": "builtin:assert-status@1",
                "with": {"expected": 200}
            }]
        });

        assert!(validator.is_valid(&document));
    }
}
