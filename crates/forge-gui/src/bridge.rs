//! The GUI never blocks its render thread on network I/O: all HTTP
//! execution happens on a dedicated background thread that owns a tokio
//! runtime and a single shared [`HttpEngine`] (so cookies persist across
//! runs). Commands flow in via an unbounded tokio channel (cheap to send
//! from sync code); events flow back out via a `std::sync::mpsc` channel
//! that the egui thread drains once per frame, with `Context::request_repaint`
//! called after every event so the UI wakes up promptly even when idle.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use forge_core::exec::HttpEngine;
use forge_core::runner::{self, CancellationToken, RunEvent, RunOptions, RunScope};
use forge_core::store::Workspace;

/// A command sent from the UI thread to the bridge thread.
pub enum Cmd {
    Run { run_id: u64, workspace: Box<Workspace>, scope: RunScope, options: RunOptions },
    Cancel { run_id: u64 },
    Shutdown,
}

/// An event sent from the bridge thread back to the UI thread.
pub enum Evt {
    Run { run_id: u64, event: RunEvent },
    /// The run could not even start (bad scope, missing environment, ...).
    RunFailed { run_id: u64, error: String },
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

        while let Some(cmd) = cmd_rx.recv().await {
            match cmd {
                Cmd::Run { run_id, workspace, scope, options } => {
                    let engine = engine.clone();
                    let evt_tx = evt_tx.clone();
                    let ctx = ctx.clone();
                    let cancels = cancels.clone();
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
                    });
                }
                Cmd::Cancel { run_id } => {
                    if let Some(token) = cancels.lock().unwrap_or_else(|p| p.into_inner()).get(&run_id) {
                        token.cancel();
                    }
                }
                Cmd::Shutdown => break,
            }
        }
    });
}
