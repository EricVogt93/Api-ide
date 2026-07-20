//! gRPC support: compile `.proto` files at runtime (no codegen) and make
//! unary calls with dynamically-typed messages.
//!
//! `protox` compiles the schema to descriptors, `prost-reflect` provides
//! `DynamicMessage` (JSON in, JSON out via its serde support) and `tonic`
//! carries the call. Only unary methods are supported; streaming methods
//! are listed but refused with a clear error.

use std::path::{Path, PathBuf};
use std::str::FromStr;

use prost_reflect::{DescriptorPool, DynamicMessage, MessageDescriptor, MethodDescriptor};
use tonic::codec::{Codec, DecodeBuf, Decoder, EncodeBuf, Encoder};
use tonic::metadata::{MetadataKey, MetadataValue};
use tonic::transport::{Channel, ClientTlsConfig};
use tonic::{Request, Status};

#[derive(Debug, thiserror::Error)]
pub enum GrpcError {
    #[error("failed to compile proto: {0}")]
    Compile(String),
    #[error("method not found: {0} (expected package.Service/Method)")]
    MethodNotFound(String),
    #[error("{0}")]
    InputShape(String),
    #[error("invalid request JSON for {type_name}: {message}")]
    RequestJson { type_name: String, message: String },
    #[error("invalid metadata {key:?}: {message}")]
    Metadata { key: String, message: String },
    #[error("invalid endpoint {0}: {1}")]
    Endpoint(String, String),
    #[error("connect failed: {0}")]
    Connect(String),
    #[error("call failed: {code}: {message}")]
    Call { code: String, message: String },
}

/// One callable method discovered in a compiled descriptor pool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrpcMethod {
    /// Full path as used on the wire, e.g. `pkg.Greeter/SayHello`.
    pub path: String,
    pub input_type: String,
    pub output_type: String,
    pub is_unary: bool,
    pub client_streaming: bool,
    pub server_streaming: bool,
}

/// Result of a successful call, any shape.
#[derive(Debug, Clone)]
pub struct GrpcResponse {
    /// Response message(s) rendered as pretty JSON — exactly one for unary
    /// and client-streaming methods, zero or more for streaming responses.
    pub messages: Vec<String>,
    /// Response metadata: headers plus (for streaming responses) trailers.
    pub metadata: Vec<(String, String)>,
}

/// Compile `.proto` files into a descriptor pool. `includes` are the import
/// search paths; when empty, each file's parent directory is used.
pub fn compile_protos(
    files: &[PathBuf],
    includes: &[PathBuf],
) -> Result<DescriptorPool, GrpcError> {
    let mut include_paths: Vec<PathBuf> = includes.to_vec();
    if include_paths.is_empty() {
        for f in files {
            if let Some(parent) = f.parent() {
                if !include_paths.contains(&parent.to_path_buf()) {
                    include_paths.push(parent.to_path_buf());
                }
            }
        }
    }
    let file_names: Vec<&Path> = files.iter().map(PathBuf::as_path).collect();
    let set = protox::compile(&file_names, &include_paths)
        .map_err(|e| GrpcError::Compile(e.to_string()))?;
    DescriptorPool::from_file_descriptor_set(set).map_err(|e| GrpcError::Compile(e.to_string()))
}

