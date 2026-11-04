//! A window listing every tool the agent can use, grouped by family: what it
//! does, whether it is on for this chat, its switch, its limits and what the
//! model is told about it. It reads the open session's tool settings and
//! changes them with the same commands as the tools popover.

use super::chrome::tool_option_rows;
use crate::app::AppState;
use crate::runtime::BackendRequest;
use crate::theme::{mono, semibold, Tokens};
use crate::widgets::tag;
use eframe::egui::{self, RichText};
use roost_core::agent::registry::{self, Capability, Family, ToolSpec};
use roost_core::agent::tool;
use roost_core::chat::ChatCommand;
use roost_core::session::{Permission, Session};

/// The widest the window is drawn.
const WIDTH: f32 = 480.0;
/// Kept free around the window.
const MARGIN: f32 = 16.0;
/// The title bar and frame of the window, which the scrolled list leaves room for.
const CHROME_H: f32 = 56.0;

/// The first sentence of a tool's description, for the row.
fn first_sentence(description: &str) -> String {
    let line = description.lines().next().unwrap_or_default();
    match line.find(". ") {
        Some(i) => line[..=i].to_string(),
        None => line.to_string(),
    }
}

/// What the tool can do in this chat: `off` (its switch, its permission, or
/// Settings), `ask` or `allowed`, and why. A tool Settings turned off reads
/// `off (Settings)`: its switch does nothing.
fn status(app: &AppState, s: &Session, spec: &ToolSpec) -> (&'static str, &'static str) {
    let settings = &app.settings;
    if spec.capability == Capability::Write && !settings.agents_can_write {
        return ("off (Settings)", "Writing files is off in Settings");
    }
    if spec.family == Family::Shell && !settings.agents_can_run {
        return ("off (Settings)", "Running commands is off in Settings");
    }
    if !registry::tool_enabled(&s.effective_tools, spec) {
        return ("off", "Switched off for this chat");
    }
    match registry::permission_of(&s.effective_tools, spec) {
        Permission::Off => ("off", "Its kind of action is off for this chat"),
        Permission::Ask => ("ask", "Asks before each use"),
        Permission::Auto => ("allowed", "Used without asking"),
    }
}

/// Draws the window when it is open. Closing it with its X shuts it.
pub(super) fn window(
    ctx: &egui::Context,
    app: &mut AppState,
    out: &mut Vec<BackendRequest>,
    t: &Tokens,
    s: &Session,
) {
    #[cfg(test)]
    {
        let p = &mut app.chat_ui.probe;
        p.tool_list_window = None;
        p.tool_list_heads.clear();
        p.tool_list_rows.clear();
        p.tool_list_toggles.clear();
        p.tool_list_states.clear();
        p.tool_list_options.clear();
        p.tool_list_resets.clear();
    }
    if app.chat_ui.tool_list_open && app.chat_ui.tool_list_for.is_some_and(|id| id != s.id) {
        app.chat_ui.tool_list_open = false;
    }
    if !app.chat_ui.tool_list_open {
        app.chat_ui.tool_list_for = None;
        return;
    }
    app.chat_ui.tool_list_for = Some(s.id);
    let screen = ctx.content_rect();
    let width = WIDTH.min(screen.width() - 2.0 * MARGIN).max(200.0);
    let list_h = (screen.height() - 2.0 * MARGIN - CHROME_H).max(120.0);
    let mut open = true;
    let shown = egui::Window::new("All tools")
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .constrain_to(screen)
        .default_pos(egui::pos2(
            screen.center().x - width / 2.0,
            screen.top() + MARGIN,
        ))
        .show(ctx, |ui| {
            ui.set_width(width);
            egui::ScrollArea::vertical()
                .max_height(list_h)
                .auto_shrink([false, true])
                .show(ui, |ui| list(ui, app, out, t, s));
        });
    #[cfg(test)]
    if let Some(r) = shown {
        app.chat_ui.probe.tool_list_window = Some(r.response.rect);
    }
    #[cfg(not(test))]
    let _ = shown;
    app.chat_ui.tool_list_open = open;
}

