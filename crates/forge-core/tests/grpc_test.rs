//! End-to-end gRPC tests: compile a fixture .proto at runtime, serve a
//! dynamic echo service over real HTTP/2 with tonic, and call it through
//! `protocols::grpc::call_unary`.

use std::path::PathBuf;
use std::str::FromStr;

use forge_core::protocols::grpc::{
    call_unary, compile_protos, list_methods, DynamicCodec, GrpcError,
};
use prost_reflect::{DescriptorPool, DynamicMessage, Value};
use tonic::codegen::{BoxFuture, Context, Poll, Service};
use tonic::{Request, Response, Status};

fn proto_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/grpc/echo.proto")
}

fn pool() -> DescriptorPool {
    compile_protos(&[proto_path()], &[]).expect("fixture proto should compile")
}

// ---------------------------------------------------------------------
// A dynamic echo server built on the same descriptors (no codegen).
// ---------------------------------------------------------------------

#[derive(Clone)]
struct EchoServer {
    pool: DescriptorPool,
}

impl tonic::server::NamedService for EchoServer {
    const NAME: &'static str = "test.Echo";
}

impl Service<tonic::codegen::http::Request<tonic::body::Body>> for EchoServer {
    type Response = tonic::codegen::http::Response<tonic::body::Body>;
    type Error = std::convert::Infallible;
    type Future = BoxFuture<Self::Response, Self::Error>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: tonic::codegen::http::Request<tonic::body::Body>) -> Self::Future {
        let pool = self.pool.clone();
        Box::pin(async move {
            if req.uri().path() != "/test.Echo/Say" {
                return Ok(tonic::codegen::http::Response::builder()
                    .status(200)
                    .header("grpc-status", "12") // UNIMPLEMENTED
                    .header("content-type", "application/grpc")
                    .body(tonic::body::Body::empty())
                    .unwrap());
            }

            let method = pool
                .get_service_by_name("test.Echo")
                .unwrap()
                .methods()
                .find(|m| m.name() == "Say")
                .unwrap();

            struct Say {
                pool: DescriptorPool,
            }
            impl tonic::server::UnaryService<DynamicMessage> for Say {
                type Response = DynamicMessage;
                type Future = BoxFuture<Response<DynamicMessage>, Status>;

                fn call(&mut self, request: Request<DynamicMessage>) -> Self::Future {
                    let reply_desc =
                        self.pool.get_message_by_name("test.EchoReply").expect("reply descriptor");
                    let caller = request
                        .metadata()
                        .get("x-caller")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or_default()
                        .to_string();
                    let message = request
                        .get_ref()
                        .get_field_by_name("message")
                        .map(|v| v.as_str().unwrap_or_default().to_string())
                        .unwrap_or_default();
                    let count = request
                        .get_ref()
                        .get_field_by_name("count")
                        .and_then(|v| v.as_i32())
                        .unwrap_or(0);

                    let mut reply = DynamicMessage::new(reply_desc);
                    reply.set_field_by_name("message", Value::String(format!("echo: {message}")));
                    reply.set_field_by_name("count", Value::I32(count + 1));
                    reply.set_field_by_name("caller", Value::String(caller));

                    let mut response = Response::new(reply);
                    response
                        .metadata_mut()
                        .insert("x-served-by", "dynamic-echo".parse().unwrap());
                    Box::pin(async move { Ok(response) })
                }
            }

            let codec = DynamicCodec::new(method.output());
            let mut grpc = tonic::server::Grpc::new(codec);
            Ok(grpc.unary(Say { pool }, req).await)
        })
    }
}

/// Serve the echo service on an ephemeral port, returning its endpoint.
async fn spawn_echo_server() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let incoming = tonic::codegen::tokio_stream::wrappers::TcpListenerStream::new(listener);

    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(EchoServer { pool: pool() })
            .serve_with_incoming(incoming)
            .await
            .ok();
    });

    format!("http://{addr}")
}

// ---------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------

#[test]
fn compiles_protos_and_lists_methods_with_streaming_flag() {
    let methods = list_methods(&pool());

    assert_eq!(methods.len(), 2);
    assert_eq!(methods[0].path, "test.Echo/Say");
    assert_eq!(methods[0].input_type, "test.EchoRequest");
    assert_eq!(methods[0].output_type, "test.EchoReply");
    assert!(methods[0].is_unary);
    assert_eq!(methods[1].path, "test.Echo/Watch");
    assert!(!methods[1].is_unary, "server-streaming method must not be unary");
}

#[tokio::test]
async fn unary_call_roundtrips_json_and_metadata() {
    let endpoint = spawn_echo_server().await;

    let response = call_unary(
        &endpoint,
        &pool(),
        "test.Echo/Say",
        r#"{"message": "hallo", "count": 41}"#,
        &[("x-caller".to_string(), "forge-test".to_string())],
    )
    .await
    .expect("call should succeed");

    let json: serde_json::Value = serde_json::from_str(&response.json).expect("valid JSON");
    assert_eq!(json["message"], "echo: hallo");
    assert_eq!(json["count"], 42);
    assert_eq!(json["caller"], "forge-test", "request metadata must reach the server");
    assert!(
        response.metadata.iter().any(|(k, v)| k == "x-served-by" && v == "dynamic-echo"),
        "response metadata missing: {:?}",
        response.metadata
    );
}

#[tokio::test]
async fn streaming_methods_are_refused_up_front() {
    let err = call_unary("http://127.0.0.1:1", &pool(), "test.Echo/Watch", "{}", &[])
        .await
        .expect_err("streaming must be refused before connecting");
    assert!(matches!(err, GrpcError::Streaming(_)), "{err:?}");
}

#[tokio::test]
async fn unknown_method_and_bad_json_give_clear_errors() {
    let err = call_unary("http://127.0.0.1:1", &pool(), "test.Echo/Nope", "{}", &[])
        .await
        .expect_err("unknown method");
    assert!(matches!(err, GrpcError::MethodNotFound(_)), "{err:?}");

    let err = call_unary("http://127.0.0.1:1", &pool(), "test.Echo/Say", "{not json", &[])
        .await
        .expect_err("bad JSON");
    match err {
        GrpcError::RequestJson { type_name, .. } => assert_eq!(type_name, "test.EchoRequest"),
        other => panic!("expected RequestJson, got {other:?}"),
    }
}

#[tokio::test]
async fn invalid_metadata_key_fails_before_connecting() {
    // No server needed — validation happens before the connect attempt.
    let err = call_unary(
        "http://127.0.0.1:1",
        &pool(),
        "test.Echo/Say",
        r#"{"message": "x"}"#,
        &[("bad key!".to_string(), "v".to_string())],
    )
    .await
    .expect_err("invalid metadata key must fail");
    assert!(matches!(err, GrpcError::Metadata { .. }), "{err:?}");

    assert!(tonic::metadata::MetadataKey::<tonic::metadata::Ascii>::from_str("x-ok").is_ok());
}
