//! Lossless request/folder export and import.
//!
//! JSON bundles keep UTF-8 files readable and encode only binary files. A
//! cURL export is an executable shell script with the exact bundle embedded
//! in comments, so importing it restores assertions, hooks and properties.

use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use walkdir::{DirEntry, WalkDir};

use super::model::{BodySpec, BodyType, RequestDocument};
use super::{assertions_path, hooks_path};

const BUNDLE_FORMAT: &str = "forge.bundle";
const BUNDLE_VERSION: u32 = 1;
const CURL_BUNDLE_BEGIN: &str = "# forge-bundle-v1: begin";
const CURL_BUNDLE_END: &str = "# forge-bundle-v1: end";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BundleFormat {
    Json,
    Curl,
}

impl BundleFormat {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Json => "forge.json",
            Self::Curl => "forge.sh",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportSummary {
    pub files: usize,
    pub requests: usize,
    pub output: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportSummary {
    pub files: Vec<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ForgeBundle {
    format: String,
    format_version: u32,
    kind: BundleKind,
    source: String,
    files: Vec<BundleFile>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum BundleKind {
    Request,
    Folder,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BundleFile {
    path: String,
    encoding: FileEncoding,
    content: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum FileEncoding {
    Utf8,
    Base64,
}

/// Export one `*.request.json` (including its sidecars) or a complete folder.
/// Existing output files are never overwritten.
pub fn export_bundle(
    project_root: &Path,
    source: &Path,
    format: BundleFormat,
    output: &Path,
) -> Result<ExportSummary, String> {
    if output.exists() {
        return Err(format!("{} already exists", output.display()));
    }
    let bundle = build_bundle(project_root, source)?;
    let requests = bundle
        .files
        .iter()
        .filter(|file| file.path.ends_with(".request.json"))
        .count();
    let rendered = match format {
        BundleFormat::Json => render_json(&bundle)?,
        BundleFormat::Curl => render_curl(&bundle)?,
    };
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)
        .map_err(|error| format!("cannot create {}: {error}", output.display()))?;
    file.write_all(rendered.as_bytes())
        .map_err(|error| format!("cannot write {}: {error}", output.display()))?;
    Ok(ExportSummary {
        files: bundle.files.len(),
        requests,
        output: output.to_path_buf(),
    })
}

/// Import a ApiWright JSON bundle or a cURL script exported by ApiWright. Bundle
/// paths are restored below `destination`; collisions abort before writing.
pub fn import_bundle(input: &Path, destination: &Path) -> Result<ImportSummary, String> {
    let text = std::fs::read_to_string(input)
        .map_err(|error| format!("cannot read {}: {error}", input.display()))?;
    let bundle = parse_bundle(&text)?;
    validate_bundle(&bundle)?;

    let decoded = bundle
        .files
        .iter()
        .map(|file| {
            let relative = safe_relative_path(&file.path)?;
            let bytes = match file.encoding {
                FileEncoding::Utf8 => file.content.as_bytes().to_vec(),
                FileEncoding::Base64 => BASE64
                    .decode(&file.content)
                    .map_err(|error| format!("invalid base64 in {}: {error}", file.path))?,
            };
            Ok((relative, bytes))
        })
        .collect::<Result<Vec<_>, String>>()?;

    let mut paths = BTreeSet::new();
    for (relative, _) in &decoded {
        if !paths.insert(relative.clone()) {
            return Err(format!(
                "bundle contains duplicate path {}",
                relative.display()
            ));
        }
        let target = destination.join(relative);
        if target.exists() {
            return Err(format!("{} already exists", target.display()));
        }
    }

    std::fs::create_dir_all(destination)
        .map_err(|error| format!("cannot create {}: {error}", destination.display()))?;
    let mut written = Vec::new();
    for (relative, bytes) in decoded {
        let target = destination.join(relative);
        let result = (|| {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
            }
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&target)
                .map_err(|error| format!("cannot create {}: {error}", target.display()))?;
            file.write_all(&bytes)
                .map_err(|error| format!("cannot write {}: {error}", target.display()))
        })();
        if let Err(error) = result {
            for path in written.iter().rev() {
                let _ = std::fs::remove_file(path);
            }
            return Err(error);
        }
        written.push(target);
    }
    Ok(ImportSummary { files: written })
}

fn build_bundle(project_root: &Path, source: &Path) -> Result<ForgeBundle, String> {
    let root = project_root
        .canonicalize()
        .map_err(|error| format!("cannot resolve {}: {error}", project_root.display()))?;
    let source = source
        .canonicalize()
        .map_err(|error| format!("cannot resolve {}: {error}", source.display()))?;
    if !source.starts_with(&root) {
        return Err(format!("{} is outside the project", source.display()));
    }

    let (kind, base, paths) = if source.is_dir() {
        let base = if source == root {
            source.clone()
        } else {
            source
                .parent()
                .ok_or_else(|| format!("{} has no parent", source.display()))?
                .to_path_buf()
        };
        (BundleKind::Folder, base, collect_folder_files(&source)?)
    } else if is_request_file(&source) {
        let base = source
            .parent()
            .ok_or_else(|| format!("{} has no parent", source.display()))?
            .to_path_buf();
        (BundleKind::Request, base, collect_request_files(&source))
    } else {
        return Err("export source must be a folder or *.request.json".to_string());
    };
    if paths.is_empty() {
        return Err("export scope contains no files".to_string());
    }

    let mut files = paths
        .into_iter()
        .map(|path| {
            let relative = path
                .strip_prefix(&base)
                .map_err(|_| format!("{} is outside export scope", path.display()))?;
            let bytes = std::fs::read(&path)
                .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
            let (encoding, content) = match String::from_utf8(bytes) {
                Ok(text) => (FileEncoding::Utf8, text),
                Err(error) => (FileEncoding::Base64, BASE64.encode(error.into_bytes())),
            };
            Ok(BundleFile {
                path: portable_path(relative)?,
                encoding,
                content,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    files.sort_by(|left, right| left.path.cmp(&right.path));

    Ok(ForgeBundle {
        format: BUNDLE_FORMAT.to_string(),
        format_version: BUNDLE_VERSION,
        kind,
        source: portable_path(source.strip_prefix(&root).unwrap_or(Path::new(".")))?,
        files,
    })
}

fn collect_folder_files(source: &Path) -> Result<Vec<PathBuf>, String> {
    let mut files = Vec::new();
    for entry in WalkDir::new(source)
        .follow_links(false)
        .into_iter()
        .filter_entry(include_entry)
    {
        let entry = entry.map_err(|error| format!("cannot scan {}: {error}", source.display()))?;
        if entry.file_type().is_file() && include_file(entry.path()) {
            files.push(entry.into_path());
        }
    }
    files.sort();
    Ok(files)
}

fn collect_request_files(request: &Path) -> Vec<PathBuf> {
    let mut candidates = vec![
        request.to_path_buf(),
        assertions_path(request),
        hooks_path(request),
    ];
    let name = request
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_default();
    for suffix in [".forge-jira", ".forge-environment", ".forge-openapi"] {
        candidates.push(request.with_file_name(format!(".{name}{suffix}")));
    }
    candidates.retain(|path| path.is_file() && include_file(path));
    candidates.sort();
    candidates.dedup();
    candidates
}

fn include_entry(entry: &DirEntry) -> bool {
    if entry.depth() == 0 || !entry.file_type().is_dir() {
        return true;
    }
    !matches!(
        entry.file_name().to_str(),
        Some(".git" | ".forge" | ".forge-local" | "node_modules" | "target")
    )
}

fn include_file(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    name != ".env.local" && !name.ends_with(".secrets.json")
}

fn is_request_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(".request.json"))
}

fn portable_path(path: &Path) -> Result<String, String> {
    let components = path
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(Ok(value.to_string_lossy().into_owned())),
            Component::CurDir => None,
            _ => Some(Err(format!("unsafe bundle path {}", path.display()))),
        })
        .collect::<Result<Vec<_>, String>>()?;
    if components.is_empty() {
        Ok(".".to_string())
    } else {
        Ok(components.join("/"))
    }
}

fn safe_relative_path(path: &str) -> Result<PathBuf, String> {
    let path = Path::new(path);
    let mut safe = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => safe.push(value),
            _ => return Err(format!("unsafe bundle path {}", path.display())),
        }
    }
    if safe.as_os_str().is_empty() {
        return Err("bundle file path must not be empty".to_string());
    }
    Ok(safe)
}

fn render_json(bundle: &ForgeBundle) -> Result<String, String> {
    let mut json = serde_json::to_string_pretty(bundle)
        .map_err(|error| format!("cannot serialize bundle: {error}"))?;
    json.push('\n');
    Ok(json)
}

fn render_curl(bundle: &ForgeBundle) -> Result<String, String> {
    let compact = serde_json::to_vec(bundle)
        .map_err(|error| format!("cannot serialize embedded bundle: {error}"))?;
    let encoded = BASE64.encode(compact);
    let mut output = String::from(
        "#!/usr/bin/env sh\n# Generated by ApiWright. Import this script to restore its full request metadata.\n",
    );
    output.push_str(CURL_BUNDLE_BEGIN);
    output.push('\n');
    for chunk in encoded.as_bytes().chunks(76) {
        output.push_str("# ");
        output.push_str(std::str::from_utf8(chunk).expect("base64 is ASCII"));
        output.push('\n');
    }
    output.push_str(CURL_BUNDLE_END);
    output.push_str("\nset -eu\n");

    let mut request_count = 0;
    for file in &bundle.files {
        if !file.path.ends_with(".request.json") {
            continue;
        }
        let text = match file.encoding {
            FileEncoding::Utf8 => &file.content,
            FileEncoding::Base64 => {
                return Err(format!("request {} is not UTF-8", file.path));
            }
        };
        let request = RequestDocument::parse(text)
            .map_err(|error| format!("invalid request {}: {error}", file.path))?;
        output.push_str("\n# ");
        output.push_str(&file.path);
        output.push('\n');
        output.push_str(&request_curl(&request));
        output.push('\n');
        request_count += 1;
    }
    if request_count == 0 {
        return Err("cURL export contains no request documents".to_string());
    }
    Ok(output)
}

fn request_curl(request: &RequestDocument) -> String {
    let mut url = request.request.url.clone();
    let query = request
        .request
        .query
        .iter()
        .filter(|item| item.enabled)
        .collect::<Vec<_>>();
    if !query.is_empty() {
        let mut first = !url.contains('?');
        for item in query {
            url.push(if first { '?' } else { '&' });
            first = false;
            url.push_str(&item.name);
            url.push('=');
            url.push_str(&item.value);
        }
    }

    let mut headers = request
        .request
        .headers
        .iter()
        .filter(|header| header.enabled)
        .map(|header| (header.name.clone(), header.value.clone()))
        .collect::<Vec<_>>();
    let mut body = Vec::new();
    match request.request.body.as_ref() {
        None
        | Some(BodySpec::Inline(super::model::InlineBody {
            body_type: BodyType::None,
            ..
        })) => {}
        Some(BodySpec::Inline(inline)) => match inline.body_type {
            BodyType::Json => {
                if !headers
                    .iter()
                    .any(|(name, _)| name.eq_ignore_ascii_case("content-type"))
                {
                    headers.push(("Content-Type".to_string(), "application/json".to_string()));
                }
                if let Some(value) = &inline.value {
                    body.push(format!("--data-raw {}", shell_quote(&value.to_string())));
                }
            }
            BodyType::Text => {
                if let Some(value) = &inline.value {
                    let value = value
                        .as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| value.to_string());
                    body.push(format!("--data-raw {}", shell_quote(&value)));
                }
            }
            BodyType::Form => add_form_body(&mut body, inline.value.as_ref(), "--data-urlencode"),
            BodyType::Multipart => add_form_body(&mut body, inline.value.as_ref(), "--form"),
            BodyType::Binary => {
                if let Some(value) = &inline.value {
                    let value = value
                        .as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| value.to_string());
                    body.push(format!(
                        "--data-binary {}",
                        shell_quote(&format!("@{value}"))
                    ));
                }
            }
            BodyType::None => {}
        },
        Some(BodySpec::Ref(reference)) => body.push(format!(
            "--data-binary {}",
            shell_quote(&format!("@{}", reference.reference))
        )),
    }

    let mut chunks = vec![
        "curl".to_string(),
        format!("--request {}", request.request.method.as_str()),
        shell_quote(&url),
    ];
    chunks.extend(
        headers
            .into_iter()
            .map(|(name, value)| format!("--header {}", shell_quote(&format!("{name}: {value}")))),
    );
    chunks.extend(body);
    chunks.join(" \\\n  ")
}

