//! Integration tests for the Bruno collection importer, driven by an
//! on-disk fixture collection under `tests/fixtures/bruno-collection`.

use std::path::PathBuf;

use forge_core::convert::{import_bruno, ImportedItem};
use forge_core::model::{
    ApiKeyPlacement, AuthConfig, BodyDef, Check, ExtractorSource, Method, NumberOp, ParamKind,
    PartContent, ValueOp,
};

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/bruno-collection")
}

#[test]
fn imports_collection_metadata_and_collection_auth() {
    let import = import_bruno(&fixture_root()).expect("fixture should import");

    assert_eq!(import.collection.name, "Payments Bruno");
    assert_eq!(import.collection.description, "Collection-level docs.");
    assert_eq!(
        import.collection.auth,
        AuthConfig::Bearer {
            token: "{{accessToken}}".to_string(),
            prefix: None
        }
    );
    assert_eq!(import.collection.request_count(), 5);
}

#[test]
fn orders_items_by_meta_seq_and_reads_folder_auth() {
    let import = import_bruno(&fixture_root()).expect("fixture should import");
    let items = &import.collection.items;

    let ImportedItem::Folder {
        name,
        auth,
        items: children,
        ..
    } = &items[0]
    else {
        panic!(
            "Charges folder (seq 1) should sort first, got {:?}",
            items[0]
        );
    };
    assert_eq!(name, "Charges");
    assert_eq!(
        *auth,
        AuthConfig::ApiKey {
            key: "X-Api-Key".to_string(),
            value: "{{apiKey}}".to_string(),
            placement: ApiKeyPlacement::Query,
        }
    );
    assert_eq!(children.len(), 2);

    let names: Vec<_> = items
        .iter()
        .map(|i| match i {
            ImportedItem::Folder { name, .. } => name.clone(),
            ImportedItem::Request(def) => def.name.clone(),
        })
        .collect();
    assert_eq!(names, ["Charges", "Login", "Upload Receipt", "Search"]);
}

#[test]
fn imports_request_with_headers_params_json_body_and_docs() {
    let import = import_bruno(&fixture_root()).expect("fixture should import");
    let ImportedItem::Folder { items, .. } = &import.collection.items[0] else {
        panic!("folder")
    };
    let ImportedItem::Request(def) = &items[0] else {
        panic!("request")
    };

    assert_eq!(def.name, "Create Charge");
    assert_eq!(def.method, Method::Post);
    assert_eq!(def.url, "{{baseUrl}}/v1/charges");
    assert_eq!(def.description, "Creates a charge.");
    assert_eq!(def.auth, AuthConfig::Inherit);

    assert_eq!(def.headers.len(), 2);
    assert_eq!(def.headers[0].key, "Content-Type");
    assert!(
        !def.headers[1].enabled,
        "~-prefixed header should import disabled"
    );
    assert_eq!(def.headers[1].key, "X-Debug");

    let query: Vec<_> = def
        .params
        .iter()
        .filter(|p| p.kind == ParamKind::Query)
        .collect();
    assert_eq!(query.len(), 2);
    assert!(!query[1].kv.enabled);

    let BodyDef::Json { text } = &def.body else {
        panic!("json body, got {:?}", def.body)
    };
    assert!(text.contains("\"currency\": \"eur\""));
    assert!(
        text.starts_with('{'),
        "indentation should be stripped: {text:?}"
    );
}

#[test]
fn maps_assert_block_to_declarative_assertions() {
    let import = import_bruno(&fixture_root()).expect("fixture should import");
    let ImportedItem::Folder { items, .. } = &import.collection.items[0] else {
        panic!("folder")
    };
    let ImportedItem::Request(def) = &items[0] else {
        panic!("request")
    };

    assert_eq!(def.assertions.len(), 3, "{:?}", def.assertions);
    assert_eq!(
        def.assertions[0].check,
        Check::StatusCode {
            op: NumberOp::Eq,
            value: 201
        }
    );
    assert_eq!(
        def.assertions[1].check,
        Check::JsonPath {
            path: "$.currency".to_string(),
            op: ValueOp::Equals,
            value: serde_json::json!("eur"),
        }
    );
    assert_eq!(
        def.assertions[2].check,
        Check::ResponseTimeBelow { max_ms: 2000 }
    );

    // The unsupported `isJson` operator lands in the skip report instead.
    assert!(
        import
            .collection
            .skipped
            .iter()
            .any(|s| s.contains("res.body.weird") && s.contains("isJson")),
        "{:?}",
        import.collection.skipped
    );
}

#[test]
fn maps_post_response_vars_to_extractors() {
    let import = import_bruno(&fixture_root()).expect("fixture should import");
    let ImportedItem::Folder { items, .. } = &import.collection.items[0] else {
        panic!("folder")
    };
    let ImportedItem::Request(def) = &items[0] else {
        panic!("request")
    };

    assert_eq!(def.extractors.len(), 3, "{:?}", def.extractors);
    assert_eq!(def.extractors[0].var, "chargeId");
    assert_eq!(
        def.extractors[0].source,
        ExtractorSource::JsonPath {
            expr: "$.id".to_string()
        }
    );
    assert_eq!(def.extractors[1].var, "fullBody");
    assert_eq!(
        def.extractors[1].source,
        ExtractorSource::JsonPath {
            expr: "$".to_string()
        }
    );
    assert!(
        !def.extractors[2].enabled,
        "~-prefixed var should import disabled"
    );

    // `res.headers.etag` is not a body read — reported, not silently dropped.
    assert!(
        import
            .collection
            .skipped
            .iter()
            .any(|s| s.contains("weird: res.headers.etag")),
        "{:?}",
        import.collection.skipped
    );
}

