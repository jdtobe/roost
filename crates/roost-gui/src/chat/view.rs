//! Rendering for the Chat view: the history rail, the conversation pane and
//! the composer. All state lives in `ChatState` (events folded) and `ChatUi`
//! (interaction state); commands are queued as `BackendRequest::Chat`.

use super::state::{local_day, status_label, RailRow, RunBadge, SendAttempt};
use crate::app::{bad, AppState, View};
use crate::browse::markdown::{self, Block};
use crate::confirm::Confirm;
use crate::runtime::BackendRequest;
use crate::theme::{mono, semibold, tokens, Tokens};
use crate::widgets::{primary_button, small_button, tag};
use eframe::egui::{
    self, vec2, Align, Color32, FontId, Key, Layout, Margin, Modifiers, RichText, Sense, Stroke,
};
use roost_core::chat::ChatCommand;
use roost_core::session::{
    AssistantStatus, Message, MessageContent, Session, SessionId, SessionLifetime, SessionMode,
    ToolStatus,
};
use roost_core::store::RowStatus;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// Margins of the conversation pane (a header band reaches over them).
pub(super) const PANE_MARGIN_X: f32 = 20.0;
pub(super) const PANE_MARGIN_Y: f32 = 16.0;
const LANDING_W: f32 = 460.0;
pub(super) const BAND_H: f32 = crate::sidebar::BAND_H;
const ROW_H: f32 = 46.0;
/// The tallest the input grows before it scrolls.
const COMPOSER_MAX_H: f32 = 160.0;

/// Diameter of the send button inside the input.
const SEND_SIZE: f32 = 30.0;

/// Space between the send button and the bubble's edges. With the input's
/// margins a one-line bubble is `SEND_SIZE + 2 * SEND_GAP` high, so the gap
/// is the same above, below and to the right.
const SEND_GAP: f32 = 5.0;

/// How long a deleted chat can be brought back.
const UNDO_SECS: u64 = 10;

/// A chat deleted a moment ago, shown as "Chat deleted · Undo".
#[derive(Debug, Clone)]
pub struct UndoToast {
    pub id: SessionId,
    pub title: String,
    pub at: Instant,
}

/// Interaction state that is not part of the conversation data.
#[derive(Default)]
pub struct ChatUi {
    /// What the user has picked or typed for each question being asked.
    pub question_drafts: std::collections::HashMap<SessionId, QuestionDraft>,
    /// Changes whose diff is shown under their tool.
    pub diffs_open: std::collections::HashSet<(SessionId, u32)>,
    /// Allow for Run was chosen for this session and awaits the confirmation.
    pub run_allow_pending: Option<SessionId>,
    /// The confirmation was asked for by a long press on the X pill, so it
    /// shows above the input; otherwise it shows in the tools menu.
    pub run_allow_from_pill: bool,
    /// Families whose list of individual tools is open, per session (0 Files, 1 Shell).
    pub tools_open: std::collections::HashSet<(SessionId, u8)>,
    /// The tool whose limits are shown under its row in the Individual tools list.
    pub limits_open: Option<String>,
    /// A session being renamed in the header, with the text so far.
    pub renaming: Option<(SessionId, String)>,
    pub confirm: Confirm,
    /// The chat that was just deleted and can still be brought back.
    pub undo: Option<UndoToast>,
    /// The step budget typed beside Continue, per agent chat.
    pub continue_steps: std::collections::HashMap<SessionId, u32>,
    /// A rail row that cannot be opened (damaged, newer, unreadable).
    pub inspect: Option<SessionId>,
    /// Put the cursor in the composer on the next frame.
    pub focus_composer: bool,
    /// Models whose tool support the model popup has already asked about.
    pub checked_models: std::collections::HashSet<String>,
    /// What the rail's search field holds.
    pub rail_query: String,
    /// The reply being read aloud.
    pub reading: Option<roost_core::session::MessageId>,
    /// What the side-question field holds.
    pub btw_input: String,
    /// The "Branch with another model" dialog, while open.
    pub branch_dialog: Option<BranchDialog>,
    /// A message to scroll into view once its chat is shown.
    pub scroll_to: Option<roost_core::session::MessageId>,
    /// Your message being edited, with the text so far.
    pub editing: Option<(roost_core::session::MessageId, String)>,
    /// Parsed Markdown of assistant replies, by message and the text length
    /// it was parsed from, so a streaming reply is parsed once per chunk.
    md_cache: std::collections::HashMap<roost_core::session::MessageId, (usize, Vec<Block>)>,
    #[cfg(test)]
    pub probe: ChatProbe,
}

/// A branch being set up: from which reply, on which model.
pub struct BranchDialog {
    pub session: SessionId,
    pub message: roost_core::session::MessageId,
    pub model: String,
}

/// Painted rectangles recorded for the headless layout tests.
#[cfg(test)]
#[derive(Debug, Clone, Default)]
pub struct ChatProbe {
    pub rail: Option<egui::Rect>,
    pub rail_search: Option<egui::Rect>,
    /// The Copy button of every message card painted this frame.
    pub copy_buttons: Vec<egui::Rect>,
    /// The time label of every message row painted this frame, with its text.
    pub stamps: Vec<(egui::Rect, String)>,
    /// The Regenerate button under the last reply, when shown.
    pub regenerate: Option<egui::Rect>,
    /// The Details button of the last reply, when shown.
    pub details: Option<egui::Rect>,
    /// The bubble of every message painted this frame.
    pub cards: Vec<egui::Rect>,
    pub edit_icon: Option<egui::Rect>,
    /// The Allow / Allow for this session / Deny buttons of an approval card.
    pub approval_buttons: Vec<egui::Rect>,
    /// The Choose folder / Skip buttons of the card shown when a tool needs a
    /// working directory.
    pub folder_buttons: Vec<egui::Rect>,
    /// The question card, its options (and the typed-answer field last, when
    /// there is one), and its Send and Skip buttons.
    pub question_card: Option<egui::Rect>,
    pub question_options: Vec<egui::Rect>,
    pub question_other: Option<egui::Rect>,
    pub question_buttons: Vec<egui::Rect>,
    /// The Off / Ask / Auto buttons of the Files · Read permission.
    pub permission_buttons: Vec<egui::Rect>,
    /// The Off / Ask buttons of the Write row in the tools menu.
    pub write_buttons: Vec<egui::Rect>,
    /// The Off / Ask / Allow buttons of the Run row, when commands are switched on.
    pub run_buttons: Vec<egui::Rect>,
    /// The "Individual tools" heading of each family card.
    pub tools_heads: Vec<egui::Rect>,
    /// The limit summary beside each listed tool, and the popover it opens
    /// with its "Change in Settings" button.
    pub tool_limit_chips: Vec<egui::Rect>,
    pub tool_limit_popover: Option<egui::Rect>,
    pub tool_limit_settings: Option<egui::Rect>,
    /// The Confirm and Cancel buttons shown after choosing Allow for Run.
    pub run_confirm: Option<(egui::Rect, egui::Rect)>,
    /// Per change (by id): its row under the tool, its Undo and Show diff buttons.
    pub change_rows: Vec<(u32, egui::Rect)>,
    pub undo_buttons: Vec<(u32, egui::Rect)>,
    pub show_diff_buttons: Vec<(u32, egui::Rect)>,
    /// Undo all in the Changes made card.
    pub undo_all_button: Option<egui::Rect>,
    /// The R / W chips in the header, with what each is set to.
    pub chips: Vec<(char, egui::Rect, roost_core::session::Permission)>,
    pub branch_icon: Option<egui::Rect>,
    pub btw_icon: Option<egui::Rect>,
    pub read_icon: Option<egui::Rect>,
    pub read_disabled: bool,
    pub btw_ask: Option<egui::Rect>,
    pub btw_add: Option<egui::Rect>,
    pub branch_plain: Option<egui::Rect>,
    pub branch_pick: Option<egui::Rect>,
    pub branch_create: Option<egui::Rect>,
    /// Links in a branch's strip: the chat each opens, and where.
    pub strip_links: Vec<(SessionId, egui::Rect)>,
    pub view_original: Option<egui::Rect>,
    pub edit_send: Option<egui::Rect>,
    pub version_counter: Option<egui::Rect>,
    pub version_prev: Option<egui::Rect>,
    pub version_next: Option<egui::Rect>,
    pub pane: Option<egui::Rect>,
    pub composer: Option<egui::Rect>,
    pub header: Option<egui::Rect>,
    /// The status text in the header ("Idle", "Waiting for you"…).
    pub status_label: Option<String>,
    /// Where the status text and the title were painted in the header.
    pub status_rect: Option<egui::Rect>,
    pub title_rect: Option<egui::Rect>,
    /// The title and text of the landing panel, when it is showing.
    pub landing: Option<egui::Rect>,
    pub path_rect: Option<egui::Rect>,
    pub rows: Vec<(SessionId, egui::Rect)>,
    pub send_button: Option<(egui::Rect, &'static str)>,
    /// The delete control of each rail row that shows one, and whether it is armed.
    pub rail_delete: Vec<(SessionId, egui::Rect, bool)>,
    pub pin_button: Option<egui::Rect>,
    pub rename_button: Option<egui::Rect>,
    pub new_button: Option<egui::Rect>,
    pub run_banner: Option<egui::Rect>,
    pub run_continue: Option<egui::Rect>,
    pub run_steps: Option<egui::Rect>,
    pub run_marker: Option<egui::Rect>,
    pub undo_toast: Option<egui::Rect>,
    pub undo_button: Option<egui::Rect>,
    pub incognito_button: Option<egui::Rect>,
    pub agent_chip: Option<egui::Rect>,
    pub plus_button: Option<egui::Rect>,
    pub folder_pill: Option<egui::Rect>,
    pub input_rect: Option<egui::Rect>,
    pub model_chip: Option<egui::Rect>,
    pub model_unload: Option<egui::Rect>,
    pub tool_groups: Vec<egui::Rect>,
    pub tool_cards: Vec<egui::Rect>,
    pub queued_chips: Vec<egui::Rect>,
    pub delete_button: Option<(egui::Rect, String)>,
}

/// The composer text field of a session; also used to drop its editor state.
pub(super) fn composer_id(id: SessionId) -> egui::Id {
    egui::Id::unique("chat-composer").with(id)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// A single-line galley cut with an ellipsis at `width`.
fn truncated(
    ui: &egui::Ui,
    text: &str,
    font: FontId,
    color: Color32,
    width: f32,
) -> Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_string(), font, color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(width.max(1.0));
    ui.painter().layout_job(job)
}

pub(super) fn secondary(ui: &mut egui::Ui, text: &str, enabled: bool) -> egui::Response {
    let t = tokens(ui);
    ui.add_enabled(
        enabled,
        egui::Button::new(RichText::new(text).size(12.0).color(t.fg2)).min_size(vec2(0.0, 24.0)),
    )
}

pub fn show(ui: &mut egui::Ui, app: &mut AppState, out: &mut Vec<BackendRequest>) {
    show_mode(ui, app, out, SessionMode::Chat);
}

/// The Work tab: the same layout, listing and opening agents.
pub fn show_work(ui: &mut egui::Ui, app: &mut AppState, out: &mut Vec<BackendRequest>) {
    show_mode(ui, app, out, SessionMode::Agent);
}

fn show_mode(
    ui: &mut egui::Ui,
    app: &mut AppState,
    out: &mut Vec<BackendRequest>,
    mode: SessionMode,
) {
    let t = tokens(ui);
    let now = Instant::now();
    #[cfg(test)]
    {
        app.chat_ui.probe = ChatProbe::default();
    }
    if let Some(why) = app.chat.unavailable.clone() {
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(t.bg).inner_margin(Margin::same(24)))
            .show(ui, |ui| {
                ui.label(RichText::new("History is in use").font(semibold(16.0)));
                ui.add_space(6.0);
                ui.add(egui::Label::new(RichText::new(why).color(t.fg2)).wrap());
            });
        return;
    }
    if app.chat_ui.confirm.is_armed("", now) || app.chat_ui.confirm_pending() {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(100));
    }
    // A destroyed session takes its text-editor state (undo history) too.
    for id in app.chat.take_forgotten() {
        ui.ctx()
            .data_mut(|d| d.remove::<egui::text_edit::TextEditState>(composer_id(id)));
    }
    if app.chat_ui.reading.is_some() {
        if app.speaker.is_speaking() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(300));
        } else {
            app.chat_ui.reading = None;
        }
    }
    let installed: Vec<String> = app
        .installed
        .rows()
        .iter()
        .map(|r| r.name().to_string())
        .collect();

    // Flat: the fill sets the sidebar apart, and the panel draws one divider on
    // its right edge. A border on all four sides doubled up with the bars
    // above and below it.
    let frame = egui::Frame::new()
        .fill(t.side_list())
        .inner_margin(Margin::same(12));
    let wanted = app.settings.sidebar_width;
    let ((), width) = crate::sidebar::show(ui, wanted, frame, t.side_edge(), |ui| {
        #[cfg(test)]
        {
            app.chat_ui.probe.rail = Some(ui.max_rect());
        }
        rail(ui, app, out, &t, &installed, mode);
    });
    crate::sidebar::finish(ui, app, width);

    // The open session is taken out for the frame so the composer can edit
    // the draft while the transcript borrows the session.
    // A session of the other kind stays loaded but is not shown on this tab.
    let (open, other) = match app.chat.open.take() {
        Some(s) if s.mode == mode => (Some(s), None),
        other => (None, other),
    };
    if let Some(s) = &open {
        egui::Panel::bottom("chat_composer")
            .resizable(false)
            .show_separator_line(false)
            .frame(
                // No box of its own: the input bubble floats over the same
                // background as the conversation.
                egui::Frame::new().fill(t.bg).inner_margin(Margin {
                    left: 20,
                    right: 20,
                    top: 4,
                    bottom: 6,
                }),
            )
            .show(ui, |ui| {
                #[cfg(test)]
                {
                    app.chat_ui.probe.composer = Some(ui.max_rect());
                }
                composer(ui, app, out, &t, s, &installed);
            });
    }
    egui::CentralPanel::default()
        .frame(
            egui::Frame::new()
                .fill(t.bg)
                .inner_margin(Margin::symmetric(PANE_MARGIN_X as i8, PANE_MARGIN_Y as i8)),
        )
        .show(ui, |ui| {
            #[cfg(test)]
            {
                app.chat_ui.probe.pane = Some(ui.max_rect());
            }
            pane(ui, app, out, &t, open.as_ref(), &installed, now, mode);
        });
    undo_toast(ui, app, out, &t, now);
    branch_dialog(ui, &t, app, out, &installed);
    app.chat.open = open.or(other);
}

