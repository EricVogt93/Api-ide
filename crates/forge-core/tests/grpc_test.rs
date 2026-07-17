//! End-to-end gRPC tests: compile a fixture .proto at runtime, serve a
//! dynamic echo service (all four method shapes) over real HTTP/2 with
//! tonic, and call it through `protocols::grpc::call`.

use std::path::PathBuf;
use std::str::FromStr;

use forge_core::protocols::grpc::{call, compile_protos, list_methods, DynamicCodec, GrpcError};
use prost_reflect::{DescriptorPool, DynamicMessage, MessageDescriptor, Value};
use tonic::codegen::tokio_stream::{self, StreamExt};
use tonic::codegen::{BoxFuture, Context, Poll, Service};
use tonic::{Request, Response, Status, Streaming};

fn proto_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/grpc/echo.proto")
}

fn pool() -> DescriptorPool {
    compile_protos(&[proto_path()], &[]).expect("fixture proto should compile")
}

// ---------------------------------------------------------------------
// A dynamic echo server built on the same descriptors (no codegen).
// ---------------------------------------------------------------------

fn reply(desc: &MessageDescriptor, message: String, count: i32, caller: &str) -> DynamicMessage {
    let mut reply = DynamicMessage::new(desc.clone());
    reply.set_field_by_name("message", Value::String(message));
    reply.set_field_by_name("count", Value::I32(count));
    reply.set_field_by_name("caller", Value::String(caller.to_string()));
    reply
}

fn req_fields(msg: &DynamicMessage) -> (String, i32) {
    let message = msg
        .get_field_by_name("message")
        .map(|v| v.as_str().unwrap_or_default().to_string())
        .unwrap_or_default();
    let count = msg.get_field_by_name("count").and_then(|v| v.as_i32()).unwrap_or(0);
    (message, count)
}

type ReplyStream =
    std::pin::Pin<Box<dyn tokio_stream::Stream<Item = Result<DynamicMessage, Status>> + Send>>;

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
            let reply_desc = pool.get_message_by_name("test.EchoReply").expect("reply descriptor");
            let method = pool
                .get_service_by_name("test.Echo")
                .and_then(|svc| {
                    let name = req.uri().path().rsplit('/').next().unwrap_or_default().to_string();
                    svc.methods().find(|m| m.name() == name)
                });
            let Some(method) = method else {
                return Ok(tonic::codegen::http::Response::builder()
                    .status(200)
                    .header("grpc-status", "12") // UNIMPLEMENTED
                    .header("content-type", "application/grpc")
                    .body(tonic::body::Body::empty())
                    .unwrap());
            };

            let codec = DynamicCodec::new(method.output());
            let mut grpc = tonic::server::Grpc::new(codec);

            let response = match method.name() {
                "Say" => {
                    struct Say(MessageDescriptor);
                    impl tonic::server::UnaryService<DynamicMessage> for Say {
                        type Response = DynamicMessage;
                        type Future = BoxFuture<Response<DynamicMessage>, Status>;
                        fn call(&mut self, request: Request<DynamicMessage>) -> Self::Future {
                            let caller = request
                                .metadata()
                                .get("x-caller")
                                .and_then(|v| v.to_str().ok())
                                .unwrap_or_default()
                                .to_string();
                            let (message, count) = req_fields(request.get_ref());
                            let out = reply(&self.0, format!("echo: {message}"), count + 1, &caller);
                            let mut response = Response::new(out);
                            response
                                .metadata_mut()
                                .insert("x-served-by", "dynamic-echo".parse().unwrap());
                            Box::pin(async move { Ok(response) })
                        }
                    }
                    grpc.unary(Say(reply_desc), req).await
                }
                "Watch" => {
                    struct Watch(MessageDescriptor);
                    impl tonic::server::ServerStreamingService<DynamicMessage> for Watch {
                        type Response = DynamicMessage;
                        type ResponseStream = ReplyStream;
                        type Future = BoxFuture<Response<Self::ResponseStream>, Status>;
                        fn call(&mut self, request: Request<DynamicMessage>) -> Self::Future {
                            let (message, count) = req_fields(request.get_ref());
                            let desc = self.0.clone();
                            let items: Vec<Result<DynamicMessage, Status>> = (0..count)
                                .map(|i| Ok(reply(&desc, format!("{message} #{i}"), i, "")))
                                .collect();
                            let stream: ReplyStream = Box::pin(tokio_stream::iter(items));
                            Box::pin(async move { Ok(Response::new(stream)) })
                        }
                    }
                    grpc.server_streaming(Watch(reply_desc), req).await
                }
                "Sum" => {
                    struct Sum(MessageDescriptor);
                    impl tonic::server::ClientStreamingService<DynamicMessage> for Sum {
                        type Response = DynamicMessage;
                        type Future = BoxFuture<Response<DynamicMessage>, Status>;
                        fn call(&mut self, request: Request<Streaming<DynamicMessage>>) -> Self::Future {
                            let desc = self.0.clone();
                            Box::pin(async move {
                                let mut stream = request.into_inner();
                                let mut total = 0;
                                let mut n = 0;
                                while let Some(msg) = stream.message().await? {
                                    let (_, count) = req_fields(&msg);
                                    total += count;
                                    n += 1;
                                }
                                Ok(Response::new(reply(&desc, format!("{n} messages"), total, "")))
                            })
                        }
                    }
                    grpc.client_streaming(Sum(reply_desc), req).await
                }
                "Chat" => {
                    struct Chat(MessageDescriptor);
                    impl tonic::server::StreamingService<DynamicMessage> for Chat {
                        type Response = DynamicMessage;
                        type ResponseStream = ReplyStream;
                        type Future = BoxFuture<Response<Self::ResponseStream>, Status>;
                        fn call(&mut self, request: Request<Streaming<DynamicMessage>>) -> Self::Future {
                            let desc = self.0.clone();
                            Box::pin(async move {
                                let stream: ReplyStream = Box::pin(
                                    request.into_inner().map(move |item| {
                                        item.map(|msg| {
                                            let (message, count) = req_fields(&msg);
                                            reply(&desc, format!("re: {message}"), count, "")
                                        })
                                    }),
                                );
                                Ok(Response::new(stream))
                            })
                        }
                    }
                    grpc.streaming(Chat(reply_desc), req).await
                }
                _ => unreachable!("method filtered above"),
            };
            Ok(response)
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
fn compiles_protos_and_lists_methods_with_streaming_flags() {
    let methods = list_methods(&pool());

    assert_eq!(methods.len(), 4);
    let by_path = |p: &str| methods.iter().find(|m| m.path == p).expect(p);
    assert!(by_path("test.Echo/Say").is_unary);
    let watch = by_path("test.Echo/Watch");
    assert!(watch.server_streaming && !watch.client_streaming);
    let sum = by_path("test.Echo/Sum");
    assert!(sum.client_streaming && !sum.server_streaming);
    let chat = by_path("test.Echo/Chat");
    assert!(chat.client_streaming && chat.server_streaming);
}

