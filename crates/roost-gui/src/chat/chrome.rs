//! The chrome around a conversation, kept to a few quiet controls: a title
//! row (title, folder chip for agents, an overflow menu), a status line, a
//! model chip under the input, and tool steps folded into one summary.

use super::view::{composer_id, same_model, secondary, tool_card, truncate_chars, unload_note};
use crate::app::AppState;
use crate::icons::Icon;
use crate::runtime::BackendRequest;
use crate::theme::{mono, semibold, Tokens};
use crate::widgets::tag;
use eframe::egui::{
    self,
    containers::menu::{MenuButton, MenuConfig},
    Align, Key, Layout, PopupCloseBehavior, RichText,
};
use roost_core::agent::registry::{self, Capability, Family};
use roost_core::chat::ChatCommand;
use roost_core::session::{
    EffectiveToolConfig, Message, MessageContent, Permission, Session, SessionLifetime,
    SessionMode, ToolStatus,
};
use std::time::Instant;

/// A menu that stays open while its controls are used.
fn sticky() -> MenuConfig {
    MenuConfig::default().close_behavior(PopupCloseBehavior::CloseOnClickOutside)
}

/// The folder with the home directory written as `~`.
pub(super) fn home_relative(path: &std::path::Path) -> String {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));
    if let Some(home) = home
        .map(std::path::PathBuf::from)
        .filter(|h| !h.as_os_str().is_empty())
    {
        if let Ok(rest) = path.strip_prefix(&home) {
            return if rest.as_os_str().is_empty() {
                "~".to_string()
            } else {
                format!("~{}{}", std::path::MAIN_SEPARATOR, rest.display())
            };
        }
    }
    path.display().to_string()
}

/// What the status dot says: idle, working (with the step of an agent) or
/// waiting for the user's answer.
pub(super) fn status_of(app: &AppState, s: &Session) -> (StatusKind, String) {
    if app.chat.approval(s.id).is_some() {
        return (StatusKind::Waiting, "Waiting for you".to_string());
    }
    if app.chat.is_running(s.id) {
        let label = match (s.mode, app.chat.step_of(s.id)) {
            (SessionMode::Agent, n) if n > 0 => {
                format!("Working · step {n} of {}", s.step_budget)
            }
            (SessionMode::Agent, _) => "Working".to_string(),
            _ => "Replying".to_string(),
        };
        return (StatusKind::Working, label);
    }
    (StatusKind::Idle, "Idle".to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StatusKind {
    Idle,
    Working,
    Waiting,
}

/// Side of the rename pencil beside the title.
const PENCIL_W: f32 = 24.0;
/// How much larger than the usual 16 px the session kind's glyph is drawn.
const TILE_ICON_SCALE: f32 = 1.8;
/// The session kind's tile in the header: bigger than the buttons beside it.
const TILE_SIZE: f32 = 44.0;
/// How far the tile and the buttons are from the header band's sides.
const HEADER_SIDE_INSET: f32 = 10.0;
/// How far the folder under the title is set in from the title's left edge.
const PATH_INSET: f32 = 6.0;

/// The folder under an agent's title: smaller than body text so it stays
/// quiet and fits more of the path.
const PATH_FONT: f32 = 11.0;

/// Height of the square icon buttons in the header.
const HEADER_BTN: f32 = 32.0;

/// A bordered square icon button.
fn icon_button(ui: &mut egui::Ui, t: &Tokens, icon: Icon, tip: &str) -> egui::Response {
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(HEADER_BTN, HEADER_BTN), egui::Sense::click());
    let resp = resp
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(tip);
    let fill = if resp.hovered() {
        t.surface2
    } else {
        t.surface
    };
    ui.painter().rect(
        rect,
        8.0,
        fill,
        egui::Stroke::new(1.0, t.line),
        egui::StrokeKind::Inside,
    );
    crate::icons::paint(ui.painter(), rect, icon, t.fg2);
    resp
}

/// Height of the row of title, status and buttons in the page header.
const HEADER_ROW_H: f32 = 44.0;

pub(super) fn header(
    ui: &mut egui::Ui,
    app: &mut AppState,
    out: &mut Vec<BackendRequest>,
    t: &Tokens,
    s: &Session,
    now: Instant,
) {
    let row = app.chat.rows().iter().find(|r| r.id == s.id).cloned();
    let shown = row
        .as_ref()
        .map_or(s.title.clone(), super::state::display_title);
    let pinned = row.as_ref().is_some_and(|r| r.pinned);
    let incognito = s.lifetime == SessionLifetime::Incognito;
    let (kind, status) = status_of(app, s);
    // The band is painted behind the content once its height is known.
    let band = ui.painter().add(egui::Shape::Noop);
    let pane = ui.max_rect();
    let start = ui.cursor().min.y;
    let first = start - pane.top() < 1.0;
    let band_top = if first {
        pane.top() - super::view::PANE_MARGIN_Y
    } else {
        start - 8.0
    };
    // Pull the row up so the padding is the same above and below it in the
    // band.
    let row_h = HEADER_ROW_H;
    ui.add_space(band_top + (super::view::BAND_H - row_h) / 2.0 - start);
    // A row of a set height, so the buttons and status are centred on it.
    // The row reaches out over the pane's margin, so the tile and buttons sit
    // close to the band's sides, about as close as the tile is to its top.
    let out_by = super::view::PANE_MARGIN_X - HEADER_SIDE_INSET;
    let wide = egui::Rect::from_min_size(
        egui::pos2(pane.left() - out_by, ui.cursor().min.y),
        egui::vec2(pane.width() + 2.0 * out_by, row_h),
    );
    let mut row_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(wide)
            .layout(Layout::right_to_left(Align::Center)),
    );
    {
        let ui = &mut row_ui;
        header_actions(ui, app, out, t, s, pinned, incognito, now);
        if incognito {
            tag(ui, "Incognito", t.accent, t.accent_soft);
        }
        let (colour, glow) = match kind {
            StatusKind::Idle => (t.fg3, false),
            StatusKind::Working => (t.accent, true),
            StatusKind::Waiting => (t.warn, true),
        };
        let label = ui.label(RichText::new(&status).size(12.5).color(t.band_ink2()));
        #[cfg(test)]
        {
            app.chat_ui.probe.status_rect = Some(label.rect);
        }
        let dot = egui::Rect::from_center_size(
            egui::pos2(label.rect.left() - 10.0, label.rect.center().y),
            egui::vec2(9.0, 9.0),
        );
        ui.add_space(14.0);
        if glow {
            ui.painter()
                .circle_filled(dot.center(), 7.0, colour.gamma_multiply(0.25));
        }
        ui.painter().circle_filled(dot.center(), 4.5, colour);
        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
            title_block(ui, app, out, t, s, &shown);
        });
    }
    ui.add_space(row_h);
    #[cfg(test)]
    {
        app.chat_ui.probe.status_label = Some(status.clone());
    }
    // As tall as the sidebar's band, so the two line up.
    let bottom = band_top + super::view::BAND_H;
    ui.add_space((bottom - ui.cursor().min.y).max(0.0));
    // Reach over the pane's margins; with nothing above the header the band
    // starts at the top edge of the pane.
    let top = band_top;
    let rect = egui::Rect::from_min_max(
        egui::pos2(pane.left() - super::view::PANE_MARGIN_X, top),
        egui::pos2(pane.right() + super::view::PANE_MARGIN_X, bottom),
    );
    ui.painter().set(
        band,
        egui::Shape::Vec(vec![
            egui::Shape::rect_filled(rect, 0.0, t.head_band()),
            egui::Shape::hline(
                rect.x_range(),
                rect.bottom(),
                egui::Stroke::new(1.0, t.line),
            ),
        ]),
    );
    #[cfg(test)]
    {
        app.chat_ui.probe.header = Some(rect);
    }
    ui.add_space(12.0);
}