impl ChatUi {
    /// A delete confirmation is showing and will time out on its own.
    fn confirm_pending(&self) -> bool {
        self.inspect.is_some() || self.renaming.is_some()
    }
}

// ---------------------------------------------------------------- the rail

fn rail(
    ui: &mut egui::Ui,
    app: &mut AppState,
    out: &mut Vec<BackendRequest>,
    t: &Tokens,
    installed: &[String],
    mode: SessionMode,
) {
    let model = app.chat.default_model(installed);
    // The search field and the two new buttons sit on a band of the theme's
    // light tint that runs to the edges of the sidebar.
    let band = ui.painter().add(egui::Shape::Noop);
    let band_left = ui.max_rect().left() - 12.0;
    let band_right = ui.max_rect().right() + 12.0;
    let band_top = ui.max_rect().top() - 12.0;
    let mut start = None;
    // New Work opens at once and the model is chosen in the session.
    let ready = model.is_some() || mode == SessionMode::Agent;
    let hint = "Install a model first (Models), then start one.";
    let (plain, incognito) = if mode == SessionMode::Agent {
        ("New agent", "New incognito agent")
    } else {
        ("New chat", "New incognito chat")
    };
    // Search with the two ways to start something beside it, centred in the band.
    ui.add_space(crate::sidebar::SEARCH_DROP);
    let field = ui
        .horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let room = (ui.available_width() - 2.0 * (RAIL_BTN + 6.0)).max(80.0);
            let field = ui
                .allocate_ui(vec2(room, RAIL_BTN), |ui| {
                    crate::widgets::search_field(ui, &mut app.chat_ui.rail_query, "Search")
                })
                .inner;
            let new = rail_icon_button(
                ui,
                t,
                crate::icons::Icon::NewChat,
                &with_shortcut(plain, "N"),
                (!ready).then_some(hint),
                true,
            );
            let hidden = rail_icon_button(
                ui,
                t,
                crate::icons::Icon::Incognito,
                &with_shortcut(incognito, "⇧N"),
                (!ready).then_some(hint),
                false,
            );
            if new.clicked() {
                start = Some(SessionLifetime::Persisted);
            }
            if hidden.clicked() {
                start = Some(SessionLifetime::Incognito);
            }
            #[cfg(test)]
            {
                app.chat_ui.probe.new_button = Some(new.rect);
                app.chat_ui.probe.incognito_button = Some(hidden.rect);
            }
            field
        })
        .inner;
    app.search_rect = Some(field.response.rect);
    app.search_clear_rect = field.clear;
    #[cfg(test)]
    {
        app.chat_ui.probe.rail_search = Some(field.response.rect);
    }
    if let Some(lifetime) = start.filter(|_| ready) {
        if mode == SessionMode::Agent {
            app.chat_ui.focus_composer = true;
            app.chat_ui.inspect = None;
            out.push(BackendRequest::Chat(ChatCommand::CreateAgent {
                lifetime,
                model: String::new(),
                workspace: None,
            }));
        } else if let Some(model) = model.clone() {
            app.chat_ui.focus_composer = true;
            app.chat_ui.inspect = None;
            out.push(BackendRequest::Chat(ChatCommand::Create {
                lifetime,
                model,
            }));
        }
    }
    ui.add_space((band_top + BAND_H - ui.cursor().min.y).max(0.0));
    ui.painter().set(
        band,
        egui::Shape::rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(band_left, band_top),
                egui::pos2(band_right, ui.cursor().min.y),
            ),
            0.0,
            t.side_band(),
        ),
    );
    ui.add_space(8.0);
    rail_list(ui, app, out, t, mode);
}

/// Deletes a session at once. A saved one can be brought back for
/// `UNDO_SECS` (the toast); an earlier toast's chat is removed for good first.
/// An incognito chat has no file and cannot come back.
pub(super) fn delete_session(
    app: &mut AppState,
    out: &mut Vec<BackendRequest>,
    id: SessionId,
    title: &str,
    saved: bool,
    now: Instant,
) {
    if let Some(prev) = app.chat_ui.undo.take() {
        out.push(BackendRequest::Chat(ChatCommand::PurgeDeleted(prev.id)));
    }
    out.push(BackendRequest::Chat(ChatCommand::Delete(id)));
    if saved {
        app.chat_ui.undo = Some(UndoToast {
            id,
            title: title.to_string(),
            at: now,
        });
    }
}

/// "Chat deleted · Undo", floating at the bottom left while the chat can
/// still be brought back; when the time is up the chat is removed for good.
fn undo_toast(
    ui: &mut egui::Ui,
    app: &mut AppState,
    out: &mut Vec<BackendRequest>,
    t: &Tokens,
    now: Instant,
) {
    let Some(toast) = app.chat_ui.undo.clone() else {
        return;
    };
    let window = std::time::Duration::from_secs(UNDO_SECS);
    if now >= toast.at + window {
        app.chat_ui.undo = None;
        out.push(BackendRequest::Chat(ChatCommand::PurgeDeleted(toast.id)));
        return;
    }
    ui.ctx().request_repaint_after(toast.at + window - now);
    let mut undo = false;
    let area = egui::Area::new(egui::Id::unique("undo-toast"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::LEFT_BOTTOM, vec2(16.0, -16.0))
        .show(ui.ctx(), |ui| {
            egui::Frame::new()
                .fill(t.surface)
                .stroke(Stroke::new(1.0, t.line))
                .corner_radius(10.0)
                .inner_margin(Margin::symmetric(14, 9))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let name = truncate_chars(&toast.title, 28);
                        ui.label(
                            RichText::new(format!("Deleted “{name}”"))
                                .size(13.0)
                                .color(t.fg2),
                        );
                        let r = ui
                            .add(
                                egui::Button::new(
                                    RichText::new("Undo").font(semibold(13.0)).color(t.accent),
                                )
                                .frame(false),
                            )
                            .on_hover_cursor(egui::CursorIcon::PointingHand);
                        #[cfg(test)]
                        {
                            app.chat_ui.probe.undo_button = Some(r.rect);
                        }
                        undo = r.clicked();
                    });
                });
        });
    #[cfg(test)]
    {
        app.chat_ui.probe.undo_toast = Some(area.response.rect);
    }
    let _ = area;
    if undo {
        app.chat_ui.undo = None;
        out.push(BackendRequest::Chat(ChatCommand::Undelete(toast.id)));
    }
}

/// Side of the square buttons beside the search field.
const RAIL_BTN: f32 = 32.0;

/// "New chat (⌘N)" on macOS, "New chat (Ctrl+N)" elsewhere.
fn with_shortcut(label: &str, key: &str) -> String {
    if cfg!(target_os = "macos") {
        let key = key.replace('⇧', "⇧⌘");
        let key = if key.starts_with('⇧') {
            key
        } else {
            format!("⌘{key}")
        };
        format!("{label} ({key})")
    } else {
        let key = key.replace('⇧', "Shift+");
        format!("{label} (Ctrl+{key})")
    }
}

/// A square icon button for the rail. `disabled` is the hover text of a
/// button that cannot be used right now.
fn rail_icon_button(
    ui: &mut egui::Ui,
    t: &Tokens,
    icon: crate::icons::Icon,
    tip: &str,
    disabled: Option<&str>,
    primary: bool,
) -> egui::Response {
    let enabled = disabled.is_none();
    let (rect, resp) = ui.allocate_exact_size(
        vec2(RAIL_BTN, RAIL_BTN),
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let resp = match disabled {
        Some(why) => resp.on_hover_text(why),
        None => resp
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(tip),
    };
    // Both buttons are outlined. The new-chat one has its icon in the theme
    // colour, and either takes a faint outline of it when pointed at.
    let hot = resp.hovered() && enabled;
    let (fill, stroke) = crate::widgets::icon_button_look(t, hot);
    let ink = match (enabled, primary) {
        (false, _) => t.fg3,
        (true, true) => t.accent,
        (true, false) => t.fg2,
    };
    ui.painter()
        .rect(rect, 8.0, fill, stroke, egui::StrokeKind::Inside);
    crate::icons::paint(ui.painter(), rect, icon, ink);
    resp
}

fn rail_list(
    ui: &mut egui::Ui,
    app: &mut AppState,
    out: &mut Vec<BackendRequest>,
    t: &Tokens,
    mode: SessionMode,
) {
    let now = Instant::now();
    let rail = app
        .chat
        .rail_for(mode, &app.chat_ui.rail_query, now_ms(), local_day);
    let selected = app.chat.open.as_ref().map(|s| s.id).or(app.chat_ui.inspect);
    let mut open_id = None;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let groups = [
                ("Pinned", &rail.pinned),
                ("Today", &rail.today),
                ("Yesterday", &rail.yesterday),
                ("Earlier", &rail.earlier),
            ];
            for (label, rows) in groups {
                if rows.is_empty() {
                    continue;
                }
                section_label(ui, t, label);
                for row in rows {
                    if rail_row(ui, t, row, selected == Some(row.id), app, out, now) {
                        open_id = Some(row.id);
                    }
                    for kid in &row.children {
                        // A branch hangs under its parent, indented.
                        let hit = egui::Frame::new()
                            .inner_margin(Margin {
                                left: 16,
                                ..Margin::ZERO
                            })
                            .show(ui, |ui| {
                                rail_row(ui, t, kid, selected == Some(kid.id), app, out, now)
                            })
                            .inner;
                        if hit {
                            open_id = Some(kid.id);
                        }
                    }
                }
            }
            if !rail.incognito.is_empty() {
                ui.horizontal(|ui| {
                    section_label(ui, t, "Incognito");
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if small_button(ui, "Clear") {
                            out.push(BackendRequest::Chat(ChatCommand::ClearIncognito));
                        }
                    });
                });
                ui.add(
                    egui::Label::new(
                        RichText::new("Memory only. Gone when closed.")
                            .size(11.5)
                            .color(t.fg3),
                    )
                    .wrap(),
                );
                for row in &rail.incognito {
                    if rail_row(ui, t, row, selected == Some(row.id), app, out, now) {
                        open_id = Some(row.id);
                    }
                }
            }
            let nothing_shown = [&rail.pinned, &rail.today, &rail.yesterday, &rail.earlier]
                .iter()
                .all(|g| g.is_empty())
                && rail.incognito.is_empty();
            if nothing_shown && !app.chat_ui.rail_query.trim().is_empty() {
                ui.add_space(8.0);
                ui.add(egui::Label::new(RichText::new("No matches.").color(t.fg3)).wrap());
            }
            if app.chat.loaded && !app.chat.rows().iter().any(|r| r.mode == mode) {
                ui.add_space(8.0);
                let none = if mode == SessionMode::Agent {
                    "No agents yet."
                } else {
                    "No chats yet."
                };
                ui.add(egui::Label::new(RichText::new(none).color(t.fg3)).wrap());
            }
        });
    if let Some(id) = open_id {
        let current = app.chat.open.as_ref().map(|s| (s.id, s.lifetime));
        open_chat(app, out, id, current);
    }
}

