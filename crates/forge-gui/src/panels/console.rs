//! WebSocket / SSE console tool window: a connection bar, a color-coded
//! message log and (for WebSocket) a send box. Connections run on the
//! bridge thread (see `bridge::Cmd::{WsConnect,SseSubscribe,...}`); this
//! module only owns the GUI-side view of them, keyed by `conn_id`.

use chrono::{DateTime, Utc};
use egui::{Color32, RichText, Ui};

use forge_core::model::KeyValue;
use forge_core::protocols::{SseEvent, WsEvent};

use crate::bridge::{Bridge, Cmd};
use crate::state::{AppState, StatusMessage};

/// Which non-REST protocol a console connection speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    Ws,
    Sse,
}

impl Protocol {
    fn label(self) -> &'static str {
        match self {
            Protocol::Ws => "WebSocket",
            Protocol::Sse => "SSE",
        }
    }
}

/// Lifecycle state of one connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnStatus {
    Connecting,
    Open,
    Closed,
    Error(String),
}

impl ConnStatus {
    fn label(&self) -> String {
        match self {
            ConnStatus::Connecting => "Connecting\u{2026}".to_string(),
            ConnStatus::Open => "Open".to_string(),
            ConnStatus::Closed => "Closed".to_string(),
            ConnStatus::Error(e) => format!("Error: {e}"),
        }
    }
}

/// Direction/kind of one logged message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    In,
    Out,
    Lifecycle,
    Error,
}

/// One logged line in a connection's message log.
#[derive(Debug, Clone)]
pub struct LogMessage {
    pub direction: Direction,
    pub at: DateTime<Utc>,
    pub text: String,
    pub expanded: bool,
}

impl LogMessage {
    fn lifecycle(text: impl Into<String>) -> Self {
        Self {
            direction: Direction::Lifecycle,
            at: Utc::now(),
            text: text.into(),
            expanded: false,
        }
    }
    fn incoming(text: impl Into<String>, at: DateTime<Utc>) -> Self {
        Self {
            direction: Direction::In,
            at,
            text: text.into(),
            expanded: false,
        }
    }
    fn outgoing(text: impl Into<String>) -> Self {
        Self {
            direction: Direction::Out,
            at: Utc::now(),
            text: text.into(),
            expanded: false,
        }
    }
    fn error(text: impl Into<String>) -> Self {
        Self {
            direction: Direction::Error,
            at: Utc::now(),
            text: text.into(),
            expanded: false,
        }
    }
}

/// One live (or recently closed) connection tracked by the console.
pub struct Connection {
    pub id: u64,
    pub protocol: Protocol,
    pub url: String,
    pub status: ConnStatus,
    pub messages: Vec<LogMessage>,
}

/// Console tool window state, held on [`AppState`].
pub struct ConsoleState {
    pub protocol: Protocol,
    pub url: String,
    pub headers: Vec<KeyValue>,
    pub connections: Vec<Connection>,
    pub active_conn: Option<u64>,
    pub send_text: String,
    pub auto_scroll: bool,
    next_conn_id: u64,
}

impl Default for ConsoleState {
    fn default() -> Self {
        Self {
            protocol: Protocol::Ws,
            url: String::new(),
            headers: Vec::new(),
            connections: Vec::new(),
            active_conn: None,
            send_text: String::new(),
            auto_scroll: true,
            next_conn_id: 0,
        }
    }
}

fn active_connection(state: &AppState) -> Option<&Connection> {
    let id = state.console.active_conn?;
    state.console.connections.iter().find(|c| c.id == id)
}

fn active_connection_mut(state: &mut AppState) -> Option<&mut Connection> {
    let id = state.console.active_conn?;
    state.console.connections.iter_mut().find(|c| c.id == id)
}

/// Fold a WebSocket event arriving from the bridge into the matching
/// connection's log.
pub fn handle_ws_event(state: &mut AppState, conn_id: u64, event: WsEvent) {
    let Some(conn) = state
        .console
        .connections
        .iter_mut()
        .find(|c| c.id == conn_id)
    else {
        return;
    };
    match event {
        WsEvent::Connected => {
            conn.status = ConnStatus::Open;
            conn.messages.push(LogMessage::lifecycle("Connected"));
        }
        WsEvent::Text { text, at } => conn.messages.push(LogMessage::incoming(text, at)),
        WsEvent::Binary { data, at } => conn.messages.push(LogMessage::incoming(
            format!("<binary, {} bytes>", data.len()),
            at,
        )),
        WsEvent::Pong => conn.messages.push(LogMessage::lifecycle("Pong")),
        WsEvent::Closed { code, reason } => {
            conn.status = ConnStatus::Closed;
            let detail = match (code, reason.is_empty()) {
                (Some(c), false) => format!(" ({c}: {reason})"),
                (Some(c), true) => format!(" ({c})"),
                (None, false) => format!(" ({reason})"),
                (None, true) => String::new(),
            };
            conn.messages
                .push(LogMessage::lifecycle(format!("Closed{detail}")));
        }
        WsEvent::Error(e) => {
            conn.status = ConnStatus::Error(e.clone());
            conn.messages.push(LogMessage::error(e));
        }
    }
}