/// The kind-of-session tile, the title (or the rename field) and, for an
/// agent, its folder.
fn title_block(
    ui: &mut egui::Ui,
    app: &mut AppState,
    out: &mut Vec<BackendRequest>,
    t: &Tokens,
    s: &Session,
    shown: &str,
) {
    let agent = s.mode == SessionMode::Agent;
    let (tile, _) = ui.allocate_exact_size(egui::vec2(TILE_SIZE, TILE_SIZE), egui::Sense::hover());
    // A quiet tile with the sidebar divider's darker edge to set it off.
    ui.painter().rect(
        tile,
        11.0,
        t.surface,
        egui::Stroke::new(1.0, t.side_edge()),
        egui::StrokeKind::Inside,
    );
    // The glyph is large in its tile, with little margin round it.
    crate::icons::paint_scaled(
        ui.painter(),
        tile,
        if agent { Icon::Agent } else { Icon::Chat },
        t.fg2,
        TILE_ICON_SCALE,
    );
    ui.add_space(4.0);
    if app
        .chat_ui
        .renaming
        .as_ref()
        .is_some_and(|(id, _)| *id == s.id)
    {
        title_or_rename(ui, app, out, t, s, shown);
        return;
    }
    let untitled = shown.trim().is_empty() || shown == roost_core::session::UNTITLED;
    let text = if untitled {
        "Untitled".to_string()
    } else {
        shown.to_string()
    };
    let font = semibold(18.0);
    let natural = ui
        .painter()
        .layout_no_wrap(text.clone(), font.clone(), t.fg)
        .size()
        .x;
    let path = s
        .workspace
        .as_ref()
        .filter(|_| agent)
        .map(|p| (home_relative(p), p.display().to_string()));
    // The title over the folder, in one column beside the tile; the folder
    // has the column's whole width because it is not beside the title.
    let mut rename = false;
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        // What is left of the row, less the pencil that follows the title.
        let room = (ui.available_width() - PENCIL_W - 8.0).max(40.0);
        ui.horizontal(|ui| {
            ui.scope(|ui| {
                ui.set_max_width(room.min(natural));
                let mut rich =
                    RichText::new(&text)
                        .font(font)
                        .color(if untitled { t.fg3 } else { t.fg });
                if untitled {
                    rich = rich.italics();
                }
                // Double-clicking the title edits it too.
                let label = ui.add(
                    egui::Label::new(rich)
                        .truncate()
                        .sense(egui::Sense::click()),
                );
                #[cfg(test)]
                {
                    app.chat_ui.probe.title_rect = Some(label.rect);
                }
                rename |= label.double_clicked();
            });
            // A small pencil beside the title, quiet until pointed at.
            let (pencil, resp) =
                ui.allocate_exact_size(egui::vec2(PENCIL_W, PENCIL_W), egui::Sense::click());
            let resp = resp
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text("Rename");
            if resp.hovered() {
                ui.painter().rect_filled(pencil, 6.0, t.surface);
            }
            crate::icons::paint(
                ui.painter(),
                pencil,
                Icon::Edit,
                if resp.hovered() { t.fg } else { t.fg2 },
            );
            #[cfg(test)]
            {
                app.chat_ui.probe.rename_button = Some(pencil);
            }
            rename |= resp.clicked();
        });
        if let Some((short, full)) = path {
            let room = ui.available_width().max(0.0);
            let cut_to = crate::widgets::ellipsize_middle(ui, &short, mono(PATH_FONT), room);
            let cut = cut_to != short;
            let r = ui
                .horizontal(|ui| {
                    ui.add_space(PATH_INSET);
                    ui.label(
                        RichText::new(cut_to)
                            .font(mono(PATH_FONT))
                            .color(t.band_ink2()),
                    )
                })
                .inner;
            #[cfg(test)]
            {
                app.chat_ui.probe.path_rect = Some(r.rect);
            }
            // Always say where it is on hover; the full path is the point
            // when it was cut.
            r.on_hover_text(if cut {
                full
            } else {
                format!("The agent works in {full}")
            });
        }
    });
    if rename {
        app.chat_ui.renaming = Some((s.id, shown.to_string()));
    }
}

fn title_or_rename(
    ui: &mut egui::Ui,
    app: &mut AppState,
    out: &mut Vec<BackendRequest>,
    _t: &Tokens,
    s: &Session,
    _shown: &str,
) {
    if let Some((id, mut text)) = app.chat_ui.renaming.clone().filter(|(id, _)| *id == s.id) {
        let edit = ui.add(
            egui::TextEdit::singleline(&mut text)
                .desired_width(ui.available_width())
                .font(semibold(18.0)),
        );
        if !edit.has_focus() && !edit.lost_focus() {
            edit.request_focus();
        }
        let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
        let escape = ui.input(|i| i.key_pressed(Key::Escape));
        if escape {
            app.chat_ui.renaming = None;
        } else if enter || edit.lost_focus() {
            // Enter, or clicking away, keeps the new name.
            if !text.trim().is_empty() {
                out.push(BackendRequest::Chat(ChatCommand::Rename(id, text.clone())));
            }
            app.chat_ui.renaming = None;
        } else {
            app.chat_ui.renaming = Some((id, text));
        }
    }
}

/// Pin and delete, as icons: a pin that fills when the session is pinned, and
/// a red trash can that asks twice (the second time it says so; the chat can
/// then still be undone for a few seconds).
#[allow(clippy::too_many_arguments)]
fn header_actions(
    ui: &mut egui::Ui,
    app: &mut AppState,
    out: &mut Vec<BackendRequest>,
    t: &Tokens,
    s: &Session,
    pinned: bool,
    incognito: bool,
    now: Instant,
) {
    // Right to left: the trash can sits at the far edge, pin next to it.
    let key = s.id.to_string();
    let armed = app.chat_ui.confirm.is_armed(&key, now);
    let ready = app.chat_ui.confirm.ready(&key, now);
    let del = if armed {
        let label = "Click again to delete";
        let galley = ui.painter().layout_no_wrap(
            label.to_string(),
            egui::FontId::proportional(12.5),
            egui::Color32::WHITE,
        );
        let (rect, resp) = ui.allocate_exact_size(
            egui::vec2(galley.size().x + 24.0, HEADER_BTN),
            egui::Sense::click(),
        );
        // Dim until a click would confirm, so it never looks ready when a
        // click would be ignored.
        let dim = if ready { 1.0 } else { 0.5 };
        ui.painter()
            .rect_filled(rect, 8.0, t.bad.gamma_multiply(dim));
        ui.painter().galley(
            rect.center() - galley.size() / 2.0,
            galley,
            egui::Color32::WHITE,
        );
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(100));
        resp
    } else {
        let (rect, resp) =
            ui.allocate_exact_size(egui::vec2(HEADER_BTN, HEADER_BTN), egui::Sense::click());
        let resp = resp
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text("Delete this session…");
        ui.painter().rect(
            rect,
            8.0,
            if resp.hovered() {
                t.bad_soft
            } else {
                t.surface
            },
            egui::Stroke::new(1.0, t.bad.gamma_multiply(0.6)),
            egui::StrokeKind::Inside,
        );
        crate::icons::paint(ui.painter(), rect, Icon::Trash, t.bad);
        resp
    };
    #[cfg(test)]
    {
        let label = if armed {
            "Click again to delete"
        } else {
            "Delete…"
        };
        app.chat_ui.probe.delete_button = Some((del.rect, label.to_string()));
    }
    if del.clicked() && app.chat_ui.confirm.click(&key, now) {
        let title = app
            .chat
            .rows()
            .iter()
            .find(|r| r.id == s.id)
            .map_or_else(|| s.title.clone(), super::state::display_title);
        super::view::delete_session(app, out, s.id, &title, !incognito, now);
    }
    if !incognito {
        let pin = icon_button(
            ui,
            t,
            Icon::Pin,
            if pinned {
                "Unpin"
            } else {
                "Pin to the top of the list"
            },
        );
        if pinned {
            // Filled and in the accent while pinned.
            ui.painter()
                .rect_filled(pin.rect.shrink(1.0), 7.0, t.accent_soft);
            crate::icons::paint(ui.painter(), pin.rect, Icon::Pin, t.accent);
        }
        #[cfg(test)]
        {
            app.chat_ui.probe.pin_button = Some(pin.rect);
        }
        if pin.clicked() {
            out.push(BackendRequest::Chat(ChatCommand::SetPinned(s.id, !pinned)));
        }
    }
}