fn add_form_body(chunks: &mut Vec<String>, value: Option<&Value>, flag: &str) {
    let Some(Value::Object(fields)) = value else {
        return;
    };
    for (name, value) in fields {
        let value = value
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| value.to_string());
        chunks.push(format!(
            "{flag} {}",
            shell_quote(&format!("{name}={value}"))
        ));
    }
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r#"'"'"'"#))
}

fn parse_bundle(text: &str) -> Result<ForgeBundle, String> {
    if text.trim_start().starts_with('{') {
        return serde_json::from_str(text)
            .map_err(|error| format!("invalid ApiWright bundle: {error}"));
    }
    let mut inside = false;
    let mut encoded = String::new();
    for line in text.lines() {
        if line.trim() == CURL_BUNDLE_BEGIN {
            inside = true;
            continue;
        }
        if line.trim() == CURL_BUNDLE_END {
            inside = false;
            break;
        }
        if inside {
            let chunk = line
                .trim()
                .strip_prefix('#')
                .map(str::trim)
                .ok_or_else(|| "invalid embedded ApiWright bundle".to_string())?;
            encoded.push_str(chunk);
        }
    }
    if inside || encoded.is_empty() {
        return Err(
            "file is neither a ApiWright JSON bundle nor a ApiWright cURL export".to_string(),
        );
    }
    let json = BASE64
        .decode(encoded)
        .map_err(|error| format!("invalid embedded ApiWright bundle: {error}"))?;
    serde_json::from_slice(&json)
        .map_err(|error| format!("invalid embedded ApiWright bundle: {error}"))
}