/// Shows a saved chat: closes the one being left (so the manager can unload
/// it), and asks for the new one. `current` is the chat on screen.
fn open_chat(
    app: &mut AppState,
    out: &mut Vec<BackendRequest>,
    id: SessionId,
    current: Option<(SessionId, SessionLifetime)>,
) {
    // Incognito sessions must never be closed here: that would destroy them.
    let leaving = current
        .filter(|(cur, life)| *cur != id && *life == SessionLifetime::Persisted)
        .map(|(cur, _)| cur);
    if let Some(prev) = leaving {
        out.push(BackendRequest::Chat(ChatCommand::Close(prev)));
    }
    let bad_row = app
        .chat
        .rows()
        .iter()
        .find(|r| r.id == id)
        .is_some_and(|r| r.status != RowStatus::Ok);
    app.chat_ui.renaming = None;
    app.chat_ui.focus_composer = !bad_row;
    if bad_row {
        app.chat_ui.inspect = Some(id);
        app.chat.open = None;
    } else {
        app.chat_ui.inspect = None;
        if current.map(|(cur, _)| cur) != Some(id) {
            out.push(BackendRequest::Chat(ChatCommand::Open(id)));
        }
    }
}

fn section_label(ui: &mut egui::Ui, t: &Tokens, text: &str) {
    ui.add_space(6.0);
    ui.label(
        RichText::new(text.to_uppercase())
            .font(semibold(11.0))
            .color(t.fg3),
    );
}

fn row_note(row: &RailRow) -> Option<(&'static str, bool)> {
    match (row.status, row.badge) {
        (RowStatus::Quarantined, _) => Some(("Damaged, set aside", true)),
        (RowStatus::NewerSchema, _) => Some(("Made by a newer Roost", true)),
        (RowStatus::Unreadable, _) => Some(("Could not be read", true)),
        (_, RunBadge::Waiting) if row.mode == SessionMode::Chat => {
            Some(("Waiting for model", false))
        }
        (_, RunBadge::Running) if row.mode == SessionMode::Chat => Some(("Replying…", false)),
        _ => None,
    }
}

/// One rail row. Clicking it opens the session; the trash can at its right
/// (shown on hover and on the selected row) takes a click and a confirming
/// second click, then the chat can still be undone for a few seconds.
fn rail_row(
    ui: &mut egui::Ui,
    t: &Tokens,
    row: &RailRow,
    selected: bool,
    app: &mut AppState,
    out: &mut Vec<BackendRequest>,
    now: Instant,
) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
    #[cfg(test)]
    app.chat_ui.probe.rows.push((row.id, rect));
    let key = row.id.to_string();
    let armed = app.chat_ui.confirm.is_armed(&key, now);
    let ready = app.chat_ui.confirm.ready(&key, now);
    let show_delete = selected || armed || ui.rect_contains_pointer(rect);
    if selected {
        // A white row with a dark edge on its left, on the tinted list.
        ui.painter().rect_filled(rect, 8.0, t.surface);
        ui.painter().rect_filled(
            egui::Rect::from_min_size(rect.min, vec2(3.0, rect.height())),
            egui::CornerRadius {
                nw: 8,
                sw: 8,
                ne: 0,
                se: 0,
            },
            t.accent,
        );
    } else if show_delete {
        ui.painter().rect_filled(rect, 8.0, t.side_band());
    }
    let reserved = match (show_delete, armed) {
        (false, _) => 0.0,
        (true, false) => 30.0,
        (true, true) => 76.0,
    };
    let incognito = row.lifetime == SessionLifetime::Incognito;
    let agent = row.mode == SessionMode::Agent;
    // A small icon marks a pinned or an incognito row.
    let marked = row.pinned || incognito;
    let left = rect.left() + 10.0 + if marked { 18.0 } else { 0.0 };
    if marked {
        let slot = egui::Rect::from_center_size(
            egui::pos2(rect.left() + 17.0, rect.top() + 16.0),
            vec2(14.0, 14.0),
        );
        if row.pinned {
            crate::icons::paint(ui.painter(), slot, crate::icons::Icon::Pin, t.accent);
        } else {
            crate::icons::paint(ui.painter(), slot, crate::icons::Icon::Incognito, t.fg3);
        }
    }
    if incognito {
        ui.painter().line_segment(
            [
                egui::pos2(rect.left() + 1.0, rect.top() + 8.0),
                egui::pos2(rect.left() + 1.0, rect.bottom() - 8.0),
            ],
            Stroke::new(2.0, t.line),
        );
    }
    // What is shown at the right of the title: a badge, or the time.
    let right_edge = rect.right() - 10.0 - reserved;
    let (side, side_is_badge) = if row.needs_you {
        (Some(("Needs you", t.warn, t.warn_soft)), true)
    } else if agent && row.badge != RunBadge::None {
        (Some(("Working", t.accent, t.accent_soft)), true)
    } else {
        (None, false)
    };
    let time = if row.empty || row.updated_at_ms == 0 {
        String::new()
    } else {
        super::state::row_time(row.updated_at_ms, now_ms())
    };
    let mut side_w = 0.0;
    if let Some((text, ink, fill)) = side {
        let g = ui
            .painter()
            .layout_no_wrap(text.to_string(), FontId::proportional(11.0), ink);
        let pill = egui::Rect::from_min_size(
            egui::pos2(right_edge - g.size().x - 14.0, rect.top() + 6.0),
            vec2(g.size().x + 14.0, 18.0),
        );
        ui.painter().rect_filled(pill, 9.0, fill);
        ui.painter().galley(
            egui::pos2(pill.left() + 7.0, pill.center().y - g.size().y / 2.0),
            g,
            ink,
        );
        side_w = pill.width() + 6.0;
    } else if !time.is_empty() {
        let g = ui
            .painter()
            .layout_no_wrap(time, FontId::proportional(11.0), t.fg3);
        let at = egui::pos2(right_edge - g.size().x, rect.top() + 9.0);
        side_w = g.size().x + 8.0;
        ui.painter().galley(at, g, t.fg3);
    }
    let _ = side_is_badge;
    let width = rect.right() - left - 10.0 - reserved - side_w;
    // A chat with nothing in it yet is a quiet "New chat" in italics.
    let (title_text, title_color, italic) =
        if row.empty && row.title == roost_core::session::UNTITLED {
            (
                if agent { "New agent" } else { "New chat" }.to_string(),
                t.fg3,
                true,
            )
        } else {
            (row.title.clone(), t.fg, false)
        };
    let title = {
        let mut job = egui::text::LayoutJob::simple_singleline(
            title_text,
            FontId::proportional(13.5),
            title_color,
        );
        if italic {
            for section in &mut job.sections {
                section.format.italics = true;
            }
        }
        job.wrap = egui::text::TextWrapping::truncate_at_width(width.max(1.0));
        ui.painter().layout_job(job)
    };
    ui.painter()
        .galley(egui::pos2(left, rect.top() + 7.0), title, title_color);
    let (sub, color) = match row_note(row) {
        Some((text, bad)) => (text.to_string(), if bad { t.bad } else { t.accent }),
        None if row.empty => ("No messages yet".to_string(), t.fg3),
        None if incognito => ("Incognito · not saved".to_string(), t.fg3),
        None if agent => match &row.folder {
            Some(path) => (
                super::chrome::home_relative(std::path::Path::new(path)),
                t.fg3,
            ),
            None => (row.last.clone(), t.fg3),
        },
        None if row.last.is_empty() => (row.model.clone(), t.fg3),
        None => (row.last.clone(), t.fg3),
    };
    let sub_width = rect.right() - left - 10.0;
    let sub =
        if agent && row.folder.is_some() && row_note(row).is_none() && !row.empty && !incognito {
            crate::widgets::ellipsize_middle(ui, &sub, FontId::proportional(11.5), sub_width)
        } else {
            sub
        };
    let sub = truncated(ui, &sub, FontId::proportional(11.5), color, sub_width);
    ui.painter()
        .galley(egui::pos2(left, rect.top() + 26.0), sub, color);

    let mut delete_clicked = false;
    if show_delete {
        let del_rect = if armed {
            egui::Rect::from_min_max(
                egui::pos2(rect.right() - 8.0 - 62.0, rect.top() + 6.0),
                egui::pos2(rect.right() - 8.0, rect.top() + 28.0),
            )
        } else {
            egui::Rect::from_center_size(
                egui::pos2(rect.right() - 17.0, rect.top() + 17.0),
                vec2(22.0, 22.0),
            )
        };
        #[cfg(test)]
        app.chat_ui
            .probe
            .rail_delete
            .push((row.id, del_rect, armed));
        let hit = ui
            .interact(del_rect, resp.id.with("delete"), Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(if armed {
                "Click again to delete this chat (you can undo it for a few seconds)"
            } else {
                "Delete chat"
            });
        if armed {
            // Dim until a click would confirm (the pause that stops a
            // double-click from deleting), so the button never looks ready
            // when it is not.
            let dim = if ready { 1.0 } else { 0.45 };
            ui.painter()
                .rect_filled(del_rect, 6.0, t.bad_soft.gamma_multiply(dim));
            ui.painter().text(
                del_rect.center(),
                egui::Align2::CENTER_CENTER,
                "Delete?",
                semibold(12.0),
                t.bad.gamma_multiply(dim),
            );
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        } else {
            if hit.hovered() {
                ui.painter()
                    .circle_filled(del_rect.center(), 11.0, t.surface);
            }
            crate::icons::paint(
                ui.painter(),
                del_rect,
                crate::icons::Icon::Trash,
                if hit.hovered() { t.bad } else { t.fg3 },
            );
        }
        if hit.clicked() {
            delete_clicked = true;
            if app.chat_ui.confirm.click(&key, now) {
                delete_session(
                    app,
                    out,
                    row.id,
                    &row.title,
                    row.lifetime == SessionLifetime::Persisted,
                    now,
                );
            }
        }
    }
    resp.clicked() && !delete_clicked
}

// ---------------------------------------------------------- the conversation

fn banner(ui: &mut egui::Ui, fg: Color32, bg: Color32, text: &str) {
    egui::Frame::new()
        .fill(bg)
        .corner_radius(8.0)
        .inner_margin(Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.add(egui::Label::new(RichText::new(text).size(13.0).color(fg)).wrap());
        });
    ui.add_space(8.0);
}

