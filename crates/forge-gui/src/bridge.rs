//! The GUI never blocks its render thread on network I/O: all HTTP
//! execution happens on a dedicated background thread that owns a tokio
//! runtime and a single shared [`HttpEngine`] (so cookies persist across
//! runs). Commands flow in via an unbounded tokio channel (cheap to send
//! from sync code); events flow back out via a `std::sync::mpsc` channel
//! that the egui thread drains once per frame, with `Context::request_repaint`
//! called after every event so the UI wakes up promptly even when idle.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use forge_core::exec::{HttpEngine, StoredCookie};
use forge_core::protocols::{sse, websocket, SseEvent, TlsMaterial, WsEvent, WsOutgoing};
use forge_core::runner::{self, CancellationToken, RunEvent, RunOptions, RunScope};
use forge_core::store::Workspace;

pub struct V1RunItem {
    pub label: String,
    pub matrix: forge_core::reqv1::MatrixCase,
    pub result: forge_core::reqv1::RunResult,
    pub response: Option<forge_core::reqv1::ResponseView>,
    pub name: String,
    pub method: String,
    pub url: String,
    pub request_headers: Vec<(String, String)>,
    pub request_body: Option<Vec<u8>>,
}

pub struct V1RunOutput {
    pub items: Vec<V1RunItem>,
}

/// A command sent from the UI thread to the bridge thread.
pub enum Cmd {
    Run {
        run_id: u64,
        workspace: Box<Workspace>,
        scope: RunScope,
        options: RunOptions,
    },
    Cancel {
        run_id: u64,
    },
    /// Open a WebSocket connection, forwarding events back as `Evt::Ws`.
    WsConnect {
        conn_id: u64,
        url: String,
        headers: Vec<(String, String)>,
        tls: TlsMaterial,
    },
    /// Send a text message over an open WebSocket connection.
    WsSend {
        conn_id: u64,
        msg: String,
    },
    /// Request a clean close of a WebSocket connection.
    WsClose {
        conn_id: u64,
    },
    /// Subscribe to an SSE stream, forwarding events back as `Evt::Sse`.
    SseSubscribe {
        conn_id: u64,
        url: String,
        headers: Vec<(String, String)>,
        tls: TlsMaterial,
    },
    /// Stop consuming an SSE stream.
    SseClose {
        conn_id: u64,
    },
    /// Compile .proto files and call a unary gRPC method; the outcome comes
    /// back as `Evt::Grpc`.
    GrpcCall {
        call_id: u64,
        protos: Vec<PathBuf>,
        endpoint: String,
        method: String,
        request_json: String,
        metadata: Vec<(String, String)>,
    },
    /// Run a reqv1 request document (from the v1 editor). `text` is the
    /// current buffer; `root` the project root; `env_name` an environment
    /// under environments/. Outcome comes back as `Evt::V1Run`.
    RunV1 {
        run_id: u64,
        root: PathBuf,
        file: PathBuf,
        text: String,
        env_name: Option<String>,
        mock: bool,
        allow_project_code: bool,
    },
    /// Run saved reqv1 documents in the selected order, threading runtime
    /// extractor output forward.
    RunV1Sequence {
        run_id: u64,
        root: PathBuf,
        files: Vec<PathBuf>,
        env_name: Option<String>,
        mock: bool,
        allow_project_code: bool,
    },
    /// Run saved reqv1 documents independently. Unlike a sequence, runtime
    /// extractor output is not threaded between documents.
    RunV1Batch {
        run_id: u64,
        root: PathBuf,
        files: Vec<PathBuf>,
        env_name: Option<String>,
        mock: bool,
        allow_project_code: bool,
    },
    /// Preview one catalog asset against the current request IR and optional
    /// response from the last run. This never sends an HTTP request.
    PreviewV1Asset {
        preview_id: u64,
        root: PathBuf,
        file: PathBuf,
        text: String,
        env_name: Option<String>,
        phase: forge_core::reqv1::model::PipelinePhase,
        uses: String,
        with: serde_json::Map<String, serde_json::Value>,
        response: Option<forge_core::reqv1::ResponseView>,
        allow_project_code: bool,
    },
    /// Ask for a snapshot of the shared `HttpEngine`'s cookie jar.
    ListCookies,
    /// Remove one cookie from the shared jar.
    RemoveCookie {
        domain: String,
        name: String,
    },
    /// Clear the whole shared cookie jar.
    ClearCookies,
    /// Load persisted cookies from `path` into the shared jar (best-effort;
    /// a missing or unreadable file is a no-op) and remember `path` so the
    /// jar is saved back there after every run and on shutdown.
    LoadCookies {
        path: PathBuf,
    },
    /// Fetch an OpenAPI spec for editor assistance: an `http(s)` URL is
    /// downloaded, anything else is read as a file under `root`. Replies
    /// with `Evt::OpenApi` carrying the raw spec text.
    FetchOpenApi {
        root: PathBuf,
        source: String,
    },
    /// Ask the configured OpenAI-compatible advisor without blocking egui.
    AskAdvisor {
        advisor_id: u64,
        root: PathBuf,
        config: crate::advisor::AdvisorConfig,
        question: String,
        context: String,
    },
    CheckForUpdates {
        manual: bool,
    },
    DownloadUpdate {
        release: crate::updater::UpdateRelease,
    },
    Shutdown,
}

