//! Explicit, lossless migration from the legacy request model.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::model::{
    ApiKeyPlacement, AuthConfig, BodyDef, Check, ExtractScope, ExtractorSource, NumberOp,
    ParamKind, RequestDef, StringOp, ValueOp,
};

use super::model::{
    BodySpec, BodyType, FormatVersion, HeaderSpec, InlineBody, PipelineEntry, PipelinePhase,
    RequestDocument, RequestKind, RequestMeta, RequestSpec,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationError {
    pub unsupported: Vec<String>,
}

impl std::fmt::Display for MigrationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "request cannot be migrated without data loss: {}",
            self.unsupported.join("; ")
        )
    }
}

impl std::error::Error for MigrationError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrationStatus {
    Ready,
    Migrated,
    Blocked,
    Exists,
}

#[derive(Debug, Clone)]
pub struct MigrationItem {
    pub source: PathBuf,
    pub target: PathBuf,
    pub status: MigrationStatus,
    pub message: String,
}

/// Plan or execute a whole-tree migration while preserving relative paths.
/// Existing targets and unsupported legacy features are reported per file
/// and never overwritten.
pub fn migrate_tree(
    source_root: &Path,
    target_root: &Path,
    dry_run: bool,
) -> Result<Vec<MigrationItem>, std::io::Error> {
    let mut sources = walkdir::WalkDir::new(source_root)
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .map_err(std::io::Error::other)?
        .into_iter()
        .filter(|entry| {
            entry.file_type().is_file() && entry.path().to_string_lossy().ends_with(".request.json")
        })
        .map(|entry| entry.into_path())
        .collect::<Vec<_>>();
    sources.sort();

    let mut report = Vec::with_capacity(sources.len());
    for source in sources {
        let relative = source.strip_prefix(source_root).unwrap_or(&source);
        let target = target_root.join(relative);
        if target.exists() {
            report.push(MigrationItem {
                source,
                target,
                status: MigrationStatus::Exists,
                message: "target already exists".to_string(),
            });
            continue;
        }
        let result = std::fs::read_to_string(&source)
            .map_err(|error| error.to_string())
            .and_then(|text| {
                serde_json::from_str::<RequestDef>(&text).map_err(|error| error.to_string())
            })
            .and_then(|legacy| {
                let stem = source
                    .file_stem()
                    .and_then(|name| name.to_str())
                    .unwrap_or("request");
                let id = stem.strip_suffix(".request").unwrap_or(stem);
                migrate_request(&legacy, id).map_err(|error| error.to_string())
            });
        match result {
            Err(message) => report.push(MigrationItem {
                source,
                target,
                status: MigrationStatus::Blocked,
                message,
            }),
            Ok(_) if dry_run => report.push(MigrationItem {
                source,
                target,
                status: MigrationStatus::Ready,
                message: "ready to migrate".to_string(),
            }),
            Ok(document) => {
                let write_result = target
                    .parent()
                    .map(std::fs::create_dir_all)
                    .transpose()
                    .and_then(|_| {
                        serde_json::to_vec_pretty(&document)
                            .map_err(std::io::Error::other)
                            .and_then(|mut json| {
                                json.push(b'\n');
                                std::fs::write(&target, json)
                            })
                    });
                match write_result {
                    Ok(()) => report.push(MigrationItem {
                        source,
                        target,
                        status: MigrationStatus::Migrated,
                        message: "migrated".to_string(),
                    }),
                    Err(error) => report.push(MigrationItem {
                        source,
                        target,
                        status: MigrationStatus::Blocked,
                        message: error.to_string(),
                    }),
                }
            }
        }
    }
    Ok(report)
}

