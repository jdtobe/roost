//! The detail pane of the Models tab: an installed model (actions, then its
//! details) or a Hugging Face repository (the Browse detail).

use crate::app::{bad, AppState, View};
use crate::browse::results::format_size;
use crate::installed::state::{
    relative_time, run_command, short_digest, thousands, DetailsState, InstalledRow,
};
use crate::runtime::BackendRequest;
use crate::theme::{mono, semibold, tokens, Tokens};
use crate::widgets::{band, ellipsize_middle, primary_button};
use eframe::egui::{self, vec2, Align2, Color32, FontId, RichText, Sense};
use roost_core::chat::ChatCommand;
use roost_core::session::SessionLifetime;
use std::time::{Instant, SystemTime};

const LABEL_W: f32 = 96.0;

pub fn show(ui: &mut egui::Ui, app: &mut AppState, out: &mut Vec<BackendRequest>) {
    let row = app.models_installed.as_ref().and_then(|name| {
        app.installed
            .rows()
            .iter()
            .find(|r| r.name() == name)
            .cloned()
    });
    match row {
        Some(row) => installed(ui, app, out, &row),
        None => {
            // The model was deleted (or is not listed any more).
            app.models_installed = None;
            crate::browse::detail::show(ui, app, out);
        }
    }
}

fn installed(
    ui: &mut egui::Ui,
    app: &mut AppState,
    out: &mut Vec<BackendRequest>,
    row: &InstalledRow,
) {
    let t = tokens(ui);
    let dark = ui.visuals().dark_mode;
    let now = Instant::now();
    let panel = egui::CentralPanel::default()
        .frame(egui::Frame::new().fill(t.surface))
        .show(ui, |ui| {
            // Measured outside the scroll area, where it is the real height.
            let view_h = ui.available_height();
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    let top = header(ui, app, out, row, now, dark, &t);
                    let (_, mid) = band(ui, t.bg, Some(t.line), 0.0, |ui| {
                        details(ui, app, row, &t, dark);
                    });
                    let rest = (view_h - top.height() - mid.height() - 32.0).max(0.0);
                    let (_, low) = band(ui, t.surface, None, rest, |ui| {
                        about(ui, app, row, &t);
                    });
                    #[cfg(test)]
                    {
                        app.browse.probe.bands = vec![top, mid, low];
                    }
                    let _ = (top, mid, low);
                });
        });
    #[cfg(test)]
    {
        app.browse.probe.detail = Some(panel.response.rect);
    }
    let _ = panel;
}

fn facts(row: &InstalledRow) -> String {
    let mut parts = vec![format_size(row.model.size)];
    for v in [&row.model.params, &row.model.quant, &row.model.family]
        .into_iter()
        .flatten()
    {
        parts.push(v.clone());
    }
    let when = relative_time(&row.model.modified, SystemTime::now());
    if when != "—" {
        parts.push(format!("modified {when}"));
    }
    parts.join("  ·  ")
}

/// The header band of an installed model: the kind tile, the name and facts,
/// and the actions at the right, in the same shape as a chat's header.
fn header(
    ui: &mut egui::Ui,
    app: &mut AppState,
    out: &mut Vec<BackendRequest>,
    row: &InstalledRow,
    now: Instant,
    dark: bool,
    t: &Tokens,
) -> egui::Rect {
    let name = row.name().to_string();
    let rect = egui::Frame::new()
        .fill(t.head_band())
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.set_min_height(44.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    actions(ui, app, out, row, now, dark);
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        let (tile, _) = ui.allocate_exact_size(vec2(44.0, 44.0), Sense::hover());
                        ui.painter().rect(
                            tile,
                            11.0,
                            t.surface,
                            egui::Stroke::new(1.0, t.side_edge()),
                            egui::StrokeKind::Inside,
                        );
                        crate::icons::paint_scaled(
                            ui.painter(),
                            tile,
                            crate::icons::Icon::Model,
                            t.fg2,
                            1.8,
                        );
                        ui.add_space(4.0);
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 0.0;
                            ui.add(
                                egui::Label::new(
                                    RichText::new(&name).font(semibold(18.0)).color(t.fg),
                                )
                                .truncate(),
                            );
                            ui.add(
                                egui::Label::new(
                                    RichText::new(facts(row)).size(12.0).color(t.band_ink2()),
                                )
                                .truncate(),
                            );
                        });
                    });
                });
            });
        })
        .response
        .rect;
    ui.painter().hline(
        rect.x_range(),
        rect.bottom(),
        egui::Stroke::new(1.0, t.line),
    );
    rect
}

