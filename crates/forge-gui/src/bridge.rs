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
use forge_core::protocols::{sse, websocket, SseEvent, WsEvent, WsOutgoing};
use forge_core::runner::{self, CancellationToken, RunEvent, RunOptions, RunScope};
use forge_core::store::Workspace;

/// A command sent from the UI thread to the bridge thread.
pub enum Cmd {
    Run { run_id: u64, workspace: Box<Workspace>, scope: RunScope, options: RunOptions },
    Cancel { run_id: u64 },
    /// Open a WebSocket connection, forwarding events back as `Evt::Ws`.
    WsConnect { conn_id: u64, url: String, headers: Vec<(String, String)> },
    /// Send a text message over an open WebSocket connection.
    WsSend { conn_id: u64, msg: String },
    /// Request a clean close of a WebSocket connection.
    WsClose { conn_id: u64 },
    /// Subscribe to an SSE stream, forwarding events back as `Evt::Sse`.
    SseSubscribe { conn_id: u64, url: String, headers: Vec<(String, String)> },
    /// Stop consuming an SSE stream.
    SseClose { conn_id: u64 },
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
    /// Ask for a snapshot of the shared `HttpEngine`'s cookie jar.
    ListCookies,
    /// Remove one cookie from the shared jar.
    RemoveCookie { domain: String, name: String },
    /// Clear the whole shared cookie jar.
    ClearCookies,
    /// Load persisted cookies from `path` into the shared jar (best-effort;
    /// a missing or unreadable file is a no-op) and remember `path` so the
    /// jar is saved back there after every run and on shutdown.
    LoadCookies { path: PathBuf },
    Shutdown,
}

/// An event sent from the bridge thread back to the UI thread.
pub enum Evt {
    Run { run_id: u64, event: RunEvent },
    /// The run could not even start (bad scope, missing environment, ...).
    RunFailed { run_id: u64, error: String },
    /// Something happened on a WebSocket connection.
    Ws { conn_id: u64, event: WsEvent },
    /// Something happened on an SSE subscription.
    Sse { conn_id: u64, event: SseEvent },
    /// A fresh snapshot of the shared cookie jar (in reply to
    /// `Cmd::ListCookies`, or after any mutating cookie command).
    Cookies(Vec<StoredCookie>),
    /// Outcome of a `Cmd::GrpcCall`: response JSON + metadata, or an error.
    Grpc { call_id: u64, result: Result<forge_core::protocols::GrpcResponse, String> },
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