/// "+3 −1" in the colours of the diff.
fn counts(ui: &mut egui::Ui, t: &Tokens, added: u32, removed: u32) {
    ui.label(
        RichText::new(format!("+{added}"))
            .font(mono(12.0))
            .color(t.ok),
    );
    ui.label(
        RichText::new(format!("−{removed}"))
            .font(mono(12.0))
            .color(crate::app::bad(ui.visuals().dark_mode)),
    );
}

/// Under a group of tool calls: one row for each file those calls changed,
/// with its diff on request and a way to take the change back.
pub(super) fn change_rows(
    ui: &mut egui::Ui,
    t: &Tokens,
    app: &mut AppState,
    out: &mut Vec<BackendRequest>,
    s: &Session,
    msgs: &[Message],
) {
    let running = app.chat.is_running(s.id);
    for rec in s
        .changes
        .iter()
        .filter(|c| msgs.iter().any(|m| m.id == c.message))
    {
        let key = (s.id, rec.id);
        let open = app.chat_ui.diffs_open.contains(&key);
        let mut toggle = false;
        let mut undo = false;
        let frame = egui::Frame::new()
            .fill(t.surface2)
            .stroke(egui::Stroke::new(1.0, t.line))
            .corner_radius(8.0)
            .inner_margin(egui::Margin::symmetric(10, 8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing = egui::vec2(8.0, 6.0);
                ui.horizontal_wrapped(|ui| {
                    let what = if rec.created { "Created" } else { "Changed" };
                    ui.label(RichText::new(what).color(t.fg2));
                    ui.label(RichText::new(&rec.path).font(mono(12.5)).color(t.fg));
                    counts(ui, t, rec.added, rec.removed);
                });
                ui.label(
                    RichText::new(if rec.undone {
                        "Undone: the original was put back."
                    } else {
                        "The original was kept before the change."
                    })
                    .size(11.5)
                    .color(t.fg3),
                );
                ui.horizontal_wrapped(|ui| {
                    let show = secondary(ui, if open { "Hide diff" } else { "Show diff" }, true);
                    toggle = show.clicked();
                    #[cfg(test)]
                    {
                        app.chat_ui
                            .probe
                            .show_diff_buttons
                            .push((rec.id, show.rect));
                    }
                    if !rec.undone {
                        let b = secondary(ui, "Undo this change", !running);
                        undo = b.clicked();
                        #[cfg(test)]
                        {
                            app.chat_ui.probe.undo_buttons.push((rec.id, b.rect));
                        }
                    }
                });
                if open {
                    super::view::diff_box(ui, t, &rec.preview);
                }
            });
        #[cfg(test)]
        {
            app.chat_ui
                .probe
                .change_rows
                .push((rec.id, frame.response.rect));
        }
        let _ = frame;
        if toggle {
            if open {
                app.chat_ui.diffs_open.remove(&key);
            } else {
                app.chat_ui.diffs_open.insert(key);
            }
        }
        if undo {
            out.push(BackendRequest::Chat(ChatCommand::UndoChange {
                session: s.id,
                change: rec.id,
            }));
        }
        ui.add_space(6.0);
    }
}

/// The Changes made card of the agent settings: every file touched, and one
/// button that takes them all back, newest first.
#[cfg_attr(not(test), allow(unused_variables))]
fn changes_made(
    ui: &mut egui::Ui,
    app: &mut AppState,
    out: &mut Vec<BackendRequest>,
    t: &Tokens,
    s: &Session,
    running: bool,
) {
    let open_count = s.changes.iter().filter(|c| !c.undone).count();
    settings_card(
        ui,
        t,
        "Changes made",
        &format!("{open_count} of {} still in place", s.changes.len()),
        |ui| {
            for rec in &s.changes {
                ui.horizontal(|ui| {
                    let room = (ui.available_width() - 110.0).max(60.0);
                    let shown = crate::widgets::ellipsize_middle(ui, &rec.path, mono(12.0), room);
                    let colour = if rec.undone { t.fg3 } else { t.fg };
                    ui.label(RichText::new(shown).font(mono(12.0)).color(colour));
                    if rec.undone {
                        ui.label(RichText::new("undone").size(11.5).color(t.fg3));
                    } else {
                        counts(ui, t, rec.added, rec.removed);
                    }
                });
            }
            let all = secondary(ui, "Undo all", open_count > 0 && !running);
            #[cfg(test)]
            {
                app.chat_ui.probe.undo_all_button = Some(all.rect);
            }
            if all.clicked() {
                out.push(BackendRequest::Chat(ChatCommand::UndoAll(s.id)));
            }
        },
    );
}

/// What a session has a capability set to, never above its maximum.
fn cap_permission(cfg: &roost_core::session::EffectiveToolConfig, cap: Capability) -> Permission {
    registry::specs()
        .iter()
        .find(|sp| sp.capability == cap)
        .map_or(Permission::Off, |sp| registry::permission_of(cfg, sp))
}

/// How long a press counts as a long one.
const LONG_PRESS_SECS: f64 = 0.5;

/// Whether a press that began at `press_start` has been held long enough.
pub(super) fn held_long(press_start: Option<f64>, now: f64) -> bool {
    press_start.is_some_and(|start| now - start >= LONG_PRESS_SECS)
}

/// What a press on a pill was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Press {
    Nothing,
    Click,
    Long,
}

/// Tells a click from a long press on `resp`. A long press fires once while
/// the button is still down, and the click that ends it is dropped.
fn press_of(ui: &egui::Ui, resp: &egui::Response) -> Press {
    let flag = resp.id.with("long-press");
    let fired = ui
        .memory(|m| m.data.get_temp::<bool>(flag))
        .unwrap_or(false);
    let down = resp.is_pointer_button_down_on();
    let press = if down && !fired {
        let (start, now) = ui.input(|i| (i.pointer.press_start_time(), i.time));
        if held_long(start, now) {
            ui.memory_mut(|m| m.data.insert_temp(flag, true));
            Press::Long
        } else {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
            Press::Nothing
        }
    } else if resp.clicked() && !fired {
        Press::Click
    } else {
        Press::Nothing
    };
    if !down && fired {
        ui.memory_mut(|m| m.data.remove::<bool>(flag));
    }
    press
}

/// The next permission when a pill is clicked: Off, Ask, then Allow, and back
/// to Off. Running commands never cycles to Allow; that takes a long press.
pub(super) fn next_permission(cap: Capability, current: Permission) -> Permission {
    match (current, cap) {
        (Permission::Off, _) => Permission::Ask,
        (Permission::Ask, Capability::Run | Capability::Fetch) => Permission::Off,
        (Permission::Ask, _) => Permission::Auto,
        (Permission::Auto, _) => Permission::Off,
    }
}

/// What a small pill under the input shows.
pub(super) enum PillBody<'a> {
    Icon(Icon),
    IconText(Icon, &'a str),
}

/// Whether an agent session can use tools yet, from what is known of its model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolsGate {
    /// No model is chosen.
    NoModel,
    /// The model's capabilities have not come back yet.
    Checking,
    /// The model has no tool support: the session is chat only.
    ChatOnly,
    Ready,
}

/// The model a session uses now: the list's view of it, else the session's own.
pub(super) fn model_in_use(app: &AppState, s: &Session) -> String {
    app.chat
        .rows()
        .iter()
        .find(|r| r.id == s.id)
        .map_or_else(|| s.model.clone(), |r| r.model.clone())
}

pub(super) fn tools_gate(app: &AppState, s: &Session) -> ToolsGate {
    let model = model_in_use(app, s);
    if model.is_empty() {
        return ToolsGate::NoModel;
    }
    match app.chat.supports_tools(&model) {
        None => ToolsGate::Checking,
        Some(false) => ToolsGate::ChatOnly,
        Some(true) => ToolsGate::Ready,
    }
}

/// Height of the pills under the input.
pub(super) const PILL_H: f32 = 26.0;

