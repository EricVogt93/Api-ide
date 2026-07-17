//! Integration tests for the Postman collection / environment importers,
//! driven by a realistic v2.1 export fixture.

use forge_core::convert::{parse_postman, parse_postman_environment, ImportedItem};
use forge_core::model::{
    ApiKeyPlacement, AuthConfig, BodyDef, Method, ParamKind, PartContent,
};

const COLLECTION: &str = include_str!("fixtures/postman_collection.json");
const ENVIRONMENT: &str = include_str!("fixtures/postman_environment.json");

#[test]
fn imports_collection_metadata_variables_and_auth() {
    let import = parse_postman(COLLECTION).expect("fixture should parse");

    assert_eq!(import.name, "Payments API");
    assert_eq!(import.description, "Postman export used by the import tests.");
    assert_eq!(import.variables.get("baseUrl").map(String::as_str), Some("https://api.example.com"));
    assert_eq!(import.variables.get("accountId").map(String::as_str), Some("acc_123"));
    assert_eq!(
        import.auth,
        AuthConfig::Bearer { token: "{{accessToken}}".to_string(), prefix: None }
    );
    assert_eq!(import.request_count(), 5);
}

#[test]
fn imports_folder_tree_with_folder_level_auth() {
    let import = parse_postman(COLLECTION).expect("fixture should parse");

    let ImportedItem::Folder { name, description, auth, items, .. } = &import.items[0] else {
        panic!("first item should be the Charges folder");
    };
    assert_eq!(name, "Charges");
    assert_eq!(description, "Charge lifecycle");
    assert_eq!(
        *auth,
        AuthConfig::ApiKey {
            key: "X-Api-Key".to_string(),
            value: "{{apiKey}}".to_string(),
            placement: ApiKeyPlacement::Query,
        }
    );
    assert_eq!(items.len(), 2);
}

#[test]
fn imports_request_with_headers_query_params_and_json_body() {
    let import = parse_postman(COLLECTION).expect("fixture should parse");
    let ImportedItem::Folder { items, .. } = &import.items[0] else { panic!("folder") };
    let ImportedItem::Request(def) = &items[0] else { panic!("request") };

    assert_eq!(def.name, "Create Charge");
    assert_eq!(def.method, Method::Post);
    // Query string is stripped from the URL; params carry it instead.
    assert_eq!(def.url, "{{baseUrl}}/v1/charges");
    assert_eq!(def.auth, AuthConfig::Inherit);

    assert_eq!(def.headers.len(), 2);
    assert!(def.headers[0].enabled);
    assert_eq!(def.headers[0].key, "Content-Type");
    assert!(!def.headers[1].enabled, "disabled header should import as disabled");
    assert_eq!(def.headers[1].description, "debug flag");

    let query: Vec<_> = def.params.iter().filter(|p| p.kind == ParamKind::Query).collect();
    assert_eq!(query.len(), 2);
    assert_eq!(query[0].kv.key, "expand");
    assert_eq!(query[0].kv.value, "customer");
    assert!(!query[1].kv.enabled, "disabled query param should import as disabled");

    let BodyDef::Json { text } = &def.body else { panic!("json body, got {:?}", def.body) };
    assert!(text.contains("\"currency\": \"eur\""));
}

#[test]
fn imports_path_variables_as_path_params() {
    let import = parse_postman(COLLECTION).expect("fixture should parse");
    let ImportedItem::Folder { items, .. } = &import.items[0] else { panic!("folder") };
    let ImportedItem::Request(def) = &items[1] else { panic!("request") };

    assert_eq!(def.name, "Get Charge");
    assert_eq!(def.url, "{{baseUrl}}/v1/charges/:chargeId");
    let path: Vec<_> = def.params.iter().filter(|p| p.kind == ParamKind::Path).collect();
    assert_eq!(path.len(), 1);
    assert_eq!(path[0].kv.key, "chargeId");
    assert_eq!(path[0].kv.value, "ch_42");
}

#[test]
fn imports_basic_auth_and_urlencoded_body() {
    let import = parse_postman(COLLECTION).expect("fixture should parse");
    let ImportedItem::Request(def) = &import.items[1] else { panic!("request") };

    assert_eq!(def.name, "Login");
    assert_eq!(
        def.auth,
        AuthConfig::Basic { username: "{{user}}".to_string(), password: "{{pass}}".to_string() }
    );
    let BodyDef::FormUrlencoded { fields } = &def.body else { panic!("urlencoded") };
    assert_eq!(fields.len(), 2);
    assert_eq!(fields[0].key, "grant_type");
    assert!(!fields[1].enabled);
}

