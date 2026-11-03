//! Headless layout tests for the Chat view: real painted rectangles at the
//! window sizes the layout is designed for, in every theme family.

use crate::app::{App, View};
use crate::browse::test_util::Harness;
use crate::runtime::BackendRequest;
use eframe::egui::{self, vec2, Pos2, Rect};
use roost_core::chat::{ChatCommand, ChatEvent};
use roost_core::config::{Settings, ThemeFamily};
use roost_core::fit::Machine;
use roost_core::session::{AssistantStatus, RunId, Session, SessionLifetime, TurnId};
use roost_core::store::{IndexRow, RowStatus};
use std::sync::mpsc::Receiver;

const SIZES: [(f32, f32); 2] = [(1180.0, 820.0), (900.0, 640.0)];
const LONG_TITLE: &str =
    "A very long conversation title that keeps going well past what any rail could hold on one line";
const LONG_MODEL: &str =
    "registry.example.com/community/an-extremely-long-model-name-for-testing:70b";

fn row_for(s: &Session, title: &str) -> IndexRow {
    IndexRow {
        id: s.id,
        title: title.into(),
        mode: s.mode,
        lifetime: s.lifetime,
        pinned: false,
        model: s.model.clone(),
        parent: None,
        created_at_ms: s.created_at_ms,
        updated_at_ms: s.updated_at_ms,
        active: false,
        status: RowStatus::Ok,
        file_mtime_ms: 0,
        file_len: 0,
        preview: Some(String::new()),
        last: Some(String::new()),
        folder: None,
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

pub(super) struct Fixture {
    pub(super) app: App,
    sent: Receiver<BackendRequest>,
    open: Session,
    bad: IndexRow,
}

fn fixture() -> Fixture {
    fixture_with(12)
}

/// `chats` ordinary saved chats besides the open one, the incognito one and
/// the damaged one; a short list keeps every group inside the viewport.
pub(super) fn fixture_with(chats: usize) -> Fixture {
    let (tx, sent) = std::sync::mpsc::channel();
    let (_, rx) = std::sync::mpsc::channel();
    let mut app = App::new(
        &Settings::default(),
        std::path::PathBuf::from("/tmp/staging"),
        Machine {
            ram_gb: 64.0,
            vram_gb: Some(24.0),
        },
        tx,
        rx,
    );
    let now = now_ms();
    let mut open = Session::new_chat(LONG_MODEL, SessionLifetime::Persisted, now);
    open.push_user(&"Please explain every detail of this. ".repeat(12), now);
    let a = open.begin_assistant(RunId::new(), TurnId::new(), now);
    open.append_assistant(
        a,
        &"A long and careful answer. ".repeat(30),
        "Let me think about it.",
    );
    open.finish_assistant(a, AssistantStatus::Complete, None, now);
    open.push_user("And again?", now);
    let f = open.begin_assistant(RunId::new(), TurnId::new(), now);
    open.append_assistant(f, "partial", "");
    open.finish_assistant(
        f,
        AssistantStatus::Failed,
        Some("Ollama stopped responding.".into()),
        now,
    );

    let mut rows = vec![row_for(&open, LONG_TITLE)];
    for i in 0..chats as u64 {
        let s = Session::new_chat(
            "llama3.2:3b",
            SessionLifetime::Persisted,
            now - i * 3_600_000,
        );
        let mut r = row_for(&s, &format!("Chat number {i}"));
        r.pinned = i == 1;
        rows.push(r);
    }
    let inc = Session::new_chat("qwen2.5:7b", SessionLifetime::Incognito, now);
    rows.push(row_for(&inc, "Quiet thoughts"));
    let damaged = Session::new_chat("m", SessionLifetime::Persisted, now - 5 * 86_400_000);
    let mut bad = row_for(&damaged, "Unreadable session");
    bad.status = RowStatus::Quarantined;
    rows.push(bad.clone());

    let st = app.state_mut();
    st.ollama = Some(Some("0.9".into()));
    st.view = View::Chat;
    st.chat.fold(ChatEvent::Rows(rows));
    st.chat.fold(ChatEvent::Opened(Box::new(open.clone())));
    Fixture {
        app,
        sent,
        open,
        bad,
    }
}

pub(super) fn run(app: &mut App, h: &mut Harness, (w, hh): (f32, f32), frames: usize) {
    for _ in 0..frames {
        h.frame(vec2(w, hh), vec![], |ui| app.show(ui));
    }
}

fn click(app: &mut App, h: &mut Harness, size: (f32, f32), at: Pos2) {
    h.frame(
        vec2(size.0, size.1),
        vec![egui::Event::PointerMoved(at)],
        |ui| app.show(ui),
    );
    for pressed in [true, false] {
        h.frame(
            vec2(size.0, size.1),
            vec![egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            }],
            |ui| app.show(ui),
        );
    }
    run(app, h, size, 2);
}

fn inside(inner: Rect, outer: Rect) -> bool {
    inner.top() >= outer.top() - 0.5
        && inner.bottom() <= outer.bottom() + 0.5
        && inner.left() >= outer.left() - 0.5
        && inner.right() <= outer.right() + 0.5
}

fn sent_chat_commands(rx: &Receiver<BackendRequest>) -> Vec<ChatCommand> {
    rx.try_iter()
        .filter_map(|r| match r {
            BackendRequest::Chat(c) => Some(c),
            _ => None,
        })
        .collect()
}

#[test]
fn every_theme_family_and_mode_keeps_chat_usable_at_minimum_size() {
    for family in [
        ThemeFamily::Meadow,
        ThemeFamily::Ember,
        ThemeFamily::Plum,
        ThemeFamily::Tide,
    ] {
        for dark in [false, true] {
            for size in SIZES {
                let mut f = fixture();
                let mut h = Harness::themed(family, dark);
                run(&mut f.app, &mut h, size, 4);
                let window = Rect::from_min_size(Pos2::ZERO, vec2(size.0, size.1));
                let p = f.app.state_mut().chat_ui.probe.clone();
                let tag = format!("{family:?} dark={dark} {size:?}");
                let (rail, pane, composer, header) = (
                    p.rail.expect("rail"),
                    p.pane.expect("pane"),
                    p.composer.expect("composer"),
                    p.header.expect("header"),
                );
                assert!(inside(rail, window), "{tag}: rail {rail:?}");
                assert!(inside(pane, window), "{tag}: pane {pane:?}");
                assert!(inside(composer, window), "{tag}: composer {composer:?}");
                assert!(
                    rail.right() <= pane.left() + 0.5,
                    "{tag}: rail overlaps pane"
                );
                // The header band reaches over the pane's margins on purpose.
                let reach =
                    pane.expand2(vec2(super::view::PANE_MARGIN_X, super::view::PANE_MARGIN_Y));
                assert!(
                    inside(header, reach),
                    "{tag}: header {header:?} pane {pane:?}"
                );
                let (send, label) = p.send_button.expect("send button");
                assert_eq!(label, "Send", "{tag}");
                assert!(
                    inside(send, composer),
                    "{tag}: send {send:?} composer {composer:?}"
                );
                let trash = p.delete_button.clone().expect("trash can").0;
                assert!(inside(trash, header), "{tag}: {trash:?} not in {header:?}");
                let chip = p.model_chip.expect("model chip");
                assert!(
                    inside(chip, composer),
                    "{tag}: {chip:?} not in {composer:?}"
                );
                assert!(!p.rows.is_empty(), "{tag}: no rail rows");
                let mut last_bottom = f32::MIN;
                for (_, r) in &p.rows {
                    assert!(r.top() >= last_bottom - 0.5, "{tag}: rows overlap");
                    assert!(
                        r.left() >= rail.left() - 0.5 && r.right() <= rail.right() + 0.5,
                        "{tag}: row {r:?} outside rail {rail:?}"
                    );
                    last_bottom = r.bottom();
                }
            }
        }
    }
}

#[test]
fn chat_is_the_first_tab() {
    assert_eq!(View::ALL[0], View::Chat);
}

#[test]
fn unavailable_history_moves_the_app_to_browse() {
    let mut f = fixture();
    let st = f.app.state_mut();
    st.on_event(&crate::runtime::UiEvent::ChatUnavailable(
        "Another Roost".into(),
    ));
    assert_eq!(st.view, View::Models);
}

#[test]
fn the_nav_shows_activity_while_a_background_run_is_active() {
    let mut f = fixture();
    let mut h = Harness::new(false);
    run(&mut f.app, &mut h, (1180.0, 820.0), 3);
    let width = |app: &mut App| {
        app.state_mut()
            .top_bar
            .iter()
            .find(|(n, _)| *n == "nav")
            .unwrap()
            .1
            .width()
    };
    let idle = width(&mut f.app);
    let id = f.open.id;
    let turn = TurnId::new();
    f.app
        .state_mut()
        .chat
        .fold(ChatEvent::TurnStarted { session: id, turn });
    run(&mut f.app, &mut h, (1180.0, 820.0), 3);
    assert!(width(&mut f.app) > idle, "badge should widen the nav");
}

#[test]
fn send_becomes_stop_during_a_run() {
    let mut f = fixture();
    let mut h = Harness::new(false);
    run(&mut f.app, &mut h, (1180.0, 820.0), 3);
    assert_eq!(
        f.app.state_mut().chat_ui.probe.send_button.unwrap().1,
        "Send"
    );
    f.app.state_mut().chat.fold(ChatEvent::TurnStarted {
        session: f.open.id,
        turn: TurnId::new(),
    });
    run(&mut f.app, &mut h, (1180.0, 820.0), 3);
    assert_eq!(
        f.app.state_mut().chat_ui.probe.send_button.unwrap().1,
        "Stop"
    );
}

fn press_enter() -> Vec<egui::Event> {
    press_enter_with(egui::Modifiers::NONE)
}

fn press_enter_with(modifiers: egui::Modifiers) -> Vec<egui::Event> {
    vec![egui::Event::Key {
        key: egui::Key::Enter,
        physical_key: Some(egui::Key::Enter),
        pressed: true,
        repeat: false,
        modifiers,
    }]
}

#[test]
fn shift_enter_adds_a_line_instead_of_sending() {
    let size = (1180.0, 820.0);
    let mut f = fixture();
    let mut h = Harness::new(false);
    let id = f.open.id;
    *f.app.state_mut().chat.draft_mut(id) = "hello".into();
    f.app.state_mut().chat_ui.focus_composer = true;
    run(&mut f.app, &mut h, size, 3);
    h.frame(
        vec2(size.0, size.1),
        press_enter_with(egui::Modifiers::SHIFT),
        |ui| f.app.show(ui),
    );
    run(&mut f.app, &mut h, size, 2);
    assert!(sent_chat_commands(&f.sent).is_empty());
    assert_eq!(f.app.state_mut().chat.draft(id), "hello\n");
}

#[test]
fn enter_sends_when_idle_and_does_nothing_during_a_run() {
    let size = (1180.0, 820.0);
    let mut f = fixture();
    let mut h = Harness::new(false);
    let id = f.open.id;
    *f.app.state_mut().chat.draft_mut(id) = "hello there".into();
    f.app.state_mut().chat_ui.focus_composer = true;
    run(&mut f.app, &mut h, size, 3);
    h.frame(vec2(size.0, size.1), press_enter(), |ui| f.app.show(ui));
    run(&mut f.app, &mut h, size, 2);
    let cmds = sent_chat_commands(&f.sent);
    assert!(
        matches!(cmds.as_slice(), [ChatCommand::Send { text, .. }] if text == "hello there"),
        "{cmds:?}"
    );
    assert_eq!(
        f.app.state_mut().chat.draft(id),
        "hello there",
        "kept until confirmed"
    );

    // A run is in progress: Enter must not send, and must keep the draft.
    let mut f = fixture();
    let id = f.open.id;
    *f.app.state_mut().chat.draft_mut(id) = "next".into();
    f.app.state_mut().chat.fold(ChatEvent::TurnStarted {
        session: id,
        turn: TurnId::new(),
    });
    f.app.state_mut().chat_ui.focus_composer = true;
    run(&mut f.app, &mut h, size, 3);
    h.frame(vec2(size.0, size.1), press_enter(), |ui| f.app.show(ui));
    run(&mut f.app, &mut h, size, 2);
    assert!(sent_chat_commands(&f.sent).is_empty());
    assert_eq!(f.app.state_mut().chat.draft(id), "next");
}

#[test]
fn incognito_group_shows_its_warning_and_clear() {
    let size = (1180.0, 820.0);
    let mut f = fixture_with(2);
    let mut h = Harness::new(false);
    run(&mut f.app, &mut h, size, 2);
    let texts = h.frame_texts(vec2(size.0, size.1), vec![], |ui| f.app.show(ui));
    let all = texts.join("\n");
    assert!(all.contains("Memory only"), "{all}");
    assert!(all.contains("Clear"), "{all}");
    assert!(all.contains("Quiet thoughts"), "{all}");
}

#[test]
fn errored_rows_open_an_explanation_with_delete_only() {
    let size = (1180.0, 820.0);
    let mut f = fixture_with(2);
    let mut h = Harness::new(false);
    run(&mut f.app, &mut h, size, 3);
    let target = f
        .app
        .state_mut()
        .chat_ui
        .probe
        .rows
        .iter()
        .find(|(id, _)| *id == f.bad.id)
        .map(|(_, r)| *r)
        .expect("bad row painted");
    click(&mut f.app, &mut h, size, target.center());
    assert!(
        !sent_chat_commands(&f.sent)
            .iter()
            .any(|c| matches!(c, ChatCommand::Open(_))),
        "an errored row is not opened"
    );
    let texts = h.frame_texts(vec2(size.0, size.1), vec![], |ui| f.app.show(ui));
    let all = texts.join("\n");
    assert!(all.contains("damaged"), "{all}");
    assert!(all.contains("Delete"), "{all}");
    assert!(!all.contains("Rename") && !all.contains("Send"), "{all}");
}

#[test]
fn switching_sessions_closes_the_previous_saved_one_so_it_can_be_evicted() {
    let size = (1180.0, 820.0);
    let mut f = fixture_with(2);
    let mut h = Harness::new(false);
    run(&mut f.app, &mut h, size, 3);
    let previous = f.open.id;
    let target = f
        .app
        .state_mut()
        .chat_ui
        .probe
        .rows
        .iter()
        .find(|(id, _)| *id != previous && *id != f.bad.id)
        .map(|(id, r)| (*id, *r))
        .expect("another row");
    click(&mut f.app, &mut h, size, target.1.center());
    let cmds = sent_chat_commands(&f.sent);
    assert!(
        matches!(cmds.as_slice(), [ChatCommand::Close(a), ChatCommand::Open(b)] if *a == previous && *b == target.0),
        "{cmds:?}"
    );
}

fn rail_row_of(app: &mut App, id: roost_core::session::SessionId) -> Rect {
    app.state_mut()
        .chat_ui
        .probe
        .rows
        .iter()
        .find(|(r, _)| *r == id)
        .map(|(_, r)| *r)
        .expect("row painted")
}

fn rail_delete_of(app: &mut App, id: roost_core::session::SessionId) -> Option<(Rect, bool)> {
    app.state_mut()
        .chat_ui
        .probe
        .rail_delete
        .iter()
        .find(|(r, _, _)| *r == id)
        .map(|(_, rect, armed)| (*rect, *armed))
}

#[test]
fn hovering_a_rail_row_shows_a_delete_control_inside_the_row() {
    let size = (1180.0, 820.0);
    let mut f = fixture_with(3);
    let mut h = Harness::new(false);
    run(&mut f.app, &mut h, size, 3);
    let target = f
        .app
        .state_mut()
        .chat_ui
        .probe
        .rows
        .iter()
        .map(|(id, _)| *id)
        .find(|id| *id != f.open.id && *id != f.bad.id)
        .unwrap();
    assert!(
        rail_delete_of(&mut f.app, target).is_none(),
        "hidden until hovered"
    );
    let row = rail_row_of(&mut f.app, target);
    h.frame(
        vec2(size.0, size.1),
        vec![egui::Event::PointerMoved(row.center())],
        |ui| f.app.show(ui),
    );
    run(&mut f.app, &mut h, size, 2);
    let (del, armed) = rail_delete_of(&mut f.app, target).expect("delete control on hover");
    assert!(!armed);
    assert!(inside(del, rail_row_of(&mut f.app, target)), "{del:?}");
    // The selected row always shows it, so it is one click away.
    assert!(rail_delete_of(&mut f.app, f.open.id).is_some());
}

/// Hovers the first ordinary row and returns its id and trash can.
fn hover_a_row_delete(
    f: &mut Fixture,
    h: &mut Harness,
    size: (f32, f32),
) -> (roost_core::session::SessionId, Rect) {
    let target = f
        .app
        .state_mut()
        .chat_ui
        .probe
        .rows
        .iter()
        .map(|(id, _)| *id)
        .find(|id| *id != f.open.id && *id != f.bad.id)
        .unwrap();
    let row = rail_row_of(&mut f.app, target);
    h.frame(
        vec2(size.0, size.1),
        vec![egui::Event::PointerMoved(row.center())],
        |ui| f.app.show(ui),
    );
    run(&mut f.app, h, size, 2);
    let (del, _) = rail_delete_of(&mut f.app, target).unwrap();
    (target, del)
}

/// Arms the row's delete with a click, then lets the pause pass without
/// sleeping and confirms with a second click.
fn confirm_row_delete(
    f: &mut Fixture,
    h: &mut Harness,
    size: (f32, f32),
    target: roost_core::session::SessionId,
    del: Rect,
) {
    click(&mut f.app, h, size, del.center());
    assert!(
        rail_delete_of(&mut f.app, target).unwrap().1,
        "the first click arms"
    );
    f.app.state_mut().chat_ui.confirm.arm(
        &target.to_string(),
        std::time::Instant::now() - std::time::Duration::from_secs(1),
    );
    run(&mut f.app, h, size, 2);
    let (del, _) = rail_delete_of(&mut f.app, target).unwrap();
    click(&mut f.app, h, size, del.center());
}

#[test]
fn the_row_trash_can_takes_a_click_and_a_confirm_after_a_short_pause() {
    let size = (1180.0, 820.0);
    let mut f = fixture_with(3);
    let mut h = Harness::new(false);
    run(&mut f.app, &mut h, size, 3);
    let (target, del) = hover_a_row_delete(&mut f, &mut h, size);
    let _ = sent_chat_commands(&f.sent);
    click(&mut f.app, &mut h, size, del.center());
    let (del, armed) = rail_delete_of(&mut f.app, target).expect("still shown while armed");
    assert!(armed, "the first click arms");
    assert!(
        !sent_chat_commands(&f.sent)
            .iter()
            .any(|c| matches!(c, ChatCommand::Delete(_) | ChatCommand::Open(_))),
        "nothing is sent yet"
    );
    // An immediate second click is inside the pause and must not delete.
    click(&mut f.app, &mut h, size, del.center());
    assert!(!sent_chat_commands(&f.sent)
        .iter()
        .any(|c| matches!(c, ChatCommand::Delete(_))));
    std::thread::sleep(std::time::Duration::from_millis(150));
    let (del, _) = rail_delete_of(&mut f.app, target).unwrap();
    click(&mut f.app, &mut h, size, del.center());
    let cmds = sent_chat_commands(&f.sent);
    assert!(
        matches!(cmds.as_slice(), [ChatCommand::Delete(d)] if *d == target),
        "{cmds:?}"
    );
}

#[test]
fn the_row_trash_can_deletes_after_the_confirm_and_the_toast_undoes_it() {
    let size = (1180.0, 820.0);
    let mut f = fixture_with(3);
    let mut h = Harness::new(false);
    run(&mut f.app, &mut h, size, 3);
    let (target, del) = hover_a_row_delete(&mut f, &mut h, size);
    let _ = sent_chat_commands(&f.sent);
    confirm_row_delete(&mut f, &mut h, size, target, del);
    let cmds = sent_chat_commands(&f.sent);
    assert!(
        matches!(cmds.as_slice(), [ChatCommand::Delete(d)] if *d == target),
        "the confirm deletes: {cmds:?}"
    );
    run(&mut f.app, &mut h, size, 2);
    let texts = texts_of(&mut f.app, &mut h, size);
    assert!(
        texts.contains("Deleted") && texts.contains("Undo"),
        "{texts}"
    );
    let undo = f.app.state_mut().chat_ui.probe.undo_button.expect("Undo");
    click(&mut f.app, &mut h, size, undo.center());
    let cmds = sent_chat_commands(&f.sent);
    assert!(
        matches!(cmds.as_slice(), [ChatCommand::Undelete(d)] if *d == target),
        "{cmds:?}"
    );
    run(&mut f.app, &mut h, size, 2);
    assert!(
        f.app.state_mut().chat_ui.undo.is_none(),
        "the toast is gone"
    );
}

#[test]
fn an_expired_toast_purges_the_chat_and_a_newer_delete_purges_the_older_one() {
    let size = (1180.0, 820.0);
    let mut f = fixture_with(4);
    let mut h = Harness::new(false);
    run(&mut f.app, &mut h, size, 3);
    let (first, del) = hover_a_row_delete(&mut f, &mut h, size);
    confirm_row_delete(&mut f, &mut h, size, first, del);
    let _ = sent_chat_commands(&f.sent);
    // Time runs out.
    {
        let toast = f.app.state_mut().chat_ui.undo.as_mut().expect("a toast");
        assert_eq!(toast.id, first);
        toast.at = std::time::Instant::now() - std::time::Duration::from_secs(11);
    }
    run(&mut f.app, &mut h, size, 2);
    let cmds = sent_chat_commands(&f.sent);
    assert!(
        matches!(cmds.as_slice(), [ChatCommand::PurgeDeleted(d)] if *d == first),
        "{cmds:?}"
    );
    assert!(f.app.state_mut().chat_ui.undo.is_none());
    // Deleting another chat while a toast shows removes the first for good.
    f.app.state_mut().chat_ui.undo = Some(super::view::UndoToast {
        id: first,
        title: "old".into(),
        at: std::time::Instant::now(),
    });
    let (second, del) = hover_a_row_delete(&mut f, &mut h, size);
    let _ = sent_chat_commands(&f.sent);
    confirm_row_delete(&mut f, &mut h, size, second, del);
    let cmds = sent_chat_commands(&f.sent);
    assert!(
        matches!(cmds.as_slice(),
            [ChatCommand::PurgeDeleted(a), ChatCommand::Delete(b)] if *a == first && *b == second),
        "{cmds:?}"
    );
}

fn loaded(name: &str, expires: &str) -> roost_core::ollama::RunningModel {
    roost_core::ollama::RunningModel {
        name: name.into(),
        size: 4_700_000_000,
        size_vram: 4_700_000_000,
        expires_at: Some(expires.into()),
    }
}

fn unload_requests(rx: &Receiver<BackendRequest>) -> Vec<String> {
    rx.try_iter()
        .filter_map(|r| match r {
            BackendRequest::UnloadModel(n) => Some(n),
            _ => None,
        })
        .collect()
}

#[test]
fn unload_notes_read_naturally() {
    use super::view::unload_note;
    let now = chrono::DateTime::parse_from_rfc3339("2026-10-27T12:00:00Z")
        .unwrap()
        .to_utc();
    assert_eq!(
        unload_note("2026-10-27T12:05:00Z", now).as_deref(),
        Some("unloads in 5 min")
    );
    assert_eq!(
        unload_note("2026-10-27T12:00:20Z", now).as_deref(),
        Some("unloads in under a minute")
    );
    assert_eq!(
        unload_note("2026-10-27T11:59:00Z", now).as_deref(),
        Some("unloading")
    );
    assert_eq!(
        unload_note("2318-01-01T00:00:00Z", now).as_deref(),
        Some("stays loaded")
    );
    assert_eq!(unload_note("not a time", now), None);
    assert_eq!(
        unload_note("2026-10-27T05:10:00.123456-07:00", now).as_deref(),
        Some("unloads in 11 min")
    );
}

// ------------------------------------------------------------- the Work tab

use roost_core::session::{SessionMode, ToolStatus};

fn agent_fixture() -> (App, Receiver<BackendRequest>, Session) {
    let (tx, sent) = std::sync::mpsc::channel();
    let (_, rx) = std::sync::mpsc::channel();
    let mut app = App::new(
        &Settings::default(),
        std::path::PathBuf::from("/tmp/staging"),
        Machine {
            ram_gb: 64.0,
            vram_gb: Some(24.0),
        },
        tx,
        rx,
    );
    let now = now_ms();
    let folder =
        std::env::temp_dir().join("a-project-with-a-rather-long-folder-name/and/some/nested/path");
    let mut s = Session::new_agent(LONG_MODEL, SessionLifetime::Persisted, folder, now).unwrap();
    s.push_user("Find where the config is loaded and explain it.", now);
    let run = RunId::new();
    let a = s.begin_assistant(run, TurnId::new(), now);
    s.append_assistant(a, "I will search the sources first.", "");
    s.finish_assistant(a, AssistantStatus::Complete, None, now);
    for (i, status, out) in [
        (
            1,
            ToolStatus::Complete,
            "src/config.rs:12: fn load() {}".repeat(20),
        ),
        (
            2,
            ToolStatus::Failed,
            "Error: access denied: .env is protected and cannot be read".to_string(),
        ),
        (3, ToolStatus::Running, String::new()),
    ] {
        let id = s.push_tool_call(
            run,
            &format!("call_{i}"),
            "search_text",
            serde_json::json!({"pattern": "load_config", "path": "src/some/very/long/path/to/search/in"}),
            now,
        );
        if status != ToolStatus::Running {
            s.finish_tool(id, &out, status, now);
        } else {
            s.start_tool(id);
        }
    }
    let mut agent_row = row_for(&s, "Config walkthrough");
    agent_row.mode = SessionMode::Agent;
    let chat = Session::new_chat("llama3.2:3b", SessionLifetime::Persisted, now);
    let chat_row = row_for(&chat, "Just a chat");
    let st = app.state_mut();
    st.ollama = Some(Some("0.9".into()));
    st.view = View::Work;
    st.chat.fold(ChatEvent::Rows(vec![agent_row, chat_row]));
    st.chat.fold(ChatEvent::Opened(Box::new(s.clone())));
    st.chat.fold(ChatEvent::ModelCapabilities {
        model: LONG_MODEL.into(),
        tools: true,
    });
    (app, sent, s)
}

#[test]
fn work_lists_only_agents_and_chat_lists_only_chats() {
    let size = (1180.0, 820.0);
    let (mut app, _sent, _s) = agent_fixture();
    let mut h = Harness::new(false);
    run(&mut app, &mut h, size, 3);
    let texts = h
        .frame_texts(vec2(size.0, size.1), vec![], |ui| app.show(ui))
        .join("\n");
    assert!(
        texts.contains("Config walkthrough") && !texts.contains("Just a chat"),
        "{texts}"
    );
    assert!(
        app.state_mut().chat_ui.probe.new_button.is_some(),
        "the new-agent button"
    );
    app.state_mut().view = View::Chat;
    run(&mut app, &mut h, size, 3);
    let texts = h
        .frame_texts(vec2(size.0, size.1), vec![], |ui| app.show(ui))
        .join("\n");
    assert!(
        texts.contains("Just a chat") && !texts.contains("Config walkthrough"),
        "{texts}"
    );
    assert!(
        texts.contains("Start a chat"),
        "an agent open on Work is not shown on Chat: {texts}"
    );
}

#[test]
fn enter_steers_a_running_agent() {
    let size = (1180.0, 820.0);
    let (mut app, sent, s) = agent_fixture();
    let mut h = Harness::new(false);
    app.state_mut().chat.fold(ChatEvent::TurnStarted {
        session: s.id,
        turn: TurnId::new(),
    });
    *app.state_mut().chat.draft_mut(s.id) = "look in src instead".into();
    app.state_mut().chat_ui.focus_composer = true;
    run(&mut app, &mut h, size, 3);
    h.frame(vec2(size.0, size.1), press_enter(), |ui| app.show(ui));
    run(&mut app, &mut h, size, 2);
    let cmds = sent_chat_commands(&sent);
    assert!(
        matches!(cmds.as_slice(), [ChatCommand::Send { text, .. }] if text == "look in src instead"),
        "{cmds:?}"
    );
}

#[test]
fn the_work_nav_item_and_menu_exist() {
    assert_eq!(View::ALL[..2], [View::Chat, View::Work]);
    let model =
        crate::menu::model::build(&crate::app::AppState::new(), crate::menu::Platform::Linux);
    let labels: Vec<_> = model
        .action_items()
        .iter()
        .map(|a| a.label.clone())
        .collect();
    assert!(labels.contains(&"Work".to_string()));
    assert!(labels.contains(&"New Agent…".to_string()));
}

// ----------------------------------------------------- the quieter chrome

fn open_menu(app: &mut App, h: &mut Harness, size: (f32, f32), at: Rect) {
    click(app, h, size, at.center());
    run(app, h, size, 2);
}

fn texts_of(app: &mut App, h: &mut Harness, size: (f32, f32)) -> String {
    h.frame_texts(vec2(size.0, size.1), vec![], |ui| app.show(ui))
        .join("\n")
}

#[test]
fn the_title_row_has_a_pencil_a_pin_and_a_trash_can_and_deleting_takes_a_confirm_then_offers_undo()
{
    let size = (1180.0, 820.0);
    let mut f = fixture_with(2);
    let mut h = Harness::new(false);
    run(&mut f.app, &mut h, size, 3);
    let p = f.app.state_mut().chat_ui.probe.clone();
    let header = p.header.unwrap();
    let (pencil, pin) = (p.rename_button.expect("pencil"), p.pin_button.expect("pin"));
    let (del, label) = p.delete_button.clone().expect("trash can");
    assert_eq!(label, "Delete…");
    for r in [pencil, pin, del] {
        assert!(inside(r, header), "{r:?} not in {header:?}");
    }
    assert!(pin.right() <= del.left(), "pin is left of the trash can");
    // No overflow menu is left.
    let shown = texts_of(&mut f.app, &mut h, size);
    assert!(
        !shown.lines().any(|l| l == "Rename" || l == "Delete"),
        "{shown}"
    );

    let _ = sent_chat_commands(&f.sent);
    click(&mut f.app, &mut h, size, del.center());
    let (del, label) = f
        .app
        .state_mut()
        .chat_ui
        .probe
        .delete_button
        .clone()
        .unwrap();
    assert_eq!(label, "Click again to delete");
    click(&mut f.app, &mut h, size, del.center());
    assert!(
        !sent_chat_commands(&f.sent)
            .iter()
            .any(|c| matches!(c, ChatCommand::Delete(_))),
        "a quick second click must not delete"
    );
    // After the pause the confirming click deletes and offers Undo.
    f.app.state_mut().chat_ui.confirm.arm(
        &f.open.id.to_string(),
        std::time::Instant::now() - std::time::Duration::from_secs(1),
    );
    run(&mut f.app, &mut h, size, 2);
    let (del, _) = f
        .app
        .state_mut()
        .chat_ui
        .probe
        .delete_button
        .clone()
        .unwrap();
    click(&mut f.app, &mut h, size, del.center());
    let cmds = sent_chat_commands(&f.sent);
    assert!(
        matches!(cmds.as_slice(), [ChatCommand::Delete(d)] if *d == f.open.id),
        "{cmds:?}"
    );
    assert!(f.app.state_mut().chat_ui.undo.is_some(), "and offers Undo");
}

#[test]
fn the_pin_toggles_and_an_incognito_chat_has_none() {
    let size = (1180.0, 820.0);
    let mut f = fixture_with(1);
    let mut h = Harness::new(false);
    run(&mut f.app, &mut h, size, 3);
    let id = f.open.id;
    let pin = f.app.state_mut().chat_ui.probe.pin_button.unwrap();
    let _ = sent_chat_commands(&f.sent);
    click(&mut f.app, &mut h, size, pin.center());
    let cmds = sent_chat_commands(&f.sent);
    assert!(
        matches!(cmds.as_slice(), [ChatCommand::SetPinned(i, true)] if *i == id),
        "{cmds:?}"
    );
    // An incognito chat is never saved, so it cannot be pinned.
    let inc = Session::new_chat("qwen2.5:7b", SessionLifetime::Incognito, now_ms());
    f.app
        .state_mut()
        .chat
        .fold(ChatEvent::Opened(Box::new(inc)));
    run(&mut f.app, &mut h, size, 3);
    assert!(f.app.state_mut().chat_ui.probe.pin_button.is_none());
    assert!(f.app.state_mut().chat_ui.probe.delete_button.is_some());
}

#[test]
fn the_pencil_and_a_double_click_make_the_title_editable() {
    let size = (1180.0, 820.0);
    let mut f = fixture_with(1);
    let mut h = Harness::new(false);
    run(&mut f.app, &mut h, size, 3);
    let id = f.open.id;
    let pencil = f.app.state_mut().chat_ui.probe.rename_button.unwrap();
    assert!(f.app.state_mut().chat_ui.renaming.is_none());
    click(&mut f.app, &mut h, size, pencil.center());
    assert_eq!(
        f.app.state_mut().chat_ui.renaming.as_ref().map(|r| r.0),
        Some(id)
    );
    // Enter keeps the new name.
    f.app.state_mut().chat_ui.renaming = Some((id, "A better name".into()));
    run(&mut f.app, &mut h, size, 3);
    let _ = sent_chat_commands(&f.sent);
    h.frame(
        vec2(size.0, size.1),
        vec![egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: Some(egui::Key::Enter),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
        |ui| f.app.show(ui),
    );
    run(&mut f.app, &mut h, size, 2);
    let cmds = sent_chat_commands(&f.sent);
    assert!(
        matches!(cmds.as_slice(), [ChatCommand::Rename(i, name)] if *i == id && name == "A better name"),
        "{cmds:?}"
    );
    assert!(f.app.state_mut().chat_ui.renaming.is_none());
}

#[test]
fn the_model_chip_shows_whether_the_model_is_in_memory_and_unloads_it() {
    let size = (1180.0, 820.0);
    let mut f = fixture_with(2);
    let mut h = Harness::new(false);
    run(&mut f.app, &mut h, size, 3);
    let texts = texts_of(&mut f.app, &mut h, size);
    assert!(
        texts.contains("○ ") && !texts.contains("● "),
        "not loaded yet: {texts}"
    );
    f.app.state_mut().running = vec![loaded(LONG_MODEL, "2099-01-01T00:00:00Z")];
    run(&mut f.app, &mut h, size, 3);
    let texts = texts_of(&mut f.app, &mut h, size);
    assert!(texts.contains("● "), "{texts}");
    let chip = f.app.state_mut().chat_ui.probe.model_chip.unwrap();
    open_menu(&mut f.app, &mut h, size, chip);
    let opened = texts_of(&mut f.app, &mut h, size);
    assert!(
        opened.contains("In memory") && opened.contains("stays loaded"),
        "{opened}"
    );
    let unload = f
        .app
        .state_mut()
        .chat_ui
        .probe
        .model_unload
        .expect("Unload in the popover");
    click(&mut f.app, &mut h, size, unload.center());
    assert_eq!(unload_requests(&f.sent), vec![LONG_MODEL.to_string()]);
    assert!(f.app.state_mut().running.is_empty());
}

#[test]
fn the_status_pill_lists_what_is_in_memory_and_says_when_nothing_is() {
    let size = (1180.0, 820.0);
    let mut f = fixture_with(2);
    let mut h = Harness::new(false);
    run(&mut f.app, &mut h, size, 3);
    let pill = f.app.state_mut().pill_rect.expect("pill");
    open_menu(&mut f.app, &mut h, size, pill);
    let empty = texts_of(&mut f.app, &mut h, size);
    assert!(empty.contains("Nothing loaded"), "{empty}");
    click(&mut f.app, &mut h, size, egui::pos2(5.0, 5.0)); // closes it
    f.app.state_mut().running = vec![loaded(LONG_MODEL, "2099-01-01T00:00:00Z")];
    run(&mut f.app, &mut h, size, 3);
    open_menu(&mut f.app, &mut h, size, pill);
    let listed = texts_of(&mut f.app, &mut h, size);
    assert!(
        listed.contains("In memory") && listed.contains(LONG_MODEL) && listed.contains("Unload"),
        "{listed}"
    );
}

#[test]
fn the_rail_has_one_new_button_and_no_footer() {
    let size = (1180.0, 820.0);
    let mut f = fixture_with(2);
    let mut h = Harness::new(false);
    run(&mut f.app, &mut h, size, 3);
    let texts = texts_of(&mut f.app, &mut h, size);
    assert!(
        f.app.state_mut().chat_ui.probe.incognito_button.is_some(),
        "the incognito button sits beside the new-chat button"
    );
    for gone in ["New incognito", "IN MEMORY", "In memory"] {
        assert!(
            !texts
                .to_uppercase()
                .lines()
                .any(|l| l == gone.to_uppercase()),
            "{gone}: {texts}"
        );
    }
    let new = f
        .app
        .state_mut()
        .chat_ui
        .probe
        .new_button
        .expect("+ New button");
    let rail = f.app.state_mut().chat_ui.probe.rail.unwrap();
    assert!(inside(new, rail), "{new:?} {rail:?}");
}

#[test]
fn the_rail_search_filters_the_list_and_stays_inside_the_rail() {
    let size = (900.0, 640.0);
    let mut f = fixture_with(3);
    let mut h = Harness::new(false);
    run(&mut f.app, &mut h, size, 3);
    let search = f
        .app
        .state_mut()
        .chat_ui
        .probe
        .rail_search
        .expect("search field");
    let rail = f.app.state_mut().chat_ui.probe.rail.unwrap();
    assert!(inside(search, rail), "{search:?} {rail:?}");
    let new = f.app.state_mut().chat_ui.probe.new_button.unwrap();
    assert!(
        search.right() <= new.left() && (search.center().y - new.center().y).abs() < 1.0,
        "the new-chat button is beside the search field: {search:?} {new:?}"
    );
    f.app.state_mut().chat_ui.rail_query = "no-such-session-anywhere".into();
    run(&mut f.app, &mut h, size, 3);
    let texts = texts_of(&mut f.app, &mut h, size);
    assert!(texts.contains("No matches."), "{texts}");
}

#[test]
fn an_agent_run_shows_progress_in_one_status_line_and_folds_tool_steps() {
    for family in [
        ThemeFamily::Meadow,
        ThemeFamily::Ember,
        ThemeFamily::Plum,
        ThemeFamily::Tide,
    ] {
        for dark in [false, true] {
            for size in SIZES {
                let (mut app, _sent, s) = agent_fixture();
                app.state_mut().chat.fold(ChatEvent::TurnStarted {
                    session: s.id,
                    turn: TurnId::new(),
                });
                app.state_mut().chat.fold(ChatEvent::SteeringQueued {
                    session: s.id,
                    text: "Only look in the src folder please, and ignore the tests entirely"
                        .into(),
                });
                let mut h = Harness::themed(family, dark);
                run(&mut app, &mut h, size, 4);
                let window = Rect::from_min_size(Pos2::ZERO, vec2(size.0, size.1));
                let p = app.state_mut().chat_ui.probe.clone();
                let tag = format!("{family:?} dark={dark} {size:?}");
                let (pane, composer, header) =
                    (p.pane.unwrap(), p.composer.unwrap(), p.header.unwrap());
                assert!(inside(pane, window) && inside(composer, window), "{tag}");
                let chip = p.agent_chip.expect("agent chip");
                let more = p.delete_button.clone().expect("trash can").0;
                assert!(
                    inside(chip, composer) && inside(more, header),
                    "{tag}: {chip:?} {more:?} {header:?}"
                );
                assert_eq!(
                    p.tool_groups.len(),
                    1,
                    "{tag}: three tool calls fold into one line"
                );
                assert!(
                    p.tool_cards.is_empty(),
                    "{tag}: cards stay folded until opened"
                );
                assert!(
                    p.queued_chips.is_empty(),
                    "{tag}: waiting messages live in the status line"
                );
                let texts = h
                    .frame_texts(vec2(size.0, size.1), vec![], |ui| app.show(ui))
                    .join("\n");
                assert!(
                    texts.contains("Used 3 tools: search_text ×3"),
                    "{tag}: {texts}"
                );
                // The group says when it started: `14:03` for today.
                let after = texts
                    .split("Used 3 tools: search_text ×3  ·  ")
                    .nth(1)
                    .unwrap_or("");
                let clock: Vec<char> = after.chars().take(5).collect();
                assert!(
                    clock.len() == 5
                        && clock[2] == ':'
                        && clock
                            .iter()
                            .enumerate()
                            .all(|(i, c)| i == 2 || c.is_ascii_digit()),
                    "{tag}: start time after the list: {texts}"
                );
                assert!(
                    texts.contains("1 failed") && texts.contains("search_text running…"),
                    "{tag}: {texts}"
                );
                assert!(texts.contains("Step 1 of 20"), "{tag}: {texts}");
                assert!(
                    texts.contains("1 message waiting for the next step"),
                    "{tag}: {texts}"
                );
            }
        }
    }
}

#[test]
fn opening_the_tool_summary_shows_the_cards_inside_the_pane() {
    for size in SIZES {
        let (mut app, _sent, _s) = agent_fixture();
        let mut h = Harness::new(false);
        run(&mut app, &mut h, size, 4);
        let group = app.state_mut().chat_ui.probe.tool_groups[0];
        click(
            &mut app,
            &mut h,
            size,
            egui::pos2(group.left() + 12.0, group.center().y),
        );
        run(&mut app, &mut h, size, 3);
        let p = app.state_mut().chat_ui.probe.clone();
        assert_eq!(p.tool_cards.len(), 3, "{:?}", p.tool_cards);
        let pane = p.pane.unwrap();
        for c in &p.tool_cards {
            assert!(
                c.left() >= pane.left() - 0.5 && c.right() <= pane.right() + 0.5,
                "{c:?} {pane:?}"
            );
        }
        // The group line and each of the three cards say when they started.
        let texts = texts_of(&mut app, &mut h, size);
        let clocks = texts
            .split(|c: char| !(c.is_ascii_digit() || c == ':'))
            .filter(|w| {
                let b = w.as_bytes();
                b.len() == 5
                    && b[2] == b':'
                    && b.iter()
                        .enumerate()
                        .all(|(i, c)| i == 2 || c.is_ascii_digit())
            })
            .count();
        // The text dump holds only what the pane shows: all four at the large
        // size, at least one card at the small one (the group line and the
        // lower cards may be scrolled out).
        if size.0 >= 1000.0 {
            assert_eq!(clocks, 4, "{size:?}: {texts}");
        } else {
            assert!(clocks >= 1, "{size:?}: {texts}");
        }
    }
}

#[test]
fn the_agent_chip_holds_the_folder_steps_and_tools_and_locks_while_running() {
    let size = (1180.0, 820.0);
    let (mut app, sent, s) = agent_fixture();
    let mut h = Harness::new(false);
    run(&mut app, &mut h, size, 4);
    let chip = app.state_mut().chat_ui.probe.agent_chip.unwrap();
    open_menu(&mut app, &mut h, size, chip);
    // The tool lists start closed; the heading opens one.
    let heads = app.state_mut().chat_ui.probe.tools_heads.clone();
    assert_eq!(heads.len(), 3, "Files, Web and Ask you, without commands");
    click(&mut app, &mut h, size, heads[0].center());
    let texts = texts_of(&mut app, &mut h, size);
    let upper = texts.to_uppercase();
    for want in ["FOLDER", "RUN", "FILES"] {
        assert!(upper.contains(want), "card {want}: {texts}");
    }
    for want in [
        "Change…",
        "Step budget",
        "Read files",
        "Write files",
        "write_file",
        "list_dir",
        "read_file",
        "find_files",
        "search_text",
    ] {
        assert!(texts.contains(want), "{want}: {texts}");
    }
    assert!(!texts.contains("Locked while a run is active."));
    let _ = (sent, s);
}

fn open_markdown_reply(f: &mut Fixture, reply: &str) {
    let now = now_ms();
    let mut s = Session::new_chat("llama3.2:3b", SessionLifetime::Persisted, now);
    s.push_user("Show me", now);
    let a = s.begin_assistant(RunId::new(), TurnId::new(), now);
    s.append_assistant(a, reply, "");
    s.finish_assistant(a, AssistantStatus::Complete, None, now);
    f.app.state_mut().chat.fold(ChatEvent::Opened(Box::new(s)));
}

#[test]
fn assistant_replies_are_drawn_as_markdown_and_user_text_is_not() {
    let size = (1180.0, 820.0);
    let mut f = fixture_with(1);
    let mut h = Harness::new(false);
    open_markdown_reply(
        &mut f,
        "Here is **bold** text:\n\n- first item\n- second item\n\n```\nlet x = 1;\n```",
    );
    run(&mut f.app, &mut h, size, 3);
    let texts = texts_of(&mut f.app, &mut h, size);
    assert!(!texts.contains("**"), "{texts}");
    assert!(!texts.contains("```"), "{texts}");
    assert!(
        texts.contains("bold") && texts.contains("first item"),
        "{texts}"
    );
    assert!(texts.contains("let x = 1;"), "{texts}");
}

#[test]
fn the_row_is_always_on_the_latest_message_and_on_hover_for_older_ones() {
    for size in SIZES {
        let mut f = fixture_with(1);
        let mut h = Harness::new(false);
        open_markdown_reply(&mut f, "A short answer.");
        run(&mut f.app, &mut h, size, 3);
        let probe = f.app.state_mut().chat_ui.probe.clone();
        let pane = probe.pane.expect("pane");
        assert_eq!(probe.copy_buttons.len(), 1, "{size:?}: latest only at rest");
        assert!(inside(probe.copy_buttons[0], pane), "{size:?}");
        let first = probe.cards[0];
        let latest = probe.cards[1];
        assert!(probe.copy_buttons[0].top() >= latest.bottom() - 0.5);
        // Pointer over the older bubble: its row appears too, below it.
        h.frame(
            vec2(size.0, size.1),
            vec![egui::Event::PointerMoved(first.center())],
            |ui| f.app.show(ui),
        );
        let probe = f.app.state_mut().chat_ui.probe.clone();
        assert_eq!(probe.copy_buttons.len(), 2, "{size:?}: hovered row shown");
        assert!(probe
            .copy_buttons
            .iter()
            .any(|b| b.top() >= first.bottom() - 0.5 && b.top() < latest.top()));
    }
}

#[test]
fn each_row_shows_when_the_message_was_sent_after_its_icons() {
    for size in SIZES {
        let mut f = fixture_with(1);
        let mut h = Harness::new(false);
        open_markdown_reply(&mut f, "A short answer.");
        run(&mut f.app, &mut h, size, 3);
        let probe = f.app.state_mut().chat_ui.probe.clone();
        let pane = probe.pane.expect("pane");
        assert_eq!(probe.stamps.len(), 1, "{size:?}: latest only at rest");
        let (rect, text) = &probe.stamps[0];
        assert!(inside(*rect, pane), "{size:?}");
        assert!(
            rect.left() > probe.details.expect("details").right(),
            "{size:?}"
        );
        let (want, _) = super::state::local_stamp(now_ms(), now_ms()).expect("time");
        assert_eq!(text.len(), want.len(), "{size:?}: {text}");
        assert!(text.contains(':'), "{size:?}: {text}");
        // Hovering the older bubble shows its row and time too.
        let first = probe.cards[0];
        h.frame(
            vec2(size.0, size.1),
            vec![egui::Event::PointerMoved(first.center())],
            |ui| f.app.show(ui),
        );
        let probe = f.app.state_mut().chat_ui.probe.clone();
        assert_eq!(probe.stamps.len(), 2, "{size:?}");
    }
}

#[test]
fn details_shows_tokens_and_speed_when_they_were_recorded() {
    use roost_core::session::ReplyStats;
    let size = (1180.0, 820.0);
    let mut f = fixture_with(1);
    let mut h = Harness::new(false);
    open_markdown_reply(&mut f, "A short answer.");
    {
        let st = f.app.state_mut();
        let open = st.chat.open.as_mut().expect("open");
        let id = open.messages.last().unwrap().id;
        open.set_reply_stats(
            id,
            ReplyStats {
                prompt_tokens: Some(12),
                reply_tokens: Some(40),
                total_ms: Some(2_500),
                load_ms: Some(300),
                eval_ms: Some(2_000),
            },
        );
    }
    run(&mut f.app, &mut h, size, 3);
    let details = f
        .app
        .state_mut()
        .chat_ui
        .probe
        .details
        .expect("Details icon");
    click(&mut f.app, &mut h, size, details.center());
    let texts = texts_of(&mut f.app, &mut h, size);
    for want in ["12 tokens", "40 tokens", "20.0 tokens/s", "2.5 s"] {
        assert!(texts.contains(want), "{want:?} missing in:\n{texts}");
    }
}

#[test]
fn details_admits_when_nothing_was_recorded() {
    let size = (1180.0, 820.0);
    let mut f = fixture_with(1);
    let mut h = Harness::new(false);
    open_markdown_reply(&mut f, "An old reply.");
    run(&mut f.app, &mut h, size, 3);
    let details = f
        .app
        .state_mut()
        .chat_ui
        .probe
        .details
        .expect("Details icon");
    click(&mut f.app, &mut h, size, details.center());
    let texts = texts_of(&mut f.app, &mut h, size);
    assert!(texts.contains("No timing was recorded"), "{texts}");
}

#[test]
fn headings_tables_quotes_and_rules_in_a_reply_are_not_shown_as_markup() {
    let size = (1180.0, 820.0);
    let mut f = fixture_with(1);
    let mut h = Harness::new(false);
    open_markdown_reply(
        &mut f,
        "# Title\n\n## Section\n\n> quoted words\n\n---\n\n| Name | Size |\n|------|------|\n| a | 1 |\n| b | 2 |\n\n1. one\n2. two",
    );
    run(&mut f.app, &mut h, size, 3);
    let texts = texts_of(&mut f.app, &mut h, size);
    for markup in [
        "# Title",
        "## Section",
        "> quoted",
        "|------|",
        "| Name",
        "---",
    ] {
        assert!(!texts.contains(markup), "{markup:?} shown raw in:\n{texts}");
    }
    for words in [
        "Title",
        "Section",
        "quoted words",
        "Name",
        "Size",
        "one",
        "two",
    ] {
        assert!(texts.contains(words), "{words:?} missing in:\n{texts}");
    }
}

#[test]
fn a_finished_reply_offers_regenerate_and_clicking_it_sends_the_command() {
    let size = (1180.0, 820.0);
    let mut f = fixture_with(1);
    let mut h = Harness::new(false);
    open_markdown_reply(&mut f, "A short answer.");
    let id = f.app.state_mut().chat.open.as_ref().unwrap().id;
    run(&mut f.app, &mut h, size, 3);
    let _ = sent_chat_commands(&f.sent);
    let button = f
        .app
        .state_mut()
        .chat_ui
        .probe
        .regenerate
        .expect("Regenerate button");
    click(&mut f.app, &mut h, size, button.center());
    let cmds = sent_chat_commands(&f.sent);
    assert!(
        matches!(cmds.as_slice(), [ChatCommand::Regenerate(s)] if *s == id),
        "{cmds:?}"
    );
}

#[test]
fn a_failed_reply_offers_retry_and_not_regenerate() {
    let size = (1180.0, 820.0);
    let mut f = fixture();
    let mut h = Harness::new(false);
    run(&mut f.app, &mut h, size, 3);
    assert!(f.app.state_mut().chat_ui.probe.regenerate.is_none());
}

fn open_two_versions(
    f: &mut Fixture,
) -> (
    roost_core::session::SessionId,
    roost_core::session::MessageId,
) {
    let now = now_ms();
    let mut s = Session::new_chat("llama3.2:3b", SessionLifetime::Persisted, now);
    let u = s.push_user("Show me", now);
    let a = s.begin_assistant(RunId::new(), TurnId::new(), now);
    s.append_assistant(a, "first answer", "");
    s.finish_assistant(a, AssistantStatus::Complete, None, now);
    s.fork_at(1);
    let b = s.begin_assistant(RunId::new(), TurnId::new(), now);
    s.append_assistant(b, "second answer", "");
    s.finish_assistant(b, AssistantStatus::Complete, None, now);
    let id = s.id;
    f.app.state_mut().chat.fold(ChatEvent::Opened(Box::new(s)));
    (id, u)
}

#[test]
fn a_message_with_versions_shows_a_counter_at_the_bottom_right_and_switches() {
    for size in SIZES {
        let mut f = fixture_with(1);
        let mut h = Harness::new(false);
        let (id, user) = open_two_versions(&mut f);
        run(&mut f.app, &mut h, size, 3);
        let _ = sent_chat_commands(&f.sent);
        let probe = f.app.state_mut().chat_ui.probe.clone();
        let pane = probe.pane.expect("pane");
        let counter = probe.version_counter.expect("counter");
        let (prev, next) = (probe.version_prev.unwrap(), probe.version_next.unwrap());
        let card = *probe.cards.last().unwrap();
        assert!(inside(counter, pane), "{size:?}");
        assert!(
            counter.top() >= card.bottom() - 0.5,
            "{size:?}: below the bubble"
        );
        assert!(
            counter.right() >= card.right() - 40.0,
            "{size:?}: at the right"
        );
        assert!(prev.right() <= next.left() + 0.5);
        let texts = texts_of(&mut f.app, &mut h, size);
        assert!(texts.contains("2 / 2"), "{texts}");
        click(&mut f.app, &mut h, size, prev.center());
        let cmds = sent_chat_commands(&f.sent);
        assert!(
            matches!(cmds.as_slice(), [ChatCommand::SwitchVersion { session, after, to: 0 }]
                if *session == id && *after == Some(user)),
            "{cmds:?}"
        );
    }
}

#[test]
fn a_message_without_versions_has_no_counter() {
    let mut f = fixture();
    let mut h = Harness::new(false);
    run(&mut f.app, &mut h, (1180.0, 820.0), 3);
    assert!(f.app.state_mut().chat_ui.probe.version_counter.is_none());
}

#[test]
fn editing_your_message_sends_the_new_text_as_an_edit() {
    let size = (1180.0, 820.0);
    let mut f = fixture_with(1);
    let mut h = Harness::new(false);
    open_markdown_reply(&mut f, "A short answer.");
    let (id, mid) = {
        let s = f.app.state_mut().chat.open.as_ref().unwrap();
        (s.id, s.messages[0].id)
    };
    run(&mut f.app, &mut h, size, 3);
    let first = f.app.state_mut().chat_ui.probe.cards[0];
    h.frame(
        vec2(size.0, size.1),
        vec![egui::Event::PointerMoved(first.center())],
        |ui| f.app.show(ui),
    );
    let edit = f
        .app
        .state_mut()
        .chat_ui
        .probe
        .edit_icon
        .expect("Edit icon");
    click(&mut f.app, &mut h, size, edit.center());
    assert_eq!(
        f.app.state_mut().chat_ui.editing.as_ref().map(|e| e.0),
        Some(mid)
    );
    f.app.state_mut().chat_ui.editing.as_mut().unwrap().1 = "a better question".into();
    run(&mut f.app, &mut h, size, 2);
    let send = f.app.state_mut().chat_ui.probe.edit_send.expect("Send");
    let _ = sent_chat_commands(&f.sent);
    click(&mut f.app, &mut h, size, send.center());
    let cmds = sent_chat_commands(&f.sent);
    assert!(
        matches!(cmds.as_slice(), [ChatCommand::Edit { session, message, text }]
            if *session == id && *message == mid && text == "a better question"),
        "{cmds:?}"
    );
    assert!(f.app.state_mut().chat_ui.editing.is_none());
}

/// A finished chat of one exchange, and a branch of it (opened).
fn open_a_branch(f: &mut Fixture) -> (Session, Session) {
    let now = now_ms();
    let mut parent = Session::new_chat("llama3.2:3b", SessionLifetime::Persisted, now);
    parent.title = "Lisbon trip".into();
    parent.push_user("Plan a trip", now);
    let a = parent.begin_assistant(RunId::new(), TurnId::new(), now);
    parent.append_assistant(a, "Day 1: Alfama", "");
    parent.finish_assistant(a, AssistantStatus::Complete, None, now);
    let mut b = parent.branch(a, None, now).unwrap();
    b.push_user("Make it about food", now);
    let st = f.app.state_mut();
    st.chat
        .fold(ChatEvent::Rows(vec![row_for(&parent, "Lisbon trip"), {
            let mut r = row_for(&b, "Lisbon trip (branch)");
            r.parent = b.parent;
            r
        }]));
    st.chat.fold(ChatEvent::Opened(Box::new(b.clone())));
    (parent, b)
}

#[test]
fn a_branched_chat_shows_where_it_came_from_and_asks_for_a_check_once() {
    for size in SIZES {
        let mut f = fixture_with(1);
        let mut h = Harness::new(false);
        let (parent, b) = open_a_branch(&mut f);
        run(&mut f.app, &mut h, size, 4);
        let cmds = sent_chat_commands(&f.sent);
        let checks = cmds
            .iter()
            .filter(|c| matches!(c, ChatCommand::CheckBranch(id) if *id == b.id))
            .count();
        assert_eq!(checks, 1, "{size:?}: {cmds:?}");
        let texts = texts_of(&mut f.app, &mut h, size);
        for want in [
            "Branched from",
            "Lisbon trip",
            "Day 1: Alfama",
            "branch point",
        ] {
            assert!(
                texts.contains(want),
                "{size:?}: {want:?} missing in:\n{texts}"
            );
        }
        let probe = f.app.state_mut().chat_ui.probe.clone();
        let pane = probe.pane.unwrap();
        let (id, link) = probe.strip_links[0];
        assert_eq!(id, parent.id);
        assert!(inside(link, pane), "{size:?}");
        click(&mut f.app, &mut h, size, link.center());
        let cmds = sent_chat_commands(&f.sent);
        assert!(
            cmds.iter()
                .any(|c| matches!(c, ChatCommand::Open(i) if *i == parent.id)),
            "{cmds:?}"
        );
    }
}

#[test]
fn the_strip_says_when_the_original_changed_or_is_gone() {
    use roost_core::chat::BranchState;
    let size = (1180.0, 820.0);
    for (state, want) in [
        (BranchState::Unchanged, "unchanged"),
        (
            BranchState::Changed,
            "The original changed after this branch was made",
        ),
        (BranchState::Deleted, "The original chat was deleted"),
    ] {
        let mut f = fixture_with(1);
        let mut h = Harness::new(false);
        let (_, b) = open_a_branch(&mut f);
        f.app.state_mut().chat.fold(ChatEvent::BranchChecked {
            session: b.id,
            state,
        });
        run(&mut f.app, &mut h, size, 3);
        let texts = texts_of(&mut f.app, &mut h, size);
        assert!(texts.contains(want), "{state:?}: {texts}");
        let view = f.app.state_mut().chat_ui.probe.view_original.is_some();
        assert_eq!(view, state == BranchState::Changed, "{state:?}");
    }
}

#[test]
fn a_chat_that_is_not_a_branch_has_no_strip() {
    let mut f = fixture();
    let mut h = Harness::new(false);
    run(&mut f.app, &mut h, (1180.0, 820.0), 3);
    let texts = texts_of(&mut f.app, &mut h, (1180.0, 820.0));
    assert!(!texts.contains("Branched from"));
}

#[test]
fn branches_are_listed_under_their_parent_in_the_sidebar() {
    let size = (1180.0, 820.0);
    let mut f = fixture_with(1);
    let mut h = Harness::new(false);
    let (parent, b) = open_a_branch(&mut f);
    run(&mut f.app, &mut h, size, 3);
    let rows = f.app.state_mut().chat_ui.probe.rows.clone();
    let rect_of = |id| {
        rows.iter()
            .find(|(i, _)| *i == id)
            .map(|(_, r)| *r)
            .unwrap()
    };
    let (p, c) = (rect_of(parent.id), rect_of(b.id));
    assert!(c.left() > p.left() + 8.0, "indented: {p:?} {c:?}");
    assert!(c.top() > p.top(), "below its parent");
}

#[test]
fn the_branch_icon_offers_two_ways_and_the_second_picks_a_model() {
    let size = (1180.0, 820.0);
    let mut f = fixture_with(1);
    let mut h = Harness::new(false);
    open_markdown_reply(&mut f, "A short answer.");
    let (id, reply) = {
        let s = f.app.state_mut().chat.open.as_ref().unwrap();
        (s.id, s.messages[1].id)
    };
    run(&mut f.app, &mut h, size, 3);
    let icon = f
        .app
        .state_mut()
        .chat_ui
        .probe
        .branch_icon
        .expect("Branch icon");
    click(&mut f.app, &mut h, size, icon.center());
    let plain = f
        .app
        .state_mut()
        .chat_ui
        .probe
        .branch_plain
        .expect("plain item");
    let _ = sent_chat_commands(&f.sent);
    click(&mut f.app, &mut h, size, plain.center());
    let cmds = sent_chat_commands(&f.sent);
    assert!(
        matches!(cmds.as_slice(), [ChatCommand::Branch { session, message, model: None }]
            if *session == id && *message == reply),
        "{cmds:?}"
    );
    // The model picker.
    click(&mut f.app, &mut h, size, icon.center());
    let pick = f
        .app
        .state_mut()
        .chat_ui
        .probe
        .branch_pick
        .expect("pick item");
    click(&mut f.app, &mut h, size, pick.center());
    assert!(f.app.state_mut().chat_ui.branch_dialog.is_some());
    run(&mut f.app, &mut h, size, 2);
    let texts = texts_of(&mut f.app, &mut h, size);
    assert!(texts.contains("Branch with another model"), "{texts}");
    let create = f
        .app
        .state_mut()
        .chat_ui
        .probe
        .branch_create
        .expect("Create");
    click(&mut f.app, &mut h, size, create.center());
    let cmds = sent_chat_commands(&f.sent);
    assert!(
        cmds.iter()
            .any(|c| matches!(c, ChatCommand::Branch { model: Some(_), .. })),
        "{cmds:?}"
    );
    assert!(f.app.state_mut().chat_ui.branch_dialog.is_none());
}

#[test]
fn a_side_question_is_asked_answered_and_can_be_added() {
    use roost_core::session::{BtwEntry, BtwId};
    let size = (1180.0, 820.0);
    let mut f = fixture_with(1);
    let mut h = Harness::new(false);
    open_markdown_reply(&mut f, "A short answer.");
    let (id, reply) = {
        let s = f.app.state_mut().chat.open.as_ref().unwrap();
        (s.id, s.messages[1].id)
    };
    run(&mut f.app, &mut h, size, 3);
    let icon = f.app.state_mut().chat_ui.probe.btw_icon.expect("BTW icon");
    click(&mut f.app, &mut h, size, icon.center());
    let texts = texts_of(&mut f.app, &mut h, size);
    assert!(texts.contains("Not added to this chat"), "{texts}");

    f.app.state_mut().chat_ui.btw_input = "why?".into();
    run(&mut f.app, &mut h, size, 2);
    let ask = f.app.state_mut().chat_ui.probe.btw_ask.expect("Ask");
    let _ = sent_chat_commands(&f.sent);
    click(&mut f.app, &mut h, size, ask.center());
    let cmds = sent_chat_commands(&f.sent);
    assert!(
        matches!(cmds.as_slice(), [ChatCommand::Btw { session, message, question }]
            if *session == id && *message == reply && question == "why?"),
        "{cmds:?}"
    );
    assert!(f.app.state_mut().chat_ui.btw_input.is_empty());

    let bid = BtwId::new();
    f.app.state_mut().chat.fold(ChatEvent::BtwStarted {
        session: id,
        id: bid,
        message: reply,
        question: "why?".into(),
    });
    run(&mut f.app, &mut h, size, 2);
    let texts = texts_of(&mut f.app, &mut h, size);
    assert!(texts.contains("Thinking"), "{texts}");

    f.app.state_mut().chat.fold(ChatEvent::BtwDone {
        session: id,
        entry: BtwEntry {
            id: bid,
            at: reply,
            question: "why?".into(),
            answer: "Because borrowing is checked at compile time.".into(),
            model: "m".into(),
            asked_at_ms: 1,
            added: false,
        },
    });
    run(&mut f.app, &mut h, size, 2);
    let texts = texts_of(&mut f.app, &mut h, size);
    assert!(texts.contains("Because borrowing"), "{texts}");
    let add = f
        .app
        .state_mut()
        .chat_ui
        .probe
        .btw_add
        .expect("Add to chat");
    let _ = sent_chat_commands(&f.sent);
    click(&mut f.app, &mut h, size, add.center());
    let cmds = sent_chat_commands(&f.sent);
    assert!(
        cmds.iter().any(
            |c| matches!(c, ChatCommand::BtwAdd { session, id: i } if *session == id && *i == bid)
        ),
        "{cmds:?}"
    );
}

#[test]
fn a_failed_side_question_shows_why() {
    use roost_core::session::BtwId;
    let size = (1180.0, 820.0);
    let mut f = fixture_with(1);
    let mut h = Harness::new(false);
    open_markdown_reply(&mut f, "A short answer.");
    let (id, reply) = {
        let s = f.app.state_mut().chat.open.as_ref().unwrap();
        (s.id, s.messages[1].id)
    };
    run(&mut f.app, &mut h, size, 3);
    let icon = f.app.state_mut().chat_ui.probe.btw_icon.unwrap();
    click(&mut f.app, &mut h, size, icon.center());
    let bid = BtwId::new();
    f.app.state_mut().chat.fold(ChatEvent::BtwStarted {
        session: id,
        id: bid,
        message: reply,
        question: "why?".into(),
    });
    f.app.state_mut().chat.fold(ChatEvent::BtwFailed {
        session: id,
        id: bid,
        message: "Ollama is not reachable.".into(),
    });
    run(&mut f.app, &mut h, size, 2);
    let texts = texts_of(&mut f.app, &mut h, size);
    assert!(texts.contains("Ollama is not reachable."), "{texts}");
    assert!(!texts.contains("Thinking"), "{texts}");
}

/// A speech engine that records what it is asked to do.
#[derive(Default)]
struct SpeechLog {
    spoken: Vec<String>,
    stops: usize,
    speaking: bool,
}

struct FakeSpeaker {
    available: Result<(), String>,
    log: std::sync::Arc<std::sync::Mutex<SpeechLog>>,
}

impl crate::speech::Speaker for FakeSpeaker {
    fn available(&self) -> Result<(), String> {
        self.available.clone()
    }
    fn speak(&mut self, text: &str) {
        let mut l = self.log.lock().unwrap();
        l.spoken.push(text.to_string());
        l.speaking = true;
    }
    fn stop(&mut self) {
        let mut l = self.log.lock().unwrap();
        l.stops += 1;
        l.speaking = false;
    }
    fn is_speaking(&mut self) -> bool {
        self.log.lock().unwrap().speaking
    }
}

fn with_speaker(
    f: &mut Fixture,
    available: Result<(), String>,
) -> std::sync::Arc<std::sync::Mutex<SpeechLog>> {
    let log = std::sync::Arc::new(std::sync::Mutex::new(SpeechLog::default()));
    f.app.state_mut().speaker = Box::new(FakeSpeaker {
        available,
        log: log.clone(),
    });
    log
}

#[test]
fn the_read_icon_speaks_the_plain_text_and_a_second_click_stops() {
    let size = (1180.0, 820.0);
    let mut f = fixture_with(1);
    let mut h = Harness::new(false);
    let log = with_speaker(&mut f, Ok(()));
    open_markdown_reply(&mut f, "A **short** answer.");
    run(&mut f.app, &mut h, size, 3);
    let icon = f
        .app
        .state_mut()
        .chat_ui
        .probe
        .read_icon
        .expect("Read icon");
    assert!(!f.app.state_mut().chat_ui.probe.read_disabled);
    click(&mut f.app, &mut h, size, icon.center());
    assert_eq!(log.lock().unwrap().spoken, ["A short answer."]);
    assert!(f.app.state_mut().chat_ui.reading.is_some());
    let icon = f.app.state_mut().chat_ui.probe.read_icon.unwrap();
    click(&mut f.app, &mut h, size, icon.center());
    assert_eq!(log.lock().unwrap().spoken.len(), 1, "no second speech");
    assert!(log.lock().unwrap().stops >= 1);
    assert!(f.app.state_mut().chat_ui.reading.is_none());
}

#[test]
fn when_the_speech_ends_by_itself_the_icon_goes_back() {
    let size = (1180.0, 820.0);
    let mut f = fixture_with(1);
    let mut h = Harness::new(false);
    let log = with_speaker(&mut f, Ok(()));
    open_markdown_reply(&mut f, "Hello.");
    run(&mut f.app, &mut h, size, 3);
    let icon = f.app.state_mut().chat_ui.probe.read_icon.unwrap();
    click(&mut f.app, &mut h, size, icon.center());
    assert!(f.app.state_mut().chat_ui.reading.is_some());
    log.lock().unwrap().speaking = false;
    run(&mut f.app, &mut h, size, 2);
    assert!(f.app.state_mut().chat_ui.reading.is_none());
}

#[test]
fn without_a_speech_engine_the_icon_is_disabled_and_does_nothing() {
    let size = (1180.0, 820.0);
    let mut f = fixture_with(1);
    let mut h = Harness::new(false);
    let log = with_speaker(&mut f, Err("No engine here.".into()));
    open_markdown_reply(&mut f, "Hello.");
    run(&mut f.app, &mut h, size, 3);
    let icon = f
        .app
        .state_mut()
        .chat_ui
        .probe
        .read_icon
        .expect("still shown");
    assert!(f.app.state_mut().chat_ui.probe.read_disabled);
    click(&mut f.app, &mut h, size, icon.center());
    assert!(log.lock().unwrap().spoken.is_empty());
    assert!(f.app.state_mut().chat_ui.reading.is_none());
}

#[test]
fn new_work_opens_a_session_without_a_model_or_a_folder_while_another_chat_is_open() {
    let size = (1180.0, 820.0);
    let (mut app, sent, _s) = agent_fixture();
    {
        let st = app.state_mut();
        st.installed
            .on_installed_loaded(&Ok(vec![crate::runtime::InstalledEntry {
                model: roost_core::ollama::InstalledModel {
                    name: "qwen:7b".into(),
                    size: 1,
                    params: None,
                    quant: None,
                    family: None,
                    modified: String::new(),
                    digest: String::new(),
                },
                source_repo: None,
            }]));
    }
    let mut h = Harness::new(false);
    run(&mut app, &mut h, size, 3);
    let button = app.state_mut().chat_ui.probe.new_button.expect("+ New");
    let _ = sent_chat_commands(&sent);
    click(
        &mut app,
        &mut h,
        size,
        egui::pos2(button.left() + 30.0, button.center().y),
    );
    let cmds = sent_chat_commands(&sent);
    assert!(
        matches!(cmds.as_slice(), [ChatCommand::CreateAgent { lifetime: SessionLifetime::Persisted, model, workspace: None }]
            if model.is_empty()),
        "New Work opens a session with no model and no folder, not a form: {cmds:?}"
    );
}

fn ask_for_approval(app: &mut App, session: roost_core::session::SessionId) {
    app.state_mut().chat.fold(ChatEvent::ApprovalRequested {
        session,
        call_id: "call_9".into(),
        name: "read_file".into(),
        arguments: serde_json::json!({"path": "src/main.rs"}),
        preview: None,
        run: None,
    });
}

#[test]
fn a_pending_approval_shows_what_the_agent_wants_and_three_ways_to_answer() {
    use roost_core::chat::ApprovalDecision;
    for size in SIZES {
        for (pick, want) in [
            (0usize, ApprovalDecision::Allow),
            (1, ApprovalDecision::AllowForSession),
            (2, ApprovalDecision::Deny),
        ] {
            let (mut app, sent, s) = agent_fixture();
            let mut h = Harness::new(false);
            ask_for_approval(&mut app, s.id);
            run(&mut app, &mut h, size, 4);
            let texts = texts_of(&mut app, &mut h, size);
            assert!(
                texts.contains("read_file") && texts.contains("src/main.rs"),
                "{texts}"
            );
            let p = app.state_mut().chat_ui.probe.clone();
            let composer = p.composer.unwrap();
            let pane = p.pane.unwrap();
            assert_eq!(p.approval_buttons.len(), 3, "{size:?}");
            for b in &p.approval_buttons {
                assert!(inside(*b, composer), "{size:?}: {b:?} outside {composer:?}");
                assert!(
                    b.top() >= pane.bottom() - 0.5,
                    "{size:?}: below the transcript"
                );
            }
            let _ = sent_chat_commands(&sent);
            click(&mut app, &mut h, size, p.approval_buttons[pick].center());
            let cmds = sent_chat_commands(&sent);
            assert!(
                matches!(cmds.as_slice(), [ChatCommand::ResolveApproval { session, call_id, decision }]
                    if *session == s.id && call_id == "call_9" && *decision == want),
                "{size:?} pick {pick}: {cmds:?}"
            );
            run(&mut app, &mut h, size, 2);
            assert!(
                app.state_mut().chat_ui.probe.approval_buttons.is_empty(),
                "the card goes away once answered"
            );
        }
    }
}

#[test]
fn the_approval_card_goes_when_the_tool_starts_or_the_run_ends() {
    let size = (1180.0, 820.0);
    for end in [0, 1] {
        let (mut app, _sent, s) = agent_fixture();
        let mut h = Harness::new(false);
        ask_for_approval(&mut app, s.id);
        run(&mut app, &mut h, size, 3);
        assert_eq!(app.state_mut().chat_ui.probe.approval_buttons.len(), 3);
        if end == 0 {
            app.state_mut().chat.fold(ChatEvent::ToolStarted {
                session: s.id,
                call_id: "call_9".into(),
                name: "read_file".into(),
                arguments: serde_json::json!({}),
            });
        } else {
            app.state_mut().chat.fold(ChatEvent::RunEnded {
                session: s.id,
                outcome: roost_core::chat::RunOutcome::Cancelled,
                message: None,
            });
        }
        run(&mut app, &mut h, size, 3);
        assert!(
            app.state_mut().chat_ui.probe.approval_buttons.is_empty(),
            "end {end}"
        );
    }
}

#[test]
fn the_tools_menu_sets_files_read_to_off_ask_or_auto() {
    use roost_core::session::Permission;
    let size = (1180.0, 820.0);
    let (mut app, sent, s) = agent_fixture();
    let mut h = Harness::new(false);
    run(&mut app, &mut h, size, 3);
    let chip = app
        .state_mut()
        .chat_ui
        .probe
        .agent_chip
        .expect("Agent chip");
    click(&mut app, &mut h, size, chip.center());
    let ask = app.state_mut().chat_ui.probe.permission_buttons.clone();
    assert_eq!(ask.len(), 3, "Off, Ask, Allow");
    let _ = sent_chat_commands(&sent);
    click(&mut app, &mut h, size, ask[1].center());
    let cmds = sent_chat_commands(&sent);
    assert!(
        matches!(cmds.as_slice(), [ChatCommand::SetCapability(id, key, Permission::Ask)]
            if *id == s.id && key == "files.read"),
        "{cmds:?}"
    );
}

#[test]
fn the_header_names_the_session_kind_and_shows_where_an_agent_works() {
    for size in SIZES {
        let (mut app, _sent, _s) = agent_fixture();
        let mut h = Harness::new(false);
        run(&mut app, &mut h, size, 4);
        let texts = texts_of(&mut app, &mut h, size);
        assert!(
            texts.contains("Config walkthrough") || texts.contains("Untitled"),
            "{texts}"
        );
        assert!(texts.contains("Idle"), "{texts}");
        assert!(
            !texts
                .lines()
                .any(|l| l.starts_with('(') && (l.contains("a-project") || l.contains('…'))),
            "the folder is not wrapped in parentheses:\n{texts}"
        );
        let p = app.state_mut().chat_ui.probe.clone();
        let header = p.header.unwrap();
        let settings = p.agent_chip.unwrap();
        let more = p.delete_button.clone().unwrap().0;
        assert!(
            inside(settings, p.composer.unwrap()) && inside(more, header),
            "{size:?}"
        );
        // The folder is its own line under the title, from the same left edge.
        let (title, path) = (p.title_rect.unwrap(), p.path_rect.expect("the folder"));
        assert!(
            path.top() >= title.bottom() - 0.5,
            "{size:?}: {path:?} under {title:?}"
        );
        assert!(
            (path.left() - title.left() - 6.0).abs() < 0.5,
            "{size:?}: the folder is set in a little from the title"
        );
        assert!(inside(path, header), "{size:?}: {path:?} in {header:?}");
    }
}

#[test]
fn a_chat_has_no_settings_button_and_no_folder() {
    let size = (1180.0, 820.0);
    let mut f = fixture();
    let mut h = Harness::new(false);
    run(&mut f.app, &mut h, size, 3);
    let texts = texts_of(&mut f.app, &mut h, size);
    assert!(!texts.contains("Agent settings"), "{texts}");
    assert!(f.app.state_mut().chat_ui.probe.agent_chip.is_none());
    assert_eq!(
        f.app.state_mut().chat_ui.probe.status_label.as_deref(),
        Some("Idle")
    );
}

#[test]
fn the_status_says_when_the_agent_needs_an_answer() {
    let size = (1180.0, 820.0);
    let (mut app, _sent, s) = agent_fixture();
    let mut h = Harness::new(false);
    ask_for_approval(&mut app, s.id);
    run(&mut app, &mut h, size, 3);
    assert_eq!(
        app.state_mut().chat_ui.probe.status_label.as_deref(),
        Some("Waiting for you")
    );
}

#[test]
fn a_very_long_folder_is_cut_in_the_middle_and_the_header_still_fits() {
    let size = (900.0, 640.0);
    let (mut app, _sent, s) = agent_fixture();
    {
        let long = std::env::temp_dir().join(
            "a/very/long/path/that/keeps/going/and/going/through/many/folders/to/some-project-name",
        );
        let st = app.state_mut();
        let open = st.chat.open.as_mut().unwrap();
        open.workspace = Some(long);
        let _ = s;
    }
    let mut h = Harness::new(false);
    run(&mut app, &mut h, size, 4);
    let texts = texts_of(&mut app, &mut h, size);
    let line = texts
        .lines()
        .find(|l| l.contains('…') && l.ends_with("some-project-name"))
        .unwrap_or_else(|| panic!("the folder was not cut:\n{texts}"));
    assert!(!line.starts_with('('), "{line}");
    let p = app.state_mut().chat_ui.probe.clone();
    assert!(inside(p.agent_chip.unwrap(), p.composer.unwrap()));
}

#[test]
fn a_very_long_title_never_runs_into_the_status_or_the_buttons() {
    let long = "A really quite remarkably long title that just keeps going and going well past the end of the line ".repeat(3);
    for size in SIZES {
        for agent in [false, true] {
            let (mut app, _sent, _s) = agent_fixture();
            let mut h = Harness::new(false);
            let id = {
                let open = app.state_mut().chat.open.as_mut().unwrap();
                open.title = long.clone();
                if !agent {
                    open.mode = SessionMode::Chat;
                }
                open.id
            };
            // The list row carries the title that is shown.
            let mut row = row_for(app.state_mut().chat.open.as_ref().unwrap(), &long);
            row.id = id;
            row.mode = if agent {
                SessionMode::Agent
            } else {
                SessionMode::Chat
            };
            app.state_mut().view = if agent { View::Work } else { View::Chat };
            app.state_mut().chat.fold(ChatEvent::Rows(vec![row]));
            run(&mut app, &mut h, size, 4);
            let p = app.state_mut().chat_ui.probe.clone();
            let title = p.title_rect.expect("title");
            let status = p.status_rect.expect("status text");
            assert!(
                title.right() <= status.left() - 8.0,
                "{size:?} agent={agent}: title {title:?} runs into the status {status:?}"
            );
            if let Some(path) = p.path_rect {
                assert!(
                    path.right() <= status.left() - 8.0,
                    "{size:?}: folder {path:?} runs into the status {status:?}"
                );
                assert!(
                    path.top() >= title.bottom() - 0.5,
                    "{size:?}: the folder is under the title"
                );
            }
            let pencil = p.rename_button.unwrap();
            assert!(
                pencil.right() <= status.left() - 4.0,
                "{size:?}: pencil over the status"
            );
        }
    }
}

#[test]
fn the_header_has_a_chip_per_capability_and_the_write_chip_goes_with_the_switch() {
    use roost_core::session::Permission;
    for size in SIZES {
        let (mut app, _sent, s) = agent_fixture();
        let mut h = Harness::new(false);
        run(&mut app, &mut h, size, 4);
        let p = app.state_mut().chat_ui.probe.clone();
        let composer = p.composer.unwrap();
        let model = p.model_chip.unwrap();
        let letters: Vec<_> = p.chips.iter().map(|c| c.0).collect();
        assert_eq!(
            letters,
            ['R', 'W', 'N'],
            "{size:?}: no X until commands exist"
        );
        for (_, r, _) in &p.chips {
            assert!(inside(*r, composer), "{size:?}: chip {r:?}");
            assert!(r.right() <= model.left(), "{size:?}: left of the model");
        }
        assert!(p.chips[0].1.right() <= p.chips[1].1.left());
        // Read allows, write asks, as a new agent is set.
        assert_eq!(p.chips[0].2, Permission::Auto);
        assert_eq!(p.chips[1].2, Permission::Ask);
        // The tools button sits on the left, the chips on the right.
        assert!(
            p.agent_chip.unwrap().right() <= p.chips[0].1.left(),
            "{size:?}"
        );
        // Switched off in Settings: no W at all.
        app.state_mut().settings.agents_can_write = false;
        run(&mut app, &mut h, size, 3);
        let letters: Vec<_> = app
            .state_mut()
            .chat_ui
            .probe
            .chips
            .iter()
            .map(|c| c.0)
            .collect();
        assert_eq!(letters, ['R', 'N'], "{size:?}");
        let _ = s;
    }
}

#[test]
fn the_tools_menu_has_a_write_row_with_off_ask_and_allow() {
    use roost_core::session::Permission;
    let size = (1180.0, 820.0);
    let (mut app, sent, s) = agent_fixture();
    let mut h = Harness::new(false);
    run(&mut app, &mut h, size, 3);
    let chip = app.state_mut().chat_ui.probe.agent_chip.unwrap();
    click(&mut app, &mut h, size, chip.center());
    let write = app.state_mut().chat_ui.probe.write_buttons.clone();
    assert_eq!(write.len(), 3, "Off, Ask, Allow");
    let _ = sent_chat_commands(&sent);
    click(&mut app, &mut h, size, write[0].center());
    let cmds = sent_chat_commands(&sent);
    assert!(
        matches!(cmds.as_slice(), [ChatCommand::SetCapability(id, key, Permission::Off)]
            if *id == s.id && key == "files.write"),
        "{cmds:?}"
    );
    // With writing off in Settings the row is not there.
    app.state_mut().settings.agents_can_write = false;
    run(&mut app, &mut h, size, 3);
    assert!(app.state_mut().chat_ui.probe.write_buttons.is_empty());
}

fn ask_to_write(app: &mut App, session: roost_core::session::SessionId) {
    use roost_core::agent::change::{DiffLine, LineKind};
    let line = |kind, text: &str| DiffLine {
        kind,
        text: text.into(),
    };
    app.state_mut().chat.fold(ChatEvent::ApprovalRequested {
        session,
        call_id: "call_9".into(),
        name: "edit_file".into(),
        arguments: serde_json::json!({"path": "src/config.rs"}),
        preview: Some(roost_core::agent::change::ChangePreview {
            path: "src/config.rs".into(),
            created: false,
            added: 3,
            removed: 1,
            lines: vec![
                line(LineKind::Same, "fn load() {"),
                line(LineKind::Removed, "    let path = default_path();"),
                line(LineKind::Added, "    let path = env_path();"),
                line(LineKind::Added, "    let more = 1;"),
                line(LineKind::Added, "    let again = 2;"),
                line(LineKind::Same, "}"),
            ],
            more: 4,
        }),
        run: None,
    });
}

#[test]
fn a_write_asks_with_the_file_the_counts_and_the_diff_and_can_be_answered() {
    use roost_core::chat::ApprovalDecision;
    for size in SIZES {
        for (pick, want) in [
            (0usize, ApprovalDecision::Allow),
            (1, ApprovalDecision::AllowForSession),
            (2, ApprovalDecision::Deny),
        ] {
            let (mut app, sent, s) = agent_fixture();
            let mut h = Harness::new(false);
            ask_to_write(&mut app, s.id);
            run(&mut app, &mut h, size, 4);
            let texts = texts_of(&mut app, &mut h, size);
            for want_text in [
                "The agent wants to change a file",
                "src/config.rs",
                "+3",
                "−1",
                "let path = env_path();",
                "4 more changed lines",
                "Allow edits to this file this session",
            ] {
                assert!(texts.contains(want_text), "{size:?}: {want_text}\n{texts}");
            }
            let p = app.state_mut().chat_ui.probe.clone();
            let composer = p.composer.unwrap();
            assert_eq!(p.approval_buttons.len(), 3);
            for b in &p.approval_buttons {
                assert!(inside(*b, composer), "{size:?}: {b:?} outside {composer:?}");
            }
            let _ = sent_chat_commands(&sent);
            click(&mut app, &mut h, size, p.approval_buttons[pick].center());
            let cmds = sent_chat_commands(&sent);
            assert!(
                matches!(cmds.as_slice(), [ChatCommand::ResolveApproval { decision, .. }]
                    if *decision == want),
                "{size:?} pick {pick}: {cmds:?}"
            );
        }
    }
}

/// An agent chat in which one write was made through the tool message
/// `message` of the fixture.
fn with_a_change(app: &mut App, session: &mut Session, undone: bool) {
    use roost_core::agent::change::{ChangePreview, DiffLine, LineKind};
    let message = session
        .messages
        .iter()
        .find(|m| matches!(m.content, roost_core::session::MessageContent::Tool { .. }))
        .map(|m| m.id)
        .expect("the fixture has tool messages");
    session.changes.push(roost_core::session::ChangeRecord {
        id: 1,
        message,
        root: std::path::PathBuf::new(),
        path: "src/config.rs".into(),
        created: false,
        before: if undone {
            String::new()
        } else {
            "old\n".into()
        },
        after_hash: 7,
        added: 3,
        removed: 1,
        preview: ChangePreview {
            path: "src/config.rs".into(),
            created: false,
            added: 3,
            removed: 1,
            lines: vec![
                DiffLine {
                    kind: LineKind::Removed,
                    text: "let a = old();".into(),
                },
                DiffLine {
                    kind: LineKind::Added,
                    text: "let a = brand_new();".into(),
                },
            ],
            more: 0,
        },
        undone,
    });
    let open = app.state_mut().chat.open.as_mut().unwrap();
    open.changes = session.changes.clone();
}

#[test]
fn a_change_shows_under_its_tool_with_show_diff_and_undo_that_sends_the_command() {
    for size in SIZES {
        let (mut app, sent, mut s) = agent_fixture();
        with_a_change(&mut app, &mut s, false);
        let mut h = Harness::new(false);
        run(&mut app, &mut h, size, 4);
        let texts = texts_of(&mut app, &mut h, size);
        for want in [
            "Changed",
            "src/config.rs",
            "+3",
            "−1",
            "Show diff",
            "Undo this change",
        ] {
            assert!(texts.contains(want), "{size:?}: {want}\n{texts}");
        }
        assert!(
            !texts.contains("brand_new"),
            "the diff is folded until asked for"
        );
        let p = app.state_mut().chat_ui.probe.clone();
        let (_, undo) = p.undo_buttons[0];
        let pane = p.pane.unwrap();
        assert!(inside(undo, pane), "{size:?}: {undo:?} in {pane:?}");
        let (_, row) = p.change_rows[0];
        assert!(inside(undo, row), "{size:?}: the button is inside its row");
        // Show diff opens the changed lines.
        let show = p.show_diff_buttons[0].1;
        click(&mut app, &mut h, size, show.center());
        run(&mut app, &mut h, size, 2);
        let texts = texts_of(&mut app, &mut h, size);
        assert!(
            texts.contains("brand_new") && texts.contains("Hide diff"),
            "{size:?}\n{texts}"
        );
        // Undo sends one command for that change.
        let p = app.state_mut().chat_ui.probe.clone();
        let _ = sent_chat_commands(&sent);
        click(&mut app, &mut h, size, p.undo_buttons[0].1.center());
        let cmds = sent_chat_commands(&sent);
        assert!(
            matches!(cmds.as_slice(), [ChatCommand::UndoChange { session, change: 1 }] if *session == s.id),
            "{size:?}: {cmds:?}"
        );
    }
}

#[test]
fn an_undone_change_says_so_and_offers_no_second_undo() {
    let size = (1180.0, 820.0);
    let (mut app, _sent, mut s) = agent_fixture();
    with_a_change(&mut app, &mut s, true);
    let mut h = Harness::new(false);
    run(&mut app, &mut h, size, 4);
    let texts = texts_of(&mut app, &mut h, size);
    assert!(texts.contains("Undone"), "{texts}");
    assert!(!texts.contains("Undo this change"), "{texts}");
    assert!(app.state_mut().chat_ui.probe.undo_buttons.is_empty());
}

#[test]
fn changes_made_lists_every_change_with_undo_all_in_the_agent_settings() {
    // Tall enough to show the whole settings list without scrolling it.
    let size = (1180.0, 1000.0);
    let (mut app, sent, mut s) = agent_fixture();
    with_a_change(&mut app, &mut s, false);
    let mut h = Harness::new(false);
    run(&mut app, &mut h, size, 3);
    let chip = app.state_mut().chat_ui.probe.agent_chip.unwrap();
    click(&mut app, &mut h, size, chip.center());
    let texts = texts_of(&mut app, &mut h, size);
    assert!(texts.to_uppercase().contains("CHANGES MADE"), "{texts}");
    assert!(texts.contains("Undo all"), "{texts}");
    let all = app
        .state_mut()
        .chat_ui
        .probe
        .undo_all_button
        .expect("Undo all");
    let _ = sent_chat_commands(&sent);
    click(&mut app, &mut h, size, all.center());
    let cmds = sent_chat_commands(&sent);
    assert!(
        matches!(cmds.as_slice(), [ChatCommand::UndoAll(id)] if *id == s.id),
        "{cmds:?}"
    );
}

#[test]
fn undo_is_off_while_the_agent_is_working() {
    let size = (1180.0, 820.0);
    let (mut app, sent, mut s) = agent_fixture();
    with_a_change(&mut app, &mut s, false);
    app.state_mut().chat.fold(ChatEvent::TurnStarted {
        session: s.id,
        turn: roost_core::session::TurnId::new(),
    });
    let mut h = Harness::new(false);
    run(&mut app, &mut h, size, 4);
    let p = app.state_mut().chat_ui.probe.clone();
    let Some((_, undo)) = p.undo_buttons.first().copied() else {
        return; // not drawn at all is also fine
    };
    let _ = sent_chat_commands(&sent);
    click(&mut app, &mut h, size, undo.center());
    assert!(
        sent_chat_commands(&sent).is_empty(),
        "a disabled button sends nothing"
    );
}

#[test]
fn a_refused_undo_tells_the_user_why() {
    let size = (1180.0, 820.0);
    let (mut app, _sent, s) = agent_fixture();
    app.state_mut().chat.fold(ChatEvent::UndoRefused {
        session: s.id,
        change: 1,
        reason: "src/config.rs was changed after the agent wrote it, so it was left alone.".into(),
    });
    let mut h = Harness::new(false);
    run(&mut app, &mut h, size, 4);
    let texts = texts_of(&mut app, &mut h, size);
    assert!(texts.contains("was left alone"), "{texts}");
}

fn open_agent_settings(app: &mut App, h: &mut Harness, size: (f32, f32)) {
    run(app, h, size, 3);
    let chip = app.state_mut().chat_ui.probe.agent_chip.unwrap();
    click(app, h, size, chip.center());
}

#[test]
fn commands_are_nowhere_in_the_chat_until_switched_on_in_settings() {
    let size = (1180.0, 820.0);
    let (mut app, _sent, _s) = agent_fixture();
    let mut h = Harness::new(false);
    open_agent_settings(&mut app, &mut h, size);
    let p = app.state_mut().chat_ui.probe.clone();
    let letters: Vec<_> = p.chips.iter().map(|c| c.0).collect();
    assert_eq!(letters, ['R', 'W', 'N']);
    assert!(p.run_buttons.is_empty(), "a Run row without the switch");
    let texts = texts_of(&mut app, &mut h, size);
    assert!(
        !texts.contains("run_command") && !texts.to_uppercase().contains("SHELL"),
        "{texts}"
    );
}

#[test]
fn with_commands_on_there_is_an_x_chip_and_a_run_row_that_starts_off() {
    use roost_core::session::Permission;
    // Tall windows: the tools menu opens above the button and scrolls.
    for size in [(1180.0, 1400.0), (900.0, 1400.0)] {
        let (mut app, sent, s) = agent_fixture();
        app.state_mut().settings.agents_can_run = true;
        let mut h = Harness::new(false);
        open_agent_settings(&mut app, &mut h, size);
        let p = app.state_mut().chat_ui.probe.clone();
        let letters: Vec<_> = p.chips.iter().map(|c| c.0).collect();
        assert_eq!(letters, ['R', 'W', 'X', 'N'], "{size:?}");
        assert_eq!(
            p.chips[2].2,
            Permission::Off,
            "{size:?}: new agents start with Run off"
        );
        assert!(inside(p.chips[2].1, p.composer.unwrap()), "{size:?}");
        assert_eq!(p.run_buttons.len(), 3, "{size:?}: Off, Ask, Allow");
        if size.1 < 800.0 {
            // In a short window the menu scrolls, so the row may be out of view.
            continue;
        }
        let _ = sent_chat_commands(&sent);
        click(&mut app, &mut h, size, p.run_buttons[1].center());
        let cmds = sent_chat_commands(&sent);
        assert!(
            matches!(cmds.as_slice(), [ChatCommand::SetCapability(id, key, Permission::Ask)]
                if *id == s.id && key == "shell.run"),
            "{size:?}: {cmds:?}"
        );
    }
}

#[test]
fn allowing_commands_without_asking_needs_one_confirmation() {
    use roost_core::session::Permission;
    let size = (1180.0, 1400.0);
    let (mut app, sent, s) = agent_fixture();
    app.state_mut().settings.agents_can_run = true;
    let mut h = Harness::new(false);
    open_agent_settings(&mut app, &mut h, size);
    let allow = app.state_mut().chat_ui.probe.run_buttons[2];
    let _ = sent_chat_commands(&sent);
    click(&mut app, &mut h, size, allow.center());
    assert!(
        sent_chat_commands(&sent).is_empty(),
        "sent before it was confirmed"
    );
    let (confirm, cancel) = app
        .state_mut()
        .chat_ui
        .probe
        .run_confirm
        .expect("a confirmation");
    // Cancelling sends nothing and puts the question away.
    click(&mut app, &mut h, size, cancel.center());
    assert!(sent_chat_commands(&sent).is_empty());
    assert!(app.state_mut().chat_ui.probe.run_confirm.is_none());
    // Asking again and confirming sets it.
    let allow = app.state_mut().chat_ui.probe.run_buttons[2];
    click(&mut app, &mut h, size, allow.center());
    let (confirm2, _) = app
        .state_mut()
        .chat_ui
        .probe
        .run_confirm
        .expect("a confirmation");
    let _ = confirm;
    click(&mut app, &mut h, size, confirm2.center());
    let cmds = sent_chat_commands(&sent);
    assert!(
        matches!(cmds.as_slice(), [ChatCommand::SetCapability(id, key, Permission::Auto)]
            if *id == s.id && key == "shell.run"),
        "{cmds:?}"
    );
}

#[test]
fn a_family_list_of_tools_opens_and_closes_from_its_heading() {
    let size = (1180.0, 820.0);
    let (mut app, _sent, _s) = agent_fixture();
    let mut h = Harness::new(false);
    open_agent_settings(&mut app, &mut h, size);
    assert!(!texts_of(&mut app, &mut h, size).contains("read_binary"));
    let head = app.state_mut().chat_ui.probe.tools_heads[0];
    click(&mut app, &mut h, size, head.center());
    assert!(texts_of(&mut app, &mut h, size).contains("read_binary"));
    let head = app.state_mut().chat_ui.probe.tools_heads[0];
    click(&mut app, &mut h, size, head.center());
    assert!(!texts_of(&mut app, &mut h, size).contains("read_binary"));
}

#[test]
fn each_listed_tool_shows_its_limit_and_a_click_explains_it() {
    use super::tool_limits;
    let lim = roost_core::agent::tools::Limits::default();
    // The chips fit at both sizes.
    for size in SIZES {
        let (mut app, _sent, _s) = agent_fixture();
        let mut h = Harness::new(false);
        open_agent_settings(&mut app, &mut h, size);
        let head = app.state_mut().chat_ui.probe.tools_heads[0];
        click(&mut app, &mut h, size, head.center());
        run(&mut app, &mut h, size, 2);
        let chips = app.state_mut().chat_ui.probe.tool_limit_chips.clone();
        assert_eq!(chips.len(), 8, "{size:?}: one per Files tool");
        for c in &chips {
            assert!(c.left() >= 0.0 && c.right() <= size.0, "{size:?}: {c:?}");
        }
    }
    // Clicking one opens its limits; the button goes to Settings.
    let size = (1180.0, 820.0);
    let (mut app, _sent, _s) = agent_fixture();
    let mut h = Harness::new(false);
    open_agent_settings(&mut app, &mut h, size);
    let head = app.state_mut().chat_ui.probe.tools_heads[0];
    click(&mut app, &mut h, size, head.center());
    run(&mut app, &mut h, size, 2);
    let texts = texts_of(&mut app, &mut h, size);
    let first = tool_limits::summary("list_dir", &lim).unwrap();
    assert!(texts.contains(&first), "{first}: {texts}");
    assert!(!texts.contains("Entries per call"), "closed until clicked");
    let chip = app.state_mut().chat_ui.probe.tool_limit_chips[0];
    click(&mut app, &mut h, size, chip.center());
    run(&mut app, &mut h, size, 2);
    let texts = texts_of(&mut app, &mut h, size);
    assert!(texts.contains("Entries per call"), "{texts}");
    assert!(texts.contains("Change in Settings"), "{texts}");
    let p = app.state_mut().chat_ui.probe.clone();
    let window = Rect::from_min_size(Pos2::ZERO, vec2(size.0, size.1));
    assert!(inside(p.tool_limit_popover.expect("popover"), window));
    click(
        &mut app,
        &mut h,
        size,
        p.tool_limit_settings.expect("button").center(),
    );
    assert_eq!(app.state_mut().view, View::Settings);
}

#[test]
fn the_shell_family_lists_run_command_when_commands_are_on() {
    let size = (1180.0, 820.0);
    let (mut app, _sent, _s) = agent_fixture();
    app.state_mut().settings.agents_can_run = true;
    let mut h = Harness::new(false);
    open_agent_settings(&mut app, &mut h, size);
    let heads = app.state_mut().chat_ui.probe.tools_heads.clone();
    assert_eq!(heads.len(), 4, "Files, Shell, Web and Ask you");
    click(&mut app, &mut h, size, heads[1].center());
    let texts = texts_of(&mut app, &mut h, size);
    assert!(texts.contains("X   run_command"), "{texts}");
    assert!(texts.to_uppercase().contains("SHELL"), "{texts}");
}

fn ask_to_run(
    app: &mut App,
    session: roost_core::session::SessionId,
    command: &str,
    prefix: Option<&str>,
) {
    app.state_mut().chat.fold(ChatEvent::ApprovalRequested {
        session,
        call_id: "call_9".into(),
        name: "run_command".into(),
        arguments: serde_json::json!({"command": command}),
        preview: None,
        run: Some(roost_core::agent::command::RunPreview {
            command: command.into(),
            folder: std::path::PathBuf::from("/home/me/Code/roost"),
            prefix: prefix.map(str::to_string),
            timeout_secs: 120,
            output_kb: 20,
        }),
    });
}

#[test]
fn a_command_asks_with_exactly_what_will_run_where_and_its_limits() {
    use roost_core::chat::ApprovalDecision;
    for size in SIZES {
        for (pick, want) in [
            (0usize, ApprovalDecision::Allow),
            (1, ApprovalDecision::AllowForSession),
            (2, ApprovalDecision::Deny),
        ] {
            let (mut app, sent, s) = agent_fixture();
            let mut h = Harness::new(false);
            ask_to_run(
                &mut app,
                s.id,
                "cargo test -p roost-core",
                Some("cargo test"),
            );
            run(&mut app, &mut h, size, 4);
            let texts = texts_of(&mut app, &mut h, size);
            for want_text in [
                "The agent wants to run a command",
                "cargo test -p roost-core",
                "/home/me/Code/roost",
                "stops after 2 minutes",
                "output kept to 20 KB",
                "your tokens are not passed on",
                "Run it",
                "Allow cargo test this session",
                "Deny",
            ] {
                assert!(texts.contains(want_text), "{size:?}: {want_text}\n{texts}");
            }
            let p = app.state_mut().chat_ui.probe.clone();
            assert_eq!(p.approval_buttons.len(), 3, "{size:?}");
            let composer = p.composer.unwrap();
            for b in &p.approval_buttons {
                assert!(inside(*b, composer), "{size:?}: {b:?} outside {composer:?}");
            }
            let _ = sent_chat_commands(&sent);
            click(&mut app, &mut h, size, p.approval_buttons[pick].center());
            let cmds = sent_chat_commands(&sent);
            assert!(
                matches!(cmds.as_slice(), [ChatCommand::ResolveApproval { session, call_id, decision }]
                    if *session == s.id && call_id == "call_9" && *decision == want),
                "{size:?} pick {pick}: {cmds:?}"
            );
        }
    }
}

#[test]
fn a_command_with_no_safe_prefix_offers_only_run_it_and_deny() {
    use roost_core::chat::ApprovalDecision;
    let size = SIZES[0];
    let (mut app, sent, s) = agent_fixture();
    let mut h = Harness::new(false);
    ask_to_run(&mut app, s.id, "make build && make install", None);
    run(&mut app, &mut h, size, 4);
    let texts = texts_of(&mut app, &mut h, size);
    assert!(!texts.contains("this session"), "{texts}");
    let p = app.state_mut().chat_ui.probe.clone();
    assert_eq!(p.approval_buttons.len(), 2);
    let _ = sent_chat_commands(&sent);
    click(&mut app, &mut h, size, p.approval_buttons[1].center());
    let cmds = sent_chat_commands(&sent);
    assert!(
        matches!(cmds.as_slice(), [ChatCommand::ResolveApproval { decision, .. }]
            if *decision == ApprovalDecision::Deny),
        "{cmds:?}"
    );
}

#[test]
fn a_very_long_command_scrolls_in_its_box_and_never_crowds_out_the_buttons() {
    for size in SIZES {
        let (mut app, _sent, s) = agent_fixture();
        let mut h = Harness::new(false);
        let long = format!("echo {}", "very-long-argument ".repeat(220));
        ask_to_run(&mut app, s.id, &long, None);
        run(&mut app, &mut h, size, 4);
        let p = app.state_mut().chat_ui.probe.clone();
        let composer = p.composer.unwrap();
        assert!(
            composer.height() < size.1 * 0.6,
            "{size:?}: the card is {} tall",
            composer.height()
        );
        for b in &p.approval_buttons {
            assert!(inside(*b, composer), "{size:?}: {b:?} outside {composer:?}");
        }
    }
}

// ------------------------------------------------- the agent asks a question

use roost_core::agent::ask::Answer;

fn ask(app: &mut App, s: &Session, args: serde_json::Value) {
    let question = roost_core::agent::ask::parse(&args).expect("a valid question");
    app.state_mut().chat.fold(ChatEvent::QuestionAsked {
        session: s.id,
        call_id: "call_7".into(),
        question,
    });
}

fn colours() -> serde_json::Value {
    serde_json::json!({"question": "Which colour should the report use?", "kind": "one",
        "options": ["Red", "Green", "Blue"]})
}

fn answers(rx: &Receiver<BackendRequest>) -> Vec<(String, Option<Answer>)> {
    sent_chat_commands(rx)
        .into_iter()
        .filter_map(|c| match c {
            ChatCommand::AnswerQuestion {
                call_id, answer, ..
            } => Some((call_id, answer)),
            _ => None,
        })
        .collect()
}

#[test]
fn a_pick_one_question_sits_above_the_input_and_sends_the_chosen_option() {
    for size in SIZES {
        let (mut app, sent, s) = agent_fixture();
        ask(&mut app, &s, colours());
        let mut h = Harness::new(false);
        run(&mut app, &mut h, size, 4);
        let p = app.state_mut().chat_ui.probe.clone();
        let window = Rect::from_min_size(Pos2::ZERO, vec2(size.0, size.1));
        let card = p.question_card.expect("the card");
        assert!(inside(card, window), "{size:?}: {card:?}");
        assert!(
            inside(card, p.composer.unwrap()),
            "{size:?}: in the composer area"
        );
        assert_eq!(p.question_options.len(), 3, "{size:?}");
        assert!(p.question_other.is_some(), "a typed answer is allowed");
        for o in &p.question_options {
            assert!(inside(*o, card), "{size:?}: {o:?} {card:?}");
        }
        let texts = texts_of(&mut app, &mut h, size);
        assert!(
            texts.contains("Which colour should the report use?"),
            "{texts}"
        );
        // Nothing to send until something is chosen.
        click(&mut app, &mut h, size, p.question_buttons[0].center());
        assert!(answers(&sent).is_empty(), "{size:?}: Send is off");
        click(&mut app, &mut h, size, p.question_options[1].center());
        run(&mut app, &mut h, size, 2);
        let send = app.state_mut().chat_ui.probe.question_buttons[0];
        click(&mut app, &mut h, size, send.center());
        let got = answers(&sent);
        assert_eq!(got.len(), 1, "{size:?}");
        assert_eq!(got[0].0, "call_7");
        assert_eq!(
            got[0].1,
            Some(Answer {
                picked: vec!["Green".into()],
                other: None
            })
        );
        assert!(
            app.state_mut().chat.question(s.id).is_none(),
            "the card goes once answered"
        );
    }
}

#[test]
fn a_pick_many_question_sends_everything_ticked_in_the_order_listed() {
    let size = SIZES[0];
    let (mut app, sent, s) = agent_fixture();
    ask(
        &mut app,
        &s,
        serde_json::json!({"question": "Which files?", "kind": "many",
            "options": ["a.txt", "b.txt", "c.txt"], "allow_other": false}),
    );
    let mut h = Harness::new(false);
    run(&mut app, &mut h, size, 4);
    assert!(
        app.state_mut().chat_ui.probe.question_other.is_none(),
        "allow_other is off"
    );
    let opts = app.state_mut().chat_ui.probe.question_options.clone();
    click(&mut app, &mut h, size, opts[2].center());
    click(&mut app, &mut h, size, opts[0].center());
    run(&mut app, &mut h, size, 2);
    let send = app.state_mut().chat_ui.probe.question_buttons[0];
    click(&mut app, &mut h, size, send.center());
    assert_eq!(
        answers(&sent)[0].1,
        Some(Answer {
            picked: vec!["a.txt".into(), "c.txt".into()],
            other: None
        })
    );
}

#[test]
fn a_typed_answer_goes_through_for_text_and_for_other() {
    use super::view::QuestionDraft;
    let size = SIZES[0];
    for (args, draft_text, want_picked) in [
        (
            serde_json::json!({"question": "Your name?", "kind": "text"}),
            "Ada",
            Vec::<String>::new(),
        ),
        (colours(), "Purple", Vec::new()),
    ] {
        let (mut app, sent, s) = agent_fixture();
        ask(&mut app, &s, args);
        app.state_mut().chat_ui.question_drafts.insert(
            s.id,
            QuestionDraft {
                call_id: "call_7".into(),
                other: draft_text.into(),
                ..Default::default()
            },
        );
        let mut h = Harness::new(false);
        run(&mut app, &mut h, size, 4);
        let send = app.state_mut().chat_ui.probe.question_buttons[0];
        click(&mut app, &mut h, size, send.center());
        assert_eq!(
            answers(&sent)[0].1,
            Some(Answer {
                picked: want_picked,
                other: Some(draft_text.into())
            })
        );
    }
}

#[test]
fn skip_sends_no_answer_and_removes_the_card() {
    let size = SIZES[1];
    let (mut app, sent, s) = agent_fixture();
    ask(&mut app, &s, colours());
    let mut h = Harness::new(false);
    run(&mut app, &mut h, size, 4);
    let skip = app.state_mut().chat_ui.probe.question_buttons[1];
    click(&mut app, &mut h, size, skip.center());
    let got = answers(&sent);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].1, None, "skip is no answer");
    run(&mut app, &mut h, size, 2);
    assert!(app.state_mut().chat_ui.probe.question_card.is_none());
}