/// A small rounded button under the input. `selected` tints it (a popup is
/// open). Returns its rectangle and response.
pub(super) fn pill(
    ui: &mut egui::Ui,
    t: &Tokens,
    body: PillBody<'_>,
    enabled: bool,
    selected: bool,
) -> (egui::Rect, egui::Response) {
    pill_tinted(ui, t, body, enabled, selected, None)
}

/// A pill whose icon and text are drawn in `tint` instead of the usual colour.
pub(super) fn pill_tinted(
    ui: &mut egui::Ui,
    t: &Tokens,
    body: PillBody<'_>,
    enabled: bool,
    selected: bool,
    tint: Option<egui::Color32>,
) -> (egui::Rect, egui::Response) {
    let galley = match &body {
        PillBody::Icon(_) => None,
        PillBody::IconText(_, text) => Some(ui.painter().layout_no_wrap(
            (*text).to_string(),
            egui::FontId::proportional(12.0),
            t.fg2,
        )),
    };
    let width = match &galley {
        None => 30.0,
        Some(g) => 12.0 + 16.0 + 5.0 + g.size().x + 10.0,
    };
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, PILL_H), sense);
    let resp = if enabled {
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        resp
    };
    let fill = if selected {
        t.accent_soft
    } else if resp.hovered() && enabled {
        t.surface
    } else {
        t.surface2
    };
    ui.painter().rect(
        rect,
        PILL_H / 2.0,
        fill,
        egui::Stroke::new(1.0, t.line),
        egui::StrokeKind::Inside,
    );
    let colour = tint.unwrap_or(if enabled { t.fg2 } else { t.fg3 });
    match (body, galley) {
        (PillBody::Icon(icon), _) => {
            crate::icons::paint(ui.painter(), rect, icon, colour);
        }
        (PillBody::IconText(icon, _), Some(g)) => {
            let at = egui::pos2(rect.left() + 12.0 + 8.0, rect.center().y);
            crate::icons::paint(
                ui.painter(),
                egui::Rect::from_center_size(at, egui::vec2(16.0, 16.0)),
                icon,
                colour,
            );
            ui.painter().galley(
                egui::pos2(
                    rect.left() + 12.0 + 16.0 + 5.0,
                    rect.center().y - g.size().y / 2.0,
                ),
                g,
                colour,
            );
        }
        _ => {}
    }
    (rect, resp)
}

/// What choosing `path` means for this chat: the folder a waiting call needs,
/// or a new working folder for an idle agent.
pub(super) fn folder_choice(
    chat: &super::state::ChatState,
    id: roost_core::session::SessionId,
    path: std::path::PathBuf,
) -> ChatCommand {
    match chat.folder_wait(id) {
        Some(wait) => ChatCommand::ProvideWorkspace {
            session: id,
            call_id: wait.call_id.clone(),
            path: Some(path),
        },
        None => ChatCommand::SetWorkspace(id, path),
    }
}

/// Asks for a folder and sends it as the agent's working folder, or as the
/// folder a paused call is waiting for.
pub(super) fn choose_folder(
    app: &mut AppState,
    out: &mut Vec<BackendRequest>,
    id: roost_core::session::SessionId,
) {
    if let Some(path) = rfd::FileDialog::new()
        .set_title("Choose the agent's folder")
        .pick_folder()
    {
        out.push(BackendRequest::Chat(folder_choice(&app.chat, id, path)));
        app.chat.folder_answered(id);
    }
}

/// The `+` under the input: what can be added to this chat.
pub(super) fn plus_menu(
    ui: &mut egui::Ui,
    app: &mut AppState,
    out: &mut Vec<BackendRequest>,
    t: &Tokens,
    s: &Session,
) {
    let running = app.chat.is_running(s.id);
    let tools_ready = tools_gate(app, s) == ToolsGate::Ready;
    let (_, resp) = pill(ui, t, PillBody::Icon(Icon::Plus), true, false);
    let resp = resp.on_hover_text("Add to this chat");
    #[cfg(test)]
    {
        app.chat_ui.probe.plus_button = Some(resp.rect);
    }
    egui::Popup::from_toggle_button_response(&resp)
        .align(egui::RectAlign::TOP_START)
        .close_behavior(PopupCloseBehavior::CloseOnClick)
        .show(|ui| {
            ui.set_min_width(200.0);
            if s.mode == SessionMode::Agent {
                let can_pick = tools_ready && (!running || app.chat.folder_wait(s.id).is_some());
                if ui
                    .add_enabled(can_pick, egui::Button::new("Working folder…").frame(false))
                    .clicked()
                {
                    choose_folder(app, out, s.id);
                }
                ui.label(
                    RichText::new("Files and images: not yet")
                        .size(11.5)
                        .color(t.fg3),
                );
            } else {
                ui.label(
                    RichText::new("Nothing to add to a chat yet")
                        .size(12.0)
                        .color(t.fg3),
                );
            }
        });
}

/// The agent's folder as a pill under the input; clicking it picks another.
pub(super) fn folder_pill(
    ui: &mut egui::Ui,
    app: &mut AppState,
    out: &mut Vec<BackendRequest>,
    t: &Tokens,
    s: &Session,
    compact: bool,
) {
    // A paused call can be answered with a folder even while the run waits.
    let waiting = app.chat.folder_wait(s.id).is_some();
    let running = app.chat.is_running(s.id) && !waiting;
    let full = s
        .workspace
        .as_ref()
        .map_or_else(|| "No folder".to_string(), |p| home_relative(p));
    let shown =
        crate::widgets::ellipsize_middle(ui, &full, egui::FontId::proportional(12.0), 220.0);
    let body = if compact {
        PillBody::Icon(Icon::Folder)
    } else {
        PillBody::IconText(Icon::Folder, &shown)
    };
    let (_, resp) = pill(ui, t, body, !running, false);
    #[cfg(test)]
    {
        app.chat_ui.probe.folder_pill = Some(resp.rect);
    }
    let hint = if s.workspace.is_some() {
        format!("{full}\nClick to choose another folder")
    } else {
        "No working directory is attached\nClick to choose a folder".to_string()
    };
    let resp = resp.on_hover_text(hint);
    if resp.clicked() && !running {
        choose_folder(app, out, s.id);
    }
}

