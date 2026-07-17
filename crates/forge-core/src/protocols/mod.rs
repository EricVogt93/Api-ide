//! Non-REST protocol sessions: GraphQL introspection, gRPC, WebSocket and
//! SSE.

pub mod graphql;
pub mod grpc;
pub mod sse;
pub mod websocket;

pub use graphql::{GqlField, GqlType, GraphQlSchema, INTROSPECTION_QUERY};
pub use grpc::{call_unary, compile_protos, list_methods, GrpcError, GrpcMethod, GrpcResponse};
pub use sse::{SseEvent, SseSession};
pub use websocket::{WsEvent, WsOutgoing, WsSession};

/// Errors shared across the non-REST protocol adapters.
#[derive(Debug, Clone, thiserror::Error)]
pub enum ProtocolError {
    #[error("connection failed: {0}")]
    Connect(String),
    #[error("{0}")]
    Http(String),
    #[error("failed to parse response: {0}")]
    Parse(String),
    #[error("connection closed")]
    Closed,
}
