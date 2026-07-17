//! Blocking HTTP for `pm.sendRequest`: scripts run synchronously inside
//! QuickJS (often on a tokio worker thread), so the actual request is done
//! by a dedicated background thread owning its own single-threaded tokio
//! runtime and one shared `reqwest::Client`. The script thread just parks
//! on a channel until the reply arrives.

use std::sync::mpsc;
use std::sync::OnceLock;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Per-request ceiling — generous enough for slow APIs, small enough that
/// a hung endpoint doesn't wedge a test run forever.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Deserialize)]
pub(super) struct SendSpec {
    pub url: String,
    #[serde(default)]
    pub method: Option<String>,
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    #[serde(default)]
    pub body: Option<String>,
}

#[derive(Debug, Serialize, Default)]
pub(super) struct SendOutcome {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub code: u16,
    pub status: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
    pub time_ms: u64,
}

struct Job {
    spec: SendSpec,
    reply: mpsc::Sender<SendOutcome>,
}

fn worker() -> &'static mpsc::Sender<Job> {
    static WORKER: OnceLock<mpsc::Sender<Job>> = OnceLock::new();
    WORKER.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("forge-pm-send-request".to_string())
            .spawn(move || {
                let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build()
                else {
                    // Without a runtime every job just gets an error reply.
                    while let Ok(job) = rx.recv() {
                        let _ = job.reply.send(SendOutcome {
                            error: Some("pm.sendRequest: no async runtime available".to_string()),
                            ..SendOutcome::default()
                        });
                    }
                    return;
                };
                let client = reqwest::Client::builder()
                    .timeout(REQUEST_TIMEOUT)
                    .build()
                    .unwrap_or_default();
                while let Ok(job) = rx.recv() {
                    let outcome = rt.block_on(perform(&client, job.spec));
                    let _ = job.reply.send(outcome);
                }
            })
            .expect("failed to spawn pm.sendRequest worker thread");
        tx
    })
}

async fn perform(client: &reqwest::Client, spec: SendSpec) -> SendOutcome {
    let method = spec.method.as_deref().unwrap_or("GET").to_ascii_uppercase();
    let Ok(method) = reqwest::Method::from_bytes(method.as_bytes()) else {
        return SendOutcome { error: Some(format!("invalid method {method:?}")), ..Default::default() };
    };

    let started = std::time::Instant::now();
    let mut builder = client.request(method, &spec.url);
    for (k, v) in &spec.headers {
        builder = builder.header(k, v);
    }
    if let Some(body) = spec.body {
        if !body.is_empty() {
            builder = builder.body(body);
        }
    }

    match builder.send().await {
        Ok(response) => {
            let code = response.status().as_u16();
            let status = response.status().canonical_reason().unwrap_or("").to_string();
            let headers = response
                .headers()
                .iter()
                .map(|(k, v)| (k.to_string(), String::from_utf8_lossy(v.as_bytes()).into_owned()))
                .collect();
            let body = match response.text().await {
                Ok(t) => t,
                Err(e) => {
                    return SendOutcome {
                        error: Some(format!("failed to read response body: {e}")),
                        ..Default::default()
                    }
                }
            };
            SendOutcome {
                error: None,
                code,
                status,
                headers,
                body,
                time_ms: started.elapsed().as_millis() as u64,
            }
        }
        Err(e) => SendOutcome { error: Some(e.to_string()), ..Default::default() },
    }
}

/// Execute `spec_json` (a [`SendSpec`]) and return a [`SendOutcome`] as
/// JSON. Never panics and never returns invalid JSON — transport and setup
/// failures land in the `error` field.
pub(super) fn send_blocking(spec_json: &str) -> String {
    let outcome = match serde_json::from_str::<SendSpec>(spec_json) {
        Ok(spec) => {
            let (reply_tx, reply_rx) = mpsc::channel();
            match worker().send(Job { spec, reply: reply_tx }) {
                Ok(()) => reply_rx.recv().unwrap_or_else(|_| SendOutcome {
                    error: Some("pm.sendRequest: worker thread died".to_string()),
                    ..Default::default()
                }),
                Err(_) => SendOutcome {
                    error: Some("pm.sendRequest: worker thread unavailable".to_string()),
                    ..Default::default()
                },
            }
        }
        Err(e) => SendOutcome { error: Some(format!("invalid request: {e}")), ..Default::default() },
    };
    serde_json::to_string(&outcome)
        .unwrap_or_else(|_| r#"{"error":"failed to serialize response"}"#.to_string())
}