/// An event sent from the bridge thread back to the UI thread.
pub enum Evt {
    Run {
        run_id: u64,
        event: RunEvent,
    },
    /// The run could not even start (bad scope, missing environment, ...).
    RunFailed {
        run_id: u64,
        error: String,
    },
    /// Something happened on a WebSocket connection.
    Ws {
        conn_id: u64,
        event: WsEvent,
    },
    /// Something happened on an SSE subscription.
    Sse {
        conn_id: u64,
        event: SseEvent,
    },
    /// A fresh snapshot of the shared cookie jar (in reply to
    /// `Cmd::ListCookies`, or after any mutating cookie command).
    Cookies(Vec<StoredCookie>),
    /// Outcome of a `Cmd::GrpcCall`: response JSON + metadata, or an error.
    Grpc {
        call_id: u64,
        result: Result<forge_core::protocols::GrpcResponse, String>,
    },
    /// Outcome of a `Cmd::RunV1`: the run result, or a parse/setup error.
    V1Run {
        run_id: u64,
        result: Result<V1RunOutput, String>,
    },
    /// Outcome of a `Cmd::PreviewV1Asset`.
    V1Preview {
        preview_id: u64,
        result: Result<forge_core::reqv1::runner::CatalogPreview, String>,
    },
    /// Raw OpenAPI spec text fetched by `Cmd::FetchOpenApi` (or the fetch
    /// error), together with the source it was loaded from.
    OpenApi {
        source: String,
        result: Result<String, String>,
    },
    Advisor {
        advisor_id: u64,
        result: Result<String, String>,
    },
    UpdateChecked {
        manual: bool,
        result: Result<Option<crate::updater::UpdateRelease>, String>,
    },
    UpdateDownloaded(Result<crate::updater::DownloadedUpdate, String>),
}

