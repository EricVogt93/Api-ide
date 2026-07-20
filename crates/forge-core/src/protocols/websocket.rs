//! A WebSocket client session backed by `tokio-tungstenite`.
//!
//! [`connect`] performs the handshake (with optional extra request
//! headers) and hands back a [`WsSession`]: an outgoing message channel the
//! caller writes to, and an event channel the caller reads from. A single
//! background task owns the socket and multiplexes both directions (via
//! `tokio::select!`, not a sink/stream split — splitting would let the
//! read and write halves race over the shared connection state, which can
//! swallow the peer's close frame during a close handshake). The task
//! stops when the session is [`close`](WsSession::close)d, dropped, or the
//! socket goes away on its own.

use chrono::{DateTime, Utc};
use futures::{SinkExt, StreamExt};
use http::{HeaderName, HeaderValue};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::{Bytes, Message};
use tokio_util::sync::CancellationToken;

use super::ProtocolError;

/// A message the caller wants to send over the socket.
#[derive(Debug, Clone)]
pub enum WsOutgoing {
    Text(String),
    Binary(Vec<u8>),
    Ping,
    Close,
}

/// Something that happened on the socket.
#[derive(Debug, Clone)]
pub enum WsEvent {
    /// The handshake completed; the socket is open.
    Connected,
    Text {
        text: String,
        at: DateTime<Utc>,
    },
    Binary {
        data: Vec<u8>,
        at: DateTime<Utc>,
    },
    /// A pong was received (in reply to our ping, or unsolicited).
    Pong,
    Closed {
        code: Option<u16>,
        reason: String,
    },
    Error(String),
}

/// A live WebSocket connection. Drop it (or call [`close`](Self::close)) to
/// tear down the background task driving the socket.
pub struct WsSession {
    pub outgoing: mpsc::UnboundedSender<WsOutgoing>,
    pub events: mpsc::UnboundedReceiver<WsEvent>,
    cancel: CancellationToken,
}

impl WsSession {
    /// Requests a clean close: a close frame is sent to the peer. The
    /// background task keeps running so it can still observe the peer's
    /// own close frame (emitted as [`WsEvent::Closed`]) before it stops.
    pub fn close(&self) {
        let _ = self.outgoing.send(WsOutgoing::Close);
    }
}

impl Drop for WsSession {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// Connects to `url`, sending `headers` as additional request headers
/// during the handshake.
pub async fn connect(
    url: &str,
    headers: &[(String, String)],
    tls: &super::TlsMaterial,
) -> Result<WsSession, ProtocolError> {
    let mut request = url
        .into_client_request()
        .map_err(|e| ProtocolError::Connect(e.to_string()))?;

    for (name, value) in headers {
        let header_name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|e| ProtocolError::Connect(format!("invalid header name {name:?}: {e}")))?;
        let header_value = HeaderValue::from_str(value).map_err(|e| {
            ProtocolError::Connect(format!("invalid header value for {name:?}: {e}"))
        })?;
        request.headers_mut().insert(header_name, header_value);
    }

    // Only build a custom TLS connector when the workspace configured
    // client-cert / extra-CA material; otherwise keep the default stack.
    let connector = if tls.is_empty() {
        None
    } else {
        Some(tokio_tungstenite::Connector::Rustls(std::sync::Arc::new(
            super::rustls_client_config(tls)?,
        )))
    };

    let (mut ws_stream, _response) =
        tokio_tungstenite::connect_async_tls_with_config(request, None, false, connector)
            .await
            .map_err(|e| ProtocolError::Connect(e.to_string()))?;

    let (outgoing_tx, mut outgoing_rx) = mpsc::unbounded_channel::<WsOutgoing>();
    let (events_tx, events_rx) = mpsc::unbounded_channel::<WsEvent>();
    let cancel = CancellationToken::new();

    let _ = events_tx.send(WsEvent::Connected);

    let task_cancel = cancel.clone();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = task_cancel.cancelled() => {
                    let _ = ws_stream.send(Message::Close(None)).await;
                    break;
                }
                outgoing = outgoing_rx.recv() => {
                    match outgoing {
                        Some(WsOutgoing::Text(text)) => {
                            let _ = ws_stream.send(Message::text(text)).await;
                        }
                        Some(WsOutgoing::Binary(data)) => {
                            let _ = ws_stream.send(Message::binary(data)).await;
                        }
                        Some(WsOutgoing::Ping) => {
                            let _ = ws_stream.send(Message::Ping(Bytes::new())).await;
                        }
                        Some(WsOutgoing::Close) => {
                            let _ = ws_stream.send(Message::Close(None)).await;
                        }
                        None => {
                            // The session (and its outgoing sender) was
                            // dropped without an explicit close.
                            let _ = ws_stream.send(Message::Close(None)).await;
                            break;
                        }
                    }
                }
                incoming = ws_stream.next() => {
                    match incoming {
                        Some(Ok(Message::Text(text))) => {
                            let _ = events_tx.send(WsEvent::Text {
                                text: text.as_str().to_string(),
                                at: Utc::now(),
                            });
                        }
                        Some(Ok(Message::Binary(data))) => {
                            let _ = events_tx.send(WsEvent::Binary {
                                data: data.to_vec(),
                                at: Utc::now(),
                            });
                        }
                        Some(Ok(Message::Ping(payload))) => {
                            let _ = ws_stream.send(Message::Pong(payload)).await;
                        }
                        Some(Ok(Message::Pong(_))) => {
                            let _ = events_tx.send(WsEvent::Pong);
                        }
                        Some(Ok(Message::Close(frame))) => {
                            let (code, reason) = match frame {
                                Some(frame) => (Some(u16::from(frame.code)), frame.reason.to_string()),
                                None => (None, String::new()),
                            };
                            // Receiving a close frame only *queues* our reply
                            // (or, if we sent ours first, just acknowledges
                            // it) — `Sink::close` is what actually flushes
                            // it to the peer. Sending a plain
                            // `Message::Close` here would fail instead,
                            // since writes are refused once either side has
                            // sent one.
                            let _ = SinkExt::close(&mut ws_stream).await;
                            let _ = events_tx.send(WsEvent::Closed { code, reason });
                            break;
                        }
                        Some(Ok(Message::Frame(_))) => {}
                        Some(Err(e)) => {
                            let _ = events_tx.send(WsEvent::Error(e.to_string()));
                            break;
                        }
                        None => {
                            // Stream ended without a close frame (e.g. the
                            // TCP connection dropped).
                            let _ = events_tx.send(WsEvent::Closed {
                                code: None,
                                reason: String::new(),
                            });
                            break;
                        }
                    }
                }
            }
        }
    });

    Ok(WsSession {
        outgoing: outgoing_tx,
        events: events_rx,
        cancel,
    })
}
