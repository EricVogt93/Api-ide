//! Integration tests for `forge_core::protocols` (GraphQL introspection,
//! WebSocket and SSE sessions).

use forge_core::protocols::graphql::{build_request_body, introspect, validate_query};
use forge_core::protocols::sse::{subscribe, SseEvent};
use forge_core::protocols::websocket::{connect, WsEvent, WsOutgoing};

use futures::{SinkExt, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ---------------------------------------------------------------------
// WebSocket
// ---------------------------------------------------------------------

#[tokio::test]
async fn websocket_connect_echo_and_close() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind test ws server");
    let addr = listener.local_addr().unwrap();

    // A tiny echo server: relays text messages back, replies to the
    // client's close frame and then stops.
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept");
        let mut ws = tokio_tungstenite::accept_async(stream)
            .await
            .expect("server handshake");
        while let Some(Ok(msg)) = ws.next().await {
            match msg {
                Message::Text(text) => {
                    let _ = ws.send(Message::Text(text)).await;
                }
                Message::Close(_) => {
                    // `Sink::close` (not `send(Message::Close(..))`) is
                    // what actually flushes tungstenite's auto-queued close
                    // reply back to the peer.
                    let _ = SinkExt::close(&mut ws).await;
                    break;
                }
                _ => {}
            }
        }
    });

    let url = format!("ws://{addr}/");
    let mut session = connect(&url, &[], &Default::default()).await.expect("client connect");

    let first = session.events.recv().await.expect("connected event");
    assert!(matches!(first, WsEvent::Connected), "got {first:?}");

    session
        .outgoing
        .send(WsOutgoing::Text("hello".to_string()))
        .expect("send text");

    let echoed = session.events.recv().await.expect("echo event");
    match echoed {
        WsEvent::Text { text, .. } => assert_eq!(text, "hello"),
        other => panic!("expected an echoed text event, got {other:?}"),
    }

    session.close();

    // Drain events until we see a clean close (there may be nothing else
    // in between).
    let mut saw_closed = false;
    while let Some(event) = session.events.recv().await {
        if let WsEvent::Closed { .. } = event {
            saw_closed = true;
            break;
        }
    }
    assert!(saw_closed, "expected a Closed event after close()");
}

// ---------------------------------------------------------------------
// SSE
// ---------------------------------------------------------------------

#[tokio::test]
async fn sse_subscribe_parses_events_then_closes() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind test sse server");
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("accept");

        // Drain (and ignore) the request.
        let mut buf = [0u8; 4096];
        let _ = stream.read(&mut buf).await;

        let body = "id: 1\nevent: greeting\ndata: hello\n\nid: 2\nevent: greeting\ndata: world\n\n";
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n{body}"
        );
        let _ = stream.write_all(response.as_bytes()).await;
        let _ = stream.shutdown().await;
    });

    let url = format!("http://{addr}/events");
    let mut session = subscribe(&url, &[], &Default::default()).await.expect("subscribe");

    let open = session.events.recv().await.expect("open event");
    assert!(matches!(open, SseEvent::Open), "got {open:?}");

    let first = session.events.recv().await.expect("first event");
    match first {
        SseEvent::Event {
            id, event, data, ..
        } => {
            assert_eq!(id, "1");
            assert_eq!(event, "greeting");
            assert_eq!(data, "hello");
        }
        other => panic!("expected an SSE event, got {other:?}"),
    }

    let second = session.events.recv().await.expect("second event");
    match second {
        SseEvent::Event {
            id, event, data, ..
        } => {
            assert_eq!(id, "2");
            assert_eq!(event, "greeting");
            assert_eq!(data, "world");
        }
        other => panic!("expected an SSE event, got {other:?}"),
    }

    let closed = session.events.recv().await.expect("closed event");
    assert!(matches!(closed, SseEvent::Closed), "got {closed:?}");
}

// ---------------------------------------------------------------------
// GraphQL
// ---------------------------------------------------------------------

#[tokio::test]
async fn graphql_introspect_parses_schema() {
    let fixture = include_str!("fixtures/protocols/introspection.json");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(fixture.as_bytes().to_vec(), "application/json"),
        )
        .mount(&server)
        .await;

    let schema = introspect(&format!("{}/graphql", server.uri()), &[])
        .await
        .expect("introspect should succeed");

    assert_eq!(schema.query_type.as_deref(), Some("Query"));
    assert_eq!(schema.mutation_type, None);

    // Introspection meta-types (`__Type`, etc) are filtered out.
    assert!(!schema.types.iter().any(|t| t.name.starts_with("__")));

    let query_type = schema
        .types
        .iter()
        .find(|t| t.name == "Query")
        .expect("Query type present");
    let pet_field = query_type
        .fields
        .iter()
        .find(|f| f.name == "pet")
        .expect("pet field present");
    assert_eq!(pet_field.type_display, "Pet");
    assert_eq!(pet_field.args, vec![("id".to_string(), "ID!".to_string())]);

    let pets_field = query_type
        .fields
        .iter()
        .find(|f| f.name == "pets")
        .expect("pets field present");
    assert_eq!(pets_field.type_display, "[Pet!]!");

    let pet_type = schema
        .types
        .iter()
        .find(|t| t.name == "Pet")
        .expect("Pet type present");
    let id_field = pet_type.fields.iter().find(|f| f.name == "id").unwrap();
    assert_eq!(id_field.type_display, "ID!");
    let name_field = pet_type.fields.iter().find(|f| f.name == "name").unwrap();
    assert_eq!(name_field.type_display, "String");
}

#[tokio::test]
async fn graphql_introspect_surfaces_http_errors() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
        .mount(&server)
        .await;

    let err = introspect(&format!("{}/graphql", server.uri()), &[])
        .await
        .expect_err("500 should be an error");
    assert!(err.to_string().contains("500"));
}

#[test]
fn validate_query_accepts_well_formed_documents() {
    validate_query("{ pet(id: \"1\") { id name } }").expect("valid query should parse");
}

#[test]
fn validate_query_rejects_syntax_errors_with_position() {
    let err = validate_query("{ pet(id: \"1\") { id name }")
        .expect_err("unterminated block should fail to parse");
    assert!(
        err.to_lowercase().contains("parse error"),
        "expected a parse error message, got: {err}"
    );
}

#[test]
fn build_request_body_defaults_and_parses_variables() {
    let body = build_request_body("{ pets { id } }", "", None).expect("empty variables ok");
    assert_eq!(body["query"], "{ pets { id } }");
    assert_eq!(body["variables"], serde_json::json!({}));
    assert!(body.get("operationName").is_none());

    let body = build_request_body(
        "query Pets($limit: Int) { pets(limit: $limit) { id } }",
        r#"{"limit": 5}"#,
        Some("Pets"),
    )
    .expect("valid variables ok");
    assert_eq!(body["variables"], serde_json::json!({ "limit": 5 }));
    assert_eq!(body["operationName"], "Pets");

    let err = build_request_body("{ pets { id } }", "not json", None)
        .expect_err("invalid variables JSON should error");
    assert!(err.contains("invalid variables JSON"));
}
