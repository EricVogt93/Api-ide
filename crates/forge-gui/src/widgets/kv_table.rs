//! Reusable editable key/value table used for query params, headers and
//! url-encoded form fields.

use egui::{Align, Layout, RichText, Ui};
use egui_extras::{Column, TableBuilder};
use forge_core::model::KeyValue;

/// A column header cell in the Relay style: small, uppercase, dimmed.
fn hcol(ui: &mut Ui, text: &str) {
    ui.label(
        RichText::new(text)
            .size(12.0)
            .strong()
            .color(ui.visuals().weak_text_color()),
    );
}

/// Render an editable key/value table over `rows`.
///
/// Each row has an enabled checkbox, key, value and (optionally)
/// description columns, plus a delete button. A trailing empty row is kept
/// appended automatically so the user always has somewhere to start typing
/// a new entry — it is materialized into `rows` as soon as it gets a key or
/// value.
///
/// Returns `true` if any row was added, edited or removed this frame.
pub fn kv_table(
    ui: &mut Ui,
    id_salt: &str,
    rows: &mut Vec<KeyValue>,
    show_description: bool,
) -> bool {
    let mut changed = false;
    let mut remove_idx: Option<usize> = None;

    // Always keep one trailing blank row so the user can type a new entry.
    if rows
        .last()
        .is_none_or(|r| !r.key.is_empty() || !r.value.is_empty())
    {
        rows.push(KeyValue::new("", ""));
    }

    let mut builder = TableBuilder::new(ui)
        .id_salt(id_salt)
        .striped(true)
        .cell_layout(Layout::left_to_right(Align::Center))
        .column(Column::exact(24.0))
        .column(Column::auto().at_least(70.0).resizable(true))
        .column(Column::remainder().at_least(70.0));
    if show_description {
        builder = builder.column(Column::auto().at_least(70.0).resizable(true));
    }
    builder = builder.column(Column::exact(26.0));

    builder
        .header(20.0, |mut header| {
            header.col(|_ui| {});
            header.col(|ui| {
                hcol(ui, "KEY");
            });
            header.col(|ui| {
                hcol(ui, "VALUE");
            });
            if show_description {
                header.col(|ui| {
                    hcol(ui, "DESCRIPTION");
                });
            }
            header.col(|_ui| {});
        })
        .body(|mut body| {
            let n = rows.len();
            #[allow(clippy::needless_range_loop)]
            for i in 0..n {
                body.row(22.0, |mut row| {
                    row.col(|ui| {
                        if ui.checkbox(&mut rows[i].enabled, "").changed() {
                            changed = true;
                        }
                    });
                    row.col(|ui| {
                        if ui.text_edit_singleline(&mut rows[i].key).changed() {
                            changed = true;
                        }
                    });
                    row.col(|ui| {
                        if ui.text_edit_singleline(&mut rows[i].value).changed() {
                            changed = true;
                        }
                    });
                    if show_description {
                        row.col(|ui| {
                            if ui.text_edit_singleline(&mut rows[i].description).changed() {
                                changed = true;
                            }
                        });
                    }
                    row.col(|ui| {
                        // Keep the always-present trailing blank row from
                        // being deletable (there is nothing to delete yet).
                        let is_trailing_blank =
                            i == n - 1 && rows[i].key.is_empty() && rows[i].value.is_empty();
                        if !is_trailing_blank && ui.small_button("\u{2715}").clicked() {
                            remove_idx = Some(i);
                        }
                    });
                });
            }
        });

    if let Some(idx) = remove_idx {
        rows.remove(idx);
        changed = true;
    }

    changed
}