#[test]
fn the_question_goes_when_its_tool_finishes_or_the_run_ends() {
    let (mut app, _sent, s) = agent_fixture();
    ask(&mut app, &s, colours());
    assert!(app.state_mut().chat.question(s.id).is_some());
    app.state_mut().chat.fold(ChatEvent::ToolFinished {
        session: s.id,
        call_id: "call_7".into(),
        ok: true,
        output: "{\"answer\":\"Red\"}".into(),
    });
    assert!(app.state_mut().chat.question(s.id).is_none());
    ask(&mut app, &s, colours());
    app.state_mut().chat.fold(ChatEvent::RunEnded {
        session: s.id,
        outcome: roost_core::chat::RunOutcome::Cancelled,
        message: None,
    });
    assert!(app.state_mut().chat.question(s.id).is_none());
}

#[test]
fn an_answered_question_reads_as_words_in_the_transcript() {
    use super::view::ask_view;
    let args = serde_json::json!({"question": "Which?", "kind": "one", "options": ["A","B"]});
    let view = |out: &str| ask_view(&args, &Some(out.to_string())).1;
    assert_eq!(ask_view(&args, &None), ("Which?".to_string(), None));
    assert_eq!(view(r#"{"answer":"B"}"#).as_deref(), Some("B"));
    assert_eq!(view(r#"{"answers":["A","C"]}"#).as_deref(), Some("A, C"));
    assert_eq!(
        view(r#"{"answers":["A"],"other":"also D"}"#).as_deref(),
        Some("A, also D")
    );
    assert_eq!(
        view(roost_core::agent::ask::SKIPPED).as_deref(),
        Some("skipped")
    );
    assert_eq!(view("Error: nope").as_deref(), Some("Error: nope"));
}

// ------------------------------------------------------------ the composer

#[test]
fn the_input_is_one_bubble_with_send_inside_and_the_pills_under_it() {
    use roost_core::session::Permission;
    for size in SIZES {
        let (mut app, _sent, _s) = agent_fixture();
        app.state_mut().settings.agents_can_run = true;
        let mut h = Harness::new(false);
        run(&mut app, &mut h, size, 4);
        let p = app.state_mut().chat_ui.probe.clone();
        let (composer, input) = (p.composer.unwrap(), p.input_rect.unwrap());
        let (send, label) = p.send_button.unwrap();
        assert_eq!(label, "Send", "{size:?}");
        assert!(inside(send, input), "{size:?}: send {send:?} in {input:?}");
        assert!(input.height() < 56.0, "{size:?}: one line high: {input:?}");
        let (plus, tools) = (p.plus_button.unwrap(), p.agent_chip.unwrap());
        let (folder, model) = (p.folder_pill.expect("folder"), p.model_chip.unwrap());
        for r in [plus, tools, folder, model] {
            assert!(
                r.top() >= input.bottom() - 0.5,
                "{size:?}: {r:?} under input"
            );
            assert!(inside(r, composer), "{size:?}: {r:?} in {composer:?}");
        }
        assert!(plus.right() <= tools.left(), "{size:?}");
        assert!(tools.right() <= folder.left(), "{size:?}");
        let letters: Vec<_> = p.chips.iter().map(|c| c.0).collect();
        assert_eq!(letters, ['R', 'W', 'X', 'N'], "{size:?}");
        assert!(
            p.chips[0].1.left() >= folder.right(),
            "{size:?}: chips right of folder"
        );
        assert!(
            p.chips[2].1.right() <= model.left(),
            "{size:?}: model is last"
        );
        assert!(model.right() <= input.right() + 0.5, "{size:?}");
        assert_eq!(p.chips[2].2, Permission::Off, "{size:?}");
    }
}

#[test]
fn the_input_grows_with_its_text_up_to_a_limit_and_send_stays_in_the_corner() {
    let size = (1180.0, 820.0);
    let mut f = fixture();
    let mut h = Harness::new(false);
    let id = f.open.id;
    run(&mut f.app, &mut h, size, 4);
    let one = f.app.state_mut().chat_ui.probe.input_rect.unwrap();
    *f.app.state_mut().chat.draft_mut(id) = "a\nb\nc\nd".into();
    run(&mut f.app, &mut h, size, 4);
    let four = f.app.state_mut().chat_ui.probe.input_rect.unwrap();
    assert!(four.height() > one.height() + 30.0, "{one:?} then {four:?}");
    *f.app.state_mut().chat.draft_mut(id) = "line\n".repeat(80);
    run(&mut f.app, &mut h, size, 4);
    let p = f.app.state_mut().chat_ui.probe.clone();
    let many = p.input_rect.unwrap();
    assert!(many.height() < 200.0, "capped, then it scrolls: {many:?}");
    let (send, _) = p.send_button.unwrap();
    assert!(inside(send, many), "{send:?} in {many:?}");
    assert!(
        many.bottom() - send.bottom() < 8.0,
        "send stays at the bottom right"
    );
    assert!(p.composer.unwrap().bottom() <= size.1, "inside the window");
}

#[test]
fn a_chat_has_no_agent_pills_under_its_input() {
    let size = (1180.0, 820.0);
    let mut f = fixture();
    let mut h = Harness::new(false);
    run(&mut f.app, &mut h, size, 4);
    let p = f.app.state_mut().chat_ui.probe.clone();
    assert!(p.plus_button.is_some() && p.model_chip.is_some());
    assert!(p.agent_chip.is_none() && p.folder_pill.is_none() && p.chips.is_empty());
}

#[test]
fn the_plus_menu_offers_the_folder_to_an_agent_and_nothing_yet_to_a_chat() {
    let size = (1180.0, 820.0);
    let (mut app, _sent, _s) = agent_fixture();
    let mut h = Harness::new(false);
    run(&mut app, &mut h, size, 4);
    let plus = app.state_mut().chat_ui.probe.plus_button.unwrap();
    click(&mut app, &mut h, size, plus.center());
    let texts = texts_of(&mut app, &mut h, size);
    assert!(texts.contains("Working folder…"), "{texts}");
    let mut f = fixture();
    let mut h = Harness::new(false);
    run(&mut f.app, &mut h, size, 4);
    let plus = f.app.state_mut().chat_ui.probe.plus_button.unwrap();
    click(&mut f.app, &mut h, size, plus.center());
    let texts = texts_of(&mut f.app, &mut h, size);
    assert!(texts.contains("Nothing to add to a chat yet"), "{texts}");
}

#[test]
fn clicking_a_permission_pill_steps_it_and_running_commands_never_steps_to_allow() {
    use roost_core::agent::registry::{self, Capability, Family};
    use roost_core::session::Permission;
    let size = (1180.0, 820.0);
    let (mut app, sent, s) = agent_fixture();
    app.state_mut().settings.agents_can_run = true;
    let mut h = Harness::new(false);
    run(&mut app, &mut h, size, 4);
    let chips = app.state_mut().chat_ui.probe.chips.clone();
    let _ = sent_chat_commands(&sent);
    // R is allowed, W asks, X is off: a click steps each one on.
    for (i, (cap, family, want)) in [
        (Capability::Read, Family::Files, Permission::Off),
        (Capability::Write, Family::Files, Permission::Auto),
        (Capability::Run, Family::Shell, Permission::Ask),
    ]
    .into_iter()
    .enumerate()
    {
        click(&mut app, &mut h, size, chips[i].1.center());
        let cmds = sent_chat_commands(&sent);
        let key = registry::capability_key(family, cap);
        assert!(
            matches!(cmds.as_slice(), [ChatCommand::SetCapability(id, k, p)]
                if *id == s.id && *k == key && *p == want),
            "{cap:?}: {cmds:?}"
        );
    }
}

#[test]
fn the_permission_steps_are_off_ask_allow_and_commands_skip_allow() {
    use super::chrome::{held_long, next_permission};
    use roost_core::agent::registry::Capability;
    use roost_core::session::Permission::{Ask, Auto, Off};
    for cap in [Capability::Read, Capability::Write] {
        assert_eq!(next_permission(cap, Off), Ask);
        assert_eq!(next_permission(cap, Ask), Auto);
        assert_eq!(next_permission(cap, Auto), Off);
    }
    assert_eq!(next_permission(Capability::Run, Off), Ask);
    assert_eq!(next_permission(Capability::Run, Ask), Off);
    assert_eq!(next_permission(Capability::Run, Auto), Off);
    assert!(!held_long(None, 10.0));
    assert!(!held_long(Some(9.8), 10.0));
    assert!(held_long(Some(9.4), 10.0));
}

#[test]
fn a_long_press_on_x_asks_once_above_the_input_and_the_menu_does_not_ask_again() {
    use roost_core::session::Permission;
    let size = (1180.0, 820.0);
    let (mut app, sent, s) = agent_fixture();
    app.state_mut().settings.agents_can_run = true;
    let mut h = Harness::new(false);
    app.state_mut().chat_ui.run_allow_pending = Some(s.id);
    app.state_mut().chat_ui.run_allow_from_pill = true;
    run(&mut app, &mut h, size, 4);
    let p = app.state_mut().chat_ui.probe.clone();
    let (confirm, _cancel) = p.run_confirm.expect("the confirmation above the input");
    assert!(
        confirm.bottom() <= p.input_rect.unwrap().top(),
        "above the input"
    );
    let _ = sent_chat_commands(&sent);
    click(&mut app, &mut h, size, confirm.center());
    let cmds = sent_chat_commands(&sent);
    assert!(
        matches!(cmds.as_slice(), [ChatCommand::SetCapability(id, key, Permission::Auto)]
            if *id == s.id && key == "shell.run"),
        "{cmds:?}"
    );
}

#[test]
fn the_two_new_buttons_sit_beside_search_and_start_a_chat_or_an_incognito_one() {
    let size = (1180.0, 820.0);
    let (mut app, sent, _s) = agent_fixture();
    {
        let st = app.state_mut();
        st.installed
            .on_installed_loaded(&Ok(vec![crate::runtime::InstalledEntry {
                model: roost_core::ollama::InstalledModel {
                    name: "qwen:7b".into(),
                    size: 1,
                    params: None,
                    quant: None,
                    family: None,
                    modified: String::new(),
                    digest: String::new(),
                },
                source_repo: None,
            }]));
        st.view = View::Chat;
    }
    let mut h = Harness::new(false);
    run(&mut app, &mut h, size, 4);
    let p = app.state_mut().chat_ui.probe.clone();
    let (search, new, hidden) = (
        p.rail_search.unwrap(),
        p.new_button.unwrap(),
        p.incognito_button.unwrap(),
    );
    assert!(search.right() <= new.left() && new.right() <= hidden.left());
    assert!(inside(hidden, p.rail.unwrap()), "{hidden:?} in the rail");
    let _ = sent_chat_commands(&sent);
    click(&mut app, &mut h, size, new.center());
    let cmds = sent_chat_commands(&sent);
    assert!(
        matches!(
            cmds.as_slice(),
            [ChatCommand::Create {
                lifetime: SessionLifetime::Persisted,
                ..
            }]
        ),
        "{cmds:?}"
    );
    click(&mut app, &mut h, size, hidden.center());
    let cmds = sent_chat_commands(&sent);
    assert!(
        matches!(
            cmds.as_slice(),
            [ChatCommand::Create {
                lifetime: SessionLifetime::Incognito,
                ..
            }]
        ),
        "{cmds:?}"
    );
}

#[test]
fn the_send_button_is_the_same_distance_from_the_top_bottom_and_right_edges() {
    for size in SIZES {
        let mut f = fixture();
        let mut h = Harness::new(false);
        run(&mut f.app, &mut h, size, 4);
        let p = f.app.state_mut().chat_ui.probe.clone();
        let input = p.input_rect.unwrap();
        let (send, _) = p.send_button.unwrap();
        let (top, bottom, right) = (
            send.top() - input.top(),
            input.bottom() - send.bottom(),
            input.right() - send.right(),
        );
        assert!(
            (top - bottom).abs() <= 1.0 && (right - bottom).abs() <= 1.0,
            "{size:?}: top {top}, bottom {bottom}, right {right}"
        );
    }
}

#[test]
fn the_composer_is_compact_below_the_bubble() {
    for size in SIZES {
        let mut f = fixture();
        let mut h = Harness::new(false);
        run(&mut f.app, &mut h, size, 4);
        let p = f.app.state_mut().chat_ui.probe.clone();
        let (input, composer, chip) = (
            p.input_rect.unwrap(),
            p.composer.unwrap(),
            p.model_chip.unwrap(),
        );
        assert!(
            chip.height() <= 27.0,
            "{size:?}: the model chip is as tall as a pill: {chip:?}"
        );
        assert!(
            composer.bottom() - input.bottom() <= 42.0,
            "{size:?}: pills and margin under the bubble: {input:?} in {composer:?}"
        );
    }
}

// ------------------------------------------------------------ richer rail rows

fn named_row(s: &Session, title: &str, preview: &str, last: &str) -> IndexRow {
    let mut r = row_for(s, title);
    r.preview = Some(preview.into());
    r.last = Some(last.into());
    r.updated_at_ms = now_ms();
    r
}

#[test]
fn chat_rows_show_the_last_message_a_quiet_new_chat_and_the_incognito_mark() {
    for size in SIZES {
        let mut f = fixture_with(0);
        let mut h = Harness::new(false);
        let said = Session::new_chat("qwen2.5:7b", SessionLifetime::Persisted, now_ms());
        let fresh = Session::new_chat("qwen2.5:7b", SessionLifetime::Persisted, now_ms());
        let hidden = Session::new_chat("qwen2.5:7b", SessionLifetime::Incognito, now_ms());
        let rows = vec![
            named_row(&said, "Planning", "what next", "the answer is here"),
            named_row(&fresh, "Untitled", "", ""),
            named_row(&hidden, "Draft a polite no", "hmm", "ok"),
        ];
        f.app.state_mut().chat.fold(ChatEvent::Rows(rows));
        run(&mut f.app, &mut h, size, 4);
        let texts = texts_of(&mut f.app, &mut h, size);
        for want in [
            "Planning",
            "the answer is here",
            "New chat",
            "No messages yet",
            "Incognito · not saved",
        ] {
            assert!(texts.contains(want), "{size:?}: {want}\n{texts}");
        }
        assert!(
            !texts.lines().any(|l| l == "Untitled"),
            "{size:?}: an empty chat is not called Untitled"
        );
        // Every row stays inside the rail.
        let p = f.app.state_mut().chat_ui.probe.clone();
        let rail = p.rail.unwrap();
        for (_, r) in &p.rows {
            assert!(inside(*r, rail), "{size:?}: {r:?} in {rail:?}");
        }
    }
}

#[test]
fn an_agent_row_shows_its_folder_and_a_working_or_needs_you_badge() {
    for size in SIZES {
        let (mut app, _sent, s) = agent_fixture();
        let mut row = named_row(&s, "Games library", "scan it", "I found 12 folders");
        row.folder = Some("/work/a-project".into());
        app.state_mut().chat.fold(ChatEvent::Rows(vec![row]));
        let mut h = Harness::new(false);
        run(&mut app, &mut h, size, 4);
        let texts = texts_of(&mut app, &mut h, size);
        assert!(texts.contains("/work/a-project"), "{size:?}\n{texts}");
        assert!(!texts.contains("Working") && !texts.contains("Needs you"));
        app.state_mut().chat.fold(ChatEvent::TurnStarted {
            session: s.id,
            turn: TurnId::new(),
        });
        run(&mut app, &mut h, size, 3);
        let texts = texts_of(&mut app, &mut h, size);
        assert!(texts.contains("Working"), "{size:?}\n{texts}");
        app.state_mut().chat.fold(ChatEvent::ApprovalRequested {
            session: s.id,
            call_id: "c1".into(),
            name: "read_file".into(),
            arguments: serde_json::json!({"path": "a"}),
            preview: None,
            run: None,
        });
        run(&mut app, &mut h, size, 3);
        let texts = texts_of(&mut app, &mut h, size);
        assert!(texts.contains("Needs you"), "{size:?}\n{texts}");
    }
}

// ------------------------------------------------------ how a run ended

fn end_the_run_with(app: &mut App, end: roost_core::session::RunEnd, behind: usize) {
    let open = app.state_mut().chat.open.as_mut().unwrap();
    let after = open.messages.len().saturating_sub(behind);
    open.last_run_end = Some(roost_core::session::RunEndNote { end, after });
}

#[test]
fn a_run_that_hit_its_step_limit_says_so_and_offers_a_bigger_budget() {
    use roost_core::session::RunEnd;
    for size in SIZES {
        let (mut app, sent, s) = agent_fixture();
        end_the_run_with(&mut app, RunEnd::StepLimit { steps: 20 }, 0);
        let mut h = Harness::new(false);
        run(&mut app, &mut h, size, 4);
        let texts = texts_of(&mut app, &mut h, size);
        for want in [
            "Stopped after using all 20 steps",
            "It was still working",
            "Steps",
            "Continue",
        ] {
            assert!(texts.contains(want), "{size:?}: {want}\n{texts}");
        }
        let p = app.state_mut().chat_ui.probe.clone();
        let (banner, go, steps) = (
            p.run_banner.expect("banner"),
            p.run_continue.expect("continue"),
            p.run_steps.expect("steps box"),
        );
        assert!(inside(banner, p.pane.unwrap()), "{size:?}: {banner:?}");
        // The text has the width; the controls sit below it at the bottom right.
        assert!(banner.width() > 300.0, "{size:?}: {banner:?}");
        assert!(go.top() > banner.top() + 30.0, "{size:?}: below the text");
        assert!(go.right() <= banner.right() && go.bottom() <= banner.bottom());
        assert!(
            steps.right() <= go.left(),
            "{size:?}: steps beside Continue"
        );
        // Continue sets the budget (20 + 30 = 50) and then sends "Continue".
        let _ = sent_chat_commands(&sent);
        click(&mut app, &mut h, size, go.center());
        let cmds = sent_chat_commands(&sent);
        assert!(
            matches!(cmds.as_slice(),
                [ChatCommand::SetStepBudget(a, 50), ChatCommand::Send { session, text }]
                    if *a == s.id && *session == s.id && text == "Continue"),
            "{size:?}: {cmds:?}"
        );
    }
}

#[test]
fn stopped_failed_and_interrupted_runs_each_have_their_words_and_a_button() {
    use roost_core::session::RunEnd;
    let size = (1180.0, 820.0);
    let cases = [
        (RunEnd::Stopped, "You stopped this run", "Continue", false),
        (
            RunEnd::Failed {
                message: "Ollama stopped responding.".into(),
            },
            "The run failed",
            "Try again",
            false,
        ),
        (
            RunEnd::Interrupted,
            "Roost was closed during this run",
            "Continue",
            false,
        ),
    ];
    for (end, headline, button, has_steps) in cases {
        let (mut app, sent, s) = agent_fixture();
        let failed = matches!(end, RunEnd::Failed { .. });
        end_the_run_with(&mut app, end, 0);
        let mut h = Harness::new(false);
        run(&mut app, &mut h, size, 4);
        let texts = texts_of(&mut app, &mut h, size);
        assert!(texts.contains(headline), "{headline}\n{texts}");
        assert!(texts.contains(button), "{button}\n{texts}");
        if failed {
            assert!(texts.contains("Ollama stopped responding."), "{texts}");
        }
        let p = app.state_mut().chat_ui.probe.clone();
        assert_eq!(p.run_steps.is_some(), has_steps, "{headline}");
        let _ = sent_chat_commands(&sent);
        click(&mut app, &mut h, size, p.run_continue.unwrap().center());
        let cmds = sent_chat_commands(&sent);
        assert!(
            matches!(cmds.as_slice(), [ChatCommand::Send { session, text }]
                if *session == s.id && text == "Continue"),
            "{headline}: {cmds:?}"
        );
    }
}

#[test]
fn once_the_conversation_moves_on_the_banner_is_a_quiet_line_without_buttons() {
    use roost_core::session::RunEnd;
    let size = (1180.0, 820.0);
    let (mut app, _sent, _s) = agent_fixture();
    // The run ended before the last message of the chat.
    end_the_run_with(&mut app, RunEnd::StepLimit { steps: 20 }, 1);
    let mut h = Harness::new(false);
    run(&mut app, &mut h, size, 4);
    let texts = texts_of(&mut app, &mut h, size);
    assert!(
        texts.contains("Stopped after using all 20 steps."),
        "{texts}"
    );
    let p = app.state_mut().chat_ui.probe.clone();
    assert!(p.run_marker.is_some(), "the marker");
    assert!(
        p.run_banner.is_none() && p.run_continue.is_none(),
        "no buttons"
    );
    assert!(
        !texts.contains("Continue gives it a fresh budget"),
        "{texts}"
    );
}

#[test]
fn no_banner_while_a_run_is_going_and_none_in_a_chat() {
    use roost_core::session::RunEnd;
    let size = (1180.0, 820.0);
    let (mut app, _sent, s) = agent_fixture();
    end_the_run_with(&mut app, RunEnd::Stopped, 0);
    app.state_mut().chat.fold(ChatEvent::TurnStarted {
        session: s.id,
        turn: TurnId::new(),
    });
    let mut h = Harness::new(false);
    run(&mut app, &mut h, size, 4);
    assert!(app.state_mut().chat_ui.probe.run_banner.is_none());
    // A plain chat never shows one, even if the field were set.
    let mut f = fixture();
    f.app.state_mut().chat.open.as_mut().unwrap().last_run_end =
        Some(roost_core::session::RunEndNote {
            end: RunEnd::Stopped,
            after: 0,
        });
    let mut h = Harness::new(false);
    run(&mut f.app, &mut h, size, 4);
    let p = f.app.state_mut().chat_ui.probe.clone();
    assert!(p.run_banner.is_none() && p.run_marker.is_none());
}

#[test]
fn a_tool_call_written_as_text_gets_a_banner_without_a_button() {
    use roost_core::session::RunEnd;
    for size in SIZES {
        let (mut app, _sent, _s) = agent_fixture();
        end_the_run_with(
            &mut app,
            RunEnd::TextToolCall {
                tool: "list_dir".into(),
            },
            0,
        );
        let mut h = Harness::new(false);
        run(&mut app, &mut h, size, 4);
        let texts = texts_of(&mut app, &mut h, size);
        for want in [
            "The reply is a tool call written as text",
            "Nothing ran (list_dir)",
            "try another model",
        ] {
            assert!(texts.contains(want), "{size:?}: {want}\n{texts}");
        }
        let p = app.state_mut().chat_ui.probe.clone();
        let banner = p.run_banner.expect("a banner");
        assert!(inside(banner, p.pane.unwrap()), "{size:?}: {banner:?}");
        assert!(
            p.run_continue.is_none() && p.run_steps.is_none(),
            "{size:?}: nothing to continue"
        );
    }
}

// ------------------------------------------------------ a flat sidebar

/// Every shape drawn in one frame, with groups flattened.
fn painted(app: &mut App, h: &mut Harness, size: (f32, f32)) -> Vec<egui::epaint::Shape> {
    fn flat(shape: &egui::epaint::Shape, into: &mut Vec<egui::epaint::Shape>) {
        match shape {
            egui::epaint::Shape::Vec(items) => items.iter().for_each(|s| flat(s, into)),
            other => into.push(other.clone()),
        }
    }
    let input = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(size.0, size.1))),
        ..Default::default()
    };
    let out = h.ctx().run_ui(input, |ui| app.show(ui));
    let mut shapes = Vec::new();
    for clipped in &out.shapes {
        flat(&clipped.shape, &mut shapes);
    }
    out.drop_without_applying_deltas();
    shapes
}

#[test]
fn the_sidebar_has_no_outline_to_double_up_with_the_bars_and_one_divider_on_its_right() {
    for size in SIZES {
        let mut f = fixture();
        let mut h = Harness::new(false);
        run(&mut f.app, &mut h, size, 4);
        // The panel is the rail plus the frame's 12 px margin on every side.
        let rail = f.app.state_mut().chat_ui.probe.rail.unwrap();
        let panel = rail.expand(12.0);
        let shapes = painted(&mut f.app, &mut h, size);
        let outlined = shapes.iter().any(|s| match s {
            egui::epaint::Shape::Rect(r) => {
                r.stroke.width > 0.0
                    && r.stroke.color.a() > 0
                    && (r.rect.min - panel.min).length() < 2.0
                    && (r.rect.max - panel.max).length() < 2.0
            }
            _ => false,
        });
        assert!(!outlined, "{size:?}: the sidebar draws its own outline");
        // One vertical divider along its right edge.
        let divider = shapes.iter().any(|s| match s {
            egui::epaint::Shape::LineSegment { points, stroke } => {
                stroke.width > 0.0
                    && (points[0].x - panel.right()).abs() < 2.0
                    && (points[1].x - panel.right()).abs() < 2.0
                    && (points[1].y - points[0].y).abs() > panel.height() * 0.5
            }
            _ => false,
        });
        assert!(divider, "{size:?}: no divider on the sidebar's right edge");
    }
}

#[test]
fn no_two_lines_run_side_by_side_across_the_window() {
    // A bar's border next to another bar's, or next to the sidebar's, read as
    // one thick double line. Every long horizontal line is at least 4 px from
    // the next one.
    for size in SIZES {
        let mut f = fixture();
        let mut h = Harness::new(false);
        run(&mut f.app, &mut h, size, 4);
        let mut ys: Vec<f32> = Vec::new();
        for shape in painted(&mut f.app, &mut h, size) {
            match shape {
                egui::epaint::Shape::LineSegment { points, stroke }
                    if stroke.width > 0.0
                        && (points[1].y - points[0].y).abs() < 0.5
                        && (points[1].x - points[0].x).abs() > size.0 * 0.6 =>
                {
                    ys.push(points[0].y);
                }
                // The top and bottom edges of an outlined bar count as lines.
                egui::epaint::Shape::Rect(r)
                    if r.stroke.width > 0.0
                        && r.stroke.color.a() > 0
                        && r.rect.width() > size.0 * 0.6 =>
                {
                    ys.push(r.rect.top());
                    ys.push(r.rect.bottom());
                }
                _ => {}
            }
        }
        ys.sort_by(f32::total_cmp);
        assert!(
            ys.len() >= 2,
            "{size:?}: the bars still have their lines: {ys:?}"
        );
        for pair in ys.windows(2) {
            assert!(
                pair[1] - pair[0] >= 4.0,
                "{size:?}: doubled lines at {pair:?}"
            );
        }
    }
}

// ------------------------------------------------------ a tinted sidebar

fn close(a: egui::Color32, b: egui::Color32) -> bool {
    a.a() > 200
        && (a.r() as i32 - b.r() as i32).abs() < 4
        && (a.g() as i32 - b.g() as i32).abs() < 4
        && (a.b() as i32 - b.b() as i32).abs() < 4
}

#[test]
fn the_sidebar_list_is_half_tinted_with_a_full_tint_band_a_white_selection_and_a_dark_divider() {
    let t = crate::theme_tokens::palette(roost_core::config::ThemeFamily::Meadow, false);
    for size in SIZES {
        let mut f = fixture();
        let mut h = Harness::new(false);
        run(&mut f.app, &mut h, size, 4);
        let p = f.app.state_mut().chat_ui.probe.clone();
        let panel = p.rail.unwrap().expand(12.0);
        let row = p
            .rows
            .iter()
            .find(|(id, _)| *id == f.open.id)
            .map(|(_, r)| *r)
            .expect("the open chat's row");
        let shapes = painted(&mut f.app, &mut h, size);
        let rects = || {
            shapes.iter().filter_map(|s| match s {
                egui::epaint::Shape::Rect(r) => Some((r.rect, r.fill)),
                _ => None,
            })
        };
        // The list behind everything.
        assert!(
            rects().any(|(r, c)| close(c, t.side_list())
                && (r.min - panel.min).length() < 2.0
                && (r.max - panel.max).length() < 2.0),
            "{size:?}: the list is not in the half tint"
        );
        // The band behind the search field and buttons: the full tint, from
        // the top of the sidebar, edge to edge.
        let band = rects()
            .find(|(r, c)| {
                close(*c, t.side_band())
                    && (r.left() - panel.left()).abs() < 2.0
                    && (r.right() - panel.right()).abs() < 2.0
                    && (r.top() - panel.top()).abs() < 2.0
            })
            .map(|(r, _)| r)
            .expect("a band of the full tint");
        let search = p.rail_search.unwrap();
        assert!(
            band.height() > 40.0 && band.height() < 90.0 && band.bottom() >= search.bottom(),
            "{size:?}: the band holds the search row: {band:?} {search:?}"
        );
        // The selected row is white with a dark 3 px edge on its left.
        assert!(
            rects().any(|(r, c)| close(c, t.surface)
                && (r.min - row.min).length() < 1.5
                && (r.max - row.max).length() < 1.5),
            "{size:?}: the selected row is not white"
        );
        assert!(
            rects().any(|(r, c)| close(c, t.accent)
                && (r.width() - 3.0).abs() < 0.5
                && (r.left() - row.left()).abs() < 1.5),
            "{size:?}: no edge on the selected row"
        );
        // One divider on the right in the darker tint.
        let divider = shapes.iter().any(|s| match s {
            egui::epaint::Shape::LineSegment { points, stroke } => {
                close(stroke.color, t.side_edge())
                    && (points[0].x - panel.right()).abs() < 2.0
                    && (points[1].y - points[0].y).abs() > panel.height() * 0.5
            }
            _ => false,
        });
        assert!(divider, "{size:?}: no dark divider on the right");
    }
}

#[test]
fn the_page_header_is_as_tall_as_the_sidebars_band_but_in_a_deeper_tint() {
    for size in SIZES {
        let mut f = fixture();
        let mut h = Harness::new(false);
        run(&mut f.app, &mut h, size, 4);
        let t = crate::theme_tokens::palette(roost_core::config::ThemeFamily::Meadow, false);
        let p = f.app.state_mut().chat_ui.probe.clone();
        let header = p.header.unwrap();
        let panel = p.rail.unwrap().expand(12.0);
        let shapes = painted(&mut f.app, &mut h, size);
        let rects = || {
            shapes.iter().filter_map(|s| match s {
                egui::epaint::Shape::Rect(r) => Some((r.rect, r.fill)),
                _ => None,
            })
        };
        let side = rects()
            .find(|(r, c)| {
                close(*c, t.side_band())
                    && (r.left() - panel.left()).abs() < 2.0
                    && (r.top() - panel.top()).abs() < 2.0
            })
            .map(|(r, _)| r)
            .expect("the sidebar's band");
        assert!(
            (side.top() - header.top()).abs() < 1.0
                && (side.bottom() - header.bottom()).abs() < 1.0,
            "{size:?}: sidebar band {side:?} and header {header:?} do not line up"
        );
        assert!(
            rects().any(|(r, c)| close(c, t.head_band()) && (r.min - header.min).length() < 1.0),
            "{size:?}: the header is not in the deeper tint"
        );
        assert!(
            !close(t.head_band(), t.side_band()),
            "{size:?}: the header and the sidebar band must differ"
        );
    }
}

#[test]
fn a_reply_that_only_thought_is_a_line_not_an_empty_card() {
    let size = (1180.0, 820.0);
    let mut f = fixture_with(1);
    let mut h = Harness::new(false);
    let now = now_ms();
    let mut s = Session::new_chat("llama3.2:3b", SessionLifetime::Persisted, now);
    s.push_user("Hello", now);
    let a = s.begin_assistant(RunId::new(), TurnId::new(), now);
    s.append_assistant(a, "", "Let me think about that.");
    s.finish_assistant(a, AssistantStatus::Complete, None, now);
    let b = s.begin_assistant(RunId::new(), TurnId::new(), now);
    s.append_assistant(b, "An answer.", "");
    s.finish_assistant(b, AssistantStatus::Complete, None, now);
    f.app.state_mut().chat.fold(ChatEvent::Opened(Box::new(s)));
    run(&mut f.app, &mut h, size, 4);
    let cards = f.app.state_mut().chat_ui.probe.cards.clone();
    assert_eq!(cards.len(), 3, "{cards:?}");
    assert!(
        cards[1].height() < 34.0 && cards[0].height() > 40.0 && cards[2].height() > 40.0,
        "the thinking-only reply is one line: {cards:?}"
    );
    let gap = cards[2].top() - cards[1].bottom();
    assert!(gap < 24.0, "no empty row of actions under it: gap {gap}");
}

#[test]
fn the_permission_pills_keep_one_colour_and_change_shape_with_what_they_allow() {
    let size = (1180.0, 820.0);
    let (mut app, _sent, _s) = agent_fixture();
    let mut h = Harness::new(false);
    run(&mut app, &mut h, size, 4);
    let t = crate::theme_tokens::palette(roost_core::config::ThemeFamily::Meadow, false);
    let chips = app.state_mut().chat_ui.probe.chips.clone();
    let shapes = painted(&mut app, &mut h, size);
    // The smallest rectangle that holds the pill is the pill itself.
    let rect_of = |chip: egui::Rect| {
        shapes
            .iter()
            .filter_map(|s| match s {
                egui::epaint::Shape::Rect(r)
                    if r.fill.a() > 0 && r.rect.contains_rect(chip.shrink(2.0)) =>
                {
                    Some(r.clone())
                }
                _ => None,
            })
            .min_by(|a, b| a.rect.area().total_cmp(&b.rect.area()))
    };
    // Read is allowed: solid in the theme colour. Write asks: the same colour
    // as an outline on the plain surface.
    let read = rect_of(chips[0].1).expect("the read pill");
    assert!(close(read.fill, t.accent), "read is solid: {:?}", read.fill);
    let write_fill = shapes.iter().any(|s| match s {
        egui::epaint::Shape::Rect(r) => {
            close(r.fill, t.accent) && r.rect.contains_rect(chips[1].1.shrink(2.0))
        }
        _ => false,
    });
    assert!(!write_fill, "write asks, so it is not solid");
    let outlined = shapes.iter().any(|s| match s {
        egui::epaint::Shape::Rect(r) => {
            close(r.stroke.color, t.accent)
                && r.stroke.width > 0.0
                && chips[1].1.expand(1.0).contains_rect(r.rect)
        }
        _ => false,
    });
    assert!(outlined, "write has an outline in the theme colour");
}

#[test]
fn the_header_buttons_and_title_are_centred_in_the_band() {
    for size in SIZES {
        let (mut app, _sent, _s) = agent_fixture();
        let mut h = Harness::new(false);
        run(&mut app, &mut h, size, 4);
        let p = app.state_mut().chat_ui.probe.clone();
        let header = p.header.unwrap();
        let trash = p.delete_button.clone().unwrap().0;
        let (title, path) = (p.title_rect.unwrap(), p.path_rect.unwrap());
        let block = title.union(path);
        // The buttons are close to the band's right edge, not a pane margin away.
        let side = header.right() - trash.right();
        assert!(
            (side - 10.0).abs() <= 1.0,
            "{size:?}: {side} from the right edge"
        );
        for (name, r) in [("buttons", trash), ("title and folder", block)] {
            let (above, below) = (r.top() - header.top(), header.bottom() - r.bottom());
            assert!(
                (above - below).abs() <= 3.0 && above >= 6.0,
                "{size:?}: {name} sit {above} below the top and {below} above the bottom of {header:?}"
            );
        }
    }
}

#[test]
fn the_session_kind_glyph_is_larger_than_the_header_buttons_icons() {
    for size in SIZES {
        let (mut app, _sent, _s) = agent_fixture();
        let mut h = Harness::new(false);
        run(&mut app, &mut h, size, 4);
        let p = app.state_mut().chat_ui.probe.clone();
        let (header, title) = (p.header.unwrap(), p.title_rect.unwrap());
        let shapes = painted(&mut app, &mut h, size);
        // The bubble is the largest line drawing left of the title.
        let widest = shapes
            .iter()
            .filter_map(|s| match s {
                egui::epaint::Shape::Path(path) => {
                    let r = Rect::from_points(&path.points);
                    (r.right() <= title.left()
                        && r.top() >= header.top()
                        && r.bottom() <= header.bottom())
                    .then_some(r.width())
                }
                _ => None,
            })
            .fold(0.0_f32, f32::max);
        assert!(widest >= 17.0, "{size:?}: the glyph is {widest} px wide");
        // And its box is bigger than the buttons beside the title.
        let tile = shapes
            .iter()
            .filter_map(|s| match s {
                egui::epaint::Shape::Rect(r)
                    if r.rect.right() <= title.left()
                        && r.rect.left() >= header.left()
                        && r.rect.width() < 60.0
                        && r.stroke.width > 0.0 =>
                {
                    Some((r.rect.width(), r.stroke.color))
                }
                _ => None,
            })
            .fold((0.0_f32, egui::Color32::TRANSPARENT), |a, b| {
                if b.0 > a.0 {
                    b
                } else {
                    a
                }
            });
        let (tile, edge) = tile;
        let want = crate::theme_tokens::palette(roost_core::config::ThemeFamily::Meadow, false)
            .side_edge();
        assert!(close(edge, want), "{size:?}: the tile's edge is {edge:?}");
        let button = p.delete_button.clone().unwrap().0.width();
        assert!(
            tile >= button + 6.0,
            "{size:?}: the tile is {tile} px, the buttons {button} px"
        );
    }
}

#[test]
fn the_landing_text_is_centred_in_the_pane_on_chat_and_on_work() {
    for size in SIZES {
        for view in [View::Chat, View::Work] {
            let mut f = fixture_with(0);
            f.app.state_mut().chat.open = None;
            f.app.state_mut().view = view;
            let mut h = Harness::new(false);
            run(&mut f.app, &mut h, size, 4);
            let p = f.app.state_mut().chat_ui.probe.clone();
            let landing = p.landing.expect("the landing panel");
            let rail = p.rail.unwrap().expand(12.0);
            let pane = Rect::from_min_max(
                egui::pos2(rail.right(), rail.top()),
                egui::pos2(size.0, size.1),
            );
            assert!(
                (landing.center().x - pane.center().x).abs() <= 12.0,
                "{size:?} {view:?}: {landing:?} is not centred across {pane:?}"
            );
            let middle = landing.center().y;
            assert!(
                middle > pane.top() + pane.height() * 0.25
                    && middle < pane.top() + pane.height() * 0.6,
                "{size:?} {view:?}: {landing:?} is not about the middle of {pane:?}"
            );
        }
    }
}

#[test]
fn the_web_pill_is_a_globe_that_starts_off_and_only_steps_between_off_and_ask() {
    use super::chrome::next_permission;
    use roost_core::agent::registry::{self, Capability, Family};
    use roost_core::session::Permission::{self, Ask, Auto, Off};
    // Never Allow from the pill, whatever the state.
    assert_eq!(next_permission(Capability::Fetch, Off), Ask);
    assert_eq!(next_permission(Capability::Fetch, Ask), Off);
    assert_eq!(next_permission(Capability::Fetch, Auto), Off);
    for size in SIZES {
        let (mut app, sent, s) = agent_fixture();
        let mut h = Harness::new(false);
        run(&mut app, &mut h, size, 4);
        let p = app.state_mut().chat_ui.probe.clone();
        let (letter, rect, perm) = *p.chips.last().expect("a pill");
        assert_eq!(
            (letter, perm),
            ('N', Permission::Off),
            "{size:?}: off by default"
        );
        assert!(
            inside(rect, p.composer.unwrap()),
            "{size:?}: the web pill {rect:?} is inside the composer"
        );
        // It is an icon, not a letter.
        let texts = texts_of(&mut app, &mut h, size);
        assert!(!texts.lines().any(|l| l == "N"), "{size:?}: {texts}");
        let _ = sent_chat_commands(&sent);
        click(&mut app, &mut h, size, rect.center());
        let cmds = sent_chat_commands(&sent);
        let key = registry::capability_key(Family::Web, Capability::Fetch);
        assert!(
            matches!(cmds.as_slice(), [ChatCommand::SetCapability(id, k, Ask)]
                if *id == s.id && *k == key),
            "{size:?}: {cmds:?}"
        );
    }
}

#[test]
fn a_fetch_asks_with_the_exact_address_and_offers_to_allow_the_host_for_the_chat() {
    use roost_core::chat::ApprovalDecision;
    for size in SIZES {
        let (mut app, sent, s) = agent_fixture();
        let mut h = Harness::new(false);
        app.state_mut().chat.fold(ChatEvent::ApprovalRequested {
            session: s.id,
            call_id: "call_w".into(),
            name: "fetch_url".into(),
            arguments: serde_json::json!({"url": "https://Docs.rs/egui/0.37/egui/"}),
            preview: None,
            run: None,
        });
        run(&mut app, &mut h, size, 4);
        let texts = texts_of(&mut app, &mut h, size);
        for want in [
            "Fetch a web page?",
            "https://Docs.rs/egui/0.37/egui/",
            "Allow docs.rs in this chat",
        ] {
            assert!(
                texts.contains(want),
                "{size:?}: {want:?} missing in:\n{texts}"
            );
        }
        let p = app.state_mut().chat_ui.probe.clone();
        assert_eq!(p.approval_buttons.len(), 3, "{size:?}");
        let _ = sent_chat_commands(&sent);
        click(&mut app, &mut h, size, p.approval_buttons[1].center());
        let cmds = sent_chat_commands(&sent);
        assert!(
            matches!(cmds.as_slice(), [ChatCommand::ResolveApproval { decision, .. }]
                if *decision == ApprovalDecision::AllowForSession),
            "{size:?}: {cmds:?}"
        );
    }
}

// ---- an agent without a folder

fn wait_for_folder(app: &mut App, session: roost_core::session::SessionId) {
    app.state_mut().chat.fold(ChatEvent::WorkspaceNeeded {
        session,
        call_id: "call_9".into(),
        name: "read_file".into(),
        arguments: serde_json::json!({"path": "src/main.rs"}),
    });
}

#[test]
fn a_call_that_needs_a_folder_shows_a_card_with_choose_and_skip() {
    for size in SIZES {
        let (mut app, sent, s) = agent_fixture();
        let mut h = Harness::new(false);
        wait_for_folder(&mut app, s.id);
        run(&mut app, &mut h, size, 4);
        let texts = texts_of(&mut app, &mut h, size);
        assert!(
            texts.contains("No working directory is attached") && texts.contains("read_file"),
            "{size:?}: {texts}"
        );
        let p = app.state_mut().chat_ui.probe.clone();
        let composer = p.composer.unwrap();
        let pane = p.pane.unwrap();
        assert_eq!(p.folder_buttons.len(), 2, "{size:?}");
        for b in &p.folder_buttons {
            assert!(inside(*b, composer), "{size:?}: {b:?} outside {composer:?}");
            assert!(
                b.top() >= pane.bottom() - 0.5,
                "{size:?}: below the transcript"
            );
        }
        let _ = sent_chat_commands(&sent);
        click(&mut app, &mut h, size, p.folder_buttons[1].center());
        let cmds = sent_chat_commands(&sent);
        assert!(
            matches!(cmds.as_slice(), [ChatCommand::ProvideWorkspace { session, call_id, path: None }]
                if *session == s.id && call_id == "call_9"),
            "{size:?}: {cmds:?}"
        );
        run(&mut app, &mut h, size, 2);
        assert!(
            app.state_mut().chat_ui.probe.folder_buttons.is_empty(),
            "the card goes away once answered"
        );
    }
}

#[test]
fn the_folder_card_goes_when_the_tool_starts_or_the_run_ends() {
    let size = (1180.0, 820.0);
    for end in [0, 1] {
        let (mut app, _sent, s) = agent_fixture();
        let mut h = Harness::new(false);
        wait_for_folder(&mut app, s.id);
        run(&mut app, &mut h, size, 3);
        assert_eq!(app.state_mut().chat_ui.probe.folder_buttons.len(), 2);
        if end == 0 {
            app.state_mut().chat.fold(ChatEvent::ToolStarted {
                session: s.id,
                call_id: "call_9".into(),
                name: "read_file".into(),
                arguments: serde_json::json!({}),
            });
        } else {
            app.state_mut().chat.fold(ChatEvent::RunEnded {
                session: s.id,
                outcome: roost_core::chat::RunOutcome::Cancelled,
                message: None,
            });
        }
        run(&mut app, &mut h, size, 3);
        assert!(app.state_mut().chat_ui.probe.folder_buttons.is_empty());
    }
}

#[test]
fn choosing_a_folder_answers_the_waiting_call_instead_of_changing_the_folder() {
    let (mut app, _sent, s) = agent_fixture();
    let path = std::path::PathBuf::from("/tmp/project");
    let idle = super::chrome::folder_choice(&app.state_mut().chat, s.id, path.clone());
    assert!(matches!(idle, ChatCommand::SetWorkspace(id, ref p) if id == s.id && *p == path));
    wait_for_folder(&mut app, s.id);
    let waiting = super::chrome::folder_choice(&app.state_mut().chat, s.id, path.clone());
    assert!(
        matches!(waiting, ChatCommand::ProvideWorkspace { session, ref call_id, path: Some(ref p) }
            if session == s.id && call_id == "call_9" && *p == path),
        "{waiting:?}"
    );
}

#[test]
fn an_agent_without_a_folder_shows_a_no_folder_pill() {
    for size in SIZES {
        let (mut app, _sent, mut s) = agent_fixture();
        s.workspace = None;
        app.state_mut().chat.fold(ChatEvent::Opened(Box::new(s)));
        let mut h = Harness::new(false);
        run(&mut app, &mut h, size, 4);
        let p = app.state_mut().chat_ui.probe.clone();
        let (pill, composer) = (
            p.folder_pill.expect("a pill even with no folder"),
            p.composer.unwrap(),
        );
        assert!(
            inside(pill, composer),
            "{size:?}: {pill:?} outside {composer:?}"
        );
    }
}

#[test]
fn the_model_popup_of_a_session_without_a_model_marks_the_tool_models() {
    let size = (1180.0, 820.0);
    let (mut app, sent, mut s) = agent_fixture();
    s.model = String::new();
    {
        let st = app.state_mut();
        let names = ["llama3:8b", "mistral:7b", "qwen:7b"];
        st.installed.on_installed_loaded(&Ok(names
            .iter()
            .map(|n| crate::runtime::InstalledEntry {
                model: roost_core::ollama::InstalledModel {
                    name: (*n).into(),
                    size: 1,
                    params: None,
                    quant: None,
                    family: None,
                    modified: String::new(),
                    digest: String::new(),
                },
                source_repo: None,
            })
            .collect()));
        let mut r = row_for(&s, "Config walkthrough");
        r.mode = SessionMode::Agent;
        st.chat.fold(ChatEvent::Rows(vec![r]));
        st.chat.fold(ChatEvent::Opened(Box::new(s)));
        st.chat.fold(ChatEvent::ModelCapabilities {
            model: "qwen:7b".into(),
            tools: true,
        });
        st.chat.fold(ChatEvent::ModelCapabilities {
            model: "llama3:8b".into(),
            tools: false,
        });
    }
    let mut h = Harness::new(false);
    run(&mut app, &mut h, size, 4);
    let chip = app
        .state_mut()
        .chat_ui
        .probe
        .model_chip
        .expect("model chip");
    assert!(texts_of(&mut app, &mut h, size).contains("Choose a model"));
    let _ = sent_chat_commands(&sent);
    click(&mut app, &mut h, size, chip.center());
    run(&mut app, &mut h, size, 3);
    let texts = texts_of(&mut app, &mut h, size);
    assert!(texts.lines().any(|l| l.trim() == "tools"), "{texts}");
    assert!(
        texts.contains("no tools") && texts.contains("checking"),
        "{texts}"
    );
    assert!(
        texts.find("qwen:7b") < texts.find("llama3:8b"),
        "tool models first: {texts}"
    );
    let cmds = sent_chat_commands(&sent);
    assert!(
        cmds.iter()
            .any(|c| matches!(c, ChatCommand::CheckModel(m) if m == "mistral:7b")),
        "the unknown model is checked: {cmds:?}"
    );
    assert!(
        !cmds
            .iter()
            .any(|c| matches!(c, ChatCommand::CheckModel(m) if m == "qwen:7b" || m == "llama3:8b")),
        "known models are not asked again: {cmds:?}"
    );
}

fn session_with_model(
    model: &str,
) -> (
    App,
    Receiver<BackendRequest>,
    roost_core::session::SessionId,
) {
    let (mut app, sent, mut s) = agent_fixture();
    s.model = model.into();
    let id = s.id;
    let mut r = row_for(&s, "Config walkthrough");
    r.mode = SessionMode::Agent;
    let st = app.state_mut();
    st.chat.fold(ChatEvent::Rows(vec![r]));
    st.chat.fold(ChatEvent::Opened(Box::new(s)));
    (app, sent, id)
}

#[test]
fn the_model_chip_is_red_without_a_model_and_flashes_when_enter_is_pressed() {
    let size = (1180.0, 820.0);
    let (mut app, _sent, id) = session_with_model("");
    let mut h = Harness::new(false);
    run(&mut app, &mut h, size, 4);
    assert_eq!(
        app.state_mut().chat_ui.probe.model_look,
        Some((true, false)),
        "red, not flashing"
    );
    *app.state_mut().chat.draft_mut(id) = "hello".into();
    app.state_mut().chat_ui.focus_composer = true;
    run(&mut app, &mut h, size, 3);
    h.frame(vec2(size.0, size.1), press_enter(), |ui| app.show(ui));
    run(&mut app, &mut h, size, 1);
    assert!(
        app.state_mut().chat_ui.model_flash.is_some(),
        "Enter started a flash"
    );
    assert_eq!(app.state_mut().chat_ui.probe.model_look, Some((true, true)));
    app.state_mut().chat_ui.model_flash = Some(f64::NEG_INFINITY);
    run(&mut app, &mut h, size, 2);
    assert_eq!(
        app.state_mut().chat_ui.probe.model_look,
        Some((true, false))
    );
    assert!(app.state_mut().chat_ui.model_flash.is_none());
}

#[test]
fn a_session_with_a_model_has_a_plain_model_chip() {
    let size = (1180.0, 820.0);
    let (mut app, _sent, _) = session_with_model(LONG_MODEL);
    let mut h = Harness::new(false);
    run(&mut app, &mut h, size, 4);
    assert_eq!(
        app.state_mut().chat_ui.probe.model_look,
        Some((false, false))
    );
}

#[test]
fn tools_and_the_folder_wait_for_the_model_to_be_known_to_support_tools() {
    use super::chrome::ToolsGate;
    let size = (1180.0, 820.0);
    // No model: nothing to use yet.
    let (mut app, sent, _) = session_with_model("");
    let mut h = Harness::new(false);
    run(&mut app, &mut h, size, 4);
    let p = app.state_mut().chat_ui.probe.clone();
    assert_eq!(p.tools_gate, Some(ToolsGate::NoModel));
    assert!(p.folder_pill.is_none() && p.chips.is_empty(), "{p:?}");
    assert_eq!(p.tools_icon_red, Some(false));
    // A model nobody has asked about: ask, and stay locked meanwhile.
    let (mut app, sent2, _) = session_with_model("fresh:1b");
    let mut h = Harness::new(false);
    run(&mut app, &mut h, size, 4);
    let p = app.state_mut().chat_ui.probe.clone();
    assert_eq!(p.tools_gate, Some(ToolsGate::Checking));
    assert!(p.folder_pill.is_none() && p.chips.is_empty(), "{p:?}");
    let cmds = sent_chat_commands(&sent2);
    assert_eq!(
        cmds.iter()
            .filter(|c| matches!(c, ChatCommand::CheckModel(m) if m == "fresh:1b"))
            .count(),
        1,
        "{cmds:?}"
    );
    // It has no tools: chat only, and the icon says so in red.
    app.state_mut().chat.fold(ChatEvent::ModelCapabilities {
        model: "fresh:1b".into(),
        tools: false,
    });
    run(&mut app, &mut h, size, 3);
    let p = app.state_mut().chat_ui.probe.clone();
    assert_eq!(p.tools_gate, Some(ToolsGate::ChatOnly));
    assert_eq!(p.tools_icon_red, Some(true));
    assert!(p.folder_pill.is_none() && p.chips.is_empty(), "{p:?}");
    let icon = p.agent_chip.unwrap();
    click(&mut app, &mut h, size, icon.center());
    run(&mut app, &mut h, size, 3);
    let texts = texts_of(&mut app, &mut h, size);
    assert!(texts.contains("chat only"), "{texts}");
    // With tools it is all there.
    app.state_mut().chat.fold(ChatEvent::ModelCapabilities {
        model: "fresh:1b".into(),
        tools: true,
    });
    run(&mut app, &mut h, size, 3);
    let p = app.state_mut().chat_ui.probe.clone();
    assert_eq!(p.tools_gate, Some(ToolsGate::Ready));
    assert_eq!(p.tools_icon_red, Some(false));
    assert!(p.folder_pill.is_some() && !p.chips.is_empty(), "{p:?}");
    let _ = sent;
}