/// Every method of every service in the pool, wire-path sorted.
pub fn list_methods(pool: &DescriptorPool) -> Vec<GrpcMethod> {
    let mut out: Vec<GrpcMethod> = pool
        .services()
        .flat_map(|svc| svc.methods().collect::<Vec<_>>())
        .map(|m| GrpcMethod {
            path: format!("{}/{}", m.parent_service().full_name(), m.name()),
            input_type: m.input().full_name().to_string(),
            output_type: m.output().full_name().to_string(),
            is_unary: !m.is_client_streaming() && !m.is_server_streaming(),
            client_streaming: m.is_client_streaming(),
            server_streaming: m.is_server_streaming(),
        })
        .collect();
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

fn find_method(pool: &DescriptorPool, path: &str) -> Result<MethodDescriptor, GrpcError> {
    let (service, method) = path
        .rsplit_once('/')
        .ok_or_else(|| GrpcError::MethodNotFound(path.to_string()))?;
    let svc = pool
        .get_service_by_name(service)
        .ok_or_else(|| GrpcError::MethodNotFound(path.to_string()))?;
    let found = svc.methods().find(|m| m.name() == method);
    found.ok_or_else(|| GrpcError::MethodNotFound(path.to_string()))
}

/// Call a method of any shape. `endpoint` is `http://host:port` (plaintext
/// HTTP/2) or `https://…` (TLS via the system trust store); `method_path`
/// is `package.Service/Method`; `request_json` is the request message as
/// JSON — for client-streaming/bidi methods a JSON array sends one message
/// per element; `metadata` entries become ASCII request metadata. Streaming
/// responses are collected until the server ends the stream.
pub async fn call(
    endpoint: &str,
    pool: &DescriptorPool,
    method_path: &str,
    request_json: &str,
    metadata: &[(String, String)],
) -> Result<GrpcResponse, GrpcError> {
    let method = find_method(pool, method_path)?;
    let inputs = parse_inputs(request_json, &method)?;

    // Validate metadata before connecting, so a typo'd key fails fast.
    let mut request_metadata = tonic::metadata::MetadataMap::new();
    for (k, v) in metadata {
        let key = MetadataKey::from_str(k).map_err(|e| GrpcError::Metadata {
            key: k.clone(),
            message: e.to_string(),
        })?;
        let value = MetadataValue::from_str(v).map_err(|e| GrpcError::Metadata {
            key: k.clone(),
            message: e.to_string(),
        })?;
        request_metadata.insert(key, value);
    }

    let mut endpoint_builder = Channel::from_shared(endpoint.to_string())
        .map_err(|e| GrpcError::Endpoint(endpoint.to_string(), e.to_string()))?;
    if endpoint.starts_with("https://") {
        endpoint_builder = endpoint_builder
            .tls_config(ClientTlsConfig::new().with_native_roots())
            .map_err(|e| GrpcError::Endpoint(endpoint.to_string(), e.to_string()))?;
    }
    let channel = endpoint_builder
        .connect()
        .await
        .map_err(|e| GrpcError::Connect(e.to_string()))?;

    let path = http::uri::PathAndQuery::from_str(&format!("/{method_path}"))
        .map_err(|e| GrpcError::MethodNotFound(format!("{method_path}: {e}")))?;
    let codec = DynamicCodec::new(method.output());

    let mut grpc = tonic::client::Grpc::new(channel);
    grpc.ready()
        .await
        .map_err(|e| GrpcError::Connect(e.to_string()))?;

    let status_err = |status: Status| GrpcError::Call {
        code: format!("{:?}", status.code()),
        message: status.message().to_string(),
    };

    let mut messages = Vec::new();
    let mut response_metadata: Vec<(String, String)>;
    match (method.is_client_streaming(), method.is_server_streaming()) {
        (false, false) => {
            let mut request = Request::new(into_single(inputs));
            *request.metadata_mut() = request_metadata;
            let response = grpc.unary(request, path, codec).await.map_err(status_err)?;
            response_metadata = ascii_metadata(response.metadata());
            messages.push(message_to_json(response.get_ref())?);
        }
        (true, false) => {
            let mut request = Request::new(tonic::codegen::tokio_stream::iter(inputs));
            *request.metadata_mut() = request_metadata;
            let response = grpc
                .client_streaming(request, path, codec)
                .await
                .map_err(status_err)?;
            response_metadata = ascii_metadata(response.metadata());
            messages.push(message_to_json(response.get_ref())?);
        }
        (false, true) => {
            let mut request = Request::new(into_single(inputs));
            *request.metadata_mut() = request_metadata;
            let response = grpc
                .server_streaming(request, path, codec)
                .await
                .map_err(status_err)?;
            response_metadata = ascii_metadata(response.metadata());
            drain_stream(response.into_inner(), &mut messages, &mut response_metadata).await?;
        }
        (true, true) => {
            let mut request = Request::new(tonic::codegen::tokio_stream::iter(inputs));
            *request.metadata_mut() = request_metadata;
            let response = grpc
                .streaming(request, path, codec)
                .await
                .map_err(status_err)?;
            response_metadata = ascii_metadata(response.metadata());
            drain_stream(response.into_inner(), &mut messages, &mut response_metadata).await?;
        }
    }

    Ok(GrpcResponse {
        messages,
        metadata: response_metadata,
    })
}

/// Parse the request JSON into input messages: a single object for
/// unary/server-streaming, an array (or a single object meaning one
/// message) for client-streaming/bidi.
fn parse_inputs(
    request_json: &str,
    method: &MethodDescriptor,
) -> Result<Vec<DynamicMessage>, GrpcError> {
    let type_name = method.input().full_name().to_string();
    let json_err = |e: &dyn std::fmt::Display| GrpcError::RequestJson {
        type_name: type_name.clone(),
        message: e.to_string(),
    };

    let value: serde_json::Value = serde_json::from_str(request_json).map_err(|e| json_err(&e))?;
    let raw_messages: Vec<serde_json::Value> = match value {
        serde_json::Value::Array(items) => items,
        other => vec![other],
    };

    if !method.is_client_streaming() && raw_messages.len() != 1 {
        return Err(GrpcError::InputShape(format!(
            "{} takes exactly one request message, got {}",
            method.full_name(),
            raw_messages.len()
        )));
    }

    raw_messages
        .into_iter()
        .map(|v| DynamicMessage::deserialize(method.input(), v).map_err(|e| json_err(&e)))
        .collect()
}

fn into_single(mut inputs: Vec<DynamicMessage>) -> DynamicMessage {
    // parse_inputs guarantees exactly one element for non-client-streaming.
    inputs.remove(0)
}

fn ascii_metadata(map: &tonic::metadata::MetadataMap) -> Vec<(String, String)> {
    map.iter()
        .filter_map(|kv| match kv {
            tonic::metadata::KeyAndValueRef::Ascii(k, v) => {
                Some((k.to_string(), v.to_str().unwrap_or_default().to_string()))
            }
            tonic::metadata::KeyAndValueRef::Binary(..) => None,
        })
        .collect()
}

async fn drain_stream(
    mut stream: tonic::Streaming<DynamicMessage>,
    messages: &mut Vec<String>,
    metadata: &mut Vec<(String, String)>,
) -> Result<(), GrpcError> {
    let status_err = |status: Status| GrpcError::Call {
        code: format!("{:?}", status.code()),
        message: status.message().to_string(),
    };
    while let Some(message) = stream.message().await.map_err(status_err)? {
        messages.push(message_to_json(&message)?);
    }
    if let Ok(Some(trailers)) = stream.trailers().await {
        metadata.extend(ascii_metadata(&trailers));
    }
    Ok(())
}

fn message_to_json(message: &DynamicMessage) -> Result<String, GrpcError> {
    let mut json = Vec::new();
    let mut serializer = serde_json::Serializer::pretty(&mut json);
    message
        .serialize_with_options(
            &mut serializer,
            &prost_reflect::SerializeOptions::new().skip_default_fields(false),
        )
        .map_err(|e| GrpcError::Call {
            code: "Internal".to_string(),
            message: e.to_string(),
        })?;
    Ok(String::from_utf8_lossy(&json).into_owned())
}

// ---------------------------------------------------------------------
// Dynamic codec
// ---------------------------------------------------------------------

/// A tonic codec for [`DynamicMessage`]s: encoding uses prost's `Message`
/// impl directly; decoding needs the output descriptor carried alongside.
#[derive(Clone)]
pub struct DynamicCodec {
    output: MessageDescriptor,
}

impl DynamicCodec {
    pub fn new(output: MessageDescriptor) -> Self {
        Self { output }
    }
}

impl Codec for DynamicCodec {
    type Encode = DynamicMessage;
    type Decode = DynamicMessage;
    type Encoder = DynamicEncoder;
    type Decoder = DynamicDecoder;

    fn encoder(&mut self) -> Self::Encoder {
        DynamicEncoder
    }

    fn decoder(&mut self) -> Self::Decoder {
        DynamicDecoder {
            output: self.output.clone(),
        }
    }
}

pub struct DynamicEncoder;

impl Encoder for DynamicEncoder {
    type Item = DynamicMessage;
    type Error = Status;

    fn encode(&mut self, item: Self::Item, dst: &mut EncodeBuf<'_>) -> Result<(), Self::Error> {
        prost::Message::encode(&item, dst).map_err(|e| Status::internal(e.to_string()))
    }
}

pub struct DynamicDecoder {
    output: MessageDescriptor,
}

impl Decoder for DynamicDecoder {
    type Item = DynamicMessage;
    type Error = Status;

    fn decode(&mut self, src: &mut DecodeBuf<'_>) -> Result<Option<Self::Item>, Self::Error> {
        let message = DynamicMessage::decode(self.output.clone(), src)
            .map_err(|e| Status::internal(format!("failed to decode response: {e}")))?;
        Ok(Some(message))
    }
}