#[allow(clippy::too_many_arguments)]
fn pane(
    ui: &mut egui::Ui,
    app: &mut AppState,
    out: &mut Vec<BackendRequest>,
    t: &Tokens,
    open: Option<&Session>,
    installed: &[String],
    now: Instant,
    mode: SessionMode,
) {
    if !app.chat.warnings.is_empty() {
        banner(ui, t.warn, t.warn_soft, &app.chat.warnings.join("  "));
        if small_button(ui, "Dismiss") {
            app.chat.dismiss_warnings();
        }
        ui.add_space(4.0);
    }
    if app.ollama == Some(None) {
        banner(
            ui,
            t.warn,
            t.warn_soft,
            "Ollama is not reachable. Start it and Roost will reconnect; your chats stay readable.",
        );
    } else if app.installed.status == crate::installed::state::ListStatus::Loaded
        && installed.is_empty()
    {
        banner(
            ui,
            t.warn,
            t.warn_soft,
            "No models are installed yet. Open Models to import one, then come back to chat.",
        );
        if small_button(ui, "Open Models") {
            app.select(View::Models);
        }
    }
    let Some(s) = open else {
        if let Some(id) = app.chat_ui.inspect {
            inspect(ui, app, out, t, id, now);
        } else {
            let (title, text) = if mode == SessionMode::Agent {
                (
                    "Start an agent",
                    "Press New to open an agent, then choose a model that supports tools. Give it a folder when it needs one: it asks the first time a tool wants to work with files. Send it a message while it works to steer it.",
                )
            } else {
                (
                    "Start a chat",
                    "Choose a conversation on the left, or start a new one. Incognito chats are never saved.",
                )
            };
            // Centred in the pane, a little above the middle.
            let wide = ui.available_width().min(LANDING_W);
            ui.add_space((ui.available_height() * 0.5 - 70.0).max(24.0));
            ui.vertical_centered(|ui| {
                ui.set_max_width(wide);
                let head = ui.label(RichText::new(title).font(semibold(18.0)));
                ui.add_space(6.0);
                let body = ui.add(
                    egui::Label::new(RichText::new(text).color(t.fg2))
                        .wrap()
                        .halign(Align::Center),
                );
                #[cfg(test)]
                {
                    app.chat_ui.probe.landing = Some(head.rect.union(body.rect));
                }
                let _ = (head, body);
            });
        }
        return;
    };
    super::chrome::header(ui, app, out, t, s, now);
    if let Some((id, model)) = app.chat.model_missing.clone() {
        if id == s.id {
            banner(
                ui,
                t.bad,
                t.bad_soft,
                &format!("The model {model} is not installed. Choose another model above."),
            );
        }
    }
    let omitted = app.chat.omitted(s.id);
    if omitted > 0 {
        ui.label(
            RichText::new(format!(
                "{omitted} earlier messages were left out of the last request to fit the model's context."
            ))
            .size(12.0)
            .color(t.fg3),
        );
    }
    // Pinned above the transcript, so a change to the original is never
    // scrolled out of sight.
    branch_strip(ui, t, app, out, s);
    let badge = app.chat.badge(s.id);
    let model = row_model(app, s);
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .stick_to_bottom(true)
        .show(ui, |ui| {
            let last = s.messages.len().saturating_sub(1);
            let marker_at = s
                .last_run_end
                .as_ref()
                .filter(|n| s.mode == SessionMode::Agent && n.after < s.messages.len())
                .map(|n| n.after);
            let mut marker_done = false;
            let mut i = 0;
            while i < s.messages.len() {
                // The marker goes above the first item that starts at or after
                // the point where the run ended (a folded group of tool calls
                // counts as one item).
                let item_end = if matches!(s.messages[i].content, MessageContent::Tool { .. }) {
                    let mut end = i;
                    while end < s.messages.len()
                        && matches!(s.messages[end].content, MessageContent::Tool { .. })
                    {
                        end += 1;
                    }
                    end
                } else {
                    i + 1
                };
                if !marker_done && marker_at.is_some_and(|at| at < item_end) {
                    marker_done = true;
                    run_end_marker(ui, t, app, s);
                }
                let m = &s.messages[i];
                if matches!(m.content, MessageContent::Tool { .. }) {
                    let mut end = i;
                    while end < s.messages.len()
                        && matches!(s.messages[end].content, MessageContent::Tool { .. })
                    {
                        end += 1;
                    }
                    super::chrome::tool_group(ui, t, &s.messages[i..end], app);
                    super::chrome::change_rows(ui, t, app, out, s, &s.messages[i..end]);
                    i = end;
                    continue;
                }
                let editing_this = app.chat_ui.editing.as_ref().is_some_and(|e| e.0 == m.id);
                let card = if editing_this {
                    edit_box(ui, t, app, out, s)
                } else {
                    message_card(ui, t, m, &model, badge, &mut app.chat_ui)
                };
                #[cfg(test)]
                app.chat_ui.probe.cards.push(card);
                if !editing_this {
                    message_actions(ui, t, app, out, s, (i, m), &model, i == last, card);
                }
                if app.chat_ui.scroll_to == Some(m.id) {
                    ui.scroll_to_rect(card, Some(Align::Center));
                    app.chat_ui.scroll_to = None;
                }
                if s.parent.is_some_and(|p| p.message_id == m.id) {
                    branch_divider(ui, t, s);
                }
                if i == last && s.mode == SessionMode::Chat {
                    retry_row(ui, app, out, s, m);
                }
                i += 1;
            }
            run_end_banner(ui, t, app, out, s);
        });
}

/// What the banner and the marker say about how a run ended: a headline and
/// an optional second line.
fn run_end_words(end: &roost_core::session::RunEnd) -> (String, Option<String>) {
    use roost_core::session::RunEnd;
    match end {
        RunEnd::StepLimit { steps } => (
            format!("Stopped after using all {steps} steps"),
            Some("It was still working. Continue gives it a fresh budget.".into()),
        ),
        RunEnd::Stopped => ("You stopped this run".into(), None),
        RunEnd::Failed { message } => (
            "The run failed".into(),
            (!message.is_empty()).then(|| message.clone()),
        ),
        RunEnd::Interrupted => (
            "Roost was closed during this run".into(),
            Some("Continue picks up from where it stopped.".into()),
        ),
        RunEnd::TextToolCall { tool } => (
            "The reply is a tool call written as text".into(),
            Some(format!(
                "Nothing ran ({tool}). This model may not handle tools well here; try another model."
            )),
        ),
    }
}

/// One quiet line where a run ended, once the conversation has moved on.
fn run_end_marker(ui: &mut egui::Ui, t: &Tokens, app: &mut AppState, s: &Session) {
    let Some(note) = &s.last_run_end else {
        return;
    };
    let (title, _) = run_end_words(&note.end);
    let r =
        ui.add(egui::Label::new(RichText::new(format!("{title}.")).size(12.0).color(t.fg3)).wrap());
    ui.add_space(6.0);
    #[cfg(test)]
    {
        app.chat_ui.probe.run_marker = Some(r.rect);
    }
    let _ = (r, app);
}

/// At the end of an agent chat whose last run stopped early: why, and a way
/// to go on. The text takes the width; the controls sit below it, at the
/// bottom right.
fn run_end_banner(
    ui: &mut egui::Ui,
    t: &Tokens,
    app: &mut AppState,
    out: &mut Vec<BackendRequest>,
    s: &Session,
) {
    use roost_core::session::RunEnd;
    #[cfg(test)]
    {
        app.chat_ui.probe.run_banner = None;
        app.chat_ui.probe.run_continue = None;
        app.chat_ui.probe.run_steps = None;
    }
    let Some(note) = &s.last_run_end else {
        return;
    };
    if s.mode != SessionMode::Agent || app.chat.is_running(s.id) || note.after != s.messages.len() {
        return;
    }
    let (title, sub) = run_end_words(&note.end);
    let (fill, edge) = match note.end {
        RunEnd::Failed { .. } => (t.bad_soft, t.bad),
        RunEnd::Stopped => (t.surface, t.line),
        RunEnd::StepLimit { .. } | RunEnd::Interrupted | RunEnd::TextToolCall { .. } => {
            (t.warn_soft, t.warn)
        }
    };
    // Going on does not help when the model cannot make tool calls.
    let button = match note.end {
        RunEnd::Failed { .. } => Some("Try again"),
        RunEnd::TextToolCall { .. } => None,
        _ => Some("Continue"),
    };
    let mut go = false;
    let mut steps = match note.end {
        RunEnd::StepLimit { steps } => *app
            .chat_ui
            .continue_steps
            .entry(s.id)
            .or_insert((steps + 30).min(100)),
        _ => s.step_budget,
    };
    ui.add_space(8.0);
    let frame = egui::Frame::new()
        .fill(fill)
        .stroke(Stroke::new(1.0, edge.gamma_multiply(0.6)))
        .corner_radius(10.0)
        .inner_margin(Margin::symmetric(14, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.add(egui::Label::new(RichText::new(&title).font(semibold(13.5)).color(t.fg)).wrap());
            if let Some(sub) = &sub {
                ui.add(egui::Label::new(RichText::new(sub).size(12.0).color(t.fg2)).wrap());
            }
            let Some(button) = button else {
                return;
            };
            ui.add_space(6.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let r = primary_button(ui, button, true);
                #[cfg(test)]
                {
                    app.chat_ui.probe.run_continue = Some(r.rect);
                }
                go = r.clicked();
                if matches!(note.end, RunEnd::StepLimit { .. }) {
                    let d = ui.add(egui::DragValue::new(&mut steps).range(1..=100));
                    #[cfg(test)]
                    {
                        app.chat_ui.probe.run_steps = Some(d.rect);
                    }
                    let _ = d;
                    ui.label(RichText::new("Steps").size(12.5).color(t.fg2));
                }
            });
        });
    #[cfg(test)]
    {
        app.chat_ui.probe.run_banner = Some(frame.response.rect);
    }
    let _ = frame;
    ui.add_space(8.0);
    if matches!(note.end, RunEnd::StepLimit { .. }) {
        app.chat_ui.continue_steps.insert(s.id, steps);
    }
    if go {
        if matches!(note.end, RunEnd::StepLimit { .. }) && steps != s.step_budget {
            out.push(BackendRequest::Chat(ChatCommand::SetStepBudget(
                s.id, steps,
            )));
        }
        if let SendAttempt::Sent(cmd) = app.chat.try_continue(s.id) {
            out.push(BackendRequest::Chat(cmd));
        }
    }
}

/// The top of a write's approval card: what it does to which file, how many
/// lines, and the changed lines themselves.
fn write_summary(ui: &mut egui::Ui, t: &Tokens, p: &roost_core::agent::change::ChangePreview) {
    ui.horizontal_wrapped(|ui| {
        let what = if p.created {
            "The agent wants to create a file"
        } else {
            "The agent wants to change a file"
        };
        ui.label(RichText::new(what).font(semibold(13.0)).color(t.fg));
    });
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.label(RichText::new(&p.path).font(mono(12.5)).color(t.fg));
        ui.label(
            RichText::new(format!("+{}", p.added))
                .font(mono(12.0))
                .color(t.ok),
        );
        ui.label(
            RichText::new(format!("−{}", p.removed))
                .font(mono(12.0))
                .color(bad(ui.visuals().dark_mode)),
        );
    });
    diff_box(ui, t, p);
    if p.more > 0 {
        ui.label(
            RichText::new(format!("… and {} more changed lines", p.more))
                .size(11.5)
                .color(t.fg3),
        );
    }
}

/// The changed lines of a change in a small scrolling box.
pub(super) fn diff_box(
    ui: &mut egui::Ui,
    t: &Tokens,
    p: &roost_core::agent::change::ChangePreview,
) {
    use roost_core::agent::change::LineKind;
    egui::Frame::new()
        .fill(t.surface)
        .stroke(Stroke::new(1.0, t.line))
        .corner_radius(6.0)
        .inner_margin(Margin::symmetric(8, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 1.0;
            egui::ScrollArea::vertical()
                .max_height(180.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    for l in &p.lines {
                        let (mark, colour, fill) = match l.kind {
                            LineKind::Same => (' ', t.fg3, None),
                            LineKind::Added => ('+', t.ok, Some(t.ok.gamma_multiply(0.12))),
                            LineKind::Removed => (
                                '-',
                                bad(ui.visuals().dark_mode),
                                Some(bad(ui.visuals().dark_mode).gamma_multiply(0.12)),
                            ),
                        };
                        let text = format!("{mark} {}", truncate_chars(&l.text, 200));
                        let label =
                            egui::Label::new(RichText::new(text).font(mono(12.0)).color(colour))
                                .truncate();
                        let r = ui.add(label);
                        if let Some(f) = fill {
                            ui.painter().rect_filled(
                                egui::Rect::from_min_max(
                                    egui::pos2(r.rect.left() - 4.0, r.rect.top()),
                                    egui::pos2(ui.max_rect().right() + 4.0, r.rect.bottom()),
                                ),
                                2.0,
                                f,
                            );
                        }
                    }
                });
        });
}

/// What a command approval says: the command itself, where it runs and the
/// limits it runs under.
fn command_summary(ui: &mut egui::Ui, t: &Tokens, r: &roost_core::agent::command::RunPreview) {
    ui.label(
        RichText::new("The agent wants to run a command")
            .font(semibold(13.0))
            .color(t.fg),
    );
    egui::Frame::new()
        .fill(t.surface)
        .stroke(Stroke::new(1.0, t.line))
        .corner_radius(7.0)
        .inner_margin(Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            // The whole command stays reachable, but a long one scrolls
            // instead of pushing the buttons off the screen.
            egui::ScrollArea::vertical()
                .max_height((ui.ctx().content_rect().height() * 0.13).clamp(60.0, 120.0))
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.add(
                        egui::Label::new(RichText::new(&r.command).font(mono(12.5)).color(t.fg))
                            .wrap(),
                    );
                });
        });
    let minutes = r.timeout_secs / 60;
    let limit = if r.timeout_secs.is_multiple_of(60) && minutes > 0 {
        format!(
            "stops after {minutes} minute{}",
            if minutes == 1 { "" } else { "s" }
        )
    } else {
        format!("stops after {} seconds", r.timeout_secs)
    };
    let facts = [
        format!("in {}", r.folder.display()),
        limit,
        format!("output kept to {} KB", r.output_kb),
        "your tokens are not passed on".to_string(),
    ];
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 14.0;
        for fact in facts {
            ui.label(RichText::new(fact).size(12.0).color(t.fg2));
        }
    });
}

/// What the agent wants to do and three ways to answer. The run waits.
/// What the user has picked or typed for one question.
#[derive(Debug, Default, Clone)]
pub struct QuestionDraft {
    pub call_id: String,
    /// Indexes of the options picked.
    pub picked: std::collections::BTreeSet<usize>,
    /// The typed answer: the whole answer for a text question, an answer of
    /// their own for the others.
    pub other: String,
}