fn actions(
    ui: &mut egui::Ui,
    app: &mut AppState,
    out: &mut Vec<BackendRequest>,
    row: &InstalledRow,
    now: Instant,
    dark: bool,
) {
    let t = tokens(ui);
    let name = row.name().to_string();
    let armed = app.installed.is_armed(&name, now);
    let ready = app.installed.confirm_ready(&name, now);
    let chat;
    let work;
    let delete;
    {
        ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
        let r = if armed {
            // Dimmed until the confirm delay has passed, but kept enabled so
            // keyboard focus survives; the state ignores early clicks.
            let fill = if ready {
                bad(dark)
            } else {
                bad(dark).gamma_multiply(0.45)
            };
            ui.add(
                egui::Button::new(
                    RichText::new("Confirm delete")
                        .size(13.0)
                        .color(Color32::WHITE),
                )
                .fill(fill)
                .min_size(vec2(0.0, 32.0)),
            )
        } else {
            // A trash can, red when pointed at like the delete controls elsewhere.
            let (rect, resp) = ui.allocate_exact_size(vec2(34.0, 32.0), Sense::click());
            let resp = resp
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text("Delete this model…");
            // Quiet until pointed at, then red.
            let hot = resp.hovered();
            let (fill, edge, ink) = if hot {
                (bad(dark).gamma_multiply(0.18), bad(dark), bad(dark))
            } else {
                (t.surface, t.line, t.fg2)
            };
            ui.painter().rect(
                rect,
                8.0,
                fill,
                egui::Stroke::new(1.0, edge),
                egui::StrokeKind::Inside,
            );
            crate::icons::paint(ui.painter(), rect, crate::icons::Icon::Trash, ink);
            resp
        };
        #[cfg(test)]
        {
            app.browse.probe.delete_button = Some(r.rect);
        }
        // A held Enter or Space repeats `clicked()`; only a fresh activation
        // may arm or confirm.
        delete = r.clicked() && !crate::confirm::activation_key_held(ui);
        let r = ui.add(
            egui::Button::new(RichText::new("Work").size(14.0).color(t.fg))
                .fill(t.surface)
                .stroke(egui::Stroke::new(1.0, t.line))
                .min_size(vec2(0.0, 32.0)),
        );
        #[cfg(test)]
        {
            app.browse.probe.work_button = Some(r.rect);
        }
        work = r.clicked();
        let r = primary_button(ui, "Chat", true);
        #[cfg(test)]
        {
            app.browse.probe.chat_button = Some(r.rect);
        }
        chat = r.clicked();
    }
    if armed {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(100));
    }
    if chat {
        out.push(BackendRequest::Chat(ChatCommand::Create {
            lifetime: SessionLifetime::Persisted,
            model: name.clone(),
        }));
        app.chat_ui.focus_composer = true;
        app.chat_ui.inspect = None;
        app.select(View::Chat);
    }
    if work {
        out.push(BackendRequest::Chat(ChatCommand::CreateAgent {
            lifetime: SessionLifetime::Persisted,
            model: name.clone(),
            workspace: None,
        }));
        app.chat_ui.focus_composer = true;
        app.chat_ui.inspect = None;
        app.select(View::Work);
    }
    if delete {
        if let Some(a) = app.installed.click_delete(&name, now) {
            out.push(BackendRequest::DeleteInstalled(a.name));
        }
    }
}