/// Fold an SSE event arriving from the bridge into the matching
/// connection's log.
pub fn handle_sse_event(state: &mut AppState, conn_id: u64, event: SseEvent) {
    let Some(conn) = state
        .console
        .connections
        .iter_mut()
        .find(|c| c.id == conn_id)
    else {
        return;
    };
    match event {
        SseEvent::Open => {
            conn.status = ConnStatus::Open;
            conn.messages.push(LogMessage::lifecycle("Connected"));
        }
        SseEvent::Event {
            id,
            event,
            data,
            at,
        } => {
            let mut text = String::new();
            if !event.is_empty() {
                text.push_str(&format!("event: {event}\n"));
            }
            if !id.is_empty() {
                text.push_str(&format!("id: {id}\n"));
            }
            text.push_str(&format!("data: {data}"));
            conn.messages.push(LogMessage::incoming(text, at));
        }
        SseEvent::Error(e) => {
            conn.status = ConnStatus::Error(e.clone());
            conn.messages.push(LogMessage::error(e));
        }
        SseEvent::Closed => {
            conn.status = ConnStatus::Closed;
            conn.messages.push(LogMessage::lifecycle("Closed"));
        }
    }
}

/// Render the Console tool window.
pub fn show(ui: &mut Ui, state: &mut AppState, bridge: &Bridge) {
    connection_bar(ui, state, bridge);
    ui.separator();
    if state.console.connections.len() > 1 {
        conn_tabs(ui, state);
        ui.separator();
    }
    log_area(ui, state);
    if matches!(
        active_connection(state).map(|c| c.protocol),
        Some(Protocol::Ws)
    ) {
        send_box(ui, state, bridge);
    }
}

fn connection_bar(ui: &mut Ui, state: &mut AppState, bridge: &Bridge) {
    ui.horizontal(|ui| {
        egui::ComboBox::from_id_salt("console-protocol")
            .selected_text(state.console.protocol.label())
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut state.console.protocol, Protocol::Ws, "WebSocket");
                ui.selectable_value(&mut state.console.protocol, Protocol::Sse, "SSE");
            });
        ui.add(
            egui::TextEdit::singleline(&mut state.console.url)
                .desired_width(300.0)
                .hint_text("ws://host/path or https://host/events"),
        );
        ui.menu_button("Headers", |ui| {
            crate::widgets::kv_table::kv_table(
                ui,
                "console-headers",
                &mut state.console.headers,
                false,
            );
        });

        let connecting_or_open = active_connection(state)
            .is_some_and(|c| matches!(c.status, ConnStatus::Open | ConnStatus::Connecting));
        if connecting_or_open {
            if ui.button("Disconnect").clicked() {
                disconnect_active(state, bridge);
            }
        } else if ui.button("Connect").clicked() {
            connect(state, bridge);
        }

        if let Some(conn) = active_connection(state) {
            let color = match conn.status {
                ConnStatus::Open => state.theme.ok_color(),
                ConnStatus::Connecting | ConnStatus::Closed => Color32::GRAY,
                ConnStatus::Error(_) => state.theme.error_color(),
            };
            crate::widgets::dot(ui, color, 5.0);
            ui.label(conn.status.label());
        }

        ui.checkbox(&mut state.console.auto_scroll, "Autoscroll");
        if ui.button("Clear").clicked() {
            if let Some(conn) = active_connection_mut(state) {
                conn.messages.clear();
            }
        }
    });
}

fn connect(state: &mut AppState, bridge: &Bridge) {
    let url = state.console.url.trim().to_string();
    if url.is_empty() {
        state.status = Some(StatusMessage::error("Enter a URL to connect"));
        return;
    }
    let headers: Vec<(String, String)> = state
        .console
        .headers
        .iter()
        .filter(|h| h.enabled && !h.key.is_empty())
        .map(|h| (h.key.clone(), h.value.clone()))
        .collect();

    // Validate workspace mTLS/CA settings before creating a connection row
    // that could otherwise remain stuck in "Connecting".
    let tls = match &state.workspace {
        Some(ws) => {
            match forge_core::protocols::TlsMaterial::from_settings(
                &ws.root,
                ws.meta.settings.tls.as_ref(),
            ) {
                Ok(material) => material,
                Err(e) => {
                    state.status = Some(StatusMessage::error(format!("TLS settings: {e}")));
                    return;
                }
            }
        }
        None => forge_core::protocols::TlsMaterial::default(),
    };

    state.console.next_conn_id += 1;
    let conn_id = state.console.next_conn_id;
    let protocol = state.console.protocol;
    state.console.connections.push(Connection {
        id: conn_id,
        protocol,
        url: url.clone(),
        status: ConnStatus::Connecting,
        messages: vec![LogMessage::lifecycle(format!(
            "Connecting to {url}\u{2026}"
        ))],
    });
    state.console.active_conn = Some(conn_id);

    let sent = match protocol {
        Protocol::Ws => bridge.send(Cmd::WsConnect {
            conn_id,
            url,
            headers,
            tls,
        }),
        Protocol::Sse => bridge.send(Cmd::SseSubscribe {
            conn_id,
            url,
            headers,
            tls,
        }),
    };
    if let Err(error) = sent {
        if let Some(connection) = active_connection_mut(state) {
            connection.status = ConnStatus::Error(error.clone());
            connection
                .messages
                .push(LogMessage::lifecycle(error.clone()));
        }
        state.status = Some(StatusMessage::error(error));
    }
}

