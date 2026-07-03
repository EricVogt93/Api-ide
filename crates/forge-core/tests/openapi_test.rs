//! End-to-end tests for the OpenAPI import pipeline: parse -> skeleton ->
//! contract generation -> binding, exercised against the petstore fixtures.

use forge_core::model::{BodyDef, Check, ParamKind};
use forge_core::openapi::{build_binding, contract_requests, operation_to_request, parse_spec};

fn fixture(name: &str) -> String {
    std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/openapi").join(name),
    )
    .expect("read fixture")
}

#[test]
fn json_and_yaml_petstore_produce_equivalent_operation_sets() {
    let json_spec = parse_spec(&fixture("petstore.json")).expect("parse json");
    let yaml_spec = parse_spec(&fixture("petstore.yaml")).expect("parse yaml");

    let mut json_ids: Vec<&str> = json_spec.operations.iter().map(|o| o.id.as_str()).collect();
    let mut yaml_ids: Vec<&str> = yaml_spec.operations.iter().map(|o| o.id.as_str()).collect();
    json_ids.sort();
    yaml_ids.sort();
    assert_eq!(json_ids, yaml_ids);
    assert_eq!(json_ids, vec!["createPet", "deletePet", "getPetById", "listPets"]);
}

#[test]
fn skeleton_for_create_pet_has_example_body_and_path_param_for_get() {
    let spec = parse_spec(&fixture("petstore.json")).expect("parse");

    let create = spec.operations.iter().find(|o| o.id == "createPet").unwrap();
    let req = operation_to_request(create);
    assert_eq!(req.url, "{{baseUrl}}/pets");
    match &req.body {
        BodyDef::Json { text } => {
            assert!(text.contains("Rex"));
            assert!(text.contains("dog"));
        }
        other => panic!("expected JSON body from example, got {other:?}"),
    }

    let get = spec.operations.iter().find(|o| o.id == "getPetById").unwrap();
    let req = operation_to_request(get);
    assert_eq!(req.url, "{{baseUrl}}/pets/:petId");
    let path_param = req.params.iter().find(|p| p.kind == ParamKind::Path).unwrap();
    assert_eq!(path_param.kv.key, "petId");
}

#[test]
fn contract_requests_generates_one_per_operation_with_schema_assertions() {
    let spec = parse_spec(&fixture("petstore.json")).expect("parse");
    let generated = contract_requests(&spec);
    assert_eq!(generated.len(), spec.operations.len());

    let (get_pet_req, op_id) = generated.iter().find(|(_, id)| id == "getPetById").unwrap();
    assert_eq!(op_id, "getPetById");
    assert!(get_pet_req.assertions.iter().any(|a| matches!(a.check, Check::StatusCode { value: 200, .. })));
    assert!(get_pet_req.assertions.iter().any(|a| matches!(a.check, Check::ContentType { .. })));
    assert!(get_pet_req.assertions.iter().any(|a| matches!(a.check, Check::JsonSchema { .. })));
    assert!(get_pet_req.assertions.iter().all(|a| a.note == "contract"));

    // delete has no 2xx response body -> only a status assertion (204 has no content).
    let (delete_req, _) = generated.iter().find(|(_, id)| id == "deletePet").unwrap();
    assert!(delete_req.assertions.iter().any(|a| matches!(a.check, Check::StatusCode { value: 204, .. })));
    assert!(!delete_req.assertions.iter().any(|a| matches!(a.check, Check::JsonSchema { .. })));
}

#[test]
fn build_binding_from_contract_requests_round_trips_operation_ids() {
    let spec = parse_spec(&fixture("petstore.json")).expect("parse");
    let generated = contract_requests(&spec);
    let pairs: Vec<(String, String)> = generated
        .iter()
        .map(|(req, op_id)| (format!("{}.request.json", req.name.to_lowercase().replace(' ', "-")), op_id.clone()))
        .collect();
    let binding = build_binding("specs/petstore.json", &pairs);
    assert_eq!(binding.spec_path, "specs/petstore.json");
    assert_eq!(binding.operations.len(), spec.operations.len());
}