#[test]
fn imports_path_params_basic_auth_multipart_and_graphql() {
    let import = import_bruno(&fixture_root()).expect("fixture should import");
    let items = &import.collection.items;

    let ImportedItem::Folder { items: charges, .. } = &items[0] else {
        panic!("folder")
    };
    let ImportedItem::Request(get_charge) = &charges[1] else {
        panic!("request")
    };
    assert_eq!(get_charge.url, "{{baseUrl}}/v1/charges/:chargeId");
    let path: Vec<_> = get_charge
        .params
        .iter()
        .filter(|p| p.kind == ParamKind::Path)
        .collect();
    assert_eq!(path.len(), 1);
    assert_eq!(path[0].kv.key, "chargeId");

    let ImportedItem::Request(login) = &items[1] else {
        panic!("request")
    };
    assert_eq!(
        login.auth,
        AuthConfig::Basic {
            username: "{{user}}".to_string(),
            password: "{{pass}}".to_string()
        }
    );
    let BodyDef::FormUrlencoded { fields } = &login.body else {
        panic!("urlencoded")
    };
    assert_eq!(fields.len(), 2);
    assert!(!fields[1].enabled);

    let ImportedItem::Request(upload) = &items[2] else {
        panic!("request")
    };
    assert_eq!(upload.auth, AuthConfig::None);
    let BodyDef::Multipart { parts } = &upload.body else {
        panic!("multipart")
    };
    assert_eq!(
        parts[0].content,
        PartContent::File {
            path: "/tmp/receipt.pdf".to_string()
        }
    );
    assert_eq!(parts[0].content_type.as_deref(), Some("application/pdf"));
    assert_eq!(
        parts[1].content,
        PartContent::Text {
            value: "Q3 receipt".to_string()
        }
    );

    let ImportedItem::Request(search) = &items[3] else {
        panic!("request")
    };
    let BodyDef::GraphQl {
        query, variables, ..
    } = &search.body
    else {
        panic!("graphql")
    };
    assert!(query.starts_with("query Charges"));
    assert_eq!(variables, "{ \"after\": null }");
    assert_eq!(
        search.auth,
        AuthConfig::AwsSigV4 {
            access_key: "AKIA123".to_string(),
            secret_key: String::new(),
            session_token: None,
            region: String::new(),
            service: String::new(),
        }
    );
}

#[test]
fn reports_skipped_scripts() {
    let import = import_bruno(&fixture_root()).expect("fixture should import");
    assert!(
        import
            .collection
            .skipped
            .iter()
            .any(|s| s.contains("Charges/Create Charge") && s.contains("script:post-response")),
        "{:?}",
        import.collection.skipped
    );
}

#[test]
fn imports_environments_with_secret_declarations() {
    let import = import_bruno(&fixture_root()).expect("fixture should import");
    assert_eq!(import.environments.len(), 1);

    let (env, secrets) = &import.environments[0];
    assert_eq!(env.name, "staging");
    assert_eq!(
        env.variables["baseUrl"].value.as_deref(),
        Some("https://staging.example.com")
    );
    assert!(env.variables["apiKey"].secret);
    assert_eq!(env.variables["apiKey"].value, None);
    assert!(
        env.variables.contains_key("oldToken"),
        "disabled secret is still declared"
    );
    assert!(
        secrets.is_empty(),
        "Bruno exports never contain secret values"
    );
}

#[test]
fn rejects_a_directory_without_bruno_json() {
    let dir = tempfile::tempdir().expect("tempdir");
    let err = import_bruno(dir.path()).expect_err("no bruno.json must fail");
    assert!(err.to_string().contains("bruno.json"));
}

#[test]
fn skips_ignored_hidden_and_unrelated_directories() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    std::fs::write(root.join("bruno.json"), r#"{"name":"Skip Test"}"#).unwrap();

    let bru = "meta {\n  name: Ping\n  type: http\n}\nget {\n  url: https://example.com\n}\n";
    std::fs::create_dir(root.join("api")).unwrap();
    std::fs::write(root.join("api/ping.bru"), bru).unwrap();

    // Must all be invisible to the import:
    std::fs::create_dir_all(root.join("node_modules/some-pkg")).unwrap();
    std::fs::write(root.join("node_modules/some-pkg/evil.bru"), bru).unwrap();
    std::fs::create_dir(root.join(".git")).unwrap();
    std::fs::write(root.join(".git/hidden.bru"), bru).unwrap();
    std::fs::create_dir(root.join("venv")).unwrap();
    std::fs::write(root.join("venv/pyvenv.bru"), bru).unwrap();
    std::fs::create_dir(root.join("src")).unwrap();
    std::fs::write(root.join("src/main.js"), "// no .bru here").unwrap();

    let import = import_bruno(root).expect("collection should import");
    assert_eq!(import.collection.request_count(), 1);
    assert_eq!(import.collection.items.len(), 1);
    let ImportedItem::Folder { name, items, .. } = &import.collection.items[0] else {
        panic!("expected the api folder, got {:?}", import.collection.items);
    };
    assert_eq!(name, "api");
    assert_eq!(items.len(), 1);
}
