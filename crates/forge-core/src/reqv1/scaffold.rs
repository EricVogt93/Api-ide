//! Creation of executable project assets with colocated catalog metadata.

use std::path::{Path, PathBuf};

use serde_json::json;

use super::catalog::{
    BuiltinIntent, BuiltinParameterKind, ProjectAssetMetadata, ProjectAssetParameter,
};
use super::index::AssetKind;
use super::model::PipelinePhase;

#[derive(Debug, Clone)]
pub struct ScaffoldedAsset {
    pub code: PathBuf,
    pub metadata: PathBuf,
}

pub fn available_path(directory: &Path, stem: &str, suffix: &str) -> PathBuf {
    let first = directory.join(format!("{stem}{suffix}"));
    if !first.exists() {
        return first;
    }
    (2..)
        .map(|number| directory.join(format!("{stem}-{number}{suffix}")))
        .find(|path| !path.exists())
        .expect("filesystem path suffix space is unbounded")
}

pub fn scaffold_asset(root: &Path, kind: AssetKind, name: &str) -> Result<ScaffoldedAsset, String> {
    if name.is_empty()
        || !name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return Err("asset name may contain only letters, numbers, '-' and '_'".to_string());
    }
    let (code, metadata) = template(kind, name)?;
    let directory = root.join("assets").join(kind.label());
    let code_path = directory.join(format!("{name}.js"));
    let metadata_path = directory.join(format!("{name}.meta.json"));
    if code_path.exists() || metadata_path.exists() {
        return Err(format!(
            "asset {name:?} already exists in {}",
            directory.display()
        ));
    }
    std::fs::create_dir_all(&directory)
        .map_err(|error| format!("cannot create {}: {error}", directory.display()))?;
    let mut metadata_json =
        serde_json::to_string_pretty(&metadata).map_err(|error| error.to_string())?;
    metadata_json.push('\n');
    std::fs::write(&code_path, code)
        .map_err(|error| format!("cannot write {}: {error}", code_path.display()))?;
    if let Err(error) = std::fs::write(&metadata_path, metadata_json) {
        let _ = std::fs::remove_file(&code_path);
        return Err(format!("cannot write {}: {error}", metadata_path.display()));
    }
    Ok(ScaffoldedAsset {
        code: code_path,
        metadata: metadata_path,
    })
}

fn parameter(
    name: &str,
    label: &str,
    kind: BuiltinParameterKind,
    default: serde_json::Value,
) -> ProjectAssetParameter {
    ProjectAssetParameter {
        name: name.to_string(),
        label: label.to_string(),
        kind,
        required: true,
        default: Some(default.clone()),
        options: Vec::new(),
        example: default.to_string(),
    }
}

fn template(kind: AssetKind, name: &str) -> Result<(&'static str, ProjectAssetMetadata), String> {
    use BuiltinIntent::{Capture, Generate, Prepare, Simulate, Validate};
    use BuiltinParameterKind::{Integer, Json, String as StringParameter};
    use PipelinePhase::{AfterResponse, BeforeRequest};

    let title = name.replace(['-', '_'], " ");
    let (code, description, intent, phase, parameters, example) = match kind {
        AssetKind::Assertion => (
            r#"function run(ctx, input) {
  return {
    passed: ctx.response.status === input.expectedStatus,
    message: "response status matches",
    expected: input.expectedStatus,
    actual: ctx.response.status
  };
}
"#,
            "Checks the response status.",
            Validate,
            Some(AfterResponse),
            vec![parameter(
                "expectedStatus",
                "Expected status",
                Integer,
                json!(200),
            )],
            json!({"expectedStatus": 200}),
        ),
        AssetKind::Hook => (
            r#"function run(ctx, input) {
  return { headers: [{ name: input.name, value: input.value }] };
}
"#,
            "Adds a configured request header.",
            Prepare,
            Some(BeforeRequest),
            vec![
                parameter("name", "Header name", StringParameter, json!("X-Trace-Id")),
                parameter("value", "Header value", StringParameter, json!("trace-1")),
            ],
            json!({"name": "X-Trace-Id", "value": "trace-1"}),
        ),
        AssetKind::Extractor => (
            r#"function run(ctx, input) {
  var runtime = {};
  runtime[input.target] = ctx.response.body;
  return { runtime: runtime };
}
"#,
            "Captures the response body in a runtime variable.",
            Capture,
            Some(AfterResponse),
            vec![parameter(
                "target",
                "Runtime variable",
                StringParameter,
                json!("responseBody"),
            )],
            json!({"target": "responseBody"}),
        ),
        AssetKind::Generator => (
            r#"function run(ctx, input) {
  return input.value;
}
"#,
            "Provides a configured reusable value.",
            Generate,
            None,
            vec![parameter("value", "Value", Json, json!({}))],
            json!({"value": {}}),
        ),
        AssetKind::Mock => (
            r#"function run(ctx, input) {
  return { status: input.status, headers: [], body: input.body };
}
"#,
            "Returns a configured mock response.",
            Simulate,
            None,
            vec![
                parameter("status", "Status", Integer, json!(200)),
                parameter("body", "Body", Json, json!({})),
            ],
            json!({"status": 200, "body": {}}),
        ),
        AssetKind::Data | AssetKind::Executable => {
            return Err("choose an executable catalog asset kind".to_string())
        }
    };
    Ok((
        code,
        ProjectAssetMetadata {
            title,
            description: description.to_string(),
            intent,
            phase,
            parameters,
            example,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_code_and_metadata_without_overwriting() {
        let root = tempfile::tempdir().unwrap();
        let created = scaffold_asset(root.path(), AssetKind::Assertion, "user-status").unwrap();

        assert!(created.code.is_file());
        let metadata: ProjectAssetMetadata =
            serde_json::from_str(&std::fs::read_to_string(created.metadata).unwrap()).unwrap();
        assert_eq!(metadata.intent, BuiltinIntent::Validate);
        assert!(scaffold_asset(root.path(), AssetKind::Assertion, "user-status").is_err());
    }

    #[test]
    fn scaffolded_metadata_matches_the_shipped_schema() {
        let root = tempfile::tempdir().unwrap();
        let created = scaffold_asset(root.path(), AssetKind::Hook, "trace").unwrap();
        let metadata: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(created.metadata).unwrap()).unwrap();
        let schema: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../schemas/asset-metadata-v1.schema.json"),
            )
            .unwrap(),
        )
        .unwrap();

        assert!(jsonschema::validator_for(&schema)
            .unwrap()
            .is_valid(&metadata));
    }

    #[test]
    fn available_paths_never_overwrite() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("new.request.json"), "{}").unwrap();

        assert_eq!(
            available_path(root.path(), "new", ".request.json"),
            root.path().join("new-2.request.json")
        );
    }

    #[test]
    fn project_assets_use_the_generated_conventional_path() {
        let root = tempfile::tempdir().unwrap();
        crate::store::Workspace::create(root.path(), "Assets").unwrap();

        let created = scaffold_asset(root.path(), AssetKind::Assertion, "status").unwrap();
        assert_eq!(
            created.code,
            root.path().join("assets/assertions/status.js")
        );

        let index = super::super::ProjectIndex::scan(root.path()).unwrap();
        let asset = index
            .assets
            .iter()
            .find(|asset| asset.path == created.code.to_string_lossy())
            .unwrap();
        assert_eq!(
            index.suggest_ref(asset, &root.path().join("requests")),
            "project:assertions/status"
        );
    }
}