/// The R, W and X pills under the input: what the agent may do, at a glance,
/// and the quickest way to change it. A click steps Off, Ask, Allow (running
/// commands steps Off and Ask only); a long press picks Allow. Read is accent
/// when allowed, write is amber when it asks, anything off is dimmed. No W
/// when writing is off in Settings, no X until commands are switched on.
pub(super) fn permission_pills(
    ui: &mut egui::Ui,
    app: &mut AppState,
    out: &mut Vec<BackendRequest>,
    t: &Tokens,
    s: &Session,
) {
    let dark = ui.visuals().dark_mode;
    let running = app.chat.is_running(s.id);
    #[cfg(test)]
    {
        app.chat_ui.probe.chips.clear();
    }
    // Right-to-left: X is placed first, so W and then R end up on its left.
    let mut chips = Vec::new();
    // The web: always there, off until turned on. It cycles Off and Ask only;
    // letting every page through is for the tools list.
    chips.push((Capability::Fetch, Family::Web, 'N', "Fetching web pages"));
    if app.settings.agents_can_run {
        chips.push((Capability::Run, Family::Shell, 'X', "Running commands"));
    }
    if app.settings.agents_can_write {
        chips.push((Capability::Write, Family::Files, 'W', "Writing files"));
    }
    chips.push((Capability::Read, Family::Files, 'R', "Reading files"));
    for (cap, family, letter, what) in chips {
        let perm = cap_permission(&s.effective_tools, cap);
        // One hue for every state, told apart by the shape: solid is allowed,
        // outlined asks first, dashed is off. Running commands is red when it
        // is allowed, never the calm colour.
        let danger = cap == Capability::Run && perm == Permission::Auto;
        let hue = if danger {
            crate::app::bad(dark)
        } else {
            t.accent
        };
        let (fg, fill, stroke) = match perm {
            Permission::Auto => (if danger { t.surface } else { t.accent_fg }, hue, hue),
            Permission::Ask => (hue, t.surface, hue),
            Permission::Off => (t.fg3, t.surface, t.line),
        };
        let sense = if running {
            egui::Sense::hover()
        } else {
            egui::Sense::click()
        };
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(28.0, PILL_H), sense);
        let width = 1.5;
        ui.painter().rect_filled(rect, PILL_H / 2.0, fill);
        let outline = rect.shrink(width / 2.0);
        if perm == Permission::Off {
            // Dashes round the pill: short strokes along a circle-ended rect.
            dashed_pill(ui.painter(), outline, stroke, width);
        } else {
            ui.painter().rect_stroke(
                outline,
                PILL_H / 2.0,
                egui::Stroke::new(width, stroke),
                egui::StrokeKind::Inside,
            );
        }
        if cap == Capability::Fetch {
            // A globe, not a letter.
            crate::icons::paint(ui.painter(), rect, Icon::Web, fg);
        } else {
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                letter,
                semibold(12.0),
                fg,
            );
        }
        let state = match perm {
            Permission::Auto => "allowed without asking",
            Permission::Ask => "asks first",
            Permission::Off => "off",
        };
        let how = if running {
            "locked while a run is active"
        } else if cap == Capability::Fetch {
            "click to switch off or ask"
        } else if cap == Capability::Run {
            "click to switch off or ask, hold to allow without asking"
        } else {
            "click to change, hold to allow without asking"
        };
        let resp = resp
            .on_hover_cursor(if running {
                egui::CursorIcon::Default
            } else {
                egui::CursorIcon::PointingHand
            })
            .on_hover_text(format!("{what}: {state} ({how})"));
        #[cfg(test)]
        {
            app.chat_ui.probe.chips.push((letter, rect, perm));
        }
        if running {
            continue;
        }
        let key = registry::capability_key(family, cap);
        match press_of(ui, &resp) {
            Press::Nothing => {}
            Press::Click => {
                app.chat_ui.run_allow_pending = None;
                app.chat_ui.run_allow_from_pill = false;
                out.push(BackendRequest::Chat(ChatCommand::SetCapability(
                    s.id,
                    key,
                    next_permission(cap, perm),
                )));
            }
            Press::Long if cap == Capability::Run => {
                // Running commands without asking is confirmed once.
                if perm != Permission::Auto {
                    app.chat_ui.run_allow_pending = Some(s.id);
                    app.chat_ui.run_allow_from_pill = true;
                }
            }
            // The web is never allowed wholesale from the pill.
            Press::Long if cap == Capability::Fetch => {}
            Press::Long => {
                if perm != Permission::Auto {
                    out.push(BackendRequest::Chat(ChatCommand::SetCapability(
                        s.id,
                        key,
                        Permission::Auto,
                    )));
                }
            }
        }
    }
    #[cfg(test)]
    {
        app.chat_ui
            .probe
            .chips
            .sort_by(|a, b| a.1.left().total_cmp(&b.1.left()));
    }
}

/// The once-only confirmation for allowing commands without asking, above the
/// input while it is pending.
pub(super) fn run_allow_banner(
    ui: &mut egui::Ui,
    app: &mut AppState,
    out: &mut Vec<BackendRequest>,
    t: &Tokens,
    id: roost_core::session::SessionId,
) {
    if app.chat_ui.run_allow_pending == Some(id) && app.chat_ui.run_allow_from_pill {
        run_allow_confirm(ui, app, out, t, id, Family::Shell);
        ui.add_space(8.0);
    }
}

/// A card inside the agent settings: a small heading over a rounded panel.
fn settings_card<R>(
    ui: &mut egui::Ui,
    t: &Tokens,
    title: &str,
    note: &str,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    egui::Frame::new()
        .fill(t.surface2)
        .stroke(egui::Stroke::new(1.0, t.line))
        .corner_radius(9.0)
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 8.0;
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(title.to_uppercase())
                        .font(semibold(11.0))
                        .color(t.fg3),
                );
                if !note.is_empty() {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(RichText::new(note).size(11.5).color(t.fg3));
                    });
                }
            });
            add(ui)
        })
        .inner
}

/// The agent's folder, step budget and tools, behind one button that says
/// so.
pub(super) fn agent_settings(
    ui: &mut egui::Ui,
    app: &mut AppState,
    out: &mut Vec<BackendRequest>,
    t: &Tokens,
    s: &Session,
) {
    let running = app.chat.is_running(s.id);
    let id = s.id;
    let gate = tools_gate(app, s);
    #[cfg(test)]
    {
        app.chat_ui.probe.tools_gate = Some(gate);
        app.chat_ui.probe.tools_icon_red = Some(gate == ToolsGate::ChatOnly);
    }
    if gate != ToolsGate::Ready {
        // Not usable yet: dimmed while there is nothing to go on, red when the
        // model is known not to support tools. Either way it says why.
        let red = gate == ToolsGate::ChatOnly;
        let (rect, resp) = pill_tinted(
            ui,
            t,
            PillBody::Icon(Icon::Tools),
            red,
            false,
            red.then_some(t.bad),
        );
        #[cfg(test)]
        {
            app.chat_ui.probe.agent_chip = Some(rect);
        }
        let _ = rect;
        let why = match gate {
            ToolsGate::NoModel => "Tools are off until you choose a model",
            ToolsGate::Checking => "Checking whether this model supports tools…",
            _ => "This session is effectively chat only: the model does not support tools",
        };
        let resp = resp.on_hover_text(why);
        if red {
            egui::Popup::from_toggle_button_response(&resp)
                .align(egui::RectAlign::TOP_START)
                .close_behavior(PopupCloseBehavior::CloseOnClickOutside)
                .show(|ui| {
                    ui.set_max_width(280.0);
                    ui.label(why);
                });
        }
        return;
    }
    let (rect, resp) = pill(ui, t, PillBody::Icon(Icon::Tools), true, false);
    let resp = resp.on_hover_text(
        "Tools and settings: the folder this agent works in, its step budget and its tools",
    );
    egui::Popup::from_toggle_button_response(&resp)
        .align(egui::RectAlign::TOP_START)
        .close_behavior(PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            ui.set_width(340.0);
            ui.spacing_mut().item_spacing.y = 10.0;
            // The cards can be taller than the room above the button, so the
            // popover scrolls instead of running off the top of the window.
            let room = (rect.top() - ui.ctx().content_rect().top() - 24.0).max(200.0);
            ui.set_max_height(room);
            egui::ScrollArea::vertical()
                .max_height(room)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    // Every tool and its settings in one window; first, so it is
                    // in reach however long the cards below are.
                    let all = secondary(ui, "All tools…", true);
                    #[cfg(test)]
                    {
                        app.chat_ui.probe.tool_list_button = Some(all.rect);
                    }
                    if all.clicked() {
                        app.chat_ui.tool_list_open = true;
                        // The window is the way on, and the popover would cover it.
                        ui.close();
                    }
                    settings_card(ui, t, "Folder", "the agent works here", |ui| {
                        let path = s
                            .workspace
                            .as_ref()
                            .map_or(String::new(), |p| p.display().to_string());
                        ui.add(
                            egui::Label::new(RichText::new(path).font(mono(12.0)).color(t.fg2))
                                .wrap(),
                        );
                        if secondary(ui, "Change…", !running).clicked() {
                            if let Some(path) = rfd::FileDialog::new()
                                .set_title("Choose the agent's folder")
                                .pick_folder()
                            {
                                out.push(BackendRequest::Chat(ChatCommand::SetWorkspace(id, path)));
                            }
                        }
                    });
                    settings_card(ui, t, "Run", "", |ui| {
                        ui.add_enabled_ui(!running, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new("Step budget").size(12.5).color(t.fg2));
                                let mut steps = s.step_budget;
                                if ui
                                    .add(egui::DragValue::new(&mut steps).range(1..=100))
                                    .changed()
                                {
                                    out.push(BackendRequest::Chat(ChatCommand::SetStepBudget(
                                        id, steps,
                                    )));
                                }
                            });
                        });
                    });
                    #[cfg(test)]
                    {
                        app.chat_ui.probe.permission_buttons.clear();
                        app.chat_ui.probe.write_buttons.clear();
                        app.chat_ui.probe.run_buttons.clear();
                        app.chat_ui.probe.run_confirm = None;
                        app.chat_ui.probe.tools_heads.clear();
                        app.chat_ui.probe.tool_option_values.clear();
                        app.chat_ui.probe.tool_option_resets.clear();
                    }
                    for family in [Family::Files, Family::Shell, Family::Web, Family::Ask] {
                        if family == Family::Shell && !app.settings.agents_can_run {
                            continue;
                        }
                        family_card(ui, app, out, t, s, family, running);
                    }
                    if !s.changes.is_empty() {
                        changes_made(ui, app, out, t, s, running);
                    }
                    if running {
                        ui.label(
                            RichText::new("Locked while a run is active.")
                                .size(11.5)
                                .color(t.fg3),
                        );
                    }
                });
        });
    #[cfg(test)]
    {
        app.chat_ui.probe.agent_chip = Some(rect);
    }
}