/// Handle to the background bridge thread.
pub struct Bridge {
    cmd_tx: tokio::sync::mpsc::UnboundedSender<Cmd>,
    evt_rx: std::sync::mpsc::Receiver<Evt>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Bridge {
    /// Spawn the background thread. `ctx` is cloned and used to request a
    /// repaint after every event is pushed, so the UI updates promptly even
    /// if it would otherwise be idle-waiting.
    pub fn new(ctx: egui::Context) -> Self {
        let (cmd_tx, cmd_rx) = tokio::sync::mpsc::unbounded_channel::<Cmd>();
        let (evt_tx, evt_rx) = std::sync::mpsc::channel::<Evt>();

        let handle = std::thread::Builder::new()
            .name("forge-bridge".to_string())
            .spawn(move || bridge_main(cmd_rx, evt_tx, ctx))
            .expect("failed to spawn forge-bridge thread");

        Self {
            cmd_tx,
            evt_rx,
            handle: Some(handle),
        }
    }

    /// Send a command to the bridge thread.
    pub fn send(&self, cmd: Cmd) -> Result<(), String> {
        self.cmd_tx
            .send(cmd)
            .map_err(|_| "background worker is unavailable".to_string())
    }

    /// Drain one pending event, if any. Call this in a loop each frame.
    pub fn try_recv(&self) -> Option<Evt> {
        self.evt_rx.try_recv().ok()
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        let _ = self.cmd_tx.send(Cmd::Shutdown);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn bridge_main(
    mut cmd_rx: tokio::sync::mpsc::UnboundedReceiver<Cmd>,
    evt_tx: std::sync::mpsc::Sender<Evt>,
    ctx: egui::Context,
) {
    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            // Nothing sensible to do without a runtime; report and bail so
            // the UI thread at least sees every run fail loudly instead of
            // hanging forever waiting for events that will never come.
            let _ = evt_tx.send(Evt::RunFailed {
                run_id: 0,
                error: format!("failed to start async runtime: {e}"),
            });
            return;
        }
    };

    rt.block_on(async move {
        let engine = Arc::new(HttpEngine::new());
        let v1_auth = Arc::new(forge_core::reqv1::AuthSession::default());
        let cancels: Arc<Mutex<HashMap<u64, CancellationToken>>> =
            Arc::new(Mutex::new(HashMap::new()));
        // Outgoing-message senders for live WebSocket connections, keyed by
        // `conn_id` — the connection's own background task owns the
        // `WsSession`; this is just how `Cmd::WsSend`/`WsClose` reach it.
        let ws_conns: Arc<Mutex<HashMap<u64, tokio::sync::mpsc::UnboundedSender<WsOutgoing>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        // Cancellation tokens for live SSE subscriptions, keyed by `conn_id`.
        let sse_cancels: Arc<Mutex<HashMap<u64, CancellationToken>>> =
            Arc::new(Mutex::new(HashMap::new()));
        // Where to persist the cookie jar (set by `Cmd::LoadCookies`), saved
        // back after every run and on shutdown.
        let cookie_path: Arc<Mutex<Option<PathBuf>>> = Arc::new(Mutex::new(None));

        while let Some(cmd) = cmd_rx.recv().await {
            match cmd {
                Cmd::Run {
                    run_id,
                    workspace,
                    scope,
                    options,
                } => {
                    let engine = engine.clone();
                    let evt_tx = evt_tx.clone();
                    let ctx = ctx.clone();
                    let cancels = cancels.clone();
                    let cookie_path = cookie_path.clone();
                    let cancel = CancellationToken::new();
                    cancels
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .insert(run_id, cancel.clone());

                    tokio::spawn(async move {
                        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<RunEvent>();

                        let forward_ctx = ctx.clone();
                        let forward_tx = evt_tx.clone();
                        let forward = tokio::spawn(async move {
                            while let Some(event) = rx.recv().await {
                                let _ = forward_tx.send(Evt::Run { run_id, event });
                                forward_ctx.request_repaint();
                            }
                        });

                        let result =
                            runner::run(&workspace, scope, options, &engine, tx, cancel).await;
                        if let Err(e) = result {
                            let _ = evt_tx.send(Evt::RunFailed {
                                run_id,
                                error: e.to_string(),
                            });
                            ctx.request_repaint();
                        }
                        let _ = forward.await;

                        cancels
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .remove(&run_id);
                        save_cookies(&engine, &cookie_path);
                    });
                }
                Cmd::Cancel { run_id } => {
                    if let Some(token) = cancels
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .get(&run_id)
                    {
                        token.cancel();
                    }
                }
                Cmd::GrpcCall {
                    call_id,
                    protos,
                    endpoint,
                    method,
                    request_json,
                    metadata,
                } => {
                    let evt_tx = evt_tx.clone();
                    let ctx = ctx.clone();
                    tokio::spawn(async move {
                        let result = async {
                            let pool = forge_core::protocols::compile_protos(&protos, &[])
                                .map_err(|e| e.to_string())?;
                            forge_core::protocols::call(
                                &endpoint,
                                &pool,
                                &method,
                                &request_json,
                                &metadata,
                            )
                            .await
                            .map_err(|e| e.to_string())
                        }
                        .await;
                        let _ = evt_tx.send(Evt::Grpc { call_id, result });
                        ctx.request_repaint();
                    });
                }
                Cmd::RunV1 {
                    run_id,
                    root,
                    file,
                    text,
                    env_name,
                    mock,
                    allow_project_code,
                } => {
                    let engine = engine.clone();
                    let v1_auth = v1_auth.clone();
                    let evt_tx = evt_tx.clone();
                    let ctx = ctx.clone();
                    tokio::spawn(async move {
                        let result = run_v1_document(
                            &engine,
                            V1RunSpec {
                                root: &root,
                                file: &file,
                                text: &text,
                                env_name: env_name.as_deref(),
                                mock,
                                allow_project_code,
                            },
                            &v1_auth,
                        )
                        .await;
                        let _ = evt_tx.send(Evt::V1Run { run_id, result });
                        ctx.request_repaint();
                    });
                }
                Cmd::RunV1Sequence {
                    run_id,
                    root,
                    files,
                    env_name,
                    mock,
                    allow_project_code,
                } => {
                    let engine = engine.clone();
                    let v1_auth = v1_auth.clone();
                    let evt_tx = evt_tx.clone();
                    let ctx = ctx.clone();
                    tokio::spawn(async move {
                        let result = run_v1_sequence(
                            &engine,
                            &root,
                            &files,
                            env_name.as_deref(),
                            mock,
                            allow_project_code,
                            &v1_auth,
                        )
                        .await;
                        let _ = evt_tx.send(Evt::V1Run { run_id, result });
                        ctx.request_repaint();
                    });
                }
                Cmd::RunV1Batch {
                    run_id,
                    root,
                    files,
                    env_name,
                    mock,
                    allow_project_code,
                } => {
                    let engine = engine.clone();
                    let v1_auth = v1_auth.clone();
                    let evt_tx = evt_tx.clone();
                    let ctx = ctx.clone();
                    tokio::spawn(async move {
                        let result = run_v1_batch(
                            &engine,
                            &root,
                            &files,
                            env_name.as_deref(),
                            mock,
                            allow_project_code,
                            &v1_auth,
                        )
                        .await;
                        let _ = evt_tx.send(Evt::V1Run { run_id, result });
                        ctx.request_repaint();
                    });
                }
                Cmd::PreviewV1Asset {
                    preview_id,
                    root,
                    file,
                    text,
                    env_name,
                    phase,
                    uses,
                    with,
                    response,
                    allow_project_code,
                } => {
                    let evt_tx = evt_tx.clone();
                    let ctx = ctx.clone();
                    tokio::spawn(async move {
                        let result = preview_v1_asset(
                            &root,
                            &file,
                            &text,
                            env_name.as_deref(),
                            phase,
                            uses,
                            with,
                            response.as_ref(),
                            allow_project_code,
                        );
                        let _ = evt_tx.send(Evt::V1Preview { preview_id, result });
                        ctx.request_repaint();
                    });
                }
                Cmd::FetchOpenApi { root, source } => {
                    let evt_tx = evt_tx.clone();
                    let ctx = ctx.clone();
                    tokio::spawn(async move {
                        let result =
                            if source.starts_with("http://") || source.starts_with("https://") {
                                async {
                                    let resp =
                                        reqwest::get(&source).await.map_err(|e| e.to_string())?;
                                    let status = resp.status();
                                    if !status.is_success() {
                                        return Err(format!("HTTP {status}"));
                                    }
                                    resp.text().await.map_err(|e| e.to_string())
                                }
                                .await
                            } else {
                                let path = root.join(&source);
                                tokio::fs::read_to_string(&path)
                                    .await
                                    .map_err(|e| format!("{}: {e}", path.display()))
                            };
                        let _ = evt_tx.send(Evt::OpenApi { source, result });
                        ctx.request_repaint();
                    });
                }
                Cmd::AskAdvisor {
                    advisor_id,
                    root,
                    config,
                    question,
                    context,
                } => {
                    let evt_tx = evt_tx.clone();
                    let ctx = ctx.clone();
                    tokio::spawn(async move {
                        let result =
                            match crate::advisor::resolve_api_key(&root, &config.api_key_env) {
                                Ok(api_key) => {
                                    crate::advisor::ask(
                                        &config,
                                        api_key.as_deref(),
                                        &question,
                                        &context,
                                    )
                                    .await
                                }
                                Err(error) => Err(error),
                            };
                        let _ = evt_tx.send(Evt::Advisor { advisor_id, result });
                        ctx.request_repaint();
                    });
                }
                Cmd::CheckForUpdates { manual } => {
                    let evt_tx = evt_tx.clone();
                    let ctx = ctx.clone();
                    tokio::spawn(async move {
                        let result = crate::updater::check_for_update().await;
                        let _ = evt_tx.send(Evt::UpdateChecked { manual, result });
                        ctx.request_repaint();
                    });
                }
                Cmd::DownloadUpdate { release } => {
                    let evt_tx = evt_tx.clone();
                    let ctx = ctx.clone();
                    tokio::spawn(async move {
                        let result = crate::updater::download_update(release).await;
                        let _ = evt_tx.send(Evt::UpdateDownloaded(result));
                        ctx.request_repaint();
                    });
                }
                Cmd::WsConnect {
                    conn_id,
                    url,
                    headers,
                    tls,
                } => {
                    let evt_tx = evt_tx.clone();
                    let ctx = ctx.clone();
                    let ws_conns = ws_conns.clone();
                    tokio::spawn(async move {
                        match websocket::connect(&url, &headers, &tls).await {
                            Ok(mut session) => {
                                ws_conns
                                    .lock()
                                    .unwrap_or_else(|p| p.into_inner())
                                    .insert(conn_id, session.outgoing.clone());
                                while let Some(event) = session.events.recv().await {
                                    let is_closed = matches!(event, WsEvent::Closed { .. });
                                    let _ = evt_tx.send(Evt::Ws { conn_id, event });
                                    ctx.request_repaint();
                                    if is_closed {
                                        break;
                                    }
                                }
                                ws_conns
                                    .lock()
                                    .unwrap_or_else(|p| p.into_inner())
                                    .remove(&conn_id);
                            }
                            Err(e) => {
                                let _ = evt_tx.send(Evt::Ws {
                                    conn_id,
                                    event: WsEvent::Error(e.to_string()),
                                });
                                ctx.request_repaint();
                            }
                        }
                    });
                }
                Cmd::WsSend { conn_id, msg } => {
                    if let Some(tx) = ws_conns
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .get(&conn_id)
                    {
                        let _ = tx.send(WsOutgoing::Text(msg));
                    }
                }
                Cmd::WsClose { conn_id } => {
                    if let Some(tx) = ws_conns
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .get(&conn_id)
                    {
                        let _ = tx.send(WsOutgoing::Close);
                    }
                }
                Cmd::SseSubscribe {
                    conn_id,
                    url,
                    headers,
                    tls,
                } => {
                    let evt_tx = evt_tx.clone();
                    let ctx = ctx.clone();
                    let sse_cancels = sse_cancels.clone();
                    let cancel = CancellationToken::new();
                    sse_cancels
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .insert(conn_id, cancel.clone());
                    tokio::spawn(async move {
                        match sse::subscribe(&url, &headers, &tls).await {
                            Ok(mut session) => loop {
                                tokio::select! {
                                    _ = cancel.cancelled() => break,
                                    item = session.events.recv() => {
                                        match item {
                                            Some(event) => {
                                                let is_closed = matches!(event, SseEvent::Closed);
                                                let _ = evt_tx.send(Evt::Sse { conn_id, event });
                                                ctx.request_repaint();
                                                if is_closed {
                                                    break;
                                                }
                                            }
                                            None => break,
                                        }
                                    }
                                }
                            },
                            Err(e) => {
                                let _ = evt_tx.send(Evt::Sse {
                                    conn_id,
                                    event: SseEvent::Error(e.to_string()),
                                });
                                ctx.request_repaint();
                            }
                        }
                        sse_cancels
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .remove(&conn_id);
                    });
                }
                Cmd::SseClose { conn_id } => {
                    if let Some(token) = sse_cancels
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .remove(&conn_id)
                    {
                        token.cancel();
                    }
                }
                Cmd::ListCookies => {
                    let _ = evt_tx.send(Evt::Cookies(engine.cookies().all()));
                    ctx.request_repaint();
                }
                Cmd::RemoveCookie { domain, name } => {
                    engine.cookies().remove(&domain, &name);
                    let _ = evt_tx.send(Evt::Cookies(engine.cookies().all()));
                    ctx.request_repaint();
                    save_cookies(&engine, &cookie_path);
                }
                Cmd::ClearCookies => {
                    engine.cookies().clear();
                    let _ = evt_tx.send(Evt::Cookies(engine.cookies().all()));
                    ctx.request_repaint();
                    save_cookies(&engine, &cookie_path);
                }
                Cmd::LoadCookies { path } => {
                    // Always clear first, even if the file is missing or
                    // fails to parse: otherwise cookies from a previously
                    // open workspace leak into this one and get persisted
                    // into *its* cookies.json on the next save.
                    engine.cookies().clear();
                    if let Ok(data) = std::fs::read_to_string(&path) {
                        restore_cookies(&engine, &data);
                    }
                    *cookie_path.lock().unwrap_or_else(|p| p.into_inner()) = Some(path);
                    let _ = evt_tx.send(Evt::Cookies(engine.cookies().all()));
                    ctx.request_repaint();
                }
                Cmd::Shutdown => {
                    save_cookies(&engine, &cookie_path);
                    break;
                }
            }
        }
    });
}

/// Run a reqv1 document from the v1 editor: parse `text`, load the named
/// environment, and run over the shared HTTP engine (or serve its mock).
/// Secrets come from `<root>/.env.local` then the process environment.
struct V1RunSpec<'a> {
    root: &'a std::path::Path,
    file: &'a std::path::Path,
    text: &'a str,
    env_name: Option<&'a str>,
    mock: bool,
    allow_project_code: bool,
}