fn list(
    ui: &mut egui::Ui,
    app: &mut AppState,
    out: &mut Vec<BackendRequest>,
    t: &Tokens,
    s: &Session,
) {
    let running = app.chat.is_running(s.id);
    ui.spacing_mut().item_spacing.y = 8.0;
    ui.add(
        egui::Label::new(
            RichText::new(
                "What the agent can use in this chat. Settings sets the ceiling; changes here apply to this chat only.",
            )
            .size(11.5)
            .color(t.fg3),
        )
        .wrap(),
    );
    let mut families: Vec<Family> = Vec::new();
    for tool in tool::all() {
        let f = tool.spec().family;
        if !families.contains(&f) {
            families.push(f);
        }
    }
    for family in families {
        ui.add_space(4.0);
        let head = ui.label(
            RichText::new(family.label().to_uppercase())
                .font(semibold(11.0))
                .color(t.fg3),
        );
        #[cfg(test)]
        app.chat_ui
            .probe
            .tool_list_heads
            .push((family.label().to_string(), head.rect));
        #[cfg(not(test))]
        let _ = head;
        for tool in tool::all().iter().filter(|t| t.spec().family == family) {
            row(ui, app, out, t, s, tool.spec(), running);
        }
    }
    if running {
        ui.label(
            RichText::new("Locked while a run is active.")
                .size(11.5)
                .color(t.fg3),
        );
    }
}

fn row(
    ui: &mut egui::Ui,
    app: &mut AppState,
    out: &mut Vec<BackendRequest>,
    t: &Tokens,
    s: &Session,
    spec: ToolSpec,
    running: bool,
) {
    let (state, why) = status(app, s, &spec);
    let mut on = registry::tool_enabled(&s.effective_tools, &spec);
    #[cfg(test)]
    let (values, resets) = (
        app.chat_ui.probe.tool_option_values.len(),
        app.chat_ui.probe.tool_option_resets.len(),
    );
    let frame = egui::Frame::new()
        .fill(t.surface2)
        .stroke(egui::Stroke::new(1.0, t.line))
        .corner_radius(9.0)
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 6.0;
            ui.horizontal(|ui| {
                ui.label(RichText::new(spec.name).font(mono(12.5)).color(t.fg));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let forced_off = state == "off (Settings)";
                    let toggle =
                        ui.add_enabled(!running && !forced_off, egui::Checkbox::new(&mut on, ""));
                    #[cfg(test)]
                    app.chat_ui.probe.tool_list_states.push((
                        spec.name.to_string(),
                        state.to_string(),
                        toggle.enabled(),
                    ));
                    #[cfg(test)]
                    app.chat_ui
                        .probe
                        .tool_list_toggles
                        .push((spec.name.to_string(), toggle.rect));
                    if toggle.changed() {
                        out.push(BackendRequest::Chat(ChatCommand::SetToolEnabled(
                            s.id,
                            spec.name.into(),
                            on,
                        )));
                    }
                    let (fg, bg) = match state {
                        "allowed" => (t.ok, t.ok_soft),
                        "ask" => (t.warn, t.warn_soft),
                        _ => (t.fg3, t.surface),
                    };
                    ui.scope(|ui| tag(ui, state, fg, bg))
                        .response
                        .on_hover_text(why);
                });
            });
            ui.add(
                egui::Label::new(
                    RichText::new(first_sentence(spec.description))
                        .size(12.0)
                        .color(t.fg2),
                )
                .wrap(),
            );
            tool_option_rows(
                ui,
                app,
                out,
                t,
                s.id,
                &s.effective_tools,
                spec.name,
                running,
            );
            egui::CollapsingHeader::new(RichText::new("What the model is told").size(11.5))
                .id_salt(("tool_list", spec.name))
                .show(ui, |ui| {
                    ui.add(
                        egui::Label::new(RichText::new(spec.description).size(12.0).color(t.fg2))
                            .wrap(),
                    );
                    ui.label(RichText::new("Parameters").size(11.5).color(t.fg3));
                    let schema = serde_json::to_string_pretty(&spec.parameters)
                        .unwrap_or_else(|_| spec.parameters.to_string());
                    ui.add(
                        egui::Label::new(RichText::new(schema).font(mono(11.0)).color(t.fg2))
                            .wrap(),
                    );
                });
        });
    #[cfg(test)]
    {
        let p = &mut app.chat_ui.probe;
        p.tool_list_rows
            .push((spec.name.to_string(), frame.response.rect));
        // The option rows are the popover's own; theirs are taken here.
        p.tool_list_options
            .extend(p.tool_option_values.split_off(values));
        p.tool_list_resets
            .extend(p.tool_option_resets.split_off(resets));
    }
    #[cfg(not(test))]
    let _ = frame;
}