/// Convert one legacy request. Anything v1 cannot represent is reported
/// explicitly; no field is silently dropped.
pub fn migrate_request(
    def: &RequestDef,
    id: impl Into<String>,
) -> Result<RequestDocument, MigrationError> {
    let mut unsupported = Vec::new();
    let mut headers = def
        .headers
        .iter()
        .map(|header| HeaderSpec {
            name: convert_vars(&header.key),
            value: convert_vars(&header.value),
            enabled: header.enabled,
        })
        .collect::<Vec<_>>();
    let mut query = Vec::new();
    let mut url = convert_vars(&def.url);
    for parameter in &def.params {
        match parameter.kind {
            ParamKind::Query => query.push(HeaderSpec {
                name: convert_vars(&parameter.kv.key),
                value: convert_vars(&parameter.kv.value),
                enabled: parameter.kv.enabled,
            }),
            ParamKind::Path if parameter.kv.enabled => {
                url = url.replace(
                    &format!(":{}", parameter.kv.key),
                    &convert_vars(&parameter.kv.value),
                );
            }
            ParamKind::Path => {}
        }
    }

    let mut pipeline = Vec::new();
    migrate_auth(
        &def.auth,
        &mut headers,
        &mut query,
        &mut pipeline,
        &mut unsupported,
    );
    let body = migrate_body(&def.body, &mut headers, &mut unsupported);
    migrate_assertions(def, &mut pipeline, &mut unsupported);
    migrate_extractors(def, &mut pipeline, &mut unsupported);

    if !def.scripts.is_empty() {
        unsupported.push("inline request scripts require project .js assets".to_string());
    }
    if !def.settings.is_default() {
        unsupported.push("per-request transport settings have no v1 equivalent".to_string());
    }
    if !unsupported.is_empty() {
        unsupported.sort();
        unsupported.dedup();
        return Err(MigrationError { unsupported });
    }

    Ok(RequestDocument {
        schema: None,
        format_version: FormatVersion,
        kind: RequestKind::Request,
        meta: RequestMeta {
            id: id.into(),
            name: def.name.clone(),
            description: (!def.description.is_empty()).then(|| def.description.clone()),
            tags: Vec::new(),
        },
        bindings: BTreeMap::new(),
        matrix: BTreeMap::new(),
        request: RequestSpec {
            method: def.method,
            url,
            headers,
            query,
            body,
        },
        pipeline,
        mock: None,
    })
}

fn migrate_auth(
    auth: &AuthConfig,
    headers: &mut Vec<HeaderSpec>,
    query: &mut Vec<HeaderSpec>,
    pipeline: &mut Vec<PipelineEntry>,
    unsupported: &mut Vec<String>,
) {
    let before = |uses: &str, with: Map<String, Value>| PipelineEntry {
        phase: PipelinePhase::BeforeRequest,
        uses: uses.to_string(),
        with,
        enabled: true,
    };
    match auth {
        AuthConfig::None => {}
        AuthConfig::Inherit => unsupported.push(
            "inherited auth must be resolved from its collection before migration".to_string(),
        ),
        AuthConfig::Basic { username, password } => pipeline.push(before(
            "builtin:basic@1",
            Map::from_iter([
                ("username".into(), Value::String(convert_vars(username))),
                ("password".into(), Value::String(convert_vars(password))),
            ]),
        )),
        AuthConfig::Bearer { token, prefix } => {
            let mut with = Map::from_iter([("token".into(), Value::String(convert_vars(token)))]);
            if let Some(prefix) = prefix {
                with.insert("prefix".into(), Value::String(prefix.clone()));
            }
            pipeline.push(before("builtin:bearer@1", with));
        }
        AuthConfig::ApiKey {
            key,
            value,
            placement,
        } => {
            let entry = HeaderSpec {
                name: convert_vars(key),
                value: convert_vars(value),
                enabled: true,
            };
            match placement {
                ApiKeyPlacement::Header => headers.push(entry),
                ApiKeyPlacement::Query => query.push(entry),
            }
        }
        other => unsupported.push(format!(
            "{} auth is not yet represented by request format v1",
            auth_name(other)
        )),
    }
}

fn auth_name(auth: &AuthConfig) -> &'static str {
    match auth {
        AuthConfig::OAuth2ClientCredentials { .. } => "OAuth2 client credentials",
        AuthConfig::OAuth2AuthCode { .. } => "OAuth2 authorization code",
        AuthConfig::Digest { .. } => "Digest",
        AuthConfig::Ntlm { .. } => "NTLM",
        AuthConfig::AwsSigV4 { .. } => "AWS SigV4",
        _ => "request",
    }
}