/// One card per family of tools: a Off / Ask / Allow row for each ability the
/// family has, then its tools, each with a switch. The Shell family is only
/// drawn when commands are switched on in Settings.
fn family_card(
    ui: &mut egui::Ui,
    app: &mut AppState,
    out: &mut Vec<BackendRequest>,
    t: &Tokens,
    s: &Session,
    family: Family,
    running: bool,
) {
    let id = s.id;
    let writes = app.settings.agents_can_write;
    let specs: Vec<_> = registry::specs()
        .into_iter()
        .filter(|sp| sp.family == family && (writes || sp.capability != Capability::Write))
        .collect();
    let on_count = specs
        .iter()
        .filter(|sp| registry::tool_enabled(&s.effective_tools, sp))
        .count();
    let note = format!("{on_count} of {} tools on", specs.len());
    settings_card(ui, t, family.label(), &note, |ui| {
        ui.add_enabled_ui(!running, |ui| {
            for cap in [
                Capability::Read,
                Capability::Write,
                Capability::Run,
                Capability::Fetch,
            ] {
                if !specs.iter().any(|sp| sp.capability == cap) {
                    continue;
                }
                let (title, sub, tips): (&str, &str, &[(&str, &str)]) = match cap {
                    // Asking you needs no permission: only the tool's own switch.
                    Capability::Ask => continue,
                    Capability::Read => (
                        "Read files",
                        "list, find, search, read",
                        &[
                            ("Off", "The agent cannot read files"),
                            ("Ask", "Ask before each file is read"),
                            ("Allow", "Read files without asking"),
                        ],
                    ),
                    Capability::Write => (
                        "Write files",
                        "create and change files in the folder",
                        &[
                            ("Off", "The agent cannot change files"),
                            ("Ask", "Ask before each change, with the changes shown"),
                            (
                                "Allow",
                                "Change files without asking. Each change is kept so you can undo it",
                            ),
                        ],
                    ),
                    Capability::Run => (
                        "Run commands",
                        "runs as you, not limited to the folder",
                        &[
                            ("Off", "The agent cannot run commands"),
                            ("Ask", "Ask before each command, showing exactly what will run"),
                            (
                                "Allow",
                                "Run commands without asking. They run as you and can change anything you can",
                            ),
                        ],
                    ),
                    Capability::Fetch => (
                        "Fetch web pages",
                        "reads a page from the internet",
                        &[
                            ("Off", "The agent cannot use the web"),
                            ("Ask", "Ask before each page, showing the address"),
                            (
                                "Allow",
                                "Fetch pages without asking. The text of each page goes to the model; addresses on this computer or your network are always refused",
                            ),
                        ],
                    ),
                };
                ui.label(RichText::new(title).font(semibold(13.0)).color(t.fg));
                ui.label(RichText::new(sub).size(11.5).color(t.fg3));
                let current = cap_permission(&s.effective_tools, cap);
                let chosen = match current {
                    Permission::Off => 0,
                    Permission::Ask => 1,
                    Permission::Auto => 2,
                };
                let mut rects = Vec::new();
                let picked = crate::widgets::segmented(ui, t, tips, chosen, &mut rects);
                #[cfg(test)]
                {
                    match cap {
                        Capability::Read => app.chat_ui.probe.permission_buttons = rects.clone(),
                        Capability::Write => app.chat_ui.probe.write_buttons = rects.clone(),
                        Capability::Run => app.chat_ui.probe.run_buttons = rects.clone(),
                        Capability::Ask | Capability::Fetch => {}
                    }
                }
                let _ = rects;
                if let Some(i) = picked {
                    let value = [Permission::Off, Permission::Ask, Permission::Auto][i];
                    let key = registry::capability_key(family, cap);
                    if cap == Capability::Run && value == Permission::Auto {
                        // Running commands without asking is confirmed once.
                        app.chat_ui.run_allow_pending = Some(id);
                        app.chat_ui.run_allow_from_pill = false;
                    } else {
                        if cap == Capability::Run {
                            app.chat_ui.run_allow_pending = None;
                        }
                        out.push(BackendRequest::Chat(ChatCommand::SetCapability(
                            id, key, value,
                        )));
                    }
                }
                if cap == Capability::Run
                    && app.chat_ui.run_allow_pending == Some(id)
                    && !app.chat_ui.run_allow_from_pill
                {
                    run_allow_confirm(ui, app, out, t, id, family);
                }
            }
            let slot = (id, family as u8);
            let open = app.chat_ui.tools_open.contains(&slot);
            let arrow = if open { "▾" } else { "▸" };
            let head = ui
                .add(
                    egui::Label::new(
                        RichText::new(format!(
                            "{arrow}  Individual tools · {on_count} of {} on",
                            specs.len()
                        ))
                        .size(11.5)
                        .color(t.fg3),
                    )
                    .sense(egui::Sense::click()),
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            #[cfg(test)]
            {
                app.chat_ui.probe.tools_heads.push(head.rect);
            }
            if head.clicked() {
                if open {
                    app.chat_ui.tools_open.remove(&slot);
                } else {
                    app.chat_ui.tools_open.insert(slot);
                }
            }
            if open {
                for spec in specs {
                    let mut on = registry::tool_enabled(&s.effective_tools, &spec);
                    let tag = match spec.capability {
                        Capability::Read => 'R',
                        Capability::Write => 'W',
                        Capability::Run => 'X',
                        Capability::Ask => 'Q',
                        Capability::Fetch => 'N',
                    };
                    let was_on = on;
                    ui.horizontal(|ui| {
                        if ui
                            .checkbox(&mut on, format!("{tag}   {}", spec.name))
                            .on_hover_text(spec.description)
                            .changed()
                        {
                            out.push(BackendRequest::Chat(ChatCommand::SetToolEnabled(
                                id,
                                spec.name.into(),
                                on,
                            )));
                        }
                        tool_limit_chip(ui, app, t, &s.effective_tools, spec.name, was_on);
                    });
                    if app.chat_ui.limits_open.as_deref() == Some(spec.name) {
                        tool_limit_panel(ui, app, out, t, s, spec.name, running);
                    }
                }
            }
        });
    });
}

/// The short limit beside a tool in the list. Clicking it shows or hides all
/// of that tool's limits under the row.
fn tool_limit_chip(
    ui: &mut egui::Ui,
    app: &mut AppState,
    t: &Tokens,
    config: &EffectiveToolConfig,
    name: &str,
    on: bool,
) {
    use super::tool_limits;
    let limits = registry::effective_limits(&app.settings.tools, config);
    let Some(summary) = tool_limits::summary(name, &limits) else {
        return;
    };
    let open = app.chat_ui.limits_open.as_deref() == Some(name);
    let color = if open {
        t.accent
    } else if on {
        t.fg3
    } else {
        t.fg3.gamma_multiply(0.55)
    };
    let chip = ui
        .add(
            egui::Label::new(RichText::new(summary).size(11.0).color(color))
                .sense(egui::Sense::click()),
        )
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text("This tool's limits");
    #[cfg(test)]
    app.chat_ui.probe.tool_limit_chips.push(chip.rect);
    if chip.clicked() {
        app.chat_ui.limits_open = if open { None } else { Some(name.to_string()) };
        app.chat_ui.limits_reveal = !open;
    }
}

/// All of one tool's limits, under its row, for this chat. Settings holds the ceilings.
fn tool_limit_panel(
    ui: &mut egui::Ui,
    app: &mut AppState,
    out: &mut Vec<BackendRequest>,
    t: &Tokens,
    s: &Session,
    name: &str,
    running: bool,
) {
    use super::tool_limits;
    let limits = registry::effective_limits(&app.settings.tools, &s.effective_tools);
    let top = ui.cursor().min.y;
    let panel = egui::Frame::new()
        .fill(t.surface2)
        .corner_radius(6.0)
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            for (what, value) in tool_limits::facts(name, &limits) {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(what).size(12.0).color(t.fg2));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(RichText::new(value).font(mono(12.0)).color(t.fg));
                    });
                });
            }
            tool_option_rows(ui, app, out, t, s.id, &s.effective_tools, name, running);
            ui.add_space(2.0);
            ui.add(
                egui::Label::new(
                    RichText::new(
                        "Smaller for a model with a small context window. Settings sets the ceiling; changes here apply to this chat only.",
                    )
                    .size(11.0)
                    .color(t.fg3),
                )
                .wrap(),
            );
            ui.add_space(2.0);
            let r = ui.button("Change in Settings");
            #[cfg(test)]
            {
                app.chat_ui.probe.tool_limit_settings = Some(r.rect);
            }
            if r.clicked() {
                app.select(crate::app::View::Settings);
            }
        });
    if std::mem::take(&mut app.chat_ui.limits_reveal) {
        ui.scroll_to_rect(panel.response.rect, None);
    }
    #[cfg(test)]
    {
        app.chat_ui.probe.tool_limit_popover = Some(egui::Rect::from_min_max(
            egui::pos2(ui.min_rect().left(), top),
            egui::pos2(ui.min_rect().right(), ui.cursor().min.y),
        ));
    }
    let _ = top;
    ui.add_space(4.0);
}

