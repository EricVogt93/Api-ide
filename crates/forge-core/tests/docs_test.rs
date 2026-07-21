use forge_core::reqv1::{load_project, load_request_document, ProjectIndex, SequenceDocument};
use forge_core::store::Workspace;

#[test]
fn demo_workspace_loads() {
    let root =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/demo-workspace");
    let ws = Workspace::load(&root).expect("demo workspace should load");
    assert_eq!(ws.meta.name, "Demo");
    assert_eq!(ws.environments.len(), 1);
    let env = &ws.environments[0].env;
    assert_eq!(env.name, "httpbin");
    assert!(env.variables.get("apiToken").unwrap().secret);
    assert_eq!(
        env.variables.get("baseUrl").unwrap().value.as_deref(),
        Some("https://httpbin.org")
    );

    assert_eq!(ws.collections.len(), 1);
    let col = &ws.collections[0];
    assert_eq!(col.meta.name, "HTTPBin");
    let requests = col.requests();
    let names: Vec<&str> = requests.iter().map(|r| r.def.name.as_str()).collect();
    assert!(names.contains(&"Get JSON"));
    assert!(names.contains(&"Post Echo"));
    assert!(names.contains(&"Auth Bearer"));
    assert!(names.contains(&"Get 404"));
    assert_eq!(requests.len(), 4);
}

#[test]
fn demo_api_project_is_a_valid_feature_gallery() {
    let root =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/demo-workspace");
    let project = load_project(&root).expect("demo project.json should load");
    assert!(project.auth.is_some(), "demo should exercise project auth");

    let index = ProjectIndex::scan(&root).expect("demo project should index");
    assert!(
        index.broken.is_empty(),
        "broken demo refs: {:?}",
        index.broken
    );
    assert!(index.requests.len() >= 5);
    assert!(index.assets.iter().any(|asset| asset.metadata.is_some()));
    assert!(index.environments.iter().any(|name| name == "demo"));

    for request in &index.requests {
        load_request_document(std::path::Path::new(&request.path))
            .unwrap_or_else(|error| panic!("invalid {}: {error}", request.rel_path));
    }

    let sequence_path = root.join("demo.sequence.json");
    let sequence = SequenceDocument::parse(
        &std::fs::read_to_string(&sequence_path).expect("demo sequence should exist"),
    )
    .expect("demo sequence should parse");
    assert_eq!(sequence.resolve_files(&root).unwrap().len(), 3);
}
