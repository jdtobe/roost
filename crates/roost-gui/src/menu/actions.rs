//! What a menu click does. The only place a menu changes the app.

use super::model::MenuAction;
use crate::app::{AppState, View};
use crate::runtime::BackendRequest;
use eframe::egui::{self, Key, Modifiers, ViewportCommand};
use roost_core::chat::ChatCommand;
use roost_core::session::SessionLifetime;

pub fn handle(app: &mut AppState, ctx: &egui::Context, action: MenuAction) -> Vec<BackendRequest> {
    match action {
        MenuAction::About => app.show_about = true,
        MenuAction::Settings => app.select(View::Settings),
        // A normal close request, so the quit confirmation still applies.
        MenuAction::Quit => ctx.send_viewport_cmd(ViewportCommand::Close),
        MenuAction::NewChat => return new_chat(app, SessionLifetime::Persisted),
        MenuAction::NewIncognito => return new_chat(app, SessionLifetime::Incognito),
        MenuAction::NewAgent => return new_agent(app),
        MenuAction::ShowView(v) => app.select(v),
        MenuAction::Cut => ctx.send_viewport_cmd(ViewportCommand::RequestCut),
        MenuAction::Copy => ctx.send_viewport_cmd(ViewportCommand::RequestCopy),
        MenuAction::Paste => ctx.send_viewport_cmd(ViewportCommand::RequestPaste),
        MenuAction::SelectAll => select_all(ctx),
        MenuAction::SetFamily(f) => {
            let change = app
                .settings_view
                .state
                .set_theme_family(&mut app.settings, f);
            return app.apply_change(change, ctx);
        }
        MenuAction::SetMode(m) => {
            let change = app.settings_view.state.set_theme(&mut app.settings, m);
            return app.apply_change(change, ctx);
        }
        MenuAction::Unload(name) => {
            app.running.retain(|m| m.name != name);
            return vec![BackendRequest::UnloadModel(name)];
        }
        MenuAction::UnloadAll => {
            let names: Vec<String> = app.running.drain(..).map(|m| m.name).collect();
            return names.into_iter().map(BackendRequest::UnloadModel).collect();
        }
        MenuAction::Minimize => ctx.send_viewport_cmd(ViewportCommand::Minimized(true)),
        MenuAction::Zoom => {
            let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
            ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized));
        }
        MenuAction::OpenUrl(url) => ctx.open_url(egui::OpenUrl::new_tab(url)),
    }
    Vec::new()
}

fn new_chat(app: &mut AppState, lifetime: SessionLifetime) -> Vec<BackendRequest> {
    let installed: Vec<String> = app
        .installed
        .rows()
        .iter()
        .map(|r| r.name().to_string())
        .collect();
    let Some(model) = app.chat.default_model(&installed) else {
        return Vec::new();
    };
    app.select(View::Chat);
    app.chat_ui.inspect = None;
    app.chat_ui.focus_composer = true;
    vec![BackendRequest::Chat(ChatCommand::Create {
        lifetime,
        model,
    })]
}

fn new_agent(app: &mut AppState) -> Vec<BackendRequest> {
    let installed: Vec<String> = app
        .installed
        .rows()
        .iter()
        .map(|r| r.name().to_string())
        .collect();
    let Some(model) = app.chat.default_model(&installed) else {
        return Vec::new();
    };
    app.select(View::Work);
    let mut requests = Vec::new();
    crate::chat::view::open_new_agent(app, Some(model), false, &mut requests);
    requests
}

