//! A Server-Sent Events (SSE) client session built on `reqwest` +
//! `eventsource-stream`.
//!
//! [`subscribe`] issues the GET request and hands back an [`SseSession`]
//! whose `events` channel yields parsed SSE events as they arrive, fed by a
//! background task that consumes the response body stream.

use chrono::{DateTime, Utc};
use eventsource_stream::Eventsource;
use futures::StreamExt;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::ProtocolError;

/// Something that happened on the event stream.
#[derive(Debug, Clone)]
pub enum SseEvent {
    /// The connection was accepted and the response is streaming.
    Open,
    Event {
        id: String,
        event: String,
        data: String,
        at: DateTime<Utc>,
    },
    Error(String),
    Closed,
}

/// A live SSE subscription. Drop it (or call [`close`](Self::close)) to
/// stop the background task consuming the stream.
pub struct SseSession {
    pub events: mpsc::UnboundedReceiver<SseEvent>,
    cancel: CancellationToken,
}

impl SseSession {
    /// Stops the background task consuming the stream. No explicit
    /// "unsubscribe" is sent — the underlying HTTP connection is simply
    /// dropped.
    pub fn close(&self) {
        self.cancel.cancel();
    }
}

impl Drop for SseSession {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// Issues a `GET` request to `url` (with `Accept: text/event-stream`) and
/// streams the response as SSE events.
pub async fn subscribe(
    url: &str,
    headers: &[(String, String)],
    tls: &super::TlsMaterial,
) -> Result<SseSession, ProtocolError> {
    let mut builder = reqwest::Client::builder();
    if let Some(pem) = &tls.client_pem {
        let identity = reqwest::Identity::from_pem(pem)
            .map_err(|e| ProtocolError::Connect(format!("invalid client certificate/key PEM: {e}")))?;
        builder = builder.identity(identity);
    }
    if let Some(pem) = &tls.extra_roots_pem {
        let certs = reqwest::Certificate::from_pem_bundle(pem)
            .map_err(|e| ProtocolError::Connect(format!("invalid CA bundle PEM: {e}")))?;
        for cert in certs {
            builder = builder.add_root_certificate(cert);
        }
    }
    let client = builder
        .build()
        .map_err(|e| ProtocolError::Connect(format!("failed to build HTTP client: {e}")))?;
    let mut request = client
        .get(url)
        .header(reqwest::header::ACCEPT, "text/event-stream");
    for (name, value) in headers {
        request = request.header(name, value);
    }

    let response = request
        .send()
        .await
        .map_err(|e| ProtocolError::Connect(e.to_string()))?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(ProtocolError::Http(format!("HTTP {status}: {body}")));
    }

    let (events_tx, events_rx) = mpsc::unbounded_channel::<SseEvent>();
    let cancel = CancellationToken::new();

    let _ = events_tx.send(SseEvent::Open);

    let mut stream = response.bytes_stream().eventsource();
    let task_cancel = cancel.clone();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = task_cancel.cancelled() => break,
                item = stream.next() => {
                    match item {
                        Some(Ok(event)) => {
                            let _ = events_tx.send(SseEvent::Event {
                                id: event.id,
                                event: event.event,
                                data: event.data,
                                at: Utc::now(),
                            });
                        }
                        Some(Err(e)) => {
                            let _ = events_tx.send(SseEvent::Error(e.to_string()));
                            break;
                        }
                        None => break,
                    }
                }
            }
        }
        let _ = events_tx.send(SseEvent::Closed);
    });

    Ok(SseSession {
        events: events_rx,
        cancel,
    })
}
