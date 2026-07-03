use forge_core::store::Workspace;

#[test]
fn demo_workspace_loads() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/demo-workspace");
    let ws = Workspace::load(&root).expect("demo workspace should load");
    assert_eq!(ws.meta.name, "Demo");
    assert_eq!(ws.environments.len(), 1);
    let env = &ws.environments[0].env;
    assert_eq!(env.name, "httpbin");
    assert!(env.variables.get("apiToken").unwrap().secret);
    assert_eq!(env.variables.get("baseUrl").unwrap().value.as_deref(), Some("https://httpbin.org"));

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