/// egui has no "select all" command, so this is the key press it would see.
fn select_all(ctx: &egui::Context) {
    ctx.input_mut(|i| {
        i.events.push(egui::Event::Key {
            key: Key::A,
            physical_key: Some(Key::A),
            pressed: true,
            repeat: false,
            modifiers: Modifiers::COMMAND,
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use roost_core::config::{ThemeFamily, ThemeMode};
    use roost_core::ollama::{InstalledModel, RunningModel};

    /// Runs `f` inside one egui pass and returns what it asked the window to do.
    fn pass<R>(
        ctx: &egui::Context,
        f: impl FnOnce(&egui::Context) -> R,
    ) -> (R, Vec<ViewportCommand>) {
        let mut result = None;
        let mut f = Some(f);
        let out = ctx.run_ui(egui::RawInput::default(), |ui| {
            if let Some(f) = f.take() {
                result = Some(f(ui.ctx()));
            }
        });
        let cmds = out
            .viewport_output
            .values()
            .flat_map(|v| v.commands.clone())
            .collect();
        out.drop_without_applying_deltas();
        (result.expect("ran"), cmds)
    }

    fn with_model() -> AppState {
        let mut app = AppState::new();
        app.installed
            .on_installed_loaded(&Ok(vec![crate::runtime::InstalledEntry {
                model: InstalledModel {
                    name: "llama3:8b".into(),
                    size: 1,
                    params: None,
                    quant: None,
                    family: None,
                    modified: String::new(),
                    digest: String::new(),
                },
                source_repo: None,
            }]));
        app
    }

    fn running(name: &str) -> RunningModel {
        RunningModel {
            name: name.into(),
            size: 1,
            size_vram: 1,
            expires_at: None,
        }
    }

    #[test]
    fn views_and_settings_switch_the_view() {
        let ctx = egui::Context::default();
        let mut app = AppState::new();
        for v in [View::Chat, View::Work, View::Models] {
            pass(&ctx, |c| handle(&mut app, c, MenuAction::ShowView(v)));
            assert_eq!(app.view, v);
        }
        pass(&ctx, |c| handle(&mut app, c, MenuAction::Settings));
        assert_eq!(app.view, View::Settings);
    }

    #[test]
    fn about_opens_the_about_window() {
        let ctx = egui::Context::default();
        let mut app = AppState::new();
        pass(&ctx, |c| handle(&mut app, c, MenuAction::About));
        assert!(app.show_about);
    }

    #[test]
    fn quit_asks_the_window_to_close_so_the_confirmation_still_applies() {
        let ctx = egui::Context::default();
        let mut app = AppState::new();
        let (reqs, cmds) = pass(&ctx, |c| handle(&mut app, c, MenuAction::Quit));
        assert!(reqs.is_empty());
        assert!(cmds.contains(&ViewportCommand::Close), "{cmds:?}");
    }

    #[test]
    fn new_chat_creates_a_session_with_the_default_model_and_shows_chat() {
        let ctx = egui::Context::default();
        let mut app = with_model();
        app.view = View::Models;
        let (reqs, _) = pass(&ctx, |c| handle(&mut app, c, MenuAction::NewChat));
        assert!(matches!(
            reqs.as_slice(),
            [BackendRequest::Chat(ChatCommand::Create { lifetime: SessionLifetime::Persisted, model })]
                if model == "llama3:8b"
        ));
        assert_eq!(app.view, View::Chat);
        assert!(app.chat_ui.focus_composer);
        let (reqs, _) = pass(&ctx, |c| handle(&mut app, c, MenuAction::NewIncognito));
        assert!(matches!(
            reqs.as_slice(),
            [BackendRequest::Chat(ChatCommand::Create {
                lifetime: SessionLifetime::Incognito,
                ..
            })]
        ));
    }

    #[test]
    fn new_agent_opens_a_work_session_with_no_model_and_no_folder() {
        let ctx = egui::Context::default();
        let mut app = with_model();
        app.view = View::Chat;
        let (reqs, _) = pass(&ctx, |c| handle(&mut app, c, MenuAction::NewAgent));
        assert_eq!(app.view, View::Work);
        assert!(app.chat_ui.focus_composer);
        assert!(matches!(
            reqs.as_slice(),
            [BackendRequest::Chat(ChatCommand::CreateAgent {
                lifetime: SessionLifetime::Persisted,
                workspace: None,
                model,
            })] if model.is_empty()
        ));
        // Even with no model installed there is a session to open.
        let mut none = AppState::new();
        let (reqs, _) = pass(&ctx, |c| handle(&mut none, c, MenuAction::NewAgent));
        assert_eq!(reqs.len(), 1);
    }

    #[test]
    fn new_chat_without_a_model_does_nothing() {
        let ctx = egui::Context::default();
        let mut app = AppState::new();
        app.view = View::Models;
        let (reqs, _) = pass(&ctx, |c| handle(&mut app, c, MenuAction::NewChat));
        assert!(reqs.is_empty());
        assert_eq!(app.view, View::Models);
    }

    #[test]
    fn theme_items_change_and_save_the_settings() {
        let ctx = egui::Context::default();
        let mut app = AppState::new();
        let (reqs, _) = pass(&ctx, |c| {
            handle(&mut app, c, MenuAction::SetFamily(ThemeFamily::Tide))
        });
        assert_eq!(app.settings.theme_family, ThemeFamily::Tide);
        assert!(
            matches!(reqs.as_slice(), [BackendRequest::ApplySettings(s)] if s.theme_family == ThemeFamily::Tide)
        );
        let (reqs, _) = pass(&ctx, |c| {
            handle(&mut app, c, MenuAction::SetMode(ThemeMode::Dark))
        });
        assert_eq!(app.settings.theme_mode, ThemeMode::Dark);
        assert_eq!(reqs.len(), 1);
        // Choosing what is already chosen saves nothing.
        let (reqs, _) = pass(&ctx, |c| {
            handle(&mut app, c, MenuAction::SetMode(ThemeMode::Dark))
        });
        assert!(reqs.is_empty());
    }

    #[test]
    fn unloading_one_or_all_models_asks_the_backend() {
        let ctx = egui::Context::default();
        let mut app = AppState::new();
        app.running = vec![running("a:1"), running("b:2")];
        let (reqs, _) = pass(&ctx, |c| {
            handle(&mut app, c, MenuAction::Unload("a:1".into()))
        });
        assert!(matches!(reqs.as_slice(), [BackendRequest::UnloadModel(n)] if n == "a:1"));
        assert_eq!(app.running.len(), 1);
        let (reqs, _) = pass(&ctx, |c| handle(&mut app, c, MenuAction::UnloadAll));
        assert!(matches!(reqs.as_slice(), [BackendRequest::UnloadModel(n)] if n == "b:2"));
        assert!(app.running.is_empty());
    }

    #[test]
    fn edit_items_use_the_egui_clipboard_commands() {
        let ctx = egui::Context::default();
        let mut app = AppState::new();
        for (action, expected) in [
            (MenuAction::Cut, ViewportCommand::RequestCut),
            (MenuAction::Copy, ViewportCommand::RequestCopy),
            (MenuAction::Paste, ViewportCommand::RequestPaste),
        ] {
            let (_, cmds) = pass(&ctx, |c| handle(&mut app, c, action.clone()));
            assert!(cmds.contains(&expected), "{action:?}: {cmds:?}");
        }
    }

    #[test]
    fn select_all_synthesizes_the_select_all_key() {
        let ctx = egui::Context::default();
        let mut app = AppState::new();
        let (events, _) = pass(&ctx, |c| {
            handle(&mut app, c, MenuAction::SelectAll);
            c.input(|i| i.events.clone())
        });
        assert!(events.iter().any(|e| matches!(
            e,
            egui::Event::Key { key: Key::A, pressed: true, modifiers, .. } if modifiers.command
        )));
    }

    #[test]
    fn window_items_drive_the_viewport() {
        let ctx = egui::Context::default();
        let mut app = AppState::new();
        let (_, cmds) = pass(&ctx, |c| handle(&mut app, c, MenuAction::Minimize));
        assert!(cmds.contains(&ViewportCommand::Minimized(true)));
        let (_, cmds) = pass(&ctx, |c| handle(&mut app, c, MenuAction::Zoom));
        assert!(cmds.contains(&ViewportCommand::Maximized(true)));
    }

    #[test]
    fn help_links_open_in_the_browser() {
        let ctx = egui::Context::default();
        let mut app = AppState::new();
        let out = ctx.run_ui(egui::RawInput::default(), |ui| {
            handle(
                &mut app,
                ui.ctx(),
                MenuAction::OpenUrl(super::super::model::REPO_URL),
            );
        });
        let opened: Vec<_> = out
            .platform_output
            .commands
            .iter()
            .filter_map(|c| match c {
                egui::OutputCommand::OpenUrl(o) => Some(o.url.clone()),
                _ => None,
            })
            .collect();
        out.drop_without_applying_deltas();
        assert_eq!(opened, [super::super::model::REPO_URL]);
    }
}