fn migrate_body(
    body: &BodyDef,
    headers: &mut Vec<HeaderSpec>,
    unsupported: &mut Vec<String>,
) -> Option<BodySpec> {
    let inline = |body_type, value| {
        Some(BodySpec::Inline(InlineBody {
            body_type,
            value: Some(value),
        }))
    };
    match body {
        BodyDef::None => None,
        BodyDef::Raw { text, .. } | BodyDef::Xml { text } => {
            inline(BodyType::Text, Value::String(convert_vars(text)))
        }
        BodyDef::Json { text } => match serde_json::from_str(&convert_vars(text)) {
            Ok(value) => inline(BodyType::Json, value),
            Err(error) => {
                unsupported.push(format!(
                    "JSON body is invalid after variable migration: {error}"
                ));
                None
            }
        },
        BodyDef::FormUrlencoded { fields } => {
            let mut object = Map::new();
            for field in fields.iter().filter(|field| field.enabled) {
                if object
                    .insert(
                        convert_vars(&field.key),
                        Value::String(convert_vars(&field.value)),
                    )
                    .is_some()
                {
                    unsupported.push(
                        "form body contains duplicate keys, which v1 cannot preserve".to_string(),
                    );
                }
            }
            inline(BodyType::Form, Value::Object(object))
        }
        BodyDef::GraphQl {
            query,
            variables,
            operation_name,
        } => {
            let variables = if variables.trim().is_empty() {
                Value::Object(Map::new())
            } else {
                match serde_json::from_str(&convert_vars(variables)) {
                    Ok(value) => value,
                    Err(error) => {
                        unsupported.push(format!("GraphQL variables are invalid JSON: {error}"));
                        Value::Null
                    }
                }
            };
            add_content_type(headers, "application/json");
            inline(
                BodyType::Json,
                serde_json::json!({
                    "query": convert_vars(query),
                    "variables": variables,
                    "operationName": operation_name,
                }),
            )
        }
        BodyDef::Multipart { .. } => {
            unsupported.push("multipart bodies are not supported by v1".to_string());
            None
        }
        BodyDef::Binary { .. } => {
            unsupported.push("binary bodies are not supported by v1".to_string());
            None
        }
    }
}

fn add_content_type(headers: &mut Vec<HeaderSpec>, value: &str) {
    if !headers
        .iter()
        .any(|header| header.name.eq_ignore_ascii_case("content-type"))
    {
        headers.push(HeaderSpec {
            name: "Content-Type".to_string(),
            value: value.to_string(),
            enabled: true,
        });
    }
}

fn migrate_assertions(
    def: &RequestDef,
    pipeline: &mut Vec<PipelineEntry>,
    unsupported: &mut Vec<String>,
) {
    for assertion in &def.assertions {
        if !assertion.note.is_empty() {
            unsupported.push("assertion notes have no v1 field".to_string());
            continue;
        }
        let mapped = match &assertion.check {
            Check::StatusCode {
                op: NumberOp::Eq,
                value,
            } => builtin("assert-status", serde_json::json!({"expected": value})),
            Check::Header {
                name,
                op: StringOp::Equals,
                value,
            } => builtin(
                "assert-header",
                serde_json::json!({"name": convert_vars(name), "value": convert_vars(value)}),
            ),
            Check::Header {
                name,
                op: StringOp::Exists,
                ..
            } => builtin(
                "assert-header",
                serde_json::json!({"name": convert_vars(name)}),
            ),
            Check::ContentType { value } => builtin(
                "assert-header",
                serde_json::json!({"name": "Content-Type", "value": convert_vars(value)}),
            ),
            Check::JsonPath { path, op, value } => {
                let operator = match op {
                    ValueOp::Equals => Some("equals"),
                    ValueOp::Contains => Some("contains"),
                    ValueOp::Exists => Some("exists"),
                    ValueOp::NotExists => Some("notExists"),
                    _ => None,
                };
                if let Some(operator) = operator {
                    let mut with = serde_json::json!({
                        "path": convert_vars(path),
                        "operator": operator,
                    });
                    if !matches!(op, ValueOp::Exists | ValueOp::NotExists) {
                        with["value"] = convert_value_vars(value);
                    }
                    builtin("assert-json-path", with)
                } else {
                    unsupported.push(format!("JSONPath operator {op:?} is not supported by v1"));
                    continue;
                }
            }
            Check::BodyContains { value } => builtin(
                "assert-body-text",
                serde_json::json!({"text": convert_vars(value)}),
            ),
            Check::BodyMatches { regex } => builtin(
                "assert-body-regex",
                serde_json::json!({"pattern": convert_vars(regex)}),
            ),
            Check::ResponseTimeBelow { max_ms } => {
                builtin("assert-response-time", serde_json::json!({"maxMs": max_ms}))
            }
            Check::JsonSchema { schema } => {
                builtin("assert-schema", serde_json::json!({"schema": schema}))
            }
            other => {
                unsupported.push(format!("assertion {:?} is not supported by v1", other));
                continue;
            }
        };
        pipeline.push(PipelineEntry {
            enabled: assertion.enabled,
            ..mapped
        });
    }
}

