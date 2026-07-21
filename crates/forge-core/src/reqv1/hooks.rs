//! Request-adjacent hook documents.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::model::{FormatVersion, PipelineEntry, RequestDocument};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HookDocument {
    #[serde(rename = "$schema", default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    pub format_version: FormatVersion,
    pub kind: HookKind,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hooks: Vec<PipelineEntry>,
}

impl Default for HookDocument {
    fn default() -> Self {
        Self {
            schema: None,
            format_version: FormatVersion,
            kind: HookKind::Hooks,
            hooks: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HookKind {
    Hooks,
}

impl HookDocument {
    pub fn parse(text: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(text)
    }

    pub fn load_for_request(request: &Path) -> Result<Self, String> {
        let path = hooks_path(request);
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                Self::parse(&text).map_err(|error| format!("invalid {}: {error}", path.display()))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(format!("cannot read {}: {error}", path.display())),
        }
    }

    pub fn save_for_request(&self, request: &Path) -> Result<(), String> {
        let path = hooks_path(request);
        if self.hooks.is_empty() {
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
        for hook in std::mem::take(&mut request.pipeline) {
            document.push(hook);
        }
        document
    }

    pub fn extend(&mut self, other: Self) {
        for hook in other.hooks {
            self.push(hook);
        }
    }

    pub fn apply_to(&self, request: &mut RequestDocument) {
        for hook in &self.hooks {
            if !request
                .pipeline
                .iter()
                .any(|existing| same_entry(existing, hook))
            {
                request.pipeline.push(hook.clone());
            }
        }
    }

    pub fn push(&mut self, hook: PipelineEntry) {
        if !self
            .hooks
            .iter()
            .any(|existing| same_entry(existing, &hook))
        {
            self.hooks.push(hook);
        }
    }
}

pub fn hooks_path(request: &Path) -> PathBuf {
    let name = request
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("request.request.json");
    let stem = name.strip_suffix(".request.json").unwrap_or(name);
    request.with_file_name(format!("{stem}.hooks.json"))
}

fn same_entry(left: &PipelineEntry, right: &PipelineEntry) -> bool {
    left.phase == right.phase
        && left.uses == right.uses
        && left.with == right.with
        && left.enabled == right.enabled
}

#[cfg(test)]
mod tests {
    use super::super::model::PipelinePhase;
    use super::*;

    #[test]
    fn saved_hook_sidecar_is_loaded_into_the_effective_request() {
        let dir = tempfile::tempdir().unwrap();
        let request = dir.path().join("get.request.json");
        std::fs::write(
            &request,
            r#"{"formatVersion":1,"kind":"request","meta":{"id":"x","name":"x"},
                "request":{"method":"GET","url":"https://example.test"}}"#,
        )
        .unwrap();
        HookDocument {
            hooks: vec![PipelineEntry {
                phase: PipelinePhase::BeforeRequest,
                uses: "builtin:header@1".to_string(),
                with: serde_json::Map::from_iter([
                    ("name".to_string(), "X-Test".into()),
                    ("value".to_string(), "yes".into()),
                ]),
                enabled: true,
            }],
            ..HookDocument::default()
        }
        .save_for_request(&request)
        .unwrap();

        let loaded = crate::reqv1::load_request_document(&request).unwrap();

        assert_eq!(
            hooks_path(Path::new("requests/users.get.request.json")),
            Path::new("requests/users.get.hooks.json")
        );
        assert_eq!(loaded.pipeline.len(), 1);
        assert_eq!(loaded.pipeline[0].uses, "builtin:header@1");
    }

    #[test]
    fn shipped_schema_accepts_a_hook_document() {
        let schema: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(
                Path::new(env!("CARGO_MANIFEST_DIR")).join("../../schemas/hooks-v1.schema.json"),
            )
            .unwrap(),
        )
        .unwrap();
        let validator = jsonschema::validator_for(&schema).unwrap();
        let document = serde_json::json!({
            "formatVersion": 1,
            "kind": "hooks",
            "hooks": [{
                "phase": "beforeRequest",
                "use": "builtin:header@1",
                "with": {"name": "X-Test", "value": "yes"}
            }]
        });

        assert!(validator.is_valid(&document));
    }
}