impl QuestionDraft {
    fn answer(&self, q: &roost_core::agent::ask::Question) -> roost_core::agent::ask::Answer {
        let other = self.other.trim();
        roost_core::agent::ask::Answer {
            picked: self
                .picked
                .iter()
                .filter_map(|i| q.options.get(*i).cloned())
                .collect(),
            other: (!other.is_empty()).then(|| other.to_string()),
        }
    }
}

/// The agent's question, above the input like an approval. Send is off until
/// the answer is one the core would accept.
fn question_card(
    ui: &mut egui::Ui,
    t: &Tokens,
    app: &mut AppState,
    out: &mut Vec<BackendRequest>,
    s: &Session,
    pending: &super::state::PendingQuestion,
) {
    use roost_core::agent::ask::{self, QuestionKind};
    let (id, q) = (s.id, &pending.question);
    let mut draft = app
        .chat_ui
        .question_drafts
        .remove(&id)
        .filter(|d| d.call_id == pending.call_id)
        .unwrap_or_else(|| QuestionDraft {
            call_id: pending.call_id.clone(),
            ..Default::default()
        });
    let (mut send, mut skip) = (false, false);
    #[cfg(test)]
    {
        app.chat_ui.probe.question_options.clear();
        app.chat_ui.probe.question_other = None;
    }
    let card = egui::Frame::new()
        .fill(t.accent_soft)
        .stroke(Stroke::new(1.0, t.accent))
        .corner_radius(10.0)
        .inner_margin(Margin::symmetric(14, 12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 8.0;
            ui.add(
                egui::Label::new(RichText::new(&q.text).font(semibold(13.5)).color(t.fg)).wrap(),
            );
            if q.kind != QuestionKind::Text {
                for (i, option) in q.options.iter().enumerate() {
                    let on = draft.picked.contains(&i);
                    let r = if q.kind == QuestionKind::One {
                        ui.radio(on, option)
                    } else {
                        let mut checked = on;
                        let r = ui.checkbox(&mut checked, option);
                        if r.changed() {
                            if checked {
                                draft.picked.insert(i);
                            } else {
                                draft.picked.remove(&i);
                            }
                        }
                        r
                    };
                    #[cfg(test)]
                    app.chat_ui.probe.question_options.push(r.rect);
                    if q.kind == QuestionKind::One && r.clicked() {
                        // One answer: an option or the typed one, not both.
                        draft.picked.clear();
                        draft.picked.insert(i);
                        draft.other.clear();
                    }
                }
            }
            if q.kind == QuestionKind::Text || q.allow_other {
                let edit = if q.kind == QuestionKind::Text {
                    egui::TextEdit::multiline(&mut draft.other)
                        .hint_text("Type your answer")
                        .desired_rows(3)
                } else {
                    egui::TextEdit::singleline(&mut draft.other).hint_text("Something else…")
                };
                let r = ui.add(edit.desired_width(f32::INFINITY));
                #[cfg(test)]
                {
                    app.chat_ui.probe.question_other = Some(r.rect);
                }
                if q.kind == QuestionKind::One && r.changed() && !draft.other.trim().is_empty() {
                    draft.picked.clear();
                }
            }
            let valid = ask::check_answer(q, &draft.answer(q)).is_ok();
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let send_r = primary_button(ui, "Send", valid);
                let skip_r = secondary(ui, "Skip", true);
                #[cfg(test)]
                {
                    app.chat_ui.probe.question_buttons = vec![send_r.rect, skip_r.rect];
                }
                send = send_r.clicked() && valid;
                skip = skip_r.clicked();
            });
        });
    #[cfg(test)]
    {
        app.chat_ui.probe.question_card = Some(card.response.rect);
    }
    #[cfg(not(test))]
    let _ = card;
    if send || skip {
        let answer = send.then(|| draft.answer(q));
        out.push(BackendRequest::Chat(ChatCommand::AnswerQuestion {
            session: id,
            call_id: pending.call_id.clone(),
            answer,
        }));
        app.chat.question_answered(id);
    } else {
        app.chat_ui.question_drafts.insert(id, draft);
    }
}

fn approval_card(
    ui: &mut egui::Ui,
    t: &Tokens,
    app: &mut AppState,
    out: &mut Vec<BackendRequest>,
    s: &Session,
    ask: &super::state::PendingApproval,
) {
    use roost_core::chat::ApprovalDecision;
    let mut answer = None;
    let dark = ui.visuals().dark_mode;
    // A command is drawn in the warning red: it is the one ask that can
    // change things outside the agent's folder.
    let (fill, stroke) = if ask.run.is_some() {
        let red = crate::app::bad(dark);
        (
            egui::Color32::from_rgba_unmultiplied(red.r(), red.g(), red.b(), 26),
            red,
        )
    } else {
        (t.warn_soft, t.warn)
    };
    egui::Frame::new()
        .fill(fill)
        .stroke(Stroke::new(1.0, stroke))
        .corner_radius(10.0)
        .inner_margin(Margin::symmetric(14, 12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 8.0;
            // (first button, the session button when there is one)
            let (allow_label, always_label): (&str, Option<String>) = match (&ask.run, &ask.preview)
            {
                (Some(r), _) => {
                    command_summary(ui, t, r);
                    (
                        "Run it",
                        r.prefix.as_ref().map(|p| format!("Allow {p} this session")),
                    )
                }
                (None, Some(p)) => {
                    write_summary(ui, t, p);
                    (
                        "Allow",
                        Some("Allow edits to this file this session".to_string()),
                    )
                }
                (None, None) if ask.name == roost_core::agent::web::TOOL => {
                    // The exact address and its host, in plain text.
                    ui.label(RichText::new("Fetch a web page?").font(semibold(14.0)));
                    ui.add(
                        egui::Label::new(
                            RichText::new("The agent wants to read this address. The page's text goes to the model you chose.")
                                .size(12.5)
                                .color(t.fg2),
                        )
                        .wrap(),
                    );
                    let url = ask
                        .arguments
                        .get("url")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("(no address)");
                    ui.add(
                        egui::Label::new(
                            RichText::new(truncate_chars(url, 300)).font(mono(12.5)).color(t.fg),
                        )
                        .wrap(),
                    );
                    let host = roost_core::agent::web::host_of(&ask.arguments);
                    (
                        "Fetch",
                        host.map(|h| format!("Allow {h} in this chat")),
                    )
                }
                (None, None) => {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new("The agent wants to run").color(t.fg2));
                        ui.label(RichText::new(&ask.name).font(mono(13.0)).color(t.fg));
                    });
                    let args = serde_json::to_string(&ask.arguments).unwrap_or_default();
                    ui.add(
                        egui::Label::new(
                            RichText::new(truncate_chars(&args, 240))
                                .font(mono(12.0))
                                .color(t.fg2),
                        )
                        .wrap(),
                    );
                    ("Allow", Some("Allow for this session".to_string()))
                }
            };
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let allow = primary_button(ui, allow_label, true);
                let always = always_label
                    .as_deref()
                    .map(|label| secondary(ui, label, true));
                let deny = secondary(ui, "Deny", true);
                #[cfg(test)]
                {
                    app.chat_ui.probe.approval_buttons = std::iter::once(allow.rect)
                        .chain(always.as_ref().map(|r| r.rect))
                        .chain(std::iter::once(deny.rect))
                        .collect();
                }
                if allow.clicked() {
                    answer = Some(ApprovalDecision::Allow);
                } else if always.is_some_and(|r| r.clicked()) {
                    answer = Some(ApprovalDecision::AllowForSession);
                } else if deny.clicked() {
                    answer = Some(ApprovalDecision::Deny);
                }
            });
        });
    if let Some(decision) = answer {
        app.chat.approval_answered(s.id);
        out.push(BackendRequest::Chat(ChatCommand::ResolveApproval {
            session: s.id,
            call_id: ask.call_id.clone(),
            decision,
        }));
    }
}

/// A tool needs a working directory and the agent has none: the run waits
/// here, above the input, until a folder is chosen or the call is skipped.
fn folder_card(
    ui: &mut egui::Ui,
    t: &Tokens,
    app: &mut AppState,
    out: &mut Vec<BackendRequest>,
    s: &Session,
    wait: &super::state::PendingFolder,
) {
    let (mut choose, mut skip) = (false, false);
    egui::Frame::new()
        .fill(t.warn_soft)
        .stroke(Stroke::new(1.0, t.warn))
        .corner_radius(10.0)
        .inner_margin(Margin::symmetric(14, 12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 8.0;
            ui.label(RichText::new("No working directory is attached").font(semibold(14.0)));
            ui.add(
                egui::Label::new(
                    RichText::new(format!(
                        "{} needs a folder to work in. Choose one to let the agent continue, or skip and it carries on without files.",
                        wait.name
                    ))
                    .size(12.5)
                    .color(t.fg2),
                )
                .wrap(),
            );
            ui.horizontal_wrapped(|ui| {
                let pick = primary_button(ui, "Choose folder…", true);
                let skip_r = secondary(ui, "Skip", true);
                #[cfg(test)]
                {
                    app.chat_ui.probe.folder_buttons = vec![pick.rect, skip_r.rect];
                }
                choose = pick.clicked();
                skip = skip_r.clicked();
            });
        });
    if choose {
        super::chrome::choose_folder(app, out, s.id);
    } else if skip {
        app.chat.folder_answered(s.id);
        out.push(BackendRequest::Chat(ChatCommand::ProvideWorkspace {
            session: s.id,
            call_id: wait.call_id.clone(),
            path: None,
        }));
    }
}

// ---------------------------------------------------------------- agents

/// One tool step of an agent: what it called, how it went, and the result.
pub(super) fn tool_card(ui: &mut egui::Ui, t: &Tokens, m: &Message, app: &mut AppState) {
    let MessageContent::Tool {
        name,
        arguments,
        output,
        status,
        ..
    } = &m.content
    else {
        return;
    };
    let _ = &app;
    let asked = (name == roost_core::agent::ask::TOOL).then(|| ask_view(arguments, output));
    let args = match &asked {
        Some((question, _)) => question.clone(),
        None => truncate_chars(&arguments.to_string(), 90),
    };
    #[cfg(test)]
    let top = ui.cursor().min.y;
    egui::Frame::new()
        .fill(t.surface2)
        .stroke(Stroke::new(1.0, t.line))
        .corner_radius(8.0)
        .inner_margin(Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new("Tool").font(semibold(11.0)).color(t.fg3));
                ui.label(RichText::new(name).font(semibold(13.0)).color(t.fg));
                match status {
                    ToolStatus::Pending => tag(ui, "Waiting", t.fg2, t.line),
                    ToolStatus::Running => tag(ui, "Running…", t.accent, t.accent_soft),
                    ToolStatus::Failed => tag(ui, "Failed", t.bad, t.bad_soft),
                    ToolStatus::Cancelled => tag(ui, "Stopped", t.warn, t.warn_soft),
                    ToolStatus::Complete => {}
                }
                // When the call started, at the right edge of the title row.
                if let Some((label, full)) =
                    crate::chat::state::local_stamp(m.created_at_ms, now_ms())
                {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new(label).font(mono(11.0)).color(t.fg3))
                            .on_hover_text(format!("Started {full}"));
                    });
                }
            });
            ui.add(
                egui::Label::new(
                    RichText::new(args)
                        .font(crate::theme::mono(11.5))
                        .color(t.fg3),
                )
                .truncate(),
            );
            if let Some((_, Some(answer))) = &asked {
                ui.add(
                    egui::Label::new(
                        RichText::new(format!("You answered: {answer}"))
                            .size(12.5)
                            .color(t.fg2),
                    )
                    .wrap(),
                );
            } else if let Some(result) = output.as_deref().filter(|o| !o.is_empty()) {
                egui::CollapsingHeader::new(RichText::new("Result").size(12.0).color(t.fg2))
                    .id_salt(("tool-result", m.id))
                    .show(ui, |ui| {
                        ui.add(
                            egui::Label::new(
                                RichText::new(truncate_chars(result, 4000))
                                    .font(crate::theme::mono(11.5))
                                    .color(t.fg2),
                            )
                            .selectable(true)
                            .wrap(),
                        );
                    });
            }
        });
    #[cfg(test)]
    app.chat_ui.probe.tool_cards.push(egui::Rect::from_min_max(
        egui::pos2(ui.min_rect().left(), top),
        egui::pos2(ui.min_rect().right(), ui.cursor().min.y),
    ));
    ui.add_space(8.0);
}

