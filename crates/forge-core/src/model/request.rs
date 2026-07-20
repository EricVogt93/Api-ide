use serde::{Deserialize, Serialize};

use super::{AssertionDef, AuthConfig, BodyDef, Extractor, KeyValue};

/// One request definition — serialized as a `*.request.json` file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestDef {
    #[serde(default = "super::default_format")]
    pub format: u32,
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    pub method: Method,
    /// Raw URL template, may contain `{{variables}}` and `:pathParams`.
    pub url: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub params: Vec<Param>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<KeyValue>,
    #[serde(default)]
    pub auth: AuthConfig,
    #[serde(default)]
    pub body: BodyDef,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assertions: Vec<AssertionDef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extractors: Vec<Extractor>,
    #[serde(default, skip_serializing_if = "Scripts::is_empty")]
    pub scripts: Scripts,
    #[serde(default, skip_serializing_if = "RequestSettings::is_default")]
    pub settings: RequestSettings,
}

impl RequestDef {
    pub fn new(name: impl Into<String>, method: Method, url: impl Into<String>) -> Self {
        Self {
            format: crate::FORMAT_VERSION,
            name: name.into(),
            description: String::new(),
            method,
            url: url.into(),
            params: Vec::new(),
            headers: Vec::new(),
            auth: AuthConfig::Inherit,
            body: BodyDef::None,
            assertions: Vec::new(),
            extractors: Vec::new(),
            scripts: Scripts::default(),
            settings: RequestSettings::default(),
        }
    }
}

/// Query or path parameter row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Param {
    #[serde(flatten)]
    pub kv: KeyValue,
    #[serde(default, skip_serializing_if = "is_query")]
    pub kind: ParamKind,
}

fn is_query(k: &ParamKind) -> bool {
    matches!(k, ParamKind::Query)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum ParamKind {
    #[default]
    Query,
    /// Substituted into `:name` segments of the URL path.
    Path,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Method {
    Get,
    Post,
    Put,
    Patch,
    Delete,
    Head,
    Options,
    Trace,
}

impl Method {
    pub const ALL: [Method; 8] = [
        Method::Get,
        Method::Post,
        Method::Put,
        Method::Patch,
        Method::Delete,
        Method::Head,
        Method::Options,
        Method::Trace,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
            Method::Put => "PUT",
            Method::Patch => "PATCH",
            Method::Delete => "DELETE",
            Method::Head => "HEAD",
            Method::Options => "OPTIONS",
            Method::Trace => "TRACE",
        }
    }

    pub fn parse(s: &str) -> Option<Method> {
        Method::ALL
            .iter()
            .copied()
            .find(|m| m.as_str().eq_ignore_ascii_case(s))
    }

    pub fn has_body_by_default(&self) -> bool {
        matches!(self, Method::Post | Method::Put | Method::Patch)
    }
}

impl std::fmt::Display for Method {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Scripts {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pre_request: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub post_response: Option<String>,
    /// Scripting language `pre_request`/`post_response` are written in.
    #[serde(default, skip_serializing_if = "is_default_lang")]
    pub language: ScriptLang,
}

impl Scripts {
    pub fn is_empty(&self) -> bool {
        self.pre_request.is_none() && self.post_response.is_none()
    }
}

/// Which scripting engine runs a request's (or a suite hook's) scripts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum ScriptLang {
    /// The Rhai scripting engine (the original, still the default).
    #[default]
    Rhai,
    /// Sandboxed JavaScript via QuickJS.
    Js,
}

pub(crate) fn is_default_lang(lang: &ScriptLang) -> bool {
    *lang == ScriptLang::default()
}

/// Per-request overrides of workspace-level execution settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RequestSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub follow_redirects: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_redirects: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verify_tls: Option<bool>,
    /// Skip this request during collection runs.
    #[serde(default, skip_serializing_if = "super::is_false")]
    pub skip_in_runs: bool,
}

impl RequestSettings {
    pub fn is_default(&self) -> bool {
        self == &RequestSettings::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_language_js_round_trips() {
        let scripts = Scripts {
            pre_request: Some("req.url = req.url;".to_string()),
            post_response: None,
            language: ScriptLang::Js,
        };
        let json = serde_json::to_string(&scripts).expect("serialize");
        assert!(json.contains(r#""language":"js""#), "{json}");
        let back: Scripts = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, scripts);
    }

    #[test]
    fn scripts_default_language_is_rhai_and_omitted_from_json() {
        let scripts = Scripts {
            pre_request: Some("x".to_string()),
            ..Default::default()
        };
        assert_eq!(scripts.language, ScriptLang::Rhai);
        let json = serde_json::to_string(&scripts).expect("serialize");
        assert!(
            !json.contains("language"),
            "default language must be omitted: {json}"
        );
        // Legacy documents without a language field parse as Rhai.
        let back: Scripts = serde_json::from_str(r#"{"preRequest":"x"}"#).expect("deserialize");
        assert_eq!(back.language, ScriptLang::Rhai);
    }
}