fn details(ui: &mut egui::Ui, app: &mut AppState, row: &InstalledRow, t: &Tokens, dark: bool) {
    ui.label(RichText::new("Details").font(semibold(14.0)));
    let line = |ui: &mut egui::Ui, label: &str, value: &mut dyn FnMut(&mut egui::Ui)| {
        ui.horizontal(|ui| {
            let (lr, _) = ui.allocate_exact_size(vec2(LABEL_W, 18.0), Sense::hover());
            ui.painter().text(
                lr.left_center(),
                Align2::LEFT_CENTER,
                label,
                FontId::proportional(12.0),
                t.fg3,
            );
            value(ui);
        });
    };
    match app.installed.details_of(row.name()) {
        None | Some(DetailsState::Loading) => {
            ui.label(RichText::new("Loading details…").size(12.5).color(t.fg3));
        }
        Some(DetailsState::Error(e)) => {
            ui.label(
                RichText::new(format!("Could not read the details. {e}"))
                    .size(12.5)
                    .color(bad(dark)),
            );
        }
        Some(DetailsState::Loaded(d)) => {
            let family = if d.family.is_empty() {
                "—".to_string()
            } else {
                d.family.clone()
            };
            line(ui, "Family", &mut |ui| {
                ui.label(RichText::new(&family).size(12.5));
            });
            let context = d
                .context
                .map_or("—".to_string(), |c| format!("{} tokens", thousands(c)));
            line(ui, "Context", &mut |ui| {
                ui.label(RichText::new(&context).size(12.5));
            });
            let caps = if d.capabilities.is_empty() {
                "—".to_string()
            } else {
                d.capabilities.join(", ")
            };
            line(ui, "Capabilities", &mut |ui| {
                ui.label(RichText::new(&caps).size(12.5));
            });
            let full = if d.digest.is_empty() {
                row.model.digest.clone()
            } else {
                d.digest.clone()
            };
            line(ui, "Digest", &mut |ui| {
                ui.label(RichText::new(short_digest(&full)).font(mono(11.5)))
                    .on_hover_text(&full);
            });
        }
    }
    let cmd = run_command(row.name());
    let mut copy = false;
    line(ui, "Run", &mut |ui| {
        let room = (ui.available_width() - 70.0).max(60.0);
        let shown = ellipsize_middle(ui, &cmd, mono(11.5), room);
        let r = ui.label(RichText::new(&shown).font(mono(11.5)));
        if shown != cmd {
            r.on_hover_text(&cmd);
        }
        let (rect, resp) = ui.allocate_exact_size(vec2(24.0, 22.0), Sense::click());
        let resp = resp
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text("Copy the command");
        if resp.hovered() {
            ui.painter().rect_filled(rect, 5.0, t.line);
        }
        let colour = if resp.hovered() { t.fg } else { t.fg3 };
        crate::icons::paint(ui.painter(), rect, crate::icons::Icon::Copy, colour);
        copy = resp.clicked();
    });
    if copy {
        ui.ctx().copy_text(cmd.clone());
    }
    let mut open_repo = None;
    line(ui, "Source", &mut |ui| match &row.source_repo {
        Some(repo) => {
            let room = ui.available_width().max(60.0);
            let shown = ellipsize_middle(ui, repo, FontId::proportional(12.5), room);
            if row.linkable_repo().is_some() {
                if ui
                    .link(RichText::new(shown).size(12.5))
                    .on_hover_text("Show this repository")
                    .clicked()
                {
                    open_repo = Some(repo.clone());
                }
            } else {
                ui.label(RichText::new(shown).size(12.5).color(t.fg2));
            }
        }
        None => {
            ui.label(
                RichText::new("Pulled from the Ollama library")
                    .size(12.5)
                    .color(t.fg3),
            );
        }
    });
    if let Some(repo) = open_repo {
        app.browse.select_repo(&repo);
        app.models_installed = None;
    }
}

/// The source repository's description, or a note that there is none.
fn about(ui: &mut egui::Ui, app: &mut AppState, row: &InstalledRow, t: &Tokens) {
    ui.label(RichText::new("About").font(semibold(14.0)));
    #[cfg(test)]
    {
        app.browse.probe.about_top = Some(ui.cursor().top());
    }
    let Some(repo) = row.linkable_repo().map(str::to_string) else {
        let why = if row.source_repo.is_some() {
            "The source of this model is not a Hugging Face repository Roost can open."
        } else {
            "This model was pulled from the Ollama library, so there is no Hugging Face description."
        };
        ui.add(egui::Label::new(RichText::new(why).size(12.5).color(t.fg3)).wrap());
        return;
    };
    app.browse.want_info(&repo);
    app.browse.want_card(&repo);
    match app.browse.info_for(&repo).cloned() {
        None => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new("Loading description…").color(t.fg3));
            });
        }
        Some(Err(e)) => {
            ui.add(
                egui::Label::new(
                    RichText::new(format!("Could not load the description. {e}"))
                        .size(12.5)
                        .color(t.fg3),
                )
                .wrap(),
            );
        }
        Some(Ok(info)) => crate::browse::about::show(ui, app, &info, &repo),
    }
}
