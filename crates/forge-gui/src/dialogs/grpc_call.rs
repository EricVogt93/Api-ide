//! gRPC Call dialog (Run menu): pick .proto files, choose a unary method,
//! edit the JSON request message and metadata, call, and read the response
//! JSON + metadata. Compilation and the call itself run on the bridge
//! thread; the dialog only holds state.

use std::path::PathBuf;

use egui::{RichText, TextEdit, Window};

use forge_core::protocols::{compile_protos, list_methods, GrpcMethod, GrpcResponse};

use crate::bridge::{Bridge, Cmd};
use crate::state::AppState;

#[derive(Default)]
pub struct GrpcCallState {
    pub open: bool,
    protos: Vec<PathBuf>,
    methods: Vec<GrpcMethod>,
    compile_error: Option<String>,
    selected_method: usize,
    endpoint: String,
    request_json: String,
    metadata: Vec<(String, String)>,
    in_flight: bool,
    next_call_id: u64,
    active_call: Option<u64>,
    response: Option<Result<GrpcResponse, String>>,
}

impl GrpcCallState {
    /// Open the dialog, optionally re-using previously loaded protos.
    pub fn open(&mut self) {
        self.open = true;
        if self.request_json.is_empty() {
            self.request_json = "{}".to_string();
        }
        if self.endpoint.is_empty() {
            self.endpoint = "http://localhost:50051".to_string();
        }
    }

    fn pick_protos(&mut self) {
        let Some(files) = rfd::FileDialog::new().add_filter("Protobuf", &["proto"]).pick_files()
        else {
            return;
        };
        self.protos = files;
        self.reload_methods();
    }

    fn reload_methods(&mut self) {
        self.methods.clear();
        self.compile_error = None;
        self.selected_method = 0;
        match compile_protos(&self.protos, &[]) {
            Ok(pool) => self.methods = list_methods(&pool),
            Err(e) => self.compile_error = Some(e.to_string()),
        }
    }

    /// Route a `Evt::Grpc` outcome into the dialog.
    pub fn handle_result(&mut self, call_id: u64, result: Result<GrpcResponse, String>) {
        if self.active_call == Some(call_id) {
            self.in_flight = false;
            self.active_call = None;
            self.response = Some(result);
        }
    }
}

/// Render the dialog if open; no-op otherwise.
pub fn show(ctx: &egui::Context, state: &mut AppState, bridge: &Bridge) {
    if !state.dialogs.grpc_call.open {
        return;
    }

    let mut window_open = true;
    Window::new("gRPC Call")
        .id(egui::Id::new("grpc-call-dialog"))
        .collapsible(false)
        .resizable(true)
        .default_size([640.0, 560.0])
        .open(&mut window_open)
        .show(ctx, |ui| {
            let dialog = &mut state.dialogs.grpc_call;

            ui.horizontal(|ui| {
                if ui.button("Load .proto…").clicked() {
                    dialog.pick_protos();
                }
                match dialog.protos.len() {
                    0 => ui.weak("no schema loaded"),
                    n => ui.label(format!(
                        "{n} file(s), {} method(s)",
                        dialog.methods.len()
                    )),
                };
            });
            if let Some(err) = &dialog.compile_error {
                ui.colored_label(ui.visuals().error_fg_color, err);
            }

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label("Endpoint:");
                ui.add(
                    TextEdit::singleline(&mut dialog.endpoint)
                        .hint_text("http://localhost:50051")
                        .desired_width(280.0),
                );
                ui.label("Method:");
                let selected_label = dialog
                    .methods
                    .get(dialog.selected_method)
                    .map(|m| m.path.clone())
                    .unwrap_or_else(|| "—".to_string());
                egui::ComboBox::from_id_salt("grpc-method")
                    .selected_text(selected_label)
                    .width(240.0)
                    .show_ui(ui, |ui| {
                        for (i, m) in dialog.methods.iter().enumerate() {
                            let label = if m.is_unary {
                                m.path.clone()
                            } else {
                                format!("{} (streaming — unsupported)", m.path)
                            };
                            ui.selectable_value(&mut dialog.selected_method, i, label);
                        }
                    });
            });
            if let Some(m) = dialog.methods.get(dialog.selected_method) {
                ui.weak(format!("{} → {}", m.input_type, m.output_type));
            }

            ui.add_space(6.0);
            ui.label("Request message (JSON):");
            egui::ScrollArea::vertical().id_salt("grpc-req").max_height(140.0).show(ui, |ui| {
                ui.add(
                    TextEdit::multiline(&mut dialog.request_json)
                        .code_editor()
                        .desired_width(f32::INFINITY)
                        .desired_rows(6),
                );
            });

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label("Metadata:");
                if ui.small_button("+").clicked() {
                    dialog.metadata.push((String::new(), String::new()));
                }
            });
            let mut remove: Option<usize> = None;
            for (i, (k, v)) in dialog.metadata.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    ui.add(TextEdit::singleline(k).hint_text("key").desired_width(180.0));
                    ui.add(TextEdit::singleline(v).hint_text("value").desired_width(280.0));
                    if ui.small_button("✕").clicked() {
                        remove = Some(i);
                    }
                });
            }
            if let Some(i) = remove {
                dialog.metadata.remove(i);
            }

            ui.add_space(8.0);
            let can_call = !dialog.in_flight
                && !dialog.endpoint.trim().is_empty()
                && dialog.methods.get(dialog.selected_method).is_some_and(|m| m.is_unary);
            if ui.add_enabled(can_call, egui::Button::new("▶ Call")).clicked() {
                let call_id = dialog.next_call_id;
                dialog.next_call_id += 1;
                dialog.active_call = Some(call_id);
                dialog.in_flight = true;
                dialog.response = None;
                bridge.send(Cmd::GrpcCall {
                    call_id,
                    protos: dialog.protos.clone(),
                    endpoint: dialog.endpoint.trim().to_string(),
                    method: dialog.methods[dialog.selected_method].path.clone(),
                    request_json: dialog.request_json.clone(),
                    metadata: dialog
                        .metadata
                        .iter()
                        .filter(|(k, _)| !k.trim().is_empty())
                        .map(|(k, v)| (k.trim().to_string(), v.clone()))
                        .collect(),
                });
            }
            if dialog.in_flight {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.weak("calling…");
                });
            }

            ui.separator();
            match &dialog.response {
                None => {
                    ui.weak("Response will appear here.");
                }
                Some(Err(e)) => {
                    ui.colored_label(ui.visuals().error_fg_color, e);
                }
                Some(Ok(response)) => {
                    if !response.metadata.is_empty() {
                        for (k, v) in &response.metadata {
                            ui.weak(format!("{k}: {v}"));
                        }
                        ui.add_space(4.0);
                    }
                    ui.horizontal(|ui| {
                        if ui.button("Copy").clicked() {
                            ui.ctx().copy_text(response.json.clone());
                        }
                        ui.label(RichText::new("OK").color(egui::Color32::from_rgb(0x49, 0x9C, 0x54)));
                    });
                    egui::ScrollArea::vertical().id_salt("grpc-res").max_height(200.0).show(
                        ui,
                        |ui| {
                            let mut text = response.json.as_str();
                            ui.add(
                                TextEdit::multiline(&mut text)
                                    .code_editor()
                                    .desired_width(f32::INFINITY),
                            );
                        },
                    );
                }
            }
        });

    if !window_open {
        state.dialogs.grpc_call.open = false;
    }
}