async fn run_v1_document(
    engine: &HttpEngine,
    spec: V1RunSpec<'_>,
    auth: &forge_core::reqv1::AuthSession,
) -> Result<V1RunOutput, String> {
    use forge_core::reqv1::{self, RunMode};

    let doc = reqv1::RequestDocument::parse(spec.text).map_err(|e| e.to_string())?;
    ensure_project_code_allowed(spec.root, &doc, spec.allow_project_code)?;
    let env = reqv1::load_request_environment(spec.root, spec.file, spec.env_name)
        .map_err(|diagnostic| diagnostic.message)?;

    // Secret provider: .env.local (KEY=value) first, then process env.
    let file_secrets = reqv1::load_file_secrets(spec.root);
    let secret = move |name: &str| {
        file_secrets
            .get(name)
            .cloned()
            .or_else(|| std::env::var(name).ok())
    };

    let mode = if spec.mock {
        RunMode::Mock
    } else {
        RunMode::Http
    };
    let items = reqv1::run_matrix_with_responses_in_session(
        &doc,
        spec.root,
        spec.file,
        env,
        &secret,
        engine,
        mode,
        CancellationToken::new(),
        auth,
    )
    .await
    .map_err(|errors| errors.to_string())?
    .into_iter()
    .enumerate()
    .map(|(index, (matrix, result, response))| V1RunItem {
        label: if matrix.is_empty() {
            result.request_id.clone()
        } else {
            format!(
                "case {} · {}",
                index + 1,
                serde_json::Value::Object(matrix.clone())
            )
        },
        matrix,
        result,
        response,
        name: doc.meta.name.clone(),
        method: doc.request.method.as_str().to_string(),
        url: doc.request.url.clone(),
        request_headers: doc
            .request
            .headers
            .iter()
            .filter(|header| header.enabled)
            .map(|header| (header.name.clone(), header.value.clone()))
            .collect(),
        request_body: doc
            .request
            .body
            .as_ref()
            .and_then(|body| serde_json::to_vec(body).ok()),
    })
    .collect();
    Ok(V1RunOutput { items })
}