/// One drag value per option the tool declares, for this session, from the
/// option's `min` to the app-wide ceiling in Settings, with the value in
/// force and a Reset when the session has its own. A change is sent when the
/// control is let go of (not on every frame of a drag), and only if it
/// differs from what is stored. All of it is disabled during a run.
#[allow(clippy::too_many_arguments)]
pub(super) fn tool_option_rows(
    ui: &mut egui::Ui,
    app: &mut AppState,
    out: &mut Vec<BackendRequest>,
    t: &Tokens,
    id: roost_core::session::SessionId,
    config: &EffectiveToolConfig,
    name: &str,
    running: bool,
) {
    let Some(tool) = roost_core::agent::tool::find(name) else {
        return;
    };
    let effective = registry::effective_limits(&app.settings.tools, config);
    for decl in tool.options() {
        let key = format!("{name}.{}", decl.key);
        let ceiling = (decl.get)(&app.settings.tools).max(decl.min);
        let stored = (decl.get)(&effective).clamp(decl.min, ceiling);
        let overridden = config.settings.contains_key(&key);
        let pending = match &app.chat_ui.option_edit {
            Some((sid, k, v)) if *sid == id && *k == key => Some(*v),
            _ => None,
        };
        let mut value = pending.unwrap_or(stored);
        ui.add_enabled_ui(!running, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(decl.label).size(12.0).color(t.fg2))
                    .on_hover_text(decl.help);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let reset = overridden.then(|| ui.button("Reset"));
                    #[cfg(test)]
                    if let Some(r) = &reset {
                        app.chat_ui.probe.tool_option_resets.push(r.rect);
                    }
                    if reset.is_some_and(|r| r.clicked()) {
                        app.chat_ui.option_edit = None;
                        out.push(BackendRequest::Chat(ChatCommand::SetToolOption {
                            session: id,
                            tool: name.into(),
                            option: decl.key.into(),
                            value: None,
                        }));
                    }
                    let r = ui
                        .add(
                            egui::DragValue::new(&mut value)
                                .range(decl.min..=ceiling)
                                .speed(((ceiling - decl.min) / 200).max(1) as f64),
                        )
                        .on_hover_text(decl.help);
                    #[cfg(test)]
                    app.chat_ui.probe.tool_option_values.push(r.rect);
                    if r.changed() {
                        app.chat_ui.option_edit = Some((id, key.clone(), value));
                    }
                    if r.drag_stopped() || r.lost_focus() {
                        app.chat_ui.option_edit = None;
                        if value != stored {
                            out.push(BackendRequest::Chat(ChatCommand::SetToolOption {
                                session: id,
                                tool: name.into(),
                                option: decl.key.into(),
                                value: Some(value),
                            }));
                        }
                    }
                });
            });
        });
    }
}

/// The question after choosing Allow for Run. Cancel is the quiet default.
fn run_allow_confirm(
    ui: &mut egui::Ui,
    app: &mut AppState,
    out: &mut Vec<BackendRequest>,
    t: &Tokens,
    id: roost_core::session::SessionId,
    family: Family,
) {
    let dark = ui.visuals().dark_mode;
    ui.add(
        egui::Label::new(
            RichText::new(
                "Commands will run without asking. They run as you and can change or delete anything you can.",
            )
            .size(12.0)
            .color(crate::app::bad(dark)),
        )
        .wrap(),
    );
    ui.horizontal(|ui| {
        let cancel = ui.add(egui::Button::new(RichText::new("Cancel").size(12.5)));
        let confirm = ui.add(
            egui::Button::new(
                RichText::new("Run without asking")
                    .size(12.5)
                    .color(crate::app::bad(dark)),
            )
            .stroke(egui::Stroke::new(1.0, crate::app::bad(dark))),
        );
        #[cfg(test)]
        {
            app.chat_ui.probe.run_confirm = Some((confirm.rect, cancel.rect));
        }
        if confirm.clicked() {
            app.chat_ui.run_allow_pending = None;
            out.push(BackendRequest::Chat(ChatCommand::SetCapability(
                id,
                registry::capability_key(family, Capability::Run),
                Permission::Auto,
            )));
        } else if cancel.clicked() {
            app.chat_ui.run_allow_pending = None;
        }
    });
    let _ = t;
}

/// One line above the input: progress of a running agent, then any notice.
pub(super) fn status_line(ui: &mut egui::Ui, app: &AppState, t: &Tokens, s: &Session) {
    let mut parts: Vec<String> = Vec::new();
    if s.mode == SessionMode::Agent && app.chat.is_running(s.id) {
        parts.push(format!(
            "Step {} of {}",
            app.chat.step_of(s.id).max(1),
            s.step_budget
        ));
        let waiting = app.chat.queued(s.id).len();
        if waiting > 0 {
            parts.push(format!(
                "{waiting} message{} waiting for the next step",
                if waiting == 1 { "" } else { "s" }
            ));
        }
    }
    if let Some(n) = &app.chat.notice {
        parts.push(n.clone());
    }
    if parts.is_empty() {
        return;
    }
    let notice = app.chat.notice.is_some();
    let color = if notice { t.warn } else { t.accent };
    ui.add(egui::Label::new(RichText::new(parts.join("  ·  ")).size(12.5).color(color)).wrap());
    #[cfg(test)]
    {
        let _ = ui;
    }
}

/// How long the model chip flashes, and how many half-cycles a second.
const MODEL_FLASH_SECS: f64 = 1.0;
const MODEL_FLASH_RATE: f64 = 6.0;

/// Below this width the pills under the input shrink: the folder to its icon
/// and the model name to a few characters.
pub(super) const COMPACT_PILLS_W: f32 = 700.0;

