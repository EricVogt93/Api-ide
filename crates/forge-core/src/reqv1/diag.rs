//! Diagnostics: every resolution/validation/execution failure is a typed
//! [`Diagnostic`] with a stable `code`, a human message, and — where known —
//! a JSON Pointer into the request document (`instance_path`) and the
//! offending asset ref. See `docs/architecture/request-format-v1.md` §17.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
    Info,
}

/// Stable diagnostic codes (§6, §8). String-typed on the wire so new codes
/// don't break consumers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Code {
    SchemaInvalid,
    AssetNotFound,
    InvalidAlias,
    InvalidPointer,
    JsonPatchFailed,
    ReferenceCycle,
    BindingCycle,
    InvalidAssetInput,
    IncompatibleAssetType,
    UnsupportedAssetVersion,
    PathEscape,
    MissingVariable,
    NullInString,
    StructuredInString,
    UnknownNamespace,
    PipelineConflict,
    AssetError,
    HttpError,
}

impl Code {
    pub fn as_str(self) -> &'static str {
        match self {
            Code::SchemaInvalid => "SCHEMA_INVALID",
            Code::AssetNotFound => "ASSET_NOT_FOUND",
            Code::InvalidAlias => "INVALID_ALIAS",
            Code::InvalidPointer => "INVALID_POINTER",
            Code::JsonPatchFailed => "JSON_PATCH_FAILED",
            Code::ReferenceCycle => "REFERENCE_CYCLE",
            Code::BindingCycle => "BINDING_CYCLE",
            Code::InvalidAssetInput => "INVALID_ASSET_INPUT",
            Code::IncompatibleAssetType => "INCOMPATIBLE_ASSET_TYPE",
            Code::UnsupportedAssetVersion => "UNSUPPORTED_ASSET_VERSION",
            Code::PathEscape => "PATH_ESCAPE",
            Code::MissingVariable => "MISSING_VARIABLE",
            Code::NullInString => "NULL_IN_STRING",
            Code::StructuredInString => "STRUCTURED_IN_STRING",
            Code::UnknownNamespace => "UNKNOWN_NAMESPACE",
            Code::PipelineConflict => "PIPELINE_CONFLICT",
            Code::AssetError => "ASSET_ERROR",
            Code::HttpError => "HTTP_ERROR",
        }
    }

    fn severity(self) -> Severity {
        match self {
            Code::PipelineConflict => Severity::Warning,
            _ => Severity::Error,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asset_ref: Option<String>,
}

impl Diagnostic {
    pub fn new(code: Code, message: impl Into<String>) -> Self {
        Self {
            severity: code.severity(),
            code: code.as_str().to_string(),
            message: message.into(),
            instance_path: None,
            asset_ref: None,
        }
    }

    pub fn at(mut self, instance_path: impl Into<String>) -> Self {
        self.instance_path = Some(instance_path.into());
        self
    }

    pub fn with_ref(mut self, asset_ref: impl Into<String>) -> Self {
        self.asset_ref = Some(asset_ref.into());
        self
    }

    /// Downgrade to info severity (e.g. "skipped, not an error").
    pub fn info(mut self) -> Self {
        self.severity = Severity::Info;
        self
    }

    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }
}

/// A resolution/execution error carrying one or more diagnostics. Resolution
/// collects independent errors before failing (§7), so this holds a list.
#[derive(Debug, Clone)]
pub struct Errors(pub Vec<Diagnostic>);

impl Errors {
    pub fn one(code: Code, message: impl Into<String>) -> Self {
        Errors(vec![Diagnostic::new(code, message)])
    }

    pub fn has_errors(&self) -> bool {
        self.0.iter().any(Diagnostic::is_error)
    }
}

impl std::fmt::Display for Errors {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for d in &self.0 {
            writeln!(f, "[{}] {}", d.code, d.message)?;
        }
        Ok(())
    }
}

impl std::error::Error for Errors {}