/// A question the agent asked and, once there is one, the answer in words.
pub(super) fn ask_view(
    arguments: &serde_json::Value,
    output: &Option<String>,
) -> (String, Option<String>) {
    let question = arguments
        .get("question")
        .and_then(|q| q.as_str())
        .unwrap_or_default()
        .to_string();
    let answer = output.as_deref().filter(|o| !o.is_empty()).map(|o| {
        if o == roost_core::agent::ask::SKIPPED {
            return "skipped".to_string();
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(o) else {
            return o.to_string();
        };
        if let Some(a) = v.get("answer").and_then(|a| a.as_str()) {
            return a.to_string();
        }
        match v.get("answers").and_then(|a| a.as_array()) {
            Some(list) => {
                let mut parts: Vec<String> = list
                    .iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect();
                parts.extend(v.get("other").and_then(|x| x.as_str()).map(str::to_string));
                parts.join(", ")
            }
            None => o.to_string(),
        }
    });
    (question, answer)
}

/// The model shown for a session: the rail row is the live source.
fn row_model(app: &AppState, s: &Session) -> String {
    app.chat
        .rows()
        .iter()
        .find(|r| r.id == s.id)
        .map_or_else(|| s.model.clone(), |r| r.model.clone())
}

fn retry_row(
    ui: &mut egui::Ui,
    app: &mut AppState,
    out: &mut Vec<BackendRequest>,
    s: &Session,
    last: &Message,
) {
    let retryable = matches!(
        &last.content,
        MessageContent::Assistant {
            status: AssistantStatus::Failed
                | AssistantStatus::Cancelled
                | AssistantStatus::Interrupted,
            ..
        }
    );
    if retryable && !app.chat.is_running(s.id) && small_button(ui, "Retry") {
        if let SendAttempt::Sent(cmd) = app.chat.try_retry(s.id) {
            out.push(BackendRequest::Chat(cmd));
        }
    }
}

pub(super) fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let cut: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{cut}…")
}

fn message_card(
    ui: &mut egui::Ui,
    t: &Tokens,
    m: &Message,
    model: &str,
    badge: RunBadge,
    cx: &mut ChatUi,
) -> egui::Rect {
    let is_assistant = matches!(m.content, MessageContent::Assistant { .. });
    let is_you = matches!(m.content, MessageContent::User { .. });
    let (who, fill, body, status, error, thinking) = match &m.content {
        MessageContent::User { text, .. } => ("You", t.surface2, text.clone(), None, None, None),
        MessageContent::Assistant {
            text,
            status,
            error,
            thinking,
            ..
        } => (
            model,
            t.surface,
            text.clone(),
            Some(*status),
            error.clone(),
            thinking.clone(),
        ),
        MessageContent::Tool { name, output, .. } => (
            name.as_str(),
            t.surface2,
            output.clone().unwrap_or_default(),
            None,
            None,
            None,
        ),
    };
    // A reply that only thought (no words, no error, not still coming) is one
    // quiet line, not an empty card.
    if is_assistant
        && body.is_empty()
        && error.is_none()
        && status != Some(AssistantStatus::Streaming)
        && thinking.as_ref().is_some_and(|x| !x.is_empty())
    {
        let th = thinking.unwrap_or_default();
        let line = ui.scope(|ui| {
            egui::CollapsingHeader::new(RichText::new("Thinking").size(12.0).color(t.fg3))
                .id_salt(("thinking", m.id))
                .show(ui, |ui| {
                    ui.add(
                        egui::Label::new(RichText::new(th).size(12.5).color(t.fg2))
                            .selectable(true)
                            .wrap(),
                    );
                });
        });
        return line.response.rect;
    }
    let card = egui::Frame::new()
        .fill(fill)
        .stroke(Stroke::new(1.0, t.line))
        .corner_radius(10.0)
        .inner_margin(Margin::symmetric(14, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(who).font(semibold(12.0)).color(t.fg2));
                if let Some(label) = status.and_then(status_label) {
                    let (fg, bg) = match status {
                        Some(AssistantStatus::Failed) => (t.bad, t.bad_soft),
                        _ => (t.warn, t.warn_soft),
                    };
                    tag(ui, label, fg, bg);
                }
            });
            if let Some(th) = thinking.filter(|x| !x.is_empty()) {
                egui::CollapsingHeader::new(RichText::new("Thinking").size(12.0).color(t.fg3))
                    .id_salt(("thinking", m.id))
                    .show(ui, |ui| {
                        ui.add(
                            egui::Label::new(RichText::new(th).size(12.5).color(t.fg2))
                                .selectable(true)
                                .wrap(),
                        );
                    });
            }
            if body.is_empty() && status == Some(AssistantStatus::Streaming) {
                let wait = if badge == RunBadge::Waiting {
                    "Waiting for model…"
                } else {
                    "Loading model…"
                };
                ui.label(RichText::new(wait).color(t.fg3));
            } else if !body.is_empty() && is_assistant {
                let len = body.len();
                if cx.md_cache.len() > 400 {
                    cx.md_cache.clear();
                }
                let entry = cx.md_cache.entry(m.id).or_default();
                if entry.0 != len || entry.1.is_empty() {
                    *entry = (len, markdown::parse_capped(&body).0);
                }
                crate::browse::about::render(ui, t, &entry.1, t.fg);
            } else if !body.is_empty() {
                ui.add(
                    egui::Label::new(RichText::new(body).size(14.0).color(t.fg))
                        .selectable(true)
                        .wrap(),
                );
            }
            if let Some(e) = error {
                ui.add(egui::Label::new(RichText::new(e).size(12.5).color(t.bad)).wrap());
            }
        });
    if is_you {
        // Your messages are marked by an accent edge, not filled with it.
        let r = card.response.rect;
        ui.painter().rect_filled(
            egui::Rect::from_min_size(
                r.min + vec2(0.0, 6.0),
                vec2(3.0, (r.height() - 12.0).max(0.0)),
            ),
            1.5,
            t.accent,
        );
    }
    card.response.rect
}

const ACTION_H: f32 = 28.0;

/// One icon button in the row under a bubble.
fn action_icon(
    ui: &mut egui::Ui,
    t: &Tokens,
    rect: egui::Rect,
    id: egui::Id,
    icon: crate::icons::Icon,
    tip: &str,
) -> egui::Response {
    let resp = ui
        .interact(rect, id, Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(tip);
    if resp.hovered() {
        ui.painter().rect_filled(rect.shrink(1.0), 6.0, t.line);
    }
    let color = if resp.hovered() { t.fg } else { t.fg3 };
    crate::icons::paint(ui.painter(), rect, icon, color);
    resp
}

/// The row of icons under a bubble: always on the latest message, on hover
/// for older ones. Its height is reserved either way so nothing moves.
#[allow(clippy::too_many_arguments)]
fn message_actions(
    ui: &mut egui::Ui,
    t: &Tokens,
    app: &mut AppState,
    out: &mut Vec<BackendRequest>,
    s: &Session,
    (index, m): (usize, &Message),
    model: &str,
    is_latest: bool,
    card: egui::Rect,
) {
    use crate::icons::{slot, Icon};
    let (text, status, stats) = match &m.content {
        MessageContent::User { text, .. } => (text, None, None),
        MessageContent::Assistant {
            text,
            status,
            stats,
            ..
        } => (text, Some(*status), *stats),
        MessageContent::Tool { .. } => return,
    };
    // Nothing to put in the row (an empty reply that is done): no room for it.
    if text.is_empty() && status != Some(AssistantStatus::Streaming) && s.version_of(m.id).is_none()
    {
        return;
    }
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), ACTION_H), Sense::hover());
    if let Some((n, total)) = s.version_of(m.id) {
        let after = index.checked_sub(1).map(|i| s.messages[i].id);
        let idle = !app.chat.is_running(s.id);
        version_counter(ui, t, app, out, row, (s.id, after), (n, total), idle);
    }
    if text.is_empty() || status == Some(AssistantStatus::Streaming) {
        return;
    }
    let details_id = egui::Id::unique("details").with(m.id);
    let branch_popup = egui::Id::unique("branch").with(m.id).with("popup");
    let btw_popup = egui::Id::unique("btw").with(m.id).with("popup");
    let popup_open = egui::Popup::is_id_open(ui.ctx(), details_id.with("popup"))
        || egui::Popup::is_id_open(ui.ctx(), branch_popup)
        || egui::Popup::is_id_open(ui.ctx(), btw_popup);
    if !(is_latest || popup_open || ui.rect_contains_pointer(card.union(row))) {
        return;
    }
    let mut n = 0;
    let mut next = || {
        n += 1;
        slot(row, n - 1)
    };
    let copy = action_icon(
        ui,
        t,
        next(),
        egui::Id::unique("copy").with(m.id),
        Icon::Copy,
        "Copy this message",
    );
    #[cfg(test)]
    app.chat_ui.probe.copy_buttons.push(copy.rect);
    if copy.clicked() {
        ui.ctx().copy_text(text.clone());
    }
    let yours = matches!(m.content, MessageContent::User { .. });
    if yours && s.mode == SessionMode::Chat && !app.chat.is_running(s.id) {
        let r = action_icon(
            ui,
            t,
            next(),
            egui::Id::unique("edit").with(m.id),
            Icon::Edit,
            "Edit and resend",
        );
        #[cfg(test)]
        {
            app.chat_ui.probe.edit_icon = Some(r.rect);
        }
        if r.clicked() {
            app.chat_ui.editing = Some((m.id, text.clone()));
        }
    }
    let complete = status == Some(AssistantStatus::Complete);
    if complete
        && is_latest
        && s.mode == SessionMode::Chat
        && s.messages.len() >= 2
        && !app.chat.is_running(s.id)
    {
        let r = action_icon(
            ui,
            t,
            next(),
            egui::Id::unique("regen").with(m.id),
            Icon::Regenerate,
            "Ask again. This reply stays as a version",
        );
        #[cfg(test)]
        {
            app.chat_ui.probe.regenerate = Some(r.rect);
        }
        if r.clicked() {
            if let SendAttempt::Sent(cmd) = app.chat.try_regenerate(s.id) {
                out.push(BackendRequest::Chat(cmd));
            }
        }
    }
    if complete {
        let branch_id = egui::Id::unique("branch").with(m.id);
        let r = action_icon(ui, t, next(), branch_id, Icon::Branch, "Branch from here");
        #[cfg(test)]
        {
            app.chat_ui.probe.branch_icon = Some(r.rect);
        }
        let mut pick = false;
        let mut plain = false;
        egui::Popup::menu(&r).show(|ui| {
            let a = ui.button("Branch into a new chat");
            let b = ui.button("Branch with another model…");
            #[cfg(test)]
            {
                app.chat_ui.probe.branch_plain = Some(a.rect);
                app.chat_ui.probe.branch_pick = Some(b.rect);
            }
            plain = a.clicked();
            pick = b.clicked();
            if plain || pick {
                ui.close();
            }
        });
        if plain {
            out.push(BackendRequest::Chat(ChatCommand::Branch {
                session: s.id,
                message: m.id,
                model: None,
            }));
        }
        if pick {
            app.chat_ui.branch_dialog = Some(BranchDialog {
                session: s.id,
                message: m.id,
                model: s.model.clone(),
            });
        }
        let btw_r = action_icon(
            ui,
            t,
            next(),
            egui::Id::unique("btw").with(m.id),
            Icon::Btw,
            "Side question (BTW)",
        );
        #[cfg(test)]
        {
            app.chat_ui.probe.btw_icon = Some(btw_r.rect);
        }
        egui::Popup::from_toggle_button_response(&btw_r)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .show(|ui| btw_panel(ui, t, app, out, s, m));
        read_icon(ui, t, app, next(), m, text);
        let r = action_icon(ui, t, next(), details_id, Icon::Details, "Details");
        #[cfg(test)]
        {
            app.chat_ui.probe.details = Some(r.rect);
        }
        egui::Popup::from_toggle_button_response(&r)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .show(|ui| details_card(ui, t, model, stats));
    }
    message_time(ui, t, app, slot(row, n), m, card);
}

/// When the message was sent, muted, after the icons. The bubble carries the
/// full date and time as a tooltip.
fn message_time(
    ui: &mut egui::Ui,
    t: &Tokens,
    app: &mut AppState,
    after: egui::Rect,
    m: &Message,
    card: egui::Rect,
) {
    let now = now_ms();
    let Some((label, full)) = crate::chat::state::local_stamp(m.created_at_ms, now) else {
        return;
    };
    let at = egui::pos2(after.left() + 6.0, after.center().y);
    let shown = ui
        .painter()
        .text(at, egui::Align2::LEFT_CENTER, &label, mono(11.0), t.fg3);
    #[cfg(test)]
    app.chat_ui.probe.stamps.push((shown, label));
    #[cfg(not(test))]
    let _ = (shown, &app);
    ui.interact(card, egui::Id::unique("sent").with(m.id), Sense::hover())
        .on_hover_text(full);
}