/// The model in use, whether it is in memory, and a way to change it.
pub(super) fn model_chip(
    ui: &mut egui::Ui,
    app: &mut AppState,
    out: &mut Vec<BackendRequest>,
    t: &Tokens,
    s: &Session,
    installed: &[String],
    max_chars: usize,
) {
    let running = app.chat.is_running(s.id);
    let model = model_in_use(app, s);
    let loaded = app
        .ollama
        .as_ref()
        .is_some_and(|o| o.is_some())
        .then(|| {
            app.running
                .iter()
                .find(|m| same_model(&m.name, &model))
                .cloned()
        })
        .flatten();
    let dot = if loaded.is_some() { "● " } else { "○ " };
    let label = if model.is_empty() {
        "Choose a model".to_string()
    } else {
        format!("{dot}{}", truncate_chars(&model, max_chars))
    };
    // A Work session cannot do anything without a model: the chip is red, and
    // flashes inverted a few times when a send is refused for want of one.
    let red = model.is_empty() && s.mode == SessionMode::Agent;
    let now = ui.input(|i| i.time);
    let elapsed = app.chat_ui.model_flash.map(|at| now - at);
    if elapsed.is_some_and(|e| e >= MODEL_FLASH_SECS) {
        app.chat_ui.model_flash = None;
    }
    let inverted = red
        && app.chat_ui.model_flash.is_some()
        && elapsed.is_some_and(|e| ((e * MODEL_FLASH_RATE) as u64).is_multiple_of(2));
    if app.chat_ui.model_flash.is_some() {
        ui.ctx().request_repaint();
    }
    #[cfg(test)]
    {
        app.chat_ui.probe.model_look = Some((red, inverted));
    }
    let text = RichText::new(label).size(12.0);
    let text = match (red, inverted) {
        (true, true) => text.color(egui::Color32::WHITE),
        (true, false) => text.color(t.bad),
        _ => text,
    };
    let (chip, _) = ui
        .scope(|ui| {
            if inverted {
                let w = &mut ui.visuals_mut().widgets;
                w.inactive.weak_bg_fill = t.bad;
                w.hovered.weak_bg_fill = t.bad;
                w.active.weak_bg_fill = t.bad;
                w.inactive.bg_stroke.color = t.bad;
            } else if red {
                ui.visuals_mut().widgets.inactive.bg_stroke.color = t.bad;
            }
            MenuButton::new(text).config(sticky()).ui(ui, |ui| {
                ui.set_min_width(240.0);
                ui.label(RichText::new("Model").font(semibold(11.0)).color(t.fg3));
                ui.add_enabled_ui(!running, |ui| {
                    let agent = s.mode == SessionMode::Agent;
                    let mut names: Vec<&str> = installed.iter().map(String::as_str).collect();
                    if !model.is_empty() && !names.contains(&model.as_str()) {
                        names.insert(0, &model);
                    }
                    if agent {
                        // An agent needs tools: say which models have them, ask
                        // about the ones not known yet (once each), and list the
                        // capable ones first.
                        for name in &names {
                            if app.chat.supports_tools(name).is_none()
                                && app.chat_ui.checked_models.insert((*name).to_string())
                            {
                                out.push(BackendRequest::Chat(ChatCommand::CheckModel(
                                    (*name).to_string(),
                                )));
                            }
                        }
                        names.sort_by_key(|n| app.chat.supports_tools(n) != Some(true));
                    }
                    for name in names {
                        ui.horizontal(|ui| {
                            if ui.selectable_label(name == model, name).clicked() && name != model {
                                out.push(BackendRequest::Chat(ChatCommand::SetModel(
                                    s.id,
                                    name.into(),
                                )));
                                ui.close();
                            }
                            if agent {
                                let (tag, colour) = match app.chat.supports_tools(name) {
                                    Some(true) => ("tools", t.ok),
                                    Some(false) => ("no tools", t.fg3),
                                    None => ("checking…", t.fg3),
                                };
                                ui.label(RichText::new(tag).size(11.0).color(colour));
                            }
                        });
                    }
                });
                ui.separator();
                match &loaded {
                    Some(m) => {
                        let note = m
                            .expires_at
                            .as_deref()
                            .and_then(|e| unload_note(e, chrono::Utc::now()))
                            .unwrap_or_else(|| "loaded".to_string());
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(format!("In memory · {note}"))
                                    .size(12.0)
                                    .color(t.ok),
                            );
                            let unload = secondary(ui, "Unload", true);
                            #[cfg(test)]
                            {
                                app.chat_ui.probe.model_unload = Some(unload.rect);
                            }
                            if unload.clicked() {
                                out.push(BackendRequest::UnloadModel(m.name.clone()));
                                app.running.retain(|x| x.name != m.name);
                                ui.close();
                            }
                        });
                    }
                    None => {
                        ui.label(
                            RichText::new(
                                "Not in memory. Ollama loads it when you send a message.",
                            )
                            .size(12.0)
                            .color(t.fg3),
                        );
                    }
                }
            })
        })
        .inner;
    #[cfg(test)]
    {
        app.chat_ui.probe.model_chip = Some(chip.rect);
    }
    let _ = chip;
}

/// Consecutive tool steps folded into one line; open it for the cards.
pub(super) fn tool_group(ui: &mut egui::Ui, t: &Tokens, msgs: &[Message], app: &mut AppState) {
    let mut names: Vec<(String, usize)> = Vec::new();
    let (mut running, mut failed) = (None, 0usize);
    for m in msgs {
        if let MessageContent::Tool { name, status, .. } = &m.content {
            match names.iter_mut().find(|(n, _)| n == name) {
                Some((_, c)) => *c += 1,
                None => names.push((name.clone(), 1)),
            }
            match status {
                ToolStatus::Running => running = Some(name.clone()),
                ToolStatus::Failed => failed += 1,
                _ => {}
            }
        }
    }
    let list = names
        .iter()
        .map(|(n, c)| {
            if *c > 1 {
                format!("{n} ×{c}")
            } else {
                n.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    let count = msgs.len();
    let mut head = format!(
        "Used {count} tool{}: {list}",
        if count == 1 { "" } else { "s" }
    );
    // When the first call of the group started.
    let started = msgs.first().and_then(|m| {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64);
        super::state::local_stamp(m.created_at_ms, now)
    });
    if let Some((label, _)) = &started {
        head.push_str(&format!("  ·  {label}"));
    }
    if failed > 0 {
        head.push_str(&format!("  ·  {failed} failed"));
    }
    if let Some(n) = &running {
        head.push_str(&format!("  ·  {n} running…"));
    }
    let color = if running.is_some() {
        t.accent
    } else if failed > 0 {
        t.warn
    } else {
        t.fg2
    };
    #[cfg(test)]
    let top = ui.cursor().min.y;
    let first = msgs.first().map(|m| m.id);
    let header = egui::CollapsingHeader::new(RichText::new(head).size(12.5).color(color))
        .id_salt(("tool-group", first))
        .show(ui, |ui| {
            for m in msgs {
                tool_card(ui, t, m, app);
            }
        });
    #[cfg(test)]
    {
        app.chat_ui.probe.tool_groups.push(egui::Rect::from_min_max(
            egui::pos2(header.header_response.rect.left(), top),
            egui::pos2(
                header.header_response.rect.right(),
                header.header_response.rect.bottom(),
            ),
        ));
    }
    if let Some((_, full)) = started {
        header
            .header_response
            .on_hover_text(format!("Started {full}"));
    }
    ui.add_space(6.0);
}

#[allow(dead_code)]
fn _id(id: roost_core::session::SessionId) -> egui::Id {
    composer_id(id)
}

/// A dashed outline for a pill: the straight sides in dashes and the round
/// ends in dots along the arc.
fn dashed_pill(p: &egui::Painter, r: egui::Rect, colour: egui::Color32, width: f32) {
    let radius = r.height() / 2.0;
    let stroke = egui::Stroke::new(width, colour);
    let mut path = Vec::new();
    let steps = 10;
    let right = egui::pos2(r.right() - radius, r.center().y);
    let left = egui::pos2(r.left() + radius, r.center().y);
    for i in 0..=steps {
        let a = std::f32::consts::PI * (-0.5 + i as f32 / steps as f32);
        path.push(right + egui::vec2(a.cos(), a.sin()) * radius);
    }
    for i in 0..=steps {
        let a = std::f32::consts::PI * (0.5 + i as f32 / steps as f32);
        path.push(left + egui::vec2(a.cos(), a.sin()) * radius);
    }
    path.push(path[0]);
    p.extend(egui::Shape::dashed_line(&path, stroke, 3.0, 2.5));
}