#[test]
fn imports_oauth2_client_credentials_and_multipart_body() {
    let import = parse_postman(COLLECTION).expect("fixture should parse");
    let ImportedItem::Request(def) = &import.items[2] else { panic!("request") };

    assert_eq!(def.name, "Upload Receipt");
    assert_eq!(
        def.auth,
        AuthConfig::OAuth2ClientCredentials {
            token_url: "{{baseUrl}}/oauth/token".to_string(),
            client_id: "forge-tests".to_string(),
            client_secret: "{{clientSecret}}".to_string(),
            scopes: vec!["receipts:write".to_string(), "receipts:read".to_string()],
            credentials_in_body: false,
        }
    );

    let BodyDef::Multipart { parts } = &def.body else { panic!("multipart") };
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0].name, "file");
    assert_eq!(parts[0].content, PartContent::File { path: "/tmp/receipt.pdf".to_string() });
    assert_eq!(parts[1].content, PartContent::Text { value: "Q3 receipt".to_string() });
    assert_eq!(parts[1].content_type.as_deref(), Some("text/plain"));
}

#[test]
fn imports_graphql_body_and_drops_unsupported_auth_with_a_note() {
    let import = parse_postman(COLLECTION).expect("fixture should parse");
    let ImportedItem::Request(def) = &import.items[3] else { panic!("request") };

    assert_eq!(def.name, "Search");
    assert_eq!(def.auth, AuthConfig::None, "unsupported ntlm auth should drop to None");
    let BodyDef::GraphQl { query, variables, .. } = &def.body else { panic!("graphql") };
    assert!(query.starts_with("query Charges"));
    assert_eq!(variables, "{ \"after\": null }");

    assert!(
        import.skipped.iter().any(|s| s.contains("Search") && s.contains("ntlm")),
        "skipped notes should mention the dropped auth: {:?}",
        import.skipped
    );
}

#[test]
fn imports_scripts_as_js_and_reports_example_responses() {
    use forge_core::model::ScriptLang;

    let import = parse_postman(COLLECTION).expect("fixture should parse");

    // Collection prerequest event becomes a beforeEach suite hook.
    assert_eq!(import.hooks.language, ScriptLang::Js);
    let before_each = import.hooks.before_each.as_deref().expect("collection prerequest imported");
    assert!(before_each.contains("pm.variables.set"), "{before_each:?}");
    assert!(import.hooks.after_each.is_none());

    // Request test event becomes its JS post-response script.
    let ImportedItem::Folder { items, .. } = &import.items[0] else { panic!("folder") };
    let ImportedItem::Request(def) = &items[0] else { panic!("request") };
    assert_eq!(def.scripts.language, ScriptLang::Js);
    let post = def.scripts.post_response.as_deref().expect("test script imported");
    assert!(post.contains("pm.test('status 201'"), "{post:?}");
    assert!(def.scripts.pre_request.is_none());

    // Scripts no longer appear in the skip list; example responses still do.
    assert!(
        !import.skipped.iter().any(|s| s.contains("script")),
        "scripts should import, not skip: {:?}",
        import.skipped
    );
    assert!(
        import.skipped.iter().any(|s| s.contains("saved example responses")),
        "example responses should be reported: {:?}",
        import.skipped
    );
}

#[test]
fn rejects_non_collection_json() {
    assert!(parse_postman("{\"foo\": 1}").is_err());
    assert!(parse_postman("not json").is_err());
}

#[test]
fn imports_environment_with_secrets_split_out() {
    let (env, secrets) = parse_postman_environment(ENVIRONMENT).expect("fixture should parse");

    assert_eq!(env.name, "Staging");
    assert_eq!(env.variables.len(), 4);

    let base = &env.variables["baseUrl"];
    assert!(!base.secret);
    assert_eq!(base.value.as_deref(), Some("https://staging.example.com"));

    let key = &env.variables["apiKey"];
    assert!(key.secret);
    assert_eq!(key.value, None, "secret values must not land in the committed env file");
    assert_eq!(secrets.get("apiKey").map(String::as_str), Some("sk_stage_123"));

    // Disabled variables are still imported.
    assert_eq!(env.variables["user"].value.as_deref(), Some("eric"));
    // A secret with no value stays declared-only.
    assert!(env.variables["emptySecret"].secret);
    assert!(!secrets.contains_key("emptySecret"));
}

#[test]
fn rejects_non_environment_json() {
    assert!(parse_postman_environment("{\"values\": 3}").is_err());
    assert!(parse_postman_environment(COLLECTION).is_err());
}