/// Read aloud / Stop under a finished reply. Disabled, with the reason, when
/// this computer has no speech tool.
fn read_icon(
    ui: &mut egui::Ui,
    t: &Tokens,
    app: &mut AppState,
    rect: egui::Rect,
    m: &Message,
    text: &str,
) {
    use crate::icons::Icon;
    let id = egui::Id::unique("read").with(m.id);
    if let Err(why) = app.speaker.available() {
        let r = ui.interact(rect, id, Sense::hover()).on_hover_text(why);
        crate::icons::paint(ui.painter(), r.rect, Icon::Read, t.line);
        #[cfg(test)]
        {
            app.chat_ui.probe.read_icon = Some(rect);
            app.chat_ui.probe.read_disabled = true;
        }
        return;
    }
    let reading = app.chat_ui.reading == Some(m.id);
    let (icon, tip) = if reading {
        (Icon::Stop, "Stop reading")
    } else {
        (Icon::Read, "Read aloud")
    };
    let r = action_icon(ui, t, rect, id, icon, tip);
    #[cfg(test)]
    {
        app.chat_ui.probe.read_icon = Some(rect);
    }
    if r.clicked() {
        if reading {
            app.speaker.stop();
            app.chat_ui.reading = None;
        } else {
            app.speaker.speak(&crate::speech::plain_text(text));
            app.chat_ui.reading = Some(m.id);
        }
    }
}

/// The side-question popover under a reply.
fn btw_panel(
    ui: &mut egui::Ui,
    t: &Tokens,
    app: &mut AppState,
    out: &mut Vec<BackendRequest>,
    s: &Session,
    m: &Message,
) {
    ui.set_min_width(360.0);
    ui.set_max_width(480.0);
    ui.label(
        RichText::new("Side question. Not added to this chat.")
            .font(mono(11.0))
            .color(t.warn),
    );
    ui.add_space(4.0);
    let pending = app.chat.btw_pending(s.id).map(str::to_string);
    let idle = !app.chat.is_running(s.id) && s.mode == SessionMode::Chat;
    ui.horizontal(|ui| {
        let edit = ui.add(
            egui::TextEdit::singleline(&mut app.chat_ui.btw_input)
                .hint_text("Ask about this reply…")
                .desired_width(ui.available_width() - 64.0),
        );
        let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
        let can = pending.is_none() && !app.chat_ui.btw_input.trim().is_empty();
        let ask = primary_button(ui, "Ask", can);
        #[cfg(test)]
        {
            app.chat_ui.probe.btw_ask = Some(ask.rect);
        }
        if (ask.clicked() || enter) && can {
            let question = std::mem::take(&mut app.chat_ui.btw_input);
            out.push(BackendRequest::Chat(ChatCommand::Btw {
                session: s.id,
                message: m.id,
                question,
            }));
        }
    });
    if let Some(q) = &pending {
        ui.add_space(6.0);
        ui.label(RichText::new(q).size(12.5).color(t.fg3));
        ui.label(RichText::new("Thinking…").color(t.fg2));
    } else if let Some(e) = app.chat.btw_error(s.id) {
        ui.add_space(6.0);
        ui.add(egui::Label::new(RichText::new(e).size(12.5).color(t.bad)).wrap());
    }
    let shown = s.btw.iter().find(|e| e.at == m.id).map(|e| e.id);
    if let (Some(entry), None) = (s.btw.iter().find(|e| e.at == m.id), &pending) {
        ui.add_space(8.0);
        ui.separator();
        ui.label(RichText::new(&entry.question).size(12.5).color(t.fg3));
        let blocks = markdown::parse_capped(&entry.answer).0;
        crate::browse::about::render(ui, t, &blocks, t.fg);
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let add = ui.add_enabled(
                idle && !entry.added,
                egui::Button::new(RichText::new("Add to chat").size(12.0)),
            );
            #[cfg(test)]
            {
                app.chat_ui.probe.btw_add = Some(add.rect);
            }
            if add.clicked() {
                out.push(BackendRequest::Chat(ChatCommand::BtwAdd {
                    session: s.id,
                    id: entry.id,
                }));
            }
            if entry.added {
                ui.label(RichText::new("Added").size(12.0).color(t.fg3));
            }
            if ui.button(RichText::new("Dismiss").size(12.0)).clicked() {
                ui.close();
            }
        });
    }
    let earlier: Vec<_> = s.btw.iter().filter(|e| Some(e.id) != shown).collect();
    if !earlier.is_empty() {
        ui.add_space(6.0);
        egui::CollapsingHeader::new(
            RichText::new(format!("Earlier side questions ({})", earlier.len()))
                .size(12.0)
                .color(t.fg3),
        )
        .id_salt(("btw-history", s.id))
        .show(ui, |ui| {
            for e in earlier {
                ui.label(RichText::new(&e.question).font(semibold(12.5)).color(t.fg2));
                ui.add(
                    egui::Label::new(
                        RichText::new(truncate_chars(&e.answer, 160))
                            .size(12.0)
                            .color(t.fg3),
                    )
                    .wrap(),
                );
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            idle && !e.added,
                            egui::Button::new(RichText::new("Add to chat").size(11.5)),
                        )
                        .clicked()
                    {
                        out.push(BackendRequest::Chat(ChatCommand::BtwAdd {
                            session: s.id,
                            id: e.id,
                        }));
                    }
                    if ui.button(RichText::new("Delete").size(11.5)).clicked() {
                        out.push(BackendRequest::Chat(ChatCommand::BtwDelete {
                            session: s.id,
                            id: e.id,
                        }));
                    }
                });
                ui.add_space(4.0);
            }
        });
    }
}

/// Where a branched chat came from, at the top of the transcript.
fn branch_strip(
    ui: &mut egui::Ui,
    t: &Tokens,
    app: &mut AppState,
    out: &mut Vec<BackendRequest>,
    s: &Session,
) {
    let (Some(origin), false) = (s.parent, s.lineage.is_empty()) else {
        return;
    };
    if app.chat.branch_check_due(s.id) {
        out.push(BackendRequest::Chat(ChatCommand::CheckBranch(s.id)));
    }
    let state = app.chat.branch_view(s.id);
    let gone = state == Some(roost_core::chat::BranchState::Deleted);
    let changed = state == Some(roost_core::chat::BranchState::Changed);
    let snippet = s
        .messages
        .iter()
        .find(|m| m.id == origin.message_id)
        .and_then(|m| match &m.content {
            MessageContent::Assistant { text, .. } => Some(truncate_chars(
                &text.split_whitespace().collect::<Vec<_>>().join(" "),
                30,
            )),
            _ => None,
        })
        .unwrap_or_default();
    let (stroke, fill) = if changed {
        (t.warn, t.warn_soft)
    } else {
        (t.line, t.surface)
    };
    let mut open: Option<(SessionId, roost_core::session::MessageId)> = None;
    egui::Frame::new()
        .fill(fill)
        .stroke(Stroke::new(1.0, stroke))
        .corner_radius(8.0)
        .inner_margin(Margin::symmetric(12, 7))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.label(RichText::new("↳ Branched from").size(12.5).color(t.fg2));
                let chain = &s.lineage;
                let first_shown = chain.len().saturating_sub(3);
                if first_shown > 0 {
                    let all: Vec<&str> = chain.iter().map(|a| a.title.as_str()).collect();
                    ui.label(RichText::new("…").color(t.fg3))
                        .on_hover_text(all.join(" › "));
                    ui.label(RichText::new("›").color(t.fg3));
                }
                for (i, a) in chain.iter().enumerate().skip(first_shown) {
                    if i > first_shown {
                        ui.label(RichText::new("›").color(t.fg3));
                    }
                    let last = i + 1 == chain.len();
                    if last && gone {
                        ui.label(RichText::new(&a.title).size(12.5).color(t.fg2));
                        continue;
                    }
                    let link = ui
                        .add(egui::Link::new(
                            RichText::new(&a.title).size(12.5).color(t.accent),
                        ))
                        .on_hover_text("Open this chat");
                    #[cfg(test)]
                    app.chat_ui
                        .probe
                        .strip_links
                        .push((a.session_id, link.rect));
                    if link.clicked() {
                        open = Some((a.session_id, a.message_id));
                    }
                }
                if !snippet.is_empty() {
                    ui.label(RichText::new("at").size(12.5).color(t.fg3));
                    ui.label(
                        RichText::new(format!("“{snippet}”"))
                            .size(12.5)
                            .color(t.fg2),
                    );
                }
                match state {
                    Some(roost_core::chat::BranchState::Unchanged) => {
                        tag(ui, "unchanged", t.ok, t.accent_soft);
                    }
                    Some(roost_core::chat::BranchState::Changed) => {
                        ui.label(
                            RichText::new("The original changed after this branch was made")
                                .size(12.5)
                                .color(t.warn),
                        );
                        let r = secondary(ui, "View original", true);
                        #[cfg(test)]
                        {
                            app.chat_ui.probe.view_original = Some(r.rect);
                        }
                        if r.clicked() {
                            open = Some((origin.session_id, origin.message_id));
                        }
                    }
                    Some(roost_core::chat::BranchState::Deleted) => {
                        ui.label(
                            RichText::new("The original chat was deleted")
                                .size(12.5)
                                .color(t.fg3),
                        );
                    }
                    None => {}
                }
            });
        });
    ui.add_space(10.0);
    if let Some((id, message)) = open {
        app.chat_ui.scroll_to = Some(message);
        open_chat(app, out, id, Some((s.id, s.lifetime)));
    }
}

/// Marks where the history copied from the original ends.
fn branch_divider(ui: &mut egui::Ui, t: &Tokens, s: &Session) {
    let title = s
        .lineage
        .last()
        .map_or("the original", |a| a.title.as_str());
    ui.add_space(4.0);
    ui.vertical_centered(|ui| {
        ui.label(
            RichText::new(format!(
                "branch point · history above is copied from “{title}”"
            ))
            .font(mono(11.5))
            .color(t.fg3),
        );
    });
    ui.add_space(8.0);
}

/// The "Branch with another model" window.
fn branch_dialog(
    ui: &mut egui::Ui,
    t: &Tokens,
    app: &mut AppState,
    out: &mut Vec<BackendRequest>,
    installed: &[String],
) {
    let Some(dialog) = app.chat_ui.branch_dialog.as_mut() else {
        return;
    };
    let mut create = false;
    let mut cancel = false;
    let mut open = true;
    egui::Window::new("Branch with another model")
        .collapsible(false)
        .resizable(false)
        .open(&mut open)
        .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0))
        .show(ui.ctx(), |ui| {
            ui.set_max_width(380.0);
            ui.add(
                egui::Label::new(
                    RichText::new(
                        "The new chat starts with everything up to this reply, then continues with:",
                    )
                    .color(t.fg2),
                )
                .wrap(),
            );
            ui.add_space(6.0);
            egui::ComboBox::from_id_salt("branch_model")
                .selected_text(dialog.model.clone())
                .width(300.0)
                .show_ui(ui, |ui| {
                    let mut names: Vec<&str> = installed.iter().map(String::as_str).collect();
                    if !names.contains(&dialog.model.as_str()) {
                        names.insert(0, dialog.model.as_str());
                    }
                    let names: Vec<String> = names.into_iter().map(str::to_string).collect();
                    for name in names {
                        ui.selectable_value(&mut dialog.model, name.clone(), name);
                    }
                });
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                let r = primary_button(ui, "Create branch", true);
                #[cfg(test)]
                {
                    app.chat_ui.probe.branch_create = Some(r.rect);
                }
                create = r.clicked();
                cancel = secondary(ui, "Cancel", true).clicked();
            });
        });
    if create {
        if let Some(d) = app.chat_ui.branch_dialog.take() {
            out.push(BackendRequest::Chat(ChatCommand::Branch {
                session: d.session,
                message: d.message,
                model: Some(d.model),
            }));
        }
    } else if cancel || !open {
        app.chat_ui.branch_dialog = None;
    }
}

