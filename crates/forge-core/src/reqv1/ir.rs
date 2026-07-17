//! Canonical intermediate representation: the fully-resolved, validated
//! runtime form of a request. The persistence model is never executed
//! directly. See `docs/architecture/request-format-v1.md` §4.

use serde_json::Value;

use crate::model::Method;

use super::model::PipelinePhase;
use super::refs::AssetDescriptor;

/// A request after schema validation, reference resolution, binding
/// resolution and variable interpolation — everything the pipeline and the
/// HTTP engine need, nothing left to look up.
#[derive(Debug, Clone)]
pub struct ResolvedRequest {
    pub id: String,
    pub name: String,
    pub method: Method,
    pub url: String,
    pub headers: Vec<ResolvedHeader>,
    pub query: Vec<ResolvedHeader>,
    pub body: ResolvedBody,
    pub pipeline: Vec<ResolvedPipelineEntry>,
    pub mock: Option<ResolvedMock>,
    /// Resolved binding values, kept for diagnostics and `${bindings.*}` use.
    pub bindings: Value,
    /// Concrete secret values that were interpolated — used to mask output.
    pub secret_values: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedHeader {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ResolvedBody {
    None,
    /// JSON body (already interpolated).
    Json(Value),
    Text(String),
    /// `application/x-www-form-urlencoded` fields.
    Form(Vec<ResolvedHeader>),
}

#[derive(Debug, Clone)]
pub struct ResolvedPipelineEntry {
    pub phase: PipelinePhase,
    pub asset: AssetDescriptor,
    /// Resolved `with` input.
    pub input: Value,
}

#[derive(Debug, Clone)]
pub enum ResolvedMock {
    Static { status: u16, headers: Vec<ResolvedHeader>, body: ResolvedBody, delay_ms: u64 },
    Dynamic { asset: AssetDescriptor, input: Value },
}

impl ResolvedRequest {
    /// Mask every recorded secret value in `s` with `***`.
    pub fn mask(&self, s: &str) -> String {
        let mut out = s.to_string();
        for secret in &self.secret_values {
            if !secret.is_empty() {
                out = out.replace(secret.as_str(), "***");
            }
        }
        out
    }
}