fn validate_bundle(bundle: &ForgeBundle) -> Result<(), String> {
    if bundle.format != BUNDLE_FORMAT {
        return Err(format!("unsupported bundle format {}", bundle.format));
    }
    if bundle.format_version != BUNDLE_VERSION {
        return Err(format!(
            "unsupported bundle version {} (this build supports {BUNDLE_VERSION})",
            bundle.format_version
        ));
    }
    if bundle.files.is_empty() {
        return Err("bundle contains no files".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const REQUEST: &str = r#"{
  "formatVersion": 1,
  "kind": "request",
  "meta": { "id": "users.get", "name": "Get user" },
  "request": {
    "method": "GET",
    "url": "https://api.example.com/users/${env.userId}",
    "headers": [{ "name": "Accept", "value": "application/json", "enabled": true }]
  }
}
"#;

    #[test]
    fn json_folder_roundtrip_preserves_nested_text_binary_and_sidecars() {
        let project = tempfile::tempdir().unwrap();
        let story = project.path().join("requests/SHOP-42");
        let nested = story.join("happy-path");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("get.request.json"), REQUEST).unwrap();
        std::fs::write(nested.join("get.assertions.json"), "{\"assertions\":[]}").unwrap();
        std::fs::write(story.join(".forge-jira"), "SHOP-42\n").unwrap();
        std::fs::write(story.join("payload.bin"), [0, 159, 146, 150]).unwrap();
        std::fs::write(story.join(".env.local"), "TOKEN=secret\n").unwrap();

        let output = project.path().join("story.forge.json");
        let summary = export_bundle(project.path(), &story, BundleFormat::Json, &output).unwrap();
        assert_eq!(summary.requests, 1);
        assert_eq!(summary.files, 4);

        let destination = tempfile::tempdir().unwrap();
        let imported = import_bundle(&output, destination.path()).unwrap();
        assert_eq!(imported.files.len(), 4);
        let restored = destination.path().join("SHOP-42");
        assert_eq!(
            std::fs::read_to_string(restored.join("happy-path/get.request.json")).unwrap(),
            REQUEST
        );
        assert_eq!(
            std::fs::read(restored.join("payload.bin")).unwrap(),
            [0, 159, 146, 150]
        );
        assert!(!restored.join(".env.local").exists());
    }

    #[test]
    fn curl_request_roundtrip_restores_assertions_hooks_and_properties() {
        let project = tempfile::tempdir().unwrap();
        let request = project.path().join("get.request.json");
        std::fs::write(&request, REQUEST).unwrap();
        std::fs::write(project.path().join("get.assertions.json"), "assertions").unwrap();
        std::fs::write(project.path().join("get.hooks.json"), "hooks").unwrap();
        std::fs::write(
            project.path().join(".get.request.json.forge-openapi"),
            "openapi.yaml\n",
        )
        .unwrap();

        let output = project.path().join("get.forge.sh");
        let summary = export_bundle(project.path(), &request, BundleFormat::Curl, &output).unwrap();
        assert_eq!(summary.files, 4);
        let script = std::fs::read_to_string(&output).unwrap();
        assert!(script.contains("curl \\\n  --request GET"));
        assert!(script.contains(CURL_BUNDLE_BEGIN));

        let destination = tempfile::tempdir().unwrap();
        import_bundle(&output, destination.path()).unwrap();
        assert_eq!(
            std::fs::read_to_string(destination.path().join("get.assertions.json")).unwrap(),
            "assertions"
        );
        assert_eq!(
            std::fs::read_to_string(destination.path().join("get.hooks.json")).unwrap(),
            "hooks"
        );
        assert_eq!(
            std::fs::read_to_string(destination.path().join(".get.request.json.forge-openapi"))
                .unwrap(),
            "openapi.yaml\n"
        );
    }

    #[test]
    fn import_rejects_traversal_and_existing_files_before_writing() {
        let destination = tempfile::tempdir().unwrap();
        std::fs::write(destination.path().join("safe.json"), "original").unwrap();
        let bundle = ForgeBundle {
            format: BUNDLE_FORMAT.to_string(),
            format_version: BUNDLE_VERSION,
            kind: BundleKind::Folder,
            source: "requests".to_string(),
            files: vec![
                BundleFile {
                    path: "safe.json".to_string(),
                    encoding: FileEncoding::Utf8,
                    content: "replacement".to_string(),
                },
                BundleFile {
                    path: "../escape.json".to_string(),
                    encoding: FileEncoding::Utf8,
                    content: "escape".to_string(),
                },
            ],
        };
        let input = destination.path().join("bad.forge.json");
        std::fs::write(&input, render_json(&bundle).unwrap()).unwrap();

        assert!(import_bundle(&input, destination.path()).is_err());
        assert_eq!(
            std::fs::read_to_string(destination.path().join("safe.json")).unwrap(),
            "original"
        );
        assert!(!destination
            .path()
            .parent()
            .unwrap()
            .join("escape.json")
            .exists());
    }
}