fn disconnect_active(state: &mut AppState, bridge: &Bridge) {
    let Some(id) = state.console.active_conn else {
        return;
    };
    let Some(conn) = state.console.connections.iter().find(|c| c.id == id) else {
        return;
    };
    let sent = match conn.protocol {
        Protocol::Ws => bridge.send(Cmd::WsClose { conn_id: id }),
        Protocol::Sse => bridge.send(Cmd::SseClose { conn_id: id }),
    };
    if let Err(error) = sent {
        state.status = Some(StatusMessage::error(error));
    }
}

fn conn_tabs(ui: &mut Ui, state: &mut AppState) {
    let mut select: Option<u64> = None;
    ui.horizontal_wrapped(|ui| {
        for conn in &state.console.connections {
            let is_active = state.console.active_conn == Some(conn.id);
            let label = format!("{}: {}", conn.protocol.label(), truncate(&conn.url, 32));
            if ui.selectable_label(is_active, label).clicked() {
                select = Some(conn.id);
            }
        }
    });
    if let Some(id) = select {
        state.console.active_conn = Some(id);
    }
}

fn log_area(ui: &mut Ui, state: &mut AppState) {
    let auto_scroll = state.console.auto_scroll;
    let mut toggle_expand: Option<usize> = None;
    let mut rendered = false;

    if let Some(conn) = active_connection(state) {
        rendered = true;
        egui::ScrollArea::vertical()
            .id_salt("console-log")
            .auto_shrink([false, false])
            .stick_to_bottom(auto_scroll)
            .show(ui, |ui| {
                for (i, msg) in conn.messages.iter().enumerate() {
                    render_message(ui, msg, i, &mut toggle_expand);
                }
            });
    }

    if !rendered {
        ui.centered_and_justified(|ui| {
            ui.weak("Connect to a WebSocket or SSE endpoint to see messages here.");
        });
    }

    if let Some(i) = toggle_expand {
        if let Some(conn) = active_connection_mut(state) {
            if let Some(m) = conn.messages.get_mut(i) {
                m.expanded = !m.expanded;
            }
        }
    }
}

fn render_message(ui: &mut Ui, msg: &LogMessage, idx: usize, toggle: &mut Option<usize>) {
    let (arrow, color, italics) = match msg.direction {
        Direction::Out => ("\u{2192}", Color32::from_rgb(0x35, 0x92, 0xC4), false),
        Direction::In => ("\u{2190}", ui.visuals().text_color(), false),
        Direction::Lifecycle => ("\u{2022}", Color32::GRAY, true),
        Direction::Error => ("\u{2715}", Color32::from_rgb(0xC7, 0x54, 0x50), false),
    };
    ui.horizontal_wrapped(|ui| {
        ui.colored_label(color, arrow);
        ui.weak(msg.at.format("%H:%M:%S").to_string());
        let truncated = msg.text.chars().count() > 300 && !msg.expanded;
        let shown = if truncated {
            format!("{}\u{2026}", msg.text.chars().take(300).collect::<String>())
        } else {
            msg.text.clone()
        };
        let text = if italics {
            RichText::new(shown).italics().color(color)
        } else {
            RichText::new(shown).color(color)
        };
        if ui
            .add(egui::Label::new(text).sense(egui::Sense::click()))
            .clicked()
        {
            *toggle = Some(idx);
        }
    });
}

fn send_box(ui: &mut Ui, state: &mut AppState, bridge: &Bridge) {
    ui.separator();
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::multiline(&mut state.console.send_text)
                .desired_rows(2)
                .desired_width(ui.available_width() - 70.0),
        );
        let can_send = active_connection(state).is_some_and(|c| c.status == ConnStatus::Open);
        if ui
            .add_enabled(can_send, egui::Button::new("Send"))
            .clicked()
        {
            send_message(state, bridge);
        }
    });
}

fn send_message(state: &mut AppState, bridge: &Bridge) {
    let Some(id) = state.console.active_conn else {
        return;
    };
    let msg = std::mem::take(&mut state.console.send_text);
    if msg.is_empty() {
        return;
    }
    if let Err(error) = bridge.send(Cmd::WsSend {
        conn_id: id,
        msg: msg.clone(),
    }) {
        state.console.send_text = msg;
        state.status = Some(StatusMessage::error(error));
        return;
    }
    if let Some(conn) = active_connection_mut(state) {
        conn.messages.push(LogMessage::outgoing(msg));
    }
}

fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        format!("{}\u{2026}", s.chars().take(max_chars).collect::<String>())
    }
}