fn migrate_extractors(
    def: &RequestDef,
    pipeline: &mut Vec<PipelineEntry>,
    unsupported: &mut Vec<String>,
) {
    for extractor in &def.extractors {
        if extractor.scope != ExtractScope::Runtime {
            unsupported.push("environment-scoped extractors are not supported by v1".to_string());
            continue;
        }
        let mapped = match &extractor.source {
            ExtractorSource::JsonPath { expr } => builtin(
                "extract-json-path",
                serde_json::json!({"path": convert_vars(expr), "target": extractor.var}),
            ),
            ExtractorSource::Header { name } => builtin(
                "extract-header",
                serde_json::json!({"name": convert_vars(name), "target": extractor.var}),
            ),
            ExtractorSource::Regex { .. } => {
                unsupported.push("regex extractors are not supported by v1".to_string());
                continue;
            }
        };
        pipeline.push(PipelineEntry {
            enabled: extractor.enabled,
            ..mapped
        });
    }
}

fn builtin(name: &str, with: Value) -> PipelineEntry {
    PipelineEntry {
        phase: PipelinePhase::AfterResponse,
        uses: format!("builtin:{name}@1"),
        with: with.as_object().cloned().unwrap_or_default(),
        enabled: true,
    }
}

fn convert_value_vars(value: &Value) -> Value {
    match value {
        Value::String(value) => Value::String(convert_vars(value)),
        Value::Array(values) => Value::Array(values.iter().map(convert_value_vars).collect()),
        Value::Object(values) => Value::Object(
            values
                .iter()
                .map(|(key, value)| (key.clone(), convert_value_vars(value)))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn convert_vars(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            out.push_str(&rest[start..]);
            return out;
        };
        let name = after[..end].trim();
        if name.is_empty() {
            out.push_str("{{}}");
        } else {
            out.push_str("${env.");
            out.push_str(name);
            out.push('}');
        }
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AssertionDef, KeyValue, Method};

    #[test]
    fn migrates_common_request_without_losing_variable_mapping() {
        let mut def = RequestDef::new("Create", Method::Post, "{{baseUrl}}/users/:id");
        def.params.push(crate::model::Param {
            kv: KeyValue::new("id", "{{userId}}"),
            kind: ParamKind::Path,
        });
        def.auth = AuthConfig::Bearer {
            token: "{{token}}".to_string(),
            prefix: None,
        };
        def.body = BodyDef::Json {
            text: r#"{"name":"{{name}}"}"#.to_string(),
        };
        def.assertions.push(AssertionDef::from(Check::StatusCode {
            op: NumberOp::Eq,
            value: 201,
        }));

        let migrated = migrate_request(&def, "users.create").unwrap();

        assert_eq!(migrated.request.url, "${env.baseUrl}/users/${env.userId}");
        assert_eq!(
            migrated.pipeline[0].with["token"],
            Value::String("${env.token}".to_string())
        );
        assert_eq!(migrated.pipeline[1].uses, "builtin:assert-status@1");
    }

    #[test]
    fn refuses_features_that_would_be_dropped() {
        let mut def = RequestDef::new("Binary", Method::Post, "https://example.test");
        def.body = BodyDef::Binary {
            path: "payload.bin".to_string(),
        };

        let error = migrate_request(&def, "binary").unwrap_err();

        assert!(error
            .unsupported
            .iter()
            .any(|item| item.contains("binary bodies")));
    }

    #[test]
    fn bulk_migration_has_a_non_writing_dry_run_and_preserves_paths() {
        let source = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(source.path().join("users")).unwrap();
        let mut request = RequestDef::new("List", Method::Get, "https://example.test/users");
        request.auth = AuthConfig::None;
        std::fs::write(
            source.path().join("users/list.request.json"),
            serde_json::to_vec(&request).unwrap(),
        )
        .unwrap();
        let output = target.path().join("migrated");

        let dry_run = migrate_tree(source.path(), &output, true).unwrap();
        assert_eq!(
            dry_run[0].status,
            MigrationStatus::Ready,
            "{}",
            dry_run[0].message
        );
        assert!(!output.exists());

        let migrated = migrate_tree(source.path(), &output, false).unwrap();
        assert_eq!(migrated[0].status, MigrationStatus::Migrated);
        assert!(output.join("users/list.request.json").is_file());

        let second = migrate_tree(source.path(), &output, false).unwrap();
        assert_eq!(second[0].status, MigrationStatus::Exists);
    }
}
