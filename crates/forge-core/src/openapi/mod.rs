//! OpenAPI 3.x import: request skeleton generation, collection binding and
//! contract-test (assertion) generation from response schemas.

mod contract;
mod import;
mod skeleton;

pub use contract::*;
pub use import::*;
pub use skeleton::*;

use std::collections::BTreeMap;

/// Build an [`crate::model::OpenApiBinding`] linking a collection to the
/// spec it was imported from, mapping each generated request file to the
/// `operationId` it was generated for.
pub fn build_binding(spec_path: &str, pairs: &[(String, String)]) -> crate::model::OpenApiBinding {
    let mut operations = BTreeMap::new();
    for (req_file, op_id) in pairs {
        operations.insert(req_file.clone(), op_id.clone());
    }
    crate::model::OpenApiBinding {
        spec_path: spec_path.to_string(),
        operations,
    }
}

/// Find and parse the project's OpenAPI spec by convention: well-known
/// root-level file names first, then everything under `specs/`. Returns
/// the first candidate that parses.
pub fn discover_spec(root: &std::path::Path) -> Option<ParsedSpec> {
    let mut candidates: Vec<std::path::PathBuf> = [
        "openapi.json",
        "openapi.yaml",
        "openapi.yml",
        "swagger.json",
        "swagger.yaml",
        "swagger.yml",
    ]
    .into_iter()
    .map(|name| root.join(name))
    .filter(|path| path.is_file())
    .collect();

    let specs = root.join("specs");
    let mut pending = specs.is_dir().then_some(specs).into_iter().collect::<Vec<_>>();
    while let Some(directory) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| {
                    matches!(
                        extension.to_ascii_lowercase().as_str(),
                        "json" | "yaml" | "yml"
                    )
                })
            {
                candidates.push(path);
            }
        }
    }
    candidates.sort();
    candidates.dedup();
    candidates.into_iter().find_map(|path| {
        let text = std::fs::read_to_string(&path).ok()?;
        parse_spec(&text).ok()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_binding_maps_request_files_to_operation_ids() {
        let binding = build_binding(
            "specs/petstore.yaml",
            &[
                (
                    "pets/list-pets.request.json".to_string(),
                    "listPets".to_string(),
                ),
                (
                    "pets/get-pet.request.json".to_string(),
                    "getPetById".to_string(),
                ),
            ],
        );
        assert_eq!(binding.spec_path, "specs/petstore.yaml");
        assert_eq!(
            binding.operations.get("pets/list-pets.request.json"),
            Some(&"listPets".to_string())
        );
        assert_eq!(binding.operations.len(), 2);
    }
}
