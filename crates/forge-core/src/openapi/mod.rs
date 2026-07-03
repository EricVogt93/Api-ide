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
    crate::model::OpenApiBinding { spec_path: spec_path.to_string(), operations }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_binding_maps_request_files_to_operation_ids() {
        let binding = build_binding(
            "specs/petstore.yaml",
            &[
                ("pets/list-pets.request.json".to_string(), "listPets".to_string()),
                ("pets/get-pet.request.json".to_string(), "getPetById".to_string()),
            ],
        );
        assert_eq!(binding.spec_path, "specs/petstore.yaml");
        assert_eq!(binding.operations.get("pets/list-pets.request.json"), Some(&"listPets".to_string()));
        assert_eq!(binding.operations.len(), 2);
    }
}
