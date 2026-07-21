//! Git-friendly OpenAPI sources inherited by folders and request files.

use std::path::{Path, PathBuf};

pub const FOLDER_OPENAPI_FILE: &str = ".forge-openapi";
pub const FILE_OPENAPI_SUFFIX: &str = ".forge-openapi";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenApiSelection {
    pub value: String,
    pub source: PathBuf,
}

fn selection_file(node: &Path) -> PathBuf {
    if node.is_dir() {
        node.join(FOLDER_OPENAPI_FILE)
    } else {
        let name = node
            .file_name()
            .map(|name| name.to_string_lossy())
            .unwrap_or_default();
        node.with_file_name(format!(".{name}{FILE_OPENAPI_SUFFIX}"))
    }
}

pub fn own_openapi(node: &Path) -> Result<Option<String>, String> {
    match std::fs::read_to_string(selection_file(node)) {
        Ok(value) => Ok((!value.trim().is_empty()).then(|| value.trim().to_string())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!(
            "cannot read OpenAPI source for {}: {error}",
            node.display()
        )),
    }
}

pub fn effective_openapi(root: &Path, node: &Path) -> Result<Option<OpenApiSelection>, String> {
    if !node.starts_with(root) {
        return Err(format!("{} is outside the project", node.display()));
    }
    let mut current = Some(node);
    while let Some(candidate) = current {
        if let Some(value) = own_openapi(candidate)? {
            return Ok(Some(OpenApiSelection {
                value,
                source: candidate.to_path_buf(),
            }));
        }
        if candidate == root {
            break;
        }
        current = candidate.parent();
    }
    Ok(None)
}

pub fn set_openapi(node: &Path, value: &str) -> Result<(), String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("OpenAPI source must not be empty".to_string());
    }
    let path = selection_file(node);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    }
    std::fs::write(&path, format!("{value}\n"))
        .map_err(|error| format!("cannot write {}: {error}", path.display()))
}

pub fn remove_openapi(node: &Path) -> Result<(), String> {
    let path = selection_file(node);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("cannot remove {}: {error}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_inherits_nearest_openapi_source() {
        let root = tempfile::tempdir().unwrap();
        let story = root.path().join("requests/checkout");
        std::fs::create_dir_all(&story).unwrap();
        let request = story.join("submit.request.json");
        std::fs::write(&request, "{}").unwrap();

        set_openapi(root.path(), "specs/root.yaml").unwrap();
        set_openapi(&story, "https://api.example.com/openapi.json").unwrap();
        assert_eq!(
            effective_openapi(root.path(), &request).unwrap().unwrap(),
            OpenApiSelection {
                value: "https://api.example.com/openapi.json".to_string(),
                source: story.clone(),
            }
        );

        remove_openapi(&story).unwrap();
        assert_eq!(
            effective_openapi(root.path(), &request)
                .unwrap()
                .unwrap()
                .value,
            "specs/root.yaml"
        );
    }
}