/// `‹ 2 / 3 ›` at the right end of the row, whatever the hover state.
#[allow(clippy::too_many_arguments)]
fn version_counter(
    ui: &mut egui::Ui,
    t: &Tokens,
    app: &mut AppState,
    out: &mut Vec<BackendRequest>,
    row: egui::Rect,
    (session, after): (SessionId, Option<roost_core::session::MessageId>),
    (n, total): (usize, usize),
    enabled: bool,
) {
    use crate::icons::Icon;
    let label = format!("{n} / {total}");
    let galley = ui.painter().layout_no_wrap(label, mono(12.0), t.fg2);
    let w = galley.size().x;
    let right = row.right() - 2.0;
    let next_r = egui::Rect::from_min_size(
        egui::pos2(right - 28.0, row.top()),
        vec2(28.0, row.height()),
    );
    let text_r = egui::Rect::from_min_size(
        egui::pos2(next_r.left() - w - 6.0, row.top()),
        vec2(w + 6.0, row.height()),
    );
    let prev_r = egui::Rect::from_min_size(
        egui::pos2(text_r.left() - 28.0, row.top()),
        vec2(28.0, row.height()),
    );
    ui.painter().galley(
        egui::pos2(text_r.left() + 3.0, row.center().y - galley.size().y / 2.0),
        galley,
        t.fg2,
    );
    let key = (session, after);
    let step = |ui: &mut egui::Ui, rect, icon, tip, id_salt: &str, ok: bool| {
        let id = egui::Id::unique(id_salt).with(key.0).with(key.1);
        if ok && enabled {
            action_icon(ui, t, rect, id, icon, tip).clicked()
        } else {
            let color = t.line;
            crate::icons::paint(ui.painter(), rect, icon, color);
            false
        }
    };
    let go_prev = step(
        ui,
        prev_r,
        Icon::Prev,
        "Previous version",
        "ver-prev",
        n > 1,
    );
    let go_next = step(
        ui,
        next_r,
        Icon::Next,
        "Next version",
        "ver-next",
        n < total,
    );
    #[cfg(test)]
    {
        app.chat_ui.probe.version_counter = Some(prev_r.union(next_r));
        app.chat_ui.probe.version_prev = Some(prev_r);
        app.chat_ui.probe.version_next = Some(next_r);
    }
    let to = if go_prev {
        Some(n - 2)
    } else if go_next {
        Some(n)
    } else {
        None
    };
    if let Some(to) = to {
        out.push(BackendRequest::Chat(ChatCommand::SwitchVersion {
            session,
            after,
            to,
        }));
    }
    let _ = app;
}

/// The bubble of one of your messages while it is being edited.
fn edit_box(
    ui: &mut egui::Ui,
    t: &Tokens,
    app: &mut AppState,
    out: &mut Vec<BackendRequest>,
    s: &Session,
) -> egui::Rect {
    let mut send = false;
    let mut cancel = false;
    let Some((mid, text)) = app.chat_ui.editing.as_mut() else {
        return egui::Rect::NOTHING;
    };
    let mid = *mid;
    let frame = egui::Frame::new()
        .fill(t.accent_soft)
        .stroke(Stroke::new(1.0, t.accent))
        .corner_radius(10.0)
        .inner_margin(Margin::symmetric(14, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                RichText::new("Edit message")
                    .font(semibold(12.0))
                    .color(t.fg2),
            );
            ui.add(
                egui::TextEdit::multiline(text)
                    .desired_rows(2)
                    .desired_width(ui.available_width()),
            );
            ui.label(
                RichText::new("Sends as a new version. The original stays one click away.")
                    .size(11.5)
                    .color(t.fg3),
            );
            ui.horizontal(|ui| {
                let blank = text.trim().is_empty();
                let r = primary_button(ui, "Send", !blank);
                #[cfg(test)]
                {
                    app.chat_ui.probe.edit_send = Some(r.rect);
                }
                send = r.clicked() && !blank;
                cancel = secondary(ui, "Cancel", true).clicked();
            });
        });
    if send {
        if let Some((_, text)) = app.chat_ui.editing.take() {
            app.chat.sending_started(s.id);
            out.push(BackendRequest::Chat(ChatCommand::Edit {
                session: s.id,
                message: mid,
                text,
            }));
        }
    } else if cancel {
        app.chat_ui.editing = None;
    }
    frame.response.rect
}

fn details_card(
    ui: &mut egui::Ui,
    t: &Tokens,
    model: &str,
    stats: Option<roost_core::session::ReplyStats>,
) {
    ui.set_min_width(230.0);
    let line = |ui: &mut egui::Ui, label: &str, value: String| {
        ui.horizontal(|ui| {
            ui.label(RichText::new(label).size(12.0).color(t.fg3));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(RichText::new(value).size(12.0).color(t.fg));
            });
        });
    };
    line(ui, "Model", model.to_string());
    let Some(s) = stats else {
        ui.label(
            RichText::new("No timing was recorded for this reply.")
                .size(12.0)
                .color(t.fg3),
        );
        return;
    };
    let count = |n: Option<u64>| n.map_or("unknown".to_string(), |n| format!("{n} tokens"));
    line(ui, "Prompt", count(s.prompt_tokens));
    line(ui, "Reply", count(s.reply_tokens));
    if let Some(tps) = s.tokens_per_second() {
        line(ui, "Speed", format!("{tps:.1} tokens/s"));
    }
    if let Some(ms) = s.total_ms {
        line(ui, "Time", format!("{:.1} s", ms as f64 / 1000.0));
    }
    if let Some(ms) = s.load_ms.filter(|ms| *ms >= 100) {
        line(ui, "Model load", format!("{:.1} s", ms as f64 / 1000.0));
    }
}

fn inspect(
    ui: &mut egui::Ui,
    app: &mut AppState,
    out: &mut Vec<BackendRequest>,
    t: &Tokens,
    id: SessionId,
    now: Instant,
) {
    let Some(row) = app.chat.rows().iter().find(|r| r.id == id).cloned() else {
        app.chat_ui.inspect = None;
        return;
    };
    let why = match row.status {
        RowStatus::Quarantined => {
            "This session file was damaged. Roost set it aside and kept it for diagnosis."
        }
        RowStatus::NewerSchema => {
            "This session was made by a newer version of Roost. It is left untouched so it still works there."
        }
        RowStatus::Unreadable => "This session file could not be read right now.",
        RowStatus::Ok => "This session is available.",
    };
    ui.label(RichText::new(&row.title).font(semibold(16.0)));
    ui.add_space(4.0);
    ui.add(egui::Label::new(RichText::new(why).color(t.fg2)).wrap());
    ui.add_space(8.0);
    let key = id.to_string();
    let label = if app.chat_ui.confirm.is_armed(&key, now) {
        "Confirm delete"
    } else {
        "Delete"
    };
    let r = secondary(ui, label, true);
    #[cfg(test)]
    {
        app.chat_ui.probe.delete_button = Some((r.rect, label.to_string()));
    }
    if r.clicked() && app.chat_ui.confirm.click(&key, now) {
        out.push(BackendRequest::Chat(ChatCommand::Delete(id)));
        app.chat_ui.inspect = None;
    }
}

// ------------------------------------------------------------ loaded models

/// Ollama lists `llama3` and `llama3:latest` for the same model.
pub(super) fn same_model(a: &str, b: &str) -> bool {
    a.strip_suffix(":latest").unwrap_or(a) == b.strip_suffix(":latest").unwrap_or(b)
}

/// "unloads in 5 min", "stays loaded" and so on, from Ollama's `expires_at`.
pub(super) fn unload_note(expires_at: &str, now: chrono::DateTime<chrono::Utc>) -> Option<String> {
    let at = chrono::DateTime::parse_from_rfc3339(expires_at)
        .ok()?
        .to_utc();
    let left = at - now;
    Some(if left > chrono::Duration::hours(24) {
        "stays loaded".to_string()
    } else if left <= chrono::Duration::zero() {
        "unloading".to_string()
    } else if left < chrono::Duration::minutes(1) {
        "unloads in under a minute".to_string()
    } else {
        format!(
            "unloads in {} min",
            (left.num_milliseconds() + 59_999) / 60_000
        )
    })
}

// ------------------------------------------------------------- the composer

fn composer(
    ui: &mut egui::Ui,
    app: &mut AppState,
    out: &mut Vec<BackendRequest>,
    t: &Tokens,
    s: &Session,
    installed: &[String],
) {
    let id = s.id;
    let running = app.chat.is_running(id);
    // The question sits right above the input, where it cannot scroll away.
    if let Some(ask) = app.chat.approval(id).cloned() {
        approval_card(ui, t, app, out, s, &ask);
        ui.add_space(8.0);
    }
    if let Some(ask) = app.chat.question(id).cloned() {
        question_card(ui, t, app, out, s, &ask);
        ui.add_space(8.0);
    }
    if let Some(wait) = app.chat.folder_wait(id).cloned() {
        folder_card(ui, t, app, out, s, &wait);
        ui.add_space(8.0);
    }
    super::chrome::run_allow_banner(ui, app, out, t, id);
    super::chrome::status_line(ui, app, t, s);
    let eid = composer_id(id);
    if std::mem::take(&mut app.chat_ui.focus_composer) {
        ui.memory_mut(|m| m.request_focus(eid));
    }
    // Plain Enter never types a newline here; it sends, or does nothing
    // while a reply is running. Shift+Enter still adds a line.
    let focused = ui.memory(|m| m.has_focus(eid));
    // `consume_key` ignores an extra Shift, so look for it first: Shift+Enter
    // must reach the text field and add a line.
    let enter = focused
        && ui.input_mut(|i| {
            let shifted = i.modifiers.shift
                || i.events.iter().any(|e| {
                    matches!(e, egui::Event::Key { key: Key::Enter, pressed: true, modifiers, .. } if modifiers.shift)
                });
            !shifted && i.consume_key(Modifiers::NONE, Key::Enter)
        });
    let mut send = enter;
    let mut stop = false;
    let agent = app.chat.is_agent(id);
    let hint = if agent {
        "Message the agent. While it works, Enter steers it."
    } else {
        "Message (Enter to send, Shift+Enter for a new line)"
    };
    let has_text = !app.chat.draft(id).trim().is_empty();
    // The input is a rounded bubble that starts one line high and grows with
    // the text; the send button floats inside it, at the bottom right.
    let bubble = egui::Frame::new()
        .fill(t.surface)
        .stroke(egui::Stroke::new(1.0, t.line))
        .corner_radius(18.0)
        .inner_margin(egui::Margin {
            left: 14,
            right: (SEND_SIZE + 14.0) as i8,
            top: 9,
            bottom: 9,
        })
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .max_height(COMPOSER_MAX_H)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::multiline(app.chat.draft_mut(id))
                            .id(eid)
                            .desired_rows(1)
                            .desired_width(ui.available_width())
                            .frame(egui::Frame::NONE)
                            .hint_text(hint),
                    );
                });
        });
    let box_rect = bubble.response.rect;
    let button = egui::Rect::from_min_size(
        egui::pos2(
            box_rect.right() - SEND_SIZE - SEND_GAP,
            box_rect.bottom() - SEND_SIZE - SEND_GAP,
        ),
        egui::vec2(SEND_SIZE, SEND_SIZE),
    );
    let live = running || has_text;
    let resp = ui
        .interact(
            button,
            composer_id(id).with("send"),
            if live {
                egui::Sense::click()
            } else {
                egui::Sense::hover()
            },
        )
        .on_hover_text(if running { "Stop" } else { "Send" });
    let (fill, glyph, icon) = if running {
        (t.bad, egui::Color32::WHITE, crate::icons::Icon::Stop)
    } else if has_text {
        (t.accent, t.accent_fg, crate::icons::Icon::SendUp)
    } else {
        (t.line, t.fg3, crate::icons::Icon::SendUp)
    };
    ui.painter()
        .circle_filled(button.center(), SEND_SIZE / 2.0, fill);
    crate::icons::paint(ui.painter(), button, icon, glyph);
    if running {
        stop = resp.clicked();
    } else if has_text {
        send |= resp.clicked();
    }
    #[cfg(test)]
    {
        app.chat_ui.probe.input_rect = Some(box_rect);
        app.chat_ui.probe.send_button = Some((button, if running { "Stop" } else { "Send" }));
    }
    ui.add_space(4.0);
    // The pills under the input: what can be added and the tools on the left,
    // what the agent may do and the model on the right.
    let compact = ui.available_width() < super::chrome::COMPACT_PILLS_W;
    ui.horizontal(|ui| {
        super::chrome::plus_menu(ui, app, out, t, s);
        if agent {
            super::chrome::agent_settings(ui, app, out, t, s);
            super::chrome::folder_pill(ui, app, out, t, s, compact);
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let name_chars = if compact { 14 } else { 28 };
            // As tall as the pills beside it.
            ui.scope(|ui| {
                ui.spacing_mut().button_padding = vec2(8.0, 2.0);
                ui.spacing_mut().interact_size.y = super::chrome::PILL_H;
                super::chrome::model_chip(ui, app, out, t, s, installed, name_chars);
            });
            if agent {
                super::chrome::permission_pills(ui, app, out, t, s);
            }
        });
    });
    if stop {
        out.push(BackendRequest::Chat(ChatCommand::Stop(id)));
    }
    if send {
        match app.chat.try_send(id) {
            SendAttempt::Sent(cmd) => out.push(BackendRequest::Chat(cmd)),
            SendAttempt::NoModel => app.chat.notice = Some("Choose a model first.".into()),
            SendAttempt::Busy | SendAttempt::Empty => {}
        }
    }
}
