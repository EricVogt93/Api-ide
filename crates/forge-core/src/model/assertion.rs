use serde::{Deserialize, Serialize};

/// A declarative assertion attached to a request, evaluated after execution.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssertionDef {
    #[serde(flatten)]
    pub check: Check,
    #[serde(
        default = "super::default_true",
        skip_serializing_if = "super::is_true"
    )]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
}

impl From<Check> for AssertionDef {
    fn from(check: Check) -> Self {
        Self {
            check,
            enabled: true,
            note: String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Check {
    /// Compare the HTTP status code.
    StatusCode {
        op: NumberOp,
        value: u16,
    },
    /// Status code class shortcut: 2 = 2xx, 4 = 4xx ...
    StatusClass {
        class: u8,
    },
    Header {
        name: String,
        op: StringOp,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        value: String,
    },
    ContentType {
        value: String,
    },
    /// Evaluate a JSONPath expression against the response body.
    JsonPath {
        path: String,
        op: ValueOp,
        #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
        value: serde_json::Value,
    },
    BodyContains {
        value: String,
    },
    BodyMatches {
        regex: String,
    },
    #[serde(rename_all = "camelCase")]
    ResponseTimeBelow {
        max_ms: u64,
    },
    /// Validate the (JSON) response body against a JSON Schema.
    JsonSchema {
        schema: serde_json::Value,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NumberOp {
    Eq,
    Ne,
    Lt,
    Lte,
    Gt,
    Gte,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StringOp {
    Equals,
    NotEquals,
    Contains,
    NotContains,
    Matches,
    Exists,
    NotExists,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ValueOp {
    Equals,
    NotEquals,
    Contains,
    Matches,
    Exists,
    NotExists,
    Lt,
    Lte,
    Gt,
    Gte,
}

impl Check {
    /// Human-readable one-line summary used in tables and test trees.
    pub fn summary(&self) -> String {
        match self {
            Check::StatusCode { op, value } => format!("status {} {}", op.symbol(), value),
            Check::StatusClass { class } => format!("status is {class}xx"),
            Check::Header { name, op, value } => {
                format!("header {name} {op:?} {value}").to_lowercase()
            }
            Check::ContentType { value } => format!("content-type is {value}"),
            Check::JsonPath { path, op, value } => match op {
                ValueOp::Exists | ValueOp::NotExists => format!("{path} {op:?}").to_lowercase(),
                _ => format!("{path} {op:?} {value}").to_lowercase(),
            },
            Check::BodyContains { value } => format!("body contains {value:?}"),
            Check::BodyMatches { regex } => format!("body matches /{regex}/"),
            Check::ResponseTimeBelow { max_ms } => format!("time < {max_ms} ms"),
            Check::JsonSchema { .. } => "body matches JSON schema".to_string(),
        }
    }
}

impl NumberOp {
    pub fn symbol(&self) -> &'static str {
        match self {
            NumberOp::Eq => "==",
            NumberOp::Ne => "!=",
            NumberOp::Lt => "<",
            NumberOp::Lte => "<=",
            NumberOp::Gt => ">",
            NumberOp::Gte => ">=",
        }
    }
}