#[tokio::test]
async fn unary_call_roundtrips_json_and_metadata() {
    let endpoint = spawn_echo_server().await;

    let response = call(
        &endpoint,
        &pool(),
        "test.Echo/Say",
        r#"{"message": "hallo", "count": 41}"#,
        &[("x-caller".to_string(), "forge-test".to_string())],
    )
    .await
    .expect("call should succeed");

    assert_eq!(response.messages.len(), 1);
    let json: serde_json::Value = serde_json::from_str(&response.messages[0]).expect("valid JSON");
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
async fn server_streaming_collects_every_message() {
    let endpoint = spawn_echo_server().await;

    let response = call(
        &endpoint,
        &pool(),
        "test.Echo/Watch",
        r#"{"message": "tick", "count": 3}"#,
        &[],
    )
    .await
    .expect("call should succeed");

    assert_eq!(response.messages.len(), 3, "{:?}", response.messages);
    let last: serde_json::Value = serde_json::from_str(&response.messages[2]).expect("valid JSON");
    assert_eq!(last["message"], "tick #2");
}

#[tokio::test]
async fn client_streaming_sends_a_json_array_as_message_stream() {
    let endpoint = spawn_echo_server().await;

    let response = call(
        &endpoint,
        &pool(),
        "test.Echo/Sum",
        r#"[{"count": 10}, {"count": 30}, {"count": 2}]"#,
        &[],
    )
    .await
    .expect("call should succeed");

    assert_eq!(response.messages.len(), 1);
    let json: serde_json::Value = serde_json::from_str(&response.messages[0]).expect("valid JSON");
    assert_eq!(json["count"], 42);
    assert_eq!(json["message"], "3 messages");
}

#[tokio::test]
async fn bidi_streaming_echoes_each_message() {
    let endpoint = spawn_echo_server().await;

    let response = call(
        &endpoint,
        &pool(),
        "test.Echo/Chat",
        r#"[{"message": "a"}, {"message": "b"}]"#,
        &[],
    )
    .await
    .expect("call should succeed");

    assert_eq!(response.messages.len(), 2, "{:?}", response.messages);
    let first: serde_json::Value = serde_json::from_str(&response.messages[0]).expect("valid JSON");
    assert_eq!(first["message"], "re: a");
    let second: serde_json::Value = serde_json::from_str(&response.messages[1]).expect("valid JSON");
    assert_eq!(second["message"], "re: b");
}

#[tokio::test]
async fn unary_rejects_a_message_array() {
    let err = call("http://127.0.0.1:1", &pool(), "test.Echo/Say", r#"[{}, {}]"#, &[])
        .await
        .expect_err("unary with two messages must be refused before connecting");
    assert!(matches!(err, GrpcError::InputShape(_)), "{err:?}");
}

#[tokio::test]
async fn unknown_method_and_bad_json_give_clear_errors() {
    let err = call("http://127.0.0.1:1", &pool(), "test.Echo/Nope", "{}", &[])
        .await
        .expect_err("unknown method");
    assert!(matches!(err, GrpcError::MethodNotFound(_)), "{err:?}");

    let err = call("http://127.0.0.1:1", &pool(), "test.Echo/Say", "{not json", &[])
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
    let err = call(
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