async fn run_v1_sequence(
    engine: &HttpEngine,
    root: &std::path::Path,
    files: &[PathBuf],
    env_name: Option<&str>,
    mock: bool,
    allow_project_code: bool,
    auth: &forge_core::reqv1::AuthSession,
) -> Result<V1RunOutput, String> {
    use forge_core::reqv1::{self, RunMode};

    if files.is_empty() {
        return Err("select at least one request for the sequence".to_string());
    }
    let mut documents = Vec::with_capacity(files.len());
    for file in files {
        let document = reqv1::load_request_document(file)?;
        ensure_project_code_allowed(root, &document, allow_project_code)?;
        if !document.matrix.is_empty() {
            return Err(format!(
                "{} has a matrix; run its matrix separately before adding it to a sequence",
                file.display()
            ));
        }
        documents.push(document);
    }
    let environments = files
        .iter()
        .map(|file| {
            reqv1::load_request_environment(root, file, env_name)
                .map_err(|diagnostic| diagnostic.message)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let file_secrets = reqv1::load_file_secrets(root);
    let secret = move |name: &str| {
        file_secrets
            .get(name)
            .cloned()
            .or_else(|| std::env::var(name).ok())
    };
    let mode = if mock { RunMode::Mock } else { RunMode::Http };
    let items = reqv1::run_sequence_with_environment_values_in_session(
        files,
        root,
        &environments,
        &secret,
        engine,
        mode,
        CancellationToken::new(),
        auth,
    )
    .await
    .map_err(|diagnostic| diagnostic.message)?
    .into_iter()
    .zip(documents)
    .map(|((result, response), document)| {
        let request_body = document
            .request
            .body
            .as_ref()
            .and_then(|body| serde_json::to_vec(body).ok());
        V1RunItem {
            label: result.request_id.clone(),
            matrix: serde_json::Map::new(),
            result,
            response,
            name: document.meta.name,
            method: document.request.method.as_str().to_string(),
            url: document.request.url,
            request_headers: document
                .request
                .headers
                .into_iter()
                .filter(|header| header.enabled)
                .map(|header| (header.name, header.value))
                .collect(),
            request_body,
        }
    })
    .collect();
    Ok(V1RunOutput { items })
}

async fn run_v1_batch(
    engine: &HttpEngine,
    root: &std::path::Path,
    files: &[PathBuf],
    env_name: Option<&str>,
    mock: bool,
    allow_project_code: bool,
    auth: &forge_core::reqv1::AuthSession,
) -> Result<V1RunOutput, String> {
    if files.is_empty() {
        return Err("no affected requests found".to_string());
    }
    let mut items = Vec::new();
    for file in files {
        let text = std::fs::read_to_string(file)
            .map_err(|error| format!("cannot read {}: {error}", file.display()))?;
        let output = run_v1_document(
            engine,
            V1RunSpec {
                root,
                file,
                text: &text,
                env_name,
                mock,
                allow_project_code,
            },
            auth,
        )
        .await?;
        items.extend(output.items);
    }
    Ok(V1RunOutput { items })
}

fn ensure_project_code_allowed(
    root: &std::path::Path,
    document: &forge_core::reqv1::RequestDocument,
    allowed: bool,
) -> Result<(), String> {
    let auth_uses_project_code = if allowed {
        false
    } else {
        forge_core::reqv1::load_project_auth_document(root)
            .map_err(|diagnostic| diagnostic.message)?
            .is_some_and(|(_, document)| document.uses_project_code())
    };
    if !allowed && (document.uses_project_code() || auth_uses_project_code) {
        Err(
            "project-owned code is disabled; inspect the asset and enable “Allow project code”"
                .to_string(),
        )
    } else {
        Ok(())
    }
}

#[allow(clippy::too_many_arguments)]
fn preview_v1_asset(
    root: &std::path::Path,
    file: &std::path::Path,
    text: &str,
    env_name: Option<&str>,
    phase: forge_core::reqv1::model::PipelinePhase,
    uses: String,
    with: serde_json::Map<String, serde_json::Value>,
    response: Option<&forge_core::reqv1::ResponseView>,
    allow_project_code: bool,
) -> Result<forge_core::reqv1::runner::CatalogPreview, String> {
    use forge_core::reqv1::{self, model::PipelineEntry};

    let mut doc = reqv1::RequestDocument::parse(text).map_err(|error| error.to_string())?;
    doc.pipeline = vec![PipelineEntry {
        phase,
        uses,
        with,
        enabled: true,
    }];
    if !allow_project_code && doc.uses_project_code() {
        return Err(
            "project-owned code is disabled; inspect the asset and enable “Allow project code”"
                .to_string(),
        );
    }
    let env = reqv1::load_request_environment(root, file, env_name)
        .map_err(|diagnostic| diagnostic.message)?;
    let file_secrets = reqv1::load_file_secrets(root);
    let secret = move |name: &str| {
        file_secrets
            .get(name)
            .cloned()
            .or_else(|| std::env::var(name).ok())
    };
    let ir = reqv1::validate(&doc, root, file, env, &secret).map_err(|diagnostics| {
        diagnostics
            .into_iter()
            .map(|diagnostic| format!("[{}] {}", diagnostic.code, diagnostic.message))
            .collect::<Vec<_>>()
            .join("; ")
    })?;
    let entry = ir
        .pipeline
        .first()
        .ok_or_else(|| "preview asset did not resolve".to_string())?;
    reqv1::runner::preview_asset(&ir, entry, response).map_err(|diagnostic| diagnostic.message)
}

/// Persist the shared cookie jar to whichever path `Cmd::LoadCookies` last
/// set, if any. Best-effort — a write failure here shouldn't crash the
/// bridge thread.
fn save_cookies(engine: &HttpEngine, cookie_path: &Mutex<Option<PathBuf>>) {
    let Some(path) = cookie_path
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clone()
    else {
        return;
    };
    if let Some(parent) = path.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
    }
    let _ = std::fs::write(path, engine.cookies().to_json());
}

/// Restore cookies from a JSON array of `StoredCookie` (as produced by
/// `CookieJar::to_json`) into `engine`'s live jar.
///
/// `CookieJar` exposes no bulk-load setter — only `store(&Url,
/// "Set-Cookie"-style str)` — so each entry is replayed as a synthetic
/// `Set-Cookie` header against a URL built from its own domain. This loses
/// host-only-vs-domain-cookie distinction (same caveat `CookieJar::from_json`
/// already documents) but otherwise round-trips faithfully.
fn restore_cookies(engine: &HttpEngine, json: &str) {
    let Ok(cookies) = serde_json::from_str::<Vec<StoredCookie>>(json) else {
        return;
    };
    for c in cookies {
        let Ok(url) = url::Url::parse(&format!("https://{}/", c.domain)) else {
            continue;
        };
        let mut set_cookie = format!(
            "{}={}; Domain={}; Path={}",
            c.name, c.value, c.domain, c.path
        );
        if c.secure {
            set_cookie.push_str("; Secure");
        }
        if c.http_only {
            set_cookie.push_str("; HttpOnly");
        }
        if let Some(expires) = c.expires {
            set_cookie.push_str(&format!(
                "; Expires={}",
                expires.to_rfc2822().replace("+0000", "GMT")
            ));
        }
        engine.cookies().store(&url, &set_cookie);
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    #[test]
    fn project_code_requires_explicit_gui_confirmation() {
        let root = tempfile::tempdir().unwrap();
        let document = forge_core::reqv1::RequestDocument::parse(
            r#"{"formatVersion":1,"kind":"request","meta":{"id":"x","name":"x"},
                "request":{"method":"GET","url":"https://example.test"},
                "pipeline":[{"phase":"afterResponse","use":"./check.js"}]}"#,
        )
        .unwrap();
        assert!(ensure_project_code_allowed(root.path(), &document, false).is_err());
        assert!(ensure_project_code_allowed(root.path(), &document, true).is_ok());
    }

    /// Poll `bridge` until an `Evt::Cookies` arrives (or panic after
    /// `timeout`). The bridge thread runs its own async runtime, so replies
    /// aren't necessarily available the instant a command is sent.
    fn recv_cookies(bridge: &Bridge, timeout: Duration) -> Vec<StoredCookie> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(Evt::Cookies(rows)) = bridge.try_recv() {
                return rows;
            }
            if Instant::now() > deadline {
                panic!("timed out waiting for Evt::Cookies");
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// `Cmd::LoadCookies` is sent every time the GUI switches workspaces
    /// (see `ForgeApp::on_workspace_opened`). It must clear the previous
    /// workspace's cookies before restoring the new one's — even when the
    /// new workspace has no `cookies.json` yet — otherwise cookies leak
    /// across workspaces and get persisted into the wrong file.
    #[test]
    fn load_cookies_clears_previous_workspace_jar_first() {
        let dir = std::env::temp_dir().join(format!(
            "forge-gui-bridge-test-{}-{}",
            std::process::id(),
            line!()
        ));
        let _ = std::fs::create_dir_all(&dir);

        let old_cookie = StoredCookie {
            domain: "old.example.com".to_string(),
            path: "/".to_string(),
            name: "session".to_string(),
            value: "old-workspace-value".to_string(),
            expires: None,
            secure: false,
            http_only: false,
        };
        let old_path = dir.join("old_cookies.json");
        std::fs::write(
            &old_path,
            serde_json::to_string(&vec![old_cookie]).expect("serialize"),
        )
        .expect("write");

        // The "new" workspace has no cookies.json of its own yet.
        let new_path = dir.join("new_cookies.json");

        let bridge = Bridge::new(egui::Context::default());

        // `Cmd::LoadCookies` itself replies with an `Evt::Cookies` snapshot
        // once it's done, so there's no need for a separate `ListCookies`
        // round-trip (which would race against — and double up with — that
        // reply, since the bridge processes commands strictly in order).
        bridge
            .send(Cmd::LoadCookies { path: old_path })
            .expect("bridge available");
        let rows = recv_cookies(&bridge, Duration::from_secs(5));
        assert_eq!(
            rows.len(),
            1,
            "expected the old workspace's cookie to have loaded"
        );

        bridge
            .send(Cmd::LoadCookies { path: new_path })
            .expect("bridge available");
        let rows = recv_cookies(&bridge, Duration::from_secs(5));
        assert!(
            rows.is_empty(),
            "switching to a workspace with no cookies.json must clear cookies left over from the previous workspace, got {rows:?}"
        );
    }
}
