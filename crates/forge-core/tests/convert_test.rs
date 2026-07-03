//! Integration tests for `forge_core::convert` (curl import/export + code
//! snippets), exercised only through the crate's public API.

use forge_core::convert::{generate, parse_curl, to_curl, CurlExportOptions, SnippetLang};
use forge_core::model::{AuthConfig, BodyDef, Method, RawLanguage};

#[test]
fn imports_a_realistic_browser_style_curl_command() {
    let cmd = r#"curl 'https://api.example.com/v2/orders?status=open' \
  -H 'Accept: application/json' \
  -H 'Content-Type: application/json' \
  -u 'svc:hunter2' \
  -b 'sid=xyz' \
  -L \
  --compressed \
  -X PUT \
  --data-raw '{"qty":3}'"#;

    let def = parse_curl(cmd).expect("valid curl command should parse");

    assert_eq!(def.method, Method::Put);
    assert_eq!(def.url, "https://api.example.com/v2/orders?status=open");
    assert!(def.params.is_empty(), "query string should stay inline, not exploded into params");
    assert_eq!(def.auth, AuthConfig::Basic { username: "svc".into(), password: "hunter2".into() });
    assert_eq!(def.settings.follow_redirects, Some(true));
    match &def.body {
        BodyDef::Json { text } => assert_eq!(text, r#"{"qty":3}"#),
        other => panic!("expected JSON body, got {other:?}"),
    }
    assert!(def.headers.iter().any(|h| h.key == "Cookie" && h.value == "sid=xyz"));
    assert!(def.headers.iter().any(|h| h.key == "Accept-Encoding"));
}

#[test]
fn export_import_roundtrip_is_stable() {
    let mut def = forge_core::model::RequestDef::new(
        "Get widget",
        Method::Get,
        "https://api.example.com/widgets/:id",
    );
    def.headers.push(forge_core::model::KeyValue::new("X-Trace", "abc-123"));
    def.auth = AuthConfig::Basic { username: "root".into(), password: "toor".into() };
    def.body = BodyDef::Raw { text: "plain text body".into(), language: RawLanguage::Text };

    let curl = to_curl(&def, &CurlExportOptions::default());
    let reparsed = parse_curl(&curl).expect("exported curl command should re-parse");

    assert_eq!(reparsed.method, def.method);
    assert_eq!(reparsed.url, def.url);
    assert_eq!(reparsed.auth, def.auth);
    assert!(reparsed.headers.iter().any(|h| h.key == "X-Trace" && h.value == "abc-123"));
    match &reparsed.body {
        BodyDef::Raw { text, .. } => assert_eq!(text, "plain text body"),
        other => panic!("expected raw body after roundtrip, got {other:?}"),
    }
}

#[test]
fn to_curl_single_line_when_multiline_disabled() {
    let def = forge_core::model::RequestDef::new("Ping", Method::Get, "https://example.com/ping");
    let opts = CurlExportOptions { multiline: false, long_flags: false };
    let out = to_curl(&def, &opts);
    assert!(!out.contains('\n'));
    assert_eq!(out, "curl -X GET 'https://example.com/ping'");
}

#[test]
fn every_snippet_language_produces_non_empty_output() {
    let mut def = forge_core::model::RequestDef::new(
        "List items",
        Method::Get,
        "https://api.example.com/items",
    );
    def.params.push(forge_core::model::Param {
        kv: forge_core::model::KeyValue::new("page", "2"),
        kind: forge_core::model::ParamKind::Query,
    });

    for lang in SnippetLang::all() {
        let snippet = generate(&def, lang);
        assert!(!snippet.is_empty(), "{lang:?} produced empty snippet");
        assert!(snippet.contains("page=2"), "{lang:?} snippet missing query param: {snippet}");
    }
}

#[test]
fn variables_survive_curl_export_verbatim() {
    let mut def =
        forge_core::model::RequestDef::new("Templated", Method::Post, "https://{{host}}/api/{{path}}");
    def.headers.push(forge_core::model::KeyValue::new("Authorization", "Bearer {{token}}"));
    def.body = BodyDef::Json { text: r#"{"id":"{{id}}"}"#.into() };

    let curl = to_curl(&def, &CurlExportOptions::default());
    assert!(curl.contains("{{host}}"));
    assert!(curl.contains("{{path}}"));
    assert!(curl.contains("{{token}}"));
    assert!(curl.contains("{{id}}"));
}
