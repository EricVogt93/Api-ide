//! Persisted request-document model (format v1). These are the *unresolved*
//! serde types — bindings and refs are still descriptions, not values. See
//! `docs/architecture/request-format-v1.md` and
//! `schemas/request-v1.schema.json`; `deny_unknown_fields` mirrors the
//! schema's `additionalProperties: false`, so a typo'd key is a hard error.

use std::collections::BTreeMap;

use json_patch::PatchOperation;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::model::Method;

/// A single API request document.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestDocument {
    #[serde(rename = "$schema", default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    pub format_version: FormatVersion,
    pub kind: RequestKind,
    pub meta: RequestMeta,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub bindings: BTreeMap<String, Binding>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub matrix: BTreeMap<String, Binding>,
    pub request: RequestSpec,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pipeline: Vec<PipelineEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mock: Option<MockDef>,
}

/// The only supported document format version. Custom (de)serialization
/// (see bottom of file) accepts the integer `1` and rejects anything else.
#[derive(Debug, Clone, Copy)]
pub struct FormatVersion;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RequestKind {
    Request,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestMeta {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

impl RequestMeta {
    pub fn is_regression(&self) -> bool {
        self.tags
            .iter()
            .any(|tag| tag.eq_ignore_ascii_case("regression"))
    }

    pub fn set_regression(&mut self, enabled: bool) {
        self.tags
            .retain(|tag| !tag.eq_ignore_ascii_case("regression"));
        if enabled {
            self.tags.push("regression".to_string());
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestSpec {
    pub method: Method,
    pub url: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<HeaderSpec>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub query: Vec<HeaderSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<BodySpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HeaderSpec {
    pub name: String,
    pub value: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

/// A request or mock body. Either inline (`type` + optional `value`) or a
/// reference to a data asset (`ref` + optional `type`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum BodySpec {
    Inline(InlineBody),
    Ref(RefBody),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InlineBody {
    #[serde(rename = "type")]
    pub body_type: BodyType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RefBody {
    #[serde(rename = "ref")]
    pub reference: String,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub body_type: Option<BodyType>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BodyType {
    Json,
    Text,
    Form,
    Multipart,
    Binary,
    None,
}

/// The unified binding model: `value` | `ref` | `use`. Exactly one, enforced
/// by per-variant `deny_unknown_fields` under an untagged enum.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Binding {
    Value(ValueBinding),
    Ref(RefBinding),
    Use(UseBinding),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValueBinding {
    pub value: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RefBinding {
    #[serde(rename = "ref")]
    pub reference: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub patch: Vec<PatchOperation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UseBinding {
    #[serde(rename = "use")]
    pub uses: String,
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub with: serde_json::Map<String, Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PipelinePhase {
    BeforeRequest,
    AfterResponse,
    OnError,
    Finally,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PipelineEntry {
    pub phase: PipelinePhase,
    #[serde(rename = "use")]
    pub uses: String,
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub with: serde_json::Map<String, Value>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

/// A mock: static (`status` + …) or executable (`use` + `with`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MockDef {
    Static(StaticMock),
    Dynamic(DynamicMock),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StaticMock {
    pub status: u16,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<HeaderSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<BodySpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delay_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DynamicMock {
    #[serde(rename = "use")]
    pub uses: String,
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub with: serde_json::Map<String, Value>,
}

// The top-level container uses camelCase for `format_version`; apply it.
impl RequestDocument {
    /// Parse a document from JSON text. Unknown fields, wrong `formatVersion`
    /// or `kind`, and malformed bindings are hard errors here.
    pub fn parse(text: &str) -> Result<RequestDocument, serde_json::Error> {
        serde_json::from_str(text)
    }

    /// Whether running this document can execute repository-owned code.
    /// Builtins are shipped by Forge; every other `use` target is a project
    /// asset and requires adapter-level trust confirmation.
    pub fn uses_project_code(&self) -> bool {
        let project_use = |uses: &str| !uses.starts_with("builtin:");
        self.bindings
            .values()
            .chain(self.matrix.values())
            .any(|binding| matches!(binding, Binding::Use(binding) if project_use(&binding.uses)))
            || self.pipeline.iter().any(|entry| project_use(&entry.uses))
            || matches!(
                &self.mock,
                Some(MockDef::Dynamic(mock)) if project_use(&mock.uses)
            )
    }

    /// Insert a binding under a stable, collision-free name and return that
    /// name for UI feedback.
    pub fn insert_binding(&mut self, suggested_name: &str, binding: Binding) -> String {
        let base: String = suggested_name
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() || character == '_' {
                    character
                } else {
                    '_'
                }
            })
            .collect();
        let base = base.trim_matches('_');
        let base = if base.is_empty() { "asset" } else { base };
        let mut name = base.to_string();
        let mut suffix = 2;
        while self.bindings.contains_key(&name) {
            name = format!("{base}{suffix}");
            suffix += 1;
        }
        self.bindings.insert(name.clone(), binding);
        name
    }
}

// `formatVersion` is a plain integer that must equal 1. Model it as a unit
// type with custom (de)serialization so any other number is rejected with a
// clear message rather than silently accepted.
impl Serialize for FormatVersion {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u32(1)
    }
}
impl<'de> Deserialize<'de> for FormatVersion {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let n = u32::deserialize(d)?;
        if n == 1 {
            Ok(FormatVersion)
        } else {
            Err(serde::de::Error::custom(format!(
                "unsupported formatVersion {n} (this build supports 1)"
            )))
        }
    }
}

/// `project.json` — aliases and provider order.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectConfig {
    #[serde(default)]
    pub format_version: Option<u32>,
    #[serde(default)]
    pub aliases: BTreeMap<String, String>,
    /// Secret provider order, e.g. ["env"]. Empty = env only.
    #[serde(default)]
    pub secrets: Vec<String>,
    /// One project-wide request that supplies short-lived authentication.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<ProjectAuthConfig>,
}

/// Project-wide auth fetched by running an ordinary request document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectAuthConfig {
    /// Project-relative `*.request.json` used to obtain the token.
    pub request: String,
    /// JSONPath selecting the token from that request's response body.
    #[serde(default = "default_auth_token_path")]
    pub token_path: String,
    /// Token lifetime as supplied by the user/API contract.
    #[serde(default = "default_auth_lifetime_seconds")]
    pub lifetime_seconds: u64,
    /// Always refresh at least this long before expiry.
    #[serde(default = "default_auth_refresh_before_seconds")]
    pub refresh_before_seconds: u64,
    /// Project-relative request folder or file receiving the Bearer header.
    #[serde(default = "default_auth_apply_to")]
    pub apply_to: String,
}

impl ProjectAuthConfig {
    pub fn for_request(request: String) -> Self {
        Self {
            request,
            token_path: default_auth_token_path(),
            lifetime_seconds: default_auth_lifetime_seconds(),
            refresh_before_seconds: default_auth_refresh_before_seconds(),
            apply_to: default_auth_apply_to(),
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.lifetime_seconds == 0 || self.refresh_before_seconds >= self.lifetime_seconds {
            return Err(
                "auth lifetime must be positive and greater than its refresh reserve".to_string(),
            );
        }
        if !self.request.ends_with(".request.json") || self.token_path.trim().is_empty() {
            return Err(
                "auth request must be a *.request.json and tokenPath must not be empty".to_string(),
            );
        }
        for (label, value) in [
            ("auth request", self.request.as_str()),
            ("auth applyTo", self.apply_to.as_str()),
        ] {
            let path = std::path::Path::new(value);
            if value.trim().is_empty()
                || path.is_absolute()
                || path
                    .components()
                    .any(|component| matches!(component, std::path::Component::ParentDir))
            {
                return Err(format!("{label} must be a project-relative path"));
            }
        }
        Ok(())
    }
}

fn default_auth_token_path() -> String {
    "$.access_token".to_string()
}

fn default_auth_lifetime_seconds() -> u64 {
    900
}

fn default_auth_refresh_before_seconds() -> u64 {
    30
}

fn default_auth_apply_to() -> String {
    "requests".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unknown_top_level_field() {
        let doc = r#"{"formatVersion":1,"kind":"request","meta":{"id":"x","name":"x"},
            "request":{"method":"GET","url":"http://x"},"auth":{}}"#;
        assert!(RequestDocument::parse(doc).is_err());
    }

    #[test]
    fn regression_flag_uses_the_existing_metadata_tags() {
        let mut document = RequestDocument::parse(
            r#"{"formatVersion":1,"kind":"request","meta":{"id":"x","name":"x","tags":["smoke"]},
                "request":{"method":"GET","url":"http://x"}}"#,
        )
        .unwrap();

        document.meta.set_regression(true);
        assert!(document.meta.is_regression());
        assert_eq!(document.meta.tags, ["smoke", "regression"]);
        document.meta.set_regression(false);
        assert_eq!(document.meta.tags, ["smoke"]);
    }

    #[test]
    fn rejects_wrong_format_version() {
        let doc = r#"{"formatVersion":2,"kind":"request","meta":{"id":"x","name":"x"},
            "request":{"method":"GET","url":"http://x"}}"#;
        let err = RequestDocument::parse(doc).unwrap_err().to_string();
        assert!(err.contains("formatVersion"), "{err}");
    }

    #[test]
    fn rejects_binding_with_two_shapes() {
        let doc = r#"{"formatVersion":1,"kind":"request","meta":{"id":"x","name":"x"},
            "request":{"method":"GET","url":"http://x"},
            "bindings":{"a":{"value":1,"ref":"data:y"}}}"#;
        assert!(RequestDocument::parse(doc).is_err());
    }

    #[test]
    fn parses_value_ref_and_use_bindings() {
        let doc = r#"{"formatVersion":1,"kind":"request","meta":{"id":"x","name":"x"},
            "request":{"method":"GET","url":"http://x"},
            "bindings":{
              "a":{"value":5},
              "b":{"ref":"data:u#/x","patch":[{"op":"replace","path":"/x","value":1}]},
              "c":{"use":"builtin:uuid@1"}
            }}"#;
        let parsed = RequestDocument::parse(doc).expect("valid");
        assert!(matches!(parsed.bindings["a"], Binding::Value(_)));
        assert!(matches!(parsed.bindings["b"], Binding::Ref(_)));
        assert!(matches!(parsed.bindings["c"], Binding::Use(_)));
    }

    #[test]
    fn parses_canonical_example() {
        let doc =
            include_str!("../../tests/fixtures/reqv1/project/requests/users/create.request.json");
        let parsed = RequestDocument::parse(doc).expect("canonical example must parse");
        assert_eq!(parsed.meta.id, "users.create");
        assert_eq!(parsed.pipeline.len(), 4);
        assert!(parsed.mock.is_some());
    }

    #[test]
    fn detects_only_project_owned_executable_assets() {
        let builtin = RequestDocument::parse(
            r#"{"formatVersion":1,"kind":"request","meta":{"id":"x","name":"x"},
                "request":{"method":"GET","url":"http://x"},
                "pipeline":[{"phase":"afterResponse","use":"builtin:assert-status@1",
                    "with":{"expected":200}}]}"#,
        )
        .unwrap();
        assert!(!builtin.uses_project_code());

        let project = RequestDocument::parse(
            r#"{"formatVersion":1,"kind":"request","meta":{"id":"x","name":"x"},
                "request":{"method":"GET","url":"http://x"},
                "pipeline":[{"phase":"afterResponse","use":"project:assertions/check"}]}"#,
        )
        .unwrap();
        assert!(project.uses_project_code());
    }

    #[test]
    fn inserted_binding_gets_a_safe_unique_name() {
        let mut document = RequestDocument::parse(
            r#"{"formatVersion":1,"kind":"request","meta":{"id":"x","name":"x"},
                "request":{"method":"GET","url":"http://x"},
                "bindings":{"request_tag":{"value":1}}}"#,
        )
        .unwrap();
        let name = document.insert_binding(
            "request-tag",
            Binding::Value(ValueBinding {
                value: Value::Bool(true),
            }),
        );

        assert_eq!(name, "request_tag2");
        assert!(document.bindings.contains_key("request_tag2"));
    }
}