        Self { cmd_tx, evt_rx, handle: Some(handle) }
    }

    /// Send a command to the bridge thread. Silently dropped if the bridge
    /// thread has already shut down.
    pub fn send(&self, cmd: Cmd) {
        let _ = self.cmd_tx.send(cmd);
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
            let _ = evt_tx.send(Evt::RunFailed { run_id: 0, error: format!("failed to start async runtime: {e}") });
            return;
        }
    };

    rt.block_on(async move {
        let engine = Arc::new(HttpEngine::new());
        let cancels: Arc<Mutex<HashMap<u64, CancellationToken>>> = Arc::new(Mutex::new(HashMap::new()));
        // Outgoing-message senders for live WebSocket connections, keyed by
        // `conn_id` — the connection's own background task owns the
        // `WsSession`; this is just how `Cmd::WsSend`/`WsClose` reach it.
        let ws_conns: Arc<Mutex<HashMap<u64, tokio::sync::mpsc::UnboundedSender<WsOutgoing>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        // Cancellation tokens for live SSE subscriptions, keyed by `conn_id`.
        let sse_cancels: Arc<Mutex<HashMap<u64, CancellationToken>>> = Arc::new(Mutex::new(HashMap::new()));
        // Where to persist the cookie jar (set by `Cmd::LoadCookies`), saved
        // back after every run and on shutdown.
        let cookie_path: Arc<Mutex<Option<PathBuf>>> = Arc::new(Mutex::new(None));

        while let Some(cmd) = cmd_rx.recv().await {
            match cmd {
                Cmd::Run { run_id, workspace, scope, options } => {
                    let engine = engine.clone();
                    let evt_tx = evt_tx.clone();
                    let ctx = ctx.clone();
                    let cancels = cancels.clone();
                    let cookie_path = cookie_path.clone();
                    let cancel = CancellationToken::new();
                    cancels.lock().unwrap_or_else(|p| p.into_inner()).insert(run_id, cancel.clone());

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

                        let result = runner::run(&workspace, scope, options, &engine, tx, cancel).await;
                        if let Err(e) = result {
                            let _ = evt_tx.send(Evt::RunFailed { run_id, error: e.to_string() });
                            ctx.request_repaint();
                        }
                        let _ = forward.await;

                        cancels.lock().unwrap_or_else(|p| p.into_inner()).remove(&run_id);
                        save_cookies(&engine, &cookie_path);
                    });
                }
                Cmd::Cancel { run_id } => {
                    if let Some(token) = cancels.lock().unwrap_or_else(|p| p.into_inner()).get(&run_id) {
                        token.cancel();
                    }
                }
                Cmd::GrpcCall { call_id, protos, endpoint, method, request_json, metadata } => {
                    let evt_tx = evt_tx.clone();
                    let ctx = ctx.clone();
                    tokio::spawn(async move {
                        let result = async {
                            let pool = forge_core::protocols::compile_protos(&protos, &[])
                                .map_err(|e| e.to_string())?;
                            forge_core::protocols::call_unary(
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
                Cmd::WsConnect { conn_id, url, headers } => {
                    let evt_tx = evt_tx.clone();
                    let ctx = ctx.clone();
                    let ws_conns = ws_conns.clone();
                    tokio::spawn(async move {
                        match websocket::connect(&url, &headers).await {
                            Ok(mut session) => {
                                ws_conns.lock().unwrap_or_else(|p| p.into_inner()).insert(conn_id, session.outgoing.clone());
                                while let Some(event) = session.events.recv().await {
                                    let is_closed = matches!(event, WsEvent::Closed { .. });
                                    let _ = evt_tx.send(Evt::Ws { conn_id, event });
                                    ctx.request_repaint();
                                    if is_closed {
                                        break;
                                    }
                                }
                                ws_conns.lock().unwrap_or_else(|p| p.into_inner()).remove(&conn_id);
                            }
                            Err(e) => {
                                let _ = evt_tx.send(Evt::Ws { conn_id, event: WsEvent::Error(e.to_string()) });
                                ctx.request_repaint();
                            }
                        }
                    });
                }
                Cmd::WsSend { conn_id, msg } => {
                    if let Some(tx) = ws_conns.lock().unwrap_or_else(|p| p.into_inner()).get(&conn_id) {
                        let _ = tx.send(WsOutgoing::Text(msg));
                    }
                }
                Cmd::WsClose { conn_id } => {
                    if let Some(tx) = ws_conns.lock().unwrap_or_else(|p| p.into_inner()).get(&conn_id) {
                        let _ = tx.send(WsOutgoing::Close);
                    }
                }
                Cmd::SseSubscribe { conn_id, url, headers } => {
                    let evt_tx = evt_tx.clone();
                    let ctx = ctx.clone();
                    let sse_cancels = sse_cancels.clone();
                    let cancel = CancellationToken::new();
                    sse_cancels.lock().unwrap_or_else(|p| p.into_inner()).insert(conn_id, cancel.clone());
                    tokio::spawn(async move {
                        match sse::subscribe(&url, &headers).await {
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
                                let _ = evt_tx.send(Evt::Sse { conn_id, event: SseEvent::Error(e.to_string()) });
                                ctx.request_repaint();
                            }
                        }
                        sse_cancels.lock().unwrap_or_else(|p| p.into_inner()).remove(&conn_id);
                    });
                }
                Cmd::SseClose { conn_id } => {
                    if let Some(token) = sse_cancels.lock().unwrap_or_else(|p| p.into_inner()).remove(&conn_id) {
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

/// Persist the shared cookie jar to whichever path `Cmd::LoadCookies` last
/// set, if any. Best-effort — a write failure here shouldn't crash the
/// bridge thread.
fn save_cookies(engine: &HttpEngine, cookie_path: &Mutex<Option<PathBuf>>) {
    let Some(path) = cookie_path.lock().unwrap_or_else(|p| p.into_inner()).clone() else { return };
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
    let Ok(cookies) = serde_json::from_str::<Vec<StoredCookie>>(json) else { return };
    for c in cookies {
        let Ok(url) = url::Url::parse(&format!("https://{}/", c.domain)) else { continue };
        let mut set_cookie = format!("{}={}; Domain={}; Path={}", c.name, c.value, c.domain, c.path);
        if c.secure {
            set_cookie.push_str("; Secure");
        }
        if c.http_only {
            set_cookie.push_str("; HttpOnly");
        }
        if let Some(expires) = c.expires {
            set_cookie.push_str(&format!("; Expires={}", expires.to_rfc2822().replace("+0000", "GMT")));
        }
        engine.cookies().store(&url, &set_cookie);
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

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
        let dir = std::env::temp_dir().join(format!("forge-gui-bridge-test-{}-{}", std::process::id(), line!()));
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
        std::fs::write(&old_path, serde_json::to_string(&vec![old_cookie]).expect("serialize")).expect("write");

        // The "new" workspace has no cookies.json of its own yet.
        let new_path = dir.join("new_cookies.json");

        let bridge = Bridge::new(egui::Context::default());

        // `Cmd::LoadCookies` itself replies with an `Evt::Cookies` snapshot
        // once it's done, so there's no need for a separate `ListCookies`
        // round-trip (which would race against — and double up with — that
        // reply, since the bridge processes commands strictly in order).
        bridge.send(Cmd::LoadCookies { path: old_path });
        let rows = recv_cookies(&bridge, Duration::from_secs(5));
        assert_eq!(rows.len(), 1, "expected the old workspace's cookie to have loaded");

        bridge.send(Cmd::LoadCookies { path: new_path });
        let rows = recv_cookies(&bridge, Duration::from_secs(5));
        assert!(
            rows.is_empty(),
            "switching to a workspace with no cookies.json must clear cookies left over from the previous workspace, got {rows:?}"
        );
    }
}
